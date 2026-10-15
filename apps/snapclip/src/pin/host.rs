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
use std::ffi::{c_void, CStr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use anyhow::{anyhow, bail, Context, Result};
use windows_sys::Win32::Foundation::{
    GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    BLACK_BRUSH, BLENDFUNCTION, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET,
    DEFAULT_PITCH, DeleteDC, DeleteObject, DIB_RGB_COLORS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE,
    DT_TOP, DrawTextW, FF_DONTCARE, FillRect, GetDC, GetStockObject, GetTextFaceW,
    HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ, OUT_TT_PRECIS, ReleaseDC, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT, WHITE_BRUSH,
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
    HWND_TOPMOST, IDC_ARROW, MSG, SM_CXDRAG, SM_CYDRAG, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, ULW_ALPHA, WM_APP, WM_DPICHANGED, WM_HOTKEY, WM_LBUTTONDBLCLK,
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

/// 贴图被移到另一块缩放的显示器上时打一行日志。
///
/// 这条路的验证只能靠实跑：`WM_DPICHANGED` 是系统对"窗口跨屏"的回应，单测里造不出
/// 真的窗口和真的显示器，所以留一行日志让跨屏那一下有据可查——旧 DPI、新 DPI、重画
/// 后的缓冲尺寸与窗口位置，四个数一摆就能看出边框和控件有没有跟着换。
const DPI_TRACE: bool = true;

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
    /// 本窗口所在显示器的 DPI。
    ///
    /// 它属于**窗口**而不是进程：贴图可以被拖到另一块不同缩放的显示器上，那时窗口的物理
    /// 尺寸、边框宽度、控件的字号与圆角半径都得整套换掉；用进程级的值，贴在 150% 副屏上
    /// 的贴图会拿主屏的 DPI 去排版。取它用 `GetDpiForWindow`，更新靠 `WM_DPICHANGED`
    /// （见 [`on_message`] 里那一段），`GetDpiForSystem()` 只回答主显示器的值。
    dpi: u32,
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
    // 这里的 DPI 只是一个**临时**值，用来给 `CreateWindowExW` 一个不至于为 0 的尺寸：
    // 窗口还不存在，问不到它落在哪块显示器上。真正的尺寸在 `WM_NCCREATE` 里按
    // `GetDpiForWindow` 重算，而那一趟会连同第一帧画面一起提交，所以这个猜测永远
    // 不会被人看见。
    let border = border_px(system_dpi());
    let (width, height) = window_size(image.width(), image.height(), 1.0, border);
    if width <= 0 || height <= 0 {
        bail!("贴图尺寸非法：{width}x{height}");
    }

    let mut seed = WindowSeed {
        id,
        image,
        position,
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
        WM_DPICHANGED => {
            // 窗口被拖到了另一块缩放的显示器上（或者那块屏的缩放被改了）。边框宽度、
            // 控件字号与圆角半径都是从 DPI 算出来的，所以这里不是"调一个字号"，而是
            // 整块重画——重画路径就是缩放那条（[`repaint_window`]），区别只在输入换成了
            // 新的 DPI。
            let Some(window) = (unsafe { window_mut(host, hwnd) }) else {
                return 0;
            };
            // 新 DPI 在 `wParam` 的高字里，系统在发这条消息之前就已经把它应用了。
            let dpi = ((wparam >> 16) & 0xFFFF) as u32;
            if dpi == 0 || dpi == window.dpi {
                return 0;
            }
            let previous = window.dpi;
            window.dpi = dpi;
            // `lParam` 是系统按新 DPI 算出的建议矩形：**位置采纳，尺寸不采纳**。尺寸由
            // `window_size` 从倍数与边框算出来，系统并不知道我们的边框与控件长什么样。
            let suggested = unsafe { *(lparam as *const RECT) };
            let position = (suggested.left, suggested.top);
            match unsafe { repaint_window(hwnd, window, position) } {
                Ok(scaled) => {
                    if DPI_TRACE {
                        eprintln!(
                            "[snapclip-app][pin] dpi id={} {previous} -> {dpi} buffer={}x{} at=({}, {})",
                            window.id, scaled.0, scaled.1, position.0, position.1
                        );
                    }
                }
                Err(err) => {
                    eprintln!("[snapclip-app][pin] 换 DPI 重画失败 id={}：{err:#}", window.id);
                }
            }
            0
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
                let border = border_px(window.dpi);
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
                // 位置、尺寸、新画面交给**同一次** `UpdateLayeredWindow`。拆成"先挪窗口
                // 再提交画面"会在中间留下一帧"新尺寸 ＋ 上一档的画面"，也就是用户看到
                // 的闪烁。理由写在 `repaint_window` 上。
                let started = now_us();
                let scaled = match unsafe { repaint_window(hwnd, window, moved) } {
                    Ok(scaled) => scaled,
                    Err(err) => {
                        eprintln!("[snapclip-app][pin] 缩放失败 id={}：{err:#}", window.id);
                        return 0;
                    }
                };
                if ZOOM_TRACE {
                    // 这条记录要能回答"闪烁发生在哪一步"，所以它不只报我们想要的数，
                    // 还报**系统当前实际状态**：窗口真的在哪、`WS_EX_LAYERED` 还在不在。
                    // 前者与 `window.position` 不等，说明有一条移动窗口的路没记账（锚点
                    // 会偏）；后者掉了，窗口就不再是分层的，重画会整块闪。
                    // 这条记录还要能回答"一档缩放贵不贵"。`apply_us` 是**整档**的成本：
                    // 重建缓冲、重采样整幅图、提交 `UpdateLayeredWindow`。缓冲大小随倍数
                    // 的平方涨（1405x1137 放到 5.6× 就是 7872x6371），所以高倍档上这条
                    // 是判断"该不该改成只画可见区域"的唯一依据。
                    let origin = window_origin(hwnd);
                    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
                    eprintln!(
                        "[snapclip-app][pin] zoom id={} zoom={zoom:.4} delta={delta} cursor=({}, {}) \
                         want=({}, {}) origin=({}, {}) buffer={}x{} layered={} apply_us={}",
                        window.id,
                        cursor.0,
                        cursor.1,
                        window.position.0,
                        window.position.1,
                        origin.0,
                        origin.1,
                        scaled.0,
                        scaled.1,
                        style & WS_EX_LAYERED != 0,
                        now_us().saturating_sub(started),
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
    let dpi = window_dpi(hwnd);
    let border = border_px(dpi);
    // `seed.size` 是创建窗口时算的临时尺寸（那一刻还没有窗口，见 `create_pin_window`），
    // 这里不采纳：窗口现在有了，尺寸按**它自己的**显示器 DPI 重算一遍。两处不一致是
    // 正常的——贴在副屏上时本来就该不一样。重算的结果连同第一帧画面在下面一次
    // `UpdateLayeredWindow` 里提交，中间没有一帧是错尺寸。
    let image_width = seed.image.width();
    let image_height = seed.image.height();
    let size = window_size(image_width, image_height, 1.0, border);
    let mut framebuffer = LayeredBuffer::new(size.0, size.1, true)?;
    paint_pin(
        framebuffer.pixels_mut(),
        size,
        border,
        seed.image.pixels(),
        image_width,
        image_height,
        1.0,
        dpi,
    );
    framebuffer
        .submit(hwnd, Some(seed.position))
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
            dpi,
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

/// 窗口现在**真的**在哪（屏幕坐标）。
///
/// 与 `PinWindowState::position` 的区别是这条问的是系统：那个字段是我们自己的记账，
/// 只在拖动与缩放时更新。两者对不上，说明有一条移动窗口的路没记账，缩放的光标锚点
/// 就会偏——这条日志的用途就是让这种偏差现形（`docs/34` §6.2 里"窗口位置靠记账"
/// 那条已知不覆盖，靠它兜底）。
fn window_origin(hwnd: HWND) -> (i32, i32) {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe { GetWindowRect(hwnd, &mut rect) };
    (rect.left, rect.top)
}

/// 重画整个窗口：按窗口现在的倍数与 DPI 重算尺寸、重画缓冲、再把新位置、新尺寸与
/// 新画面**一次**交上去。
///
/// 两个触发点：换了一档缩放（`WM_MOUSEWHEEL`），和窗口被拖到了另一块 DPI 不同的
/// 显示器上（`WM_DPICHANGED`）。两者的共同点是"决定尺寸的输入变了"，所以它们共用
/// 同一条重画路径——尺寸、边框、控件排版全是从 `window.zoom` 与 `window.dpi` 算出来的
/// 纯函数，换了输入就整块重建，不留"这一处改了、那一处忘了"的缝。
///
/// 缩放没变的那些消息（拖动、单击、鼠标进出）一律不碰缓冲，拖动因此仍然是纯
/// `SetWindowPos`——"别把无关的活放进高频路径"是拖动不跟手那条故障的反向教训。
///
/// 返回缩放后的整窗尺寸（供日志）。
///
/// **"一次到位"是这里唯一不能改的东西**，它是闪烁的根因（见 `docs/34` §4.2.2）：
///
/// 1. 新缓冲先在旁边建好、画满——此时屏幕上一个像素都没动，失败就原样返回。
/// 2. `UpdateLayeredWindow` **一次**给出新位置、新尺寸与新画面。它比较的是
///    "这份缓冲的尺寸"，而 `SetWindowPos` 只知道自己给的数字——两者分成两次调用时，
///    中间那一帧是"新尺寸 ＋ 上一档的画面"（缩放围绕光标移动窗口，位置几乎每档都
///    变，所以连位置都是错的）。缩放每档都重建整块缓冲，本来就要调
///    `UpdateLayeredWindow`，位置与尺寸因此是免费附带的，不需要第二次窗口操作。
///
/// 拖动**不**走这条路：拖动不改尺寸，逐条 `WM_MOUSEMOVE` 只做一次 `SetWindowPos`
/// （见 `docs/34` §4.2），把重画放进高频路径正是"拖动不跟手"那条故障的反面。
///
/// # Safety
///
/// 同 [`window_mut`]：`window` 必须是宿主线程 `HostState` 里仍然活着的一个元素，
/// 而且调用期间没有别人同时可变借用那个 `HostState`。
unsafe fn repaint_window(
    hwnd: HWND,
    window: &mut PinWindowState,
    position: (i32, i32),
) -> Result<(i32, i32)> {
    let dpi = window.dpi;
    let border = border_px(dpi);
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
        dpi,
    );

    framebuffer
        .submit(hwnd, Some(position))
        .context("提交重画后的贴图内容失败")?;
    window.position = position;

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
/// 缩放走的是**重采样**（[`plan_axis`]），不是"取最近的源像素"。取最近在缩放的两个
/// 方向上都错：放大时每个源像素被铺成一个方块，斜线成了楼梯（用户报的就是这个，
/// 「放大后的锯齿感很明显」）；缩小时直接把中间的源像素丢掉，高频细节变成摩尔纹。
/// 代价是重采样会让细节略微变软——但软了还认得出是什么字，锯齿就认不出了。
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

    let border = border.max(0) as usize;
    let source_width = image_width as usize;
    let source_height = image_height as usize;
    // 目标尺寸走的是和 `window_size` 同一个算式，不是调用方另算一遍的 `size`。
    let target_width = scaled_dimension(f64::from(image_width), zoom).max(0) as usize;
    let target_height = scaled_dimension(f64::from(image_height), zoom).max(0) as usize;

    if width == 0
        || height == 0
        || border >= width
        || border >= height
        || target_width == 0
        || target_height == 0
        || source_width == 0
        || source_height == 0
    {
        // 窗口小到放不下图像，那就整块都是边框。
        buffer.fill(BORDER_COLOR);
        return;
    }

    let drawn_width = target_width.min(width - border);
    let drawn_height = target_height.min(height - border);

    fill_border(buffer, (width, height), border, (drawn_width, drawn_height));

    // 两条轴各自算一次抽头表，再在 `resample_image` 里取叉积。抽头表按 `target_*` 建，
    // 这里只用它前面 `drawn_*` 个：被窗口裁掉的那些目标像素的抽头不需要。
    resample_image(
        buffer,
        width,
        (border, border),
        image,
        (source_width, source_height),
        (drawn_width, drawn_height),
        &plan_axis(source_width, target_width),
        &plan_axis(source_height, target_height),
    );

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

/// 把边框色铺在图像区**之外**的那几圈上。
///
/// 不是 `buffer.fill`：图像区紧接着就会被整块重写，而在高倍档上那块缓冲是 200 MB 量级
/// （1405x1137 放到 5.6× 就是 7870x6369），白填一遍等于把写带宽送给马上被覆盖的像素。
/// 这是个实测出来的量：最近邻时代它只是浪费，改成重采样之后每一毫秒都要自己争。
fn fill_border(buffer: &mut [u32], size: (usize, usize), border: usize, image: (usize, usize)) {
    let (width, height) = size;
    let left = border.min(width);
    let right = (border + image.0).min(width);
    let top = border.min(height);
    let bottom = (border + image.1).min(height);

    // 上边与下边整宽；只补这两条，其余行交给图象两侧那两条。
    for row in (0..top).chain(bottom..height) {
        let start = row * width;
        buffer[start..start + width].fill(BORDER_COLOR);
    }
    for row in top..bottom {
        let start = row * width;
        buffer[start..start + left].fill(BORDER_COLOR);
        buffer[start + right..start + width].fill(BORDER_COLOR);
    }
}

/// 定点权重的量程：`WEIGHT_ONE` 表示"权重全在这一个抽头上"。
const WEIGHT_SHIFT: u32 = 16;
const WEIGHT_ONE: u32 = 1 << WEIGHT_SHIFT;

/// 一条轴上的重采样计划：每个目标像素取哪些源像素、各占多少权重。
///
/// 权重是定点数，因为浮点权重会让"平坦颜色重采样之后还是那个颜色"带上一段容差，而它
/// 本该是恒等的；逐目标像素的权重和也就能用整数断言成**恰好** `WEIGHT_ONE`。
struct AxisPlan {
    /// 第 `d` 个目标像素的抽头是 `taps[starts[d]..starts[d + 1]]`。长度是目标像素数
    /// 加一，多出来的那个是结尾，于是取抽头不用再查第二张表。
    starts: Vec<u32>,
    /// 抽头：(源下标, 权重)。同一条轴上的所有目标像素共用这一块。
    taps: Vec<(u32, u32)>,
}

impl AxisPlan {
    fn taps_for(&self, destination: usize) -> &[(u32, u32)] {
        let start = self.starts[destination] as usize;
        let end = self.starts[destination + 1] as usize;
        &self.taps[start..end]
    }
}

/// 给一条轴做重采样计划。
///
/// 滤波器是三角形，也就是双线性（双线性正是它半径 1 的那一档）：放大时半径固定 1 个源
/// 像素，每个目标像素恒定取两个抽头；缩小时半径撑到 `1 / 放大倍数`，抽头数随缩小倍数
/// 变多，但**总工作量仍与源像素数同阶**——目标像素少了，每个多取几个——所以缩得越狠
/// 不会越慢。
///
/// 半径不能小于 1。先前那版取 0.5，结果在正好 2× 时每个采样点都落在某个源像素中心
/// 0.25 个像素以内、够不到相邻那个，于是**又退回成了最近邻**——锯齿照旧。三角形在半径
/// 1 处的权重是 `1 - |t|`，要跨过一个像素间距才真正开始混合。
///
/// 归一化逐目标像素做，而且**必须恰好等于 `WEIGHT_ONE`**：差一个定点单位就足以让平坦
/// 颜色重采样之后出现一道一像素的亮边或暗边。
fn plan_axis(source_length: usize, destination_length: usize) -> AxisPlan {
    if source_length == 0 || destination_length == 0 {
        return AxisPlan {
            starts: vec![0, 0],
            taps: Vec::new(),
        };
    }
    // 恒等轴：一个抽头、满权重。1× 必须逐像素还原，靠浮点凑不出来。
    if source_length == destination_length {
        let mut starts = Vec::with_capacity(destination_length + 1);
        let mut taps = Vec::with_capacity(destination_length);
        for index in 0..destination_length {
            starts.push(index as u32);
            taps.push((index as u32, WEIGHT_ONE));
        }
        starts.push(destination_length as u32);
        return AxisPlan { starts, taps };
    }

    let scale = source_length as f64 / destination_length as f64;
    let radius = scale.max(1.0);

    // 放大（`scale < 1`）：滤波器半径固定在 1 个源像素，也就是双线性。这一档**显式**做成
    // 两个抽头，而不是让它从下面的通用循环里掉出来——"每个目标像素恰好两个抽头"是快路
    // （`resample_magnified`）认的形状，而通用循环在边缘处只会给出一个抽头（够不到第二
    // 个源像素），一个抽头就足以让整条轴退回通用循环：边缘那一个像素在画面上看不出来，
    // 代价却是整幅图慢两三倍。
    //
    // 权重与通用循环算出来的是同一个数（`1 - |source - center|` 在半径 1 处），两端夹取
    // 到同一个下标时两个权重之和仍是满值，于是"两种写法给出同一个像素"是成立的。
    if scale < 1.0 {
        let mut starts = Vec::with_capacity(destination_length + 1);
        let mut taps = Vec::with_capacity(destination_length * 2);
        let last = (source_length - 1) as f64;
        for destination in 0..destination_length {
            starts.push((destination * 2) as u32);
            let center = (destination as f64 + 0.5) * scale - 0.5;
            let left = center.floor();
            let fraction = center - left;
            let first = left.clamp(0.0, last) as u32;
            let second = (left + 1.0).clamp(0.0, last) as u32;
            let weight = (fraction * f64::from(WEIGHT_ONE)).round() as u32;
            taps.push((first, WEIGHT_ONE - weight));
            taps.push((second, weight));
        }
        starts.push((destination_length * 2) as u32);
        return AxisPlan { starts, taps };
    }

    let mut starts = Vec::with_capacity(destination_length + 1);
    let mut taps = Vec::with_capacity(destination_length * 2);
    // 两个都放在循环外复用：逐目标像素重新分配一次 `Vec` 会让这一步比滤波本身还贵。
    let mut weights: Vec<(u32, f64)> = Vec::new();
    let mut fixed: Vec<u32> = Vec::new();
    for destination in 0..destination_length {
        starts.push(taps.len() as u32);
        // 源像素 `i` 的中心在坐标 `i` 上（不是 `i + 0.5`），目标像素 `d` 的中心落在
        // `(d + 0.5) * scale - 0.5`。两边都按中心对齐，边缘才不会整体漂半个像素。
        let center = (destination as f64 + 0.5) * scale - 0.5;
        let first = (center - radius).ceil() as i64;
        let last = (center + radius).floor() as i64;

        weights.clear();
        let mut sum = 0.0;
        for source in first.max(0)..=last.min(source_length as i64 - 1) {
            let weight = 1.0 - (source as f64 - center).abs() / radius;
            if weight <= 0.0 {
                // 抽头正好落在滤波器的零点上：权重是 0，留着只是白算一遍。
                continue;
            }
            sum += weight;
            weights.push((source as u32, weight));
        }
        if weights.is_empty() || sum <= 0.0 {
            // 到不了这里（半径不小于 1 保证至少够得到一个源像素中心）。真到了宁可退回
            // 最近邻：一个空抽头的目标像素会是一块纯色亮斑，比锯齿刺眼得多。
            let nearest = center.round().clamp(0.0, (source_length - 1) as f64) as u32;
            taps.push((nearest, WEIGHT_ONE));
            continue;
        }

        fixed.clear();
        let mut biggest = 0usize;
        for (index, (_, weight)) in weights.iter().enumerate() {
            if *weight > weights[biggest].1 {
                biggest = index;
            }
            fixed.push((weight / sum * f64::from(WEIGHT_ONE)).round() as u32);
        }
        // 四舍五入的零头全部补给最重的那个抽头，让权重和**恰好**是 `WEIGHT_ONE`。
        // 这里不夹取：最重的抽头至少占 `WEIGHT_ONE / 抽头数`，而零头不超过抽头数的一半，
        // 两边差着好几个数量级。
        let total: i64 = fixed.iter().map(|value| i64::from(*value)).sum();
        fixed[biggest] = (i64::from(fixed[biggest]) + i64::from(WEIGHT_ONE) - total) as u32;
        debug_assert_eq!(
            fixed.iter().map(|value| u64::from(*value)).sum::<u64>(),
            u64::from(WEIGHT_ONE),
        );

        for (index, (source, _)) in weights.iter().enumerate() {
            taps.push((*source, fixed[index]));
        }
    }
    starts.push(taps.len() as u32);
    AxisPlan { starts, taps }
}

/// 把源图按两条轴的重采样计划画进缓冲。
///
/// 两条轴的抽头取叉积：一次缩放要读的源像素就是 `x 抽头 × y 抽头`。权重是两条轴各自的
/// 定点数，乘积的定点量程因此是 `WEIGHT_ONE²`（`u64` 里 32 位）。累加用 `u64`：抽头最多
/// 时（0.1× 缩放）一次加 11×11 个 `255 * 2^32`，离溢出还差五个数量级。
/// 按两条轴的抽头表把图像重采样到 `origin` 处，按目标行切成几段并行做。
///
/// 并行的依据是实测：5.6× 一张 1405x1137 的图，缓冲是 50 Mpx（7870x6369），串行画要
/// 209 ms——用户滚一档就看得出来，而这件事对目标行是**天然可分**的：每段只写自己的行，
/// 读的源图和两张抽头表都是只读的。实测把它压到几十毫秒，比改画质之前的最近邻还快。
///
/// 段的任务很短（一档一次），所以段数取可用核数并封顶；再多的段只会换来调度开销。
fn resample_image(
    buffer: &mut [u32],
    row_pitch: usize,
    origin: (usize, usize),
    image: &[u32],
    source: (usize, usize),
    drawn: (usize, usize),
    x_plan: &AxisPlan,
    y_plan: &AxisPlan,
) {
    if drawn.0 == 0 || drawn.1 == 0 {
        return;
    }
    let first = origin.1 * row_pitch;
    let rows = &mut buffer[first..first + drawn.1 * row_pitch];

    // 行数太少时开线程是净亏：一次 spawn 的开销比那几行本身还贵。
    const PARALLEL_MIN_ROWS: usize = 16;
    let threads = if drawn.1 >= PARALLEL_MIN_ROWS {
        std::thread::available_parallelism().map_or(1, |count| count.get().min(8))
    } else {
        1
    };
    if threads <= 1 {
        resample_band(rows, row_pitch, origin.0, 0, image, source, drawn, x_plan, y_plan);
        return;
    }

    let band = drawn.1.div_ceil(threads);
    std::thread::scope(|scope| {
        let mut first_row = 0usize;
        for chunk in rows.chunks_mut(band * row_pitch) {
            let rows = chunk.len() / row_pitch;
            let origin_x = origin.0;
            let start_row = first_row;
            first_row += rows;
            scope.spawn(move || {
                resample_band(
                    chunk,
                    row_pitch,
                    origin_x,
                    start_row,
                    image,
                    source,
                    (drawn.0, rows),
                    x_plan,
                    y_plan,
                );
            });
        }
    });
}

/// 一段目标行（`buffer` 已经是这一段自己的行）。`first_row` 是这一段在整幅目标里的行号，
/// 用来查 y 轴的抽头。
///
/// 三条路，按抽头表的形状选：`1×` 是整行复制、放大是每个目标像素两个抽头的并行混合、
/// 其余（也就是缩小）走通用循环。
#[allow(clippy::too_many_arguments)]
fn resample_band(
    buffer: &mut [u32],
    row_pitch: usize,
    origin_x: usize,
    first_row: usize,
    image: &[u32],
    source: (usize, usize),
    drawn: (usize, usize),
    x_plan: &AxisPlan,
    y_plan: &AxisPlan,
) {
    if identity_plan(x_plan) && identity_plan(y_plan) {
        resample_identity(buffer, row_pitch, origin_x, first_row, image, source.0, drawn, y_plan);
        return;
    }
    if two_tap_plan(x_plan) && two_tap_plan(y_plan) {
        resample_magnified(
            buffer, row_pitch, origin_x, first_row, image, source.0, drawn, x_plan, y_plan,
        );
        return;
    }
    resample_filtered(
        buffer, row_pitch, origin_x, first_row, image, source, drawn, x_plan, y_plan,
    );
}

/// `1×` 的那条轴：一个抽头、满权重。两条轴都是它时整行搬运就够。
fn identity_plan(plan: &AxisPlan) -> bool {
    let destinations = plan.starts.len().saturating_sub(1);
    plan.taps.len() == destinations
        && (0..destinations).all(|index| plan.taps[index] == (index as u32, WEIGHT_ONE))
}

/// 放大的那条轴：每个目标像素恰好两个抽头。抽头**不必**是相邻的源像素——边缘处会夹取
/// 到同一个下标——所以这里只查形状，不查下标之间的关系。
fn two_tap_plan(plan: &AxisPlan) -> bool {
    let destinations = plan.starts.len().saturating_sub(1);
    plan.taps.len() == destinations * 2
        && (0..=destinations).all(|index| plan.starts[index] == (index * 2) as u32)
}

/// 两条轴都是恒等轴：逐行搬运。
///
/// `first_row` 是这一段在**整幅目标**里的第一行，用来查 y 轴抽头；`buffer` 里的行号则是
/// 从这一段自己算起的，所以写的位置是 `row * row_pitch`。分段并行之后这两个行号不再
/// 相等，混用会让每一段都画到错的行上——而画面只是"某一档缩放画错了"，不报错。
fn resample_identity(
    buffer: &mut [u32],
    row_pitch: usize,
    origin_x: usize,
    first_row: usize,
    image: &[u32],
    source_width: usize,
    drawn: (usize, usize),
    y_plan: &AxisPlan,
) {
    for row in 0..drawn.1 {
        let source_start = y_plan.taps[first_row + row].0 as usize * source_width;
        let destination_start = row * row_pitch + origin_x;
        buffer[destination_start..destination_start + drawn.0]
            .copy_from_slice(&image[source_start..source_start + drawn.0]);
    }
}

/// 混合用的定点权重，满值是 `BLEND_ONE`。权重因此落在 `0..=256`，上限是 9 位而不是更多，
/// 理由见 `blend_pixels`。
///
/// 抽头表里存的是 16 位权重，进这里之前降一档（`(weight + 128) >> 8`）：`(1 << 16) + 128`
/// 右移 8 位正好是 `BLEND_ONE`，两个定点量程因此是严丝合缝对上的。
const BLEND_SHIFT: u32 = 8;
const BLEND_ONE: u32 = 1 << BLEND_SHIFT;

/// 两个像素按定点权重混合，四个通道（含 alpha）在一个 32 位字里并行做完。
///
/// 用的是经典的"两个 16 位通道对"技巧：`0x00FF00FF` 掩出第 0、2 通道，`(pixel >> 8)`
/// 之后再掩同一张掩码得到第 1、3 通道。权重取 9 位是因为 `255 * 256 = 65280` 还塞得进
/// 16 位通道，两个乘积相加（最多也是 `255 * 256`，因为两个权重加起来恰好是 `BLEND_ONE`）
/// 也不越过通道边界，于是四个通道可以一起算而不互相污染。
///
/// 第二个权重由补数得到（`BLEND_ONE - weight`）而不是各自独立舍入：平坦颜色因此是**逐位**
/// 还原，不依赖舍入方向。两个抽头之下，9 位权重的误差上限是 1/512 个色阶，远小于量化
/// 误差本身，这也是它敢降到 9 位的原因；抽头很多的缩小路径不走这里（见 `resample_filtered`）。
#[inline]
fn blend_pixels(first: u32, second: u32, weight_second: u32) -> u32 {
    debug_assert!(weight_second <= BLEND_ONE);
    let weight_first = BLEND_ONE - weight_second;
    let round = 1 << (BLEND_SHIFT - 1);
    let low = (first & 0x00FF_00FF) * weight_first
        + (second & 0x00FF_00FF) * weight_second
        + round;
    let high = ((first >> 8) & 0x00FF_00FF) * weight_first
        + ((second >> 8) & 0x00FF_00FF) * weight_second
        + round;
    ((low >> BLEND_SHIFT) & 0x00FF_00FF) | (((high >> BLEND_SHIFT) & 0x00FF_00FF) << 8)
}

/// 两个抽头的重采样（放大）。
///
/// 两次一维混合（先沿 x 再沿 y）而不是一次二维四点加权：后者要四个像素一起按
/// `weight_x * weight_y` 加权，四通道就得回到 `resample_filtered` 那种逐通道的 64 位
/// 累加。两次一维混合的中间结果会多舍入一次，误差不超过一个色阶，代价换来的是一行里
/// 四个像素的读能被复用。
#[allow(clippy::too_many_arguments)]
fn resample_magnified(
    buffer: &mut [u32],
    row_pitch: usize,
    origin_x: usize,
    first_row: usize,
    image: &[u32],
    source_width: usize,
    drawn: (usize, usize),
    x_plan: &AxisPlan,
    y_plan: &AxisPlan,
) {
    for row in 0..drawn.1 {
        let plan_row = first_row + row;
        let (source_row_0, _) = y_plan.taps[plan_row * 2];
        let (source_row_1, weight_row_1) = y_plan.taps[plan_row * 2 + 1];
        let start_0 = source_row_0 as usize * source_width;
        let start_1 = source_row_1 as usize * source_width;
        let weight_y = (weight_row_1 + (1 << 7)) >> 8;
        let destination_start = row * row_pitch + origin_x;
        for column in 0..drawn.0 {
            let (source_x_0, _) = x_plan.taps[column * 2];
            let (source_x_1, weight_x_1) = x_plan.taps[column * 2 + 1];
            let weight_x = (weight_x_1 + (1 << 7)) >> 8;
            let top = blend_pixels(
                image[start_0 + source_x_0 as usize],
                image[start_0 + source_x_1 as usize],
                weight_x,
            );
            let bottom = blend_pixels(
                image[start_1 + source_x_0 as usize],
                image[start_1 + source_x_1 as usize],
                weight_x,
            );
            buffer[destination_start + column] = blend_pixels(top, bottom, weight_y);
        }
    }
}

/// 通用循环：任意抽头数（缩小时会到十几个）。
///
/// 逐通道用 64 位累加，因为这里的权重是两条轴各 16 位相乘得到的 32 位定点数——抽头多的
/// 时候只有这个精度才够（9 位权重分到十几个抽头上，误差能到几个色阶）。
#[allow(clippy::too_many_arguments)]
fn resample_filtered(
    buffer: &mut [u32],
    row_pitch: usize,
    origin_x: usize,
    first_row: usize,
    image: &[u32],
    source: (usize, usize),
    drawn: (usize, usize),
    x_plan: &AxisPlan,
    y_plan: &AxisPlan,
) {
    const SHIFT: u32 = 2 * WEIGHT_SHIFT;
    const HALF: u64 = 1 << (SHIFT - 1);

    for row in 0..drawn.1 {
        let destination_start = row * row_pitch + origin_x;
        let y_taps = y_plan.taps_for(first_row + row);
        for column in 0..drawn.0 {
            let x_taps = x_plan.taps_for(column);
            let mut channels = [0u64; 4];
            for &(source_y, weight_y) in y_taps {
                let source_row = source_y as usize * source.0;
                for &(source_x, weight_x) in x_taps {
                    let weight = u64::from(weight_x) * u64::from(weight_y);
                    let mut pixel = image[source_row + source_x as usize];
                    for channel in &mut channels {
                        *channel += u64::from(pixel & 0xFF) * weight;
                        pixel >>= 8;
                    }
                }
            }
            let mut pixel = 0u32;
            for (channel, sum) in channels.iter().enumerate() {
                // 权重归一化保证结果落在 0..=255；这里的夹取只是替权重出错兜底。
                let value = ((sum + HALF) >> SHIFT).min(255) as u32;
                pixel |= value << (channel * 8);
            }
            buffer[destination_start + column] = pixel;
        }
    }
}

/// 边框宽度（像素）。按这个窗口所在显示器的 DPI 折算，保证在高 DPI 屏幕上物理宽度一致。
fn border_px(dpi: u32) -> i32 {
    ((BORDER_DIP as f64) * (dpi as f64) / 96.0).round().max(1.0) as i32
}

/// 窗口所在显示器的 DPI。取不到就退到系统 DPI，再不行按 96 算。
///
/// 这是窗口该用的那个 DPI：`WM_DPICHANGED` 之外的所有排版（边框、控件字号、圆角）
/// 都要问它，问 `system_dpi()` 等于假定窗口永远在主显示器上。
fn window_dpi(hwnd: HWND) -> u32 {
    unsafe {
        let Some(proc) = user32_proc(c"GetDpiForWindow") else {
            return system_dpi();
        };
        let get_dpi_for_window: unsafe extern "system" fn(HWND) -> u32 = std::mem::transmute(proc);
        match get_dpi_for_window(hwnd) {
            0 => system_dpi(),
            dpi => dpi,
        }
    }
}

/// 系统 DPI。取不到就按 96 算。
///
/// 只留给"还没有窗口"的那一处（创建贴图之前先算一个临时尺寸）和上面那个兜底。
fn system_dpi() -> u32 {
    unsafe {
        let Some(proc) = user32_proc(c"GetDpiForSystem") else {
            return 96;
        };
        let get_dpi_for_system: unsafe extern "system" fn() -> u32 = std::mem::transmute(proc);
        match get_dpi_for_system() {
            0 => 96,
            dpi => dpi,
        }
    }
}

/// 取 user32 里一个导出函数的地址，取不到返回 `None`。
///
/// `windows-sys` 0.59 没绑定 `GetDpiForSystem` / `GetDpiForWindow` 这两个高 DPI 入口，
/// 只能自己取。两个都在 `user32.dll` 里，所以这一段加载逻辑只写一遍。
unsafe fn user32_proc(name: &CStr) -> Option<*const c_void> {
    // SAFETY: `user32.dll` 在每个进程里都已经加载，`GetModuleHandleW` 只读模块表；
    // `name` 来自调用点的字面量，以 NUL 结尾，满足 `GetProcAddress` 对字符串的要求。
    unsafe {
        let module = GetModuleHandleW(wide("user32.dll").as_ptr());
        if module.is_null() {
            return None;
        }
        GetProcAddress(module, name.as_ptr().cast()).map(|proc| proc as *const c_void)
    }
}

// ─────────────────────────── 缩放倍数控件 ───────────────────────────
//
// 贴图右上角那个显示当前倍数的圆角小标（`docs/34` §4.2.2）。样式照 `magnifier_pass.rs`
// 里放大镜面板的同名控件：14 DIP 的粗体（700，放大镜用的是 `DWRITE_FONT_WEIGHT_BOLD`）、
// 左右 8 DIP、上下 5 DIP 内边距、距角 6 DIP、6 DIP 圆角。尺寸照搬是为了让两个控件看起来
// 是同一个东西；**实现完全不同**——放大镜在 D2D 上画，贴图只能往自己的 `u32` 缓冲里逐像素写。

/// 控件文字的字号（DIP），与放大镜那颗一致。
const BADGE_FONT_DIP: f64 = snapclip_capture::overlay_font::ZOOM_BADGE_FONT_SIZE_DIP as f64;
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
/// 格式**与放大镜那颗一字不差**（`magnifier_pass.rs` 的同名控件）：整档不写小数
/// （`1×`、`2×`、`10×`），半档写一位（`0.5×`、`1.9×`），乘号是 `×` 而不是字母 `x`。
/// 用户对这个控件的要求原话是"文本格式和放大镜框的保持一致，使用 ×"。
///
/// 判定"是不是整档"用文字上的四舍五入，而不是拿档位去算：`1.0`、`2.0` 这些值在
/// 整数十分位上是精确的，`round()` 拿到的就是它自己。
fn badge_label(zoom: f64) -> String {
    let zoom = clamp_zoom(zoom);
    let rounded = zoom.round();
    if zoom >= 10.0 || (zoom - rounded).abs() < 0.01 {
        format!("{rounded:.0}×")
    } else {
        format!("{zoom:.1}×")
    }
}

/// 控件里那行字的光栅化结果：**覆盖率掩码**。
///
/// `coverage` 是每像素 0–255 的覆盖度（不是颜色），尺寸就是文字的像素外框。做成掩码
/// 而不是"已经染好色的位图"，是因为颜色决定权必须留在 [`blend_coverage`] 里：文字色
/// 与底色是两个常量，混色是一次整数乘法，比再建一张彩色位图便宜得多。
/// 一行字光栅化出来的**墨迹掩码**：`coverage` 是笔画盖住每个像素的比例（0–255），
/// `width × height` 恰好是墨迹的包围盒——行与列上的空白都裁掉了。
///
/// 裁成包围盒不是为了省内存（一张掩码几十字节），而是因为控件要按**墨迹**居中：
/// 行框里字的上沿与下沿差多少由 GDI 的字模度量决定，只有量出来才知道。
struct TextMask {
    coverage: Vec<u16>,
    width: i32,
    height: i32,
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

/// 白底黑字位图里一个像素"有多黑"，也就是笔画盖住它的比例（0 = 全白 = 没笔画）。
///
/// 取**三个通道的平均**而不是单看一个：GDI 的字体平滑是 ClearType 档（[`create_badge_face`]），
/// 按子像素分别染色，同一个像素的红绿蓝是三个不同的覆盖率；只读一个通道等于把笔画横向
/// 砍掉一半，边缘看上去就是一格一格的。平均回来正是"这个像素被笔画盖住多少"。
fn ink_coverage(pixel: u32) -> u16 {
    let red = (pixel >> 16) & 0xFF;
    let green = (pixel >> 8) & 0xFF;
    let blue = pixel & 0xFF;
    (255 - (red + green + blue) / 3) as u16
}

/// 这个像素算不算有笔画。
///
/// 判据与"会不会画上去"必须是同一条：`blend_coverage` 画的是所有覆盖率非零的像素，
/// 这里就得把非零的都算进包围盒。两边用不同的阈值（比如这里漏掉覆盖率 1 的那一圈），
/// 包围盒就比真正画上去的窄一格，按它居中会**随机**偏一个像素——取决于那圈极浅的
/// 抗锯齿边缘落在哪一侧。
fn has_ink(pixel: &u32) -> bool {
    ink_coverage(*pixel) > 0
}

/// 控件用的字体：按覆盖层的族序逐个问 GDI，并把真正选中的族名**读回来核对**。
///
/// GDI 不会因为族名不认识而失败，它会静默替换一个（本机既不注册内嵌子集、又没有那几个界面
/// 族时，"HarmonyOS Sans SC" 会变成 MS Sans Serif）。所以填进去的名字什么也证明不了：
/// `CreateFontW` 之后必须用 `GetTextFaceW` 把 DC 里实际选中的名字读回来，对上了才认。
///
/// 返回的字体**已经**选进 `memory_dc`，连它替换掉的那个对象一起交出去；恢复与释放由调用方
/// 收尾。最后一个族无条件接受：能走到那一步说明读回校验本身在这个环境里失灵（族名被本地化、
/// 或 API 被挂钩），此时画一个替身字体总比整颗控件消失好。
unsafe fn select_badge_font(memory_dc: HDC, font_height: i32) -> Option<(HFONT, HGDIOBJ)> {
    // 覆盖层那颗在渲染器启动时注册过，这里再调一次是幂等的——但贴图可以独立于覆盖层存在
    // （只开贴图不开截图），所以不能指望别人先注册。
    snapclip_capture::overlay_font::register();
    let families = snapclip_capture::overlay_font::FAMILIES;
    unsafe {
        for (index, family) in families.iter().enumerate() {
            let font = create_badge_face(font_height, family);
            if font.is_null() {
                continue;
            }
            let previous = SelectObject(memory_dc, font.into());
            if index + 1 == families.len()
                || face_name(memory_dc).is_some_and(|name| name == *family)
            {
                return Some((font, previous));
            }
            SelectObject(memory_dc, previous);
            DeleteObject(font.into());
        }
    }
    None
}

/// `CreateFontW` 的固定参数：字号、字重、质量档一处写死，族名是唯一的变量。
unsafe fn create_badge_face(font_height: i32, family: &str) -> HFONT {
    unsafe {
        CreateFontW(
            font_height,
            0,
            0,
            0,
            // 与放大镜那颗倍数的 `DWRITE_FONT_WEIGHT_BOLD` 同档
            // （`magnifier_pass.rs` 里 `info_text_format_mut(false, ZOOM_BADGE_FONT_SIZE_DIP, …BOLD, true)`）。
            // 这里曾经是 `FW_SEMIBOLD`（600）：字号、内边距、圆角都照搬了，唯独笔画比放大镜
            // 那颗细一档，摆在一起看就不是同一个控件（用户 m07964）。
            snapclip_capture::overlay_font::ZOOM_BADGE_FONT_WEIGHT,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_TT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            // 要**系统默认的那一档**（ClearType），不是灰度反锯齿。
            //
            // 两个原因，都是量出来的（144 DPI、`1.9×`、字号 21；判据与数字都在
            // `the_badge_text_weighs_the_same_as_on_the_badge_background` 里，跑它加
            // `--nocapture` 会把这几个数打出来）：
            // ClearType 一档不 hinting、笔画按字体的**自然宽度**排（掩码 43x17、有墨迹 324
            // 像素），灰度档会把笔画往格子上吸、吸细一圈（42x17、289 像素，少 12%）；
            // 而且 ClearType 的覆盖率自带 gamma 校正，直接线性混到深底上与"GDI 自己把白字
            // 画在这个底色上"几乎逐像素同重（1.004x），灰度档只有 0.91x——白字压深底本来
            // 就显细，再让掉 gamma 就更细。
            //
            // 覆盖率仍然要按三通道平均读（[`ink_coverage`]）：ClearType 是**子像素**染色，
            // 一个像素的红绿蓝是三个不同的覆盖率，平均回来才是"这个像素被笔画盖住多少"。
            CLEARTYPE_QUALITY as u32,
            DEFAULT_PITCH as u32 | FF_DONTCARE as u32,
            wide(family).as_ptr(),
        )
    }
}

/// `memory_dc` 里**真正**选中的字体族名：`CreateFontW` 要的是一个愿望，这个是结果。
///
/// `GetTextFaceW` 返回的字符数含结尾的 NUL，减 1 才是名字长度；读不出来时返回 `None`，调用
/// 方据此认为"没对上"。
unsafe fn face_name(memory_dc: HDC) -> Option<String> {
    let mut buffer = [0u16; 64];
    let count = unsafe { GetTextFaceW(memory_dc, buffer.len() as i32, buffer.as_mut_ptr()) };
    if count <= 0 {
        return None;
    }
    let length = (count as usize - 1).min(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..length]))
}

/// 用 GDI 把一行字画进一张**白底黑字**的位图，再把"有多黑"当成覆盖率读出来
/// （[`ink_coverage`]）。
///
/// 白底黑字是刻意的：GDI 会把反锯齿的中间值算进像素颜色里，而"黑色有多少"正好就是
/// "笔画的覆盖率有多少"。若反过来画白字黑底，颜色与覆盖率会混在同一个通道里分不开。
///
/// 掩码是全黑的（覆盖率 0）也不当失败——那句话只是"字没渲染出来"，控件会显示成一个
/// 空的圆角色块，缩放本身照常工作。为一个装饰性的小标把 F6 搞失败是不划算的。
fn rasterize_text(text: &str, dpi: u32) -> Option<TextMask> {
    // 字号与控件高度用同一个折算函数：两边各算一遍，迟早会算出两个不一样的像素数。
    let height = scale_dip(BADGE_FONT_DIP, dpi);
    let font_height = -height;
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
            // 字体族与字重都照搬放大镜那颗倍数控件：它走的是内嵌的 `HarmonyOS Sans SC`
            // （`overlay_font::FAMILY`），贴图这边得问 GDI 要同一个族。GDI 不会因为族名不认识
            // 而失败——它会**悄悄换一个**——所以"要到了没有"只能问回 DC（见
            // `select_badge_font`），光把名字填进去证明不了任何事。
            let selected = select_badge_font(memory_dc, font_height);
            let mask = if let Some((font, previous_font)) = selected {
                let white = GetStockObject(WHITE_BRUSH);
                let previous_brush = SelectObject(memory_dc, white);
                SetBkMode(memory_dc, TRANSPARENT as i32);
                SetTextColor(memory_dc, 0x0000_0000);

                // 行框留一倍余量：字在行框里的落点由 GDI 的字模度量决定（实测基线与行框
                // 下沿齐平），行框高度正好取字号时笔画的最后一行会被切在 DIB 外面。多画
                // 几行、量出墨迹再裁掉，比假设它一定放得下可靠。
                let lines = font_height.unsigned_abs() * 2;
                let width = (dpi.max(96) * 3) as i32;
                let mut info: BITMAPINFO = std::mem::zeroed();
                info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                info.bmiHeader.biWidth = width;
                // **负高度 = 自顶向下**，也就是缓冲的第 0 行是屏幕上的最上面一行。
                //
                // 这里必须是负的：下面把 `bits` 当连续的行读，而正高度（自下向上）的
                // DIB 第 0 行是**图的底行**，读出来的字会**上下颠倒**。曾经写成
                // `-font_height`，而 `font_height` 本身就是负的，两个负号抵消成了
                // 正高度，于是贴图右上角的倍数控件一直是倒着的。
                info.bmiHeader.biHeight = -(lines as i32);
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
                        bottom: lines as i32,
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
                    let rows = lines as usize;
                    let source = std::slice::from_raw_parts(bits as *const u32, stride * rows);

                    // 掩码裁成**墨迹的包围盒**：先量出字占了哪几行，再只在这几行上量它
                    // 占哪几列。控件要按墨迹居中（见 `composite_badge`），而字在行框里
                    // 上沿离顶差多少完全由 GDI 的字模度量决定——那是量出来的，不是算出来的。
                    let mut ink_top = rows;
                    let mut ink_bottom = rows;
                    for (index, row) in source.chunks_exact(stride).enumerate() {
                        if row.iter().any(has_ink) {
                            if ink_top == rows {
                                ink_top = index;
                            }
                            ink_bottom = index + 1;
                        }
                    }
                    let ink_height = ink_bottom.saturating_sub(ink_top);

                    let mut ink_left = stride;
                    let mut ink_right = 0usize;
                    for row in source.chunks_exact(stride).skip(ink_top).take(ink_height) {
                        if let Some(first) = row.iter().position(has_ink) {
                            ink_left = ink_left.min(first);
                        }
                        if let Some(last) = row.iter().rposition(has_ink) {
                            ink_right = ink_right.max(last + 1);
                        }
                    }
                    // 一个字都没渲染出来时让区间空掉，而不是留下 `[stride, 0)` 这种反区间。
                    let ink_left = ink_left.min(ink_right);
                    let text_width = ink_right - ink_left;
                    let mut coverage = Vec::with_capacity(text_width * ink_height);
                    for row in source.chunks_exact(stride).skip(ink_top).take(ink_height) {
                        for pixel in &row[ink_left..ink_right] {
                            coverage.push(ink_coverage(*pixel));
                        }
                    }

                    SelectObject(memory_dc, previous_bitmap);
                    DeleteObject(bitmap.into());
                    Some(TextMask {
                        coverage,
                        width: text_width as i32,
                        height: ink_height as i32,
                    })
                };
                SelectObject(memory_dc, previous_brush);
                SelectObject(memory_dc, previous_font);
                DeleteObject(font.into());
                mask
            } else {
                None
            };
            DeleteDC(memory_dc);
            mask
        })();
        ReleaseDC(std::ptr::null_mut(), screen_dc);
        result
    }
}

/// 把一个 DIP 长度折成本机像素，至少 1（免得在高 DPI 下折成 0）。
fn scale_dip(dip: f64, dpi: u32) -> i32 {
    let scale = f64::from(dpi.max(96)) / 96.0;
    (dip * scale).round().max(1.0) as i32
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
        let scale_dip = |dip: f64| scale_dip(dip, dpi);
        Self {
            pad_h: scale_dip(BADGE_PAD_H_DIP),
            pad_v: scale_dip(BADGE_PAD_V_DIP),
            inset: scale_dip(BADGE_INSET_DIP),
            radius: scale_dip(BADGE_RADIUS_DIP),
        }
    }
}

/// 控件在整窗缓冲里的落点：它要画的字模，以及 `(left, top, width, height)`。
///
/// `None` 表示这一档放不下控件、整个不画（让位规则见 [`composite_badge`]）。宽度由字模
/// 量出来的文字宽度决定，所以落点和字模只能一起算出来——这也正是它要能被问到的原因：
/// 控件压在图像上，任何"整幅图像逐像素"的断言都必须先把这个矩形让出去。
fn badge_placement(
    image_width: u32,
    image_height: u32,
    zoom: f64,
    border: i32,
    dpi: u32,
) -> Option<(Arc<TextMask>, (i32, i32, i32, i32))> {
    let style = BadgeStyle::new(dpi);
    let mask = text_mask(&badge_label(zoom), dpi)?;
    let badge_width = mask.width + style.pad_h * 2;
    // 高度取自**字号**而不是量出来的文字高度，就为了和放大镜那颗对齐：那边是
    // `font_sz + pad_v * 2`（`magnifier_pass.rs`），字的实际高矮不参与。所以字是
    // **居中落在这个固定框里**，而不是让框去贴字（见 [`composite_badge`]）。
    let badge_height = scale_dip(BADGE_FONT_DIP, dpi) + style.pad_v * 2;

    // 图像区在整窗缓冲里的范围。
    let right = border + scaled_dimension(f64::from(image_width), zoom);
    let bottom = border + scaled_dimension(f64::from(image_height), zoom);

    let badge_right = right - style.inset;
    let badge_bottom = border + style.inset + badge_height;
    let badge_left = badge_right - badge_width;
    if badge_left < border || badge_bottom > bottom || badge_width <= 0 || badge_height <= 0 {
        return None;
    }
    Some((
        mask,
        (badge_left, border + style.inset, badge_width, badge_height),
    ))
}

/// 把缩放倍数控件画到整窗缓冲的右上角。
///
/// **只覆盖图像区，不碰边框**：控件的右上角对齐的是"图像区右上角往里缩 `inset`"，
/// 而不是窗口的右上角。贴图那圈边框是它与底下的窗口分开的唯一标记，被控件压掉一段
/// 就会看起来像没贴好。
///
/// 让位规则（落点由 [`badge_placement`] 算）：图像区小到放不下控件时**整个不画**，不缩小、
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
    let Some((mask, (badge_left, badge_top, badge_width, badge_height))) =
        badge_placement(image_width, image_height, zoom, border, dpi)
    else {
        return;
    };
    let style = BadgeStyle::new(dpi);

    let row_pitch = size.0 as usize;
    for row in 0..badge_height {
        let destination_row = badge_top + row;
        if destination_row < 0 || destination_row >= size.1 {
            continue;
        }
        for column in 0..badge_width {
            // 圆角边缘按**覆盖率**混合，不是二值的"画/不画"：二值遮罩下四个角的圆弧
            // 只能落在整像素上，那条曲线就必然是一格一格的阶梯，而它正是控件里唯一
            // 一条斜的轮廓——比文字更显眼（字形本身有灰度抗锯齿，圆角没有）。
            let coverage = edge_coverage(rounded_distance(
                column as f32 + 0.5,
                row as f32 + 0.5,
                badge_width as f32,
                badge_height as f32,
                style.radius as f32,
            ));
            if coverage == 0 {
                continue;
            }
            let index = destination_row as usize * row_pitch + (badge_left + column) as usize;
            if let Some(pixel) = buffer.get_mut(index) {
                *pixel = over(*pixel, BADGE_BG_COLOR, coverage);
            }
        }
    }

    // 墨迹在控件里**两个方向都居中**。水平方向本来就是这么算的：控件宽度 = 墨迹宽度
    // + 左右内边距，于是 `(控件宽 - 墨迹宽) / 2` 正好就是 `pad_h`。竖直方向原来把上下
    // 内边距加在**行框**上，而行框里字的落点由 GDI 的字模度量决定——行框比墨迹高一大截、
    // 字又贴在行框下沿，于是字看上去明显偏下（96 DPI 偏 2.5 像素，150% 下偏 4.5 像素）。
    blend_coverage(
        buffer,
        row_pitch,
        badge_left + (badge_width - mask.width) / 2,
        badge_top + (badge_height - mask.height) / 2,
        &mask,
        BADGE_TEXT_COLOR,
    );
}

/// 一个点到圆角矩形边界的**有符号**距离（像素），负值在内部、正值在外部。
///
/// 圆角矩形是"矩形把四个角用半径 `radius` 的圆弧切掉"：到直边的距离是 `|q| - 半长`，
/// 到圆弧的距离则要再减掉半径。把 `q` 的负分量归零再取长度，得到的正是"离最近的角
/// 有多远"，于是同一个式子同时管住了直边与圆弧，不需要分四种情况（旧版 `inside_rounded`
/// 那个逐象限的写法就是为了绕开这一条，而且它只能给出布尔值）。
fn rounded_distance(x: f32, y: f32, width: f32, height: f32, radius: f32) -> f32 {
    let half = (width * 0.5, height * 0.5);
    let radius = radius.min(half.0).min(half.1).max(0.0);
    let qx = (x - half.0).abs() - (half.0 - radius);
    let qy = (y - half.1).abs() - (half.1 - radius);
    let outside = (qx.max(0.0) * qx.max(0.0) + qy.max(0.0) * qy.max(0.0)).sqrt();
    outside + qx.max(qy).min(0.0) - radius
}

/// 有符号距离 → 覆盖率：以边界为中心、1 像素宽的线性斜坡。
///
/// 斜坡宽度取 1 像素是有依据的：采样点在像素中心，一个像素的"面积"就是 1×1，边界穿过
/// 它时覆盖率正好从 0 线性走到 255。距离 `-0.5` 及以内是纯内部（`255`）、`+0.5` 及以外
/// 是纯外部（`0`），所以控件内部的像素值**逐位**等于底色，不会被这条抗锯齿改动。
fn edge_coverage(distance: f32) -> u32 {
    ((0.5 - distance).clamp(0.0, 1.0) * 255.0).round() as u32
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

    /// 把整窗内容提交到窗口，可以直接附带新的位置与尺寸。
    ///
    /// `position` 为 `Some` 时**这一次调用同时完成搬窗口、改尺寸、换画面**，这是缩放
    /// 不闪的唯一办法：`UpdateLayeredWindow` 的 `psize` 本身就是"分层窗口的新尺寸"，
    /// 把尺寸留给 `SetWindowPos` 就会分成两步，而两步之间窗口显示的仍是上一档的画面
    /// （缩小时是"窗口还大、层已经小了"，露出来的那块不在层里）。
    ///
    /// 拖动路径传 `None`：拖动不改尺寸，逐条消息只挪窗口，不该顺手重建一个 `POINT`。
    fn submit(&self, hwnd: HWND, position: Option<(i32, i32)>) -> Result<()> {
        unsafe {
            let size = SIZE {
                cx: self.width,
                cy: self.height,
            };
            let src = POINT { x: 0, y: 0 };
            let destination = position.map(|(x, y)| POINT { x, y });
            let destination = match &destination {
                Some(point) => point,
                None => std::ptr::null(),
            };
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
                destination,
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

/// `GetWindowLongPtrW` 的扩展样式下标。
///
/// `windows-sys` 把 `GWL_EXSTYLE` 定义在 `WINDOW_LONG_PTR_INDEX` 里，但那个类型是
/// `i32` 而 `GetWindowLongPtrW` 收的是 `WINDOW_LONG_PTR_INDEX`——本仓为了少拉一个
/// 导入，和 `GWLP_USERDATA` 一样在这里按数值声明。
const GWL_EXSTYLE: i32 = -20;

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

    /// 控件里的白字压在深灰底上"看起来有多重"，必须与 GDI 直接在同一个底色上画白字一样重。
    ///
    /// 参考那一版刻意写死 `CLEARTYPE_QUALITY`（5）：放大镜那颗控件走 D2D，D2D 的文字也是
    /// ClearType + gamma 校正，所以"GDI 用 ClearType 把白字画在这个底色上"就是它该有的样子。
    /// 生产那一版走 `create_badge_face` 的档位；把它改回灰度反锯齿（4）时这个比值会掉到
    /// 0.91——比 1 少 9%，5% 的门槛就能拦住。加 `--nocapture` 跑它会把量到的数打出来。
    #[test]
    fn the_badge_text_weighs_the_same_as_on_the_badge_background() {
        fn mean_channel(pixel: u32) -> u32 {
            (((pixel >> 16) & 0xFF) + ((pixel >> 8) & 0xFF) + (pixel & 0xFF)) / 3
        }
        fn linear_light(value: u32) -> f64 {
            (value as f64 / 255.0).powf(2.2)
        }
        /// 白字压在深底上时这个像素"看起来有多少墨"：换到线性光里，再按底色归一化。
        fn perceived_ink(pixel: u32, background: u32) -> f64 {
            let low = linear_light(background);
            ((linear_light(mean_channel(pixel)) - low) / (1.0 - low)).max(0.0)
        }
        /// GDI 直接在 `fill` 上画 `text_colour` 的字，返回整幅位图。
        unsafe fn painted(quality: u32, fill: u32, text_colour: u32, text: &str) -> Vec<u32> {
            unsafe {
                let font_height = -scale_dip(BADGE_FONT_DIP, 144);
                let screen_dc = GetDC(std::ptr::null_mut());
                let memory_dc = CreateCompatibleDC(screen_dc);
                let font = CreateFontW(
                    font_height,
                    0,
                    0,
                    0,
                    snapclip_capture::overlay_font::ZOOM_BADGE_FONT_WEIGHT,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET as u32,
                    OUT_TT_PRECIS as u32,
                    CLIP_DEFAULT_PRECIS as u32,
                    quality,
                    DEFAULT_PITCH as u32 | FF_DONTCARE as u32,
                    wide(snapclip_capture::overlay_font::FAMILY).as_ptr(),
                );
                let previous_font = SelectObject(memory_dc, font.into());
                let lines = font_height.unsigned_abs() * 2;
                let width = 432;
                let mut info: BITMAPINFO = std::mem::zeroed();
                info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                info.bmiHeader.biWidth = width;
                info.bmiHeader.biHeight = -(lines as i32);
                info.bmiHeader.biPlanes = 1;
                info.bmiHeader.biBitCount = 32;
                info.bmiHeader.biCompression = BI_RGB;
                let mut bits: *mut c_void = std::ptr::null_mut();
                let bitmap =
                    CreateDIBSection(memory_dc, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
                let previous_bitmap = SelectObject(memory_dc, bitmap.into());
                let stride = width as usize;
                let rows = lines as usize;
                let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, stride * rows);
                pixels.fill(fill);
                SetBkMode(memory_dc, TRANSPARENT as i32);
                SetTextColor(memory_dc, text_colour);
                let mut rect = RECT {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: lines as i32,
                };
                let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
                DrawTextW(
                    memory_dc,
                    wide.as_ptr(),
                    -1,
                    &raw mut rect,
                    DT_LEFT | DT_TOP | DT_SINGLELINE | DT_NOPREFIX,
                );
                let copy = pixels.to_vec();
                SelectObject(memory_dc, previous_bitmap);
                DeleteObject(bitmap.into());
                SelectObject(memory_dc, previous_font);
                DeleteObject(font.into());
                DeleteDC(memory_dc);
                ReleaseDC(std::ptr::null_mut(), screen_dc);
                copy
            }
        }

        let background = BADGE_BG_COLOR & 0xFF;
        let mask = text_mask("1.9×", 144).expect("控件标签总要能光栅化");
        let masked: f64 = mask
            .coverage
            .iter()
            .map(|coverage| {
                let blended = over(BADGE_BG_COLOR, BADGE_TEXT_COLOR, u32::from(*coverage));
                perceived_ink(blended, background)
            })
            .sum();
        let reference: f64 = (unsafe { painted(5, BADGE_BG_COLOR, 0x00FF_FFFF, "1.9×") })
            .iter()
            .map(|pixel| perceived_ink(*pixel, background))
            .sum();
        assert!(
            reference > 100.0,
            "参考那一版得真的画出字来，不然比值没有意义（量到 {reference:.1}）"
        );
        let ratio = masked / reference;
        println!(
            "控件白字压在底色上：掩码 {}x{}、有墨迹 {} 像素、看起来的重量比 {ratio:.3}、参考图上墨迹 {reference:.1}",
            mask.width,
            mask.height,
            mask.coverage.iter().filter(|coverage| **coverage > 0).count(),
        );
        assert!(
            (ratio - 1.0).abs() < 0.05,
            "控件里的白字看起来该与直接在底色上画的字一样重，量到 {ratio:.3}（灰度反锯齿那版是 0.91）"
        );
    }

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

    /// 光栅出来的字形必须**正着**。
    ///
    /// 这条测试守的是一个真实的故障：`CreateDIBSection` 的 `biHeight` 是正数时位图
    /// **自下向上**，而读像素的代码把 `bits` 当成连续的从上到下的行——整行字就反了。
    /// 当时写成 `-font_height`，而 `font_height` 本身是负的，两个负号抵消成正值，
    /// 贴图右上角的倍数控件因此一直是上下颠倒的（用户 m06398 报的就是这个）。
    ///
    /// 判据用 `7`：它的横杠在上、斜尾在下，墨迹明显偏上。96 DPI 下这张掩码是 7x10，
    /// 逐行笔画数是 `[7, 3, 3, 3, 3, 3, 2, 3, 3, 2]`，重心落在第 3.75 行，而中点是第 4.5 行；
    /// 位图一翻过来重心就跑到第 5.25 行。掩码裁到墨迹包围盒之后这条仍然成立——裁掉的
    /// 只是行框里的空白，不是行序。
    ///
    /// **不能**再用"`8×` 与 `8` 的最上一行笔画齐平"那类判据：掩码裁到墨迹之后，两张掩码的
    /// 第一行都必然是有笔画的那一行，等式退化成恒真，倒着也能过。
    #[test]
    fn glyphs_are_rasterized_upright() {
        let mask = text_mask("7", 96).expect("96 DPI 下应当能光栅化");
        let (mut total, mut weighted) = (0u32, 0u32);
        for row in 0..mask.height {
            let start = row as usize * mask.width as usize;
            let ink = mask.coverage[start..start + mask.width as usize]
                .iter()
                .filter(|value| **value > 0)
                .count() as u32;
            total += ink;
            weighted += ink * row as u32;
        }
        assert!(total > 0, "`7` 居然一点笔画都没有");
        // 用整数比，免得为一条重心断言引入浮点：重心在中点之上 <=> 2 * 加权和 < 总数 * (高 - 1)。
        assert!(
            2 * weighted < total * (mask.height as u32 - 1),
            "`7` 的墨迹重心应当偏上（横杠在上、斜尾在下），现在 {}x{} 的掩码里重心在第 {:.2} 行——字形被上下翻过来了",
            mask.width,
            mask.height,
            f64::from(weighted) / f64::from(total)
        );
    }

    /// 控件里的字在**两个方向上都居中**，而且不贴边。
    ///
    /// 量的是**渲染出来的缓冲**，不是内部数字。字形掩码原来只把列裁到墨迹、行留着整个行框，
    /// 而上下内边距是加在行框上的，于是字在竖直方向明显偏下（用户 m07766 报的就是这个：
    /// 96 DPI 偏 2.5 像素、150% 偏 4.5 像素）。判据取"两边的间距相差不超过 1 像素"——
    /// 墨迹尺寸与控件尺寸的差是奇数时，一像素的不对称是像素栅格本身的极限，
    /// 再要求严格相等就只能靠调字号去凑。
    #[test]
    fn the_badge_label_is_centred_in_its_button() {
        const IMAGE: u32 = 0xFF00_8000;
        let (image_width, image_height) = (200u32, 100u32);
        let border = 1;
        for dpi in [96u32, 120, 144, 192] {
            let zoom = 3.0;
            let size = window_size(image_width, image_height, zoom, border);
            let image = vec![IMAGE; (image_width * image_height) as usize];
            let mut buffer = vec![0u32; (size.0 * size.1) as usize];
            paint_pin(
                &mut buffer,
                size,
                border,
                &image,
                image_width,
                image_height,
                zoom,
                dpi,
            );

            let (_, (left, top, width, height)) =
                badge_placement(image_width, image_height, zoom, border, dpi)
                    .expect("这个尺寸下控件应当放得下");
            let style = BadgeStyle::new(dpi);
            let at = |x: i32, y: i32| buffer[y as usize * size.0 as usize + x as usize];

            // 只跳过"同时贴着横边与竖边"的四个角：圆角的圆弧与它的过渡像素也在那里，
            // 混进来量到的就是控件本身而不是字。直边上的像素恰好是底色，不碍事。
            let corner = |x: i32, y: i32| {
                let horizontal = x < left + style.radius || x >= left + width - style.radius;
                let vertical = y < top + style.radius || y >= top + height - style.radius;
                horizontal && vertical
            };
            let mut ink = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
            for y in top..(top + height) {
                for x in left..(left + width) {
                    if !corner(x, y) && at(x, y) != BADGE_BG_COLOR {
                        ink = (ink.0.min(x), ink.1.max(x), ink.2.min(y), ink.3.max(y));
                    }
                }
            }
            assert!(ink.0 != i32::MAX, "{dpi} DPI：控件里一颗字的像素都没有");
            let left_margin = ink.0 - left;
            let right_margin = left + width - 1 - ink.1;
            let top_margin = ink.2 - top;
            let bottom_margin = top + height - 1 - ink.3;
            assert!(
                (left_margin - right_margin).abs() <= 1 && (top_margin - bottom_margin).abs() <= 1,
                "{dpi} DPI：字没居中——左 {left_margin} / 右 {right_margin}，上 {top_margin} / 下 {bottom_margin}"
            );
            assert!(
                left_margin >= 1 && right_margin >= 1 && top_margin >= 1 && bottom_margin >= 1,
                "{dpi} DPI：字贴到控件边上了——左 {left_margin} / 右 {right_margin}，上 {top_margin} / 下 {bottom_margin}"
            );
        }
    }

    /// 放大是**插值**出来的，回到 1× 则必须逐像素还原。
    ///
    /// 这两条一起就是 §6.1 T10 关心的全部。放大后必须出现"原图里根本没有"的中间值：
    /// 实现要是退回成取最近的源像素，缓冲里就只会出现原图里那些值，第一条断言会失败。
    /// 回到 1× 的逐像素还原是另一半——缩放每次都从**原图**重采样，而不是拿上一帧再缩一次。
    #[test]
    fn zooming_up_interpolates_and_one_to_one_is_pixel_exact() {
        let (image_width, image_height) = (4u32, 3u32);
        // 相邻像素差 20 个通道值。不能拿 `image_of` 那个逐像素加一的序列：它相邻只差 1，
        // 插值出来的"中间值"四舍五入之后就是端点本身，断言分不出插值与不插值。
        let image: Vec<u32> = (0..12).map(|index| 0xFF00_0000 | (index * 20 + 10)).collect();
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
        // 断言写成"客户端坐标 → 缓冲下标"由下面这个闭包算，**不要手写 `3*10+3` 这种
        // 算式**：这张图里手写算式连着错了三次（先忘了边框、又把 `x=2` 当成边框、
        // 最后把一个乘法算错），而缓冲布局本身是想验证的东西，不该由手工算术来核对。
        let at = |x: usize, y: usize| -> u32 { buffer[y * doubled.0 as usize + x] };
        // 判据是"和最近邻会画成的样子哪里不同"：2× 时最近邻把每个源像素铺成 2x2 的方块，
        // 重采样则应当在大多数格子上给出别的值。这比"数插值出来的中间值"稳——插出来的值
        // 偶尔正好等于某个源值（斜坡上很常见），"和最近邻不同"没有这种巧合。
        let mut differs = 0usize;
        for y in 0..6usize {
            for x in 0..8usize {
                if at(x + 1, y + 1) != image[(y / 2) * 4 + x / 2] {
                    differs += 1;
                }
            }
        }
        assert!(
            differs >= 40,
            "48 个内容像素里只有 {differs} 个不同于最近邻的结果——说明放大又退回成\
             最近邻（每个源像素铺成一个方块）"
        );
        // 原图是递增的斜坡，插值不会让它回头。
        for y in 1..7usize {
            for x in 1..8usize {
                assert!(
                    at(x, y) <= at(x + 1, y),
                    "第 {y} 行第 {x} 列：插值把递增的斜坡弄回头了"
                );
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

    /// 抽头表的两条硬约束：权重和**恰好**是 `WEIGHT_ONE`，源下标全在范围内。
    ///
    /// 这是重采样里唯一一处能悄悄错掉的地方——差一个定点单位，平坦颜色上就出现一道
    /// 亮边；源下标越界则直接是 `image[...]` panic。两个方向都从 1 试到几百，为的是把
    /// 恒等轴、放大、缩小、以及"目标比源多一个像素"这种边界全扫一遍。
    #[test]
    fn every_destination_pixel_gets_normalized_taps_inside_the_source() {
        for source in [1usize, 2, 3, 7, 64, 600] {
            for destination in [1usize, 2, 3, 8, 64, 599, 600, 601, 4000] {
                let plan = plan_axis(source, destination);
                assert_eq!(plan.starts.len(), destination + 1);
                for index in 0..destination {
                    let taps = plan.taps_for(index);
                    assert!(
                        !taps.is_empty(),
                        "{source}→{destination} 的第 {index} 个像素一个抽头都没有"
                    );
                    let sum: u64 = taps.iter().map(|(_, weight)| u64::from(*weight)).sum();
                    assert_eq!(
                        sum,
                        u64::from(WEIGHT_ONE),
                        "{source}→{destination} 的第 {index} 个像素权重和是 {sum}"
                    );
                    for (source_index, _) in taps {
                        assert!(
                            (*source_index as usize) < source,
                            "{source}→{destination} 取到了范围外的源下标 {source_index}"
                        );
                    }
                }
            }
        }
    }

    /// 平坦颜色经过任何缩放都应当**逐像素**保持原色。
    ///
    /// 这是重采样最容易被破坏的那条不变量：权重和不为 1 会让画面整体偏亮或偏暗，边缘
    /// 抽头被夹取后没有重新归一化则会让四边出现一圈暗边。两者都是"看着差不多、量起来
    /// 差一截"，只有逐像素断言抓得住。
    #[test]
    fn a_flat_colour_survives_every_zoom_exactly() {
        let colour = 0xFF_3C_78_A0u32;
        let (image_width, image_height) = (600u32, 600u32);
        let image = vec![colour; (image_width * image_height) as usize];
        let border = 1;
        // 控件压在图像区右上角，它下面那一带不参与"等于原色"的断言。
        let badge_bottom = border as usize
            + scale_dip(BADGE_FONT_DIP, TEST_DPI) as usize
            + scale_dip(BADGE_PAD_V_DIP, TEST_DPI) as usize * 2
            + scale_dip(BADGE_INSET_DIP, TEST_DPI) as usize
            + 1;

        for zoom in [ZOOM_MIN, 0.37, 1.0, 2.0] {
            let size = window_size(image_width, image_height, zoom, border);
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
            let target = (
                scaled_dimension(f64::from(image_width), zoom) as usize,
                scaled_dimension(f64::from(image_height), zoom) as usize,
            );
            assert!(
                target.1 > badge_bottom + 8,
                "{zoom}× 时图像区只有 {} 像素高，控件底下没剩下可查的行",
                target.1
            );
            for row in badge_bottom..border as usize + target.1 {
                for column in border as usize..border as usize + target.0 {
                    assert_eq!(
                        buffer[row * size.0 as usize + column],
                        colour,
                        "{zoom}× 时客户端 ({column},{row}) 不再是原色"
                    );
                }
            }
        }
    }

    /// 缩小时必须**平均**掉被覆盖的源像素，不能挑一个代表。
    ///
    /// 高频棋盘格是最能分辨这两者的输入：取最近邻只会得到 0 或 255（棋盘格原样缩小，
    /// 变成摩尔纹），重采样则得到中间灰。
    #[test]
    fn shrinking_averages_the_pixels_it_covers() {
        let (image_width, image_height) = (16u32, 16u32);
        let checkerboard: Vec<u32> = (0..image_width * image_height)
            .map(|index| {
                let (x, y) = (index % image_width, index / image_width);
                let value = if (x + y) % 2 == 0 { 0 } else { 255 };
                0xFF00_0000 | value
            })
            .collect();
        let border = 1;
        let zoom = 0.25;
        let size = window_size(image_width, image_height, zoom, border);
        let mut buffer = vec![0u32; (size.0 * size.1) as usize];
        paint_pin(
            &mut buffer,
            size,
            border,
            &checkerboard,
            image_width,
            image_height,
            zoom,
            TEST_DPI,
        );

        let target = scaled_dimension(f64::from(image_width), zoom);
        assert_eq!(target, 4);
        for row in 0..target as usize {
            for column in 0..target as usize {
                let value = (buffer[(row + 1) * size.0 as usize + column + 1] & 0xFF) as i32;
                assert!(
                    value > 0 && value < 255,
                    "({column},{row}) 是 {value}：端点值说明缩小时只挑了一个源像素而没平均"
                );
                assert!(
                    (value - 128).abs() <= 24,
                    "({column},{row}) 是 {value}，离棋盘格的均值 128 太远"
                );
            }
        }
    }

    /// 掩码里必须出现 0 与 255 之外的值：只有两个值就说明文字是硬边（没有反锯齿），
    /// 在 150% 缩放下看就是一圈锯齿。
    #[test]
    fn the_text_mask_has_intermediate_coverage() {
        let mask = text_mask("1.9×", 96).expect("96 DPI 下应当能光栅化");
        assert!(mask.width > 0 && mask.height > 0, "掩码是空的");
        assert!(
            mask.coverage.iter().any(|value| *value > 0 && *value < 255),
            "掩码里只有 0 和 255：字体平滑被关掉了，文字边缘会是硬锯齿"
        );
        assert!(
            mask.coverage.iter().any(|value| *value == 0),
            "掩码里没有空白像素，多半是宽度量错了"
        );
        assert_eq!(
            mask.coverage.len(),
            mask.width as usize * mask.height as usize
        );
    }

    /// 控件里的字必须是**覆盖层那颗的字**：放大镜的倍数标签走 `overlay_font::FAMILY`，
    /// 贴图这边要到的必须同名。
    ///
    /// 这条不是重复上面那条：GDI 对不认识的族名不报错，它会静默替一个，所以"我填了那个名字"
    /// 什么也证明不了——只有把 DC 里选中的族名读回来才算数。
    #[test]
    fn the_badge_is_drawn_in_the_overlay_typeface() {
        let height = scale_dip(BADGE_FONT_DIP, TEST_DPI);
        unsafe {
            let screen_dc = GetDC(std::ptr::null_mut());
            assert!(!screen_dc.is_null(), "取屏幕 DC 失败");
            let memory_dc = CreateCompatibleDC(screen_dc);
            assert!(!memory_dc.is_null(), "取内存 DC 失败");
            let (font, previous) = select_badge_font(memory_dc, -height).expect("一个族都没要到");
            assert_eq!(
                face_name(memory_dc).as_deref(),
                Some(snapclip_capture::overlay_font::FAMILY),
                "贴图的倍数控件与放大镜那颗不是同一个字体族"
            );
            SelectObject(memory_dc, previous);
            DeleteObject(font.into());
            DeleteDC(memory_dc);
            ReleaseDC(std::ptr::null_mut(), screen_dc);
        }
    }

    /// 让上一条的断言有意义：族名对不上时，读回来必须**不是**那个名字。
    ///
    /// 没有这条，`select_badge_font` 的校验退化成恒真判断（比如 `GetTextFaceW` 传错参数、
    /// 或拿请求名当结果名比）也照样能让上一条过。
    #[test]
    fn a_family_gdi_does_not_have_does_not_read_back() {
        let height = scale_dip(BADGE_FONT_DIP, TEST_DPI);
        unsafe {
            let screen_dc = GetDC(std::ptr::null_mut());
            let memory_dc = CreateCompatibleDC(screen_dc);
            let font = create_badge_face(-height, "SnapClip No Such Font");
            assert!(!font.is_null(), "CreateFontW 连替身都不给");
            let previous = SelectObject(memory_dc, font.into());
            assert_ne!(
                face_name(memory_dc).as_deref(),
                Some("SnapClip No Such Font"),
                "GDI 声称有一颗不存在的字体：读回校验这条路本身失灵了"
            );
            SelectObject(memory_dc, previous);
            DeleteObject(font.into());
            DeleteDC(memory_dc);
            ReleaseDC(std::ptr::null_mut(), screen_dc);
        }
    }

    /// 并行混合的两个端点必须**精确**：权重全在一边时不该有任何舍入，否则平坦颜色的边缘
    /// 会挂上一圈色偏。
    #[test]
    fn the_packed_blend_hits_both_ends_and_keeps_flat_colours() {
        let first = 0x1234_5678u32;
        let second = 0x9ABC_DEF0u32;
        assert_eq!(blend_pixels(first, second, 0), first);
        assert_eq!(blend_pixels(first, second, BLEND_ONE), second);
        for weight in (0..=BLEND_ONE).step_by(7) {
            assert_eq!(
                blend_pixels(second, second, weight),
                second,
                "权重 {weight} 上的同色混合"
            );
        }
    }

    /// 快路与通用路必须给出同一个结果。
    ///
    /// 两条轴都是两个抽头时走 `resample_magnified`：它先把四通道塞进一个 32 位字并行混合，
    /// 而且两次一维混合之间多舍入一次，所以允许差**两个**色阶（9 位权重 0.5 + 中间舍入
    /// 0.5）。再多就说明并行混合的通道串味了——掩码写错或权重越界时画面只是"有点脏"，
    /// 不会明显报错，这条断言是唯一能从数字上看出来的地方。
    #[test]
    fn the_packed_blend_agrees_with_the_general_one() {
        let (source_width, source_height) = (9usize, 7usize);
        let image: Vec<u32> = (0..source_width * source_height)
            .map(|index| {
                let red = (index * 37 % 256) as u32;
                let green = (index * 91 % 256) as u32;
                let blue = (index * 143 % 256) as u32;
                0xFF00_0000 | (red << 16) | (green << 8) | blue
            })
            .collect();
        let (target_width, target_height) = (23usize, 19usize);
        let x_plan = plan_axis(source_width, target_width);
        let y_plan = plan_axis(source_height, target_height);
        assert!(
            two_tap_plan(&x_plan) && two_tap_plan(&y_plan),
            "这条用例的前提是放大"
        );

        // 三种目标矩形都要试：高比宽大、宽比高大、以及只剩一行一列的两端。
        for (width, height) in [(23usize, 19usize), (23, 1), (1, 19), (7, 5)] {
            let mut fast = vec![0u32; width * height];
            let mut general = vec![0u32; width * height];
            resample_magnified(
                &mut fast,
                width,
                0,
                0,
                &image,
                source_width,
                (width, height),
                &x_plan,
                &y_plan,
            );
            resample_filtered(
                &mut general,
                width,
                0,
                0,
                &image,
                (source_width, source_height),
                (width, height),
                &x_plan,
                &y_plan,
            );
            for (index, (one, other)) in fast.iter().zip(general.iter()).enumerate() {
                for channel in 0..4 {
                    let left = (one >> (channel * 8)) & 0xFF;
                    let right = (other >> (channel * 8)) & 0xFF;
                    assert!(
                        left.abs_diff(right) <= 2,
                        "{width}x{height} 的第 {index} 个像素第 {channel} 通道：\
                         {left:02X} 对 {right:02X}",
                    );
                }
            }
        }
    }

    /// 分段并行必须和串行画出**一模一样**的东西。
    ///
    /// 分段把"整幅目标里的行号"和"这一段缓冲里的行号"拆成了两个量，混用不会报错，只会让画面
    /// 在某一档缩放上错几行——所以这条断言比它看起来重要。`resample_image` 会真的开线程，
    /// `resample_band` 是单段那一条路。
    #[test]
    fn the_parallel_resample_matches_the_serial_one() {
        let (source_width, source_height) = (17usize, 40usize);
        let image: Vec<u32> = (0..source_width * source_height)
            .map(|index| 0xFF00_0000 | (index as u32).wrapping_mul(2_654_435_761).rotate_right(11))
            .collect();

        // 三种缩放分别走三条路：缩小是通用循环、1× 是整行复制、放大是并行混合。
        for zoom in [0.5, 1.0, 3.0] {
            let target_width = scaled_dimension(source_width as f64, zoom) as usize;
            let target_height = scaled_dimension(source_height as f64, zoom) as usize;
            let x_plan = plan_axis(source_width, target_width);
            let y_plan = plan_axis(source_height, target_height);
            let (pitch, drawn) = (target_width + 2, (target_width, target_height));
            assert!(drawn.1 >= 16, "这条用例要真的走到并行那一段");

            let mut parallel = vec![7u32; pitch * (drawn.1 + 2)];
            let mut serial = vec![7u32; pitch * (drawn.1 + 2)];
            resample_image(
                &mut parallel,
                pitch,
                (1, 1),
                &image,
                (source_width, source_height),
                drawn,
                &x_plan,
                &y_plan,
            );
            let rows = pitch..pitch + drawn.1 * pitch;
            resample_band(
                &mut serial[rows],
                pitch,
                1,
                0,
                &image,
                (source_width, source_height),
                drawn,
                &x_plan,
                &y_plan,
            );
            assert!(
                parallel == serial,
                "zoom={zoom}：分段并行和串行画出来的不是同一块缓冲"
            );
        }
    }

    /// 边框只铺在图像区**之外**：图像那几块必须一个像素都没被碰过。
    #[test]
    fn the_border_never_touches_the_image_area() {
        let (width, height) = (12usize, 9usize);
        let (border, image) = (2usize, (6usize, 4usize));
        let mut buffer = vec![0u32; width * height];
        fill_border(&mut buffer, (width, height), border, image);

        for row in 0..height {
            for column in 0..width {
                let inside = (border..border + image.0).contains(&column)
                    && (border..border + image.1).contains(&row);
                let pixel = buffer[row * width + column];
                assert_eq!(
                    pixel == BORDER_COLOR,
                    !inside,
                    "({column}, {row}) 在图像区里={inside}，像素 {pixel:08X}",
                );
            }
        }
    }

    /// 旧画法（最近邻）的参考实现，只为在同一个基准里给出"改重采样贵了多少"。
    ///
    /// 这段代码**不在产品路径上**，它存在的理由是让 `paint_pin_cost_at_several_zooms` 能
    /// 报出一对数字。上一版的实现已经被删掉（它画的每个源像素都是一个方块），没有它就没
    /// 法回答"这次改画质是不是把缩放拖慢了"。
    fn nearest_reference(
        buffer: &mut [u32],
        size: (i32, i32),
        border: i32,
        image: &[u32],
        image_width: u32,
        image_height: u32,
        zoom: f64,
    ) {
        let width = size.0 as usize;
        let border = border as usize;
        let target_width = scaled_dimension(f64::from(image_width), zoom).max(0) as usize;
        let target_height = scaled_dimension(f64::from(image_height), zoom).max(0) as usize;
        let factor_x = target_width as f64 / f64::from(image_width);
        let factor_y = target_height as f64 / f64::from(image_height);
        for row in 0..target_height {
            let source_row = (row as f64 / factor_y) as usize * image_width as usize;
            for column in 0..target_width {
                let source_x = (column as f64 / factor_x) as usize;
                buffer[(border + row) * width + border + column] = image[source_row + source_x];
            }
        }
    }

    /// 量一次重采样的成本，不断言什么。
    ///
    /// 跑法：`cargo test --release -p snapclip-app --lib paint_pin_cost -- --ignored --nocapture`
    ///
    /// 数字随机器变，只作量级判断。留在这里是因为"缩放卡不卡"最终只取决于这个函数：
    /// 缓冲大小按倍数的**平方**涨（1405x1137 放到 5.6× 就是 7872x6371），抽头数按缩小
    /// 倍数涨，两头的成本都不在别处。上一版的最近邻没有这个量，改重采样之后必须有。
    #[test]
    #[ignore = "测量而不是断言；用 --ignored --nocapture 手动跑"]
    fn paint_pin_cost_at_several_zooms() {
        let (image_width, image_height) = (1405u32, 1137u32);
        let image: Vec<u32> = (0..image_width * image_height)
            .map(|index| 0xFF00_0000 | (index & 0xFF))
            .collect();
        let border = 1;
        for zoom in [0.1, 0.5, 1.0, 2.0, 5.6] {
            let size = window_size(image_width, image_height, zoom, border);
            let mut buffer = vec![0u32; (size.0 * size.1) as usize];
            let started = std::time::Instant::now();
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
            let seconds = started.elapsed().as_secs_f64();
            let megapixels = f64::from(size.0) * f64::from(size.1) / 1.0e6;

            let started = std::time::Instant::now();
            nearest_reference(
                &mut buffer,
                size,
                border,
                &image,
                image_width,
                image_height,
                zoom,
            );
            let nearest = started.elapsed().as_secs_f64();

            println!(
                "paint_pin zoom={zoom:.3} buffer={}x{} ({megapixels:.1} Mpx) \
                 重采样 {:.1} ms ({:.1} Mpx/s)，最近邻 {:.1} ms，倍数 {:.2}×",
                size.0,
                size.1,
                seconds * 1000.0,
                megapixels / seconds,
                nearest * 1000.0,
                seconds / nearest,
            );
        }
    }

    /// 对比用的合成图案：96x56，把贴图放大时最容易露馅的几样东西摆在一块。
    ///
    /// 上排是**真实字形**——直接借控件那一套白底黑字的覆盖率掩码（`text_mask`），所以这里
    /// 看到的抗锯齿与控件上那颗字一模一样；中排是 1 像素竖条纹、45° 斜线、圆轮廓；
    /// 下排是一条 1 像素横线加一条渐变。这五样分别对应"核会不会把它抹平""阶梯有多硬"
    /// "曲线会不会起波纹""边界像素落在哪半边"。
    fn filter_comparison_pattern() -> (usize, usize, Vec<u32>) {
        const WIDTH: usize = 96;
        const HEIGHT: usize = 56;
        const INK: u32 = 0xFF20_2020;
        const PAPER: u32 = 0xFFF0_F0F0;
        let mut image = vec![PAPER; WIDTH * HEIGHT];
        let mut put = |x: i64, y: i64, color: u32| {
            if x >= 0 && y >= 0 && (x as usize) < WIDTH && (y as usize) < HEIGHT {
                image[y as usize * WIDTH + x as usize] = color;
            }
        };

        if let Some(mask) = text_mask("5.6×", 96) {
            for row in 0..mask.height {
                for column in 0..mask.width {
                    let coverage = u32::from(mask.coverage[(row * mask.width + column) as usize]);
                    if coverage == 0 {
                        continue;
                    }
                    // 掩码是覆盖率：这里把它染成"黑字压在浅底上"，和控件里那次混色同源。
                    let value = (0x20 * coverage + 0xF0 * (255 - coverage)) / 255;
                    put(
                        2 + i64::from(column),
                        2 + i64::from(row),
                        0xFF00_0000 | (value << 16) | (value << 8) | value,
                    );
                }
            }
        }

        for y in 18..40 {
            for x in 0..30 {
                put(x, y, if x % 2 == 0 { INK } else { PAPER });
            }
        }
        for step in 0..20 {
            put(34 + step, 18 + step, INK);
        }
        for degree in 0..3600 {
            let angle = std::f64::consts::TAU * f64::from(degree) / 3600.0;
            put(
                (78.0 + 11.0 * angle.cos()).round() as i64,
                (29.0 + 11.0 * angle.sin()).round() as i64,
                INK,
            );
        }
        for x in 0..WIDTH {
            put(x as i64, 42, INK);
        }
        for x in 0..WIDTH {
            let value = (x * 255 / (WIDTH - 1)) as u32;
            for y in 44..HEIGHT {
                put(x as i64, y as i64, 0xFF00_0000 | (value << 16) | (value << 8) | value);
            }
        }
        (WIDTH, HEIGHT, image)
    }

    fn nearest_kernel(offset: f64) -> f64 {
        if offset.abs() < 0.5 { 1.0 } else { 0.0 }
    }

    fn triangle_kernel(offset: f64) -> f64 {
        (1.0 - offset.abs()).max(0.0)
    }

    /// Mitchell–Netravali 三次核（`B = C = 1/3`）。峰值是 0.888 而不是 1，无所谓——权重
    /// 逐目标像素归一化。它在 1.5 附近有一个约 -0.056 的负瓣，也就是"振铃"的来源，
    /// 而负瓣正是生产那套 `u32` 定点权重装不下的东西（见 [`reference_resample`]）。
    fn mitchell_kernel(offset: f64) -> f64 {
        const B: f64 = 1.0 / 3.0;
        const C: f64 = 1.0 / 3.0;
        let x = offset.abs();
        if x < 1.0 {
            ((12.0 - 9.0 * B - 6.0 * C) * x * x * x + (-18.0 + 12.0 * B + 6.0 * C) * x * x
                + (6.0 - 2.0 * B))
                / 6.0
        } else if x < 2.0 {
            ((-B - 6.0 * C) * x * x * x
                + (6.0 * B + 30.0 * C) * x * x
                + (-12.0 * B - 48.0 * C) * x
                + (8.0 * B + 24.0 * C))
                / 6.0
        } else {
            0.0
        }
    }

    /// Lanczos-3。最锐，负瓣也最深（约 -0.07），高对比边缘上的振铃最明显。
    fn lanczos3_kernel(offset: f64) -> f64 {
        fn sinc(t: f64) -> f64 {
            if t == 0.0 {
                1.0
            } else {
                let pi_t = std::f64::consts::PI * t;
                pi_t.sin() / pi_t
            }
        }
        let x = offset.abs();
        if x >= 3.0 { 0.0 } else { sinc(x) * sinc(x / 3.0) }
    }

    /// 参考重采样：与生产**同一套几何**，但权重是 `f64`。
    ///
    /// 用浮点不是为了替换生产实现，而是为了让 Mitchell 与 Lanczos3 能参与比较：它们都有
    /// 负瓣，而生产的权重是 `u32` 定点数、装不下负值。几何必须逐条对齐 [`plan_axis`]，
    /// 否则比较的是两个变量而不是两个核：中心对齐（`(d + 0.5) * scale - 0.5`）、缩小时
    /// 按 `scale` 把核撑开、逐目标像素归一化。`the_reference_resampler_agrees_with_production`
    /// 就是钉住"逐条对齐"的那条断言。
    fn reference_resample(
        image: &[u32],
        width: usize,
        height: usize,
        zoom: f64,
        support: f64,
        kernel: fn(f64) -> f64,
    ) -> (usize, usize, Vec<u32>) {
        let target_width = scaled_dimension(width as f64, zoom) as usize;
        let target_height = scaled_dimension(height as f64, zoom) as usize;
        let axis = |source_length: usize, target_length: usize| {
            let scale = source_length as f64 / target_length as f64;
            let filter_scale = scale.max(1.0);
            let radius = support * filter_scale;
            (0..target_length)
                .map(|destination| {
                    let center = (destination as f64 + 0.5) * scale - 0.5;
                    let first = (center - radius).ceil() as i64;
                    let last = (center + radius).floor() as i64;
                    let mut taps: Vec<(usize, f64)> = Vec::new();
                    let mut sum = 0.0;
                    for source in first.max(0)..=last.min(source_length as i64 - 1) {
                        let weight = kernel((source as f64 - center) / filter_scale);
                        if weight == 0.0 {
                            continue;
                        }
                        sum += weight;
                        taps.push((source as usize, weight));
                    }
                    if taps.is_empty() || sum == 0.0 {
                        let nearest =
                            center.round().clamp(0.0, (source_length - 1) as f64) as usize;
                        taps.push((nearest, 1.0));
                    } else {
                        for tap in &mut taps {
                            tap.1 /= sum;
                        }
                    }
                    taps
                })
                .collect::<Vec<_>>()
        };
        let x_taps = axis(width, target_width);
        let y_taps = axis(height, target_height);

        let mut out = vec![0u32; target_width * target_height];
        for (row, row_taps) in y_taps.iter().enumerate() {
            for (column, column_taps) in x_taps.iter().enumerate() {
                let mut channels = [0.0f64; 4];
                for &(source_y, weight_y) in row_taps {
                    for &(source_x, weight_x) in column_taps {
                        let weight = weight_x * weight_y;
                        let pixel = image[source_y * width + source_x];
                        for (channel, sum) in channels.iter_mut().enumerate() {
                            *sum += f64::from((pixel >> (channel * 8)) & 0xFF) * weight;
                        }
                    }
                }
                let mut pixel = 0xFF00_0000u32;
                for (channel, sum) in channels.iter().enumerate() {
                    pixel |= (sum.round().clamp(0.0, 255.0) as u32) << (channel * 8);
                }
                out[row * target_width + column] = pixel;
            }
        }
        (target_width, target_height, out)
    }

    /// 把一张 `u32` BGRA 缓冲写成 PNG。测试专用，不走生产那条编码路径。
    fn write_comparison_png(path: &std::path::Path, width: usize, height: usize, pixels: &[u32]) {
        let mut rgba = Vec::with_capacity(width * height * 4);
        for pixel in pixels {
            rgba.extend_from_slice(&[
                (pixel >> 16) as u8,
                (pixel >> 8) as u8,
                *pixel as u8,
                0xFF,
            ]);
        }
        let file = std::fs::File::create(path).expect("建 PNG 文件");
        let mut encoder =
            png::Encoder::new(std::io::BufWriter::new(file), width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .expect("写 PNG 头")
            .write_image_data(&rgba)
            .expect("写 PNG 数据");
    }

    /// 放大滤波器对比：同一张合成图案，用四种核各放大一遍，叠成一张 PNG。
    ///
    /// 跑法：`cargo test --release -p snapclip-app --lib filter_comparison -- --ignored --nocapture`
    ///
    /// 图落在 `target/filter-compare/`，**从上到下**是 nearest / triangle / mitchell /
    /// lanczos3 四条带子；最上面那条是"不滤波"的基线，另外三条都在跟它比"软化与阶梯的
    /// 取舍"。之所以留一个只出图、不断言的测试：贴图放大该用哪个重建核只能靠眼睛回答，
    /// 而回答过的那张图应该能被重新生成——`docs/34` §4.2.3 的结论就挂在它上面。
    #[test]
    #[ignore = "出图而不是断言；用 --ignored --nocapture 手动跑"]
    fn filter_comparison_sheets() {
        let (width, height, image) = filter_comparison_pattern();
        let filters: [(&str, f64, fn(f64) -> f64); 4] = [
            ("nearest", 0.5, nearest_kernel),
            ("triangle", 1.0, triangle_kernel),
            ("mitchell", 2.0, mitchell_kernel),
            ("lanczos3", 3.0, lanczos3_kernel),
        ];
        let directory = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/filter-compare"
        ));
        std::fs::create_dir_all(directory).expect("建对比图目录");

        for zoom in [2.0f64, 4.0] {
            let gap = 4usize;
            let mut bands = Vec::new();
            for (name, support, kernel) in filters {
                let started = std::time::Instant::now();
                let patch = reference_resample(&image, width, height, zoom, support, kernel);
                let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                let megapixels = (patch.0 * patch.1) as f64 / 1_000_000.0;
                println!(
                    "zoom={zoom:.1} {name:<9} {}x{} 用时 {elapsed_ms:.1} ms（每 Mpx {:.1} ms）",
                    patch.0,
                    patch.1,
                    elapsed_ms / megapixels,
                );
                bands.push(patch);
            }

            let sheet_width = bands[0].0;
            let sheet_height =
                bands.iter().map(|band| band.1).sum::<usize>() + gap * (bands.len() - 1);
            let mut sheet = vec![0xFF80_8080u32; sheet_width * sheet_height];
            let mut row_offset = 0usize;
            for (band_width, band_height, band) in &bands {
                for row in 0..*band_height {
                    let destination = (row_offset + row) * sheet_width;
                    sheet[destination..destination + *band_width]
                        .copy_from_slice(&band[row * *band_width..(row + 1) * *band_width]);
                }
                row_offset += band_height + gap;
            }
            let path = directory.join(format!("zoom-{zoom:.1}.png"));
            write_comparison_png(&path, sheet_width, sheet_height, &sheet);
            println!(
                "{}（{sheet_width}x{sheet_height}，从上到下：nearest / triangle / mitchell / lanczos3）",
                path.display()
            );
        }
    }

    /// 参考重采样与生产实现必须给出同一个像素，差不超过 1（定点与浮点的舍入）。
    ///
    /// 钉住这一条，`filter_comparison_sheets` 那张图才是**关于生产实现**的证据：图里
    /// triangle 那一带与 `paint_pin` 画出来的是同一张。
    #[test]
    fn the_reference_resampler_agrees_with_production() {
        let (image_width, image_height) = (64u32, 48u32);
        let image = image_of(image_width, image_height);
        for zoom in [1.7, 2.0, 3.3] {
            let border = 1;
            let size = window_size(image_width, image_height, zoom, border);
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
            let (patch_width, patch_height, patch) = reference_resample(
                &image,
                image_width as usize,
                image_height as usize,
                zoom,
                1.0,
                triangle_kernel,
            );
            assert_eq!(
                (patch_width, patch_height),
                (
                    scaled_dimension(f64::from(image_width), zoom) as usize,
                    scaled_dimension(f64::from(image_height), zoom) as usize,
                )
            );
            // 控件最后画、画在图像之上，所以"整幅图像逐像素"的对比必须把它那一块让出去——
            // 矩形是问 `badge_placement` 要的，不是估出来的。
            let badge = badge_placement(image_width, image_height, zoom, border, TEST_DPI)
                .map(|(_, rect)| rect);
            let mut compared = 0usize;
            let mut skipped = 0usize;
            for row in 0..patch_height {
                for column in 0..patch_width {
                    let x = border + column as i32;
                    let y = border + row as i32;
                    if badge.is_some_and(|(left, top, width, height)| {
                        left <= x && x < left + width && top <= y && y < top + height
                    }) {
                        skipped += 1;
                        continue;
                    }
                    compared += 1;
                    let produced = buffer[y as usize * size.0 as usize + x as usize];
                    let expected = patch[row * patch_width + column];
                    for channel in 0..4 {
                        let shift = channel * 8;
                        let a = (produced >> shift) & 0xFF;
                        let b = (expected >> shift) & 0xFF;
                        assert!(
                            a.abs_diff(b) <= 1,
                            "zoom={zoom} ({column},{row}) 通道 {channel}：生产 {a} 参考 {b}"
                        );
                    }
                }
            }
            assert!(compared > 0, "zoom={zoom} 一个像素都没比到");
            assert_eq!(compared + skipped, patch_width * patch_height);
            if badge.is_some() {
                assert!(skipped > 0, "zoom={zoom} 控件压在图像上，应当让出过像素");
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
    fn border_is_one_dip_rounded_with_a_floor_of_one_pixel() {
        // 1 DIP 折成像素：100% 是 1、150% 是 1.5 → 2、200% 是 2。
        assert_eq!(border_px(96), 1);
        assert_eq!(border_px(144), 2);
        assert_eq!(border_px(192), 2);
        // 低于 100% 的 DPI 会折出 0.5 及以下，但边框永远至少 1 像素：0 宽的边框等于
        // 图像直接糊到窗口边缘，那圈"贴图与底下的窗口分开"的标记就没了。
        assert_eq!(border_px(48), 1);
        assert!(border_px(0) >= 1);
    }

    /// 控件上的字是这个功能的**唯一可见证据**：倍数只在那个小标里。
    ///
    /// 格式规则与放大镜那颗一字不差（`magnifier_pass.rs`），对应用户的原话
    /// "文本格式和放大镜框的保持一致，使用 ×"。
    #[test]
    fn the_badge_spells_the_zoom_the_way_the_magnifier_does() {
        // 整档不写小数。
        assert_eq!(badge_label(1.0), "1×");
        assert_eq!(badge_label(2.0), "2×");
        assert_eq!(badge_label(10.0), "10×");
        // 半档写一位。
        assert_eq!(badge_label(0.5), "0.5×");
        assert_eq!(badge_label(1.9), "1.9×");
        assert_eq!(badge_label(0.1), "0.1×");
        // 步长 0.1，所以一位小数永远够用；脏值显示钳过之后的。
        assert_eq!(badge_label(f64::NAN), "1×");
        assert_eq!(badge_label(99.0), "10×");
        // `0` 与负数不是"很小的倍数"，是"这不是一个倍数"，归到下限。
        assert_eq!(badge_label(0.0), "0.1×");
    }

    /// 圆角：四角切掉、正中和四条边的中点都留着；**而且边缘是渐变的**。
    ///
    /// 旧实现（`inside_rounded`）返回的是一个布尔值，于是圆角那段圆弧只能落在整像素上，
    /// 弧线必然是一格一格的——这就是用户报的"倍数控件也有锯齿"。所以这个测试除了验证
    /// 形状，还必须验证"存在既不是全透明也不是全实心的像素"，否则它测不出抗锯齿。
    #[test]
    fn rounding_feathers_the_edge_instead_of_switching_it_on_and_off() {
        let (width, height, radius) = (20.0f32, 16.0f32, 6.0f32);
        let coverage_at = |x: f32, y: f32| edge_coverage(rounded_distance(x, y, width, height, radius));
        // 四角外那一块：距离为正，完全透明。
        for (x, y) in [(0.5, 0.5), (19.5, 0.5), (0.5, 15.5), (19.5, 15.5)] {
            assert_eq!(coverage_at(x, y), 0, "({x},{y}) 是圆角该切掉的那块");
        }
        // 正中与四条边的中点：距离够负，完全实心，值逐位等于底色。
        for (x, y) in [
            (10.0, 8.0),
            (0.5, 8.0),
            (19.5, 8.0),
            (10.0, 0.5),
            (10.0, 15.5),
        ] {
            assert_eq!(coverage_at(x, y), 255, "({x},{y}) 该实心");
        }
        // 压在边界上正好一半；1 像素的斜坡就是覆盖率从 0 线性走到 255。
        assert_eq!(edge_coverage(0.0), 128);
        assert_eq!(edge_coverage(-0.5), 255);
        assert_eq!(edge_coverage(0.5), 0);
        assert_eq!(edge_coverage(-9.0), 255);
        assert_eq!(edge_coverage(9.0), 0);

        // 圆角弧线上必须有过渡像素——二值遮罩下这个计数恒为 0。
        let mut feathered = 0;
        for row in 0..height as i32 {
            for column in 0..width as i32 {
                let coverage = coverage_at(column as f32 + 0.5, row as f32 + 0.5);
                if coverage != 0 && coverage != 255 {
                    feathered += 1;
                }
            }
        }
        assert!(feathered > 0, "圆角上应当有半透明的过渡像素");

        // 半径为 0 就是方角：(0.5,0.5) 这个像素中心在 4x4 里面，距离为负。
        assert!(rounded_distance(0.5, 0.5, 4.0, 4.0, 0.0) < 0.0);
        // 半径被夹到半边长以内：给 99 等于给一条半径 8 的弧（16 高的一半），
        // 角还是照切，只是不会算出负半径那种无意义的形状。
        assert!(rounded_distance(0.5, 0.5, 20.0, 16.0, 99.0) > 0.0);
        assert!(rounded_distance(2.0, 2.0, 4.0, 4.0, 99.0) < 0.0);
    }

    /// 控件四角的抗锯齿要真的落在**画出来的缓冲**里，而不只是那个覆盖率函数算得出来。
    ///
    /// 底下的图像用一整块纯色，于是控件边缘出现第三种颜色就只可能是"底色 × 图像"的
    /// 混合。扫描范围只取控件左上角那一块 `radius × radius` 的方块：那里没有文字
    /// （文字从 `pad_h`/`pad_v` 开始），所以不会把字形的抗锯齿误当成圆角的。
    #[test]
    fn the_badge_corner_is_feathered_against_the_image_it_covers() {
        let (image_width, image_height) = (200u32, 100u32);
        let border = 1;
        let zoom = 3.0;
        let size = window_size(image_width, image_height, zoom, border);
        const FLAT: u32 = 0xFF00_8000;
        let image = vec![FLAT; (image_width * image_height) as usize];
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

        let style = BadgeStyle::new(TEST_DPI);
        let (_, (badge_left, badge_top, _, _)) =
            badge_placement(image_width, image_height, zoom, border, TEST_DPI)
                .expect("这个尺寸下控件应当放得下");
        let at = |x: i32, y: i32| buffer[y as usize * size.0 as usize + x as usize];

        let mut cut = 0;
        let mut blended = 0;
        for row in 0..style.radius {
            for column in 0..style.radius {
                match at(badge_left + column, badge_top + row) {
                    FLAT => cut += 1,
                    BADGE_BG_COLOR => {}
                    _ => blended += 1,
                }
            }
        }
        assert!(cut > 0, "圆角该把角上的像素让给底下的图像");
        assert!(blended > 0, "圆角边缘应当有半透明的过渡像素");
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
        let (_, (badge_left, badge_top, badge_width, badge_height)) =
            badge_placement(image_width, image_height, zoom, border, TEST_DPI)
                .expect("这个尺寸下控件应当放得下");
        let badge_right = badge_left + badge_width;

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
