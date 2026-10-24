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
use super::{save_panel, ConflictPolicy, PanelValues, Remembered};
use snapclip_capture::save_format::SaveFormat;

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
    | windows_sys::Win32::UI::WindowsAndMessaging::WS_EX_TOPMOST
    // `IsDialogMessageW` only descends into the children of a window that says it is a container,
    // and descending is how `Tab` reaches the next field and how `Enter` and `Esc` become commands
    // at all (`Enter` is a save, `Esc` a cancel). Without the bit the panel is a window full of
    // loose controls and its keyboard is whatever each control happens to do with it.
    | windows_sys::Win32::UI::WindowsAndMessaging::WS_EX_CONTROLPARENT;

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
///
/// Only the top padding is left in the body: the mock's `12px` at the bottom is the footer's own
/// top padding, and the footer now sits below the body rather than being drawn by it.
pub const BODY_PAD_TOP_DIP: f32 = 4.0;

/// `.foot { padding: 12px 16px 12px }` (`save-panel-demo-v3.html:274`) around `--btn-h: 32px`
/// (`:50`), so the strip is `12 + 32 + 12 = 56` dip.
pub const FOOTER_HEIGHT_DIP: f32 = 56.0;

/// The shell's height before there is any content.
///
/// The mock sizes the panel by its content, so there is no `height` to copy. The default content
/// is the header, the body's top padding, the preview column and the footer, which is
/// `44 + 4 + 460 + 56 = 564` dip. Later steps replace this with a measurement of the real rows;
/// until then the window reserves exactly the height the mock's default content needs, so the
/// shell looks like the mock rather than like an arbitrary rectangle.
pub const PANEL_HEIGHT_DIP: f32 =
    HEADER_HEIGHT_DIP + BODY_PAD_TOP_DIP + PREVIEW_HEIGHT_DIP + FOOTER_HEIGHT_DIP;

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

/// The size limit's default and floor, in kilobytes (`:647`, `:236`).
///
/// The floor is the design's: `下限 10`. A limit below what a format's own header costs could never
/// be met, so a value under the floor is treated as the floor rather than as an impossible request.
pub const DEFAULT_LIMIT_KB: u32 = 500;
pub const MIN_LIMIT_KB: u32 = 10;

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
    /// What this shell remembers from one panel to the next: the name template and the next number.
    ///
    /// Both outlive a single panel on purpose — the spec keeps the template and the number across
    /// saves (`:201`), and a panel is opened per capture — so they cannot live inside one. The handle
    /// is shared rather than copied so the panel that writes a file is also the one that moves the
    /// counter, and no second copy can disagree.
    pub remembered: std::sync::Arc<std::sync::Mutex<Remembered>>,
    /// Where the path field opens: the user's desktop, from the shell rather than from the mockup.
    ///
    /// The mockup's `C:\Users\阿伟\Desktop` (`:544`) is one person's desktop, so it is not a
    /// default. Empty means the shell declined to say, and the panel then opens on the empty
    /// value the path validation exists to describe rather than on an invented directory.
    pub directory: String,
    /// Where the footer's copy button sends the picture. `None` means the real clipboard; a test
    /// hands in a recorder so it can see what was asked for without touching the user's clipboard.
    pub copy: Option<CopySink>,
}

impl Default for ShellOptions {
    fn default() -> Self {
        Self {
            width_dip: PANEL_WIDTH_DIP,
            height_dip: PANEL_HEIGHT_DIP,
            preview: None,
            capture_size: None,
            remembered: std::sync::Arc::new(std::sync::Mutex::new(Remembered::default())),
            directory: String::new(),
            copy: None,
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
        GetWindowRect, IsDialogMessageW, LoadCursorW, RegisterClassW, SetWindowLongPtrW,
        SetWindowPos, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, GWL_EXSTYLE,
        GWLP_USERDATA, GW_HWNDPREV, IDC_ARROW, MSG, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER,
        WNDCLASSW, WS_CAPTION, WS_EX_TOPMOST, WS_OVERLAPPED, WS_SYSMENU,
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
        // The footer buttons' widths, measured once, here. The paint and the click handling both
        // read these numbers, so a click can only land on the button whose rectangle is on screen
        // rather than on a second measurement that happens to agree. Measuring needs only the
        // DirectWrite factory, so it works before the first paint and a click that arrives early
        // still lands where the button will be.
        let footer_widths = measure_footer_widths(hwnd, renderer.as_ref());
        // The path row's button width, measured for the same reason: the input's rectangle gives up
        // that width, and the control and the painted button have to be told the same number.
        let path_change = measure_path_change(hwnd, renderer.as_ref());
        // The shortcut chips' widths, measured here for the same reason the buttons' are: the click
        // handler tests the chips the paint drew, and three words of different lengths make three
        // chips of different widths.
        let path_chips = measure_path_chips(hwnd, renderer.as_ref());
        // The size row's four shortcuts, measured here for the same reason: a click on `50%` has to
        // land on the chip the paint drew, and each chip is as wide as its own word.
        let size_presets = measure_size_presets(hwnd, renderer.as_ref());
    let size_mode_widths = measure_size_mode(hwnd, renderer.as_ref());
        // The size limit's four words, measured here for the same reason: the switch's right edge is
        // where the label line ends, so its click target is as wide as `限制体积`.
        let limit_words = measure_limit_words(hwnd, renderer.as_ref());
        // The conflict link's two words: the one drawn is the one clicked, so both are measured once
        // here rather than inside the paint.
        let conflict_widths = measure_conflict_link(hwnd, renderer.as_ref());
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
            fill: snapclip_ui::panel::FILL_WHITE,
            segment: snapclip_ui::panel::quality_segment_state(&metrics, &rects, scale, 0),
            segment_scale: scale,
            footer_widths,
            path_change,
            format: 0,
            format_open: false,
            // Handed over rather than created here: the template and the counter are shared by every
            // panel of this shell, and a panel that made its own would start from the default again.
            remembered: options.remembered.clone(),
            // Nothing has been attempted yet, so the footer's status line has nothing to say.
            status: None,
            // Both menus start closed: the panel opens on the form, not on a popup.
            var_open: false,
            path_chips,
            size_presets,
        size_mode: snapclip_ui::panel::SIZE_MODE_CHOSEN,
        size_mode_widths,
            // The limit starts switched off, with no measurement behind it: the switch is the only
            // thing that asks for one, and a number measured for a limit nobody asked about would be
            // work the panel does on every open for nothing.
            limit_on: false,
            conflict: ConflictPolicy::AutoRename,
            conflict_shown: false,
            conflict_widths,
            limit_words,
            estimate: None,
            over_limit: false,
            copy: options
                .copy
                .take()
                .map(|sink| sink.0)
                .unwrap_or_else(|| std::sync::Arc::new(SystemImageCopy)),
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
        // The template the last panel left behind, or the spec's default on the first open (`:68`).
        // Read once here rather than inside the loop: the field's first text is a copy, and the
        // remembered template is what the next panel will read.
        let template = options
            .remembered
            .lock()
            .map(|remembered| remembered.template.clone())
            .unwrap_or_default();
        let initial_text = |id| {
            initial_text(
                id,
                options.capture_size,
                &template,
                options.directory.as_str(),
            )
        };
        if let Some(s) = (state as *mut WindowState).as_mut() {
            for id in snapclip_ui::panel::FieldId::ALL {
                let control = create_field_edit(
                    hwnd,
                    id.rect(&metrics, &rects, scale, s.path_change),
                    &initial_text(id),
                    id.face(),
                );
                s.set_control(id, control);
            }
        }
        // The controls now hold the name and the directory, so the question can be asked: the panel
        // opens with the link already showing if the file it would name is there (:1149).
        refresh_conflict(hwnd);

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
            // `IsDialogMessage` is what makes this a form rather than a window full of loose
            // controls: `Tab` moves between the fields, and the keys a focused control does not want
            // arrive as `WM_COMMAND` (`Enter` as `IDOK`, `Esc` as `IDCANCEL`). It gives the focused
            // control first refusal, so a control that wants the key — a multi-line editor's newline,
            // for instance — still gets it. This is why the panel carries `WS_EX_CONTROLPARENT`.
            if IsDialogMessageW(hwnd, &message) != 0 {
                continue;
            }
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

/// What the panel needs from the clipboard in order to copy the picture it is showing.
///
/// One method with three numbers rather than the capture crate's port, because the port takes a PNG
/// path and the panel has pixels rather than a file: it copies before anything is written, which is
/// what the footer's copy button is for. The type exists so the window can be tested with a
/// clipboard that records instead of writing to the user's machine.
pub trait ImageCopy: Send + Sync + 'static {
    fn copy_image(&self, width: u32, height: u32, bgra: &[u8]) -> Result<(), String>;
}

/// A copy destination that can sit in [`ShellOptions`], which derives `Debug`.
///
/// The trait object has no `Debug` to print, and printing the sink's identity would say nothing a
/// test could use, so this wrapper prints its own name and inherits the clone.
#[derive(Clone)]
pub struct CopySink(pub std::sync::Arc<dyn ImageCopy>);

impl std::fmt::Debug for CopySink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CopySink(..)")
    }
}

/// The real clipboard, through the shell's existing writer.
pub struct SystemImageCopy;

impl ImageCopy for SystemImageCopy {
    fn copy_image(&self, width: u32, height: u32, bgra: &[u8]) -> Result<(), String> {
        crate::clipboard::SystemClipboard.copy_image(&crate::clipboard::ClipboardImage {
            width,
            height,
            pixels: bgra,
            channels: crate::clipboard::Channels::Bgra,
            png: None,
        })
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
    /// The colour a transparent pixel becomes when the format cannot carry alpha (`S.fill`, `:724`).
    ///
    /// Straight order in the buffer's channels, which is what `PanelValues::fill` and the fill row's
    /// swatches both read, so the colour the panel shows chosen and the colour the encoder
    /// composites are one value. Only JPEG spends it; PNG and this project's WebP keep their alpha.
    fill: [u8; 3],
    /// The quality control's state. Its geometry comes from the same producer the painter draws
    /// with, so a click lands on the option whose block the user can see, and its selection is owned
    /// by `segmented`'s state machine rather than by a second set of rules here. The chosen option is
    /// read from this, never kept beside it.
    segment: snapclip_ui::panel::SegmentState,
    /// The scale the segment geometry was built at. The rectangles are pixels, so a window that
    /// moves between monitors has to rebuild them.
    segment_scale: f32,
    /// The footer buttons' widths, in the order `FOOTER_BUTTONS` lists them.
    ///
    /// Measured once and read by both the paint and the click handling, so the rectangle a button is
    /// drawn in and the rectangle a click is tested against are one set of numbers. Measuring inside
    /// the paint would leave the hit test guessing, and guessing is how a click lands on a button
    /// that is not there.
    footer_widths: [i32; 3],
    /// The path row's `更改` button width, measured once beside the footer's.
    ///
    /// The input's rectangle gives up this width, so the control and the paint read the same number
    /// rather than each deciding how much room the buttons need.
    path_change: i32,
    /// Which format the format control shows, by index into FMT_OPTIONS. The control is drawn by
    /// the frame and answered here, so the number lives beside the other state the frame reads.
    format: usize,
    /// Whether the format menu is open. Opening draws the accent border, turns the chevron over and
    /// paints the menu; the flag lives here because only the window knows what the last click hit.
    format_open: bool,
    /// What the shell remembers from one panel to the next: the name template and the next number.
    /// The panel reads the number when it saves, moves it only after a save succeeds, and writes the
    /// template back as the name is edited.
    remembered: std::sync::Arc<std::sync::Mutex<Remembered>>,
    /// What the last save attempt has to say, and how loudly (`docs/41` A12).
    ///
    /// `None` is the panel that has not failed at anything: the status line's space is empty. A
    /// failure fills it, and the panel **stays open** so the sentence can be read and the value that
    /// caused it corrected — closing on failure, which is what it used to do, made every rejection
    /// invisible.
    status: Option<(snapclip_ui::copy::Level, String)>,
    /// Whether the token menu is open. Only one menu is open at a time, and opening this one closes
    /// the format's, because two popups over one form would be a puzzle rather than a menu.
    var_open: bool,
    /// The three shortcut chips' widths, in the order `PATH_CHIP_WORDS` lists them.
    ///
    /// Measured once beside the footer's, for the same reason: the click handler tests the chips
    /// that were drawn rather than a second measurement of the same three words.
    path_chips: [i32; 3],
    /// The four size shortcuts' widths, in the design's order (`:615`–`:618`), measured once beside
    /// the footer's for the same reason: the click handler tests the chips the paint drew.
    size_presets: [i32; 4],
    /// Which unit the two size fields speak: an index into `SIZE_MODE_WORDS` (`数值`, `百分比`,
    /// `:585`, `:586`).
    ///
    /// Only the meaning of the fields changes when this changes; the size itself does not, which is
    /// why switching rewrites the two texts from the size that is already there (`:1072`).
    size_mode: usize,
    /// The two mode buttons' widths, measured once beside the shortcuts' for the same reason: the
    /// click handler tests the rectangles the paint drew.
    size_mode_widths: [i32; 2],
    /// Whether the size limit is switched on (`:232`). While it is, the quality line shows the
    /// `不超过 [ 500 ] KB` group instead of the tier segment, and a save is required to fit — one
    /// flag, asked by the painter, the click handler and the save.
    limit_on: bool,
    /// Whether a taken name is reused (Overwrite) or numbered (AutoRename), which the footer's
    /// link flips (:989).
    conflict: ConflictPolicy,
    /// Whether the name the panel would write is already a file, which is what shows the link
    /// (:1149). Recomputed when the name, the directory or the format changes, not on every paint:
    /// the answer comes from the file system.
    conflict_shown: bool,
    /// Both of the link's words, measured once beside the footer's widths so the link that is drawn
    /// is the link that is clicked.
    conflict_widths: [i32; 2],
    /// The limit's four words, measured once beside the other words so the switch that is drawn is
    /// the switch that is clicked.
    limit_words: snapclip_ui::panel::LimitWidths,
    /// What this panel would write, as the encoder measured it (`:244`).
    ///
    /// `None` before the first measurement: an estimate that has not been made is not zero bytes,
    /// and a row that said `0.00 KB` would be inventing a number.
    estimate: Option<u64>,
    /// Whether the last measurement was over the limit, which decides the note's shape and whether
    /// the escape hatch is offered (`:234`).
    over_limit: bool,
    /// Where the footer's copy button sends the picture. Behind a trait so a test can see what was
    /// asked for without writing to the user's clipboard.
    copy: std::sync::Arc<dyn ImageCopy>,
}

/// The footer buttons' widths for this window, or zeros if Direct2D never started.
///
/// Zeros are not a plausible width, and they are not meant to be: without a renderer there is no
/// panel to paint and no button to click, and the caller logs that. Inventing a width would put a
/// click target where nothing is drawn.
fn measure_footer_widths(
    hwnd: windows_sys::Win32::Foundation::HWND,
    renderer: Option<&snapclip_ui::panel::PanelRenderer>,
) -> [i32; 3] {
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96) as f32;
    match renderer.map(|renderer| renderer.measure_footer_widths(dpi)) {
        Some(Ok(widths)) => widths,
        Some(Err(error)) => {
            eprintln!("[save-panel] could not measure the footer buttons: {error:?}");
            [0; 3]
        }
        None => [0; 3],
    }
}

/// The path row's `更改` button width, measured the same way and for the same reason: the input's
/// rectangle leaves room for this button, so both the control and the paint read this one number.

/// The conflict link's two words, measured once beside the footer's widths for the same reason: the
/// word drawn is the word clicked, and the word changes with the policy.
fn measure_conflict_link(
    hwnd: windows_sys::Win32::Foundation::HWND,
    renderer: Option<&snapclip_ui::panel::PanelRenderer>,
) -> [i32; 2] {
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96) as f32;
    match renderer.map(|renderer| renderer.measure_conflict_link(dpi)) {
        Some(Ok(widths)) => widths,
        Some(Err(error)) => {
            eprintln!("[save-panel] could not measure the conflict link: {error:?}");
            [0; 2]
        }
        None => [0; 2],
    }
}

fn measure_path_change(
    hwnd: windows_sys::Win32::Foundation::HWND,
    renderer: Option<&snapclip_ui::panel::PanelRenderer>,
) -> i32 {
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96) as f32;
    match renderer.map(|renderer| renderer.measure_path_change(dpi)) {
        Some(Ok(width)) => width,
        Some(Err(error)) => {
            eprintln!("[save-panel] could not measure the path row's button: {error:?}");
            0
        }
        None => 0,
    }
}

/// The three shortcut chips' widths, measured once like the buttons above.
fn measure_path_chips(
    hwnd: windows_sys::Win32::Foundation::HWND,
    renderer: Option<&snapclip_ui::panel::PanelRenderer>,
) -> [i32; 3] {
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96) as f32;
    match renderer.map(|renderer| renderer.measure_path_chips(dpi)) {
        Some(Ok(widths)) => widths,
        Some(Err(error)) => {
            eprintln!("[save-panel] could not measure the shortcut chips: {error:?}");
            [0; 3]
        }
        None => [0; 3],
    }
}

/// The four size shortcuts' widths, measured once like the buttons above.
fn measure_size_presets(
    hwnd: windows_sys::Win32::Foundation::HWND,
    renderer: Option<&snapclip_ui::panel::PanelRenderer>,
) -> [i32; 4] {
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96) as f32;
    match renderer.map(|renderer| renderer.measure_size_presets(dpi)) {
        Some(Ok(widths)) => widths,
        Some(Err(error)) => {
            eprintln!("[save-panel] could not measure the size shortcuts: {error:?}");
            [0; 4]
        }
        None => [0; 4],
    }
}

/// The two size-mode buttons' widths, measured once like the shortcuts above.
fn measure_size_mode(
    hwnd: windows_sys::Win32::Foundation::HWND,
    renderer: Option<&snapclip_ui::panel::PanelRenderer>,
) -> [i32; 2] {
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96) as f32;
    match renderer.map(|renderer| renderer.measure_size_mode(dpi)) {
        Some(Ok(widths)) => widths,
        Some(Err(error)) => {
            eprintln!("[save-panel] could not measure the size mode buttons: {error:?}");
            [0; 2]
        }
        None => [0; 2],
    }
}

/// The size limit's four words, measured once like the buttons above.
///
/// Its own function rather than a field of the state's constructor, because `WM_DPICHANGED` has to
/// re-measure it too: the widths are pixels, and a window that moves to another monitor would
/// otherwise test clicks against the old monitor's words. With no widths at all, the switch would be
/// as wide as its track and the label would be unreachable — so an unmeasurable run falls back to
/// zero rather than to a guess.
fn measure_limit_words(
    hwnd: windows_sys::Win32::Foundation::HWND,
    renderer: Option<&snapclip_ui::panel::PanelRenderer>,
) -> snapclip_ui::panel::LimitWidths {
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96) as f32;
    match renderer.map(|renderer| renderer.measure_limit_words(dpi)) {
        Some(Ok(widths)) => widths,
        Some(Err(error)) => {
            eprintln!("[save-panel] could not measure the size limit's words: {error:?}");
            snapclip_ui::panel::LimitWidths::default()
        }
        None => snapclip_ui::panel::LimitWidths::default(),
    }
}

/// Rewrite the two size fields in `index`'s unit, leaving the size itself alone (`:1072`, `:1201`).
///
/// The fields are read with the unit currently in force and written with the one being switched to.
/// That is the whole of the mockup's behaviour: the numbers are a way of writing the size down, not
/// the size, so switching units must not resize anything.
fn switch_size_mode(
    hwnd: windows_sys::Win32::Foundation::HWND,
    state: &mut WindowState,
    index: usize,
) {
    let (width, height) = match state.preview.as_ref() {
        Some((_, width, height)) => (*width, *height),
        None => return,
    };
    let source = super::scale::Size::new(width, height);
    let size = super::scale::output_size(
        super::scale::SizeMode::from_index(state.size_mode),
        &control_text(state.control(snapclip_ui::panel::FieldId::SizeWidth)),
        &control_text(state.control(snapclip_ui::panel::FieldId::SizeHeight)),
        source,
    );
    let text = super::scale::display_text(
        super::scale::SizeMode::from_index(index),
        size,
        source,
    );
    state.set_size_text(&text.0, &text.1);
    state.size_mode = index;
    unsafe {
        windows_sys::Win32::Graphics::Gdi::InvalidateRect(hwnd, std::ptr::null(), 0);
    }
}

/// Put the capture's own size back into the two fields, in the unit the mode is in (`:1213`).
///
/// The mode is deliberately not reset: the mockup resets the size and re-renders the digits in
/// whichever unit is showing, so resetting while in percentages lands on `100`, not on a pixel count.
fn reset_size(hwnd: windows_sys::Win32::Foundation::HWND, state: &mut WindowState) {
    let (width, height) = match state.preview.as_ref() {
        Some((_, width, height)) => (*width, *height),
        None => return,
    };
    let source = super::scale::Size::new(width, height);
    let text = super::scale::display_text(
        super::scale::SizeMode::from_index(state.size_mode),
        source,
        source,
    );
    state.set_size_text(&text.0, &text.1);
    unsafe {
        windows_sys::Win32::Graphics::Gdi::InvalidateRect(hwnd, std::ptr::null(), 0);
    }
}

/// The characters a control holds, as UTF-16, which is the unit the control itself counts in.
///
/// The insertion has to work in that unit rather than in a `String`'s bytes: a name with a Chinese
/// character has more bytes than positions, and converting between the two in the middle of an
/// insertion is how half a character ends up in a file name.
fn control_text_utf16(hwnd: windows_sys::Win32::Foundation::HWND) -> Vec<u16> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW};
    if hwnd.is_null() {
        return Vec::new();
    }
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return Vec::new();
    }
    let mut buffer = vec![0u16; len as usize + 1];
    let written = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    buffer.truncate(written.max(0) as usize);
    buffer
}

/// Replace a control's text and put the caret where the caller says.
fn set_control_text_and_caret(
    hwnd: windows_sys::Win32::Foundation::HWND,
    text: &[u16],
    caret: usize,
) {
    use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;
    const EM_SETSEL: u32 = 0x00B1;
    if hwnd.is_null() {
        return;
    }
    let mut wide = text.to_vec();
    while wide.last() == Some(&0) {
        wide.pop();
    }
    let text_ptr = wide.as_ptr();
    // `SetWindowTextW` takes a NUL-terminated string; the slice has no terminator, so it is added.
    let mut terminated = wide.clone();
    terminated.push(0);
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetWindowTextW(
            hwnd,
            terminated.as_ptr(),
        );
        let _ = text_ptr;
        SendMessageW(
            hwnd,
            EM_SETSEL,
            caret as usize,
            caret as isize,
        );
    }
}

/// Where a control's selection is, in the units it counts in.
fn control_selection(hwnd: windows_sys::Win32::Foundation::HWND) -> (usize, usize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;
    const EM_GETSEL: u32 = 0x00B0;
    if hwnd.is_null() {
        return (0, 0);
    }
    let mut start: u32 = 0;
    let mut end: u32 = 0;
    unsafe {
        SendMessageW(
            hwnd,
            EM_GETSEL,
            &mut start as *mut u32 as usize,
            &mut end as *mut u32 as isize,
        );
    }
    (start as usize, end as usize)
}

/// What the panel's name field holds, read back from the control that owns it.
///
/// The text lives in the child `EDIT`, not in Rust, so a save has to ask the control. Reading it
/// here rather than keeping a copy means there is no second place that could disagree with what
/// the user sees.
fn control_text(hwnd: windows_sys::Win32::Foundation::HWND) -> String {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW};
    if hwnd.is_null() {
        return String::new();
    }
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return String::new();
    }
    let mut buffer = vec![0u16; len as usize + 1];
    let written = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    String::from_utf16_lossy(&buffer[..written.max(0) as usize])
}

/// The local date and time in the forms the naming template asks for: `YYYY-MM-DD` and `HHmm`
/// (`docs/design/…规格.md` names both). Local rather than UTC, because a file named for yesterday
/// is a surprise; no separator in the time because a colon is not a legal file name character.
/// The application's own name, which [软件名] expands to (:198) and the token menu shows.
const APP_NAME: &str = "SnapClip";

fn local_date_and_time() -> (String, String) {
    let mut now: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut now) };
    (
        format!("{:04}-{:02}-{:02}", now.wYear, now.wMonth, now.wDay),
        format!("{:02}{:02}", now.wHour, now.wMinute),
    )
}

/// Which format a menu index stands for.
///
/// The menu's order is the design's (FMT_OPTIONS, :717): PNG, JPG, WebP. The mapping lives here
/// rather than in the message handler because a click is not needed to check it, and because the
/// index and the format are both small integers whose swap would save a file of the wrong kind.
fn format_of_index(index: usize) -> SaveFormat {
    match index {
        1 => SaveFormat::Jpeg,
        2 => SaveFormat::Webp,
        _ => SaveFormat::Png,
    }
}

/// The panel's answers as the save step wants them.
///
/// Separate from [`save_from_panel`] because the two fields it reads are both strings and of the
/// same kind: swapping the directory and the name would produce a save that looks entirely
/// reasonable and writes to the wrong place. A function can be asserted without a window.
fn panel_values<'a>(
    directory: &'a str,
    name: &'a str,
    date: &'a str,
    time: &'a str,
    sequence: u32,
    format: SaveFormat,
    quality: u8,
    fill: [u8; 3],
    conflict: ConflictPolicy,
) -> PanelValues<'a> {
    PanelValues {
        directory,
        name,
        // The format control's choice, mapped from its index by the one function that knows the
        // order of the menu.
        format,
        // The quality tier's number, mapped by the one function that knows `docs/42` D8.
        quality,
        // Whether a taken name is reused or numbered, which the footer's link can change.
        conflict,
        fill,
        date,
        time,
        // Supplied by the shell's shared counter rather than assumed here: the number a save uses
        // is the one the previous successful save left behind, and the panel has no copy of it.
        sequence,
        app: APP_NAME,
    }
}

/// What a save attempt has to say in the footer's status line, if anything.
///
/// One place decides this, and it is a pure function so it can be asserted without a window — the
/// same reason `panel_values` and `initial_text` are functions. Three rules are visible here:
///
/// - a **success** says nothing: the panel closes, so there is no line to read;
/// - a **cancel** says nothing: nothing went wrong, the user changed their mind (`SaveError`'s own
///   comment), and an error colour would be a lie about what happened;
/// - everything else says something, at the level its own rule gives it, so a warning that does not
///   block a save and an error that does cannot be painted the same red.
///
/// Answers a click on one of the fill row's three swatches (`:658`–`:660`), reporting whether the
/// colour changed.
///
/// The first two are the design's fixed colours and simply become the fill. The third is an
/// `<input type="color">` in the mock, so it opens the system picker instead: only a confirmed
/// choice changes the colour, which is the same rule the folder picker follows — a cancelled dialog
/// leaves the panel exactly as it was. The colour travels in the buffer's channel order, and
/// `COLORREF` is `0x00BBGGRR`, so the conversion happens here once rather than at each use.
fn choose_fill(hwnd: windows_sys::Win32::Foundation::HWND, index: usize) -> bool {
    let Some(s) = state_mut(hwnd) else {
        return false;
    };
    match index {
        0 | 1 => {
            let wanted = if index == 0 {
                snapclip_ui::panel::FILL_WHITE
            } else {
                snapclip_ui::panel::FILL_BLACK
            };
            let changed = s.fill != wanted;
            s.fill = wanted;
            changed
        }
        _ => {
            use windows_sys::Win32::UI::Controls::Dialogs::{CHOOSECOLORW, ChooseColorW};
            let current = s.fill;
            let start = u32::from(current[0])
                | u32::from(current[1]) << 8
                | u32::from(current[2]) << 16;
            let mut custom: [u32; 16] = [0; 16];
            let mut picker = CHOOSECOLORW {
                lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
                hwndOwner: hwnd,
                hInstance: std::ptr::null_mut(),
                rgbResult: start,
                lpCustColors: custom.as_mut_ptr(),
                // `CC_RGBINIT` starts on the colour the panel already has; `CC_FULLOPEN` opens the
                // custom half straight away, which is the only half a swatch that says "custom"
                // should lead to.
                Flags: 0x0000_0001 | 0x0000_0002,
                lCustData: 0,
                lpfnHook: None,
                lpTemplateName: std::ptr::null(),
            };
            let confirmed = unsafe { ChooseColorW(&mut picker) } != 0;
            if !confirmed {
                return false;
            }
            let picked = [
                (picker.rgbResult & 0xff) as u8,
                ((picker.rgbResult >> 8) & 0xff) as u8,
                ((picker.rgbResult >> 16) & 0xff) as u8,
            ];
            let changed = s.fill != picked;
            s.fill = picked;
            changed
        }
    }
}

/// The wording comes from the two places that own it: the validation table's `copy` (with its filled
/// `detail` when it has one), and `SaveError::message` for the rest.
fn status_of(
    result: &Result<service::SaveOutcome, service::SaveError>,
) -> Option<(snapclip_ui::copy::Level, String)> {
    match result {
        Ok(_) => None,
        Err(service::SaveError::Cancelled) => None,
        Err(service::SaveError::Validation { copy, detail }) => Some((
            copy.level(),
            detail.clone().unwrap_or_else(|| copy.message().to_owned()),
        )),
        // The rest have no copy-table key: a capability refusal, a format this build cannot encode,
        // a full disk, an I/O failure. Their `message` is developer-facing and ascii, which is
        // recorded in `docs/41` as a gap rather than dressed up as user-facing wording here.
        Err(error) => Some((snapclip_ui::copy::Level::Error, error.message())),
    }
}

/// What one of the dialog manager's commands means for this panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shortcut {
    /// `Enter`, which the mockup titles as the save button's key (`:686`).
    Save,
    /// `Esc`, which the mockup titles as cancel's (`:684`); cancelling closes the panel (`docs/42`
    /// D12).
    Cancel,
}

/// The dialog manager's two command ids, and what the panel does with them.
///
/// `IsDialogMessageW` gives the focused control first refusal on a key. `Enter` in a single-line
/// edit is not wanted by the control, so the manager sends `WM_COMMAND` with `IDOK` (1) to the
/// panel; `Esc` arrives the same way as `IDCANCEL` (2). The mapping is a function rather than two
/// `if`s inside the message handler, for the reason `status_of` and `note_edit` are functions: a
/// rule that only lives inside a handler is a rule no test can reach.
fn shortcut_command(command: usize) -> Option<Shortcut> {
    match command {
        1 => Some(Shortcut::Save),
        2 => Some(Shortcut::Cancel),
        _ => None,
    }
}

/// Save, and close the panel only if a file exists.
///
/// The footer's save button and the `Enter` key both land here, so the two cannot disagree about
/// what a save does: success closes the panel, and a rejection leaves it open with the reason in the
/// footer, which is what `docs/41` A12 is about.
fn attempt_save(hwnd: windows_sys::Win32::Foundation::HWND) {
    if save_from_panel(hwnd).is_some() {
        // The picture exists, so the panel has nothing left to ask.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                hwnd,
                windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                0,
                0,
            );
        }
    } else {
        // The panel stays open with the reason in its footer: closing here is what made every
        // rejected save invisible (`docs/41` A12).
        unsafe {
            windows_sys::Win32::Graphics::Gdi::InvalidateRect(hwnd, std::ptr::null(), 0);
        }
    }
}

/// Save what the panel is showing, through the one path that writes files.
///
/// Returns the outcome so the caller can decide whether to close: a panel that closed on failure
/// would take the reason with it.
fn save_from_panel(hwnd: windows_sys::Win32::Foundation::HWND) -> Option<service::SaveOutcome> {
    // Every attempt starts with a clean line: a sentence from the previous failure would otherwise
    // sit above a save that is about to succeed, and the user would read the wrong outcome.
    if let Some(state) = state_mut(hwnd) {
        state.status = None;
        // ...and the fields that were marked stop being marked, for the same reason: the answer is
        // about to be replaced by a newer one.
        state.fields.clear_messages();
    }
    // What this attempt would write, assembled by the same function the estimate row uses, so the
    // number on screen and the file that lands describe one request rather than two.
    let Some((attempt, name)) = with_panel_request(hwnd, |values, frame, name| {
        let attempt = save_under_limit(hwnd, values, frame);
        (attempt, name.to_owned())
    }) else {
        eprintln!("[save-panel] save with no capture: there is nothing to write");
        return None;
    };
    let Some(attempt) = attempt else {
        // The limit could not be met at any size the encoder accepts, so nothing was written: a file
        // that breaks the rule the user asked for is worse than no file, and the row already offers
        // the escape hatch that shrinks the picture. The line that would say so has no sentence in
        // the design yet, which is recorded in `docs/41`.
        eprintln!("[save-panel] no size fits the limit, so nothing was written");
        return None;
    };
    // One write of the counter, after the attempt: `after` is the rule, and it lives in one place so
    // the cancel and failure cases cannot drift from it. Only the counter is written back; the
    // template was written as it was typed and must not be overwritten with the one read above.
    if let Some(s) = state(hwnd) {
        let remembered = s
            .remembered
            .lock()
            .map(|remembered| remembered.clone())
            .unwrap_or_default();
        let moved = remembered.sequence.after(&attempt, &name);
        if let Ok(mut guard) = s.remembered.lock() {
            guard.sequence = moved;
        }
    }
    // What the line says comes from `status_of`; this only records it. The panel is told to repaint
    // by the caller, which is the only place that knows whether the window should stay open.
    let status = status_of(&attempt);
    if let Some(state) = state_mut(hwnd) {
        state.status = status.clone();
        // A validation key names the field it is about, and that field's border says so too: the
        // status line says what happened, the border says where. Other failures have no field.
        if let Err(service::SaveError::Validation { copy, .. }) = &attempt {
            state.fields.mark(snapclip_ui::panel::problem_of(*copy));
        }
    }
    match &attempt {
        Ok(outcome) => eprintln!(
            "[save-panel] saved {} bytes to {} as {}",
            outcome.bytes,
            outcome.path.display(),
            outcome.format.label()
        ),
        Err(error) => eprintln!(
            "[save-panel] save failed: {} ({})",
            status.as_ref().map(|(_, text)| text.as_str()).unwrap_or(""),
            error.message()
        ),
    }
    attempt.ok()
}

/// Build the panel's values and frame, hand them to `f`, and return its answer.
///
/// One place assembles what the panel would write. The estimate row and the save both come through
/// here, so the number on screen and the file that lands cannot describe different requests, and
/// editing one of them (a field, the mode, the tier) does not have to be remembered twice.
///
/// The name is handed to `f` because the save moves the counter according to whether the name asked
/// for one, and the weighing has no use for it.
fn with_panel_request<T>(
    hwnd: windows_sys::Win32::Foundation::HWND,
    f: impl FnOnce(
        &super::assemble::PanelValues<'_>,
        &super::scale::Frame<'_>,
        &str,
    ) -> T,
) -> Option<T> {
    let s = state(hwnd)?;
    let (bgra, width, height) = s.preview.as_ref()?;
    let name = control_text(s.control(snapclip_ui::panel::FieldId::Name));
    let directory = control_text(s.control(snapclip_ui::panel::FieldId::Path));
    // What the two size fields say, read at save time like the name is: the controls are what the
    // user sees, and a half-edited pair falls back to the selection rather than to a guess.
    let source = super::scale::Size::new(*width, *height);
    let output = super::scale::output_size(
        super::scale::SizeMode::from_index(s.size_mode),
        &control_text(s.control(snapclip_ui::panel::FieldId::SizeWidth)),
        &control_text(s.control(snapclip_ui::panel::FieldId::SizeHeight)),
        source,
    );
    let (date, time) = local_date_and_time();
    // The number this attempt would use. Read here because the name is expanded with it; it is moved
    // afterwards only if the save succeeded and the name asked for one.
    let sequence = s
        .remembered
        .lock()
        .map(|remembered| remembered.sequence.next())
        .unwrap_or(1);
    let values = panel_values(
        &directory,
        &name,
        &date,
        &time,
        sequence,
        format_of_index(s.format),
        super::assemble::quality_of_tier(s.chosen_quality()),
        s.fill,
        s.conflict,
    );
    let frame = super::scale::Frame {
        bgra,
        source,
        output,
    };
    if output != source {
        eprintln!(
            "[save-panel] resampling {}x{} to {}x{}",
            source.width, source.height, output.width, output.height
        );
    }
    Some(f(&values, &frame, &name))
}

/// The limit as a number of kilobytes, from what the field says.
///
/// Empty, unparseable and below the design's floor all resolve to the floor (`:236`): a limit that
/// nothing can meet would refuse every save, and refusing everything is not what an unreadable
/// number should mean.
fn limit_kilobytes(text: &str) -> u32 {
    text.trim()
        .parse::<u32>()
        .map(|kilobytes| kilobytes.max(MIN_LIMIT_KB))
        .unwrap_or(MIN_LIMIT_KB)
}

/// The limit in bytes, or `None` when the switch is off.
///
/// The switch, not the field, decides whether a limit applies: the field keeps its number while the
/// limit is off, which is what makes turning the switch back on restore the value the user chose.
fn limit_bytes(hwnd: windows_sys::Win32::Foundation::HWND) -> Option<u64> {
    let s = state(hwnd)?;
    if !s.limit_on {
        return None;
    }
    let text = control_text(s.control(snapclip_ui::panel::FieldId::Limit));
    Some(u64::from(limit_kilobytes(&text)) * 1024)
}

/// The sentence under the quality line (`:234`), built from the last measurement.
///
/// A pure function so the two shapes and the digits between them can be asserted without a window.
/// `None` when nothing has been measured: an empty note reserves the line's space and keeps the rows
/// below it from jumping when a number arrives (the space is reserved in the layout).
fn limit_note(estimate: Option<u64>, over: bool, png: bool) -> Option<String> {
    use crate::save::estimate::format_bytes;
    use snapclip_ui::panel::{
        LIMIT_NOTE_OK_AFTER, LIMIT_NOTE_OK_BEFORE, LIMIT_NOTE_OVER_AFTER, LIMIT_NOTE_OVER_BEFORE,
        LIMIT_NOTE_OVER_PNG_BEFORE,
    };
    let written = format_bytes(estimate?);
    Some(if over {
        let before = if png {
            LIMIT_NOTE_OVER_PNG_BEFORE
        } else {
            LIMIT_NOTE_OVER_BEFORE
        };
        format!("{before}{written}{LIMIT_NOTE_OVER_AFTER}")
    } else {
        format!("{LIMIT_NOTE_OK_BEFORE}{written}{LIMIT_NOTE_OK_AFTER}")
    })
}

/// Save, shrinking first when a limit is given and the panel's own size would be over it.
///
/// `None` means the limit could not be met at any size the encoder accepts, and therefore that
/// nothing was written.
fn save_under_limit(
    hwnd: windows_sys::Win32::Foundation::HWND,
    values: &super::assemble::PanelValues<'_>,
    frame: &super::scale::Frame<'_>,
) -> Option<Result<service::SaveOutcome, service::SaveError>> {
    save_with_limit(limit_bytes(hwnd), values, frame)
}

/// The rule behind [`save_under_limit`], with the limit passed in.
///
/// A free function so the promise can be asserted without a window: what a limit means is a
/// property of the file, and a test that went through the window would be asserting the window.
fn save_with_limit(
    limit: Option<u64>,
    values: &super::assemble::PanelValues<'_>,
    frame: &super::scale::Frame<'_>,
) -> Option<Result<service::SaveOutcome, service::SaveError>> {
    let Some(limit) = limit else {
        return Some(save_panel(values, frame));
    };
    let fitted = super::estimate::fit_under(frame.output, limit, |size| {
        super::assemble::weigh(values, &frame.at(size)).ok()
    });
    let Some((size, _bytes)) = fitted else {
        return None;
    };
    Some(save_panel(values, &frame.at(size)))
}

/// Toggle the switch, show or hide its input, and take the measurement the row needs.
fn toggle_limit(hwnd: windows_sys::Win32::Foundation::HWND) {
    {
        let Some(s) = state_mut(hwnd) else {
            return;
        };
        s.limit_on = !s.limit_on;
        // A measurement from the previous state describes a different request, and a stale number
        // beside a live switch is worse than none.
        s.estimate = None;
        s.over_limit = false;
    }
    refresh_estimate(hwnd);
    // The limit's input is a native child, so it appears and disappears with the switch rather than
    // with the painter; `apply_menu_occlusion` is the one place that decides which controls exist on
    // screen, and it reads `limit_on` for this one.
    apply_menu_occlusion(hwnd);
    unsafe { windows_sys::Win32::Graphics::Gdi::InvalidateRect(hwnd, std::ptr::null(), 0) };
}

/// Measure what this panel would write and update the note and the estimate.
///
/// Called at the moments the panel changes what the file would be, and **never from the paint**: a
/// measurement is a real encode, so a repaint that measured would turn a drag into a slideshow.
fn refresh_estimate(hwnd: windows_sys::Win32::Foundation::HWND) {
    let on = state(hwnd).map(|s| s.limit_on).unwrap_or(false);
    if !on {
        if let Some(s) = state_mut(hwnd) {
            s.estimate = None;
            s.over_limit = false;
        }
        return;
    }
    let measured =
        with_panel_request(hwnd, |values, frame, _| super::assemble::weigh(values, frame));
    let limit = limit_bytes(hwnd);
    if let Some(s) = state_mut(hwnd) {
        s.estimate = measured.and_then(|outcome| outcome.ok());
        s.over_limit =
            matches!((s.estimate, limit), (Some(bytes), Some(limit)) if bytes > limit);
    }
    unsafe { windows_sys::Win32::Graphics::Gdi::InvalidateRect(hwnd, std::ptr::null(), 0) };
}

/// Shrink the two size fields until the file fits under the limit (`:235`).
///
/// The rule is the encoder's length, not a fraction of an estimate: `fit_under` weighs each
/// candidate with a real encode. The size that fits is written back into the two cells, so what the
/// panel shows is what the file is, and the next save starts from it rather than shrinking again.
fn shrink_to_fit(hwnd: windows_sys::Win32::Foundation::HWND) {
    let Some(limit) = limit_bytes(hwnd) else {
        return;
    };
    let fitted = with_panel_request(hwnd, |values, frame, _| {
        super::estimate::fit_under(frame.output, limit, |size| {
            super::assemble::weigh(values, &frame.at(size)).ok()
        })
    })
    .flatten();
    let Some((size, bytes)) = fitted else {
        eprintln!("[save-panel] even the smallest size is over the limit");
        return;
    };
    if let Some(s) = state_mut(hwnd) {
        let Some((_, width, height)) = s.preview.as_ref() else {
            return;
        };
        let source = super::scale::Size::new(*width, *height);
        let text = super::scale::display_text(
            super::scale::SizeMode::from_index(s.size_mode),
            size,
            source,
        );
        s.set_size_text(&text.0, &text.1);
        s.estimate = Some(bytes);
        s.over_limit = bytes > limit;
    }
    unsafe { windows_sys::Win32::Graphics::Gdi::InvalidateRect(hwnd, std::ptr::null(), 0) };
}

/// Put a directory into the path field.
///
/// The field is the only place the shell reads the directory from, so a shortcut chip changes the
/// field rather than a second copy of the path. That also means the user sees what the chip did,
/// and can edit the result before saving.
fn set_directory(hwnd: windows_sys::Win32::Foundation::HWND, directory: &str) {
    let Some(state) = state(hwnd) else {
        return;
    };
    let control = state.control(snapclip_ui::panel::FieldId::Path);
    if control.is_null() {
        return;
    }
    let text: Vec<u16> = directory.encode_utf16().collect();
    let caret = text.len();
    set_control_text_and_caret(control, &text, caret);
    unsafe { windows_sys::Win32::Graphics::Gdi::InvalidateRect(hwnd, std::ptr::null(), 0) };
}

/// The word the conflict link shows, which is the action a press would take (:1150).
///
/// Renaming is the default, so the link offers overwriting, and only offers renaming once the policy
/// has been flipped. A label that named the current policy instead would read as a statement of fact
/// and be unclickable in the mind of anyone who reads it.
fn conflict_link(policy: ConflictPolicy) -> snapclip_ui::copy::ConflictLink {
    match policy {
        ConflictPolicy::AutoRename => snapclip_ui::copy::ConflictLink::Overwrite,
        ConflictPolicy::Overwrite => snapclip_ui::copy::ConflictLink::Rename,
    }
}

/// Ask the file system whether the name this panel would write is already taken (:1149).
///
/// The answer decides whether the link is shown, and a name that is free resets the policy to
/// numbering (:1138), because overwriting is only ever offered against a file that exists. The
/// question is asked when the name, the directory or the format changes rather than on every paint,
/// since the answer comes from the file system and a repaint is not evidence that it changed.
fn refresh_conflict(hwnd: windows_sys::Win32::Foundation::HWND) {
    let taken = with_panel_request(hwnd, |values, frame, _| {
        super::assemble::request_for(
            values,
            frame.output.width as u64,
            frame.output.height as u64,
        )
        .ok()
        .and_then(|request| super::service::plain_target(&request).ok())
        .map(|path| path.exists())
        .unwrap_or(false)
    })
    .unwrap_or(false);
    let mut changed = false;
    if let Some(s) = state_mut(hwnd) {
        changed = s.conflict_shown != taken;
        s.conflict_shown = taken;
        if !taken && s.conflict != ConflictPolicy::AutoRename {
            s.conflict = ConflictPolicy::AutoRename;
            changed = true;
        }
    }
    if changed {
        unsafe { windows_sys::Win32::Graphics::Gdi::InvalidateRect(hwnd, std::ptr::null(), 0) };
    }
}

/// Ask the shell for a folder and put the answer in the path field.
///
/// `SHBrowseForFolderW` rather than `IFileOpenDialog`, because the app takes Windows through
/// `windows-sys` and the COM dialog would mean carrying interface definitions for one call. It is
/// the older dialog; it is also the one that needs no apartment, no `CoInitialize` and no new
/// dependency. Cancelling changes nothing, which is the distinction that makes this safe to click.
/// The folder the browse dialog opens at: the path field's text, when it names something.
///
/// The dialog takes a NUL-terminated UTF-16 string through its lParam, and the buffer has to
/// outlive the call, which is why this hands back the buffer rather than a pointer. An empty field
/// asks for nothing and leaves the dialog at the shell's own default, rather than at a path that is
/// not a path.
fn browse_initial_path(current: &str) -> Option<Vec<u16>> {
    let trimmed = current.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut buffer: Vec<u16> = trimmed.encode_utf16().collect();
    buffer.push(0);
    Some(buffer)
}

/// Tells the folder dialog where to start, once it exists (BFFM_INITIALIZED).
///
/// A callback rather than a field, because BROWSEINFOW has no "start here" member: the starting
/// folder arrives as a message after the dialog is built, carrying the string the caller put in
/// lParam.
unsafe extern "system" fn browse_initial(
    dialog: windows_sys::Win32::Foundation::HWND,
    message: u32,
    _lparam: windows_sys::Win32::Foundation::LPARAM,
    data: windows_sys::Win32::Foundation::LPARAM,
) -> i32 {
    use windows_sys::Win32::UI::Shell::{BFFM_INITIALIZED, BFFM_SETSELECTIONW};
    if message == BFFM_INITIALIZED as u32 && data != 0 {
        // A string pointer the caller owns crosses the boundary here; it lives in choose_folder's
        // frame for the whole call, which outlives the dialog.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW(
                dialog,
                BFFM_SETSELECTIONW,
                1,
                data,
            );
        }
    }
    0
}

fn choose_folder(hwnd: windows_sys::Win32::Foundation::HWND) {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{
        BROWSEINFOW, SHBrowseForFolderW, SHGetPathFromIDListW,
    };
    let title: Vec<u16> = "选择保存位置\0".encode_utf16().collect();
    // :1133 the dialog opens where the field already points, so correcting a folder that is nearly
    // right does not mean walking back from the shell's default every time.
    let current = state(hwnd)
        .map(|s| control_text(s.control(snapclip_ui::panel::FieldId::Path)))
        .unwrap_or_default();
    let initial = browse_initial_path(&current);
    let mut display = [0u16; 260];
    let mut info = BROWSEINFOW {
        hwndOwner: hwnd,
        pidlRoot: std::ptr::null_mut(),
        pszDisplayName: display.as_mut_ptr(),
        lpszTitle: title.as_ptr(),
        // `BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE`: folders only, and a dialog the user can
        // resize and navigate — the default is a tree that starts somewhere unhelpful.
        ulFlags: 0x0001 | 0x0040,
        lpfn: Some(browse_initial),
        lParam: initial
            .as_ref()
            .map(|path| path.as_ptr() as isize)
            .unwrap_or(0),
        iImage: 0,
    };
    let pidl = unsafe { SHBrowseForFolderW(&mut info) };
    if pidl.is_null() {
        // Cancelled. Nothing is written, because "no answer" is not an answer.
        return;
    }
    let mut path = [0u16; 260];
    let ok = unsafe { SHGetPathFromIDListW(pidl, path.as_mut_ptr()) } != 0;
    unsafe { CoTaskMemFree(pidl as *const core::ffi::c_void) };
    if !ok {
        eprintln!("[save-panel] the chosen folder has no file system path");
        return;
    }
    let len = path.iter().position(|unit| *unit == 0).unwrap_or(path.len());
    let chosen = String::from_utf16_lossy(&path[..len]);
    set_directory(hwnd, &chosen);
}

/// Open the folder the path field names, in the shell's own browser.
///
/// The path is the user's text, so it can be anything: a path that does not exist is a logged
/// no-op rather than a dialog, because the field's own validation already explains bad paths and a
/// second voice saying the same thing is noise.
fn open_directory(path: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    if path.trim().is_empty() {
        return;
    }
    let probe = std::path::Path::new(path);
    if !probe.is_dir() {
        eprintln!("[save-panel] cannot open {}: not a directory", probe.display());
        return;
    }
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let target: Vec<u16> = format!("{}\0", probe.display()).encode_utf16().collect();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // `ShellExecuteW` returns a value greater than 32 on success; the cast keeps the comparison in
    // one type, which is what the API's own documentation asks for.
    if result as isize <= 32 {
        eprintln!("[save-panel] could not open {}: {}", probe.display(), result as isize);
    }
}

/// Insert a token into the name field where the caret is.
///
/// The rule that decides the new text and the caret is `insert_variable`, a free function, so it can
/// be checked without a control; this only decides which field receives it, and keeps the focus
/// where it was so the next token goes on the same line as the last.
fn insert_token(hwnd: windows_sys::Win32::Foundation::HWND, token: &str) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    let Some(state) = state(hwnd) else {
        return;
    };
    let control = state.control(snapclip_ui::panel::FieldId::Name);
    if control.is_null() {
        return;
    }
    let text = control_text_utf16(control);
    let (start, end) = control_selection(control);
    let (text, caret) = snapclip_ui::panel::insert_variable(&text, start, end, token);
    set_control_text_and_caret(control, &text, caret);
    unsafe { SetFocus(control) };
    unsafe { windows_sys::Win32::Graphics::Gdi::InvalidateRect(hwnd, std::ptr::null(), 0) };
}

/// Copy the picture the panel is showing, at the size its two cells say.
///
/// The same size rule the save uses (`scale::output_size` and `scale::scaled`), so what lands on the
/// clipboard is what would have been written: a copy button that disagreed with the save button
/// about the size would be worse than no copy button.
fn copy_picture(
    hwnd: windows_sys::Win32::Foundation::HWND,
    state: &WindowState,
) -> Option<Result<(), String>> {
    let (bgra, width, height) = state.preview.as_ref()?;
    let source = super::scale::Size::new(*width, *height);
    let output = super::scale::output_size(
        super::scale::SizeMode::from_index(state.size_mode),
        &control_text(state.control(snapclip_ui::panel::FieldId::SizeWidth)),
        &control_text(state.control(snapclip_ui::panel::FieldId::SizeHeight)),
        source,
    );
    let frame = super::scale::Frame {
        bgra,
        source,
        output,
    };
    let pixels = super::scale::scaled(&frame);
    let _ = hwnd;
    Some(state.copy.copy_image(output.width, output.height, &pixels))
}

/// Which folder a shortcut chip stands for, by index into `PATH_CHIP_WORDS` (`:739`–`:741`).
///
/// The shell's own folders, in the mockup's order: desktop, pictures, downloads. A folder the
/// system declines to name answers `None`, and the chip then does nothing rather than writing an
/// invented path into the field the user is about to save into.
fn shortcut_directory(index: usize) -> Option<String> {
    let directory = match index {
        0 => super::dirs::desktop(),
        1 => super::dirs::pictures(),
        2 => super::dirs::downloads(),
        _ => return None,
    };
    if directory.is_empty() {
        eprintln!("[save-panel] the system did not name shortcut {index}");
        return None;
    }
    Some(directory)
}

/// Hide the native text boxes a menu would be drawn underneath.
///
/// Windows paints a window's children after the window itself, so a menu the frame draws cannot
/// cover a text box — the box wins the overlap. While a menu is open the boxes it covers are
/// hidden and the rest are left alone, so the fields not under the menu keep their caret and their
/// focus ring. When no menu is open every box is shown again.
fn apply_menu_occlusion(hwnd: windows_sys::Win32::Foundation::HWND) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE, SW_SHOWNA};
    let Some(state) = state(hwnd) else {
        return;
    };
    let metrics = snapclip_ui::panel::DEMO;
    let scale = dpi_scale(hwnd);
    let rects = metrics.rects(scale);
    let window_height = (metrics.panel_height() as f32 * scale).round() as i32;
    let menu = if state.var_open {
        Some(snapclip_ui::panel::var_menu_rect(&metrics, &rects, scale))
    } else if state.format_open {
        Some(snapclip_ui::panel::fmt_menu_rect(
            &metrics,
            &rects,
            scale,
            window_height,
        ))
    } else {
        None
    };
    let fields = snapclip_ui::panel::FieldId::ALL;
    let boxes: Vec<snapclip_model::geometry::Rect> = fields
        .iter()
        .map(|id| id.rect(&metrics, &rects, scale, state.path_change))
        .collect();
    // The one place that decides which boxes a menu covers: `fields_under` is a free function so
    // the rule can be asserted without a window, and so two handlers cannot answer it differently.
    let hidden = menu
        .map(|menu| snapclip_ui::panel::fields_under(menu, &boxes))
        .unwrap_or_default();
    for (index, id) in fields.iter().enumerate() {
        let control = state.control(*id);
        if control.is_null() {
            continue;
        }
        let show = !hidden.contains(&index)
            // The limit's input exists only while the switch is on: the quality line shows the
            // `不超过 [ 500 ] KB` group then and the tier segment otherwise, so a control the frame
            // does not draw must not be visible — or focusable — either.
            && (*id != snapclip_ui::panel::FieldId::Limit || state.limit_on);
        unsafe {
            let _ = ShowWindow(control, if show { SW_SHOWNA } else { SW_HIDE });
        }
    }
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

    /// Put a size into the two cells' controls, which is what a shortcut, a mode switch and the
    /// reset all do.
    ///
    /// The controls are where the size lives — the save reads their text — so these write the text
    /// rather than a second pair of numbers that the paint would then have to agree with. One writer,
    /// and it is the one the user can also type into. The words are the caller's, because what the
    /// fields hold depends on the mode: pixels in one, percentages of the capture in the other
    /// (`scale::display_text` decides, `:1072`).
    fn set_size_text(&self, width: &str, height: &str) {
        use windows_sys::Win32::UI::WindowsAndMessaging::SetWindowTextW;
        for (id, value) in [
            (snapclip_ui::panel::FieldId::SizeWidth, width),
            (snapclip_ui::panel::FieldId::SizeHeight, height),
        ] {
            let text: Vec<u16> = value.encode_utf16().chain([0]).collect();
            unsafe {
                SetWindowTextW(self.control(id), text.as_ptr());
            }
        }
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
/// Apply one `WM_COMMAND` notification to the shell's memory, reporting whether it changed.
///
/// The handler's part is three lines and this function's part is the rule, kept apart for the same
/// reason `status_of` and `initial_text` are free functions: a decision that lives only inside a
/// message handler is a decision no test can reach. What is asserted here is which notifications
/// count — a change in the name box writes the template, and everything else (focus in either
/// direction, a change in any other field) leaves it alone, because a path being edited is not a
/// name being edited.
fn note_edit(
    remembered: &mut Remembered,
    id: snapclip_ui::panel::FieldId,
    is_change: bool,
    edited: &str,
) -> bool {
    if !is_change || id != snapclip_ui::panel::FieldId::Name {
        return false;
    }
    remembered.remember_template(edited)
}

/// What each field holds when the panel opens.
///
/// A free function rather than a closure so that the one thing that can go wrong here can be
/// asserted without a window: the width and the height are two numbers of the same type, and
/// handing the width to the height cell would look entirely reasonable on screen. The path row
/// starts empty because the panel's own default is a question for the next stage, and the name
/// row starts with **the remembered template** — the spec's `SnapClip_[日期]` on a first run
/// (`:68`), and whatever the last panel was left with afterwards (`:201`).
/// A free function rather than a closure so that the one thing that can go wrong here can be
/// asserted without a window: the width and the height are two numbers of the same type, and
/// handing the width to the height cell would look entirely reasonable on screen. The path row
/// starts at **the directory the shell named** — the mockup's `C:\Users\阿伟\Desktop` (`:544`) is
/// one person's desktop, so the real one is asked for rather than copied — and the name row starts
/// with **the remembered template**: the spec's `SnapClip_[日期]` on a first run (`:68`), and
/// whatever the last panel was left with afterwards (`:201`).
fn initial_text(
    id: snapclip_ui::panel::FieldId,
    capture_size: Option<(u32, u32)>,
    template: &str,
    directory: &str,
) -> String {
    use snapclip_ui::panel::FieldId;
    match (id, capture_size) {
        (FieldId::Path, _) => directory.to_string(),
        (FieldId::Name, _) => template.to_string(),
        (FieldId::SizeWidth, Some((width, _))) => width.to_string(),
        (FieldId::SizeHeight, Some((_, height))) => height.to_string(),
        // The limit's default is the mockup's own `500` (`:647`). It is a default rather than a
        // measurement, which is why it survives the switch being off: the control exists either way
        // and only its visibility and the sentence beside it depend on the switch.
        (FieldId::Limit, _) => DEFAULT_LIMIT_KB.to_string(),
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
                    // The estimate row's two strings live here rather than in the state: they are
                    // formatted from the measurement and read by one paint, and a stored copy would
                    // be a second place that has to be kept in step with the number it came from.
                    let limit_note = limit_note(
                        s.estimate,
                        s.over_limit,
                        format_of_index(s.format) == SaveFormat::Png,
                    );
                    let estimate_text = s.estimate.map(crate::save::estimate::format_bytes);
                    // The over-resolution hint and the token menu's values, read here for the same
                    // reason the save reads them there: the width and height the sentence warns
                    // about are the size fields' own answer, and a menu promising a date the file
                    // name would not get is worse than no menu.
                    let (date_text, time_text) = local_date_and_time();
                    let source_size = s
                        .preview
                        .as_ref()
                        .map(|(_, width, height)| super::scale::Size::new(*width, *height))
                        .unwrap_or(super::scale::Size::new(0, 0));
                    let output_size = super::scale::output_size(
                        super::scale::SizeMode::from_index(s.size_mode),
                        &control_text(s.control(snapclip_ui::panel::FieldId::SizeWidth)),
                        &control_text(s.control(snapclip_ui::panel::FieldId::SizeHeight)),
                        source_size,
                    );
                    let warn_hint = snapclip_ui::panel::warn_hint_shown(
                        (output_size.width, output_size.height),
                        (source_size.width, source_size.height),
                    );
                    let sequence_text = s
                        .remembered
                        .lock()
                        .map(|remembered| remembered.sequence.next())
                        .unwrap_or(1)
                        .to_string();
                    let resolution_text =
                        format!("{}x{}", output_size.width, output_size.height);
                    // The surface fill is skipped when DWM is drawing the material, so a backdrop
                    // keeps showing through while the frame's own lines still land on top of it.
                    painted = renderer
                        .paint_with_preview(
                            hdc as *mut _,
                            area,
                            &snapclip_ui::panel::PaintInput {
                                dpi,
                                fill_surface: !s.backdrop,
                                fields: s.fields,
                                preview: s
                                    .preview
                                    .as_ref()
                                    .map(|(bgra, w, h)| (bgra.as_slice(), *w, *h)),
                                quality: s.chosen_quality(),
                                format: s.format,
                                format_open: s.format_open,
                                fill: s.fill,
                                footer_widths: Some(s.footer_widths),
                                path_change: s.path_change,
                                conflict: s.conflict_shown.then(|| conflict_link(s.conflict)),
                                conflict_widths: Some(s.conflict_widths),
                                var_open: s.var_open,
                                path_chips: Some(s.path_chips),
                                size_presets: Some(s.size_presets),
                                size_mode: s.size_mode,
                                size_mode_widths: Some(s.size_mode_widths),
                                warn_hint,
                                variable_values: Some([
                                    date_text.as_str(),
                                    time_text.as_str(),
                                    sequence_text.as_str(),
                                    resolution_text.as_str(),
                                    APP_NAME,
                                ]),
                                // The switch is always drawn — turning it on is the whole point of
                                // it — while the note and the escape hatch exist only when a
                                // measurement has something to say (`:234`, `:235`).
                                limit: Some(snapclip_ui::panel::LimitView {
                                    on: s.limit_on,
                                    switch_word_px: s.limit_words.switch_word_px,
                                    note: limit_note.as_deref(),
                                    note_warn: s.over_limit,
                                    auto_shrink_px: if s.limit_on && s.over_limit {
                                        s.limit_words.auto_shrink_px
                                    } else {
                                        0
                                    },
                                    estimate: estimate_text.as_deref(),
                                    // A synchronous encode cannot show the state while it runs, so
                                    // the finished word is the only one this panel can honestly
                                    // draw; `docs/41` records why `编码中` would be theatre here.
                                    encode_state: if s.estimate.is_some() {
                                        Some(snapclip_ui::panel::ENCODE_STATE_DONE)
                                    } else {
                                        None
                                    },
                                    encode_done: s.estimate.is_some(),
                                    encode_state_px: s.limit_words.measured_px,
                                }),
                                status: s
                                    .status
                                    .as_ref()
                                    .map(|(level, text)| (*level, text.as_str())),
                            },
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
                // The conflict link (:989): a press flips the policy for the next save. Its word is
                // the action, so the rectangle to test is the one for the word being drawn.
                let link = state(hwnd).and_then(|s| {
                    s.conflict_shown.then(|| {
                        let word = conflict_link(s.conflict);
                        (s.conflict_widths[word.index()], s.footer_widths)
                    })
                });
                if let Some((width, widths)) = link {
                    let metrics = snapclip_ui::panel::DEMO;
                    let scale = dpi_scale(hwnd);
                    if snapclip_ui::panel::conflict_link_contains(
                        &metrics,
                        scale,
                        widths,
                        width,
                        (point.x, point.y),
                    ) {
                        if let Some(s) = state_mut(hwnd) {
                            s.conflict = match s.conflict {
                                ConflictPolicy::AutoRename => ConflictPolicy::Overwrite,
                                ConflictPolicy::Overwrite => ConflictPolicy::AutoRename,
                            };
                        }
                        windows_sys::Win32::Graphics::Gdi::InvalidateRect(
                            hwnd,
                            std::ptr::null(),
                            0,
                        );
                        return 0;
                    }
                }
            }
            // The token menu is the topmost thing the frame draws, so it is answered before
            // anything it covers: a click on a token must insert that token rather than fall
            // through to a row underneath, and a click anywhere else first closes the menu.
            if event == snapclip_ui::panel::PointerEvent::Down {
                let metrics = snapclip_ui::panel::DEMO;
                let scale = dpi_scale(hwnd);
                let rects = metrics.rects(scale);
                let point = (point.x, point.y);
                let open = state(hwnd).map(|s| s.var_open).unwrap_or(false);
                if open {
                    if let Some(index) = snapclip_ui::panel::var_item_at(&metrics, &rects, scale, point)
                    {
                        let token = snapclip_ui::panel::VARIABLE_TOKENS[index];
                        if let Some(s) = state_mut(hwnd) {
                            s.var_open = false;
                        }
                        insert_token(hwnd, token);
                        apply_menu_occlusion(hwnd);
                        return 0;
                    }
                    let on_wand =
                        snapclip_ui::panel::name_wand_contains(&metrics, &rects, scale, point);
                    let inside = snapclip_ui::panel::var_menu_contains(&metrics, &rects, scale, point)
                        || on_wand;
                    if inside {
                        // The button again closes the menu, which is what a button that opened it
                        // should do; its padding is swallowed so a near miss does not dismiss it.
                        if on_wand {
                            if let Some(s) = state_mut(hwnd) {
                                s.var_open = false;
                            }
                            apply_menu_occlusion(hwnd);
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                        }
                        return 0;
                    }
                    if let Some(s) = state_mut(hwnd) {
                        s.var_open = false;
                    }
                    apply_menu_occlusion(hwnd);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                    return 0;
                }
                // The variable button itself, which is inside the name field and therefore above
                // the field's text: only the strip the control gave up can be clicked, and that is
                // the strip this tests.
                if snapclip_ui::panel::name_wand_contains(&metrics, &rects, scale, point) {
                    // One menu at a time: opening this one closes the format's, so a click cannot
                    // leave two popups over the same form.
                    if let Some(s) = state_mut(hwnd) {
                        s.var_open = true;
                        s.format_open = false;
                    }
                    apply_menu_occlusion(hwnd);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                    return 0;
                }
            }
            // The path row's controls, before the footer and after the menus: the input is a native
            // child, so a click that lands on it never arrives here, which is why only the three
            // chips and the two drawn buttons are tested.
            if event == snapclip_ui::panel::PointerEvent::Down {
                let metrics = snapclip_ui::panel::DEMO;
                let scale = dpi_scale(hwnd);
                let rects = metrics.rects(scale);
                let point = (point.x, point.y);
                let (change, chips) = state(hwnd)
                    .map(|s| (s.path_change, s.path_chips))
                    .unwrap_or((0, [0; 3]));
                if snapclip_ui::panel::path_change_contains(&metrics, &rects, scale, change, point) {
                    choose_folder(hwnd);
                    return 0;
                }
                if snapclip_ui::panel::path_open_contains(&metrics, &rects, scale, point) {
                    open_directory(&control_text(
                        state(hwnd)
                            .map(|s| s.control(snapclip_ui::panel::FieldId::Path))
                            .unwrap_or(std::ptr::null_mut()),
                    ));
                    return 0;
                }
                if let Some(index) =
                    snapclip_ui::panel::path_chip_at(&metrics, &rects, scale, &chips, point)
                {
                    // The shortcut is the shell's answer for that folder, not a literal in the
                    // panel: the mockup's three paths name one person's home (`:739`–`:741`).
                    if let Some(directory) = shortcut_directory(index) {
                        set_directory(hwnd, &directory);
                    }
                    return 0;
                }
            }
            // The format control's menu hangs over everything below it, so it is answered before
            // the footer: a click on an option must choose a format rather than fall through to a
            // button that happens to be underneath. The rectangles come from the same producers the
            // painter uses, and the window's height is the panel's own, which is what the menu's
            // flip rule is written against.
            if event == snapclip_ui::panel::PointerEvent::Down {
                let metrics = snapclip_ui::panel::DEMO;
                let scale = dpi_scale(hwnd);
                let rects = metrics.rects(scale);
                let window_height = (metrics.panel_height() as f32 * scale).round() as i32;
                let point = (point.x, point.y);
                let open = state(hwnd).map(|s| s.format_open).unwrap_or(false);
                if open {
                    if let Some(index) = snapclip_ui::panel::fmt_item_at(
                        &metrics,
                        &rects,
                        scale,
                        window_height,
                        true,
                        point,
                    ) {
                        if let Some(s) = state_mut(hwnd) {
                            s.format = index;
                            s.format_open = false;
                        }
                        // The file's format decides what it weighs, so a measurement taken under the
                        // old one describes a file that no longer exists. Only a limit that is on
                        // needs the new number, which is why the refresh itself checks the switch.
                        refresh_estimate(hwnd);
                        // The extension is part of the target, so a different format asks about a
                        // different file: shot.png may be free while shot.jpg is not.
                        refresh_conflict(hwnd);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                        return 0;
                    }
                    let inside = snapclip_ui::panel::fmt_menu_contains(
                        &metrics,
                        &rects,
                        scale,
                        window_height,
                        point,
                    ) || snapclip_ui::panel::fmt_button_contains(&metrics, &rects, scale, point);
                    if !inside {
                        // A click anywhere else dismisses the menu and does nothing else: the first
                        // click after opening a menu belongs to the menu, which is how menus behave.
                        if let Some(s) = state_mut(hwnd) {
                            s.format_open = false;
                        }
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                        return 0;
                    }
                }
                if snapclip_ui::panel::fmt_button_contains(&metrics, &rects, scale, point) {
                    let still_open = !open;
                    if let Some(s) = state_mut(hwnd) {
                        s.format_open = still_open;
                        // One menu at a time, in this direction too.
                        s.var_open = false;
                    }
                    apply_menu_occlusion(hwnd);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                    return 0;
                }
            }
            // The footer's buttons are drawn by the frame as well, and their widths are the ones the
            // paint used, so the rectangle tested here is the rectangle on screen.
            if event == snapclip_ui::panel::PointerEvent::Down {
                let metrics = snapclip_ui::panel::DEMO;
                let scale = dpi_scale(hwnd);
                let widths = state(hwnd).map(|s| s.footer_widths).unwrap_or([0; 3]);
                for button in snapclip_ui::panel::FOOTER_BUTTONS {
                    if !snapclip_ui::panel::footer_button_contains(
                        &metrics,
                        scale,
                        widths,
                        button,
                        (point.x, point.y),
                    ) {
                        continue;
                    }
                    match button {
                        // The picture goes to the clipboard at the size the two cells say, through
                        // the same rule the save uses. A failure is logged and leaves the panel
                        // open: the clipboard can be held by another process for a moment, and that
                        // is not a reason to close the window or to invent a message about it.
                        snapclip_ui::panel::FooterButton::Copy => {
                            if let Some(state) = state(hwnd) {
                                match copy_picture(hwnd, state) {
                                    Some(Ok(())) => eprintln!("[save-panel] copied the picture"),
                                    Some(Err(error)) => {
                                        eprintln!("[save-panel] could not copy: {error}")
                                    }
                                    // No capture, so there is no picture: the development entry's
                                    // empty panel has nothing to copy and says so only in the log.
                                    None => eprintln!("[save-panel] nothing to copy"),
                                }
                            }
                        }
                        snapclip_ui::panel::FooterButton::Cancel => {
                            windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                                hwnd, WM_CLOSE, 0, 0,
                            );
                        }
                        snapclip_ui::panel::FooterButton::Save => {
                            attempt_save(hwnd);
                        }
                    }
                    return 0;
                }

                // The four size shortcuts under the cells (`:614`, `:258`). A shortcut sets both
                // numbers through the same rule the save applies, so what the panel shows and what
                // the file gets cannot disagree.
                let presets = state(hwnd).map(|s| s.size_presets).unwrap_or([0; 4]);
                let rects = metrics.rects(scale);
                if let Some(index) = snapclip_ui::panel::size_preset_contains(
                    &metrics,
                    &rects,
                    scale,
                    presets,
                    (point.x, point.y),
                ) {
                    if let Some(s) = state(hwnd) {
                        if let Some((_, width, height)) = s.preview.as_ref() {
                            let source = super::scale::Size::new(*width, *height);
                            let (width, height) = snapclip_ui::panel::size_preset_size(
                                snapclip_ui::panel::SIZE_PRESET_PCT[index],
                                (*width, *height),
                            );
                            // The shortcut names a share of the capture; the fields then say it in
                            // whichever unit the mode is in, so a shortcut in percent mode shows the
                            // percentage it was asked for rather than a pixel count.
                            let text = super::scale::display_text(
                                super::scale::SizeMode::from_index(s.size_mode),
                                super::scale::Size::new(width, height),
                                source,
                            );
                            s.set_size_text(&text.0, &text.1);
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                        }
                    }
                    return 0;
                }

                // The two mode buttons (`数值`, `百分比`, `:585`, `:586`). Switching the unit does not
                // change the size; it rewrites the two fields from the size that is already there
                // (`:1072`), which is why they are read with the old unit and written with the new.
                let mode_widths = state(hwnd).map(|s| s.size_mode_widths).unwrap_or([0; 2]);
                if let Some(index) = snapclip_ui::panel::size_mode_contains(
                    &metrics,
                    &rects,
                    scale,
                    mode_widths,
                    (point.x, point.y),
                ) {
                    if let Some(s) = state_mut(hwnd) {
                        switch_size_mode(hwnd, s, index);
                    }
                    return 0;
                }

                // The reset (`:589`, `:1213`): both numbers back to the capture's own size, in
                // whichever unit the mode is in — one hundred percent, when it is percentages. The
                // mode itself is left alone, because the mockup leaves it alone.
                if snapclip_ui::panel::size_reset_contains(&metrics, &rects, scale, (point.x, point.y))
                {
                    if let Some(s) = state_mut(hwnd) {
                        reset_size(hwnd, s);
                    }
                    return 0;
                }
            }
            // The size limit's switch (`:231`) and the escape hatch it offers (`:235`). Both are
            // drawn by the frame from the same rectangles this tests, and the link answers only
            // while the note says the limit was missed — a link that shrank the picture when it
            // already fit would be a button with nothing to do.
            if event == snapclip_ui::panel::PointerEvent::Down {
                let metrics = snapclip_ui::panel::DEMO;
                let scale = dpi_scale(hwnd);
                let rects = metrics.rects(scale);
                let point = (point.x, point.y);
                let (over, words) = state(hwnd)
                    .map(|s| (s.limit_on && s.over_limit, s.limit_words))
                    .unwrap_or((false, snapclip_ui::panel::LimitWidths::default()));
                let switch = snapclip_ui::panel::limit_switch_rect(
                    &metrics,
                    &rects,
                    scale,
                    words.switch_word_px,
                );
                if snapclip_ui::panel::rect_contains(switch, point) {
                    toggle_limit(hwnd);
                    return 0;
                }
                if over {
                    let link = snapclip_ui::panel::auto_shrink_rect(
                        &metrics,
                        &rects,
                        scale,
                        words.auto_shrink_px,
                    );
                    if snapclip_ui::panel::rect_contains(link, point) {
                        shrink_to_fit(hwnd);
                        return 0;
                    }
                }
            }
            // The fill row's swatches answer only while the row is drawn: `:1102` shows it for JPG
            // alone, and the same predicate gates this, so a swatch that is not on screen cannot
            // change the colour the encoder will composite with.
            if event == snapclip_ui::panel::PointerEvent::Down {
                let metrics = snapclip_ui::panel::DEMO;
                let scale = dpi_scale(hwnd);
                let rects = metrics.rects(scale);
                let shown = state(hwnd)
                    .map(|s| snapclip_ui::panel::fill_row_shown(s.format))
                    .unwrap_or(false);
                if shown {
                    if let Some(index) = snapclip_ui::panel::fill_swatch_at(
                        &metrics,
                        &rects,
                        scale,
                        (point.x, point.y),
                    ) {
                        if choose_fill(hwnd, index) {
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                        }
                        return 0;
                    }
                }
            }
            // The quality control only answers while it is drawn. PNG has no tier to pick, so the row
            // shows the hint instead (`:639`, `:1096`), and a click on that space has to do nothing
            // rather than change a setting nothing on screen refers to.
            let chosen = state_mut(hwnd)
                .filter(|s| snapclip_ui::panel::quality_segment_shown(s.format))
                .map(|s| s.quality_pointer(dpi_scale(hwnd), event, point))
                .unwrap_or(false);
            if chosen {
                // The tier decides the encoder's quality, so the estimate belongs to the old one
                // until it is taken again for the new one.
                refresh_estimate(hwnd);
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
            // The footer's widths are pixels, so a new scale makes the old ones stale. They are
            // measured from the same renderer the paint uses, so re-measuring here keeps the click
            // targets on the buttons as they are now drawn.
            if let Some(s) = state_mut(hwnd) {
                s.footer_widths = measure_footer_widths(hwnd, s.renderer.as_ref());
                // The chips' widths are pixels too, and so is the `更改` button's, so a new scale
                // makes all three stale for the same reason.
                s.path_chips = measure_path_chips(hwnd, s.renderer.as_ref());
                s.path_change = measure_path_change(hwnd, s.renderer.as_ref());
                // The size shortcuts are words too, so their widths are pixels and go stale the same
                // way.
                s.size_presets = measure_size_presets(hwnd, s.renderer.as_ref());
                // The two mode buttons are words as well, measured in two weights.
                s.size_mode_widths = measure_size_mode(hwnd, s.renderer.as_ref());
                // The limit's words are pixels too, so they go stale the same way: the switch would
                // otherwise be tested against the previous monitor's word width.
                s.limit_words = measure_limit_words(hwnd, s.renderer.as_ref());
                // The link's words are pixels too, so they go stale on a dpi change as well.
                s.conflict_widths = measure_conflict_link(hwnd, s.renderer.as_ref());
            }
            apply_menu_occlusion(hwnd);
            0
        }
        // `Esc` is a cancel, and at this layer cancelling the panel *is* closing it: the shell's
        // state machine owns what that means (`Shell::close`), and the window only has to stop
        // being a window. The close button arrives with the controls.
        WM_KEYDOWN => {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_RETURN};
            if wparam == VK_ESCAPE as usize {
                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(hwnd, WM_CLOSE, 0, 0);
            } else if wparam == VK_RETURN as usize {
                // The panel itself can hold the focus — a click on empty space leaves it there — and
                // then no control is there to translate the key, so the panel answers it directly.
                // The same key pressed inside a field arrives through `shortcut_command` instead.
                attempt_save(hwnd);
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
            const EN_CHANGE: u16 = 0x0300;
            let notification = ((wparam >> 16) & 0xFFFF) as u16;
            let control = lparam as windows_sys::Win32::Foundation::HWND;
            // A command with no control behind it is the dialog manager's, not a child's:
            // `IsDialogMessage` sends the keys a focused control will not take as `IDOK` and
            // `IDCANCEL`. The mockup's own titles promise the two (`保存 (Enter)`, `:686`; `取消
            // (Esc)`, `:684`).
            if control.is_null() {
                match shortcut_command(wparam & 0xFFFF) {
                    Some(Shortcut::Save) => attempt_save(hwnd),
                    Some(Shortcut::Cancel) => {
                        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                            hwnd, WM_CLOSE, 0, 0,
                        );
                    }
                    None => {}
                }
                return 0;
            }
            let mut repaint = false;
            let mut conflict_dirty = false;
            if let Some(s) = state_mut(hwnd) {
                if let Some(id) = s.field_of(control) {
                    // The name field is a template, and the spec keeps it as it is typed (`:201`), so
                    // the shell can open the next panel with what this one was left holding. Setting
                    // a control's text fires the same notification as typing does — the panel's own
                    // initial text arrives here too — which is why the rule, not this handler,
                    // decides whether anything actually changed.
                    if notification == EN_CHANGE {
                        let edited = control_text(control);
                        if let Ok(mut remembered) = s.remembered.lock() {
                            note_edit(&mut remembered, id, true, &edited);
                        }
                        // The name and the directory decide which file the link is about, so a change
                        // to either re-asks the question; the other fields cannot move the target.
                        conflict_dirty = matches!(
                            id,
                            snapclip_ui::panel::FieldId::Name | snapclip_ui::panel::FieldId::Path
                        );
                    }
                    // Focus is not allowed to overwrite a message about the value: the field would
                    // fall silent exactly while the user is fixing what it complained about.
                    let next = match notification {
                        EN_SETFOCUS => Some(true),
                        EN_KILLFOCUS => Some(false),
                        _ => None,
                    };
                    if let Some(focused) = next {
                        let before = s.fields.state_of(id);
                        s.fields.set_focus(id, focused);
                        repaint = s.fields.state_of(id) != before;
                    }
                }
            }
            if conflict_dirty {
                refresh_conflict(hwnd);
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

    /// A save that worked says nothing: the panel closes, so there would be no line to read.
    #[test]
    fn a_successful_save_leaves_the_status_line_alone() {
        let saved = service::SaveOutcome {
            path: std::path::PathBuf::from(r"C:\shots\SnapClip_2026-10-24.png"),
            bytes: 12,
            format: SaveFormat::Png,
            warnings: Vec::new(),
        };
        assert_eq!(status_of(&Ok(saved)), None);
    }

    /// A cancel is not a failure: nothing went wrong, the user changed their mind, and painting it
    /// in the error colour would be a claim about what happened that is not true.
    #[test]
    fn a_cancelled_save_says_nothing() {
        assert_eq!(status_of(&Err(service::SaveError::Cancelled)), None);
    }

    /// A rejected save shows the sentence its key owns, at the level that key carries. The wording
    /// is not re-spelled here: `copy` is the one place that has it.
    #[test]
    fn a_rejected_save_shows_the_copy_tables_sentence_and_level() {
        let result = Err(service::SaveError::Validation {
            copy: snapclip_ui::copy::ValidationCopy::StemEmpty,
            detail: None,
        });
        assert_eq!(
            status_of(&result),
            Some((
                snapclip_ui::copy::Level::Error,
                "文件名不能为空".to_owned()
            ))
        );

        // A warning key carries the warning level, and the panel paints it differently because of
        // the level rather than because someone remembered to.
        let warning = Err(service::SaveError::Validation {
            copy: snapclip_ui::copy::ValidationCopy::DirectoryMissing,
            detail: None,
        });
        assert_eq!(
            status_of(&warning),
            Some((
                snapclip_ui::copy::Level::Warn,
                snapclip_ui::copy::ValidationCopy::DirectoryMissing
                    .message()
                    .to_owned()
            ))
        );
    }

    /// The filled sentence wins over the template: `含非法字符 {…}` with the list spelled out is
    /// what the user needs, and the template with its braces would be worse than useless.
    #[test]
    fn a_filled_detail_wins_over_the_keys_template() {
        let result = Err(service::SaveError::Validation {
            copy: snapclip_ui::copy::ValidationCopy::StemIllegalChars,
            detail: Some(snapclip_ui::copy::illegal_chars("a*b")),
        });
        assert_eq!(
            status_of(&result),
            Some((
                snapclip_ui::copy::Level::Error,
                snapclip_ui::copy::illegal_chars("a*b")
            ))
        );
    }

    /// A failure with no copy-table key still says something rather than vanishing: its own message,
    /// at error level. Those messages are developer-facing for now (`docs/41` A20).
    #[test]
    fn a_failure_without_a_key_still_says_something() {
        let result = Err(service::SaveError::Io("disk on fire".to_owned()));
        let (level, text) = status_of(&result).expect("an I/O failure is visible");
        assert_eq!(level, snapclip_ui::copy::Level::Error);
        assert!(text.contains("disk on fire"), "{text}");
    }

    /// The directory and the name are both strings, so nothing about the types stops them being
    /// handed over the wrong way round. A save with them swapped looks entirely reasonable and
    /// writes somewhere else, which is why the pairing is asserted rather than noticed later.
    #[test]
    fn the_save_takes_the_directory_and_the_name_in_that_order() {
        let values = panel_values(
            r"C:\shots",
            "SnapClip_[日期]",
            "2026-10-24",
            "0115",
            7,
            format_of_index(0),
            crate::save::assemble::quality_of_tier(0),
            snapclip_ui::panel::FILL_WHITE,
            ConflictPolicy::AutoRename,
        );
        assert_eq!(values.directory, r"C:\shots");
        assert_eq!(values.name, "SnapClip_[日期]");
        assert_eq!(values.date, "2026-10-24");
        assert_eq!(values.time, "0115");
        assert_eq!(values.sequence, 7, "the number comes from the shell's counter");
        assert_eq!(values.app, "SnapClip");
    }

    /// The menu's order is the design's (FMT_OPTIONS, :717), and the index it reports is not a
    /// format: the translation lives in one function so a save cannot write a JPG where the panel
    /// said WebP. Both are small integers, which is exactly the pair this repository has had to
    /// assert separately every time.
    #[test]
    fn the_menu_index_is_the_format_the_save_writes() {
        assert_eq!(format_of_index(0), SaveFormat::Png, ".PNG is first (:717)");
        assert_eq!(format_of_index(1), SaveFormat::Jpeg, ".JPG is second");
        assert_eq!(format_of_index(2), SaveFormat::Webp, ".WebP is third");
        assert_eq!(
            format_of_index(99),
            SaveFormat::Png,
            "an out-of-range index falls back to the first rather than panicking"
        );
    }

    /// The time the spec asks for is `HHmm` (`docs/design/…规格.md`): no separator, because a colon
    /// is not a legal file name character, and local rather than UTC, because a file named for
    /// yesterday is a surprise.
    #[test]
    fn the_local_date_and_time_are_filename_shaped() {
        let (date, time) = local_date_and_time();
        assert_eq!(date.len(), 10, "YYYY-MM-DD: {date}");
        assert_eq!(date.as_bytes()[4], b'-');
        assert_eq!(date.as_bytes()[7], b'-');
        assert_eq!(time.len(), 4, "HHmm: {time}");
        assert!(
            date.bytes().chain(time.bytes()).all(|b| b.is_ascii_digit() || b == b'-'),
            "nothing in a file name may need escaping: {date} {time}"
        );
    }

    /// The two size cells are two numbers of the same type, so nothing about the type stops the
    /// width from being handed to the height cell. On screen that would look entirely reasonable,
    /// which is why the pairing is asserted here rather than noticed later.
    #[test]
    fn the_size_cells_take_the_width_and_the_height_in_that_order() {
        let size = Some((1440, 900));
        assert_eq!(
            initial_text(
                snapclip_ui::panel::FieldId::SizeWidth,
                size,
                crate::save::DEFAULT_TEMPLATE,
                ""
            ),
            "1440"
        );
        assert_eq!(
            initial_text(
                snapclip_ui::panel::FieldId::SizeHeight,
                size,
                crate::save::DEFAULT_TEMPLATE,
                ""
            ),
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
            assert_eq!(
                initial_text(id, None, crate::save::DEFAULT_TEMPLATE, ""),
                "",
                "{id:?} must stay empty"
            );
        }
    }

    /// `Enter` saves and `Esc` cancels, and nothing else is a shortcut.
    ///
    /// The ids are the dialog manager's: `IDOK` is 1 and `IDCANCEL` is 2. `IsDialogMessage` sends
    /// them when the focused control will not take the key, which is why the panel could not see
    /// `Enter` at all until the message loop started translating: a single-line edit swallows it, so
    /// there was no `WM_KEYDOWN` for the panel to receive.
    #[test]
    fn the_two_shortcut_keys_are_enter_and_escape() {
        assert_eq!(shortcut_command(1), Some(Shortcut::Save), "IDOK is Enter (:686)");
        assert_eq!(shortcut_command(2), Some(Shortcut::Cancel), "IDCANCEL is Esc (:684)");
        for other in [0usize, 3, 7] {
            assert_eq!(
                shortcut_command(other),
                None,
                "{other} is a command id, but not one of these two"
            );
        }
    }

    /// The name row opens with the remembered template; the path row opens where the shell says.
    ///
    /// The template is the spec's (`:68`), not the mockup's placeholder: the box holds a template
    /// whose variables are expanded at save time (`:189`), and `SnapClip_[日期]` is what the mockup
    /// puts in it (`:556`). A panel with no memory — a first run — gets that default.
    #[test]
    fn the_name_row_starts_with_the_template_it_was_given() {
        assert_eq!(
            initial_text(
                snapclip_ui::panel::FieldId::Name,
                None,
                crate::save::DEFAULT_TEMPLATE,
                ""
            ),
            "SnapClip_[日期]"
        );
        // A remembered template wins over the default: the panel opens with what the last one held.
        assert_eq!(
            initial_text(snapclip_ui::panel::FieldId::Name, None, "我的_[序号]", ""),
            "我的_[序号]"
        );
    }

    /// The path row opens on the directory the shell found, and on nothing when it declined.
    ///
    /// The mockup's `C:\Users\阿伟\Desktop` (`:544`) is one person's desktop, so the value is
    /// passed in rather than copied; this asserts it is carried through unchanged, and that an
    /// empty answer stays empty instead of turning into some plausible directory.
    #[test]
    fn the_path_row_starts_where_the_shell_says_and_nowhere_else() {
        use snapclip_ui::panel::FieldId;

        assert_eq!(
            initial_text(
                FieldId::Path,
                None,
                crate::save::DEFAULT_TEMPLATE,
                "D:\\Pictures"
            ),
            "D:\\Pictures",
            "the path row shows the directory it was given"
        );
        assert_eq!(
            initial_text(FieldId::Path, None, crate::save::DEFAULT_TEMPLATE, ""),
            "",
            "a shell that declines leaves the field empty rather than inventing a directory"
        );
    }

    /// The template survives the panel that held it.
    ///
    /// This walks the cycle the window performs, without a window: the panel opens with the
    /// remembered template, the change notification writes an edited one back, and the next panel
    /// opens with that. The assertion that the two differ is what makes the first line mean
    /// something — otherwise "both open with the default" would pass too.
    #[test]
    fn the_next_panel_opens_with_the_template_the_last_one_was_left_with() {
        use snapclip_ui::panel::FieldId;

        let remembered = std::sync::Arc::new(std::sync::Mutex::new(Remembered::default()));
        let read = |remembered: &std::sync::Arc<std::sync::Mutex<Remembered>>| {
            remembered.lock().map(|r| r.template.clone()).unwrap_or_default()
        };

        let first = read(&remembered);
        assert_eq!(
            initial_text(FieldId::Name, None, &first, ""),
            "SnapClip_[日期]",
            "a first run opens with the spec's template"
        );

        // What the change notification does when the user types.
        remembered
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remember_template("我的_[序号]");

        let second = read(&remembered);
        assert_eq!(
            initial_text(FieldId::Name, None, &second, ""),
            "我的_[序号]",
            "the next panel opens with the remembered template"
        );
        assert_ne!(first, second, "the two panels do not open with the same text");
    }

    /// Which notifications write the remembered template.
    ///
    /// The path row is the one that matters: it is the other `EDIT` in the panel, its text is also a
    /// string, and a rule that matched on "a text field changed" would put a directory into the name
    /// box of the next panel. Every other field has the same shape, so the rule names the field.
    #[test]
    fn only_a_change_in_the_name_box_writes_the_template() {
        use snapclip_ui::panel::FieldId;

        let mut remembered = Remembered::default();

        assert!(
            note_edit(&mut remembered, FieldId::Name, true, "我的_[序号]"),
            "a change in the name box is a template edit"
        );
        assert_eq!(remembered.template, "我的_[序号]");

        assert!(
            !note_edit(&mut remembered, FieldId::Path, true, "C:\\shots"),
            "a change in the path box is not an edit of the name"
        );
        assert_eq!(
            remembered.template, "我的_[序号]",
            "the path must not become the template"
        );

        assert!(
            !note_edit(&mut remembered, FieldId::Name, false, "别的"),
            "gaining or losing focus is not an edit"
        );
        assert_eq!(
            remembered.template, "我的_[序号]",
            "focus must not write the text it was given"
        );
    }

    /// Opening a panel sets the name box's text to the remembered template, and setting a control's
    /// text fires the same change notification as typing does. The handler therefore runs with the
    /// text it has just installed, which must be a no-op rather than a write that could reorder the
    /// shell's state behind the open.
    #[test]
    fn installing_the_initial_text_is_not_a_change() {
        let mut remembered = Remembered::default();
        let initial = initial_text(
            snapclip_ui::panel::FieldId::Name,
            None,
            &remembered.template,
            "",
        );

        assert!(
            !remembered.remember_template(&initial),
            "the panel's own initial text must not count as an edit"
        );
        assert_eq!(
            remembered.template,
            crate::save::DEFAULT_TEMPLATE,
            "the remembered template is unchanged"
        );
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
            fill: snapclip_ui::panel::FILL_WHITE,
            segment: snapclip_ui::panel::quality_segment_state(
                &snapclip_ui::panel::DEMO,
                &snapclip_ui::panel::DEMO.rects(1.0),
                1.0,
                0,
            ),
            segment_scale: 1.0,
            footer_widths: [40, 52, 64],
            path_change: 56,
            format: 0,
            format_open: false,
            remembered: std::sync::Arc::new(std::sync::Mutex::new(Remembered::default())),
            status: None,
            var_open: false,
            path_chips: [40, 52, 64],
            size_presets: [40, 36, 36, 36],
        size_mode: snapclip_ui::panel::SIZE_MODE_CHOSEN,
        size_mode_widths: [56, 64],
            limit_on: false,
        conflict: ConflictPolicy::AutoRename,
        conflict_shown: false,
        conflict_widths: [0; 2],
            limit_words: snapclip_ui::panel::LimitWidths::default(),
            estimate: None,
            over_limit: false,
            copy: std::sync::Arc::new(SystemImageCopy),
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
            fill: snapclip_ui::panel::FILL_WHITE,
            segment: snapclip_ui::panel::quality_segment_state(
                &snapclip_ui::panel::DEMO,
                &snapclip_ui::panel::DEMO.rects(1.0),
                1.0,
                0,
            ),
            segment_scale: 1.0,
            footer_widths: [40, 52, 64],
            path_change: 56,
            format: 0,
            format_open: false,
            remembered: std::sync::Arc::new(std::sync::Mutex::new(Remembered::default())),
            status: None,
            var_open: false,
            path_chips: [40, 52, 64],
            size_presets: [40, 36, 36, 36],
        size_mode: snapclip_ui::panel::SIZE_MODE_CHOSEN,
        size_mode_widths: [56, 64],
            limit_on: false,
        conflict: ConflictPolicy::AutoRename,
        conflict_shown: false,
        conflict_widths: [0; 2],
            limit_words: snapclip_ui::panel::LimitWidths::default(),
            estimate: None,
            over_limit: false,
            copy: std::sync::Arc::new(SystemImageCopy),
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
            fill: snapclip_ui::panel::FILL_WHITE,
            segment: snapclip_ui::panel::quality_segment_state(
                &snapclip_ui::panel::DEMO,
                &snapclip_ui::panel::DEMO.rects(1.0),
                1.0,
                2,
            ),
            segment_scale: 1.0,
            footer_widths: [40, 52, 64],
            path_change: 56,
            format: 0,
            format_open: false,
            remembered: std::sync::Arc::new(std::sync::Mutex::new(Remembered::default())),
            status: None,
            var_open: false,
            path_chips: [40, 52, 64],
            size_presets: [40, 36, 36, 36],
        size_mode: snapclip_ui::panel::SIZE_MODE_CHOSEN,
        size_mode_widths: [56, 64],
            limit_on: false,
        conflict: ConflictPolicy::AutoRename,
        conflict_shown: false,
        conflict_widths: [0; 2],
            limit_words: snapclip_ui::panel::LimitWidths::default(),
            estimate: None,
            over_limit: false,
            copy: std::sync::Arc::new(SystemImageCopy),
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
        // The mock's defaults, with the height derivation spelled out so a change is deliberate.
        // The last term is the footer (`:274` with `--btn-h: 32` at `:50`), not the mock's `12px`
        // body bottom padding: that padding is the footer's own top padding now.
        assert_eq!(PANEL_WIDTH_DIP, 920.0);
        assert_eq!(PANEL_RADIUS_DIP, 12.0);
        assert_eq!(PANEL_PADDING_DIP, 16.0);
        assert_eq!(COLUMN_GAP_DIP, 12.0);
        assert_eq!(
            PANEL_HEIGHT_DIP,
            44.0 + 4.0 + 460.0 + 56.0,
            "the mock's content height: header (94) + body top (100) + preview (44) + footer (274)"
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
    /// A clipboard that records instead of writing, so a test can see what was asked for without
    /// touching the user's clipboard.
    struct RecordingCopy {
        seen: std::sync::Mutex<Vec<(u32, u32, usize)>>,
    }

    impl ImageCopy for RecordingCopy {
        fn copy_image(&self, width: u32, height: u32, bgra: &[u8]) -> Result<(), String> {
            self.seen
                .lock()
                .expect("the recorder's lock")
                .push((width, height, bgra.len()));
            Ok(())
        }
    }

    /// The footer's copy button hands the clipboard the picture at the size the two cells say, and
    /// it is the same rule the save uses: a copy that disagreed with the save about the size would
    /// be worse than no copy button.
    #[test]
    fn copying_uses_the_size_the_cells_say_and_the_picture_that_came_in() {
        let recorder = std::sync::Arc::new(RecordingCopy {
            seen: std::sync::Mutex::new(Vec::new()),
        });
        // Four by three BGRA, tightly packed: twelve pixels, forty eight bytes.
        let bgra = vec![0x7fu8; 4 * 3 * 4];
        let mut state = WindowState {
            backdrop: false,
            renderer: None,
            fields: snapclip_ui::panel::FieldStates::IDLE,
            controls: [std::ptr::null_mut(); snapclip_ui::panel::FieldId::ALL.len()],
            preview: Some((bgra.clone(), 4, 3)),
            fill_brush: std::ptr::null_mut(),
            fill: snapclip_ui::panel::FILL_WHITE,
            segment: snapclip_ui::panel::quality_segment_state(
                &snapclip_ui::panel::DEMO,
                &snapclip_ui::panel::DEMO.rects(1.0),
                1.0,
                0,
            ),
            segment_scale: 1.0,
            footer_widths: [40, 52, 64],
            path_change: 56,
            format: 0,
            format_open: false,
            remembered: std::sync::Arc::new(std::sync::Mutex::new(Remembered::default())),
            status: None,
            var_open: false,
            path_chips: [40, 52, 64],
            size_presets: [40, 36, 36, 36],
        size_mode: snapclip_ui::panel::SIZE_MODE_CHOSEN,
        size_mode_widths: [56, 64],
            limit_on: false,
        conflict: ConflictPolicy::AutoRename,
        conflict_shown: false,
        conflict_widths: [0; 2],
            limit_words: snapclip_ui::panel::LimitWidths::default(),
            estimate: None,
            over_limit: false,
            copy: recorder.clone(),
        };
        state.controls.fill(std::ptr::null_mut());

        // With no controls the size cells read as empty text, so the output size is the source's:
        // the panel says "leave it as it came" and the copy follows that.
        let outcome = copy_picture(std::ptr::null_mut(), &state);
        let seen = recorder.seen.lock().expect("the recorder's lock").clone();
        assert_eq!(
            outcome,
            Some(Ok(())),
            "a panel with a picture and no resizing copies it"
        );
        assert_eq!(
            seen,
            vec![(4, 3, 48)],
            "one copy, at the picture's own size, with every byte the panel holds"
        );

        // A panel with no picture has nothing to copy, and says so by answering `None` rather than
        // by copying an empty image.
        state.preview = None;
        assert_eq!(copy_picture(std::ptr::null_mut(), &state), None);
    }

    /// The three shortcut chips are the shell's own folders, and a chip past the third is not a
    /// chip: it answers `None` rather than a directory nobody asked for.
    #[test]
    fn a_shortcut_beyond_the_three_is_not_a_shortcut() {        assert_eq!(shortcut_directory(3), None);
        assert_eq!(shortcut_directory(99), None);
        for index in 0..3 {
            if let Some(path) = shortcut_directory(index) {
                assert!(
                    path.contains(":\\") || path.starts_with("\\\\"),
                    "shortcut {index} must be an absolute path, and this is {path:?}"
                );
                assert!(
                    !path.ends_with('\\'),
                    "shortcut {index} must not end with a separator, and this is {path:?}"
                );
            }
        }
    }

    /// What the limit field means, including the cases where it does not say a number.
    ///
    /// The floor is the design's (`:236`: `下限 10`), and it is also the answer for text that is not
    /// a number: a limit nothing can meet would refuse every save, and "unreadable" must not mean
    /// "impossible".
    #[test]
    fn the_limit_field_reads_as_kilobytes_or_as_the_floor() {
        assert_eq!(limit_kilobytes("500"), 500);
        assert_eq!(limit_kilobytes(" 300 "), 300, "surrounding space is not a number");
        assert_eq!(limit_kilobytes("5"), MIN_LIMIT_KB, "below the design's floor");
        assert_eq!(limit_kilobytes("0"), MIN_LIMIT_KB, "zero kilobytes is not a limit");
        assert_eq!(limit_kilobytes(""), MIN_LIMIT_KB, "an empty field is not a number");
        assert_eq!(limit_kilobytes("abc"), MIN_LIMIT_KB, "nor is a word");
        assert_eq!(limit_kilobytes("-1"), MIN_LIMIT_KB, "nor is a negative count");
    }

    /// The sentence under the quality line, in its three shapes (`:234`).
    ///
    /// The digits are asserted as well as the words: a note that always said the same number would
    /// pass a test that only looked for the phrase.
    #[test]
    fn the_note_says_what_the_limit_did() {
        let fits = limit_note(Some(234_567), false, false).expect("a measured note");
        assert_eq!(
            fits,
            format!(
                "{}{}{}",
                snapclip_ui::panel::LIMIT_NOTE_OK_BEFORE,
                crate::save::estimate::format_bytes(234_567),
                snapclip_ui::panel::LIMIT_NOTE_OK_AFTER
            ),
            "a file under the limit says so, with its own weight"
        );
        let over = limit_note(Some(655_360), true, false).expect("a measured note");
        assert!(
            over.starts_with(snapclip_ui::panel::LIMIT_NOTE_OVER_BEFORE)
                && over.ends_with(snapclip_ui::panel::LIMIT_NOTE_OVER_AFTER)
                && over.contains(&crate::save::estimate::format_bytes(655_360)),
            "an over-limit jpeg says the lowest tier is still too big, and how big: {over:?}"
        );
        let over_png = limit_note(Some(655_360), true, true).expect("a measured note");
        assert!(
            over_png.starts_with(snapclip_ui::panel::LIMIT_NOTE_OVER_PNG_BEFORE)
                && over_png.ends_with(snapclip_ui::panel::LIMIT_NOTE_OVER_AFTER),
            "png has no tier to lower, so its sentence says it is lossless instead: {over_png:?}"
        );
        assert_eq!(limit_note(None, false, false), None, "nothing measured, nothing said");
        assert_eq!(limit_note(None, true, true), None, "nothing measured, nothing claimed");
    }

    /// The limit is a promise about the **file**, which is the only place it can be checked.
    ///
    /// The picture is noise because noise does not compress: it makes the unlimited file larger than
    /// the limit this test then asks for, so the shrinking half cannot pass by the picture having
    /// been small already.
    #[test]
    fn a_limit_the_picture_exceeds_writes_a_file_under_it() {
        let directory = std::env::temp_dir().join(format!(
            "snapclip-limit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&directory).expect("a scratch directory");
        let directory_text = directory.to_string_lossy().into_owned();
        // Nine hundred by seven hundred of noise: the pixels are incompressible, so a limit bites.
        let (width, height) = (900u32, 700u32);
        let mut bgra = Vec::with_capacity((width as usize) * (height as usize) * 4);
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..(width as usize * height as usize) {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            bgra.extend_from_slice(&[seed as u8, (seed >> 8) as u8, (seed >> 16) as u8, 255]);
        }
        let size = crate::save::scale::Size::new(width, height);
        let frame = crate::save::scale::Frame {
            bgra: &bgra,
            source: size,
            output: size,
        };
        // A literal name, so the values' borrow outlives the call rather than the argument.
        let values = |name: &'static str| {
            panel_values(
                &directory_text,
                name,
                "2026-10-24",
                "1042",
                1,
                SaveFormat::Png,
                92,
                snapclip_ui::panel::FILL_WHITE,
                ConflictPolicy::AutoRename,
            )
        };

        let unlimited = save_with_limit(None, &values("noise"), &frame)
            .expect("no limit is no refusal")
            .expect("the unlimited save");
        let unlimited_bytes = std::fs::metadata(&unlimited.path)
            .expect("the unlimited file")
            .len();
        let limit: u64 = 200 * 1024;
        assert!(
            unlimited_bytes > limit,
            "the picture must be over the limit for the shrinking half to mean anything: \
             {unlimited_bytes} bytes"
        );

        let limited = save_with_limit(Some(limit), &values("capped"), &frame)
            .expect("a limit this size is reachable")
            .expect("the limited save");
        let limited_bytes = std::fs::metadata(&limited.path)
            .expect("the limited file")
            .len();
        assert!(
            limited_bytes <= limit,
            "the file is {limited_bytes} bytes, over the {limit} byte limit"
        );
        // The file is a real picture at a smaller size, not an empty file that trivially fits.
        assert!(
            limited_bytes > 1024,
            "a ten kilobyte limit should still leave a picture: {limited_bytes} bytes"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
}

    /// The link's word is the action a press takes, so the two policies name each other.
    #[test]
    fn the_conflict_link_offers_the_policy_that_is_not_in_force() {
        // Renaming is the default, so the link offers overwriting, and the words are the design's
        // own (copy::ConflictLink). A label naming the policy in force would read as a statement
        // of fact rather than as something to press.
        assert_eq!(
            conflict_link(ConflictPolicy::AutoRename),
            snapclip_ui::copy::ConflictLink::Overwrite
        );
        assert_eq!(
            conflict_link(ConflictPolicy::Overwrite),
            snapclip_ui::copy::ConflictLink::Rename
        );
        assert_eq!(conflict_link(ConflictPolicy::AutoRename).text(), "改为覆盖");
        assert_eq!(conflict_link(ConflictPolicy::Overwrite).text(), "改为重命名");
    }

    /// The choice has to survive the trip from the panel to the request, or the link would flip a
    /// flag that nothing reads and the next save would quietly rename anyway.
    #[test]
    fn the_panels_conflict_choice_reaches_the_values() {
        let values = panel_values(
            r"C:\\shots",
            "SnapClip_[日期]",
            "2026-10-24",
            "0115",
            7,
            format_of_index(0),
            crate::save::assemble::quality_of_tier(0),
            snapclip_ui::panel::FILL_WHITE,
            ConflictPolicy::Overwrite,
        );
        assert_eq!(values.conflict, ConflictPolicy::Overwrite);
    }

    /// The file system decides, and the panel's choice is what picks between replacing and keeping
    /// beside. Asserted on the files because that is where the difference is.
    #[test]
    fn a_taken_name_is_replaced_or_kept_beside_by_the_panels_choice() {
        let directory = std::env::temp_dir().join(format!(
            "snapclip-conflict-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&directory).expect("a scratch directory");
        let directory_text = directory.to_string_lossy().into_owned();
        let (width, height) = (8u32, 8u32);
        let mut bgra = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                bgra.extend_from_slice(&[(x * 31) as u8, (y * 31) as u8, 128, 255]);
            }
        }
        let size = crate::save::scale::Size::new(width, height);
        let frame = crate::save::scale::Frame {
            bgra: &bgra,
            source: size,
            output: size,
        };
        let values = |conflict: ConflictPolicy| {
            panel_values(
                &directory_text,
                "shot",
                "2026-10-24",
                "1042",
                1,
                SaveFormat::Png,
                92,
                snapclip_ui::panel::FILL_WHITE,
                conflict,
            )
        };

        // Last night's screenshot, holding bytes no png encoder would ever write.
        let plain = directory.join("shot.png");
        std::fs::write(&plain, b"last night").expect("a sentinel file");

        let renamed = save_panel(&values(ConflictPolicy::AutoRename), &frame).expect("the save");
        assert_ne!(
            renamed.path, plain,
            "a taken name is not reused while renaming"
        );
        assert_eq!(
            std::fs::read(&plain).expect("the sentinel"),
            b"last night",
            "renaming has to leave the existing file alone"
        );
        assert!(renamed.path.exists(), "the renamed file is on disk");

        let overwritten = save_panel(&values(ConflictPolicy::Overwrite), &frame).expect("the save");
        assert_eq!(overwritten.path, plain, "overwriting keeps the name");
        assert_ne!(
            std::fs::read(&plain).expect("the replaced file"),
            b"last night",
            "overwriting replaces the bytes"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// The folder dialog starts where the field points, and asks for the shell's default when the
    /// field names nothing rather than for a path that is not a path.
    #[test]
    fn the_folder_dialog_starts_where_the_path_field_points() {
        assert!(browse_initial_path("").is_none());
        assert!(browse_initial_path("   ").is_none());
        let path = browse_initial_path(r"C:\shots").expect("a path asks for a start");
        assert_eq!(*path.last().expect("terminated"), 0, "the dialog reads a c string");
        assert_eq!(
            String::from_utf16_lossy(&path[..path.len() - 1]),
            r"C:\shots"
        );
    }
