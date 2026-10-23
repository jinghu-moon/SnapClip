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
// Two facts that were each learned by shipping a panel nobody could see
// -------------------------------------------------------------------------------------------------

/// The panel's extended window style.
///
/// It **must** contain `WS_EX_TOPMOST`: the two windows this panel replaces, the capture overlay
/// and its dim layer, are both topmost. A panel created without that bit is painted underneath
/// them and still reports `IsWindowVisible` as true, because every check that only asks the window
/// about itself answers yes. That is exactly how a panel that is never seen passes every test, and
/// it is why this is a constant with a test rather than a literal at the call site.
pub(crate) const PANEL_EX_STYLE: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WS_EX_TOOLWINDOW
    | windows_sys::Win32::UI::WindowsAndMessaging::WS_EX_TOPMOST;

/// Replace the panel in `slot`: **close the old one, then create the new one, then store it.**
///
/// The order is the contract, not an implementation detail. A window handle is reused once its
/// window is destroyed, so closing a stale handle *after* the new panel exists can post
/// `WM_CLOSE` to the panel that was just opened, because the system handed it the same number.
/// Closing first removes the window the stale number could have referred to.
///
/// `create` is a factory rather than a value so that the whole sequence lives inside this
/// function: a caller that builds the new panel before calling in would reintroduce the bug the
/// order exists to prevent. The sequence is asserted in `panel_slot_tests` through the two
/// closures, which the test supplies and therefore observes.
pub(crate) fn replace_panel<T, E>(
    slot: &std::sync::Mutex<Option<T>>,
    create: impl FnOnce() -> Result<T, E>,
    close: impl FnOnce(T),
) -> Result<(), E> {
    let previous = slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(previous) = previous {
        close(previous);
    }
    let next = create()?;
    slot.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .replace(next);
    Ok(())
}

#[cfg(test)]
mod panel_slot_tests {
    use super::{PANEL_EX_STYLE, replace_panel};
    use std::sync::Mutex;
    use windows_sys::Win32::UI::WindowsAndMessaging::{WS_EX_TOOLWINDOW, WS_EX_TOPMOST};

    #[test]
    fn the_panel_asks_to_be_above_the_windows_it_replaces() {
        assert_ne!(
            PANEL_EX_STYLE & WS_EX_TOPMOST,
            0,
            "the capture overlay and its dim layer are topmost, so a panel without this bit is \
             painted underneath them and still reports itself visible"
        );
        assert_ne!(
            PANEL_EX_STYLE & WS_EX_TOOLWINDOW,
            0,
            "the panel stays out of the taskbar"
        );
    }

    #[test]
    fn the_previous_panel_is_closed_before_the_new_one_exists() {
        let slot: Mutex<Option<u32>> = Mutex::new(Some(1));
        let order: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
        // What the closer sees is the whole point: if the slot still names the old panel while
        // the new window already exists, a stale handle can name the window that took its place.
        let seen = Mutex::new(None);
        replace_panel(
            &slot,
            || {
                order.lock().unwrap().push("create");
                Ok::<u32, ()>(2)
            },
            |previous| {
                order.lock().unwrap().push("close");
                *seen.lock().unwrap() = Some((previous, *slot.lock().unwrap()));
            },
        )
        .unwrap();

        assert_eq!(
            *seen.lock().unwrap(),
            Some((1, None)),
            "the closer must run while the slot is empty: that is what keeps a stale handle from \
             naming the panel that was just created"
        );
        assert_eq!(
            *order.lock().unwrap(),
            vec!["close", "create"],
            "close, then create: creating before closing is how a stale handle ends up naming the \
             new panel"
        );
        assert_eq!(*slot.lock().unwrap(), Some(2));
    }
}

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
#[derive(Debug, Clone)]
pub struct ShellOptions {
    /// Size in dip, which is the mock's unit.
    pub width_dip: f32,
    pub height_dip: f32,
    /// The captured picture for the preview card: tightly packed top down BGRA, then width and
    /// height. `None` is the development entry's empty panel. Owned rather than borrowed because
    /// the options cross a thread boundary and the picture outlives the caller.
    pub preview: Option<(Vec<u8>, u32, u32)>,
    /// The captured size in pixels, shown in the two size cells. It comes from the same selection
    /// the picture does, so it is absent exactly when the picture is: the development entry opens
    /// a panel with no capture, and a panel with no capture has no size to show. Writing a
    /// plausible number there would be inventing the answer to a question the panel exists to
    /// answer.
    pub capture_size: Option<(u32, u32)>,
}

impl Default for ShellOptions {
    fn default() -> Self {
        Self {
            width_dip: PANEL_WIDTH_DIP,
            height_dip: PANEL_HEIGHT_DIP,
            preview: None,
            capture_size: None,
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
        self.request_close();
        self.join();
        // The window is gone and the thread is joined; clearing the handle keeps `Drop` from
        // posting a second `WM_CLOSE` to a number the system is free to hand to another window.
        self.hwnd = 0;
    }

    /// Wait for the window to be closed by the user, then join. Used by the development entry, which
    /// has no other reason to stay alive.
    pub fn wait(mut self) {
        self.join();
    }

    fn request_close(&mut self) {
        #[cfg(windows)]
        if self.hwnd != 0 {
            // A window handle is reused once its window is destroyed, and this handle may belong
            // to a panel the user already closed. Posting `WM_CLOSE` to a reused number closes
            // whatever window inherited it — in practice the panel that was just opened, which is
            // how a second `Ctrl+S` produced neither a panel nor an error. Ask whether the window
            // still exists first, and skip the post when it does not.
            let alive = unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(self.hwnd as _)
            } != 0;
            eprintln!(
                "[save-panel] request_close hwnd={:#x} alive={alive} -> {}",
                self.hwnd,
                if alive { "posting WM_CLOSE" } else { "skipping (stale handle)" }
            );
            if alive {
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                        self.hwnd as _,
                        windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                        0,
                        0,
                    );
                }
            }
        }
    }

    fn join(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ShellHandle {
    fn drop(&mut self) {
        // A handle dropped without `close` still must not leak a window thread, and the `Drop` of
        // `ShellHandle::close`'s joined thread is already gone by now. This goes through the same
        // guarded request as `close`, because a dropped handle is the case most likely to be stale.
        self.request_close();
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
fn open_and_pump(mut options: ShellOptions, ready: std::sync::mpsc::Sender<Result<isize, ShellError>>) {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetClassNameW, GetMessageW, GetWindow, GetWindowLongW,
        GetWindowRect, LoadCursorW, RegisterClassW, SetWindowLongPtrW, SetWindowPos, ShowWindow,
        TranslateMessage, CS_HREDRAW, CS_VREDRAW, GWL_EXSTYLE, GWLP_USERDATA, GW_HWNDPREV, IDC_ARROW,
        MSG, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER, WNDCLASSW, WS_CAPTION,
        WS_EX_TOPMOST, WS_OVERLAPPED, WS_SYSMENU,
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
            PANEL_EX_STYLE,
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
        // The child's background: the field's own fill, resolved from `--fill` over the surface.
        // A native `EDIT` cannot be translucent, so the composite is computed once here.
        let fill = snapclip_ui::panel::field_colours(
            snapclip_ui::panel::FieldState::Idle,
            SURFACE_RGBA,
        )
        .fill;
        let fill_colour = u32::from(fill[0]) | (u32::from(fill[1]) << 8) | (u32::from(fill[2]) << 16);
        let fill_brush = windows_sys::Win32::Graphics::Gdi::CreateSolidBrush(fill_colour);
        // The quality control starts on the first option, at this window's scale. It is built here
        // rather than lazily so the first paint has a real choice to mark.
        let scale = dpi_scale(hwnd);
        let metrics = snapclip_ui::panel::DEMO;
        let rects = metrics.rects(scale);
        let state = Box::into_raw(Box::new(WindowState {
            backdrop,
            renderer,
            preview: options.preview.take(),
            fields: snapclip_ui::panel::FieldStates::IDLE,
            controls: [std::ptr::null_mut(); snapclip_ui::panel::FieldId::ALL.len()],
            fill_brush,
            segment: snapclip_ui::panel::quality_segment_state(&metrics, &rects, scale, 0),
            segment_scale: scale,
        }));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);

        // Four fields, one constructor: real `EDIT` children, each inset inside the frame the frame
        // painter draws for its own rect (`:542` the path row, `:556` the name row, `:235` the two
        // size cells). Their font is the embedded subset registered for GDI, so the native controls
        // and the drawn panel share one face without installing anything on the machine.
        //
        // The loop is over the field list rather than over four hand-written calls: a field is an
        // entry in that list, so the fifth one is a variant plus a rect, not another copy of this.
        //
        // The size cells show the captured dimensions, which come from the selection the panel was
        // opened with. `initial_text` is a free function rather than a closure so that the pairing
        // of width and height can be asserted without a window: they are two numbers of the same
        // type, and swapping them would look entirely reasonable on screen.
        let dpi = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96);
        let scale = (f64::from(dpi) / 96.0) as f32;
        let metrics = snapclip_ui::panel::DEMO;
        let rects = metrics.rects(scale);
        // The numeric face has to be resolvable before the first paint asks DirectWrite for it, so
        // it is registered here rather than relying on the controls below happening to register it
        // first. `AddFontMemResourceEx` is idempotent, and both subsets stay registered for the life
        // of the process.
        snapclip_ui::panel::register_number_font();
        let initial_text = |id| initial_text(id, options.capture_size);
        if let Some(s) = (state as *mut WindowState).as_mut() {
            for id in snapclip_ui::panel::FieldId::ALL {
                let control = create_field_edit(
                    hwnd,
                    id.rect(&metrics, &rects, scale),
                    &initial_text(id),
                    id.face(),
                );
                s.set_control(id, control);
            }
        }

        ShowWindow(hwnd, SW_SHOW);
        {
            let mut rect: windows_sys::Win32::Foundation::RECT = std::mem::zeroed();
            let got = windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rect);
            let visible = windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd);
            let mut screen: windows_sys::Win32::Foundation::RECT = std::mem::zeroed();
            let screen_ok = GetWindowRect(hwnd, &mut screen);
            let own_topmost =
                GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0;
            eprintln!(
                "[save-panel] shown hwnd={hwnd:p} visible={visible} client={}x{} (GetClientRect={got}) \
                 rect=({},{})-({},{}) (GetWindowRect={screen_ok}) topmost={own_topmost}",
                rect.right - rect.left,
                rect.bottom - rect.top,
                screen.left,
                screen.top,
                screen.right,
                screen.bottom,
            );
            // Who is above us, and are they topmost? The capture side's overlay and dim layer are,
            // so if either is still up and visible the panel is painted underneath it no matter what
            // the window says about itself. Class names identify them: SnapClipCaptureOverlay and the
            // dim layer's class.
            let mut above = GetWindow(hwnd, GW_HWNDPREV);
            let mut step = 0;
            while !above.is_null() && step < 6 {
                let mut class: [u16; 128] = [0; 128];
                let len = GetClassNameW(above, class.as_mut_ptr(), class.len() as i32);
                let name = String::from_utf16_lossy(&class[..len.max(0) as usize]);
                let ex = GetWindowLongW(above, GWL_EXSTYLE) as u32;
                let above_visible =
                    windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(above);
                eprintln!(
                    "[save-panel]   above[{step}] hwnd={above:p} class={name:?} \
                     visible={above_visible} topmost={}",
                    ex & WS_EX_TOPMOST != 0
                );
                above = GetWindow(above, GW_HWNDPREV);
                step += 1;
            }
        }
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
    /// The frame state of **every** form field, keyed by identity. The native `EDIT` children draw no
    /// border, no focus ring and no error colour (`snapclip_ui::panel::field`), so the frame is told
    /// what to draw around each of them.
    fields: snapclip_ui::panel::FieldStates,
    /// The child window for each field, indexed the same way `FieldId::ALL` is. Every question about a
    /// control — what colours it, which field just took focus — is answered by finding its identity in
    /// this table, so adding a field is a new table entry rather than a new branch in the message
    /// handlers. Reading a single shared state for every control is what this replaced.
    controls: [windows_sys::Win32::Foundation::HWND; snapclip_ui::panel::FieldId::ALL.len()],
    /// The captured picture, handed to the frame painter so the preview card shows the real
    /// selection rather than a stand-in.
    preview: Option<(Vec<u8>, u32, u32)>,
    /// The brush `WM_CTLCOLOREDIT` returns. The child paints its own background, and on a dark panel
    /// the default white would show as a lit rectangle, so the field's resolved fill is handed over
    /// instead. Created once and owned by this state.
    fill_brush: windows_sys::Win32::Graphics::Gdi::HBRUSH,
    /// The quality control's state. Its geometry comes from the same producer the painter draws
    /// with, so a click lands on the option whose block the user can see, and its selection is owned
    /// by `segmented`'s state machine rather than by a second set of rules here. The chosen option is
    /// read from this, never kept beside it.
    segment: snapclip_ui::panel::SegmentState,
    /// The scale the segment geometry was built at. The rectangles are pixels, so a window that
    /// moves between monitors has to rebuild them.
    segment_scale: f32,
}

impl WindowState {
    /// The child window of one field, or null before it has been created.
    fn control(&self, id: snapclip_ui::panel::FieldId) -> windows_sys::Win32::Foundation::HWND {
        self.controls[Self::slot(id)]
    }

    /// Record a field's child window. The only writer of the table.
    fn set_control(
        &mut self,
        id: snapclip_ui::panel::FieldId,
        hwnd: windows_sys::Win32::Foundation::HWND,
    ) {
        self.controls[Self::slot(id)] = hwnd;
    }

    /// Which field a child window is, if any. This is the lookup that keeps the message handlers free
    /// of per-field branches: `WM_CTLCOLOREDIT` and `WM_COMMAND` both start here.
    ///
    /// An unregistered handle is not a field. The table starts as nulls, so a lookup that matched on
    /// equality alone would answer with the first field for a handle nobody created — which is the
    /// same mistake as reading one shared state, one layer down.
    fn field_of(
        &self,
        hwnd: windows_sys::Win32::Foundation::HWND,
    ) -> Option<snapclip_ui::panel::FieldId> {
        if hwnd.is_null() {
            return None;
        }
        snapclip_ui::panel::FieldId::ALL
            .into_iter()
            .find(|id| self.control(*id) == hwnd)
    }

    /// A field's index in the table, taken from the field list so the two cannot drift.
    fn slot(id: snapclip_ui::panel::FieldId) -> usize {
        snapclip_ui::panel::FieldId::ALL
            .iter()
            .position(|candidate| *candidate == id)
            .expect("every field is in the list")
    }

    /// The option the quality control marks as chosen.
    fn chosen_quality(&self) -> usize {
        snapclip_ui::panel::chosen_quality(&self.segment)
    }

    /// The segment's state at this scale, rebuilt when the scale has moved since it was built. What
    /// the user picked survives the rebuild: a DPI change must not change the choice.
    fn segment_at_scale(&mut self, scale: f32) -> &mut snapclip_ui::panel::SegmentState {
        if self.segment_scale != scale {
            let metrics = snapclip_ui::panel::DEMO;
            let rects = metrics.rects(scale);
            let chosen = self.chosen_quality();
            self.segment =
                snapclip_ui::panel::quality_segment_state(&metrics, &rects, scale, chosen);
            self.segment_scale = scale;
        }
        &mut self.segment
    }

    /// Feed one pointer event to the quality control, and report whether the choice changed.
    ///
    /// This is the only writer of the chosen option. Both the hit rectangles and the drawn
    /// rectangles come from `snapclip_ui::panel`, and the press-and-release pairing is the state
    /// machine's, so this adds no second opinion about either.
    fn quality_pointer(
        &mut self,
        scale: f32,
        event: snapclip_ui::panel::PointerEvent,
        point: snapclip_model::geometry::Point,
    ) -> bool {
        let before = self.chosen_quality();
        let chosen = snapclip_ui::panel::quality_pointer(self.segment_at_scale(scale), event, point);
        chosen.is_some() && self.chosen_quality() != before
    }
}

/// Create the name field: a real `EDIT` child, inset inside the frame the frame painter draws.
///
/// Two things make this a native control rather than a drawn one. The font is the **embedded
/// What each field holds when the panel opens.
///
/// A free function rather than a closure so that the one thing that can go wrong here can be
/// asserted without a window: the width and the height are two numbers of the same type, and
/// handing the width to the height cell would look entirely reasonable on screen. The path row
/// starts empty because the panel's own default is a question for the next stage, and the name
/// row starts with the mockup's default name.
fn initial_text(
    id: snapclip_ui::panel::FieldId,
    capture_size: Option<(u32, u32)>,
) -> String {
    use snapclip_ui::panel::FieldId;
    match (id, capture_size) {
        (FieldId::Name, _) => "捕获".to_string(),
        (FieldId::SizeWidth, Some((width, _))) => width.to_string(),
        (FieldId::SizeHeight, Some((_, height))) => height.to_string(),
        // No capture, no number. The development entry opens a panel with nothing captured, and a
        // plausible size there would be invented rather than measured.
        _ => String::new(),
    }
}

/// subset**, registered for GDI from memory with `AddFontMemResourceEx`, so the child shares the
/// panel's face without the family being installed on the machine. And the child is **inset** by
/// `EDIT_INSET_DIP`, so the rounded corner the user sees is the frame's, not the child's square one.
#[cfg(windows)]
#[allow(unsafe_code)]
fn create_field_edit(
    parent: windows_sys::Win32::Foundation::HWND,
    field: snapclip_model::geometry::Rect,
    text: &str,
    face: snapclip_ui::panel::FieldFace,
) -> windows_sys::Win32::Foundation::HWND {
    use windows_sys::Win32::Graphics::Gdi::{
        AddFontMemResourceEx, CreateFontW, CLEARTYPE_QUALITY, DEFAULT_CHARSET, FF_DONTCARE,
        FW_SEMIBOLD,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, GetClientRect, ES_AUTOHSCROLL, ES_RIGHT, WS_CHILD, WS_TABSTOP, WS_VISIBLE,
    };

    unsafe {
        // Register the subset from memory. The returned handle is intentionally never removed: the
        // registration lives as long as the process, and `RemoveFontMemResourceEx` on a handle the
        // GDI objects still reference would pull the face out from under them.
        let mut loaded: u32 = 0;
        // The face decides which subset has to be registered: the size cells are set in the numeric
        // font, everything else in the UI font. Both are registered from memory and both stay
        // registered for the life of the process.
        let bytes = match face {
            snapclip_ui::panel::FieldFace::Text => snapclip_capture::overlay_font::SUBSET,
            snapclip_ui::panel::FieldFace::Number => snapclip_ui::panel::NUMBER_SUBSET,
        };
        let _ = AddFontMemResourceEx(
            bytes.as_ptr() as *const core::ffi::c_void,
            bytes.len() as u32,
            std::ptr::null_mut(),
            &mut loaded,
        );

        let dpi = GetDpiForWindow(parent).max(96);
        let scale = f64::from(dpi) / 96.0;
        let inset = (f64::from(snapclip_ui::panel::EDIT_INSET_DIP) * scale).round() as i32;

        let family_name = match face {
            snapclip_ui::panel::FieldFace::Text => snapclip_ui::panel::FAMILY,
            snapclip_ui::panel::FieldFace::Number => snapclip_ui::panel::NUMBER_FAMILY,
        };
        let family: Vec<u16> = family_name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // The font is `--fs-input`, not the field's height. They were confused once and the control
        // filled its own box edge to edge.
        // The font size comes from the face: `--fs-input` for a field, `--fs-num` for a number.
        // The field's height was confused with its font size once, and the control filled its box.
        let height = snapclip_ui::panel::edit_font_height_for(scale as f32, face.font_dip());
        let font = CreateFontW(
            height,
            0,
            0,
            0,
            FW_SEMIBOLD as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            CLEARTYPE_QUALITY as u32,
            FF_DONTCARE as u32,
            family.as_ptr(),
        );

        let class: Vec<u16> = "EDIT\0".encode_utf16().collect();
        let text: Vec<u16> = format!("{text}\0").encode_utf16().collect();
        let edit = CreateWindowExW(
            0,
            class.as_ptr(),
            text.as_ptr(),
            // `.dim-input { text-align: right }` (`:242`): a number is right aligned in its control,
            // so the digits line up against the unit beside them.
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | ES_AUTOHSCROLL as u32
                | if face.right_aligned() { ES_RIGHT as u32 } else { 0 },
            field.left + inset,
            field.top + inset,
            (field.right - field.left - inset * 2).max(1),
            (field.bottom - field.top - inset * 2).max(1),
            parent,
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null_mut(),
        );
        if edit.is_null() {
            return edit;
        }

        use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;
        const WM_SETFONT: u32 = 0x0030;
        SendMessageW(edit, WM_SETFONT, font as usize, 1);

        // The child is inside the frame's rect, so its own square corners are covered by the frame's
        // fill; nothing else has to be clipped.
        let mut client: windows_sys::Win32::Foundation::RECT = std::mem::zeroed();
        GetClientRect(parent, &mut client);
        edit
    }
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
        BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect, PAINTSTRUCT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, DestroyWindow, GetWindowLongPtrW, PostQuitMessage, SetWindowPos,
        GWLP_USERDATA, HTCLIENT, HTCAPTION, SWP_NOACTIVATE, SWP_NOZORDER, WM_CLOSE, WM_DESTROY,
        WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_NCCALCSIZE,
        WM_NCHITTEST, WM_PAINT,
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
                        .paint_with_preview(
                            hdc as *mut _,
                            area,
                            dpi,
                            !s.backdrop,
                            s.fields,
                            s.preview.as_ref().map(|(bgra, w, h)| (bgra.as_slice(), *w, *h)),
                            s.chosen_quality(),
                        )
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
        // The quality control is drawn by the frame rather than being a child window, so its clicks
        // arrive here. The coordinates in `lParam` are client pixels, and the segment's rectangles
        // are pixels too — `rects(scale)` has already converted every dip — so the point is used as
        // it arrives and no conversion happens twice.
        WM_LBUTTONDOWN | WM_LBUTTONUP => {
            let point = snapclip_model::geometry::Point::new(
                (lparam & 0xffff) as i16 as i32,
                ((lparam >> 16) & 0xffff) as i16 as i32,
            );
            let event = if message == WM_LBUTTONDOWN {
                snapclip_ui::panel::PointerEvent::Down
            } else {
                snapclip_ui::panel::PointerEvent::Up
            };
            // The close button is drawn by the frame, so its clicks arrive here too. The button is
            // hit tested, not the 14 dip glyph inside it: the mockup's button is 26 dip (`:96`) and
            // its svg is 14 (`:98`), and a corner target should be the size of the thing that looks
            // clickable. Closing goes through the same message `Esc` posts, and the window posts it
            // to its own handle, so there is no stale number to aim at.
            if event == snapclip_ui::panel::PointerEvent::Down {
                let metrics = snapclip_ui::panel::DEMO;
                let rects = metrics.rects(dpi_scale(hwnd));
                if snapclip_ui::panel::close_button_contains(&rects, dpi_scale(hwnd), (point.x, point.y))
                {
                    windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(hwnd, WM_CLOSE, 0, 0);
                    return 0;
                }
            }
            let chosen = state_mut(hwnd)
                .map(|s| s.quality_pointer(dpi_scale(hwnd), event, point))
                .unwrap_or(false);
            if chosen {
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
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
        // The child's colours. An `EDIT` paints its own background from the system colours, which on
        // a dark panel is a lit white rectangle, so the field's resolved fill and its text colour are
        // handed over here instead.
        //
        // Which field is asking comes from the table, not from a state the window happens to track:
        // `lparam` is the child's handle, and the answer is that child's own entry. Reading one shared
        // state here is what made a focused path row paint the name row's colours, and it would have
        // gone on being wrong for every field added afterwards.
        WM_CTLCOLOREDIT => {
            if let Some(s) = state(hwnd) {
                let state_of_field = s
                    .field_of(lparam as windows_sys::Win32::Foundation::HWND)
                    .map(|id| s.fields.state_of(id))
                    .unwrap_or(snapclip_ui::panel::FieldState::Idle);
                let colours = snapclip_ui::panel::field_colours(state_of_field, SURFACE_RGBA);
                let child = wparam as windows_sys::Win32::Graphics::Gdi::HDC;
                windows_sys::Win32::Graphics::Gdi::SetBkColor(child, bgr(colours.fill));
                windows_sys::Win32::Graphics::Gdi::SetTextColor(child, bgr(colours.text));
                return s.fill_brush as isize;
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        // Focus is not the child's business: the border and the ring around it belong to the frame,
        // so the window is invalidated and the frame repainted with the new state. The field is found
        // in the table rather than compared against each control in turn, so the next field added is
        // wired by existing.
        //
        // Repainting the whole frame rather than just the field's rect is deliberate — one panel of
        // six fields is a few hundred microseconds, and a dirty-rect scheme would be a second geometry
        // path to keep in step with the first (`docs/41` S2).
        WM_COMMAND => {
            const EN_SETFOCUS: u16 = 0x0100;
            const EN_KILLFOCUS: u16 = 0x0200;
            let notification = ((wparam >> 16) & 0xFFFF) as u16;
            let control = lparam as windows_sys::Win32::Foundation::HWND;
            let mut repaint = false;
            if let Some(s) = state_mut(hwnd) {
                if let Some(id) = s.field_of(control) {
                    let next = match notification {
                        EN_SETFOCUS => Some(snapclip_ui::panel::FieldState::Focused),
                        EN_KILLFOCUS => Some(snapclip_ui::panel::FieldState::Idle),
                        _ => None,
                    };
                    if let Some(next) = next {
                        s.fields.set(id, next);
                        repaint = true;
                    }
                }
            }
            if repaint {
                windows_sys::Win32::Graphics::Gdi::InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_CLOSE => {
            eprintln!("[save-panel] WM_CLOSE received hwnd={hwnd:p}");
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

/// The same pointer, mutably. The window thread is the only writer — the state is created on it, read
/// by its own message procedure, and dropped by it — so there is no aliasing to guard against.
#[cfg(windows)]
#[allow(unsafe_code)]
fn state_mut(hwnd: windows_sys::Win32::Foundation::HWND) -> Option<&'static mut WindowState> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, GWLP_USERDATA};
    unsafe {
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
        pointer.as_mut()
    }
}

/// GDI wants `0x00BBGGRR`, which is the reverse of the byte order the palette is written in.
#[cfg(windows)]
fn bgr(rgba: [u8; 4]) -> u32 {
    u32::from(rgba[0]) | (u32::from(rgba[1]) << 8) | (u32::from(rgba[2]) << 16)
}

/// `WM_CTLCOLOREDIT`: the child asks its parent what to paint its background with.
#[cfg(windows)]
const WM_CTLCOLOREDIT: u32 = 0x0133;

/// `WM_COMMAND`: the child reports focus changes through it.
#[cfg(windows)]
const WM_COMMAND: u32 = 0x0111;

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

    /// The two size cells are two numbers of the same type, so nothing about the type stops the
    /// width from being handed to the height cell. On screen that would look entirely reasonable,
    /// which is why the pairing is asserted here rather than noticed later.
    #[test]
    fn the_size_cells_take_the_width_and_the_height_in_that_order() {
        let size = Some((1440, 900));
        assert_eq!(
            initial_text(snapclip_ui::panel::FieldId::SizeWidth, size),
            "1440"
        );
        assert_eq!(
            initial_text(snapclip_ui::panel::FieldId::SizeHeight, size),
            "900"
        );
    }

    /// The development entry opens a panel with nothing captured. A plausible size there would be
    /// invented rather than measured, and the panel exists to tell the user what they captured.
    #[test]
    fn a_panel_without_a_capture_shows_no_size() {
        for id in [
            snapclip_ui::panel::FieldId::SizeWidth,
            snapclip_ui::panel::FieldId::SizeHeight,
        ] {
            assert_eq!(initial_text(id, None), "", "{id:?} must stay empty");
        }
    }

    /// The name row's default is the mockup's, and the path row has no default yet.
    #[test]
    fn the_name_row_starts_with_the_mocks_default_name() {
        assert_eq!(
            initial_text(snapclip_ui::panel::FieldId::Name, None),
            "捕获"
        );
        assert_eq!(initial_text(snapclip_ui::panel::FieldId::Path, None), "");
    }

    /// The control table is the one place a field's identity is decided, and the message handlers ask
    /// it rather than comparing handles one field at a time. What has to hold is that the lookup is
    /// unambiguous and that a handle nobody registered is not mistaken for a field.
    #[test]
    fn the_control_table_answers_with_one_field_per_handle() {
        let mut state = WindowState {
            backdrop: false,
            renderer: None,
            fields: snapclip_ui::panel::FieldStates::IDLE,
            controls: [std::ptr::null_mut(); snapclip_ui::panel::FieldId::ALL.len()],
            preview: None,
            fill_brush: std::ptr::null_mut(),
            segment: snapclip_ui::panel::quality_segment_state(
                &snapclip_ui::panel::DEMO,
                &snapclip_ui::panel::DEMO.rects(1.0),
                1.0,
                0,
            ),
            segment_scale: 1.0,
        };

        // Every field has its own slot: no two fields share an index, which is what would let one
        // field's state answer for another.
        let mut slots: Vec<usize> = snapclip_ui::panel::FieldId::ALL
            .into_iter()
            .map(WindowState::slot)
            .collect();
        slots.sort_unstable();
        slots.dedup();
        assert_eq!(
            slots.len(),
            snapclip_ui::panel::FieldId::ALL.len(),
            "each field needs its own entry in the table"
        );

        // A handle nobody registered is not a field, so the handlers fall back to idle rather than
        // to whichever field happened to be first.
        assert_eq!(state.field_of(std::ptr::null_mut()), None);

        // And a registered handle resolves to the field that registered it.
        let handle = 0x1234 as windows_sys::Win32::Foundation::HWND;
        state.set_control(snapclip_ui::panel::FieldId::SizeHeight, handle);
        assert_eq!(
            state.field_of(handle),
            Some(snapclip_ui::panel::FieldId::SizeHeight),
            "a control answers as the field that owns it"
        );
        assert_eq!(state.control(snapclip_ui::panel::FieldId::SizeHeight), handle);
        for id in snapclip_ui::panel::FieldId::ALL {
            if id != snapclip_ui::panel::FieldId::SizeHeight {
                assert_eq!(state.control(id), std::ptr::null_mut(), "{id:?} was not registered");
            }
        }
    }

    /// The window's own writer for the quality choice: a click on an option changes it, a click
    /// anywhere else does not, and a press released over a different option is not a choice.
    ///
    /// This is the half the `snapclip-ui` test cannot reach, because the choice belongs to the
    /// window that receives the messages rather than to the frame that draws the block.
    #[test]
    fn the_window_is_the_only_writer_of_the_quality_choice() {
        let mut state = WindowState {
            backdrop: false,
            renderer: None,
            fields: snapclip_ui::panel::FieldStates::IDLE,
            controls: [std::ptr::null_mut(); snapclip_ui::panel::FieldId::ALL.len()],
            preview: None,
            fill_brush: std::ptr::null_mut(),
            segment: snapclip_ui::panel::quality_segment_state(
                &snapclip_ui::panel::DEMO,
                &snapclip_ui::panel::DEMO.rects(1.0),
                1.0,
                0,
            ),
            segment_scale: 1.0,
        };

        let centre = |index: usize| {
            let metrics = snapclip_ui::panel::DEMO;
            let rects = metrics.rects(1.0);
            let rect = snapclip_ui::panel::quality_option_rect(&metrics, &rects, 1.0, index);
            snapclip_model::geometry::Point::new(
                (rect.left + rect.right) / 2,
                (rect.top + rect.bottom) / 2,
            )
        };

        assert_eq!(state.chosen_quality(), 0, "the panel opens on the first option");

        // A click on the second option: press and release on the same one.
        state.quality_pointer(1.0, snapclip_ui::panel::PointerEvent::Down, centre(1));
        assert!(
            state.quality_pointer(1.0, snapclip_ui::panel::PointerEvent::Up, centre(1)),
            "clicking an option is a change the window has to repaint"
        );
        assert_eq!(state.chosen_quality(), 1);

        // Clicking the option that is already chosen is not a change, so nothing needs repainting.
        state.quality_pointer(1.0, snapclip_ui::panel::PointerEvent::Down, centre(1));
        assert!(
            !state.quality_pointer(1.0, snapclip_ui::panel::PointerEvent::Up, centre(1)),
            "choosing what is already chosen is not a change"
        );
        assert_eq!(state.chosen_quality(), 1);

        // A press released elsewhere is not a choice either.
        state.quality_pointer(1.0, snapclip_ui::panel::PointerEvent::Down, centre(0));
        assert!(!state.quality_pointer(1.0, snapclip_ui::panel::PointerEvent::Up, centre(2)));
        assert_eq!(state.chosen_quality(), 1);

        // And a click that misses the track entirely changes nothing.
        assert!(!state.quality_pointer(
            1.0,
            snapclip_ui::panel::PointerEvent::Down,
            snapclip_model::geometry::Point::new(0, 0)
        ));
    }

    /// The scale the geometry was built at is remembered, so a window that moves to a monitor with
    /// a different scale rebuilds the rectangles without losing the choice.
    #[test]
    fn a_scale_change_rebuilds_the_geometry_and_keeps_the_choice() {
        let mut state = WindowState {
            backdrop: false,
            renderer: None,
            fields: snapclip_ui::panel::FieldStates::IDLE,
            controls: [std::ptr::null_mut(); snapclip_ui::panel::FieldId::ALL.len()],
            preview: None,
            fill_brush: std::ptr::null_mut(),
            segment: snapclip_ui::panel::quality_segment_state(
                &snapclip_ui::panel::DEMO,
                &snapclip_ui::panel::DEMO.rects(1.0),
                1.0,
                2,
            ),
            segment_scale: 1.0,
        };

        let metrics = snapclip_ui::panel::DEMO;
        let before = snapclip_ui::panel::quality_option_rect(&metrics, &metrics.rects(1.0), 1.0, 0);

        // Touch the state at a different scale, which is what a DPI change does.
        let _ = state.segment_at_scale(1.5);
        let after = snapclip_ui::panel::quality_option_rect(&metrics, &metrics.rects(1.5), 1.5, 0);

        assert_eq!(state.segment_scale, 1.5, "the new scale is remembered");
        assert_eq!(state.chosen_quality(), 2, "a DPI change must not change the choice");
        assert!(
            after.left > before.left,
            "and the rectangles really were rebuilt at the new scale"
        );
    }

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
