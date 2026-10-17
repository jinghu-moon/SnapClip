//! 差分实验：**贴图风格的 layered 窗口（正向对照）** vs **倒计时遮罩风格的 layered 窗口**。
//!
//! 目的（docs/23 §14.55.9.8）：上一轮"显示前后截图逐字节相同"的实验**缺正向对照**，所以
//! 无法区分"窗口没被合成"与"观测手段没捕获到变化"。这里两个窗口同时存在、用**同一种**观测
//! 方式（屏幕 `BitBlt` + `CAPTUREBLT`，它会包含 layered 窗口）分别取像素均值，每一步都
//! 同时报告两个窗口的读数。对照窗口始终不变，用来证明"观测手段此刻是有效的"。
//!
//! 跑法（活桌面）：
//!
//! ```text
//! cargo test -p snapclip-capture --test layered_differential -- --ignored --nocapture
//! ```
//!
//! 看什么：每一步打印 `A=…`（贴图风格对照）与 `B=…`（遮罩风格）。A 必须始终是红色；
//! B 从红变成"桌面色"的那一步，就是使遮罩从观测中消失的最小配置差异。

#![cfg(windows)]

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CombineRgn, CreateCompatibleDC, CreateDIBSection, CreateRectRgn, DeleteDC,
    DeleteObject, GetDC, ReleaseDC, SelectObject, SetWindowRgn, AC_SRC_ALPHA, AC_SRC_OVER,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, CAPTUREBLT, DIB_RGB_COLORS, HBITMAP, HDC,
    RGN_DIFF, SRCCOPY,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetWindowRect, PeekMessageW,
    RegisterClassW, SetWindowPos, ShowWindow, UnregisterClassW, UpdateLayeredWindow, MSG,
    PM_REMOVE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_SHOW, ULW_ALPHA,
    WM_QUIT, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP, WS_VISIBLE,
};


/// 让本进程与产品一致地 per-monitor-v2 感知。
///
/// 不这么做会毁掉整个观测：`cargo test` 的进程默认不是 DPI 感知的，`CreateWindowExW`
/// 的坐标是**虚拟化**的，而屏幕 `BitBlt` 取的是**物理**像素 ⇒ 取样矩形与窗口错位，
/// 取到的多半是桌面（上一轮"逐字节相同"的结论就是这么来的）。
fn be_dpi_aware() {
    use windows_sys::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 一个 layered 窗口 + 它的 DIB。
struct Layered {
    hwnd: HWND,
    memory_dc: HDC,
    bitmap: HBITMAP,
    bits: *mut u32,
    width: i32,
    height: i32,
    alpha: u8,
}

// DIB 的位图指针由本结构独占持有，不跨线程共享。
unsafe impl Send for Layered {}

impl Layered {
    /// 照 `apps/snapclip/src/pin/host.rs::create_pin_window` 的形状创建：
    /// ex-style 由调用方给（对照窗口用贴图的那组），**普通样式位由 `visible_at_creation` 决定**
    /// —— 这正是本轮要单独验证的变量。
    fn new(
        class: &[u16],
        instance: *mut c_void,
        ex_style: u32,
        visible_at_creation: bool,
        (x, y, width, height): (i32, i32, i32, i32),
        alpha: u8,
    ) -> Self {
        let style = if visible_at_creation {
            WS_POPUP | WS_VISIBLE
        } else {
            WS_POPUP
        };
        let hwnd = unsafe {
            CreateWindowExW(
                ex_style,
                class.as_ptr(),
                std::ptr::null(),
                style,
                x,
                y,
                width,
                height,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null_mut(),
            )
        };
        assert!(!hwnd.is_null(), "CreateWindowExW 失败：{}", unsafe {
            GetLastError()
        });

        // DIB：32bpp top-down，与贴图窗口一致（premultiplied，ULW_ALPHA 要求）。
        let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };
        let memory_dc = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
        assert!(!memory_dc.is_null(), "CreateCompatibleDC 失败");
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bitmap = unsafe {
            CreateDIBSection(
                memory_dc,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            )
        };
        assert!(!bitmap.is_null() && !bits.is_null(), "CreateDIBSection 失败");
        unsafe { SelectObject(memory_dc, bitmap as _) };

        let mut window = Self {
            hwnd,
            memory_dc,
            bitmap,
            bits: bits.cast::<u32>(),
            width,
            height,
            alpha,
        };
        let alpha = window.alpha;
        window.fill([255, 0, 0, alpha]);
        window.submit(None);
        // 遮罩风格在创建时不带 `WS_VISIBLE`，所以这里显式显示一次（这就是被怀疑的一步）。
        if !visible_at_creation {
            unsafe { ShowWindow(hwnd, SW_SHOW) };
        }
        window
    }

    /// 整窗填一种颜色（传 straight alpha，这里自己做 premultiply）。
    fn fill(&mut self, rgba: [u8; 4]) {
        let [r, g, b, a] = rgba;
        let premul = |c: u8| ((c as u32 * a as u32) / 255) as u32;
        let pixel = (a as u32) << 24 | premul(b) << 16 | premul(g) << 8 | premul(r);
        let count = (self.width * self.height) as usize;
        for index in 0..count {
            unsafe { *self.bits.add(index) = pixel };
        }
    }

    fn submit(&self, position: Option<(i32, i32)>) {
        let size = windows_sys::Win32::Foundation::SIZE {
            cx: self.width,
            cy: self.height,
        };
        let src = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        let destination = position.map(|(x, y)| windows_sys::Win32::Foundation::POINT { x, y });
        let destination = match &destination {
            Some(point) => point,
            None => std::ptr::null(),
        };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let ok = unsafe {
            UpdateLayeredWindow(
                self.hwnd,
                std::ptr::null_mut(),
                destination,
                &size,
                self.memory_dc,
                &src,
                0,
                &blend,
                ULW_ALPHA,
            )
        };
        assert_ne!(ok, 0, "UpdateLayeredWindow 失败：{}", unsafe {
            GetLastError()
        });
    }

    fn rect(&self) -> (i32, i32, i32, i32) {
        let mut rect = windows_sys::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        unsafe { GetWindowRect(self.hwnd, &mut rect) };
        (rect.left, rect.top, rect.right, rect.bottom)
    }
}

impl Drop for Layered {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
            DeleteObject(self.bitmap as _);
            DeleteDC(self.memory_dc);
        }
    }
}

/// 用屏幕 `BitBlt`（带 `CAPTUREBLT`）取一个矩形的**平均颜色**。
///
/// `CAPTUREBLT` 是关键：它把 layered 窗口也算进去，所以"取到桌面色"就说明那块是真的看不见。
fn screen_mean(rect: (i32, i32, i32, i32)) -> [f64; 3] {
    let (left, top, right, bottom) = rect;
    let width = (right - left).max(1);
    let height = (bottom - top).max(1);
    let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
    info.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width,
        biHeight: -height,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        biSizeImage: 0,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };
    let screen = unsafe { GetDC(std::ptr::null_mut()) };
    let memory = unsafe { CreateCompatibleDC(screen) };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let bitmap = unsafe {
        CreateDIBSection(memory, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0)
    };
    assert!(!bitmap.is_null() && !bits.is_null(), "观测用 DIB 创建失败");
    unsafe { SelectObject(memory, bitmap as _) };
    let ok = unsafe { BitBlt(memory, 0, 0, width, height, screen, left, top, SRCCOPY | CAPTUREBLT) };
    assert_ne!(ok, 0, "BitBlt 失败：{}", unsafe { GetLastError() });

    let pixels = unsafe { std::slice::from_raw_parts(bits.cast::<u32>(), (width * height) as usize) };
    let mut sum = [0f64; 3];
    for pixel in pixels {
        sum[0] += (pixel & 0xff) as f64;
        sum[1] += ((pixel >> 8) & 0xff) as f64;
        sum[2] += ((pixel >> 16) & 0xff) as f64;
    }
    let count = pixels.len() as f64;
    let mean = [sum[0] / count, sum[1] / count, sum[2] / count];
    unsafe {
        DeleteObject(bitmap as _);
        DeleteDC(memory);
        ReleaseDC(std::ptr::null_mut(), screen);
    }
    mean
}

fn pump(ms: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    let mut message: MSG = unsafe { std::mem::zeroed() };
    while std::time::Instant::now() < deadline {
        unsafe {
            while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if message.message == WM_QUIT {
                    return;
                }
                DispatchMessageW(&message);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

unsafe extern "system" fn wndproc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

fn register(class_name: &str) -> (*mut c_void, Vec<u16>) {
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    assert!(!instance.is_null(), "GetModuleHandleW 失败");
    let name = wide(class_name);
    let mut class: WNDCLASSW = unsafe { std::mem::zeroed() };
    class.lpfnWndProc = Some(wndproc);
    class.hInstance = instance;
    class.lpszClassName = name.as_ptr();
    class.hCursor = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::LoadCursorW(
            std::ptr::null_mut(),
            windows_sys::Win32::UI::WindowsAndMessaging::IDC_ARROW,
        )
    };
    class.hbrBackground = unsafe {
        windows_sys::Win32::Graphics::Gdi::GetStockObject(
            windows_sys::Win32::Graphics::Gdi::BLACK_BRUSH,
        ) as _
    };
    let registered = unsafe { RegisterClassW(&class) };
    assert_ne!(registered, 0, "RegisterClassW 失败：{}", unsafe {
        GetLastError()
    });
    (instance, name)
}

/// 贴图风格的 ex-style：`WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST`。
const PIN_EX: u32 = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST;
/// 遮罩风格的 ex-style（上一轮那行诊断的 `0x080800A8`）。
const DIM_EX: u32 = PIN_EX | WS_EX_TRANSPARENT;

fn report(step: &str, control: &Layered, subject: &Layered) {
    pump(220);
    eprintln!(
        "[diff] {step:<34} A(pin-style)={:?}  B(dim-style)={:?}",
        screen_mean(control.rect()).map(|v| v.round()),
        screen_mean(subject.rect()).map(|v| v.round()),
    );
}

#[test]
#[ignore = "L3 (docs/31 D-14): needs a live desktop; layered windows must be composited to mean anything"]
fn a_pin_style_layered_window_shows_what_a_dim_style_one_does_not() {
    be_dpi_aware();
    let (instance, name) = register("SnapClipLayeredDiffProbe");

    // 两个窗口都放在屏幕左侧、互不重叠；A 永远不变（正向对照）。
    let at_a = (80, 120, 240, 120);
    let at_b = (80, 320, 240, 120);

    // ── a. 基线：两个都是不透明红 ─────────────────────────────────────────
    let control = Layered::new(&name, instance, PIN_EX, true, at_a, 255);
    let mut subject = Layered::new(&name, instance, DIM_EX, false, at_b, 255);
    report("a. baseline (opaque red)", &control, &subject);

    // ── b. 遮罩改为半透明（α≈129）─────────────────────────────────────────
    subject.alpha = 129;
    subject.fill([255, 0, 0, 129]);
    subject.submit(None);
    report("b. subject alpha=129", &control, &subject);

    // ── c. 已经带 WS_EX_TRANSPARENT（DIM_EX 里就有）——确认它不影响可见性 ──
    report("c. subject WS_EX_TRANSPARENT", &control, &subject);
    drop(subject);

    // ── d. 加窗口区域（选区开洞那种形状）─────────────────────────────────
    let subject = Layered::new(&name, instance, DIM_EX, false, at_b, 129);
    {
        // 客户区坐标！`SetWindowRgn` 的矩形相对窗口客户区，传屏幕坐标会把整窗裁空。
        let (width, height) = (subject.width, subject.height);
        unsafe {
            let outer = CreateRectRgn(0, 0, width, height);
            let hole = CreateRectRgn(40, 30, width - 40, height - 30);
            CombineRgn(
                outer,
                outer,
                hole,
                RGN_DIFF,
            );
            SetWindowRgn(subject.hwnd, outer, 1);
        }
    }
    report("d. subject + window region", &control, &subject);

    // ── e. 加 WDA_EXCLUDEFROMCAPTURE（预期：从**截图**里消失，对照仍在）──
    let affinity = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetWindowDisplayAffinity(
            subject.hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::WDA_EXCLUDEFROMCAPTURE,
        )
    };
    eprintln!("[diff] e. SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE) -> {affinity}");
    report("e. subject + excludefromcapture", &control, &subject);

    // ── f. 改回 WDA_NONE 并重新显示，再看一次 ────────────────────────────
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetWindowDisplayAffinity(
            subject.hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::WDA_NONE,
        );
        SetWindowPos(
            subject.hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
    report("f. subject back to WDA_NONE + reshown", &control, &subject);
    drop(subject);

    // ── g. 唯一变量：创建时带 WS_VISIBLE（贴图窗口的做法）───────────────
    let mut subject = Layered::new(&name, instance, DIM_EX, true, at_b, 129);
    report("g. subject WS_VISIBLE at creation", &control, &subject);

    // 区域 + 排除 + 半透明 + WS_VISIBLE：这就是产品想要的组合。
    {
        // 客户区坐标！`SetWindowRgn` 的矩形相对窗口客户区，传屏幕坐标会把整窗裁空。
        let (width, height) = (subject.width, subject.height);
        unsafe {
            let outer = CreateRectRgn(0, 0, width, height);
            let hole = CreateRectRgn(40, 30, width - 40, height - 30);
            CombineRgn(
                outer,
                outer,
                hole,
                RGN_DIFF,
            );
            SetWindowRgn(subject.hwnd, outer, 1);
            windows_sys::Win32::UI::WindowsAndMessaging::SetWindowDisplayAffinity(
                subject.hwnd,
                windows_sys::Win32::UI::WindowsAndMessaging::WDA_EXCLUDEFROMCAPTURE,
            );
        }
    }
    report("h. g + region + exclude", &control, &subject);

    // ── i. 关键一步：加了区域之后**再提交一次** UpdateLayeredWindow ──────
    // 假设：`SetWindowRgn(.., bRedraw=TRUE)` 重画窗口时会把 layered 层的内容清掉，
    // 之后必须重新提交才会重新上屏。
    subject.submit(None);
    report("i. h + resubmit after SetWindowRgn", &control, &subject);

    // ── j. 再确认"先设区域、后提交"这个顺序也能成立（产品要的顺序）──────
    subject.fill([255, 0, 0, 129]);
    subject.submit(None);
    report("j. i + refill (alpha 129) + submit", &control, &subject);

    drop(subject);
    drop(control);
    unsafe { UnregisterClassW(name.as_ptr(), instance) };
}

/// 观测手段可信度检查：一个**普通**（非 layered）的红色窗口 + 桌面基线。
///
/// 如果连它都取不到红色，那 `BitBlt` 在这台机器上根本看不到我们的窗口 ——
/// 之前所有"逐字节相同"的结论作废，必须换观测方式（WGC）。
#[test]
#[ignore = "L3 (docs/31 D-14): needs a live desktop"]
fn the_probe_itself_can_see_a_plain_window() {
    be_dpi_aware();
    let (instance, name) = register("SnapClipLayeredDiffProbePlain");
    // 类背景用纯红实心刷：窗口靠 WM_ERASEBKGND 自己上色，不需要我们画。
    let brush = unsafe { windows_sys::Win32::Graphics::Gdi::CreateSolidBrush(0x0000ff) };
    let mut class: WNDCLASSW = unsafe { std::mem::zeroed() };
    class.lpfnWndProc = Some(wndproc);
    class.hInstance = instance;
    class.hbrBackground = brush;
    // 名字缓冲区要活到 RegisterClassW 之后；用一份持久的 Vec。
    let plain_name = wide("SnapClipLayeredDiffProbePlainRed");
    class.lpszClassName = plain_name.as_ptr();
    assert_ne!(unsafe { RegisterClassW(&class) }, 0, "RegisterClassW 失败");

    let (x, y, w, h) = (80, 520, 240, 120);
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            plain_name.as_ptr(),
            std::ptr::null(),
            WS_POPUP | WS_VISIBLE,
            x,
            y,
            w,
            h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null_mut(),
        )
    };
    assert!(!hwnd.is_null(), "CreateWindowExW 失败：{}", unsafe { GetLastError() });

    // 让它画一次（背景刷在 WM_ERASEBKGND 里由 DefWindowProc 用类刷子填）。
    unsafe { windows_sys::Win32::Graphics::Gdi::InvalidateRect(hwnd, std::ptr::null(), 1) };
    pump(300);

    let rect = {
        let mut rect = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetWindowRect(hwnd, &mut rect) };
        (rect.left, rect.top, rect.right, rect.bottom)
    };
    eprintln!(
        "[diff] plain-window (non-layered, red) = {:?}   desktop baseline = {:?}",
        screen_mean(rect).map(|v| v.round()),
        screen_mean((1200, 1200, 1400, 1300)).map(|v| v.round()),
    );

    unsafe {
        DestroyWindow(hwnd);
        windows_sys::Win32::Graphics::Gdi::DeleteObject(brush as _);
        UnregisterClassW(plain_name.as_ptr(), instance);
        UnregisterClassW(name.as_ptr(), instance);
    }
}
