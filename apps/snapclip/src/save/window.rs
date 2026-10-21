//! The save panel's window shell (`docs/41` S2, step 1).
//!
//! This stage builds the *frame* and nothing else: no fields, no buttons, no painting of rows
//! (`docs/41` S2 step 1). What it settles is the shape every later step has to live inside —
//! which window, which thread, which lifecycle, and what "cancel" means (`docs/42` D12).
//!
//! ## The window is not part of the overlay
//!
//! The panel and the capture frame are **mutually exclusive** (`docs/42` D11): the frame is gone
//! before the panel appears, so the panel is an ordinary top-level window, not a layer inside a
//! layered window. That is what makes native `EDIT` children possible at all, and it is also why
//! the frosted backdrop the mock asks for is reachable — DWM backdrops require an ordinary window.
//!
//! ## Geometry comes from the mock, with line numbers
//!
//! Every constant below is taken from `docs/design/save-panel-demo-v3.html` rather than invented,
//! and the derivation is written next to it. The height is the one value the mock does not state
//! directly: its panel is content-sized, so the shell reserves the height of the mock's default
//! content block and records the arithmetic.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use super::service;

// -------------------------------------------------------------------------------------------------
// Geometry (from the mock)
// -------------------------------------------------------------------------------------------------

/// `--panel-w: 920px` — `save-panel-demo-v3.html:41`.
pub const PANEL_WIDTH_DIP: f32 = 920.0;

/// `--r-panel: 12px` — `save-panel-demo-v3.html:52`.
pub const PANEL_RADIUS_DIP: f32 = 12.0;

/// `--pad: 16px` — `save-panel-demo-v3.html:42`.
pub const PANEL_PADDING_DIP: f32 = 16.0;

/// `--col-gap: 12px` — `save-panel-demo-v3.html:45`.
pub const COLUMN_GAP_DIP: f32 = 12.0;

/// `.head { height: 44px }` — `save-panel-demo-v3.html:94`.
pub const HEADER_HEIGHT_DIP: f32 = 44.0;

/// `--preview-h: 460px` — `save-panel-demo-v3.html:44`.
pub const PREVIEW_HEIGHT_DIP: f32 = 460.0;

/// `.body { padding: 4px 16px 12px }` — `save-panel-demo-v3.html:100`.
pub const BODY_PAD_TOP_DIP: f32 = 4.0;
/// See [`BODY_PAD_TOP_DIP`]; the mock's body padding is `4px 16px 12px`.
pub const BODY_PAD_BOTTOM_DIP: f32 = 12.0;

/// The shell's height before there is any content.
///
/// The mock sizes the panel by its content, so there is no `height` to copy. The default content
/// is header + body padding + the preview column, which is `44 + 4 + 460 + 12 = 520` dip. Later
/// steps replace this with a measurement of the real rows; until then the window reserves exactly
/// the height the mock's default content needs, so the shell looks like the mock rather than like
/// an arbitrary rectangle.
pub const PANEL_HEIGHT_DIP: f32 =
    HEADER_HEIGHT_DIP + BODY_PAD_TOP_DIP + PREVIEW_HEIGHT_DIP + BODY_PAD_BOTTOM_DIP;

/// `--surface: rgba(23,23,28,.92)` — `save-panel-demo-v3.html:25`.
///
/// Kept as the four channels the mock states. The shell only uses this as the **fallback fill**
/// when DWM cannot give us a backdrop: the mock's look is a translucent material over the desktop,
/// and inventing a solid colour for the normal case would be a silent deviation.
pub const SURFACE_RGBA: [u8; 4] = [23, 23, 28, 235];

/// `--blur: 6px` — `save-panel-demo-v3.html:64`.
///
/// Recorded because it is the mock's material, not because we can honour it: DWM's backdrops have
/// their own radii and blurs and are not tunable to six pixels. The substitute is recorded where
/// the backdrop is applied.
pub const BACKDROP_BLUR_DIP: f32 = 6.0;

// -------------------------------------------------------------------------------------------------
// D12: what happens to the capture after the panel is cancelled
// -------------------------------------------------------------------------------------------------

/// Which policy the panel runs when the user cancels (`docs/42` D12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterCancel {
    /// Default: the capture ends with the panel.
    ExitCapture,
    /// Optional: the capture frame comes back so the selection can be adjusted again.
    ReturnToCapture,
}

/// A policy object rather than a boolean, so the "return to the frame" behaviour can be added as a
/// second implementation without touching the panel's state machine.
pub trait AfterCancelPolicy {
    fn after_cancel(&self) -> AfterCancel;
}

/// D12's default.
#[derive(Debug, Default, Clone, Copy)]
pub struct ExitCapture;

impl AfterCancelPolicy for ExitCapture {
    fn after_cancel(&self) -> AfterCancel {
        AfterCancel::ExitCapture
    }
}

/// D12's configurable alternative. The type exists so the seam is real; the behaviour it implies
/// (re-raising the capture frame) belongs to the stage that owns that frame, not to this shell.
#[derive(Debug, Default, Clone, Copy)]
pub struct ReturnToCapture;

impl AfterCancelPolicy for ReturnToCapture {
    fn after_cancel(&self) -> AfterCancel {
        AfterCancel::ReturnToCapture
    }
}

// -------------------------------------------------------------------------------------------------
// Cancellation, shared with the save service
// -------------------------------------------------------------------------------------------------

/// The save service's cancellation signal, in the shape [`service::Cancel`] polls.
///
/// The shell owns it, the save polls it, and closing the panel sets it **before** the state machine
/// moves to closing — that ordering is the point of the type.
#[derive(Debug, Clone, Default)]
pub struct SaveCancellation {
    flag: Arc<AtomicBool>,
}

impl SaveCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the running save to stop. Idempotent.
    pub fn signal(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    pub fn is_signalled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

impl service::Cancel for SaveCancellation {
    fn cancelled(&mut self) -> bool {
        self.is_signalled()
    }
}

// -------------------------------------------------------------------------------------------------
// Lifecycle
// -------------------------------------------------------------------------------------------------

/// Where the panel is in its life. Exactly one at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The window has been asked for but has not reported being shown yet.
    Opening,
    /// Visible, waiting for the user.
    Open,
    /// A save is in flight.
    Saving,
    /// Closing. `Shell::outcome` says why.
    Closing,
}

/// Why the panel closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Committed,
    Cancelled,
    Failed,
}

/// The state machine, deliberately free of Win32.
///
/// Every transition returns `false` when it does not apply, instead of panicking or silently
/// correcting: a panel that receives "save finished" while it is still opening has a bug upstream,
/// and it is cheaper to see that in a test than to guess what the user meant.
pub struct Shell<P: AfterCancelPolicy = ExitCapture> {
    phase: Phase,
    outcome: Option<Outcome>,
    policy: P,
    /// How many times the after-cancel policy has been consulted. D12's promise is "once".
    policy_runs: Arc<AtomicU32>,
    cancellation: SaveCancellation,
}

impl<P: AfterCancelPolicy> Shell<P> {
    pub fn new(policy: P) -> Self {
        Self {
            phase: Phase::Opening,
            outcome: None,
            policy,
            policy_runs: Arc::new(AtomicU32::new(0)),
            cancellation: SaveCancellation::new(),
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn outcome(&self) -> Option<Outcome> {
        self.outcome
    }

    /// The signal the save service polls. Cloning is cheap and stays the **same** signal, which is
    /// what lets a test ask the shell for it and then poll it the way the service does.
    pub fn cancellation(&self) -> SaveCancellation {
        self.cancellation.clone()
    }

    /// `Opening -> Open`, once the window says it is visible.
    pub fn opened(&mut self) -> bool {
        if self.phase == Phase::Opening {
            self.phase = Phase::Open;
            true
        } else {
            false
        }
    }

    /// `Open -> Saving`.
    pub fn start_saving(&mut self) -> bool {
        if self.phase == Phase::Open {
            self.phase = Phase::Saving;
            true
        } else {
            false
        }
    }

    /// `Saving -> Closing(Committed)`.
    pub fn committed(&mut self) -> bool {
        self.finish(Outcome::Committed)
    }

    /// `Saving -> Closing(Failed)`.
    pub fn failed(&mut self) -> bool {
        self.finish(Outcome::Failed)
    }

    fn finish(&mut self, outcome: Outcome) -> bool {
        if self.phase != Phase::Saving {
            return false;
        }
        self.phase = Phase::Closing;
        self.outcome = Some(outcome);
        true
    }

    /// The user asked to close, whatever the phase.
    ///
    /// This is what `Esc` and the close button both mean (`docs/41` S2 step 1 test 2): closing is a
    /// **cancel**, never a commit. While a save is in flight the signal goes out **before** the
    /// phase moves, so the save can stop writing rather than racing the window's destruction.
    ///
    /// Returns the D12 decision exactly once; a second close returns `None`.
    pub fn close(&mut self) -> Option<AfterCancel> {
        if self.phase == Phase::Closing {
            return None;
        }
        if self.phase == Phase::Saving {
            // Ordering matters: the writer must learn about this before anything else happens.
            self.cancellation.signal();
        }
        self.phase = Phase::Closing;
        self.outcome = Some(Outcome::Cancelled);
        self.policy_runs.fetch_add(1, Ordering::SeqCst);
        Some(self.policy.after_cancel())
    }

    /// How many times the after-cancel policy has run. D12: never more than one.
    pub fn after_cancel_runs(&self) -> u32 {
        self.policy_runs.load(Ordering::SeqCst)
    }
}

// -------------------------------------------------------------------------------------------------
// Window
// -------------------------------------------------------------------------------------------------

/// What the shell was asked to open.
#[derive(Debug, Clone, Copy)]
pub struct ShellOptions {
    /// Size in dip, which is the mock's unit.
    pub width_dip: f32,
    pub height_dip: f32,
}

impl Default for ShellOptions {
    fn default() -> Self {
        Self {
            width_dip: PANEL_WIDTH_DIP,
            height_dip: PANEL_HEIGHT_DIP,
        }
    }
}

#[derive(Debug)]
pub enum ShellError {
    /// The window thread could not create the window; the string is the Win32 failure.
    Create(String),
}

/// A live shell window, owned by a thread of its own.
pub struct ShellHandle {
    hwnd: isize,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ShellHandle {
    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    /// Ask the window to close and wait for its thread.
    ///
    /// The taskbar entry, the message queue and the abort path all end here: one posted `WM_CLOSE`
    /// and one join, so a caller that got a handle can always get rid of it.
    pub fn close(mut self) {
        #[cfg(windows)]
        if self.hwnd != 0 {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                    self.hwnd as _,
                    windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                    0,
                    0,
                );
            }
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ShellHandle {
    fn drop(&mut self) {
        // A handle dropped without `close` still must not leak a window thread, and the `Drop` of
        // `ShellHandle::close`'s joined thread is already gone by now.
        #[cfg(windows)]
        if self.hwnd != 0 {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                    self.hwnd as _,
                    windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                    0,
                    0,
                );
            }
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Open the shell on its own thread and return once the window exists.
///
/// Why its own thread: the application's UI runs on GPUI's loop, and a Win32 window with native
/// `EDIT` children needs a Win32 message pump. A dedicated thread keeps both loops honest, makes
/// shutdown a posted message plus a join, and matches how the capture overlay already owns its
/// window.
pub fn open(options: ShellOptions) -> Result<ShellHandle, ShellError> {
    let (ready, created) = std::sync::mpsc::channel::<Result<isize, ShellError>>();
    let thread = std::thread::Builder::new()
        .name("snapclip-save-panel".to_owned())
        .spawn(move || open_and_pump(options, ready))
        .map_err(|error| ShellError::Create(format!("could not start the panel thread: {error}")))?;

    match created.recv() {
        Ok(Ok(hwnd)) => Ok(ShellHandle {
            hwnd,
            thread: Some(thread),
        }),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => {
            let _ = thread.join();
            Err(ShellError::Create(
                "the panel thread exited before reporting a window".to_owned(),
            ))
        }
    }
}

#[cfg(not(windows))]
fn open_and_pump(_options: ShellOptions, ready: std::sync::mpsc::Sender<Result<isize, ShellError>>) {
    let _ = ready.send(Err(ShellError::Create(
        "the save panel is a Win32 window".to_owned(),
    )));
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn open_and_pump(options: ShellOptions, ready: std::sync::mpsc::Sender<Result<isize, ShellError>>) {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetMessageW, LoadCursorW, RegisterClassW, SetWindowLongPtrW,
        SetWindowPos, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, IDC_ARROW,
        MSG, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER, WNDCLASSW, WS_CAPTION, WS_EX_TOOLWINDOW,
        WS_OVERLAPPED, WS_SYSMENU,
    };

    let class_name: Vec<u16> = "SnapClipSavePanel\0".encode_utf16().collect();
    let title: Vec<u16> = "保存\0".encode_utf16().collect();

    static REGISTERED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let registered = *REGISTERED.get_or_init(|| unsafe {
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(std::ptr::null()),
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        RegisterClassW(&class) != 0
    });
    if !registered {
        let _ = ready.send(Err(ShellError::Create(format!(
            "RegisterClassW failed: {}",
            std::io::Error::last_os_error()
        ))));
        return;
    }

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            0,
            0,
            64,
            64,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null_mut(),
        )
    };
    if hwnd.is_null() {
        let _ = ready.send(Err(ShellError::Create(format!(
            "CreateWindowExW failed: {}",
            std::io::Error::last_os_error()
        ))));
        return;
    }

    // The mock's dip become pixels at the window's own DPI, then the window is centred in the work
    // area of the monitor it landed on. Both happen after creation because the DPI comes from the
    // window, not from the desktop.
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let scale = f64::from(dpi) / 96.0;
    let width = (f64::from(options.width_dip) * scale).round().max(1.0) as i32;
    let height = (f64::from(options.height_dip) * scale).round().max(1.0) as i32;

    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let work: RECT = if !monitor.is_null() && GetMonitorInfoW(monitor, &mut info) != 0 {
            info.rcWork
        } else {
            RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            }
        };
        let x = work.left + (work.right - work.left - width).max(0) / 2;
        let y = work.top + (work.bottom - work.top - height).max(0) / 2;
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            x,
            y,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );

        // The material. `DWMWA_SYSTEMBACKDROP_TYPE` only exists on Windows 11 22H2 and later, so
        // the failure is remembered rather than ignored: without a backdrop the window has to paint
        // the mock's surface itself, and a silently transparent window would look broken.
        let backdrop = apply_backdrop(hwnd);
        let renderer = snapclip_ui::panel::PanelRenderer::new().ok();
let state = Box::into_raw(Box::new(WindowState { backdrop, renderer }));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);

        ShowWindow(hwnd, SW_SHOW);
    }

    let _ = ready.send(Ok(hwnd as isize));

    let mut message: MSG = unsafe { std::mem::zeroed() };
    unsafe {
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

/// Per-window state that the WndProc needs. It is boxed and pointed at by `GWLP_USERDATA`, which is
/// the only way to carry Rust data into a Win32 callback without a global.
#[cfg(windows)]
struct WindowState {
    /// Whether DWM agreed to draw a backdrop. When it did not, `WM_PAINT` fills the mock's surface.
    backdrop: bool,
    /// The frame's painter. Absent only if Direct2D could not start, in which case the
    /// fallback fill still gives the window a readable surface.
    renderer: Option<snapclip_ui::panel::PanelRenderer>,
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn apply_backdrop(hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
    unsafe {
    use windows_sys::Win32::Graphics::Dwm::{DwmExtendFrameIntoClientArea, DwmSetWindowAttribute};
    // `MARGINS` is declared with the controls bindings, which is where `DwmExtendFrameIntoClientArea`
    // expects it from; the DWM module does not re-export it.
    use windows_sys::Win32::UI::Controls::MARGINS as DwmMargins;

    // Documented attribute ids. Spelled out rather than imported so that a missing constant in the
    // bindings cannot silently change what we ask for.
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWA_BORDER_COLOR: u32 = 34;
    const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
    const DWMWCP_ROUND: u32 = 2;
    const DWMSBT_TRANSIENTWINDOW: u32 = 2;
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;

    let dark: i32 = 1;
    let corner: u32 = DWMWCP_ROUND;
    let border: u32 = DWMWA_COLOR_NONE;
    let backdrop: u32 = DWMSBT_TRANSIENTWINDOW;
    let size = std::mem::size_of::<u32>() as u32;

    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
        &dark as *const i32 as *const _,
        std::mem::size_of::<i32>() as u32,
    );
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &corner as *const u32 as *const _,
        size,
    );
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_BORDER_COLOR,
        &border as *const u32 as *const _,
        size,
    );
    let attached = DwmSetWindowAttribute(
        hwnd,
        DWMWA_SYSTEMBACKDROP_TYPE,
        &backdrop as *const u32 as *const _,
        size,
    );
    if attached != 0 {
        return false;
    }
    // The client area has no frame of its own, so the backdrop is extended into it. Without this
    // the material only shows under the (removed) caption.
    let margins = DwmMargins {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    DwmExtendFrameIntoClientArea(hwnd, &margins) == 0
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
extern "system" fn window_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    message: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    // One block for the whole procedure: every statement below is an FFI call on a handle the
    // system passed us, and splitting them would spread `unsafe` over forty lines without making
    // any of them more checkable.
    unsafe {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, PAINTSTRUCT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, DestroyWindow, GetWindowLongPtrW, PostQuitMessage, SetWindowPos,
        GWLP_USERDATA, HTCLIENT, HTCAPTION, SWP_NOACTIVATE, SWP_NOZORDER, WM_CLOSE, WM_DESTROY,
        WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_NCCALCSIZE, WM_NCHITTEST, WM_PAINT,
    };

    match message {
        // The mock has its own title bar, so the frame's caption is taken away by claiming the whole
        // window as client area. The window still has `WS_CAPTION`, which is what keeps DWM's
        // rounded corners, shadow and backdrop working.
        WM_NCCALCSIZE if wparam != 0 => 0,
        // The mock's header is the drag area.
        WM_NCHITTEST => {
            let y = (lparam >> 16) as i16 as i32;
            let header = (HEADER_HEIGHT_DIP * dpi_scale(hwnd)).round() as i32;
            if y < header {
                HTCAPTION as isize
            } else {
                HTCLIENT as isize
            }
        }
        WM_ERASEBKGND => {
            // With a backdrop the client must stay empty so DWM's material shows through.
            if state(hwnd).map(|s| s.backdrop).unwrap_or(false) {
                1
            } else {
                0
            }
        }
        WM_PAINT => {
            let mut paint: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut paint);
            let mut client: RECT = std::mem::zeroed();
            windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut client);
            let mut painted = false;
            if let Some(s) = state(hwnd) {
                if let Some(renderer) = s.renderer.as_ref() {
                    let dpi = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd) as f32;
                    let area = snapclip_model::geometry::Rect::new(
                        client.left,
                        client.top,
                        client.right,
                        client.bottom,
                    );
                    // The surface fill is skipped when DWM is drawing the material, so a backdrop
                    // keeps showing through while the frame's own lines still land on top of it.
                    painted = renderer
                        .paint(hdc as *mut _, area, dpi, !s.backdrop)
                        .is_ok();
                }
            }
            if !painted && !state(hwnd).map(|s| s.backdrop).unwrap_or(false) {
                // The mock's surface, as the fallback when DWM refused a backdrop.
                let [r, g, b, _a] = SURFACE_RGBA;
                let colour = u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16);
                let brush = CreateSolidBrush(colour);
                let mut client: RECT = std::mem::zeroed();
                windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut client);
                FillRect(hdc, &client, brush);
                DeleteObject(brush);
            }
            EndPaint(hwnd, &paint);
            0
        }
        WM_DPICHANGED => {
            // lParam is the suggested rect at the new DPI; follow it so the mock's dip stay dip.
            let suggested = &*(lparam as *const RECT);
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            0
        }
        // `Esc` is a cancel, and at this layer cancelling the panel *is* closing it: the shell's
        // state machine owns what that means (`Shell::close`), and the window only has to stop
        // being a window. The close button arrives with the controls.
        WM_KEYDOWN => {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
            if wparam == VK_ESCAPE as usize {
                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
            if !pointer.is_null() {
                drop(Box::from_raw(pointer));
            }
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn state(hwnd: windows_sys::Win32::Foundation::HWND) -> Option<&'static WindowState> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, GWLP_USERDATA};
    unsafe {
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const WindowState;
        pointer.as_ref()
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn dpi_scale(hwnd: windows_sys::Win32::Foundation::HWND) -> f32 {
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96);
    f64::from(dpi) as f32 / 96.0
}

/// What DWM actually granted, read back rather than assumed.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn corner_preference(hwnd: isize) -> Option<u32> {
    read_u32(hwnd, 33)
}

/// See [`corner_preference`]; `None` means the system has no system-backdrop attribute at all.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn system_backdrop(hwnd: isize) -> Option<u32> {
    read_u32(hwnd, 38)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn read_u32(hwnd: isize, attribute: u32) -> Option<u32> {
    use windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute;
    let mut value: u32 = 0;
    let status = unsafe {
        DwmGetWindowAttribute(
            hwnd as _,
            attribute,
            &mut value as *mut u32 as *mut _,
            std::mem::size_of::<u32>() as u32,
        )
    };
    if status == 0 {
        Some(value)
    } else {
        None
    }
}

// -------------------------------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A policy that counts its own invocations, because D12 promises "exactly once".
    struct Counting {
        runs: Arc<AtomicU32>,
        answer: AfterCancel,
    }

    impl AfterCancelPolicy for Counting {
        fn after_cancel(&self) -> AfterCancel {
            self.runs.fetch_add(1, Ordering::SeqCst);
            self.answer
        }
    }

    fn counting() -> (Counting, Arc<AtomicU32>) {
        let runs = Arc::new(AtomicU32::new(0));
        (
            Counting {
                runs: Arc::clone(&runs),
                answer: AfterCancel::ExitCapture,
            },
            runs,
        )
    }

    #[test]
    fn the_shell_opens_at_the_mock_size_and_radius() {
        // The mock's defaults, with the height derivation spelled out so a change is deliberate.
        assert_eq!(PANEL_WIDTH_DIP, 920.0);
        assert_eq!(PANEL_RADIUS_DIP, 12.0);
        assert_eq!(PANEL_PADDING_DIP, 16.0);
        assert_eq!(COLUMN_GAP_DIP, 12.0);
        assert_eq!(
            PANEL_HEIGHT_DIP,
            44.0 + 4.0 + 460.0 + 12.0,
            "the mock's content height: header (94) + body top (100) + preview (44) + body bottom (100)"
        );
    }

    #[test]
    fn a_panel_starts_opening_and_holds_one_phase_at_a_time() {
        let mut shell = Shell::new(ExitCapture);

        assert_eq!(shell.phase(), Phase::Opening);
        assert_eq!(shell.outcome(), None);

        assert!(shell.opened(), "Opening -> Open");
        assert_eq!(shell.phase(), Phase::Open);

        assert!(
            !shell.committed(),
            "a commit before a save is not a legal transition"
        );
        assert_eq!(shell.phase(), Phase::Open, "an illegal event changes nothing");

        assert!(shell.start_saving(), "Open -> Saving");
        assert_eq!(shell.phase(), Phase::Saving);

        assert!(shell.committed(), "Saving -> Closing");
        assert_eq!(shell.phase(), Phase::Closing);
        assert_eq!(shell.outcome(), Some(Outcome::Committed));

        assert!(
            !shell.close().is_some(),
            "closing a closed panel decides nothing"
        );
    }

    #[test]
    fn escape_is_a_cancel_and_not_a_commit() {
        let mut shell = Shell::new(ExitCapture);
        shell.opened();

        let decided = shell.close().expect("the first close decides");

        assert_eq!(decided, AfterCancel::ExitCapture);
        assert_eq!(shell.outcome(), Some(Outcome::Cancelled));
        assert_ne!(shell.outcome(), Some(Outcome::Committed));
        assert_eq!(shell.phase(), Phase::Closing);
    }

    #[test]
    fn cancelling_runs_the_after_cancel_policy_exactly_once() {
        let (policy, runs) = counting();
        let mut shell = Shell::new(policy);
        shell.opened();

        assert_eq!(shell.after_cancel_runs(), 0, "nothing has been decided yet");

        let first = shell.close();
        let second = shell.close();

        assert_eq!(first, Some(AfterCancel::ExitCapture));
        assert_eq!(second, None, "a second close must not decide again");
        assert_eq!(runs.load(Ordering::SeqCst), 1, "D12: exactly once");
        assert_eq!(shell.after_cancel_runs(), 1);
    }

    #[test]
    fn closing_while_saving_cancels_the_save_before_the_window_goes() {
        let mut shell = Shell::new(ExitCapture);
        shell.opened();
        shell.start_saving();

        let signal = shell.cancellation();
        assert!(!signal.is_signalled(), "nothing has cancelled the save yet");

        shell.close();

        assert!(
            signal.is_signalled(),
            "closing must signal the save, or the window dies under a writer"
        );
        // The same object the service polls, not a copy that happens to be equal.
        let mut polled = shell.cancellation();
        assert!(service::Cancel::cancelled(&mut polled));
        assert_eq!(shell.outcome(), Some(Outcome::Cancelled));
    }

    #[test]
    #[ignore = "needs an interactive desktop"]
    fn the_panel_window_opens_and_closes() {
        let shell = open(ShellOptions::default()).expect("the shell window opens");

        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;
            let hwnd = shell.hwnd();
            assert_ne!(hwnd, 0);
            assert_ne!(
                unsafe { IsWindow(hwnd as _) },
                0,
                "the window handle is not a window"
            );

            // What DWM actually granted, printed rather than assumed: the mock's 12 dip radius and
            // 6 dip blur are not things a Win32 window can request, so the report has to say what
            // it got instead.
            let corner = corner_preference(hwnd);
            let backdrop = system_backdrop(hwnd);
            eprintln!("[save-panel] corner preference = {corner:?}, backdrop = {backdrop:?}");

            shell.close();

            assert_eq!(
                unsafe { IsWindow(hwnd as _) },
                0,
                "closing the handle leaves no window behind"
            );
        }
    }
}
