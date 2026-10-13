//! 贴图窗口（pin window）：把一块像素显示成屏幕上一个只读的、置顶的、不抢焦点
//! 的小窗口，可以拖动，双击关掉。
//!
//! 设计依据：`docs/34-pin-window-design.md`。这里只实现那份文档里「已决定」的部分，
//! 文档里标为「已推迟」的（缩放、右键菜单、阴影、按窗口截图）一律不做。
//!
//! # 为什么只有一条线程
//!
//! 所有贴图窗口共用一条宿主线程。理由不是省线程，而是**消息泵必须与窗口同线程**：
//! 拖动和双击是窗口自己收到的消息，要在收到消息的线程上同步处理，绝不能跨线程
//! 转发。既然每个窗口都需要一条能阻塞在 `GetMessageW` 上的泵，那就只留一条，
//! 让所有窗口挂在上面——多开一条线程不会让任何一条路径变短。
//!
//! # 为什么整窗提交而不是只提交脏区
//!
//! 贴图窗口是静态的：贴上去之后除非被拖动，画面不变。`UpdateLayeredWindow`
//! 每次都会把整窗换成新内容，没有局部更新的概念，所以「整窗提交」在这里不是
//! 性能取舍，而是这套 API 唯一的用法。
//!
//! # 线程亲和性
//!
//! `LayeredBuffer` 里握着 HDC 和 DIB，只能在宿主线程上碰。为了从类型上堵住误用，
//! `PinWindowHost::create_pin`（可以从任意线程调用）只把请求投进通道，真正操作
//! GDI 对象的代码全部在宿主线程内部，不对外暴露任何 GDI 句柄。

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use anyhow::{anyhow, bail, Context, Result};
use windows_sys::Win32::Foundation::{
    GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLACK_BRUSH, BLENDFUNCTION,
    CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET,
    DEFAULT_PITCH, DeleteDC, DeleteObject, DIB_RGB_COLORS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE,
    DT_TOP, DrawTextW, FF_DONTCARE, FW_SEMIBOLD, FillRect, GetDC, GetStockObject, HBITMAP, HBRUSH,
    HDC, HGDIOBJ, OUT_TT_PRECIS, ReleaseDC, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
    WHITE_BRUSH,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Performance::{
    QueryPerformanceCounter, QueryPerformanceFrequency,
};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MOD_NOREPEAT, RegisterHotKey, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE,
    TRACKMOUSEEVENT, UnregisterHotKey, VK_F6,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW,
    GetSystemMetrics, GetWindowLongPtrW, GetWindowRect, IsWindow,
    LoadCursorW, PostThreadMessageW, RegisterClassW, SetWindowLongPtrW, SetWindowPos,
    TranslateMessage, UnregisterClassW, UpdateLayeredWindow, CREATESTRUCTW, CS_DBLCLKS, HTCLIENT,
    HWND_TOPMOST, IDC_ARROW, MSG, SM_CXDRAG, SM_CYDRAG, SWP_NOACTIVATE, SWP_NOCOPYBITS,
    SWP_NOMOVE, SWP_NOREDRAW, SWP_NOSIZE, ULW_ALPHA, WM_APP, WM_HOTKEY, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCREATE,
    WM_NCDESTROY, WM_NCHITTEST, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;

use super::source::ImageSource;
use super::{PinHome, PinSelection};

/// `WM_APP` 之后的自定义消息：宿主线程用它来唤醒消息泵，去处理通道里的新命令。
///
/// `GetMessageW` 会阻塞，所以往通道里投命令之后必须再发一下这个消息，否则命令
/// 要等到窗口下一次收到用户输入才会被看见。
const WM_APP_PIN_WAKE: u32 = WM_APP + 0x51;

/// 窗口类名。用了它就要在退出前注销。
const PIN_WINDOW_CLASS: &str = "SnapClipPinWindow";

/// `F6` 的 `RegisterHotKey` 编号。
///
/// 注册在宿主线程上，所以只要与同一个线程上的编号不同即可；真正让它不会撞车的
/// 是这套编号自己的一段（`0x5347`，紧跟捕获那边的 `0x5343`–`0x5346`），
/// 而不是「别人不会用这个数」的假设。
const PIN_HOTKEY_ID: i32 = 0x5347;

/// 贴图窗口的边框宽度（DIP）。这条边不是为了好看，是为了让贴图与它下面那个
/// 同尺寸的窗口在视觉上分开——没有它，「贴上了」和「没贴上」看起来一样。
const BORDER_DIP: i32 = 1;

/// 边框颜色，BGR + 不透明 alpha。深灰在中浅色截图上都看得见。
const BORDER_COLOR: u32 = 0xFF40_4040;

/// 缩放范围，两端都是用户定下的（`docs/34` §4.2.1）。
///
/// 下限 `0.1`：缩到极小仍然看得出贴的是哪块画面（放大镜式的全局观感）。
/// 上限 `10.0`：注意它与**捕获侧放大镜**的 `ZOOM_MAX = 40.0` 不同——放大镜在看一个
/// 像素，贴图在看一整块画面，10 倍已经把它放大到超出任何显示器。
///
/// 步长是 `0.1`，所以这 100 档正好是 `0.1`–`10.0`，`100%` 落在正中间
/// （见 [`zoom_step`]）。
const ZOOM_MIN: f64 = 0.1;
const ZOOM_MAX: f64 = 10.0;

/// 滚轮一"档"的标准增量。`WM_MOUSEWHEEL` 的 `zDelta` 是它的倍数，飞轮鼠标会发更小
/// 的值，所以要累加到达标才动一档（这是 `WHEEL_DELTA` 存在的全部理由）。
const WHEEL_DELTA: i32 = 120;

/// 每档缩放都打一行日志。
///
/// 留着它是为了能回答一个光看代码答不了的问题：**滚轮消息到底有没有送到贴图窗口**。
/// 它是靠捕获换来的，而捕获能不能拿到取决于窗口样式和系统的路由规则，值得用一次
/// 实跑确认，而不是假设。确认完之后这一行为 `false`（拖动那条路的账本同理，
/// 见 [`DRAG_PROFILE`]）。
const ZOOM_TRACE: bool = true;

/// `WM_MOUSEWHEEL` 的 `lParam` 里两个坐标的提取。
///
/// 不能先做整数右移再靠 `as i16` 变回负数：把 64 位的值右移 16 位之后，高字里那一位
/// 符号早就落在了 `0xFFFF` 掩码之外，`as i16` 拿到的是零。掩码取到低 16 位之后再
/// 从 `u16` 转 `i16`，负坐标才是负的——多显示器下副屏在主屏左边时，这个点必须是负数。
fn wheel_x(lparam: LPARAM) -> i32 {
    (lparam & 0xFFFF) as u16 as i16 as i32
}

fn wheel_y(lparam: LPARAM) -> i32 {
    ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32
}

/// `WM_MOUSEWHEEL` 的 `wParam` 高字，已符号扩展。
fn wheel_delta(wparam: WPARAM) -> i32 {
    ((wparam >> 16) & 0xFFFF) as u16 as i16 as i32
}

/// 「贴图没了」这条通道的容量。
///
/// 见 `PinWindowHost::spawn` 里为什么它有界。
const EVENT_QUEUE_CAPACITY: usize = 64;

/// 从 `WNDCLASSW` 创建窗口时传给 `WndProc` 的初始化参数，经 `CreateWindowExW`
/// 的 `lpCreateParams` 过去，在 `WM_NCCREATE` 里取回。放在堆上，保证在
/// `CreateWindowExW` 返回之前地址稳定。
struct WindowSeed {
    id: u64,
    image: ImageSource,
    position: (i32, i32),
    size: (i32, i32),
    /// 指向宿主线程 `HostState` 的裸指针。宿主线程在整个消息循环期间都持有
    /// 它，窗口的存活期严格短于宿主线程，所以这个指针在窗口的整个生命周期内
    /// 都有效。
    host: *mut HostState,
}

/// 一次贴图请求：一块**已经准备好**的像素，以及它要落在屏幕上的位置。
///
/// 像素必须由调用方备好。这台宿主线程不回读屏幕，也不解析窗口——它上面运行的是
/// 用户按下 F6 那一刻的窗口消息，而那一刻覆盖层可能还盖着整块显示器。图从哪来是
/// [`PinHome`](super::PinHome) 的事，不是这里的。
pub struct PinRequest {
    /// 由调用方发的编号，见 `PinManager::take_id`。
    pub id: u64,
    /// 要显示的画面。
    pub image: ImageSource,
    /// 上屏位置（屏幕坐标），指的是**含边框**的窗口左上角。
    pub position: (i32, i32),
}

/// 宿主线程发回来的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinEvent {
    /// 一个贴图窗口消失了（双击关闭、宿主销毁、或创建后立刻失败）。
    /// 调用方据此回收自己那边的记录。
    Closed { id: u64 },
}

/// 一个已经上屏（或正在上屏）的贴图。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PinHandle {
    pub id: u64,
}

/// 宿主线程的句柄。可以在任意线程上创建和销毁贴图。
///
/// 克隆是廉价的：克隆出来的东西共享同一条宿主线程和同一份消息通道。
#[derive(Clone)]
pub struct PinWindowHost {
    inner: Arc<HostInner>,
}

struct HostInner {
    commands: Sender<HostCommand>,
    control: Sender<ControlMessage>,
    events: Receiver<PinEvent>,
    /// 宿主线程是否还活着。线程退出时置 false。
    alive: AtomicBool,
    /// 宿主线程 id。线程退出后会被系统复用，但只要 `alive` 还是 true，
    /// 它就是我们要唤醒的那条线程。
    thread_id: u32,
    thread: parking_lot::Mutex<Option<JoinHandle<()>>>,
}

enum HostCommand {
    Create(Box<PinRequest>),
    Close(PinHandle),
}

enum ControlMessage {
    Shutdown,
}

impl PinWindowHost {
    /// 起一条宿主线程，并在返回之前确认它已经能收命令。
    ///
    /// 这里刻意**不**做「注册窗口类」之外的任何初始化：窗口在收到第一条命令时
    /// 才创建。没贴图的时候，这条线程只是一条阻塞在 `GetMessageW` 上的空泵。
    pub fn spawn(home: Arc<dyn PinHome>) -> Result<Self> {
        let (command_tx, command_rx) = mpsc::channel::<HostCommand>();
        let (control_tx, control_rx) = mpsc::channel::<ControlMessage>();
        // 有界：这条通道只会因为「创建失败」而收到东西（正常关闭是调用方自己要求
        // 的，它本来就知道），所以它绝不该积压。有界是防止它变成一条无界队列——
        // 若哪天有人在这里加了一条每窗口都发的消息，满队列会立刻暴露这个错误，
        // 而不是悄悄吃掉几百 KB。
        let (event_tx, event_rx) = mpsc::sync_channel::<PinEvent>(EVENT_QUEUE_CAPACITY);
        let (ready_tx, ready_rx) = mpsc::channel::<Result<HostReady, String>>();

        let thread = thread::Builder::new()
            .name("snapclip-pin".to_string())
            .spawn(move || {
                if let Err(err) = run_host(&ready_tx, control_rx, command_rx, event_tx, home) {
                    // 线程跑起来之后才失败，已经没有调用方可以回报了，只能记日志。
                    eprintln!("[snapclip-app][pin] 贴图宿主线程异常退出：{err:#}");
                }
            })
            .context("创建贴图宿主线程失败")?;

        match ready_rx.recv() {
            Ok(Ok(ready)) => Ok(Self {
                inner: Arc::new(HostInner {
                    commands: command_tx,
                    control: control_tx,
                    events: event_rx,
                    alive: AtomicBool::new(true),
                    thread_id: ready.thread_id,
                    thread: parking_lot::Mutex::new(Some(thread)),
                }),
            }),
            Ok(Err(message)) => {
                let _ = thread.join();
                bail!("贴图宿主线程初始化失败：{message}");
            }
            Err(_) => {
                let _ = thread.join();
                bail!("贴图宿主线程在报告初始化结果之前就退出了");
            }
        }
    }

    /// 把一次贴图请求投给宿主线程。
    ///
    /// 返回即代表命令已经交给宿主线程，不代表窗口已经上屏——上屏结果通过
    /// `try_recv_event` 的 `PinEvent::Closed` 反向汇报。
    ///
    /// 句柄不回传：调用方自己发号（见 [`PinRequest::id`]），所以它在投出请求的
    /// 那一刻就已经知道句柄了，不需要为一次跨线程往返等在这个调用里。这也是
    /// 句柄必须是同步返回的原因——阻塞调用方的消息循环去换一个编号是不必要的。
    pub fn dispatch(&self, request: PinRequest) -> Result<PinHandle> {
        let handle = PinHandle { id: request.id };
        self.send(HostCommand::Create(Box::new(request)))?;
        Ok(handle)
    }

    /// 主动关掉一个贴图。不存在的 id 会被忽略。
    pub fn close_pin(&self, handle: PinHandle) -> Result<()> {
        self.send(HostCommand::Close(handle))
    }

    /// 取一条宿主线程发回来的事件，没有就返回 `None`。
    ///
    /// 非阻塞轮询。贴图数量很少，调用方在自己的消息循环里顺手取一下即可，
    /// 不值得为它专门开一条等待线程。
    pub fn try_recv_event(&self) -> Option<PinEvent> {
        self.inner.events.try_recv().ok()
    }

    fn send(&self, command: HostCommand) -> Result<()> {
        if !self.inner.alive.load(Ordering::Acquire) {
            bail!("贴图宿主线程已经不在了");
        }
        self.inner
            .commands
            .send(command)
            .map_err(|_| anyhow!("贴图宿主线程已经不在了"))?;
        // 唤醒阻塞在 GetMessageW 上的消息泵。
        unsafe {
            PostThreadMessageW(self.inner.thread_id, WM_APP_PIN_WAKE, 0, 0);
        }
        Ok(())
    }
}

impl Drop for HostInner {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        let _ = self.control.send(ControlMessage::Shutdown);
        unsafe {
            PostThreadMessageW(self.thread_id, WM_APP_PIN_WAKE, 0, 0);
        }
        if let Some(thread) = self.thread.lock().take() {
            // 宿主线程会先销毁所有贴图窗口再退出，所以这里等得到。
            let _ = thread.join();
        }
    }
}

struct HostReady {
    thread_id: u32,
}

/// 宿主线程内部状态。
///
/// 这些字段只被宿主线程读写，所以不需要锁。
struct HostState {
    class_name: Vec<u16>,
    instance: HINSTANCE,
    windows: Vec<PinWindowState>,
    /// 往调用方汇报「某个贴图没了」的通道。
    events: SyncSender<PinEvent>,
    /// 下一个贴图编号，**只给 F6 那条路径用**。
    ///
    /// F6 是窗口消息，直接落在宿主线程上，没有调用方站在旁边发号。调用方路径
    /// 的编号在 [`PinManager::take_id`](super::PinManager::take_id) 上发。各自从
    /// 1 开始是有意的：编号不跨路径，就没有需要解释的洞。
    next_id: u64,
    /// F6 的取图来源。
    ///
    /// 宿主线程在收到 `WM_HOTKEY` 时现取现贴：图必须和「按下 F6 的那一刻」同时
    /// 成立，缓存一份在宿主线程上只会让它变旧。
    home: Arc<dyn PinHome>,
}

impl HostState {
    fn position_of(&self, hwnd: HWND) -> Option<usize> {
        self.windows.iter().position(|w| w.hwnd == hwnd)
    }

    /// 取下一个贴图编号。
    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// 上报「这个贴图没了」。
    ///
    /// 队列满了不是静默失败：那意味着有东西在往这条通道里塞不属于它的消息。
    fn report_closed(&mut self, id: u64) {
        if self.events.try_send(PinEvent::Closed { id }).is_err() {
            eprintln!(
                "[snapclip-app][pin] 「贴图已关闭」队列已满，丢弃 id={id}；\
                 这条通道只应承载创建失败，不该积压"
            );
        }
    }
}

/// 一次拖动结束时把账本打出来。
///
/// 这一行要回答的问题只有一个：**卡顿是"消息来得稀"还是"每条处理得慢"**。
/// `moves` 与 `elapsed_ms` 一起给出消息到达率，`set_position_us`/`moved` 给出每条
/// 的平均代价——两者是哪一种，看这两个数就够了。
fn report_drag_perf(id: u64, perf: &DragPerf) {
    if perf.moves == 0 && perf.moved == 0 {
        return;
    }
    let elapsed_ms = now_us().saturating_sub(perf.started_us) / 1000;
    let average_us = if perf.moved == 0 {
        0
    } else {
        perf.set_position_us / u64::from(perf.moved)
    };
    // 到达率：每毫秒几条。小于 0.06 附近（约每 16 ms 一条）就说明消息本身就稀，
    // 瓶颈不在 SetWindowPos。
    let per_second = if elapsed_ms == 0 {
        0
    } else {
        u64::from(perf.moves) * 1000 / elapsed_ms
    };
    eprintln!(
        "[snapclip-app][pin] drag id={id} moves={} moved={} elapsed_ms={elapsed_ms} \
         move_rate_per_s={per_second} set_position_us_avg={average_us} \
         set_position_us_max={} set_position_us_total={}",
        perf.moves, perf.moved, perf.set_position_max_us, perf.set_position_us
    );
}

/// 一个贴图窗口的宿主侧状态。
struct PinWindowState {
    id: u64,
    hwnd: HWND,
    /// 整窗内容的 DIB 后备缓冲。尺寸随缩放变化，所以每次改缩放都要重建（见
    /// [`resize_framebuffer`]）。
    ///
    /// 提交之后不再被读——`UpdateLayeredWindow` 已经把它拷进了窗口。它的作用是
    /// **持有**：DIB 和内存 DC 的生存期要跟着窗口走，窗口没了才 `Drop` 释放。
    framebuffer: LayeredBuffer,
    /// 原图像素。缩放**永远从这一份重采样**，不从上一次的缩放结果再缩放。
    ///
    /// 这一点是缩放质量的根：每次重采样都会丢一次信息，叠加若干次之后 8× 再回到 1×
    /// 已经不是原来那张图了（§6.1 的 T10 钉的就是这条）。
    image: Arc<Vec<u32>>,
    image_width: u32,
    image_height: u32,
    /// 当前缩放倍数，钳在 `[ZOOM_MIN, ZOOM_MAX]`。
    ///
    /// 存的是它，不是"当前像素尺寸"：窗口尺寸由它和一个纯函数[算出来](scaled_dimension)，
    /// 于是窗口尺寸、绘制内容、坐标换算三者不可能各说一套。
    zoom: f64,
    /// 尚未凑够一档的滚轮增量。
    ///
    /// `WM_MOUSEWHEEL` 的 `zDelta` 是 120 的倍数，但高精度滚轮会发更小的值，累加到达标
    /// 才动一档是 `WHEEL_DELTA` 的规定用法。留在窗口状态里而不在消息里就地处理，
    /// 是因为"差一点"这件事要跨消息记账。
    wheel_remainder: i32,
    /// 是否已经向系统登记过"光标离开要通知我"。
    ///
    /// `TrackMouseEvent` 只对**下一次**离开有效，所以每次拿到 `WM_MOUSELEAVE` 就要重新登记。
    /// 这个标志同时兼作"光标现在在窗口里"：它是按下 `SetCapture` 的前提（见 `on_message`
    /// 里 `WM_MOUSEMOVE` 那段）。
    tracking_leave: bool,
    /// 窗口左上角位置（屏幕坐标，含边框）。
    position: (i32, i32),
    /// 按下左键那一刻的光标位置；`None` 表示没有正在进行的拖动。
    drag_cursor: Option<(i32, i32)>,
    /// 本次拖动开始时窗口在哪里。
    ///
    /// 拖动位置由**这一个锚点**算出：`窗口 = 锚点 + (当前光标 − 按下时光标)`。这正是
    /// 问题的反面——早先写成"窗口 = 窗口 + 光标增量"，每收到一条 `WM_MOUSEMOVE` 就把
    /// 上一次的结果再加一遍增量，窗口于是越跑越偏、最后完全跟不上鼠标。相对量不能
    /// 累加在会被自己改动的基准上；从固定锚点算绝对位置，丢消息、消息合并、光标被
    /// 屏幕边缘夹住都不影响正确性。
    drag_anchor: (i32, i32),
    /// 本次拖动是否已经越过系统阈值。越过之前不移动窗口，这样单击不会误挪。
    drag_moved: bool,
    /// 本次拖动的耗时账。只在 `DRAG_PROFILE` 为真时被填。
    drag_perf: DragPerf,
}

/// 拖动路径的耗时账本。
///
/// 存在的理由：拖动卡顿有**两个完全不同的原因**——消息到得太稀（每个 `WM_MOUSEMOVE`
/// 间隔大），和消息到得够密但每条处理太慢（`SetWindowPos` 慢）。两者的修法相反，
/// 所以必须先知道是哪一种，不能靠猜。
#[derive(Default)]
struct DragPerf {
    /// 本次拖动收到的 `WM_MOUSEMOVE` 条数。
    moves: u32,
    /// 其中真正执行了 `SetWindowPos` 的条数（越过阈值之后的）。
    moved: u32,
    /// `SetWindowPos` 的累计耗时（微秒）。
    set_position_us: u64,
    /// 单次 `SetWindowPos` 的最大耗时（微秒）。
    set_position_max_us: u64,
    /// 拖动开始时刻（微秒，来自高精度计数器）。
    started_us: u64,
}

/// 关掉它就不再记账，用于确认账本自身没有拖慢拖动。
const DRAG_PROFILE: bool = true;

/// 高精度时间戳（微秒）。计数器取不到时返回 0，账本会退化成"耗时为 0"，
/// 而不是报错——它只是诊断设施，不该影响拖动本身。
fn now_us() -> u64 {
    unsafe {
        let mut frequency = 0i64;
        if QueryPerformanceFrequency(&mut frequency) == 0 || frequency == 0 {
            return 0;
        }
        let mut counter = 0i64;
        if QueryPerformanceCounter(&mut counter) == 0 {
            return 0;
        }
        (counter as u128 * 1_000_000 / frequency as u128) as u64
    }
}

/// 宿主线程主体：注册窗口类、报 ready、处理命令、跑消息泵、注销窗口类。
fn run_host(
    ready: &Sender<Result<HostReady, String>>,
    control: Receiver<ControlMessage>,
    commands: Receiver<HostCommand>,
    events: SyncSender<PinEvent>,
    home: Arc<dyn PinHome>,
) -> Result<()> {
    let class_name: Vec<u16> = PIN_WINDOW_CLASS
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    if instance.is_null() {
        let message = format!("取模块句柄失败，错误码 {}", last_error());
        let _ = ready.send(Err(message.clone()));
        bail!(message);
    }
    if let Err(err) = register_window_class(instance, &class_name) {
        let _ = ready.send(Err(format!("{err:#}")));
        return Err(err);
    }
    let thread_id = unsafe { GetCurrentThreadId() };

    // F6 注册在这条线程上：`WM_HOTKEY` 只会投到注册它的线程队列，而贴图的
    // 屏幕回读与窗口创建都要在这条线程上按顺序发生（见 `PinRequest::WindowAtPoint`）。
    //
    // `RegisterHotKey` 传的是空窗口，于是 F6 变成一条**线程消息**（`msg.hwnd` 为
    // null），这条线程的消息泵必须自己把它交给处理函数——`DispatchMessageW` 对
    // 没有窗口的消息是静默丢弃的。这一点踩过：见 `unsafe extern "system" fn wnd_proc`
    // 上面那一段。
    //
    // 失败**不致命**：贴图能力本身（宿主线程、窗口、`PinManager` 的调用方路径）
    // 都还在，只是热键这条入口没了。热键被别的程序占着是可能的，所以这里只报告，
    // 不让整个模块起不来。
    //
    // 代价要说清：第一版**只有 F6 这一条入口**（没有托盘菜单，见 `docs/34` §4.2），
    // 所以注册失败就等于用户按不动任何东西。这条消息要能在日志里被一眼看到。
    if unsafe { RegisterHotKey(std::ptr::null_mut(), PIN_HOTKEY_ID, MOD_NOREPEAT, VK_F6 as u32) } == 0
    {
        eprintln!(
            "[snapclip-app][pin] 注册 F6 失败（错误码 {}）：贴图热键不可用，其余功能照常",
            last_error()
        );
    } else {
        // 成功的路径也要留一行：F5/F7/F9 都会打印注册结果，贴图这条不打印的话，
        // 「F6 按下去没反应」就只能靠猜是没注册上还是没收到。
        eprintln!("[snapclip-app][pin] 注册全局热键 F6 id={PIN_HOTKEY_ID}");
    }

    let mut state = HostState {
        class_name,
        instance,
        windows: Vec::new(),
        events,
        next_id: 1,
        home,
    };

    if ready.send(Ok(HostReady { thread_id })).is_err() {
        // 调用方已经放弃（`spawn` 提前返回），不能留一个没人注销的窗口类。
        unregister_window_class(state.instance, &state.class_name);
        return Ok(());
    }

    let mut pending: VecDeque<HostCommand> = VecDeque::new();
    loop {
        // 控制消息（退出）优先，然后一次性把命令搬空。
        if control.try_recv().is_ok() {
            break;
        }
        pending.extend(commands.try_iter());
        while let Some(command) = pending.pop_front() {
            match command {
                HostCommand::Create(request) => {
                    let id = request.id;
                    if let Err(err) = create_pin_window(&mut state, id, request.image, request.position)
                    {
                        eprintln!("[snapclip-app][pin] 创建贴图窗口失败：{err:#}");
                        // 创建失败也报一条关闭：调用方是按 id 记账的，
                        // 不报就等于它永远等着一张不存在的贴图。
                        state.report_closed(id);
                    }
                }
                HostCommand::Close(handle) => {
                    if let Some(window) = state.windows.iter().find(|w| w.id == handle.id) {
                        let hwnd = window.hwnd;
                        if unsafe { IsWindow(hwnd) } != 0 {
                            // 销毁是同步的：`WM_DESTROY`/`WM_NCDESTROY` 在这一行里跑完，
                            // 窗口状态也随之从 `state.windows` 里摘掉。
                            unsafe { DestroyWindow(hwnd) };
                        }
                    }
                }
            }
        }

        // 窗口全没了也不退出：F6 还注册在这条线程上，贴图可以再来一张。
        // 这条线程与进程同寿，退出只由 `ControlMessage::Shutdown` 决定。

        // 消息泵。窗口消息派发到 `wnd_proc`；**没有窗口的消息**必须自己处理。
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        let ret = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if ret <= 0 {
            if ret < 0 {
                eprintln!(
                    "[snapclip-app][pin] 贴图宿主线程 GetMessageW 失败：{}",
                    last_error()
                );
            }
            break;
        }
        match route_message(msg.hwnd, msg.message) {
            MessageRoute::HandleHere => {
                // `F6` 是用 `RegisterHotKey(null, …)` 注册的，于是 `WM_HOTKEY` 是一条
                // **线程消息**：`msg.hwnd` 为 null，`DispatchMessageW` 对它是静默丢弃
                // （没有窗口可以派发），窗口过程永远不会被调用。这里自己把它交给
                // 处理函数，与 `snapclip-capture` 的覆盖层消息泵同一个做法。
                //
                // 这条分支踩过一次：F6 注册成功、日志干净、按下去毫无反应，就是因为
                // 消息在这里被丢掉了。
                on_message(
                    &mut state,
                    std::ptr::null_mut(),
                    msg.message,
                    msg.wParam,
                    msg.lParam,
                );
            }
            MessageRoute::Dispatch => unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            },
        }
    }

    // 收摊：销毁残留的贴图窗口，退掉热键，注销窗口类。
    let hwnds: Vec<HWND> = state.windows.iter().map(|w| w.hwnd).collect();
    for hwnd in hwnds {
        if unsafe { IsWindow(hwnd) } != 0 {
            unsafe { DestroyWindow(hwnd) };
        }
    }
    unsafe { UnregisterHotKey(std::ptr::null_mut(), PIN_HOTKEY_ID) };
    unregister_window_class(state.instance, &state.class_name);
    Ok(())
}

/// 一条取到的消息该由谁处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageRoute {
    /// 没有窗口的消息：`DispatchMessageW` 会静默丢弃，必须交给 `on_message`。
    HandleHere,
    /// 有窗口的消息：照常 `TranslateMessage` + `DispatchMessageW`。
    Dispatch,
}

/// 消息泵的路由判定。
///
/// 抽成独立函数是为了能被单测盯住——`F6` 曾经因为这一条判断缺失而完全无声：
/// `RegisterHotKey(null, …)` 让 `WM_HOTKEY` 成为线程消息，`hwnd` 为 null，
/// 而 `DispatchMessageW` 对没有窗口的消息什么也不做。
fn route_message(hwnd: HWND, _message: u32) -> MessageRoute {
    if hwnd.is_null() {
        MessageRoute::HandleHere
    } else {
        MessageRoute::Dispatch
    }
}

/// `F6` 被按下：把刚截的那张图贴出来。
///
/// `host` 由消息泵保证非空（它就是消息泵自己手里的那个状态）。
fn on_pin_hotkey(host: *mut HostState) {
    // 每一个 F6 都先留一行，再决定做什么：`RegisterHotKey` 成功但消息没到，
    // 和消息到了却还没有可贴的图，是两种完全不同的故障，日志必须能分开它们。
    eprintln!("[snapclip-app][pin] WM_HOTKEY F6 received");
    let state = unsafe { &mut *host };
    let started = now_us();
    // 先取图再发号：没有图的时候什么都不发生，编号表里不该留下一个洞。
    let Some(selection) = state.home.take_selection() else {
        eprintln!("[snapclip-app][pin] F6 还没有可贴的图：先按 F5 截一张");
        return;
    };
    let took_us = now_us().saturating_sub(started);
    let id = state.take_id();
    let converted = now_us();
    if let Err(err) = pin_home_selection(state, id, selection) {
        eprintln!("[snapclip-app][pin] F6 贴图失败：{err:#}");
        state.report_closed(id);
    }
    let total_us = now_us().saturating_sub(started);
    let convert_us = converted.saturating_sub(started).saturating_sub(took_us);
    eprintln!(
        "[snapclip-app][pin] create stage=hotkey_to_window total_us={total_us} \
         take_us={took_us} convert_and_window_us={convert_us}"
    );
}

/// 把 [`PinHome`] 交出来的一块选区贴成窗口。
///
/// 位置由选区自己决定：它带着屏幕坐标（`region`），所以贴图落在原地——「钉住」
/// 在视觉上就是这副画面留在它刚才在的地方，用户不必再把窗口拖回去。
fn pin_home_selection(state: &mut HostState, id: u64, selection: PinSelection) -> Result<()> {
    let region = selection.region;
    let width = u32::try_from(region.width()).context("选区宽度非法")?;
    let height = u32::try_from(region.height()).context("选区高度非法")?;
    if width == 0 || height == 0 {
        bail!("选区为空：({}, {}) - ({}, {})", region.left, region.top, region.right, region.bottom);
    }
    let expected = width as usize * height as usize * 4;
    if selection.bytes().len() != expected {
        bail!(
            "选区像素 {} 字节，与 {width}x{height} 不符",
            selection.bytes().len()
        );
    }
    let started = now_us();
    let image = ImageSource::from_bgra_bytes(selection.bytes(), width, height)?;
    let convert_us = now_us().saturating_sub(started);
    let started = now_us();
    create_pin_window(state, id, image, (region.left, region.top))?;
    let window_us = now_us().saturating_sub(started);
    // 拆开是为了定位瓶颈：`convert` 是字节→`u32` 的重解释与分配，`window` 是
    // DIB 建立、绘制、`UpdateLayeredWindow` 提交、`ShowWindow`。
    eprintln!(
        "[snapclip-app][pin] create stage=decode_and_window size={width}x{height} \
         px={} convert_us={convert_us} window_us={window_us}",
        width as usize * height as usize
    );
    Ok(())
}

/// 创建并上屏一个贴图窗口。
fn create_pin_window(
    state: &mut HostState,
    id: u64,
    image: ImageSource,
    position: (i32, i32),
) -> Result<()> {
    let border = border_px();
    let width = image.width() as i32 + border * 2;
    let height = image.height() as i32 + border * 2;
    if width <= 0 || height <= 0 {
        bail!("贴图尺寸非法：{width}x{height}");
    }

    let mut seed = WindowSeed {
        id,
        image,
        position,
        size: (width, height),
        host: state as *mut HostState,
    };

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            state.class_name.as_ptr(),
            std::ptr::null(),
            // `WS_VISIBLE` 必须在创建时就带上。`WS_POPUP` 只决定形状，不含可见性；
            // 少了这一位，窗口会以"已存在但从不显示"的状态活下来——`CreateWindowExW`
            // 成功、`UpdateLayeredWindow` 成功、鼠标消息照收，屏幕上什么都没有。
            // 捕获侧的覆盖层不是这样：它是 `WS_POPUP` 配 `SetWindowPos(…, SWP_SHOWWINDOW)`。
            WS_POPUP | WS_VISIBLE,
            position.0,
            position.1,
            width,
            height,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            state.instance,
            (&mut seed as *mut WindowSeed).cast::<c_void>(),
        )
    };
    if hwnd.is_null() {
        bail!("CreateWindowExW 失败，错误码 {}", last_error());
    }

    // 创建成功的这一行必须留下。上一次故障（`WS_VISIBLE` 漏掉）的全部表现就是
    // 这一行及其之前的日志一切正常、屏幕上什么都没有——没有这行，日志里
    // "窗口建没建起来"这个问题就答不了。
    eprintln!(
        "[snapclip-app][pin] created id={id} size={width}x{height} at=({}, {})",
        position.0, position.1
    );

    // `wnd_proc` 在 `WM_NCCREATE` 里已经把窗口状态推进 `state.windows`。
    // 没推进去说明初始化中途失败（`WM_NCCREATE` 返回 0 会让创建失败）。
    if state.position_of(hwnd).is_none() {
        unsafe { DestroyWindow(hwnd) };
        bail!("贴图窗口在初始化期间失败");
    }

    // 显式置顶。`WS_EX_TOPMOST` 只决定新窗口在置顶组里的相对次序，
    // 提这一次才保证它压在已经存在的置顶窗口上面。
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE,
        );
    }
    Ok(())
}

/// 窗口消息处理。
///
/// # Safety
///
/// 作为 `WNDCLASSW::lpfnWndProc` 由系统调用，只能挂在我们自己注册的窗口类上。
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            return on_wm_nccreate(hwnd, lparam);
        }
        let host = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut HostState;
        if host.is_null() {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }
        on_message(host, hwnd, message, wparam, lparam)
    }
}

/// 除 `WM_NCCREATE` 之外的全部消息。
///
/// `host` 已经确认非空：它是 `WM_NCCREATE` 存进 `GWLP_USERDATA` 的那个指针，
/// 只在窗口存续期间有效，而窗口的存活期严格短于宿主线程。
fn on_message(
    host: *mut HostState,
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCDESTROY => {
            on_wm_ncdestroy(host, hwnd);
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_HOTKEY => {
            // 空窗口的 `WM_HOTKEY` 是一条线程消息，由消息泵直接送到这里
            // （`hwnd` 为 null；原因见消息泵那段注释）。有窗口的那种不会发生：
            // 本模块只注册过这一条热键，而且是按空窗口注册的。
            on_pin_hotkey(host);
            0
        }
        WM_NCHITTEST => {
            // 贴图窗口**参与**命中测试，落在它上面的鼠标由它自己处理。
            //
            // 早先想让贴图对鼠标透明（返回 `HTTRANSPARENT`），但那样窗口就再也
            // 收不到任何鼠标消息——拖动和双击都是鼠标消息，会一起失效。取消贴图
            // 是贴图唯一的撤销动作，它必须能手到擒来，所以这里选择可交互。
            //
            // 「看的时候不挡操作」这件事排在后面：它需要一个外加条件（比如按住
            // 某个修饰键才穿透），`docs/34-pin-window-design.md` 里把它记为待定，
            // 而不是在这里靠一个会连累主交互的返回值含糊过去。
            HTCLIENT as LRESULT
        }
        WM_LBUTTONDOWN => {
            if let Some(window) = unsafe { window_mut(host, hwnd) } {
                let mut point = POINT { x: 0, y: 0 };
                if unsafe { GetCursorPos(&mut point) } != 0 {
                    window.drag_cursor = Some((point.x, point.y));
                    window.drag_anchor = window.position;
                    window.drag_moved = false;
                    if DRAG_PROFILE {
                        window.drag_perf = DragPerf {
                            started_us: now_us(),
                            ..DragPerf::default()
                        };
                    }
                }
            }
            0
        }
        WM_MOUSEMOVE => {
            if let Some(window) = unsafe { window_mut(host, hwnd) } {
                // 光标在窗口里。两件与拖动无关的事在这里发生，都是"悬停"这个状态的
                // 入口：
                // 1. 登记"离开时通知我"。`WM_MOUSELEAVE` 是唯一可靠的离开信号——
                //    靠比较坐标猜边界会在快速划过时漏掉。
                // 2. 按住捕获。`WM_MOUSEWHEEL` 走的是**焦点**路由，而贴图窗口是
                //    `WS_EX_NOACTIVATE`、永远不持有焦点，不捕获就一个滚轮消息都
                //    收不到（见 `WM_MOUSEWHEEL` 那段）。
                //
                // 捕获和拖动没有冲突：拖动期间光标可能被甩出窗口，捕获恰好让
                // `WM_MOUSEMOVE` 继续送到这里，拖动因此不会在中途断掉。
                if !window.tracking_leave {
                    acquire_hover(hwnd, window);
                }
                if DRAG_PROFILE && window.drag_cursor.is_some() {
                    window.drag_perf.moves += 1;
                }
                if let Some(down) = window.drag_cursor {
                    let mut point = POINT { x: 0, y: 0 };
                    if unsafe { GetCursorPos(&mut point) } != 0 {
                        let dx = point.x - down.0;
                        let dy = point.y - down.1;
                        if !window.drag_moved {
                            // 先过系统拖动阈值再挪窗口：这样单击（按下又松开）
                            // 绝不会把贴图挪偏一两个像素。
                            let threshold_x =
                                unsafe { GetSystemMetrics(SM_CXDRAG) }.max(1);
                            let threshold_y =
                                unsafe { GetSystemMetrics(SM_CYDRAG) }.max(1);
                            if dx.abs() >= threshold_x || dy.abs() >= threshold_y {
                                window.drag_moved = true;
                            }
                        }
                        if window.drag_moved {
                            // 从锚点算绝对位置，不做累加。
                            let (x, y) = drag_position(
                                window.drag_anchor,
                                down,
                                (point.x, point.y),
                            );
                            let started = if DRAG_PROFILE { now_us() } else { 0 };
                            let landed = unsafe {
                                SetWindowPos(
                                    hwnd,
                                    HWND_TOPMOST,
                                    x,
                                    y,
                                    0,
                                    0,
                                    SWP_NOSIZE | SWP_NOACTIVATE,
                                )
                            };
                            if landed != 0 {
                                // 只有窗口真的动了才记账：否则位置会记成"它想去
                                // 的地方"，下一帧的锚点计算就建在一个假事实上。
                                window.position = (x, y);
                            }
                            if DRAG_PROFILE {
                                let elapsed = now_us().saturating_sub(started);
                                window.drag_perf.moved += 1;
                                window.drag_perf.set_position_us += elapsed;
                                window.drag_perf.set_position_max_us =
                                    window.drag_perf.set_position_max_us.max(elapsed);
                            }
                        }
                    }
                }
            }
            0
        }
        WM_LBUTTONUP => {
            if let Some(window) = unsafe { window_mut(host, hwnd) } {
                let was_dragging = window.drag_cursor.is_some();
                window.drag_cursor = None;
                window.drag_moved = false;
                if DRAG_PROFILE && was_dragging {
                    report_drag_perf(window.id, &window.drag_perf);
                }
                // 拖动期间光标可能被甩到窗口外面，那条路的规则是"离开就放手"
                // （见 `WM_MOUSELEAVE`）。现在拖动结束了，如果光标其实还在窗口上，
                // 捕获要**重新拿回来**——否则回到窗口里滚轮就没有了，而用户根本
                // 没离开过贴图，这在他看来是"拖完之后缩放突然失灵"。
                if !window.tracking_leave && cursor_is_over(hwnd) {
                    acquire_hover(hwnd, window);
                }
            }
            0
        }
        WM_MOUSELEAVE => {
            if let Some(window) = unsafe { window_mut(host, hwnd) } {
                window.tracking_leave = false;
                // 拖动中不放手：捕获是拖动能在光标被甩出窗口后继续的原因。
                // 拖动结束时那条 `WM_LBUTTONUP` 会把捕获补回来（见下）。
                if window.drag_cursor.is_none() {
                    unsafe { ReleaseCapture() };
                }
            }
            0
        }
        WM_MOUSEWHEEL => {
            // 光标压在贴图上滚 = 以**光标底下那个像素**为锚点缩放。
            //
            // 为什么必须靠上一条 `WM_MOUSEMOVE` 里的 `SetCapture`：MSDN 写明
            // `WM_MOUSEWHEEL` 送给**焦点窗口**，而贴图窗口是 `WS_EX_NOACTIVATE`、
            // 从不持有焦点。没有捕获的话，用户在贴图上滚轮，消息去的是他背后那个
            // 真正在前台的窗口——贴图会一动不动，而且顺带把别人的页面滚了。捕获在
            // 光标进入窗口时建立、离开时释放，正好覆盖"鼠标放在贴图上"这个范围。
            //
            // `lParam` 是**屏幕**坐标，不是客户区坐标：缩放锚点要先减去窗口位置。
            let delta = wheel_delta(wparam);
            if delta == 0 {
                return 0;
            }
            let cursor = (wheel_x(lparam), wheel_y(lparam));
            if let Some(window) = unsafe { window_mut(host, hwnd) } {
                let (zoom, remainder, notches) =
                    wheel_zoom(window.zoom, window.wheel_remainder, delta);
                window.wheel_remainder = remainder;
                // 没凑够一档就到此为止，窗口一个像素都不动。不然每个滚轮消息都会
                // 重建一次整块缓冲并重新提交一次 `UpdateLayeredWindow`。
                if notches == 0 || (zoom - window.zoom).abs() < f64::EPSILON {
                    return 0;
                }
                let border = border_px();
                // 位置先算：它只依赖缩放前后倍数和光标，不依赖新的窗口尺寸。
                let moved = zoom_with_cursor(
                    window.zoom,
                    zoom,
                    window.position,
                    window.image_width,
                    window.image_height,
                    border,
                    cursor,
                );
                window.zoom = zoom;
                // 位置和尺寸交给同一次 `SetWindowPos`。这里**不能**拆成"先改尺寸、
                // 再挪位置"：那中间会显示一帧"新尺寸 ＋ 旧位置"的旧画面，也就是用户
                // 看到的闪烁。理由写在 `apply_zoom` 上。
                let scaled = match unsafe { apply_zoom(hwnd, window, moved) } {
                    Ok(scaled) => scaled,
                    Err(err) => {
                        eprintln!("[snapclip-app][pin] 缩放失败 id={}：{err:#}", window.id);
                        return 0;
                    }
                };
                if ZOOM_TRACE {
                    eprintln!(
                        "[snapclip-app][pin] zoom id={} zoom={zoom:.4} delta={delta} \
                         cursor=({}, {}) at=({}, {}) buffer={}x{}",
                        window.id, cursor.0, cursor.1, window.position.0, window.position.1,
                        scaled.0, scaled.1
                    );
                }
            }
            0
        }
        WM_LBUTTONDBLCLK => {
            // 双击取消贴图。这是文档里定下的主撤销动作。
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_APP_PIN_WAKE => 0,
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

/// `WM_NCCREATE`：把 `CreateWindowExW` 递过来的种子转成窗口状态。
///
/// 返回 0 会让创建失败；1 表示继续。
fn on_wm_nccreate(hwnd: HWND, lparam: LPARAM) -> LRESULT {
    unsafe {
        let create = lparam as *const CREATESTRUCTW;
        if create.is_null() {
            return 0;
        }
        let seed = (*create).lpCreateParams as *mut WindowSeed;
        if seed.is_null() {
            return 0;
        }
        let host = (*seed).host;
        if host.is_null() {
            return 0;
        }
        if let Err(err) = attach_window(hwnd, &mut *seed) {
            eprintln!("[snapclip-app][pin] 贴图窗口初始化失败：{err:#}");
            // 返回 0 让 `CreateWindowExW` 失败并回收这个窗口。
            return 0;
        }
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, host as isize);
        1
    }
}

/// `WM_NCDESTROY`：窗口没了。摘掉状态、释放 DIB、回报调用方。
///
/// 这条消息每次销毁都会到：双击关的、宿主收摊时关的、`CreateWindowExW` 在
/// 初始化中途失败时回收的，都走这里。所以「摘状态 + 报关闭」是唯一的收口，
/// 调用方不需要在别处补记。
fn on_wm_ncdestroy(host: *mut HostState, hwnd: HWND) {
    let index = unsafe { (*host).position_of(hwnd) };
    let Some(index) = index else {
        return;
    };
    // 移出状态 = 释放 DIB 与内存 DC。
    let window = unsafe { (*host).windows.remove(index) };
    let id = window.id;
    drop(window);
    // 「销毁」必须和「创建」成对留在日志里。量贴图的内存时，`created` 出现了三次而
    // 读取内存的脚本看到 GDI 对象还是一个没少，这是两种完全不同的故障——是窗口没销毁，
    // 还是销毁了但资源没还——没有这一行就只能靠猜。
    eprintln!(
        "[snapclip-app][pin] destroyed id={id} remaining={}",
        unsafe { (*host).windows.len() }
    );
    unsafe { notify_closed(host, id) };
}

/// 取某个窗口的可变状态。找不到（正在销毁）就返回 `None`。
///
/// # Safety
///
/// `host` 必须是宿主线程里那个仍然存活的 `HostState` 指针，且这次借用期间
/// 没有别人同时持有它的可变借用。调用点只有 `wnd_proc` 一条消息处理路径，
/// 满足这个前提。
unsafe fn window_mut<'a>(host: *mut HostState, hwnd: HWND) -> Option<&'a mut PinWindowState> {
    let state = unsafe { &mut *host };
    let index = state.position_of(hwnd)?;
    Some(&mut state.windows[index])
}

/// 往调用方那边发一条「这个贴图没了」。
///
/// # Safety
///
/// 同 `window_mut`。调用前必须已经把这个窗口从 `state.windows` 里摘掉，
/// 否则这里的可变借用会和调用方手上的借用打架。
unsafe fn notify_closed(host: *mut HostState, id: u64) {
    let state = unsafe { &mut *host };
    state.report_closed(id);
}

/// 在 `WM_NCCREATE` 里把窗口状态和它的 DIB 建好。
///
/// # Safety
///
/// `seed.host` 必须是宿主线程里仍然存活的 `HostState` 指针。
unsafe fn attach_window(hwnd: HWND, seed: &mut WindowSeed) -> Result<()> {
    if seed.host.is_null() {
        bail!("窗口种子里的宿主状态指针为空");
    }
    let border = border_px();
    let size = (seed.size.0, seed.size.1);
    // 创建时缩放恒为 1.0，所以窗口尺寸就是 1.0 倍那份算式的结果——和 `apply_zoom`
    // 走的是同一个 `window_size`，不会出现"创建时算一套、缩放时算另一套"。
    let image_width = seed.image.width();
    let image_height = seed.image.height();
    let scaled = window_size(image_width, image_height, 1.0, border);
    debug_assert_eq!(scaled, size);
    let mut framebuffer = LayeredBuffer::new(size.0, size.1, true)?;
    paint_pin(
        framebuffer.pixels_mut(),
        size,
        border,
        seed.image.pixels(),
        image_width,
        image_height,
        1.0,
        system_dpi(),
    );
    framebuffer
        .submit(hwnd)
        .context("首次提交贴图内容失败")?;

    // 原图像素在**第一次提交之后**才取所有权：`seed.image` 是 `ImageSource`（拥有
    // `Vec<u32>`），把它交给 `Arc` 是零拷贝，重绘因此不必再准备第二份。
    let image = Arc::new(seed.image.take_pixels());

    unsafe {
        (*seed.host).windows.push(PinWindowState {
            id: seed.id,
            hwnd,
            framebuffer,
            image,
            image_width,
            image_height,
            zoom: 1.0,
            wheel_remainder: 0,
            tracking_leave: false,
            position: seed.position,
            drag_cursor: None,
            drag_anchor: seed.position,
            drag_moved: false,
            drag_perf: DragPerf::default(),
        });
    }
    Ok(())
}

/// 进入"光标在贴图上"这个状态：登记离开通知，并按住鼠标捕获。
///
/// 两件事必须一起做，不能只做一件：
/// - 只 `SetCapture` 不登记 `TME_LEAVE`，捕获就永远收不回来，用户光标早就移开了，
///   而系统仍然把鼠标输入送给贴图——那是把别人的窗口按住了。
/// - 只登记不捕获，滚轮消息照旧送去焦点窗口（见 [`on_message`] 里 `WM_MOUSEWHEEL`
///   那段），功能等于没做。
///
/// 登记失败就当没有悬停：不设 `tracking_leave`，下一条 `WM_MOUSEMOVE` 会重试。
/// 这里**不**捕获，因为拿不到"离开"通知的捕获是无法释放的。
fn acquire_hover(hwnd: HWND, window: &mut PinWindowState) {
    let mut track = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        // `MSDN` 要求填这个字段，但只对 `TME_HOVER` 有意义。
        dwHoverTime: 0,
    };
    if unsafe { TrackMouseEvent(&mut track) } == 0 {
        eprintln!(
            "[snapclip-app][pin] TrackMouseEvent 失败 id={} 错误码 {}",
            window.id,
            last_error()
        );
        return;
    }
    window.tracking_leave = true;
    unsafe { SetCapture(hwnd) };
}

/// 光标现在是不是压在窗口的矩形里（屏幕坐标）。
///
/// 用来判断"拖动把光标甩出窗口之后，松开左键时人还在不在贴图上"——不在就不该重新
/// 按住捕获。比较的是**整窗矩形**（含边框）而不是客户区，因为边框也在窗口里，用户
/// 把光标放在边框上同样算"在贴图上"。
fn cursor_is_over(hwnd: HWND) -> bool {
    let mut cursor = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut cursor) } == 0 {
        return false;
    }
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return false;
    }
    cursor.x >= rect.left && cursor.x < rect.right && cursor.y >= rect.top && cursor.y < rect.bottom
}

/// 滚轮改缩放：把窗口内容与窗口尺寸一起换掉。
///
/// 只在缩放真的变了的时候调用。缩放没变的那些消息（拖动、单击、鼠标进出）一律不碰
/// 缓冲，拖动因此仍然是纯 `SetWindowPos`——"别把无关的活放进高频路径"是拖动不跟手
/// 那条故障的反向教训。
///
/// # Safety
///
/// 同 [`window_mut`]：`window` 必须是宿主线程 `HostState` 里仍然活着的一个元素，
/// 而且调用期间没有别人同时可变借用那个 `HostState`。
/// 换一档缩放：重画缓冲、按新位置与尺寸挪窗口、提交新画面。
///
/// 返回缩放后的整窗尺寸（供日志）。
///
/// **顺序是这里唯一不能改的东西**，它是闪烁的根因（见 `docs/34` §4.2.2）：
///
/// 1. 新缓冲先在旁边建好、画满——此时屏幕上一个像素都没动，失败就原样返回。
/// 2. `SetWindowPos` **一次**同时给出新位置和新尺寸。位置和尺寸分成两次调用时，
///    中间那一次是"新尺寸 ＋ 旧位置"（缩放围绕光标移动窗口，位置几乎每档都变），
///    而窗口在那一瞬间显示的仍是**上一档**的画面。
/// 3. 窗口尺寸到位之后才 `UpdateLayeredWindow`。
///    `UpdateLayeredWindow` **会自己按缓冲尺寸改窗口**，所以反过来先提交的话，
///    缩小的时候会有一瞬间"窗口还大、层已经小了"——露出来的那块不在层的缓冲里，
///    屏幕上就是一块透明的闪。放大时反过来是一块被裁掉。
///
/// `SWP_NOREDRAW | SWP_NOCOPYBITS` 是为了让第 2 步**不产生任何绘制**：窗口内容是
/// 第 3 步的层缓冲，系统在这里去拷一份旧像素或立刻重绘都是多余的工作，而且拷的、
/// 画的都是马上要被覆盖的东西。
unsafe fn apply_zoom(hwnd: HWND, window: &mut PinWindowState, position: (i32, i32)) -> Result<(i32, i32)> {
    let border = border_px();
    let scaled = window_size(window.image_width, window.image_height, window.zoom, border);
    let mut framebuffer = LayeredBuffer::new(scaled.0, scaled.1, false)?;
    paint_pin(
        framebuffer.pixels_mut(),
        scaled,
        border,
        &window.image,
        window.image_width,
        window.image_height,
        window.zoom,
        system_dpi(),
    );

    if unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            position.0,
            position.1,
            scaled.0,
            scaled.1,
            SWP_NOACTIVATE | SWP_NOREDRAW | SWP_NOCOPYBITS,
        )
    } == 0
    {
        bail!("调整贴图窗口位置与尺寸失败，错误码 {}", last_error());
    }
    window.position = position;

    framebuffer
        .submit(hwnd)
        .context("提交缩放后的贴图内容失败")?;

    let previous = std::mem::replace(&mut window.framebuffer, framebuffer);
    drop(previous);
    Ok(scaled)
}

/// 拖动时窗口该落在哪里：从锚点算**绝对**位置。
///
/// 抽成纯函数是为了能单测。这里写错过一次，代价是「贴图不跟鼠标」：原来的写法是
/// `窗口 = 窗口 + 光标增量`，而窗口位置在每次移动后都被改掉，于是每一条
/// `WM_MOUSEMOVE` 都把上一次的结果又加上一遍位移，窗口以拖动距离的速率跑飞，
/// 最后完全脱离光标。这里的写法对同一组输入永远给同一个答案，也是为什么它必须
/// 收 `anchor`（拖动开始时窗口在哪）而不是收 `current`（窗口现在在哪）。
fn drag_position(anchor: (i32, i32), cursor_down: (i32, i32), cursor_now: (i32, i32)) -> (i32, i32) {
    (
        anchor.0 + (cursor_now.0 - cursor_down.0),
        anchor.1 + (cursor_now.1 - cursor_down.1),
    )
}

/// 一档缩放之后是多少倍：**在整十分位上加减 1**。
///
/// 步长是 `0.1`（用户定下的，`docs/34` §4.2.1），所以倍数天然活在一个 0.1 的栅格上，
/// 而这个栅格可以用**整数**十分位精确表示。这就是这里的做法：把倍数折算成"第几档"
/// （`round(zoom * 10)`），加减 1，再折回 `f64`。
///
/// # 为什么不直接 `zoom + 0.1`
///
/// `0.1` 在二进制里没有精确表示。连着加四次 `0.1` 得到的是 `0.4` 附近那个**最近**的
/// `f64`，而它并不等于字面上的 `0.4`；滚几十档之后误差会积到看得见的程度（窗口尺寸是
/// `round(图像宽 × 倍数)`，差一点就少一列像素）。整十分位加减没有这个问题：整数永远
/// 精确，折回 `f64` 时每次都是同一个最近值，于是"上一档再下一档"必然回到**逐位相同**
/// 的原值——`zoom_step(zoom_step(z, 1), -1) == z` 是算出来的，不是碰巧成立的。
///
/// 这也让精确的 `1.0` 自动成立：`1.0` 是第 10 档，从任何档上下走都会落到它上面。
///
/// 钳位在折回之后做（[`clamp_zoom`]），所以撞到 `0.1` / `10.0` 之后再滚只是停在原地，
/// 不会绕回去。
fn zoom_step(current: f64, direction: i32) -> f64 {
    if direction == 0 {
        return clamp_zoom(current);
    }
    let current = clamp_zoom(current);
    let tenths = (current * TENTHS_PER_UNIT).round();
    let next = tenths + f64::from(direction.signum() * ZOOM_STEP_TENTHS);
    clamp_zoom(zoom_from_tenths(next))
}

/// 第几档 → 倍数。
///
/// 两点相乘和整数相乘在这里是**精确**的（结果是同一个最近 `f64`），所以过了这一步
/// 再 [钳位](clamp_zoom)，两端的档位就不会带浮点误差。
fn zoom_from_tenths(tenths: f64) -> f64 {
    tenths / TENTHS_PER_UNIT
}

/// 取整和除法隔多远就不再是"同一个档"。
///
/// 只用来把边界那一档按回常量上：`1.0 / 10.0` 与字面量 `0.1` 差 `5.5e-17`，
/// 比这里小四个数量级。取这么小的值是为了它**够不着**相邻档之间的距离（`0.2` 与
/// `0.1` 差 0.1，八个数量级之外），所以它只能命中边界本身，不会把别的档吸过来。
const ZOOM_BOUND_EPSILON: f64 = 1e-12;

/// 把缩放倍数钳进合法范围，并挡住 `NaN`。
///
/// 钳位**先把两端按回字面量常量**再返回（见 [`zoom_from_tenths`]）。少了这一步，
/// "一直往下滚"会停在 `0.09999999999999999` 而不是 `0.1`：数值上没差，但它不等于
/// `ZOOM_MIN`，"滚到底就停在边界上"这句话就没法被断言，也没法被信任。
///
/// `NaN` 归成 `1.0`（基准），`0` 与负数归成 `ZOOM_MIN`——它们都是"这不是一个倍数"，
/// 而两个方向的上限各自不同：往下的尽头是 `0.1`，往上的尽头是 `10.0`。
fn clamp_zoom(zoom: f64) -> f64 {
    if zoom.is_nan() {
        return 1.0;
    }
    if zoom <= 0.0 {
        return ZOOM_MIN;
    }
    let clamped = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
    if (clamped - ZOOM_MIN).abs() < ZOOM_BOUND_EPSILON {
        return ZOOM_MIN;
    }
    if (clamped - ZOOM_MAX).abs() < ZOOM_BOUND_EPSILON {
        return ZOOM_MAX;
    }
    clamped
}

/// 倍数里一整个单位有多少个十分位。`1.0` 是 10 档。
const TENTHS_PER_UNIT: f64 = 10.0;

/// 一档缩放走几个十分位。
const ZOOM_STEP_TENTHS: i32 = 1;

/// 一个源方向上有多少像素变成屏幕像素：向上取整，且至少 1。
///
/// 取整必须和 [`paint_pin`] 里算目标尺寸用的是同一个函数。两处各算一遍的话，窗口会比
/// 缓冲大一个像素（或者反过来），症状是一列边框颜色突然变宽。
fn scaled_dimension(source: f64, zoom: f64) -> i32 {
    let scaled = (source * clamp_zoom(zoom)).round().max(1.0);
    // 宁可截断也不要 `as i32` 在溢出时回绕成负数——那会让 `LayeredBuffer::new` 报
    // "尺寸非法"，而真正的原因是这个 4K×8 倍的窗口根本不该被建出来。
    scaled.min(i32::MAX as f64) as i32
}

/// 整窗尺寸 = 图像缩放后的尺寸 + 两边边框。
///
/// **只有这一个算式**：创建窗口、改缩放、以及绘制时要画多大，全都问它。散成两三处
/// 各算一遍，症状是窗口比内容大一个像素（多出一条边框色）或者小一个像素（最后一列
/// 被裁掉）。
fn window_size(image_width: u32, image_height: u32, zoom: f64, border: i32) -> (i32, i32) {
    (
        scaled_dimension(image_width as f64, zoom) + border * 2,
        scaled_dimension(image_height as f64, zoom) + border * 2,
    )
}

/// 客户端坐标 → 原图像素坐标（整数，落在哪个像素上）。
///
/// 反过来用就是"光标底下是图像的哪一点"，这是光标锚定缩放的输入。边框上、图像外
/// 返回 `None`：那里没有可以钉住的像素。
fn image_point_at_client(
    image_width: u32,
    image_height: u32,
    zoom: f64,
    border: i32,
    client: (i32, i32),
) -> Option<(i32, i32)> {
    let zoom = clamp_zoom(zoom);
    let x = ((client.0 - border) as f64 / zoom).floor();
    let y = ((client.1 - border) as f64 / zoom).floor();
    if x < 0.0 || y < 0.0 || x >= f64::from(image_width) || y >= f64::from(image_height) {
        return None;
    }
    Some((x as i32, y as i32))
}

/// 原图像素坐标 → 客户端坐标。它是 [`image_point_at_client`] 的反函数，两者必须成对
/// 使用，缩放的不变量（见 [`zoom_with_cursor`]）才成立。
fn image_point_to_client(
    zoom: f64,
    border: i32,
    image: (i32, i32),
) -> (i32, i32) {
    let zoom = clamp_zoom(zoom);
    (
        (f64::from(image.0) * zoom).round() as i32 + border,
        (f64::from(image.1) * zoom).round() as i32 + border,
    )
}

/// 滚轮缩放的核心：**光标底下的那个像素在屏幕上不动**。
///
/// 这是缩放唯一的"正确性"判据，也是它比"围绕窗口中心缩放"好用的原因：用户把光标压在
/// 想看的那个字上滚，那个字留在原地，别的都以它为中心伸缩。
///
/// 锚点用**原图像素坐标**表达，所以这条不变量与缩放倍数无关：
/// `image_point_at_client(zoom', cursor) == image_point_at_client(zoom, cursor)`。
///
/// 光标落在边框上（`image_point_at_client` 给 `None`）时位置不变，窗口只是向右下
/// 长大。不选"围绕中心"是为了少一个输入：中心缩放要先知道窗口尺寸，而尺寸又是缩放的
/// 结果，绕一圈只会多一处可以写错的地方。
#[allow(clippy::too_many_arguments)]
fn zoom_with_cursor(
    zoom: f64,
    zoom_after: f64,
    position: (i32, i32),
    image_width: u32,
    image_height: u32,
    border: i32,
    cursor: (i32, i32),
) -> (i32, i32) {
    let cursor_client = (cursor.0 - position.0, cursor.1 - position.1);
    let Some(anchor) =
        image_point_at_client(image_width, image_height, zoom, border, cursor_client)
    else {
        return position;
    };
    let after = image_point_to_client(zoom_after, border, anchor);
    (
        position.0 + cursor_client.0 - after.0,
        position.1 + cursor_client.1 - after.1,
    )
}

/// 滚轮该把缩放带到哪里。
///
/// 抽出来是为了能单测"累加到达标才动"这条：高精度滚轮一个消息只给 30，四次才够一档，
/// 而"差 30"这件事必须跨消息记着——正是它让 `remainder` 有了存在的理由。
///
/// 返回值第三项是这一档的方向（`0` 表示还没凑够）。调用方靠它判断"这次要不要真的
/// 重绘"，否则每个滚轮消息都会重建一遍缓冲。
fn wheel_zoom(zoom: f64, remainder: i32, delta: i32) -> (f64, i32, i32) {
    let mut remainder = remainder.saturating_add(delta);
    let mut notches = 0;
    // `saturating_*`：`wparam` 是外部输入，`delta` 可以接近 `i32::MAX`，一次滚轮
    // 理论上能把累加顶到溢出。溢出在 release 下是**回绕**，会让方向反一下。
    while remainder.abs() >= WHEEL_DELTA {
        let step = remainder.signum();
        notches += step;
        remainder = remainder.saturating_sub(step * WHEEL_DELTA);
    }
    let zoom = if notches == 0 {
        zoom
    } else {
        let mut zoom = zoom;
        // 方向可能一次走过好几档（快速滚轮），逐档走才能让每一档都吃到钳位：
        // 一次性跳 `ZOOM_STEP_TENTHS * notches` 会从 10.0 之上直接落到 10.0 以外，
        // 中途那些"本该停在边界上"的档就没了。
        let mut remaining = notches;
        while remaining != 0 {
            let step = remaining.signum();
            zoom = zoom_step(zoom, step);
            remaining -= step;
        }
        zoom
    };
    (zoom, remainder, notches)
}

/// 把边框和图像画进整窗缓冲。
///
/// 纯函数，就是为了能单测：这里出错的典型样子是图像整体偏移一个像素、某一行串到
/// 相邻行去，或者缩放时把最后一列少画一个像素。
///
/// 缩放用的是**最近邻**：目标区域的每个像素都回原图取它落在哪个源像素上。不选双线性
/// 有两个理由，都不是口味问题——贴图要看的是文本和图表的细节，重采样会把它抹糊；而且
/// 最近邻在整数倍缩放（1× → 2× → 1×）下是**精确可逆**的，§6.1 的 T10 就是拿这个
/// 当断言。
fn paint_pin(
    buffer: &mut [u32],
    size: (i32, i32),
    border: i32,
    image: &[u32],
    image_width: u32,
    image_height: u32,
    zoom: f64,
    dpi: u32,
) {
    let width = size.0.max(0) as usize;
    let height = size.1.max(0) as usize;
    debug_assert_eq!(buffer.len(), width * height);
    buffer.fill(BORDER_COLOR);

    let border = border.max(0) as usize;
    if width == 0 || height == 0 || border >= width || border >= height {
        // 窗口小到放不下边框，那就整块都是边框。
        return;
    }
    // 目标尺寸走的是和 `window_size` 同一个算式，不是调用方另算一遍的 `size`。
    let target_width = scaled_dimension(f64::from(image_width), zoom).max(0) as usize;
    let target_height = scaled_dimension(f64::from(image_height), zoom).max(0) as usize;
    if target_width == 0 || target_height == 0 {
        return;
    }

    let source_width = image_width as usize;
    let source_height = image_height as usize;
    if source_width == 0 || source_height == 0 {
        return;
    }
    // 按整窗宽高比给每个缩放轴一个系数：整数倍时它恰好是 1、0.5、2……，于是
    // `x * factor` 这类写法在 1× 下退化成恒等，不会引入任何舍入。
    let factor_x = target_width as f64 / f64::from(image_width);
    let factor_y = target_height as f64 / f64::from(image_height);
    if factor_x <= 0.0 || factor_y <= 0.0 {
        return;
    }

    let drawn_width = target_width.min(width.saturating_sub(border));
    let drawn_height = target_height.min(height.saturating_sub(border));
    for row in 0..drawn_height {
        let source_y = (row as f64 / factor_y) as usize;
        if source_y >= source_height {
            break;
        }
        let dest_row = row + border;
        let dest_start = dest_row * width + border;
        let source_row = source_y * source_width;
        for column in 0..drawn_width {
            let source_x = (column as f64 / factor_x) as usize;
            if source_x >= source_width {
                break;
            }
            buffer[dest_start + column] = image[source_row + source_x];
        }
    }

    // 倍数控件最后画：它要压在图像上，而上面那段会把整块图像区重写一遍。
    composite_badge(
        buffer,
        size,
        image_width,
        image_height,
        zoom,
        border as i32,
        dpi,
    );
}

/// 边框宽度（像素）。按系统 DPI 折算，保证在高 DPI 屏幕上物理宽度一致。
fn border_px() -> i32 {
    let dpi = system_dpi();
    ((BORDER_DIP as f64) * (dpi as f64) / 96.0).round().max(1.0) as i32
}

/// 系统 DPI。取不到就按 96 算。
fn system_dpi() -> u32 {
    unsafe {
        let module = GetModuleHandleW(wide("user32.dll").as_ptr());
        if module.is_null() {
            return 96;
        }
        // `GetDpiForSystem` 在 windows-sys 0.59 里没有绑定，手工取地址。
        // 拿不到就退到 96 DPI。
        let Some(proc) = GetProcAddress(module, c"GetDpiForSystem".as_ptr().cast()) else {
            return 96;
        };
        let get_dpi_for_system: unsafe extern "system" fn() -> u32 = std::mem::transmute(proc);
        match get_dpi_for_system() {
            0 => 96,
            dpi => dpi,
        }
    }
}

// ─────────────────────────── 缩放倍数控件 ───────────────────────────
//
// 贴图右上角那个显示当前倍数的圆角小标（`docs/34` §4.2.2）。样式照 `magnifier_pass.rs`
// 里放大镜面板的同名控件：14 DIP 粗体、左右 8 DIP、上下 5 DIP 内边距、距角 6 DIP、
// 6 DIP 圆角。尺寸照搬是为了让两个控件看起来是同一个东西；**实现完全不同**——放大镜
// 在 D2D 上画，贴图只能往自己的 `u32` 缓冲里逐像素写。

/// 控件文字的字号（DIP），与放大镜那颗一致。
const BADGE_FONT_DIP: f64 = 14.0;
const BADGE_PAD_H_DIP: f64 = 8.0;
const BADGE_PAD_V_DIP: f64 = 5.0;
const BADGE_INSET_DIP: f64 = 6.0;
const BADGE_RADIUS_DIP: f64 = 6.0;

/// 控件底色与文字色，都是不透明 premultiplied BGRA。
///
/// 比边框（`BORDER_COLOR`）浅一档，这样它压在图像上时边缘能自己分出来；不透明是因为
/// 贴图整窗都不透明（捕获侧的像素 alpha 被强制成 255），留一个半透明的小块只会让
/// 底下的图像透出来变脏。
const BADGE_BG_COLOR: u32 = 0xFF52_5252;
const BADGE_TEXT_COLOR: u32 = 0xFFFF_FFFF;

/// 把倍数变成控件上的字。
///
/// **百分比**（`docs/34` §4.2.2）：用户说的是"基准是 100%"，那就把基准显示成 100%。
/// 步长 0.1 决定结果只需要一位小数，四舍五入到整数后 `%` 恰好把那一档也表达清楚了
/// （`0.1` → `10%`，`10.0` → `1000%`）。
fn badge_label(zoom: f64) -> String {
    let percent = (clamp_zoom(zoom) * 100.0).round();
    format!("{}%", percent as i64)
}

/// 控件里那行字的光栅化结果：**覆盖率掩码**。
///
/// `coverage` 是每像素 0–255 的覆盖度（不是颜色），尺寸就是文字的像素外框。做成掩码
/// 而不是"已经染好色的位图"，是因为颜色决定权必须留在 [`blend_coverage`] 里：文字色
/// 与底色是两个常量，混色是一次整数乘法，比再建一张彩色位图便宜得多。
struct TextMask {
    coverage: Vec<u16>,
    width: i32,
    height: i32,
    /// 文字占的像素宽度（`coverage` 的 stride 用的就是它）。给排版用。
    text_width: i32,
}

thread_local! {
    /// 文字掩码的缓存，key 是 `(标签, DPI)`。
    ///
    /// 存在的理由是缩放**每次滚轮都重画整块缓冲**：同一个 `150%` 在一次连续滚动里会被
    /// 重新光栅化很多遍（放大到 150%、再放大、再缩回来……），而光栅化要建 DC、选字体、
    /// 建 DIB。标签只有 100 种、DPI 只有一两种，所以缓存是无界的也不会长。
    ///
    /// `thread_local` 而不是字段：它跟着宿主线程走，池子随线程一起消失，不需要谁去清。
    static TEXT_MASKS: RefCell<HashMap<(String, u32), Arc<TextMask>>> =
        RefCell::new(HashMap::new());
}

/// 光栅化一行字，带缓存。
fn text_mask(text: &str, dpi: u32) -> Option<Arc<TextMask>> {
    let key = (text.to_string(), dpi);
    if let Some(hit) = TEXT_MASKS.with(|cache| cache.borrow().get(&key).cloned()) {
        return Some(hit);
    }
    let mask = Arc::new(rasterize_text(text, dpi)?);
    TEXT_MASKS.with(|cache| cache.borrow_mut().insert(key, Arc::clone(&mask)));
    Some(mask)
}

/// 用 GDI 把一行字画进一张**白底黑字**的位图，再把红色通道当成覆盖率读出来。
///
/// 白底黑字是刻意的：GDI 会把反锯齿的中间值算进像素颜色里，而"黑色有多少"正好就是
/// "笔画的覆盖率有多少"。若反过来画白字黑底，颜色与覆盖率会混在同一个通道里分不开。
///
/// 掩码是全黑的（覆盖率 0）也不当失败——那句话只是"字没渲染出来"，控件会显示成一个
/// 空的圆角色块，缩放本身照常工作。为一个装饰性的小标把 F6 搞失败是不划算的。
fn rasterize_text(text: &str, dpi: u32) -> Option<TextMask> {
    let scale = f64::from(dpi.max(96)) / 96.0;
    let height = (BADGE_FONT_DIP * scale).round().max(1.0);
    let font_height = -(height as i32);
    unsafe {
        let screen_dc = GetDC(std::ptr::null_mut());
        if screen_dc.is_null() {
            return None;
        }
        // 用一个函数收尾，好让中途任何一处 `return None` 都不会漏掉 `DeleteDC`。
        let result = (|| -> Option<TextMask> {
            let memory_dc = CreateCompatibleDC(screen_dc);
            if memory_dc.is_null() {
                return None;
            }
            let font = CreateFontW(
                font_height,
                0,
                0,
                0,
                FW_SEMIBOLD as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_TT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                0,
                DEFAULT_PITCH as u32 | FF_DONTCARE as u32,
                wide("Segoe UI").as_ptr(),
            );
            let mask = if font.is_null() {
                None
            } else {
                let previous_font = SelectObject(memory_dc, font.into());
                let white = GetStockObject(WHITE_BRUSH);
                let previous_brush = SelectObject(memory_dc, white);
                SetBkMode(memory_dc, TRANSPARENT as i32);
                SetTextColor(memory_dc, 0x0000_0000);

                let width = (dpi.max(96) * 3) as i32;
                let mut info: BITMAPINFO = std::mem::zeroed();
                info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                info.bmiHeader.biWidth = width;
                // 负高度 = 自顶向下：行号直接是像素列号，不用再翻一次。
                info.bmiHeader.biHeight = -font_height;
                info.bmiHeader.biPlanes = 1;
                info.bmiHeader.biBitCount = 32;
                info.bmiHeader.biCompression = BI_RGB;

                let mut bits: *mut c_void = std::ptr::null_mut();
                let bitmap = CreateDIBSection(
                    memory_dc,
                    &info,
                    DIB_RGB_COLORS,
                    &mut bits,
                    std::ptr::null_mut(),
                    0,
                );
                let mask = if bitmap.is_null() || bits.is_null() {
                    None
                } else {
                    let previous_bitmap = SelectObject(memory_dc, bitmap.into());
                    let mut rect = RECT {
                        left: 0,
                        top: 0,
                        right: width,
                        bottom: font_height.abs(),
                    };
                    // `FillRect` / `DrawTextW` 的签名收 `*mut RECT`（老 API 没有 `const`），
                    // 但两者都不写这块矩形，所以用 `&raw mut` 借出指针即可，不需要可变绑定。
                    FillRect(memory_dc, &raw mut rect, white);

                    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
                    DrawTextW(
                        memory_dc,
                        wide.as_ptr(),
                        -1,
                        &raw mut rect,
                        DT_LEFT | DT_TOP | DT_SINGLELINE | DT_NOPREFIX,
                    );

                    let stride = width as usize;
                    let rows = font_height.unsigned_abs() as usize;
                    let source = std::slice::from_raw_parts(bits as *const u32, stride * rows);

                    // 文字只占左边一小截，量出来之后掩码就只留那一截。
                    let mut text_width = 0usize;
                    for row in source.chunks_exact(stride) {
                        let last = row.iter().rposition(|pixel| (pixel & 0xFF) < 250);
                        if let Some(last) = last {
                            text_width = text_width.max(last + 1);
                        }
                    }
                    let mut coverage = Vec::with_capacity(text_width * rows);
                    for row in source.chunks_exact(stride) {
                        for pixel in &row[..text_width] {
                            // 白底黑字：红通道越大越白，覆盖率就是它的补。
                            coverage.push(255 - (pixel & 0xFF) as u16);
                        }
                    }

                    SelectObject(memory_dc, previous_bitmap);
                    DeleteObject(bitmap.into());
                    Some(TextMask {
                        coverage,
                        width: text_width as i32,
                        height: rows as i32,
                        text_width: text_width as i32,
                    })
                };
                SelectObject(memory_dc, previous_brush);
                SelectObject(memory_dc, previous_font);
                DeleteObject(font.into());
                mask
            };
            DeleteDC(memory_dc);
            mask
        })();
        ReleaseDC(std::ptr::null_mut(), screen_dc);
        result
    }
}
/// 控件的排版尺寸，按 DPI 折算。
struct BadgeStyle {
    /// 左右内边距。
    pad_h: i32,
    /// 上下内边距。
    pad_v: i32,
    /// 距图像右上角的距离。
    inset: i32,
    /// 圆角半径。
    radius: i32,
}

impl BadgeStyle {
    fn new(dpi: u32) -> Self {
        let scale = f64::from(dpi.max(96)) / 96.0;
        let scale_dip = |dip: f64| (dip * scale).round().max(0.0) as i32;
        Self {
            pad_h: scale_dip(BADGE_PAD_H_DIP),
            pad_v: scale_dip(BADGE_PAD_V_DIP),
            inset: scale_dip(BADGE_INSET_DIP),
            radius: scale_dip(BADGE_RADIUS_DIP),
        }
    }
}

/// 把缩放倍数控件画到整窗缓冲的右上角。
///
/// **只覆盖图像区，不碰边框**：控件的右上角对齐的是"图像区右上角往里缩 `inset`"，
/// 而不是窗口的右上角。贴图那圈边框是它与底下的窗口分开的唯一标记，被控件压掉一段
/// 就会看起来像没贴好。
///
/// 让位规则（见调用方 [`paint_pin`]）：图像区小到放不下控件时**整个不画**，不缩小、
/// 不裁切——一个缺角的 `1000%` 比没有控件更容易被当成贴图坏了。
fn composite_badge(
    buffer: &mut [u32],
    size: (i32, i32),
    image_width: u32,
    image_height: u32,
    zoom: f64,
    border: i32,
    dpi: u32,
) {
    let style = BadgeStyle::new(dpi);
    let Some(mask) = text_mask(&badge_label(zoom), dpi) else {
        return;
    };
    let badge_width = mask.text_width + style.pad_h * 2;
    let badge_height = mask.height + style.pad_v * 2;

    // 图像区在整窗缓冲里的范围。
    let (left, top) = (border, border);
    let right = border + scaled_dimension(f64::from(image_width), zoom);
    let bottom = border + scaled_dimension(f64::from(image_height), zoom);

    let badge_right = right - style.inset;
    let badge_bottom = top + style.inset + badge_height;
    let badge_left = badge_right - badge_width;
    if badge_left < left || badge_bottom > bottom || badge_width <= 0 || badge_height <= 0 {
        return;
    }

    let row_pitch = size.0 as usize;
    for row in 0..badge_height {
        let destination_row = top + style.inset + row;
        if destination_row < 0 || destination_row >= size.1 {
            continue;
        }
        for column in 0..badge_width {
            if !inside_rounded(column, row, badge_width, badge_height, style.radius) {
                continue;
            }
            let index = destination_row as usize * row_pitch + (badge_left + column) as usize;
            if let Some(pixel) = buffer.get_mut(index) {
                *pixel = BADGE_BG_COLOR;
            }
        }
    }

    blend_coverage(
        buffer,
        row_pitch,
        badge_left + style.pad_h,
        top + style.inset + style.pad_v,
        &mask,
        BADGE_TEXT_COLOR,
    );
}

/// 一个点是不是在一个圆角矩形里。
///
/// 四个角各问一次到圆心的距离；圆心之外那四块小方角被圆切掉的才是"外"。
fn inside_rounded(column: i32, row: i32, width: i32, height: i32, radius: i32) -> bool {
    let radius = radius.min(width / 2).min(height / 2).max(0);
    if radius == 0 {
        return true;
    }
    let x = if column < radius {
        radius - column
    } else if column >= width - radius {
        column - (width - radius - 1)
    } else {
        0
    };
    let y = if row < radius {
        radius - row
    } else if row >= height - radius {
        row - (height - radius - 1)
    } else {
        0
    };
    x * x + y * y <= radius * radius
}

/// 把一段覆盖率覆盖到缓冲里。`mask` 的左上角落在 `(x, y)`。
fn blend_coverage(
    buffer: &mut [u32],
    row_pitch: usize,
    x: i32,
    y: i32,
    mask: &TextMask,
    color: u32,
) {
    for row in 0..mask.height {
        let destination_row = y + row;
        if destination_row < 0 {
            continue;
        }
        for column in 0..mask.width {
            let coverage = mask.coverage[(row * mask.width + column) as usize];
            if coverage == 0 {
                continue;
            }
            let index = destination_row as usize * row_pitch + (x + column) as usize;
            if let Some(destination) = buffer.get_mut(index) {
                *destination = over(*destination, color, coverage as u32);
            }
        }
    }
}

/// 一个源颜色按覆盖率盖在已有的自左乘像素上，返回自左乘的结果。
///
/// 用 `blend` 而不是 `destination * (1 - a) + source * a`：两者在 `a` 取 0 或 255 时
/// **数值上等价**，但 `blend` 里那个 `+ 127` 让中间值不会越出 `u32`，也不需要再夹一次。
fn over(destination: u32, color: u32, coverage: u32) -> u32 {
    let source = [color & 0xFF, (color >> 8) & 0xFF, (color >> 16) & 0xFF];
    let mut channels = [0u32; 3];
    for channel in 0..3 {
        let destination_channel = (destination >> (channel * 8)) & 0xFF;
        channels[channel] = blend(destination_channel, source[channel], coverage);
    }
    channels[0] | (channels[1] << 8) | (channels[2] << 16) | (0xFF << 24)
}

/// 两个通道按覆盖率混合：`0` 取前者，`255` 取后者。
fn blend(one: u32, other: u32, coverage: u32) -> u32 {
    (one * (255 - coverage) + other * coverage + 127) / 255
}

/// 一块交给 `UpdateLayeredWindow` 的整窗位图。

///
/// 用 `u32` 而不是 `u8` 持有 DIB 内存：`CreateDIBSection` 返回的缓冲区按 4 字节
/// 对齐，用 `u32` 表达这个前提，就不用到处写 `bytemuck` 式的转换。
struct LayeredBuffer {
    memory_dc: HDC,
    bitmap: HBITMAP,
    previous_bitmap: HGDIOBJ,
    pixels: *mut u32,
    width: i32,
    height: i32,
    /// 像素数，用来构造切片。
    len: usize,
}

// 所有 GDI 句柄只在宿主线程上用，`LayeredBuffer` 从不离开宿主线程。
unsafe impl Send for LayeredBuffer {}

impl LayeredBuffer {
    /// `zeroed` 为 false 时不清零：调用方马上会把整块像素写满（屏幕回读就是），
    /// 清零只是白白多写一遍。
    fn new(width: i32, height: i32, zeroed: bool) -> Result<Self> {
        if width <= 0 || height <= 0 {
            bail!("位图尺寸非法：{width}x{height}");
        }
        unsafe {
            // `GetDC(null)` 拿屏幕 DC：用它创建的 DIB 与屏幕同格式，
            // 提交时不需要任何转换。
            let screen_dc = GetDC(std::ptr::null_mut());
            if screen_dc.is_null() {
                bail!("GetDC(屏幕) 失败");
            }
            let memory_dc = CreateCompatibleDC(screen_dc);
            if memory_dc.is_null() {
                ReleaseDC(std::ptr::null_mut(), screen_dc);
                bail!("CreateCompatibleDC 失败");
            }

            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            info.bmiHeader.biWidth = width;
            // 负高度 = 自上而下，第 0 行就是屏幕上的第 0 行。
            info.bmiHeader.biHeight = -height;
            info.bmiHeader.biPlanes = 1;
            info.bmiHeader.biBitCount = 32;
            info.bmiHeader.biCompression = BI_RGB;

            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(
                screen_dc,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            ReleaseDC(std::ptr::null_mut(), screen_dc);
            if bitmap.is_null() || bits.is_null() {
                DeleteDC(memory_dc);
                bail!("CreateDIBSection 失败，错误码 {}", last_error());
            }
            let previous_bitmap = SelectObject(memory_dc, bitmap as HGDIOBJ);
            if previous_bitmap.is_null() {
                DeleteObject(bitmap as HGDIOBJ);
                DeleteDC(memory_dc);
                bail!("SelectObject(DIB) 失败，错误码 {}", last_error());
            }

            let pixels = bits.cast::<u32>();
            let len = (width as usize) * (height as usize);
            // 新 DIB 的内容是未定义的，先清零，免得首次提交前露出垃圾像素。
            if zeroed {
                std::ptr::write_bytes(pixels, 0, len);
            }

            Ok(Self {
                memory_dc,
                bitmap,
                previous_bitmap,
                pixels,
                width,
                height,
                len,
            })
        }
    }

    fn pixels_mut(&mut self) -> &mut [u32] {
        unsafe { std::slice::from_raw_parts_mut(self.pixels, self.len) }
    }

    /// 把整窗内容提交到窗口。
    fn submit(&self, hwnd: HWND) -> Result<()> {
        unsafe {
            let size = SIZE {
                cx: self.width,
                cy: self.height,
            };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                // 255 = 不额外压暗；透明度完全由每个像素自己的 alpha 决定。
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let ok = UpdateLayeredWindow(
                hwnd,
                std::ptr::null_mut(),
                std::ptr::null(),
                &size,
                self.memory_dc,
                &src,
                0,
                &blend,
                ULW_ALPHA,
            );
            if ok == 0 {
                bail!("UpdateLayeredWindow 失败，错误码 {}", last_error());
            }
        }
        Ok(())
    }

    /// 释放 GDI 对象。可重复调用。
    fn release(&mut self) {
        unsafe {
            if !self.memory_dc.is_null() {
                if !self.previous_bitmap.is_null() {
                    SelectObject(self.memory_dc, self.previous_bitmap);
                    self.previous_bitmap = std::ptr::null_mut();
                }
                DeleteDC(self.memory_dc);
                self.memory_dc = std::ptr::null_mut();
            }
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap as HGDIOBJ);
                self.bitmap = std::ptr::null_mut();
            }
            self.pixels = std::ptr::null_mut();
            self.len = 0;
        }
    }
}

impl Drop for LayeredBuffer {
    fn drop(&mut self) {
        self.release();
    }
}

/// `GWLP_USERDATA` 的数值。`windows-sys` 把它定义在 `WINDOW_LONG_PTR_INDEX` 里，
/// 这里直接用数值，省掉一层类型转换。
const GWLP_USERDATA: i32 = -21;

fn register_window_class(instance: HINSTANCE, class_name: &[u16]) -> Result<()> {
    unsafe {
        let mut class: WNDCLASSW = std::mem::zeroed();
        class.style = CS_DBLCLKS;
        class.lpfnWndProc = Some(wnd_proc);
        class.hInstance = instance;
        class.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
        // 窗口内容全部来自 DIB，背景笔刷只在 DWM 还没拿到分层内容的那一瞬
        // 起作用，用黑色避免闪白。不用自建笔刷，省一个要记得删的对象。
        class.hbrBackground = GetStockObject(BLACK_BRUSH) as HBRUSH;
        class.lpszClassName = class_name.as_ptr();
        if RegisterClassW(&class) == 0 {
            bail!("RegisterClassW 失败，错误码 {}", GetLastError());
        }
    }
    Ok(())
}

fn unregister_window_class(instance: HINSTANCE, class_name: &[u16]) {
    unsafe {
        UnregisterClassW(class_name.as_ptr(), instance);
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_error() -> u32 {
    unsafe { GetLastError() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 边框色独立写一遍，避免「实现改了测试跟着改」。
    const BORDER: u32 = 0xFF40_4040;

    /// 测试一律按 96 DPI 算：`system_dpi()` 在无桌面会话里也可能返回别的值，
    /// 而这里要断言的是像素布局，不是 DPI 折算。
    const TEST_DPI: u32 = 96;

    #[test]
    fn a_null_window_message_is_handled_here_and_never_dispatched() {
        // 这条守的是 F6 唯一一次真实故障的根因：`RegisterHotKey(null, …)` 让
        // `WM_HOTKEY` 成为**线程消息**（`hwnd` 为 null），而 `DispatchMessageW`
        // 对它是静默丢弃。修好之前的表现是「注册成功、日志干净、按下去毫无反应」。
        //
        // 断言分两层：路由必须选 `HandleHere`（也就是真的会进 `on_message`），
        // 而且 `WM_HOTKEY` 无论在哪种窗口情形下都不能掉进 `Dispatch`。
        assert_eq!(
            route_message(std::ptr::null_mut(), WM_HOTKEY),
            MessageRoute::HandleHere
        );
        assert_eq!(
            route_message(std::ptr::null_mut(), WM_APP_PIN_WAKE),
            MessageRoute::HandleHere
        );
        assert_eq!(
            route_message(std::ptr::null_mut(), WM_NCHITTEST),
            MessageRoute::HandleHere
        );
    }

    #[test]
    fn a_window_message_is_dispatched_to_the_window() {
        // 有窗口的消息仍然必须走 `DispatchMessageW`：贴图的拖动与双击都在窗口
        // 过程里，把它们也拦到自己手里就等于让窗口失效。
        let hwnd = 0x1234 as HWND;
        assert_eq!(
            route_message(hwnd, WM_MOUSEMOVE),
            MessageRoute::Dispatch
        );
        assert_eq!(
            route_message(hwnd, WM_LBUTTONDBLCLK),
            MessageRoute::Dispatch
        );
        assert_eq!(
            route_message(hwnd, WM_NCHITTEST),
            MessageRoute::Dispatch
        );
    }

    #[test]
    fn dragging_keeps_the_window_under_the_cursor() {
        // 位置必须等于锚点 + 总位移，而不是任一次中间结果。
        let anchor = (100, 200);
        let down = (500, 500);

        assert_eq!(drag_position(anchor, down, (501, 500)), (101, 200));
        assert_eq!(drag_position(anchor, down, (510, 505)), (110, 205));
        assert_eq!(drag_position(anchor, down, (530, 490)), (130, 190));

        // 同一个光标位置无论问几次都是同一个答案（幂等）：消息重复、合并、
        // 甚至整段丢失都不会留下累积误差。
        for _ in 0..5 {
            assert_eq!(drag_position(anchor, down, (530, 490)), (130, 190));
        }

        // 回到按下时那一点，窗口必须回到原地——这是"跟手"的最直白判据。
        assert_eq!(drag_position(anchor, down, down), anchor);
    }

    #[test]
    fn a_cumulative_drag_would_lose_the_cursor() {
        // 上一条只能证明这个公式对，证明不了「为什么必须给公式一个锚点」。
        // 故障出在消息处理循环里：窗口位置在每条消息之后被**改写**，如果下一
        // 条消息把它当基准再加上位移，误差就一次一次攒起来。这里把那个循环
        // 两种写法各跑一遍，差值就是当初屏幕上看到的现象。
        let anchor = (100, 200);
        let down = (500, 500);
        let path = [(511, 503), (543, 521), (600, 560), (688, 611), (720, 629)];

        let mut anchored = anchor;
        let mut cumulative = anchor;
        for cursor in path {
            anchored = drag_position(anchor, down, cursor);
            // 旧写法：`窗口 = 窗口 + (光标 − 按下点)`，基准是"窗口现在在哪"。
            cumulative = (
                cumulative.0 + (cursor.0 - down.0),
                cumulative.1 + (cursor.1 - down.1),
            );
        }

        // 光标一共只走了 220 像素，锚点写法把它老老实实翻译成窗口位移。
        assert_eq!(anchored, (320, 329));
        // 累加写法同时把每一段位移都重复计入，窗口多走了一个身位。
        assert_eq!(cumulative, (662, 524));
        // 而且差距随拖动距离单调放大：这正是"越拖越不跟手"。
        assert!(cumulative.0 - anchored.0 > 300);
        assert!(
            (cumulative.0 - anchored.0) > (543 - down.0) * 4,
            "拖动越久偏得越多"
        );
    }

    /// 一行像素的测试数据。
    fn image_of(width: u32, height: u32) -> Vec<u32> {
        (0..(width * height)).map(|i| 0xFF00_0000 | (i + 1)).collect()
    }

    #[test]
    fn paints_border_around_image() {
        let image = image_of(2, 2);
        let mut buffer = vec![0u32; 4 * 4];
        paint_pin(&mut buffer, (4, 4), 1, &image, 2, 2, 1.0, TEST_DPI);

        for (x, y) in [(0, 0), (3, 0), (0, 3), (3, 3), (1, 0), (0, 1)] {
            assert_eq!(buffer[y * 4 + x], BORDER, "({x},{y}) 应当是边框");
        }
        // 中间 2x2 是原图，逐像素对齐。
        assert_eq!(buffer[1 * 4 + 1], image[0]);
        assert_eq!(buffer[1 * 4 + 2], image[1]);
        assert_eq!(buffer[2 * 4 + 1], image[2]);
        assert_eq!(buffer[2 * 4 + 2], image[3]);
    }

    #[test]
    fn image_rows_do_not_bleed_into_each_other() {
        // 3x2 的图贴进 5x4 的窗口（边框 1）。用不同的行值来抓「串行」。
        let image = vec![
            1, 1, 1, // 第一行
            2, 2, 2, // 第二行
        ];
        let mut buffer = vec![0u32; 5 * 4];
        paint_pin(&mut buffer, (5, 4), 1, &image, 3, 2, 1.0, TEST_DPI);

        assert_eq!(&buffer[1 * 5 + 1..1 * 5 + 4], &[1, 1, 1]);
        assert_eq!(&buffer[2 * 5 + 1..2 * 5 + 4], &[2, 2, 2]);
        // 首末行仍是边框色。
        assert_eq!(buffer[0], BORDER);
        assert_eq!(buffer[3 * 5 + 4], BORDER);
    }

    #[test]
    fn clamps_when_image_is_larger_than_window() {
        // 不是正常路径，但绝不允许越界 panic。
        let image = image_of(8, 8);
        let mut buffer = vec![0u32; 2 * 2];
        paint_pin(&mut buffer, (2, 2), 1, &image, 8, 8, 1.0, TEST_DPI);
        assert_eq!(buffer.len(), 4);
    }

    #[test]
    fn tiny_window_is_all_border() {
        // 1x1 的窗口放不下边框，应当整块是边框色而不是 panic。
        let image = image_of(4, 4);
        let mut buffer = vec![0u32; 1];
        paint_pin(&mut buffer, (1, 1), 1, &image, 4, 4, 1.0, TEST_DPI);
        assert_eq!(buffer[0], BORDER);
    }

    /// 一档就是 0.1，所以倍数永远停在 0.1 的栅格上，且**上一档再下一档逐位相同**。
    ///
    /// 这条现在是**精确**断言（不再需要容差）：步长是整十分位，`zoom_step` 用整数加减
    /// 再折回 `f64`，折回时每次得到的是同一个最近值。旧实现（乘除 `1.1`）做不到这一点，
    /// 所以那时这里容忍 `1e-9`；换成 0.1 之后容差就没有存在的理由了。
    #[test]
    fn a_step_up_then_down_returns_to_the_same_value() {
        let mut slot = 1.0;
        for _ in 0..20 {
            let up = zoom_step(slot, 1);
            let back = zoom_step(up, -1);
            assert_eq!(back, slot, "从 {slot} 放大到 {up} 再缩小应当回到原值");
            slot = up;
        }
    }

    /// 步长是 0.1：从 1.0 往上十档正好到 2.0，往下十档正好到……`1.0` 之下是 `0.9`。
    #[test]
    fn a_step_is_one_tenth() {
        assert!((zoom_step(1.0, 1) - 1.1).abs() < 1e-12);
        assert!((zoom_step(1.0, -1) - 0.9).abs() < 1e-12);
        // 100% 是基准，所以 1.0 必须是一个精确的档位。
        assert_eq!(zoom_step(0.9, 1), 1.0);

        let mut zoom = 1.0;
        for _ in 0..10 {
            zoom = zoom_step(zoom, 1);
        }
        assert_eq!(zoom, 2.0, "十档 0.1 就是 +1.0");
    }

    #[test]
    fn zoom_stops_at_its_bounds() {
        // 一直往上滚到底就是 ZOOM_MAX，再滚不动了。
        let mut high = 1.0;
        for _ in 0..500 {
            high = zoom_step(high, 1);
        }
        assert_eq!(high, ZOOM_MAX);
        let mut low = 1.0;
        for _ in 0..500 {
            low = zoom_step(low, -1);
        }
        assert_eq!(low, ZOOM_MIN);
        // `0` 档是恒等：滚轮没凑够一档时走的正是这条路。
        assert_eq!(zoom_step(2.5, 0), 2.5);
    }

    /// 高精度滚轮：一个消息只给 30，四次才够一档，而且账要跨消息记着。
    #[test]
    fn a_high_precision_wheel_accumulates_until_a_whole_notch() {
        let mut zoom = 1.0;
        let mut remainder = 0;
        for _ in 0..3 {
            let (next, rest, notches) = wheel_zoom(zoom, remainder, 30);
            zoom = next;
            remainder = rest;
            assert_eq!(notches, 0, "还没凑够一档就不该动");
            assert_eq!(zoom, 1.0);
        }
        assert_eq!(remainder, 90);
        let (next, rest, notches) = wheel_zoom(zoom, remainder, 30);
        assert_eq!(notches, 1);
        assert_eq!(rest, 0);
        assert!((next - 1.1).abs() < 1e-12, "实际 {next}");
    }

    #[test]
    fn one_wheel_message_can_carry_several_notches() {
        // 普通鼠标一次滚三格：`zDelta = 360`，必须走三档而不是一档。
        let (zoom, remainder, notches) = wheel_zoom(1.0, 0, 3 * WHEEL_DELTA);
        assert_eq!(notches, 3);
        assert_eq!(remainder, 0);
        let expected = 1.3;
        assert!((zoom - expected).abs() < 1e-12, "实际 {zoom}");
        // 反向同理。
        let (down, _, notches) = wheel_zoom(1.0, 0, -3 * WHEEL_DELTA);
        assert_eq!(notches, -3);
        assert!((down - 0.7).abs() < 1e-12, "实际 {down}");
    }

    /// **缩放唯一的正确性判据**：光标底下那个像素在屏幕上不动。
    #[test]
    fn zooming_keeps_the_pixel_under_the_cursor_still() {
        let (image_width, image_height) = (100u32, 80u32);
        let border = 1;
        let size = window_size(image_width, image_height, 1.0, border);
        assert_eq!(size, (102, 82));
        let position = (300, 200);
        // 光标压在窗口里 (40, 30) 处，也就是原图 (39, 29) 那个像素上。
        let cursor = (position.0 + 40, position.1 + 30);
        let anchor_before =
            image_point_at_client(image_width, image_height, 1.0, border, (40, 30));
        assert_eq!(anchor_before, Some((39, 29)));

        let zoom_after = 3.0;
        let moved = zoom_with_cursor(
            1.0,
            zoom_after,
            position,
            image_width,
            image_height,
            border,
            cursor,
        );
        // 放大 3 倍：窗口左上角要往左上退，好让那个像素留在光标底下。
        let cursor_client_after = (cursor.0 - moved.0, cursor.1 - moved.1);
        let anchor_after = image_point_at_client(
            image_width,
            image_height,
            zoom_after,
            border,
            cursor_client_after,
        );
        assert_eq!(
            anchor_after, anchor_before,
            "光标底下的原图像素在缩放前后必须是同一个"
        );
    }

    #[test]
    fn zooming_around_a_border_cursor_leaves_the_window_where_it_was() {
        // 光标在边框上，没有可以钉住的像素，退化成"左上角不动"。
        let position = (100, 100);
        let cursor = (100, 100);
        let moved = zoom_with_cursor(1.0, 2.0, position, 50, 50, 1, cursor);
        assert_eq!(moved, position);
    }

    #[test]
    fn image_point_outside_the_image_is_none() {
        // 边框上、图像外都不该给出锚点。
        assert_eq!(image_point_at_client(10, 10, 1.0, 1, (0, 5)), None);
        assert_eq!(image_point_at_client(10, 10, 1.0, 1, (5, 0)), None);
        assert_eq!(image_point_at_client(10, 10, 1.0, 1, (11, 5)), None);
        assert_eq!(image_point_at_client(10, 10, 1.0, 1, (5, 11)), None);
        assert_eq!(image_point_at_client(10, 10, 1.0, 1, (5, 5)), Some((4, 4)));
    }

    /// 从原图重新采样，而不是从上一帧的缩放结果再缩放。
    ///
    /// 这是 §6.1 T10 钉的那条：整数倍缩放来回走一趟必须**逐像素**回到原来那张图。
    /// 如果实现改成"缩放当前缓冲"，这里的每个像素都会被重采样两次，值就不等了。
    #[test]
    fn integer_zoom_there_and_back_is_pixel_exact() {
        let (image_width, image_height) = (4u32, 3u32);
        let image = image_of(image_width, image_height);
        let border = 1;

        let doubled = window_size(image_width, image_height, 2.0, border);
        assert_eq!(doubled, (10, 8));
        // 给 `paint_pin` 的目标尺寸必须和 `window_size` 用的是同一个算式，现在合成
        // 一个函数取。旧写法把整窗尺寸（10x8）当目标尺寸传进去，于是 `factor_x` 变成
        // 10/4=2.5，方块落在客户端 x=1,2 而不是 3,4——这条测试因此断言错了三处。
        let doubled_scaled = (
            scaled_dimension(f64::from(image_width), 2.0),
            scaled_dimension(f64::from(image_height), 2.0),
        );
        assert_eq!(doubled_scaled, (8, 6));
        let mut buffer = vec![0u32; (doubled.0 * doubled.1) as usize];
        paint_pin(
            &mut buffer,
            doubled,
            border,
            &image,
            image_width,
            image_height,
            2.0,
            TEST_DPI,
        );
        // 每个源像素铺成 2x2 的方块：这是最近邻在整数倍下的定义。窗口 10x8、边框 1，
        // 所以内容从客户端 (1,1) 起，图像第 (col,row) 个像素占客户端
        // `x = 1+2*col ..= 2+2*col`、`y = 1+2*row ..= 2+2*row`。
        //
        // 断言写成"客户端坐标 → 缓冲下标"由下面这个闭包算，**不要手写 `3*10+3` 这种
        // 算式**：这张图里手写算式连着错了三次（先忘了边框、又把 `x=2` 当成边框、
        // 最后把一个乘法算错），而缓冲布局本身是想验证的东西，不该由手工算术来核对。
        let at = |x: usize, y: usize| -> u32 { buffer[y * doubled.0 as usize + x] };
        for row in 0..3usize {
            for col in 0..4usize {
                let expected = image[row * 4 + col];
                for dy in 0..2usize {
                    for dx in 0..2usize {
                        let (x, y) = (1 + 2 * col + dx, 1 + 2 * row + dy);
                        assert_eq!(
                            at(x, y),
                            expected,
                            "客户端 ({x},{y}) 应当是图像 ({col},{row}) 的 {}",
                            format_args!("{expected:08X}")
                        );
                    }
                }
            }
        }
        // 外面一圈必须是边框：内容区正好是 x=1..8、y=1..6。
        for x in 0..10usize {
            assert_eq!(at(x, 0), BORDER_COLOR, "顶边 x={x}");
            assert_eq!(at(x, 7), BORDER_COLOR, "底边 x={x}");
        }
        for y in 0..8usize {
            assert_eq!(at(0, y), BORDER_COLOR, "左边 y={y}");
            assert_eq!(at(9, y), BORDER_COLOR, "右边 y={y}");
        }

        // 再按 1.0 倍画回一张同样大的缓冲，应当逐像素等于原图。
        let single = window_size(image_width, image_height, 1.0, border);
        let single_scaled = (
            scaled_dimension(f64::from(image_width), 1.0),
            scaled_dimension(f64::from(image_height), 1.0),
        );
        assert_eq!(single_scaled, (4, 3));
        let mut back = vec![0u32; (single.0 * single.1) as usize];
        paint_pin(
            &mut back,
            single,
            border,
            &image,
            image_width,
            image_height,
            1.0,
            TEST_DPI,
        );
        for row in 0..image_height as usize {
            for column in 0..image_width as usize {
                let index = (row + 1) * single.0 as usize + column + 1;
                assert_eq!(
                    back[index],
                    image[row * image_width as usize + column],
                    "({column},{row}) 回到 1.0 倍时应当与原图逐像素相同"
                );
            }
        }
    }

    #[test]
    fn scaled_dimension_never_collapses_or_overflows() {
        // 无论怎么缩，一个方向上至少留 1 个像素——0 会让 `LayeredBuffer::new` 报错。
        assert_eq!(scaled_dimension(1.0, ZOOM_MIN), 1);
        assert_eq!(scaled_dimension(0.0, 1.0), 1);
        // 4K 图放到 10 倍是 38400，离 `i32` 还差得远——这条断言留在这里是为了钉住
        // "算式不能回绕"，而不是说这个窗口会被建出来：真正的上限是内存（38400² × 4
        // 字节的 DIB 不可能分配）。
        assert_eq!(scaled_dimension(3840.0, ZOOM_MAX), 38_400);
        // 真到溢出才夹住：`i32::MAX` 之上不许回绕成负数。
        assert_eq!(scaled_dimension(1.0e9, ZOOM_MAX), i32::MAX);
        // 正常情况按四舍五入。
        assert_eq!(scaled_dimension(10.0, 1.1), 11);
        assert_eq!(scaled_dimension(100.0, 1.5), 150);
    }

    #[test]
    fn window_size_is_image_times_zoom_plus_border() {
        assert_eq!(window_size(100, 50, 1.0, 1), (102, 52));
        assert_eq!(window_size(100, 50, 2.0, 1), (202, 102));
        assert_eq!(window_size(100, 50, 0.5, 1), (52, 27));
    }

    /// 滚轮坐标的符号扩展。多显示器下副屏在主屏左边，坐标就是负的。
    #[test]
    fn wheel_coordinates_survive_being_negative() {
        // 屏幕坐标 (-100, -50)：低字 -100，高字 -50。
        let lparam = ((-50i64 as u16 as i64) << 16 | (-100i64 as u16 as i64)) as LPARAM;
        assert_eq!(wheel_x(lparam), -100);
        assert_eq!(wheel_y(lparam), -50);
        // 正数也别弄坏。
        let positive = ((300i64) << 16 | 200i64) as LPARAM;
        assert_eq!(wheel_x(positive), 200);
        assert_eq!(wheel_y(positive), 300);
    }

    #[test]
    fn wheel_delta_is_signed() {
        // 向上滚一格 = +120，向下 = -120。`wParam` 的高字是 `i16`。
        assert_eq!(wheel_delta((WHEEL_DELTA as usize) << 16), WHEEL_DELTA);
        assert_eq!(
            wheel_delta(((-WHEEL_DELTA as i64 as u16 as usize) << 16) as usize),
            -WHEEL_DELTA
        );
    }

    #[test]
    fn border_is_at_least_one_pixel() {
        assert!(border_px() >= 1);
    }

    /// 控件上的字是这个功能的**唯一可见证据**：倍数只在那个小标里。
    #[test]
    fn the_badge_spells_the_zoom_as_a_percentage() {
        assert_eq!(badge_label(1.0), "100%");
        assert_eq!(badge_label(0.1), "10%");
        assert_eq!(badge_label(10.0), "1000%");
        assert_eq!(badge_label(1.5), "150%");
        // 步长 0.1 意味着小数点后一位永远够用，四舍五入到整数不丢档。
        assert_eq!(badge_label(2.55), "255%");
        // 倍数是脏值时不显示脏值，显示钳过之后的。
        assert_eq!(badge_label(f64::NAN), "100%");
        assert_eq!(badge_label(99.0), "1000%");
    }

    /// 圆角：四角切掉，正中和四条边的中点都留着。
    #[test]
    fn rounding_cuts_only_the_corners() {
        let (width, height, radius) = (20, 16, 6);
        for (x, y) in [(0, 0), (19, 0), (0, 15), (19, 15), (1, 1), (18, 14)] {
            assert!(
                !inside_rounded(x, y, width, height, radius),
                "({x},{y}) 是圆角该切掉的那块"
            );
        }
        for (x, y) in [(10, 8), (0, 8), (19, 8), (10, 0), (10, 15), (5, 5)] {
            assert!(inside_rounded(x, y, width, height, radius), "({x},{y}) 该留着");
        }
        // 半径被夹到边长以内：2 的半径放到 4x4 上，连 (0,0) 都在内。
        assert!(inside_rounded(0, 0, 4, 4, 0));
    }

    /// 控件落在**图像区**的右上角，不碰窗口边框。
    #[test]
    fn the_badge_sits_in_the_corner_of_the_image_and_not_on_the_border() {
        let (image_width, image_height) = (200u32, 100u32);
        let border = 1;
        let zoom = 3.0;
        let size = window_size(image_width, image_height, zoom, border);
        let image = image_of(image_width, image_height);
        let mut buffer = vec![0u32; (size.0 * size.1) as usize];
        paint_pin(
            &mut buffer,
            size,
            border,
            &image,
            image_width,
            image_height,
            zoom,
            TEST_DPI,
        );

        let at = |x: i32, y: i32| -> u32 { buffer[y as usize * size.0 as usize + x as usize] };
        let style = BadgeStyle::new(TEST_DPI);
        let mask = text_mask(&badge_label(zoom), TEST_DPI).expect("掩码应当能光栅化");
        let badge_width = mask.text_width + style.pad_h * 2;
        let badge_height = mask.height + style.pad_v * 2;
        let right = border + scaled_dimension(f64::from(image_width), zoom);
        let badge_right = right - style.inset;
        let badge_left = badge_right - badge_width;
        let badge_top = border + style.inset;

        // 控件里靠右下的一小片只可能是底色或文字色，两者都不等于那一带的原图像素。
        let mut rendered = 0;
        for y in (badge_top + badge_height / 2)..(badge_top + badge_height) {
            for x in (badge_left + badge_width / 2)..badge_right {
                if at(x, y) != BADGE_BG_COLOR {
                    rendered += 1;
                }
            }
        }
        assert!(rendered > 0, "控件里应当有字（底色之外还有别的像素）");

        // 边框一格不动：控件对齐的是图像区，不是窗口。
        for x in 0..size.0 {
            assert_eq!(at(x, 0), BORDER_COLOR, "顶边 x={x} 不该被控件压掉");
            assert_eq!(at(x, size.1 - 1), BORDER_COLOR, "底边 x={x}");
        }
        for y in 0..size.1 {
            assert_eq!(at(0, y), BORDER_COLOR, "左边 y={y}");
            assert_eq!(at(size.0 - 1, y), BORDER_COLOR, "右边 y={y}");
        }
        assert_eq!(at(size.0 - 1, 0), BORDER_COLOR, "窗口右上角仍是边框");
    }

    /// 图像区小到放不下控件时**整个不画**，不缩不裁。
    #[test]
    fn the_badge_is_skipped_when_the_image_is_too_small() {
        let (image_width, image_height) = (4u32, 3u32);
        let border = 1;
        let size = window_size(image_width, image_height, 1.0, border);
        let image = image_of(image_width, image_height);
        let mut buffer = vec![0u32; (size.0 * size.1) as usize];
        paint_pin(
            &mut buffer,
            size,
            border,
            &image,
            image_width,
            image_height,
            1.0,
            TEST_DPI,
        );

        // 6x5 的窗口里没有一处该是控件底色；整块就是边框加原图。
        assert!(
            !buffer.contains(&BADGE_BG_COLOR),
            "太小的窗口不该画控件，实际画了"
        );
        for row in 0..image_height as usize {
            for column in 0..image_width as usize {
                let index = (row + 1) * size.0 as usize + column + 1;
                assert_eq!(buffer[index], image[row * image_width as usize + column]);
            }
        }
    }

    /// 混色的两端必须精确：覆盖率为 0 时一个比特都不动，为 255 时完全取源色。
    #[test]
    fn coverage_blending_hits_both_ends_exactly() {
        let destination = 0xFF12_3456;
        assert_eq!(over(destination, BADGE_TEXT_COLOR, 0), destination);
        assert_eq!(over(destination, BADGE_TEXT_COLOR, 255), BADGE_TEXT_COLOR);
        // 中间值必须落在两端之间（不越界、不回绕）。
        let middle = over(destination, BADGE_TEXT_COLOR, 128);
        let channel = |pixel: u32, shift: u32| (pixel >> shift) & 0xFF;
        for shift in [0, 8, 16] {
            let value = channel(middle, shift);
            let low = channel(destination, shift).min(channel(BADGE_TEXT_COLOR, shift));
            let high = channel(destination, shift).max(channel(BADGE_TEXT_COLOR, shift));
            assert!((low..=high).contains(&value), "通道 {shift} 越界：{value}");
        }
        assert_eq!(middle >> 24, 0xFF, "alpha 必须留在 255（贴图整窗不透明）");
    }
}
