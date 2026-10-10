//! `P0.6` / `E-INJECT-1`: does a *posted* wheel message actually scroll a browser?
//!
//! `docs/30` §24.6 commits the scroll driver to two parallel transports —
//! `SendInput` and `PostMessageW` — and picks between them up front from the
//! target's elevation and foreground state. That choice is only defensible if
//! `PostMessageW(WM_MOUSEWHEEL)` reaches a Chromium render widget at all;
//! Chromium is the one target we cannot ship without (`docs/30` §25.1).
//! `refer/shot-refer/Crisp-main` claims Chromium ignores posted wheel messages;
//! `snow_shot` posts them and descends to the deepest child window first. Both
//! cannot be right, and the difference decides whether the two transports are
//! genuinely parallel or whether `PostMessageW` is dead code.
//!
//! This module is the experiment, not the implementation: it is the smallest
//! harness that can answer the question without a capture backend, a canvas, or
//! a session. It measures a scroll the only way the design itself would — by
//! reading pixels and recovering the displacement — so a positive result is
//! evidence about the *product* mechanism, not about a private mock.
//!
//! Run it with a real interactive desktop (it opens and drives windows):
//!
//! ```text
//! cargo test -p snapclip-capture --lib inject_probe -- --ignored --nocapture
//! ```
//!
//! The pure-logic tests in this file (`the_shift_estimator_*`) are not ignored:
//! they run everywhere, and they are what makes the ignored test's numbers
//! trustworthy — a shift estimator that cannot recover a known synthetic shift
//! proves nothing about a wheel.
//!
//! ## Measurement
//!
//! For each arm we capture the target's *client* area with
//! [`bitblt::capture_rect`], trim the right edge (both fixtures have a scrollbar
//! there, and a scrollbar thumb moving is not content), reduce it to one
//! "ink" value per row, and then cross-correlate the before/after row series
//! over every candidate displacement. The recovered displacement is compared
//! against a ≥40 px threshold — the exit condition `docs/30` §35 gives `P0.6`,
//! not the exact injected amount.
//!
//! The row signal has to be free of periodic aliasing or it would report a
//! confident wrong shift on any list of equally sized lines. Both fixtures are
//! therefore built from **randomly sized lines** (a xorshift32 generator), so
//! the row series is a random sequence sampled at a fixed line height: shifting
//! it by exactly one line lands on different amplitudes, not on itself.
//! `the_shift_estimator_is_not_fooled_by_line_structure` asserts that property
//! directly, at a shift that is an exact multiple of the line height.
//!
//! ## Fixtures
//!
//! The second target is a **top-level `EDIT` control we create ourselves**
//! rather than `notepad.exe`. Windows 11's Notepad is a packaged app whose
//! window may not exist under the classic class name, and a fixture that
//! silently fails to appear would look like a negative result. An `EDIT`
//! control is a real, keyboard-focus-driven Win32 scroller with its own
//! independent ground truth (`EM_GETFIRSTVISIBLELINE`), so it validates both the
//! injection *and* the measurement before Chrome is measured at all.
//!
//! ## Second experiment: window-level capture (`P0.05` / `E-CAP-1`)
//!
//! `docs/30` §24.2 commits the scroll frame source to **window-level** WGC
//! (`IGraphicsCaptureItemInterop::CreateForWindow`) and to **one pool reused for
//! the whole session**. Today's production path only ever calls
//! `CreateForMonitor` and builds a pool per capture, so both claims are
//! unmeasured. `capture_window_arm` measures them on the target classes
//! `docs/30` `E-CAP-1` names — Chrome, Edge, Electron, WinUI3, Notepad — plus
//! WebView2, which this machine reports as unreachable rather than untested:
//! `CreateForWindow`, a three-buffer free-threaded pool, ten frame slots, the
//! black-frame ratio, and how many `Recreate` calls the pool needed.
//!
//! ```text
//! cargo test -p snapclip-capture --lib capture_probe -- --ignored --nocapture
//! ```
//!
//! Since `P2.01` the arm drives the **production** session (`wgc::WgcSession`)
//! rather than a private copy of it, so these numbers describe the code the scroll
//! loop will use. Two behaviours are still the arm's own, and both are recorded in
//! `docs/30` §24.2 rather than hidden here: it **collects** session-option failures
//! (the session records them as diagnostics instead of propagating them, because a
//! missing `GraphicsCaptureSession2/3` interface is a finding, not a reason to lose
//! the frames); and it **records a frame-less slot as idle instead of failing the
//! arm**. The first run failed Chrome and Edge at 1500ms and looked like two
//! unavailable targets; the failure was the probe's assumption that a window keeps
//! producing frames whether or not its content changes.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::ptr;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    BOOL, CloseHandle, FALSE, HWND, LPARAM, LRESULT, POINT, RECT, TRUE, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect,
    PAINTSTRUCT,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput, SetFocus, VK_DOWN, VK_MENU,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CWP_SKIPINVISIBLE, ChildWindowFromPointEx, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, ES_AUTOVSCROLL, ES_MULTILINE, EnumWindows, GetClassNameW, GetClientRect,
    GetCursorPos, GetForegroundWindow, GetWindowDisplayAffinity, GetWindowTextW,
    GetWindowThreadProcessId, HWND_TOP,
    HWND_TOPMOST,
    IsIconic, IsWindowVisible, MSG,
    PM_REMOVE, PeekMessageW, PostMessageW, RegisterClassW, SB_LINEDOWN, SPI_GETMOUSEWHEELROUTING,
    SPI_GETWHEELSCROLLLINES, SW_RESTORE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SendMessageW,
    SetCursorPos, SetForegroundWindow, SetWindowDisplayAffinity, SetWindowPos, ShowWindow,
    SystemParametersInfoW,
    TranslateMessage, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WM_ERASEBKGND, WM_MOUSEWHEEL, WM_PAINT,
    WM_VSCROLL, WNDCLASSW,
    WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL, WindowFromPoint,
};

use crate::geometry::{Point, Rect};
use crate::scroll::canvas::MemoryBudget;
use crate::scroll::observation::Axis;
use crate::scroll::ports::{FrameError, FrameSource, InjectOutcome, Poll, ScrollActuator};
use crate::scroll::session::{Disposal, ScrollPlan, ScrollRuntime, StopReason};
use crate::windows::monitor;
use crate::windows::scroll_actuator::{
    self, Aim, InjectPath, InjectRequest, InjectStatus, TargetProbe, WheelRouting, Win32Injection,
};
use crate::windows::scroll_source::{WgcFrameBackend, WgcFrameSource};
use crate::windows::win::{bitblt, d3d11, wgc};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};

/// Pixels trimmed from the right edge before measuring: the scrollbar.
const SCROLLBAR_TRIM: u32 = 24;

/// Row-value pairs closer than this are treated as the same peak, so that a
/// plateau does not report itself as its own runner-up.
const PEAK_SEPARATION: i32 = 8;

/// `docs/30` §35 `P0.6`: a scroll of at least this many pixels counts as proof
/// that the transport drove the target.
const MIN_PROVEN_SHIFT: i32 = 40;

/// `EM_GETFIRSTVISIBLELINE` — the edit control's own account of where it is.
/// Used as an independent ground truth for the Win32 fixture.
const EM_GETFIRSTVISIBLELINE: u32 = 0x04CE;

const WHEEL_DELTA: i32 = 120;

// ---------------------------------------------------------------------------
// Pure signal handling (no Windows, runs in CI)
// ---------------------------------------------------------------------------

/// Zero-normalised cross correlation. Returns 0.0 when either input is flat,
/// which is the honest answer: a constant row series constrains nothing.
fn zncc(a: &[f64], b: &[f64]) -> f64 {
    assert_eq!(a.len(), b.len(), "zncc needs equal-length windows");
    let n = a.len() as f64;
    if n == 0.0 {
        return 0.0;
    }
    let mean_a = a.iter().sum::<f64>() / n;
    let mean_b = b.iter().sum::<f64>() / n;
    let mut covariance = 0.0;
    let mut variance_a = 0.0;
    let mut variance_b = 0.0;
    for i in 0..a.len() {
        let da = a[i] - mean_a;
        let db = b[i] - mean_b;
        covariance += da * db;
        variance_a += da * da;
        variance_b += db * db;
    }
    if variance_a <= 1e-12 || variance_b <= 1e-12 {
        return 0.0;
    }
    covariance / (variance_a.sqrt() * variance_b.sqrt())
}

/// How far the content moved between two captures.
#[derive(Clone, Copy, Debug)]
struct ShiftEstimate {
    /// Positive means the viewport moved *down*: `after[0]` holds what
    /// `before[shift]` held, so content scrolled up out of view. This is the
    /// same sign convention the design's `d` uses.
    shift: i32,
    /// Correlation at `shift`.
    correlation: f64,
    /// Best correlation at a displacement at least [`PEAK_SEPARATION`] away.
    /// A runner-up close to the winner means the peak is not unique.
    runner_up: f64,
    /// Correlation at zero displacement. A scroll must beat this.
    zero_correlation: f64,
}

/// Recover the displacement between two row series of equal length.
///
/// Only the overlapping part is correlated, and at least half of the window
/// must overlap, which bounds the search to what a single wheel step can
/// produce.
fn estimate_shift(before: &[f64], after: &[f64]) -> ShiftEstimate {
    let height = before.len().min(after.len());
    let zero_correlation = zncc(&before[..height], &after[..height]);
    let max_shift = (height / 2) as i32;

    let mut best = ShiftEstimate {
        shift: 0,
        correlation: f64::NEG_INFINITY,
        runner_up: f64::NEG_INFINITY,
        zero_correlation,
    };
    let mut scored: Vec<(i32, f64)> = Vec::with_capacity(max_shift as usize + 1);
    for shift in 0..=max_shift {
        let window = height - shift as usize;
        let correlation = zncc(
            &before[shift as usize..shift as usize + window],
            &after[..window],
        );
        scored.push((shift, correlation));
        if correlation > best.correlation {
            best.correlation = correlation;
            best.shift = shift;
        }
    }
    for (shift, correlation) in scored {
        if (shift - best.shift).abs() >= PEAK_SEPARATION && correlation > best.runner_up {
            best.runner_up = correlation;
        }
    }
    best
}

/// Deterministic xorshift32 — the fixtures must be reproducible byte for byte,
/// so no clock or thread-local randomness is allowed in here.
struct XorShift32(u32);

impl XorShift32 {
    fn new(seed: u32) -> Self {
        Self(if seed == 0 { 0x9E37_79B9 } else { seed })
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    fn next_in(&mut self, low: u32, high: u32) -> u32 {
        debug_assert!(low < high);
        low + self.next_u32() % (high - low)
    }
}

/// One fixture line: a random number of printable characters. The character
/// *count* is what makes the per-row ink amplitude random, which is what keeps
/// the row series free of periodic aliasing.
fn random_line(rng: &mut XorShift32) -> String {
    const ALPHABET: &[u8] =
        b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.,;:-_()[]{}/|+=*%$#@!?";
    let length = rng.next_in(10, 60) as usize;
    let mut line = String::with_capacity(length);
    for _ in 0..length {
        line.push(ALPHABET[rng.next_in(0, ALPHABET.len() as u32) as usize] as char);
    }
    line
}

fn random_lines(count: usize, seed: u32) -> Vec<String> {
    let mut rng = XorShift32::new(seed);
    (0..count).map(|_| random_line(&mut rng)).collect()
}

// ---------------------------------------------------------------------------
// Win32 plumbing
// ---------------------------------------------------------------------------

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

#[derive(Clone, Debug)]
pub(crate) struct WindowInfo {
    pub(crate) hwnd: HWND,
    pub(crate) title: String,
    pub(crate) class: String,
}

fn window_title(hwnd: HWND) -> String {
    let mut buffer = [0u16; 512];
    let length = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    if length <= 0 {
        return String::new();
    }
    from_wide(&buffer[..length as usize])
}

fn window_class(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    let length = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    if length <= 0 {
        return String::new();
    }
    from_wide(&buffer[..length as usize])
}

unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        let out = &mut *(lparam as *mut Vec<WindowInfo>);
        if IsWindowVisible(hwnd) != FALSE {
            out.push(WindowInfo {
                hwnd,
                title: window_title(hwnd),
                class: window_class(hwnd),
            });
        }
    }
    TRUE
}

pub(crate) fn visible_windows() -> Vec<WindowInfo> {
    let mut out: Vec<WindowInfo> = Vec::new();
    unsafe {
        EnumWindows(Some(collect_window), &mut out as *mut _ as LPARAM);
    }
    out
}

/// Poll until a visible top-level window's title contains `needle`.
fn find_window_by_title(needle: &str, timeout: Duration) -> Option<WindowInfo> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(found) = visible_windows()
            .into_iter()
            .find(|w| w.title.contains(needle))
        {
            return Some(found);
        }
        if Instant::now() >= deadline {
            return None;
        }
        pump_for(Duration::from_millis(100));
    }
}

/// The window's client area, in virtual-desktop physical pixels.
fn client_rect_of(hwnd: HWND) -> Option<Rect> {
    let mut client: RECT = unsafe { std::mem::zeroed() };
    if unsafe { GetClientRect(hwnd, &mut client) } == FALSE {
        return None;
    }
    let mut origin = POINT { x: 0, y: 0 };
    if unsafe { ClientToScreen(hwnd, &mut origin) } == FALSE {
        return None;
    }
    Some(Rect::new(
        origin.x,
        origin.y,
        origin.x + client.right,
        origin.y + client.bottom,
    ))
}

/// Descend from `root` to the deepest child window containing `screen`.
///
/// This is the step the reference implementation adds and `Crisp` does not:
/// Chromium's renderer lives in a child window (`Chrome_RenderWidgetHostHWND`),
/// so a message posted to the frame is a message posted to the wrong window.
fn deepest_child_at(root: HWND, screen: Point) -> HWND {
    let mut current = root;
    for _ in 0..16 {
        let mut origin = POINT { x: 0, y: 0 };
        if unsafe { ClientToScreen(current, &mut origin) } == FALSE {
            break;
        }
        let local = POINT {
            x: screen.x - origin.x,
            y: screen.y - origin.y,
        };
        let child = unsafe { ChildWindowFromPointEx(current, local, CWP_SKIPINVISIBLE) };
        if child.is_null() || child == current {
            break;
        }
        current = child;
    }
    current
}

/// Drain this thread's message queue for `duration`.
///
/// Every wait in this file goes through here rather than `sleep`: the Win32
/// fixture is a window owned by this thread, so if we stopped pumping it would
/// stop scrolling — and then the experiment would measure our own neglect.
pub(crate) fn pump_for(duration: Duration) {
    let deadline = Instant::now() + duration;
    let mut message: MSG = unsafe { std::mem::zeroed() };
    while Instant::now() < deadline {
        while unsafe { PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_REMOVE) } != FALSE {
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        std::thread::sleep(Duration::from_millis(4));
    }
}

/// Raise `hwnd` and give it the keyboard focus, which is what `SendInput`
/// needs: a wheel event from the system input queue goes to the focused window.
///
/// `require_focus` is true only for windows this process owns — `SetFocus` is
/// refused for another process's window, and Chromium sets its own focus once
/// it is foreground.
fn bring_to_front(hwnd: HWND, require_focus: bool) {
    unsafe {
        ShowWindow(hwnd, SW_RESTORE);
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        SetForegroundWindow(hwnd);
    }
    pump_for(Duration::from_millis(700));
    if unsafe { GetForegroundWindow() } != hwnd {
        // The foreground lock refuses a plain `SetForegroundWindow` while another process owns
        // the foreground, and it refuses silently — which would turn this arm into a negative
        // answer it never tested. Injecting a key press makes this process the source of the
        // last input, one of the documented conditions that lifts the lock, so the retry after
        // the tap is what the lock accepts. A bare Alt press opens no menu.
        let _ = send_input_keys(VK_MENU, 1);
        pump_for(Duration::from_millis(150));
        unsafe {
            SetForegroundWindow(hwnd);
        }
        pump_for(Duration::from_millis(400));
    }
    if require_focus {
        unsafe {
            SetFocus(hwnd);
        }
    }
    pump_for(Duration::from_millis(300));
    let foreground = unsafe { GetForegroundWindow() };
    let focus = unsafe { GetFocus() };
    eprintln!("[P0.6] foreground={foreground:?} focus={focus:?} wanted={hwnd:?}");
    assert!(
        foreground == hwnd,
        "could not bring the fixture window to the foreground (got {foreground:?}, wanted {hwnd:?}). \
         This is an environment failure, not a scroll result — the probe refuses to report a \
         negative answer it did not actually test."
    );
    if require_focus {
        assert!(
            focus == hwnd,
            "the fixture window is foreground but some other window owns the focus ({focus:?}), so \
             an injected wheel would be delivered there. SetFocus is the only way to make the \
             SendInput arm mean anything."
        );
    }
}

/// Prove that this process can inject input at all.
///
/// A wheel notch that never arrives and a process that cannot inject look
/// identical in the result table, and they call for opposite responses: one is a
/// finding about `WM_MOUSEWHEEL`, the other is a broken environment. A relative
/// mouse move is the cheapest unambiguous probe.
fn send_input_liveness() -> Result<i32, String> {
    let mut before = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut before) } == FALSE {
        return Err("GetCursorPos failed before the liveness check".into());
    }
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 40,
                dy: 0,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let sent = unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
    pump_for(Duration::from_millis(120));
    let mut after = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut after) } == FALSE {
        return Err("GetCursorPos failed after the liveness check".into());
    }
    if sent != 1 {
        return Err(format!("SendInput reported {sent} of 1 events"));
    }
    Ok(after.x - before.x)
}

/// Does this window scroll at all, and does our measurement see it?
///
/// Sending `WM_VSCROLL` directly bypasses the wheel entirely, so a positive
/// result separates "the fixture cannot scroll" from "the wheel did not arrive".
fn vscroll_self_check(hwnd: HWND, client: Rect, steps: i32) -> Result<i32, String> {
    let before = capture_settled(client, Duration::from_secs(2))?;
    for _ in 0..steps {
        unsafe {
            SendMessageW(hwnd, WM_VSCROLL, SB_LINEDOWN as usize, 0);
        }
    }
    pump_for(Duration::from_millis(250));
    let after = capture_settled(client, Duration::from_secs(2))?;
    Ok(estimate_shift(&before, &after).shift)
}

// ---------------------------------------------------------------------------
// The observation window: a Win32 scroller that reports what it receives
// ---------------------------------------------------------------------------

/// Rows of content drawn by the probe's own window. The pattern is a
/// pseudo-random on/off stripe per content line, so the row series is a random
/// binary sequence: no periodic aliasing, and a shift of exactly one line is
/// never mistaken for no movement.
const WHEEL_FIXTURE_LINE_HEIGHT: i32 = 3;
const WHEEL_FIXTURE_LINES_PER_NOTCH: i32 = 5;
const WHEEL_FIXTURE_CONTENT_LINES: i32 = 4000;

static WHEEL_FIXTURE_OFFSET: AtomicI32 = AtomicI32::new(0);
static WHEEL_FIXTURE_WHEELS: AtomicU64 = AtomicU64::new(0);

fn wheel_fixture_dark(content_line: i32) -> bool {
    let mut x = (content_line as u32).wrapping_mul(0x9E37_79B9);
    x ^= x >> 15;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x & 0x100 != 0
}

fn wheel_fixture_scroll(delta: i32) {
    let limit = WHEEL_FIXTURE_CONTENT_LINES * WHEEL_FIXTURE_LINE_HEIGHT;
    let next = (WHEEL_FIXTURE_OFFSET.load(Ordering::Relaxed) + delta).clamp(0, limit);
    WHEEL_FIXTURE_OFFSET.store(next, Ordering::Relaxed);
}

unsafe fn wheel_fixture_paint(hwnd: HWND) -> LRESULT {
    unsafe {
        let mut paint: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(hwnd, &mut paint);
        let mut client: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut client);
        let dark = CreateSolidBrush(0x0020_2020u32);
        let light = CreateSolidBrush(0x00F0_F0F0u32);
        let offset = WHEEL_FIXTURE_OFFSET.load(Ordering::Relaxed);
        let mut y = 0;
        while y < client.bottom {
            let line = (offset + y) / WHEEL_FIXTURE_LINE_HEIGHT;
            let mut band: RECT = std::mem::zeroed();
            band.left = 0;
            band.right = client.right;
            band.top = y;
            band.bottom = (y + WHEEL_FIXTURE_LINE_HEIGHT).min(client.bottom);
            FillRect(
                dc,
                &band,
                if wheel_fixture_dark(line) {
                    dark
                } else {
                    light
                },
            );
            y += WHEEL_FIXTURE_LINE_HEIGHT;
        }
        DeleteObject(dark);
        DeleteObject(light);
        EndPaint(hwnd, &paint);
    }
    0
}

unsafe extern "system" fn wheel_fixture_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_MOUSEWHEEL => {
            WHEEL_FIXTURE_WHEELS.fetch_add(1, Ordering::Relaxed);
            let delta = ((wparam >> 16) as u16) as i16 as i32;
            let notches = delta / WHEEL_DELTA;
            wheel_fixture_scroll(
                -notches * WHEEL_FIXTURE_LINES_PER_NOTCH * WHEEL_FIXTURE_LINE_HEIGHT,
            );
            unsafe { InvalidateRect(hwnd, ptr::null(), FALSE) };
            0
        }
        WM_VSCROLL => {
            if (wparam & 0xFFFF) as i32 == SB_LINEDOWN {
                wheel_fixture_scroll(WHEEL_FIXTURE_LINES_PER_NOTCH * WHEEL_FIXTURE_LINE_HEIGHT);
                unsafe { InvalidateRect(hwnd, ptr::null(), FALSE) };
            }
            0
        }
        WM_PAINT => unsafe { wheel_fixture_paint(hwnd) },
        WM_ERASEBKGND => 1,
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// A window whose only purpose is to say whether `WM_MOUSEWHEEL` arrived.
///
/// `PostMessageW` returning `TRUE` only means the message was queued; it does
/// not mean a window consumed it. Counting arrivals in the window procedure is
/// the only way to tell "the target ignored the message" from "the message
/// never got there".
struct WheelFixture {
    hwnd: HWND,
}

impl WheelFixture {
    fn create(rect: Rect) -> Result<Self, String> {
        let class = wide("SnapclipScrollProbeFixture");
        let title = wide("snapclip scroll probe fixture");
        // Leaked on purpose: a window class keeps the *pointer* it was given,
        // not a copy, so the name must outlive every window of the class. It is
        // leaked before the registration call so the address cannot move.
        let class = Box::leak(class.into_boxed_slice());
        unsafe {
            RegisterClassW(&WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(wheel_fixture_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: ptr::null_mut(),
                hIcon: ptr::null_mut(),
                hCursor: ptr::null_mut(),
                hbrBackground: ptr::null_mut(),
                lpszMenuName: ptr::null(),
                lpszClassName: class.as_ptr(),
            });
        }
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_VSCROLL | WS_VISIBLE,
                rect.left,
                rect.top,
                rect.width(),
                rect.height(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err("CreateWindowExW(probe fixture) returned null".into());
        }
        Ok(Self { hwnd })
    }

    fn client_rect(&self) -> Result<Rect, String> {
        client_rect_of(self.hwnd).ok_or_else(|| "the probe fixture has no client rect".to_string())
    }

    fn offset(&self) -> i32 {
        WHEEL_FIXTURE_OFFSET.load(Ordering::Relaxed)
    }

    fn wheels_received(&self) -> u64 {
        WHEEL_FIXTURE_WHEELS.load(Ordering::Relaxed)
    }
}

impl Drop for WheelFixture {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}

// ---------------------------------------------------------------------------
// Capture and measurement
// ---------------------------------------------------------------------------

/// One "ink" value per row of the captured client area.
fn row_signature(bitmap: &bitblt::CapturedBitmap) -> Vec<f64> {
    let stride = bitmap.width as usize;
    let usable = stride.saturating_sub(SCROLLBAR_TRIM as usize).max(1);
    let scale = 1.0 / (usable as f64 * 3.0 * 255.0);
    (0..bitmap.height as usize)
        .map(|y| {
            let row = &bitmap.pixels[y * stride * 4..(y + 1) * stride * 4];
            let mut sum = 0u64;
            for x in 0..usable {
                let pixel = &row[x * 4..x * 4 + 4];
                sum += pixel[0] as u64 + pixel[1] as u64 + pixel[2] as u64;
            }
            sum as f64 * scale
        })
        .collect()
}

/// Below this, a captured client area has no row structure to track: a blank page, a page
/// already scrolled to its end, an empty editor. Movement cannot be observed there at all,
/// so a zero in that row says nothing about whether the wheel arrived.
const FLAT_SIGNATURE_VARIANCE: f64 = 1e-5;

/// The smallest client area an arm is allowed to aim at.
///
/// A window smaller than this is not a scrolling surface a user would point at: Windows
/// parks minimized windows off-screen instead of resizing them, so a tiny client area is
/// how a helper or a shadow window shows up in a window enumeration.
const MIN_USABLE_CLIENT_AREA: i64 = 300 * 300;

/// The client area of a window, or `None` if it has none.
fn client_area(hwnd: HWND) -> Option<i64> {
    let client = client_rect_of(hwnd)?;
    Some(i64::from(client.width().max(0)) * i64::from(client.height().max(0)))
}

/// What the arm is about to aim at, printed before it runs.
///
/// Every zero in the matrix has two readings — the wheel never arrived, or there was
/// nothing to scroll — and only the target's own state can separate them. This is the
/// state: who the window is, where the wheel will land, and whether the captured area has
/// any structure to move.
fn describe_scroll_target(label: &'static str, root: HWND, client: Rect) -> Result<f64, String> {
    let center = Point::new(
        (client.left + client.right) / 2,
        (client.top + client.bottom) / 2,
    );
    let deepest = deepest_child_at(root, center);
    let variance = signature_variance(&capture_signature(client)?);
    let flat = if variance < FLAT_SIGNATURE_VARIANCE {
        " FLAT (nothing to scroll here)"
    } else {
        ""
    };
    let owner = process_image_path(root)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<unknown process>".to_string());
    eprintln!(
        "[P0.09] {label}: title {:?} class {} owned by {owner} child {} variance {variance:.8}{flat} client {client:?}",
        window_title(root),
        window_class(root),
        window_class(deepest),
    );
    let under_cursor = unsafe { WindowFromPoint(POINT { x: center.x, y: center.y }) };
    eprintln!(
        "[P0.09] {label}: WindowFromPoint({}, {}) is {under_cursor:?} class {}",
        center.x,
        center.y,
        window_class(under_cursor),
    );
    Ok(variance)
}

/// Where the page itself says it is scrolled to, read from its window title.
///
/// The fixture keeps a ` y=<scrollY>` suffix on the title it is given, so the probes get a
/// measurement channel that never touches pixels. It exists because a captured area can be
/// flat — blank, at the end of the document — while the page is scrolling perfectly well,
/// and because a client area can be smaller than the window's landing point. A zero in the
/// capture column and a zero here mean different things.
fn scroll_y_from_title(title: &str) -> Option<i32> {
    let (_, value) = title.rsplit_once(" y=")?;
    value.trim().parse().ok()
}

fn page_scroll_y(hwnd: HWND) -> Option<i32> {
    scroll_y_from_title(&window_title(hwnd))
}

/// Print what the page says about its own position, before and after an arm.
///
/// Silent for windows that do not report one, so the log only grows for the targets whose
/// answer can be cross-checked.
fn report_page_delta(label: &str, transport: Transport, before: Option<i32>, after: Option<i32>) {
    if before.is_none() && after.is_none() {
        return;
    }
    match (before, after) {
        (Some(before), Some(after)) => eprintln!(
            "[P0.09] {label} {transport:?}: the page says y {before} -> {after} (moved {})",
            after - before
        ),
        _ => eprintln!("[P0.09] {label} {transport:?}: the page reports y {before:?} -> {after:?}"),
    }
}

fn capture_signature(client: Rect) -> Result<Vec<f64>, String> {
    let bitmap = bitblt::capture_rect(client)?;
    Ok(row_signature(&bitmap))
}

/// How much a signature varies between its rows.
///
/// A zero movement row in the matrix has two readings that the movement number alone cannot
/// tell apart: the wheel never reached the scrolling surface, or the surface had nothing to
/// scroll. A flat region — a blank page, a page scrolled to its end, an empty editor — is
/// the second reading, and it is flat whatever the wheel does, so its variance is what
/// decides whether a zero is evidence about injection at all.
fn signature_variance(signature: &[f64]) -> f64 {
    if signature.is_empty() {
        return 0.0;
    }
    let count = signature.len() as f64;
    let mean = signature.iter().sum::<f64>() / count;
    signature
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / count
}

/// Capture until two consecutive frames agree, so that Chromium's smooth
/// scrolling animation has finished before we measure where it stopped.
fn capture_settled(client: Rect, timeout: Duration) -> Result<Vec<f64>, String> {
    let deadline = Instant::now() + timeout;
    let mut last = capture_signature(client)?;
    loop {
        pump_for(Duration::from_millis(150));
        let next = capture_signature(client)?;
        if zncc(&last, &next) > 0.9995 || Instant::now() >= deadline {
            return Ok(next);
        }
        last = next;
    }
}

// ---------------------------------------------------------------------------
// Injection
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Transport {
    /// `SendInput`: goes through the system input queue to the focused window.
    SendInput,
    /// `PostMessageW` to the deepest child under the cursor.
    PostMessage,
    /// `SendInput` of an arrow key. A control experiment for Chromium: it is
    /// injected through the same queue but arrives as a keystroke, so it tells
    /// "Chromium ignores injected input" apart from "Chromium scrolled and we
    /// failed to measure it".
    ArrowDown,
    /// The **product** path: `scroll_actuator::inject` with
    /// [`InjectPath::SendInput`]. `P3.01` requires the matrix to be reproducible
    /// through the shipped code, not only through the probe's own copy of it.
    ProductSendInput,
    /// The product path with [`InjectPath::PostMessageW`], including the child
    /// sink that the probe's own `post_message_notches` also does.
    ProductPost,
}

impl Transport {
    /// Whether this arm drives the shipped actuator rather than the probe's own
    /// copy of the wire format.
    fn is_product(self) -> bool {
        matches!(self, Transport::ProductSendInput | Transport::ProductPost)
    }

    fn label(self) -> &'static str {
        match self {
            Transport::SendInput => "SendInput",
            Transport::PostMessage => "PostMessageW",
            Transport::ArrowDown => "SendInput(down-key)",            Transport::ProductSendInput => "product/SendInput",
            Transport::ProductPost => "product/PostMessageW",
        }
    }
}

/// Which coordinate space goes into the posted message's `lParam`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CoordSpace {
    /// Client coordinates of the window the message is posted to — what
    /// `snow_shot`'s `scrollinput.cpp` does.
    Client,
    /// Screen coordinates — what the `WM_MOUSEWHEEL` documentation says.
    /// Reported as a diagnostic; the asserted arms use `Client`.
    Screen,
}

impl CoordSpace {
    fn label(self) -> &'static str {
        match self {
            CoordSpace::Client => "client",
            CoordSpace::Screen => "screen",
        }
    }
}

/// Fire one wheel notch through the system input queue.
fn send_input_notches(notches: i32) -> Result<u32, String> {
    let mut delivered = 0;
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: (-WHEEL_DELTA) as u32,
                dwFlags: MOUSEEVENTF_WHEEL,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    for _ in 0..notches {
        let sent = unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
        if sent != 1 {
            return Err(format!(
                "SendInput delivered {sent} of 1 events (Win32 error {}) — blocked, most likely \
                 because the foreground window is at a higher integrity level than this process",
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
            ));
        }
        delivered += sent;
        pump_for(Duration::from_millis(30));
    }
    Ok(delivered)
}

fn make_lparam(x: i32, y: i32) -> isize {
    (((y as u32 & 0xFFFF) << 16) | (x as u32 & 0xFFFF)) as isize
}

/// Fire a key through the system input queue, as the control experiment.
///
/// Chromium may ignore a posted wheel because it never saw a real input event;
/// a keystroke from the same queue reaches whatever has the focus, so if this
/// scrolls the page the injection path is intact and the wheel result is about
/// wheels, not about input in general.
fn send_input_keys(virtual_key: u16, presses: i32) -> Result<u32, String> {
    let mut delivered = 0;
    for _ in 0..presses {
        for flags in [0, KEYEVENTF_KEYUP] {
            let input = INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: virtual_key,
                        wScan: 0,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            let sent = unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
            if sent != 1 {
                return Err(format!(
                    "SendInput delivered {sent} of 1 key events (Win32 error {})",
                    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
                ));
            }
            delivered += sent;
            pump_for(Duration::from_millis(20));
        }
    }
    Ok(delivered)
}

/// Fire wheel notches at a window by posting messages to the deepest child that
/// contains `screen`.
fn post_message_notches(
    root: HWND,
    screen: Point,
    space: CoordSpace,
    notches: i32,
) -> Result<u32, String> {
    let target = deepest_child_at(root, screen);
    if target.is_null() {
        return Err("ChildWindowFromPointEx descended to a null window".into());
    }
    let mut origin = POINT { x: 0, y: 0 };
    if unsafe { ClientToScreen(target, &mut origin) } == FALSE {
        return Err("ClientToScreen failed for the deepest child".into());
    }
    let (x, y) = match space {
        CoordSpace::Client => (screen.x - origin.x, screen.y - origin.y),
        CoordSpace::Screen => (screen.x, screen.y),
    };
    let lparam = make_lparam(x, y);
    let wparam: WPARAM = (((-WHEEL_DELTA) as u32 & 0xFFFF) << 16) as usize;

    let mut delivered = 0;
    for _ in 0..notches {
        let posted = unsafe { PostMessageW(target, WM_MOUSEWHEEL, wparam, lparam) };
        if posted == FALSE {
            return Err(format!(
                "PostMessageW(WM_MOUSEWHEEL) to {target:?} ({}) was refused (Win32 error {}); \
                 a post has no queue slot to fail on, so this is an access or validity failure",
                window_class(target),
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
            ));
        }
        delivered += 1;
        pump_for(Duration::from_millis(30));
    }
    Ok(delivered)
}

// ---------------------------------------------------------------------------
// Fixture 1: a top-level EDIT control we own
// ---------------------------------------------------------------------------

struct EditFixture {
    hwnd: HWND,
}

impl EditFixture {
    fn create(lines: &[String], rect: Rect) -> Result<Self, String> {
        let class = wide("EDIT");
        let empty = wide("");
        let style = WS_OVERLAPPEDWINDOW
            | WS_VSCROLL
            | WS_VISIBLE
            | ES_MULTILINE as u32
            | ES_AUTOVSCROLL as u32;
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                empty.as_ptr(),
                style,
                rect.left,
                rect.top,
                rect.width(),
                rect.height(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err("CreateWindowExW(EDIT) returned null".into());
        }
        let body = lines.join("\r\n");
        let body = wide(&body);
        unsafe {
            SendMessageW(hwnd, 0x000C, 0, body.as_ptr() as isize); // WM_SETTEXT
        }
        Ok(Self { hwnd })
    }

    fn client_rect(&self) -> Result<Rect, String> {
        client_rect_of(self.hwnd).ok_or_else(|| "the EDIT fixture has no client rect".to_string())
    }

    /// The control's own account of its scroll position, in lines.
    fn first_visible_line(&self) -> i32 {
        unsafe { SendMessageW(self.hwnd, EM_GETFIRSTVISIBLELINE, 0, 0) as i32 }
    }
}

impl Drop for EditFixture {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}

// ---------------------------------------------------------------------------
// Fixture 2: Chromium
// ---------------------------------------------------------------------------

/// Chrome specifically. `find_chromium` only asked for "any Chromium", which was
/// enough for the injection probe and is not enough for `P0.05`/`P0.09`: those
/// compare Chrome against Edge, so the two have to be findable apart.
fn find_chrome() -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os("CHROME_PATH") {
        let path = PathBuf::from(configured);
        if path.is_file() {
            return Some(path);
        }
    }
    [
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
}

/// Edge specifically (the capture probe needs it as a target of its own).
fn find_edge() -> Option<PathBuf> {
    [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
}

fn find_chromium() -> Option<PathBuf> {
    find_chrome().or_else(find_edge)
}

/// The placeholder in `tests/fixtures/scroll-demo.html` that each run replaces with
/// its own title.
const DEMO_TITLE_TOKEN: &str = "snapclip-scroll-demo-token";

/// The demo's random-height texture region, used as the page's landing anchor.
///
/// The transport is measured *here* on purpose. The demo also contains a text region,
/// and a text region is a 19 px carrier: its row signature has many near-equal peaks,
/// so on it a "no movement" verdict says more about the estimator than about the
/// injection. That case is real and it has to be measured — it is `E-ACC-1`'s job
/// (`docs/30` §15.4: the row fingerprint produces candidates and never decides) — but
/// it must not be the thing this probe calls "the wheel did not arrive".
const DEMO_TEXTURE_ANCHOR: &str = "#rows-section";

/// The Chromium fixture: the repository's demo page when it is present, otherwise a
/// generated stack of randomly sized bars.
///
/// The demo (`crates/snapclip-capture/tests/fixtures/scroll-demo.html`) is preferred
/// deliberately: a probe that only ever meets a fixture it generated itself measures
/// the fixture, not the browser. The demo carries a sticky header, a text block, a
/// period-free texture region, lazy images, a CSS animation and a finite
/// infinite-scroll sentinel — the phenomena `docs/30` §18 and §25 have to survive.
///
/// The fallback keeps a stripped checkout working, and it encodes the one property
/// the shift measurement depends on:
///
/// Text would be more realistic and much worse to measure: a text line is
/// ~13 px of glyphs followed by ~6 px of leading, so the row signal carries a
/// strong carrier at the line height, and a large scroll then reports a
/// confident *spurious* peak on that carrier instead of the true displacement.
/// Bars with random heights (4–40 px) remove the carrier: the row series is a
/// random step function, and a shift of any size is either the truth or
/// nothing.
fn write_chromium_fixture(dir: &Path, title: &str) -> Result<PathBuf, String> {
    let demo = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scroll-demo.html");
    if let Ok(html) = std::fs::read_to_string(&demo) {
        let path = dir.join("fixture.html");
        std::fs::write(&path, html.replace(DEMO_TITLE_TOKEN, title))
            .map_err(|error| format!("writing {path:?} failed: {error}"))?;
        return Ok(path);
    }
    let mut state = XorShift32::new(0x51AB_1E5D);
    let mut bars = String::new();
    let mut total = 0;
    while total < 20_000 {
        let height = 4 + (state.next_u32() % 37) as i32;
        let color = if state.next_u32() & 0x100 != 0 {
            "#101418"
        } else {
            "#eef2f6"
        };
        bars.push_str(&format!(
            "<div style=\"height:{height}px;background:{color}\"></div>"
        ));
        total += height;
    }
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <title>{title}</title>\
         <style>html,body{{margin:0;padding:0;background:#fff}}\
         div{{margin:0;padding:0}}</style>\
         </head><body>{bars}</body></html>"
    );
    let path = dir.join("fixture.html");
    std::fs::write(&path, html).map_err(|error| format!("writing {path:?} failed: {error}"))?;
    Ok(path)
}

/// The document the Notepad arm opens.
///
/// An empty Notepad is a menu bar and a caret, so ten frames of it would measure the
/// window frame rather than a document. The search order is: `P0.05_PROBE_TEXT`, then
/// `docs/Temp/p0-05-notepad.txt` (the git-ignored scratch copy of the long document
/// this probe is usually asked to use), then a generated filler — a tall document is
/// the requirement, and failing over a particular file would be a probe that measures
/// its own configuration instead of the capture path.
fn notepad_document(scratch: &Path) -> (PathBuf, String) {
    if let Some(explicit) = std::env::var_os("P0.05_PROBE_TEXT") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return (path.clone(), format!("{} (from P0.05_PROBE_TEXT)", path.display()));
        }
    }
    let scratch_copy =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(r"..\..\docs\Temp\p0-05-notepad.txt");
    if scratch_copy.is_file() {
        return (
            scratch_copy.clone(),
            format!("{} (scratch document)", scratch_copy.display()),
        );
    }

    let path = scratch.join("notepad-document.txt");
    let mut text = String::new();
    for line in 0..800 {
        text.push_str(&format!(
            "line {line:04} — the window-level capture path is measured on a document, \
             not on an empty editor\n"
        ));
    }
    match std::fs::write(&path, text) {
        Ok(()) => (path, "generated filler, 800 lines".to_string()),
        Err(error) => (path.clone(), format!("{} (filler could not be written: {error})", path.display())),
    }
}

fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    format!("file:///{}", text.trim_start_matches('/'))
}

/// Launch Chromium on a throwaway profile.
///
/// The profile directory is created fresh every run: Chromium restores the
/// previous session otherwise and silently ignores `--window-size`, which would
/// make the fixture's geometry differ from the one we measured.
fn launch_chromium(executable: &Path, url: &str, profile: &Path) -> Result<Child, String> {
    let _ = std::fs::remove_dir_all(profile);
    std::fs::create_dir_all(profile)
        .map_err(|error| format!("creating the Chromium profile dir failed: {error}"))?;
    if !executable.is_file() {
        return Err(format!("no Chromium build at {executable:?}"));
    }
    std::process::Command::new(executable)
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-session-crashed-bubble")
        .arg("--disable-features=Translate,TranslateUI,InfiniteSessionRestore")
        .arg("--disable-background-networking")
        .arg("--disable-popup-blocking")
        // The probe measures device pixels; a scale factor would make the
        // injected notches and the measured rows disagree.
        .arg("--force-device-scale-factor=1")
        .arg("--window-size=1200,900")
        .arg("--window-position=60,40")
        .arg(format!("--app={url}"))
        .spawn()
        .map_err(|error| format!("launching {executable:?} failed: {error}"))
}

fn kill_process_tree(child: &mut Child) {
    let pid = child.id();
    let _ = std::process::Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}

// ---------------------------------------------------------------------------
// The experiment
// ---------------------------------------------------------------------------

struct Outcome {
    target: &'static str,
    transport: &'static str,
    space: &'static str,
    delivered: u32,
    line_delta: Option<i32>,
    measured_px: i32,
    correlation: f64,
    runner_up: f64,
    zero_correlation: f64,
    error: Option<String>,
}

impl Outcome {
    fn moved(&self) -> bool {
        self.error.is_none() && self.measured_px >= MIN_PROVEN_SHIFT
    }

    fn verdict(&self) -> &'static str {
        if self.error.is_some() {
            "ERROR"
        } else if self.measured_px >= MIN_PROVEN_SHIFT {
            "SCROLLED"
        } else if self.measured_px > 0 {
            "below threshold"
        } else {
            "NO MOVEMENT"
        }
    }
}

/// Whether this arm is responsible for placing the cursor before it injects a wheel.
///
/// Normally it is: under `MOUSE_POS` routing the wheel goes to whatever window is under the
/// cursor, so an arm that means to measure one window has to put the cursor over it first. A
/// lower-integrity sender cannot do that — `SetCursorPos` fails and leaves the last error at 0
/// — so for the integrity arm the operator places the cursor with a higher-integrity helper
/// and sets `SNAPCLIP_UIPI_NO_AIM`, which leaves only the injection step under test. Without
/// that split the arm reports "the cursor could not be placed" and says nothing about whether
/// the wheel itself would have been delivered.
fn arm_places_the_cursor() -> bool {
    std::env::var_os("SNAPCLIP_UIPI_NO_AIM").is_none()
}

/// Run one arm: settle, capture, inject, settle, measure.
fn run_arm(
    target: &'static str,
    transport: Transport,
    space: CoordSpace,
    root: HWND,
    client: Rect,
    notches: i32,
    line_probe: Option<&dyn Fn() -> i32>,
) -> Outcome {
    let mut outcome = Outcome {
        target,
        transport: transport.label(),
        space: space.label(),
        delivered: 0,
        line_delta: None,
        measured_px: 0,
        correlation: 0.0,
        runner_up: 0.0,
        zero_correlation: 0.0,
        error: None,
    };

    // Capture the *window handle* the message will go to before foregrounding,
    // so the reported class is the one we actually talk to.
    let before = match capture_settled(client, Duration::from_secs(3)) {
        Ok(signature) => signature,
        Err(error) => {
            outcome.error = Some(format!("pre-injection capture failed: {error}"));
            return outcome;
        }
    };
    let line_before = line_probe.map(|probe| probe());

    let injection = match transport {
        Transport::SendInput => {
            let center = Point::new(
                (client.left + client.right) / 2,
                (client.top + client.bottom) / 2,
            );
            if arm_places_the_cursor() && unsafe { SetCursorPos(center.x, center.y) } == FALSE {
                outcome.error = Some(format!(
                    "SetCursorPos to ({}, {}) failed (Win32 error {})",
                    center.x,
                    center.y,
                    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
                ));
                return outcome;
            }
            // Under `MOUSE_POS` routing the wheel goes wherever the cursor is, which is not
            // necessarily the window this arm means to measure: a topmost fixture, another
            // browser at the same coordinates, or a separate input window of the same
            // application. Printing the aim is what separates "the target ignored the wheel"
            // from "the wheel was never aimed at the target".
            let aimed = unsafe { WindowFromPoint(POINT { x: center.x, y: center.y }) };
            eprintln!(
                "[P0.6] {target}: the wheel is aimed at {aimed:?} class {} (target {root:?})",
                window_class(aimed)
            );
            send_input_notches(notches)
        }
        Transport::PostMessage => {
            let center = Point::new(
                (client.left + client.right) / 2,
                (client.top + client.bottom) / 2,
            );
            let deepest = deepest_child_at(root, center);
            eprintln!(
                "[P0.6] {target}: deepest child under the cursor is {:?} class={}",
                deepest,
                window_class(deepest)
            );
            post_message_notches(root, center, space, notches)
        }
        Transport::ArrowDown => Err("the arrow-key control runs stepwise only".into()),
        Transport::ProductSendInput | Transport::ProductPost => {
            Err("the product path runs stepwise only".into())
        }
    };
    match injection {
        Ok(delivered) => outcome.delivered = delivered,
        Err(error) => {
            outcome.error = Some(error);
            return outcome;
        }
    }

    let after = match capture_settled(client, Duration::from_secs(4)) {
        Ok(signature) => signature,
        Err(error) => {
            outcome.error = Some(format!("post-injection capture failed: {error}"));
            return outcome;
        }
    };
    if let Some(probe) = line_probe {
        outcome.line_delta = Some(probe() - line_before.unwrap_or(0));
    }

    let estimate = estimate_shift(&before, &after);
    outcome.measured_px = estimate.shift;
    outcome.correlation = estimate.correlation;
    outcome.runner_up = estimate.runner_up;
    outcome.zero_correlation = estimate.zero_correlation;
    outcome
}

/// Inject one step at a time and measure each step.
///
/// A burst can move the page further than one viewport, and the row estimator
/// can only see displacements smaller than the captured height — a burst would
/// look exactly like "nothing happened". One observation per injected step is
/// also what the product's per-frame displacement estimate does, so the sum is
/// a measurement of the mechanism the design depends on, not of a shortcut.
/// One notch through the **shipped** actuator (`P3.01` exit condition 2).
///
/// The probe's own `send_input_notches`/`post_message_notches` stay, because the
/// numbers they produced are recorded in `docs/30` §24.6.1 and a measurement is
/// not reproducible by swapping the instrument under it. This arm exists so the
/// same matrix can *also* be produced by the code that ships — which is the only
/// way to know the shipped code is the one that was measured.
fn deliver_via_product(root: HWND, screen: Point, path: InjectPath) -> Result<u32, String> {
    let request = InjectRequest {
        target: root as isize,
        screen,
        notches: 1,
        axis: Axis::Vertical,
        path,
        aim: if arm_places_the_cursor() {
            Aim::PlaceCursor
        } else {
            // A low-integrity sender cannot `SetCursorPos`; the operator aims.
            Aim::AssumePlaced
        },
    };
    let outcome = scroll_actuator::inject(&scroll_actuator::Win32Injection, &request);
    match outcome.status {
        InjectStatus::Posted => {
            if let Some(target_window) = outcome.target_window {
                if target_window != root as isize {
                    eprintln!(
                        "[P3.01] the product path descended to {target_window:?} class={} (root {root:?})",
                        window_class(target_window as HWND)
                    );
                }
            }
            Ok(outcome.delivered)
        }
        status => Err(format!(
            "the product path refused the request: {status:?} (target_window {:?})",
            outcome.target_window
        )),
    }
}

fn run_arm_stepwise(
    target: &'static str,
    transport: Transport,
    space: CoordSpace,
    root: HWND,
    client: Rect,
    steps: i32,
    line_probe: Option<&dyn Fn() -> i32>,
) -> Outcome {
    let mut outcome = Outcome {
        target,
        transport: transport.label(),
        space: space.label(),
        delivered: 0,
        line_delta: None,
        measured_px: 0,
        correlation: 1.0,
        runner_up: 0.0,
        zero_correlation: 0.0,
        error: None,
    };
    let line_before = line_probe.map(|probe| probe());
    let mut previous = match capture_settled(client, Duration::from_secs(3)) {
        Ok(signature) => signature,
        Err(error) => {
            outcome.error = Some(format!("pre-injection capture failed: {error}"));
            return outcome;
        }
    };

    let center = Point::new(
        (client.left + client.right) / 2,
        (client.top + client.bottom) / 2,
    );
    let mut total = 0;
    for step in 0..steps {
        let delivery = match transport {
            Transport::SendInput => {
                if arm_places_the_cursor() && unsafe { SetCursorPos(center.x, center.y) } == FALSE {
                    outcome.error = Some(format!(
                    "SetCursorPos to ({}, {}) failed (Win32 error {})",
                    center.x,
                    center.y,
                    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
                ));
                    return outcome;
                }
                if step == 0 {
                    // Under `MOUSE_POS` routing the wheel goes wherever the cursor is, which
                    // is not necessarily the window this arm means to measure. Printing the
                    // aim separates "the target ignored the wheel" from "the wheel was never
                    // aimed at the target".
                    let aimed = unsafe { WindowFromPoint(POINT { x: center.x, y: center.y }) };
                    eprintln!(
                        "[P0.6] {target}: the wheel is aimed at {aimed:?} class {} (target {root:?})",
                        window_class(aimed)
                    );
                }
                send_input_notches(1)
            }
            Transport::ArrowDown => send_input_keys(VK_DOWN, 1),
            Transport::PostMessage => post_message_notches(root, center, space, 1),
            Transport::ProductSendInput => deliver_via_product(root, center, InjectPath::SendInput),
            Transport::ProductPost => deliver_via_product(root, center, InjectPath::PostMessageW),
        };
        match delivery {
            Ok(sent) => outcome.delivered += sent,
            Err(error) => {
                outcome.error = Some(error);
                return outcome;
            }
        }

        let current = match capture_settled(client, Duration::from_secs(4)) {
            Ok(signature) => signature,
            Err(error) => {
                outcome.error = Some(format!("capture after step {step} failed: {error}"));
                return outcome;
            }
        };
        let estimate = estimate_shift(&previous, &current);
        if step == 0 {
            outcome.zero_correlation = estimate.zero_correlation;
        }
        total += estimate.shift;
        outcome.correlation = outcome.correlation.min(estimate.correlation);
        outcome.runner_up = outcome.runner_up.max(estimate.runner_up);
        previous = current;
    }

    outcome.measured_px = total;
    if let Some(probe) = line_probe {
        outcome.line_delta = Some(probe() - line_before.unwrap_or(0));
    }
    outcome
}

fn print_table(outcomes: &[Outcome]) {
    eprintln!();
    eprintln!(
        "{:<10} {:<14} {:<8} {:>6} {:>8} {:>7} {:>9} {:>7} {:>8}  {}",
        "target",
        "transport",
        "coords",
        "sent",
        "shift_px",
        "corr",
        "runner_up",
        "corr@0",
        "line_d",
        "verdict"
    );
    for outcome in outcomes {
        let line_delta = outcome
            .line_delta
            .map(|delta| delta.to_string())
            .unwrap_or_else(|| "-".into());
        eprintln!(
            "{:<10} {:<14} {:<8} {:>6} {:>8} {:>7.3} {:>9.3} {:>7.3} {:>8}  {}{}",
            outcome.target,
            outcome.transport,
            outcome.space,
            outcome.delivered,
            outcome.measured_px,
            outcome.correlation,
            outcome.runner_up,
            outcome.zero_correlation,
            line_delta,
            outcome.verdict(),
            outcome
                .error
                .as_ref()
                .map(|error| format!(" — {error}"))
                .unwrap_or_default(),
        );
    }
    eprintln!();
}

/// `P0.6` / `E-INJECT-1`, the whole matrix.
///
/// Ignored by default: it opens windows, steals focus, and needs Chromium.
#[test]
#[ignore = "P0.6: needs a real interactive desktop and a Chromium build"]
fn inject_probe() {
    let _ = monitor::set_per_monitor_v2_awareness();

    let token = format!(
        "{:08x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    );
    let scratch = std::env::temp_dir().join(format!("snapclip-p06-{token}"));
    std::fs::create_dir_all(&scratch).expect("creating the probe scratch dir");

    let lines = random_lines(1400, 0x5EED_1234);
    let notches = 8;

    let mut outcomes: Vec<Outcome> = Vec::new();
    let mut own_arrivals: Vec<(&'static str, u64)> = Vec::new();

    // --- Step 0: can this process inject input at all?
    let live_delta = match send_input_liveness() {
        Ok(delta) => delta,
        Err(error) => {
            panic!("P0.6 environment failure: the SendInput liveness check failed: {error}")
        }
    };
    eprintln!(
        "[P0.6] SendInput liveness: a relative move of +40 px moved the cursor {live_delta} px"
    );

    // --- Arm 1: a Win32 scroller we own, which also validates the measurement.
    match EditFixture::create(
        &lines,
        Rect::from_origin_size(Point::new(80, 60), 1100, 820),
    ) {
        Ok(fixture) => {
            bring_to_front(fixture.hwnd, true);
            let client = fixture.client_rect().expect("EDIT client rect");
            eprintln!(
                "[P0.6] EDIT fixture: hwnd={:?} client={client:?} first_visible_line={}",
                fixture.hwnd,
                fixture.first_visible_line()
            );
            let probe = fixture.hwnd;
            let line_probe =
                move || unsafe { SendMessageW(probe, EM_GETFIRSTVISIBLELINE, 0, 0) as i32 };
            // Does this window scroll at all, and does the measurement see it?
            // A direct `WM_VSCROLL` bypasses the wheel entirely, so a zero here
            // indicts the fixture or the estimator rather than the transport.
            match vscroll_self_check(fixture.hwnd, client, 8) {
                Ok(shift) => eprintln!(
                    "[P0.6] EDIT self-check: SendMessageW(WM_VSCROLL) moved the content {shift} px"
                ),
                Err(error) => eprintln!("[P0.6] EDIT self-check could not run: {error}"),
            }
            for transport in [Transport::SendInput, Transport::PostMessage] {
                // Re-foreground between arms: the previous arm left the window
                // scrolled, not necessarily on top.
                bring_to_front(fixture.hwnd, true);
                outcomes.push(run_arm(
                    "win32-edit",
                    transport,
                    CoordSpace::Client,
                    fixture.hwnd,
                    client,
                    notches,
                    Some(&line_probe),
                ));
            }
        }
        Err(error) => {
            panic!("the EDIT fixture could not be created: {error}");
        }
    }

    // --- Arm 1b: our own window procedure, which counts the wheel messages it
    // receives. `PostMessageW` returning TRUE only means the message was
    // queued; this is the only target that can tell "the target ignored it"
    // apart from "it never arrived".
    match WheelFixture::create(Rect::from_origin_size(Point::new(200, 140), 1000, 760)) {
        Ok(fixture) => {
            bring_to_front(fixture.hwnd, true);
            let client = fixture.client_rect().expect("probe fixture client rect");
            eprintln!(
                "[P0.6] wheel fixture: hwnd={:?} client={client:?}",
                fixture.hwnd
            );
            let line_probe = || fixture.offset();
            for transport in [Transport::SendInput, Transport::PostMessage] {
                bring_to_front(fixture.hwnd, true);
                let wheels_before = fixture.wheels_received();
                outcomes.push(run_arm(
                    "win32-own",
                    transport,
                    CoordSpace::Client,
                    fixture.hwnd,
                    client,
                    notches,
                    Some(&line_probe),
                ));
                let arrivals = fixture.wheels_received() - wheels_before;
                eprintln!(
                    "[P0.6] win32-own {}: the window procedure received {arrivals} \
                     WM_MOUSEWHEEL messages",
                    transport.label()
                );
                own_arrivals.push((transport.label(), arrivals));
            }
        }
        Err(error) => panic!("the wheel-counting fixture could not be created: {error}"),
    }

    // --- Arm 2: Chromium, the target that decides the question.
    let chromium = find_chromium();
    let chromium_arm = chromium.as_ref().map(|executable| {
        // The fixture's title *is* the handle this arm finds the window by, so it has
        // to be the same string that is searched for below. The demo page carries a
        // placeholder rather than a composed title, so passing the bare token would
        // instead produce a title that never matches.
        let title_token = format!("snapclip-inject-{token}");
        let html = write_chromium_fixture(&scratch, &title_token).expect("writing the fixture");
        let profile = scratch.join("chromium-profile");
        let url = format!("{}{DEMO_TEXTURE_ANCHOR}", file_url(&html));
        let child = launch_chromium(executable, &url, &profile);
        (child, html, title_token)
    });
    match chromium_arm {
        Some((Ok(mut child), html, title_token)) => {
            match find_window_by_title(&title_token, Duration::from_secs(30)) {
                Some(window) => {
                    bring_to_front(window.hwnd, false);
                    let client = client_rect_of(window.hwnd).expect("Chromium client rect");
                    eprintln!(
                        "[P0.6] Chromium fixture: hwnd={:?} class={} client={client:?} html={html:?}",
                        window.hwnd, window.class
                    );
                    for transport in [
                        Transport::SendInput,
                        Transport::PostMessage,
                        Transport::ArrowDown,
                    ] {
                        bring_to_front(window.hwnd, false);
                        outcomes.push(run_arm_stepwise(
                            "chromium",
                            transport,
                            CoordSpace::Client,
                            window.hwnd,
                            client,
                            notches,
                            None,
                        ));
                    }
                    // Diagnostic only: the documented coordinate space.
                    bring_to_front(window.hwnd, false);
                    outcomes.push(run_arm_stepwise(
                        "chromium",
                        Transport::PostMessage,
                        CoordSpace::Screen,
                        window.hwnd,
                        client,
                        notches,
                        None,
                    ));
                }
                None => panic!(
                    "Chromium opened but no window titled {title_token:?} appeared within 30s — \
                     environment failure, not a result"
                ),
            }
            kill_process_tree(&mut child);
        }
        Some((Err(error), _, _)) => panic!("Chromium could not be launched: {error}"),
        None => panic!(
            "no Chromium build found (looked for Chrome then Edge, and $CHROME_PATH). \
             The experiment is meaningless without it."
        ),
    }

    print_table(&outcomes);
    let _ = std::fs::remove_dir_all(&scratch);

    // --- The decision gate.
    //
    // Each step rules out one explanation for the next step's failure, so that a
    // negative answer at the end is about `WM_MOUSEWHEEL` and nothing else.
    let named = |target: &str, transport: Transport| {
        outcomes
            .iter()
            .find(|o| o.target == target && o.transport == transport.label())
    };
    let arrival_count = |transport: Transport| {
        own_arrivals
            .iter()
            .find(|(label, _)| *label == transport.label())
            .map(|(_, count)| *count)
            .expect("the wheel-counting arm ran")
    };

    // 1. Can this process inject input at all? A wheel that never arrives and a
    //    process that cannot inject look identical in the table and call for
    //    opposite responses.
    assert!(
        live_delta != 0,
        "P0.6 environment failure: SendInput(relative mouse move) did not move the cursor at all. \
         This process cannot inject input — most likely a low-level mouse hook in another process \
         (PixPin installs `SetWindowsHookEx` and has filtered synthetic events before) or a \
         non-interactive session. Nothing below would mean anything, so the probe stops here \
         rather than reporting a negative result it did not test."
    );

    // 2. A window we own: did the message arrive at all?
    let own_post = named("win32-own", Transport::PostMessage).expect("arm ran");
    let own_send_input = named("win32-own", Transport::SendInput).expect("arm ran");
    assert!(
        arrival_count(Transport::PostMessage) > 0,
        "P0.6 precondition failed: PostMessageW(WM_MOUSEWHEEL) never reached a window this \
         process owns (0 arrivals, shift {} px). The posting protocol is wrong — check the \
         deepest-child descent and the lParam coordinate space before interpreting Chromium.",
        own_post.measured_px
    );
    assert!(
        arrival_count(Transport::SendInput) > 0,
        "P0.6 precondition failed: SendInput produced no WM_MOUSEWHEEL in a window this process \
         owns (0 arrivals, shift {} px), even though the process can inject input. Something is \
         consuming or redirecting the wheel events.",
        own_send_input.measured_px
    );

    // 3. A real Win32 window we own: did an injected wheel actually scroll it?
    assert!(
        own_post.moved() && own_send_input.moved(),
        "P0.6 precondition failed: an injected wheel reached our own scroller \
         (SendInput {} px, PostMessageW {} px) but did not move it. The measurement or the \
         fixture is broken, so Chrome must not be measured against it.",
        own_send_input.measured_px,
        own_post.measured_px
    );
    eprintln!(
        "[P0.6] protocol verified on our own window procedure: SendInput {} px, PostMessageW {} px",
        own_send_input.measured_px, own_post.measured_px
    );

    // 4. The `EDIT` control is reported, not required: whether a plain edit
    //    control acts on `WM_MOUSEWHEEL` is a finding about that control class,
    //    not a precondition for believing the Chromium arms.
    let edit_send_input = named("win32-edit", Transport::SendInput).expect("arm ran");
    let edit_post = named("win32-edit", Transport::PostMessage).expect("arm ran");
    if !edit_post.moved() {
        eprintln!(
            "[P0.6] note: a plain `EDIT` control did not scroll from an injected wheel \
             (SendInput {} px, PostMessageW {} px, EM_GETFIRSTVISIBLELINE delta {:?}). The \
             transport works — see step 3 — so this is about the control, not the injection.",
            edit_send_input.measured_px, edit_post.measured_px, edit_post.line_delta
        );
    }

    // Then: the decisive cell.
    let chrome_send_input = named("chromium", Transport::SendInput).expect("arm ran");
    let chrome_post = named("chromium", Transport::PostMessage).expect("arm ran");
    let chrome_keys = named("chromium", Transport::ArrowDown).expect("arm ran");
    // The control arm comes first, because it decides how much the two wheel
    // arms below are allowed to mean.
    if chrome_keys.moved() {
        eprintln!(
            "[P0.6] control: SendInput(down-key) scrolled Chromium by {} px, so the fixture \
             scrolls and the window acts on injected input — the wheel arms are measuring the \
             wheel.",
            chrome_keys.measured_px
        );
    } else {
        eprintln!(
            "[P0.6] control: SendInput(down-key) did NOT scroll Chromium either ({} px, \
             correlation {:.3}). Chromium in this environment does not act on injected input at \
             all, so no wheel result here can be attributed to the transport.",
            chrome_keys.measured_px, chrome_keys.correlation
        );
    }
    assert!(
        chrome_send_input.moved(),
        "P0.6 precondition failed: SendInput did not scroll Chromium (shift {} px, correlation \
         {:.3}), and the SendInput(down-key) control moved it {} px. Without a working baseline \
         the PostMessageW arm cannot be interpreted.",
        chrome_send_input.measured_px,
        chrome_send_input.correlation,
        chrome_keys.measured_px
    );
    assert!(
        chrome_post.moved(),
        "P0.6 RESULT: Chromium did not scroll from PostMessageW(WM_MOUSEEWHEEL) (shift {} px, \
         correlation {:.3}, runner_up {:.3}) while SendInput moved it {} px and an injected \
         down-key moved it {} px. docs/30 §24.6's \"two parallel transports\" is then wrong for \
         the core target: re-derive the injection decision (docs/24 §S3.2) — the coherent \
         fallbacks are (a) SendInput only, with PostMessageW demoted from a transport to a \
         defensive extra for elevated targets (docs/30 §24.5), or (b) keep PostMessageW but drop \
         the claim that it is available for Chromium in general. Do not edit the design to match \
         this test on the strength of one machine without re-running it.",
        chrome_post.measured_px,
        chrome_post.correlation,
        chrome_post.runner_up,
        chrome_send_input.measured_px,
        chrome_keys.measured_px
    );
    eprintln!(
        "[P0.6] both transports drove Chromium: docs/30 §24.6 is confirmed by experiment on this \
         machine (SendInput {} px, PostMessageW {} px)",
        chrome_send_input.measured_px, chrome_post.measured_px
    );
}

// ---------------------------------------------------------------------------
// Fixture self-validation (runs in CI, no desktop)
// ---------------------------------------------------------------------------

/// The measurement must recover a shift it was told in advance.
///
/// This is the `docs/30` §29.3 rule — a fixture has to prove itself before its
/// verdicts count — applied to the one piece of this probe that is real
/// product code: the displacement estimate.
#[test]
fn the_shift_estimator_recovers_a_known_synthetic_shift() {
    let before: Vec<f64> = {
        let mut rng = XorShift32::new(0xA11CE);
        (0..600)
            .map(|_| rng.next_u32() as f64 / u32::MAX as f64)
            .collect()
    };
    for shift in [1usize, 7, 40, 137, 299] {
        let mut after = before[shift..].to_vec();
        let mut rng = XorShift32::new(0xBEEF + shift as u32);
        after.extend((0..shift).map(|_| rng.next_u32() as f64 / u32::MAX as f64));
        let estimate = estimate_shift(&before, &after);
        assert_eq!(
            estimate.shift as usize, shift,
            "recovered {} instead of {shift}",
            estimate.shift
        );
        assert!(
            estimate.correlation > 0.99,
            "correlation at the true shift was only {:.4}",
            estimate.correlation
        );
        assert!(
            estimate.zero_correlation < estimate.correlation,
            "the true shift must beat no movement"
        );
    }
}

/// A list of equally sized text lines is the obvious way to make this estimator
/// report a confident wrong answer: every line looks like every other line, so
/// a shift of exactly one line could correlate as well as the true shift.
///
/// It does not, because the fixtures randomise the *ink* per line. This test
/// states that as an assertion, at a shift that is an exact multiple of the line
/// height — the worst case — and it is the reason `random_line` exists.
#[test]
fn the_shift_estimator_is_not_fooled_by_line_structure() {
    const LINE_HEIGHT: usize = 19;
    const LINES: usize = 500;
    let ink: Vec<f64> = {
        let mut rng = XorShift32::new(0xC0FFEE);
        (0..LINES)
            .map(|_| rng.next_u32() as f64 / u32::MAX as f64)
            .collect()
    };
    let signature: Vec<f64> = (0..LINES * LINE_HEIGHT)
        .map(|row| ink[row / LINE_HEIGHT])
        .collect();

    for line_shift in [1usize, 6, 13] {
        let shift = line_shift * LINE_HEIGHT;
        let mut after = signature[shift..].to_vec();
        after.extend(std::iter::repeat(0.0).take(shift));
        let estimate = estimate_shift(&signature, &after);
        assert_eq!(
            estimate.shift as usize, shift,
            "a shift of {shift} px ({line_shift} lines) was reported as {} px — periodic aliasing",
            estimate.shift
        );
        assert!(
            estimate.runner_up < estimate.correlation,
            "the peak at {shift} px is not unique (runner-up {:.4} vs {:.4})",
            estimate.runner_up,
            estimate.correlation
        );
    }
}

/// The names `winuser.h:5321-5325` gives the three routing values.
const ROUTING_NAMES: [&str; 3] = ["focus", "hybrid", "mouse-position"];

/// `#define WHEEL_PAGESCROLL (UINT_MAX)` (`winuser.h:1269`); `windows-sys` does not
/// export it.
const WHEEL_PAGESCROLL: u32 = u32::MAX;

/// How far one wheel notch scrolls, as the *user* configured it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WheelLines {
    /// `0`: the wheel does not scroll at all. No injection path can work around this,
    /// so the actuator has to know it before it injects anything.
    None,
    /// `WHEEL_PAGESCROLL`: one notch scrolls a whole page, so the step is clamped
    /// rather than treated as a pixel count.
    Page,
    /// A positive line count. It is only the initial guess for the step: the closed
    /// loop still learns the real per-target gain (`docs/30` §16, mechanism P1).
    Lines(u32),
}

fn classify_wheel_lines(raw: u32) -> WheelLines {
    match raw {
        0 => WheelLines::None,
        WHEEL_PAGESCROLL => WheelLines::Page,
        lines => WheelLines::Lines(lines),
    }
}

/// Read the user's `SPI_GETWHEELSCROLLLINES` (`winuser.h:5162`, `0x0068`).
///
/// Read, never assumed: this is a user-changeable setting (`docs/30` F-15), so P3.03's
/// `choose()` takes it as a value instead of freezing it into a constant.
fn read_wheel_scroll_lines() -> Result<u32, String> {
    read_system_parameter_u32(SPI_GETWHEELSCROLLLINES, "SPI_GETWHEELSCROLLLINES")
}

/// Read `SPI_GETMOUSEWHEELROUTING` (`winuser.h:5319`, `0x201C`): `0` focus, `1` hybrid,
/// `2` mouse position. Only `2` lets a non-foreground window receive `SendInput`, which
/// is what makes OQ-5 a measurement rather than an assumption (`docs/30` §24.6).
fn read_mouse_wheel_routing() -> Result<u32, String> {
    read_system_parameter_u32(SPI_GETMOUSEWHEELROUTING, "SPI_GETMOUSEWHEELROUTING")
}

fn read_system_parameter_u32(action: u32, name: &str) -> Result<u32, String> {
    let mut value: u32 = 0;
    // SAFETY: `pvparam` points at a live `u32`, which is the documented size for both
    // SPI_GETWHEELSCROLLLINES and SPI_GETMOUSEWHEELROUTING.
    let ok = unsafe {
        SystemParametersInfoW(
            action,
            0,
            std::ptr::from_mut(&mut value).cast::<core::ffi::c_void>(),
            0,
        )
    };
    if ok == 0 {
        return Err(format!(
            "SystemParametersInfoW({name}) failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(value)
}

/// `docs/31` `P0.07`: the wheel settings are a **runtime** input, not a design premise.
///
/// `SPI_GETWHEELSCROLLLINES` (`winuser.h:5162`, `0x0068`) decides how far one notch
/// scrolls and is user-changeable; `SPI_GETMOUSEWHEELROUTING` (`winuser.h:5319`,
/// `0x201C`) decides *which* window receives the wheel at all. Both change what
/// `ScrollActuator::choose()` may assume, so they are read once at startup and passed
/// in as values (`docs/30` §11.4, §24.6; OQ-5) — never frozen into a compile-time
/// constant. This probe measures the local machine; its output is the record for
/// `docs/30 §11.4`, and it proves the raw values fit the documented domains.
#[test]
fn system_parameter_reads_return_documented_defaults() {
    let lines = read_wheel_scroll_lines().expect("SPI_GETWHEELSCROLLLINES must be readable");
    assert!(
        lines == 0 || lines == WHEEL_PAGESCROLL || (1..=100).contains(&lines),
        "SPI_GETWHEELSCROLLLINES returned {lines}, which is neither 0 (no scrolling), WHEEL_PAGESCROLL (u32::MAX) nor a plausible line count"
    );

    let routing = read_mouse_wheel_routing().expect("SPI_GETMOUSEWHEELROUTING must be readable");
    assert!(
        routing <= 2,
        "SPI_GETMOUSEWHEELROUTING returned {routing}; winuser.h:5321-5325 documents only 0 (focus), 1 (hybrid), 2 (mouse position)"
    );

    eprintln!(
        "[P0.07] local values: SPI_GETWHEELSCROLLLINES = {lines} ({:?}), SPI_GETMOUSEWHEELROUTING = {routing} ({})",
        classify_wheel_lines(lines),
        ROUTING_NAMES[routing as usize]
    );
}

/// A `WHEELSCROLLLINES` of `0` means "do not scroll at all", and `WHEEL_PAGESCROLL`
/// means one notch scrolls a whole page. Neither is an error: the second only clamps
/// the step size, the first is a hard capability fact that no injection path can work
/// around, so the actuator has to know the difference before it injects anything.
#[test]
fn a_zero_wheel_scroll_lines_setting_means_the_wheel_does_not_scroll() {
    assert_eq!(classify_wheel_lines(0), WheelLines::None);
    assert_eq!(classify_wheel_lines(WHEEL_PAGESCROLL), WheelLines::Page);
    assert_eq!(classify_wheel_lines(3), WheelLines::Lines(3));
    assert_eq!(classify_wheel_lines(1), WheelLines::Lines(1));
}

// ---------------------------------------------------------------------------
// Window-level WGC capture (`P0.05` / `E-CAP-1`)
// ---------------------------------------------------------------------------

/// `docs/31` `P0.05`: ten frames per target. Ten is enough to see a black-frame
/// ratio and to catch the "first frame is fine, the rest go stale" failure that
/// decides whether one pool may stay alive for the whole session.
const CAPTURE_FRAMES: usize = 10;

/// How long one frame slot waits before it is called idle. Measured 2026-10-08,
/// this is a *cadence* timeout and not an availability test: a live target answered
/// in 11–64ms, while a static one answered never, so the timeout separates `Idle`
/// from failure with two orders of magnitude to spare.
///
/// The pool capacity this arm runs with is no longer a probe constant: since
/// `P2.01` the arm drives `wgc::WgcSession`, so it measures the production value
/// (`wgc::SESSION_POOL_BUFFERS`).
const CAPTURE_FRAME_TIMEOUT: Duration = Duration::from_millis(1500);

/// Consecutive idle slots that end an arm early. Ten idle slots would spend ten
/// timeouts to learn what three already say; the first run showed a live target
/// answering every ~520ms (a caret blink), so three consecutive idle slots still
/// cannot mistake a slow-but-live target for a dead one.
const CAPTURE_MAX_CONSECUTIVE_IDLE: usize = 3;

/// A frame flatter than this never had content composed into it. This is a
/// detection threshold, not a tuning knob: it is deliberately low because a real
/// window is never flat, while an uncomposed texture is exactly flat.
const BLACK_FRAME_VARIANCE: f64 = 1.0;

/// Variance is estimated on every other row and column. A flat frame stays flat
/// under any sampling; subsampling only makes a 1200x900 target four times cheaper
/// to scan.
const VARIANCE_STEP: usize = 2;

/// How a single frame came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    /// The frame carries an image.
    Content,
    /// The frame is flat: WGC handed out a texture that was never composed.
    Black,
    /// No frame arrived inside the slot: the target's content did not change.
    ///
    /// Measured 2026-10-08: `TryGetNextFrame` answers a null interface pointer —
    /// which the projection surfaces as `Err` carrying `S_OK` (`#code=0`) — when
    /// there is nothing new to show. That is the normal state of a window nobody
    /// is scrolling, and `docs/30` §4.1 already models it as `FramePoll::Idle`.
    Idle,
}

/// Luma variance of a packed BGRA8 buffer.
///
/// Variance rather than "is it all zero" on purpose: a target that paints a solid
/// colour is as flat as a black one, and both mean the same thing to a stitcher.
/// The luma weights are the integer approximation `docs/30` §15.6 fixes for the
/// matcher (`Y = (77R + 150G + 29B) >> 8`), so the probe and the product agree on
/// what "brightness" means.
fn frame_luma_variance(bgra: &[u8], width: u32, height: u32) -> f64 {
    if width == 0 || height == 0 {
        return 0.0;
    }
    let (width, height) = (width as usize, height as usize);
    let (mut sum, mut sum_sq, mut count) = (0.0f64, 0.0f64, 0u64);
    let mut y = 0usize;
    while y < height {
        let mut x = 0usize;
        while x < width {
            let offset = (y * width + x) * 4;
            let (Some(blue), Some(green), Some(red)) = (
                bgra.get(offset).copied(),
                bgra.get(offset + 1).copied(),
                bgra.get(offset + 2).copied(),
            ) else {
                break;
            };
            let luma = (77.0 * f64::from(red) + 150.0 * f64::from(green) + 29.0 * f64::from(blue))
                / 256.0;
            sum += luma;
            sum_sq += luma * luma;
            count += 1;
            x += VARIANCE_STEP;
        }
        y += VARIANCE_STEP;
    }
    if count == 0 {
        return 0.0;
    }
    let mean = sum / count as f64;
    (sum_sq / count as f64) - mean * mean
}

fn classify_frame(bgra: &[u8], width: u32, height: u32) -> FrameKind {
    if frame_luma_variance(bgra, width, height) < BLACK_FRAME_VARIANCE {
        FrameKind::Black
    } else {
        FrameKind::Content
    }
}

/// One frame's measurement.
#[derive(Debug, Clone, Copy)]
struct CaptureFrameReport {
    index: usize,
    /// From asking for a frame to holding one.
    wait_ms: f64,
    /// From holding one to its pixels being in CPU memory.
    readback_ms: f64,
    variance: f64,
    kind: FrameKind,
    content_size: (i32, i32),
}

/// What one target produced, when it produced anything.
#[derive(Debug, Default)]
struct CaptureSummary {
    item_size: (i32, i32),
    /// `Direct3D11CaptureFramePool::Recreate` calls. The only legitimate trigger is
    /// a size change; anything else means the pool cannot be reused.
    recreates: u32,
    frames: Vec<CaptureFrameReport>,
    /// Session-option calls that failed. Collected rather than propagated: a missing
    /// interface is a finding for `docs/30` §24.2, not a reason to lose the frames.
    option_errors: Vec<String>,
}

impl CaptureSummary {
    fn black_frames(&self) -> usize {
        self.frames
            .iter()
            .filter(|frame| frame.kind == FrameKind::Black)
            .count()
    }

    fn idle_frames(&self) -> usize {
        self.frames
            .iter()
            .filter(|frame| frame.kind == FrameKind::Idle)
            .count()
    }

    /// Frames that actually arrived. An arm that only ever saw idle slots never
    /// proved that its target can be captured.
    fn delivered_frames(&self) -> usize {
        self.frames.len() - self.idle_frames()
    }

    /// The exit condition's "first frame cost": everything the session spent
    /// before the first frame arrived, idle slots included.
    fn first_frame_ms(&self) -> f64 {
        self.frames
            .iter()
            .take_while(|frame| frame.kind == FrameKind::Idle)
            .chain(self.frames.iter().find(|frame| frame.kind != FrameKind::Idle))
            .map(|frame| frame.wait_ms + frame.readback_ms)
            .sum()
    }

    fn worst_readback_ms(&self) -> f64 {
        self.frames
            .iter()
            .map(|frame| frame.readback_ms)
            .fold(0.0f64, f64::max)
    }
}

/// One arm of the experiment: a target, or the reason there is no target.
#[derive(Debug)]
struct CaptureArmReport {
    target: &'static str,
    hwnd: HWND,
    /// `Err` always carries a specific reason, and every reason produced by a
    /// Windows API carries its `#code=` HRESULT (`win::hresult`).
    outcome: Result<CaptureSummary, String>,
}

/// The targets `docs/31` `P0.05` names, in the order the report prints them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureTargetKind {
    Chrome,
    Edge,
    Electron,
    /// A WinUI3 desktop app: `docs/30` `E-CAP-1` names it as a target class, and
    /// the machine inventory confirmed a launch recipe and its window class.
    WinUi3,
    WebView2,
    Notepad,
}

impl CaptureTargetKind {
    /// The five classes `docs/30` `E-CAP-1` names, plus WebView2. WebView2 is kept
    /// in the list rather than dropped, because "no reachable WebView2 window on
    /// this machine" is a measurement while an absent arm is an omission: the
    /// inventory found the runtime installed (154.0.4258.53/.62) and no host with a
    /// visible top-level window.
    const ALL: [Self; 6] = [
        Self::Chrome,
        Self::Edge,
        Self::Electron,
        Self::WinUi3,
        Self::WebView2,
        Self::Notepad,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Edge => "edge",
            Self::Electron => "electron",
            Self::WinUi3 => "winui3",
            Self::WebView2 => "webview2",
            Self::Notepad => "notepad",
        }
    }
}

/// A launched or attached target: the window to capture, and what to clean up.
///
/// Cleanup is by process id rather than by the `Child` handle, because three of the
/// launchers do not hand back the process that owns the window: `explorer.exe` exits
/// immediately after starting a packaged app, Image File Execution Options redirect
/// `notepad.exe` to Notepad3, and a browser window belongs to a renderer child.
struct LaunchedTarget {
    window: HWND,
    class: String,
    /// `None` when the window belongs to an application the probe did not start —
    /// killing that would close a window the user is working in.
    kill_pid: Option<u32>,
    /// The spawned launcher, kept so it is reaped instead of leaking a handle.
    process: Option<Child>,
    /// What was actually launched or attached to, for the report.
    note: String,
}

/// `docs/31` `P0.05` first self-check: the black-frame detector must be able to tell
/// an uncomposed frame from a composed one, or the whole arm reports noise.
///
/// The detector is brightness-invariant, so a solid white frame is "black" too —
/// that is the point: what a stitcher needs to know is whether there is *anything
/// to match*, not whether the pixels are dark.
#[test]
fn the_capture_probe_detects_a_black_frame() {
    let empty = [0u8; 0];
    assert_eq!(
        classify_frame(&empty, 0, 0),
        FrameKind::Black,
        "an empty buffer has no content to match"
    );

    let flat_black = vec![0u8; 64 * 64 * 4];
    assert_eq!(classify_frame(&flat_black, 64, 64), FrameKind::Black);

    let flat_white = vec![255u8; 64 * 64 * 4];
    assert_eq!(
        classify_frame(&flat_white, 64, 64),
        FrameKind::Black,
        "a solid colour is as flat as black — the detector measures structure, not darkness"
    );

    let flat_gray = vec![128u8; 64 * 64 * 4];
    assert_eq!(classify_frame(&flat_gray, 64, 64), FrameKind::Black);

    // Half black, half white: two solid halves are exactly the structure a stitcher
    // can lock on to, so this must not be reported as an uncomposed frame.
    let mut split = vec![0u8; 64 * 64 * 4];
    for y in 0..64usize {
        for x in 32..64usize {
            let offset = (y * 64 + x) * 4;
            split[offset] = 255;
            split[offset + 1] = 255;
            split[offset + 2] = 255;
        }
    }
    assert_eq!(classify_frame(&split, 64, 64), FrameKind::Content);

    // A gradient has structure everywhere, so it is content by this definition —
    // whether the *matcher* can use it is a different question (`docs/30` §16).
    let mut gradient = vec![0u8; 64 * 64 * 4];
    for y in 0..64usize {
        for x in 0..64usize {
            let offset = (y * 64 + x) * 4;
            let value = ((x + y) * 2) as u8;
            gradient[offset] = value;
            gradient[offset + 1] = value;
            gradient[offset + 2] = value;
        }
    }
    assert_eq!(classify_frame(&gradient, 64, 64), FrameKind::Content);
}

/// `docs/31` `P0.05` second self-check: a handle that is not a window must come back
/// as "unavailable" with its error code, not as a panic and not as an empty success.
///
/// This is the path every missing target takes (no Electron installed, a WebView2
/// host that is not running), so it has to be exercised by the suite rather than
/// discovered on the one machine that happens to lack a target. Since `P2.01` it
/// goes through the **production** item factory, so the probe and the scroll path
/// cannot disagree about what "not a target" means.
#[test]
fn an_unknown_window_handle_reports_unavailable() {
    let error = wgc::create_item_for_window(0)
        .err()
        .expect("CreateForWindow(null) must not produce a capture item");
    assert!(
        matches!(&error, wgc::WgcError::InvalidTarget { .. }),
        "a null handle is not a target, and it is not a capture failure either: {error:?}"
    );
    assert!(
        error.to_string().contains("#code="),
        "the unavailability reason must keep the HRESULT for diagnostics, got {error}"
    );
}

/// The window class every Chromium-based browser and Electron application uses for its
/// top-level window. It is *not* unique on a desktop — four unrelated processes owned
/// one on the machine this probe was written against — so it may only be used together
/// with something else that identifies the window (a fresh title, or the executable
/// that owns it).
const CHROMIUM_WINDOW_CLASS: &str = "Chrome_WidgetWin_1";

/// The window class a **packaged** WinUI3 / UWP application's window uses.
///
/// Measured, not assumed. Starting the Windows 11 Calculator and listing top-level
/// windows shows the app's window as an `ApplicationFrameWindow` titled `计算器` — the
/// shell hosts a packaged app's window inside its own frame, and the "WinUI3" class is
/// what *desktop* WinUI3 apps (PowerToys, its command palette) report instead.
///
/// This class is shared with every packaged app that is not running (the shell keeps a
/// hidden frame for each), so it is only meaningful together with "this window was not
/// there before", and never with a title: the title is localised.
const PACKAGED_APP_WINDOW_CLASS: &str = "ApplicationFrameWindow";

fn window_handles() -> Vec<HWND> {
    visible_windows()
        .into_iter()
        .map(|window| window.hwnd)
        .collect()
}

/// How to recognise the window a launcher just brought up.
pub(crate) enum WindowMatch<'a> {
    /// Any new top-level window. Notepad, an Electron app and a WinUI3 app each
    /// name their windows differently, so guessing a title reports "the target
    /// never appeared" when in fact it appeared under another name.
    Any,
    /// The unique title the fixture asked for, known only to the launcher.
    TitleContains(&'a str),
    /// The window class, for launchers whose window title is localised.
    Class(&'a str),
}

/// Wait for a top-level window that was not there before the target was launched.
pub(crate) fn wait_for_new_window(
    known: &[HWND],
    wanted: WindowMatch<'_>,
    timeout: Duration,
) -> Option<WindowInfo> {
    let deadline = Instant::now() + timeout;
    loop {
        for window in visible_windows() {
            if known.contains(&window.hwnd) {
                continue;
            }
            let matched = match &wanted {
                WindowMatch::Any => true,
                WindowMatch::TitleContains(needle) => window.title.contains(needle),
                WindowMatch::Class(class) => window.class == *class,
            };
            if matched {
                return Some(window);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        pump_for(Duration::from_millis(100));
    }
}

/// The classes of the top-level windows that appeared since `known` was sampled.
///
/// "No window of class X appeared" is not a diagnosable sentence; the same sentence
/// plus the classes that *did* appear is. A packaged app's window is hosted in the
/// shell's own frame, and its class is not something to guess at.
fn new_window_classes(known: &[HWND]) -> String {
    let mut classes: Vec<String> = visible_windows()
        .into_iter()
        .filter(|window| !known.contains(&window.hwnd))
        .map(|window| window.class)
        .collect();
    classes.sort();
    classes.dedup();
    if classes.is_empty() {
        "no new top-level window appeared at all".to_string()
    } else {
        format!("the new top-level windows have classes {classes:?}")
    }
}

/// The process id that owns `hwnd`.
fn pid_of_window(hwnd: HWND) -> Option<u32> {
    let mut pid = 0u32;
    // SAFETY: `hwnd` comes from `EnumWindows`, and the out-parameter is a live
    // local. A zero pid is reported as "unknown" to the caller.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    (pid != 0).then_some(pid)
}

/// The full image path of the process that owns `hwnd`.
///
/// Used to attach to an Electron window that is already running: these apps are
/// single-instance, so starting one again only raises the existing window, and a
/// launch-and-wait arm would time out against an application that is right there.
fn process_image_path(hwnd: HWND) -> Option<PathBuf> {
    let pid = pid_of_window(hwnd)?;
    // SAFETY: the handle is closed on every path below. `PROCESS_QUERY_LIMITED_
    // INFORMATION` is the least privilege that answers the question and needs no
    // elevation against a same-integrity process.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid) };
    if handle.is_null() {
        return None;
    }
    let mut buffer = [0u16; 512];
    let mut length = buffer.len() as u32;
    // SAFETY: `buffer` is `length` elements long and stays alive across the call.
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length) };
    // SAFETY: `handle` came from `OpenProcess` and is closed exactly once.
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        return None;
    }
    Some(PathBuf::from(String::from_utf16_lossy(
        &buffer[..length as usize],
    )))
}

/// Kill a process tree by id.
///
/// `/T` is what makes the browser case work — the window is owned by a renderer
/// child, not by the launcher we spawned — and it is also what makes the
/// Image-File-Execution-Options redirect harmless: the redirected process is the
/// one that owns the window, whatever started it.
fn kill_pid(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// End whatever this arm started, and reap the launcher handle.
fn shutdown_target(target: &mut LaunchedTarget) {
    if let Some(pid) = target.kill_pid {
        kill_pid(pid);
    }
    if let Some(mut process) = target.process.take() {
        let _ = process.wait();
    }
}

/// Capture ten frames of one window and measure them.
///
/// Every failure path returns a reason instead of panicking: "this target is
/// unavailable, with this error code" is a *result* of `P0.05` (exit condition ③),
/// and a panic would erase the other four targets' measurements with it.
fn capture_window_arm(target: &'static str, hwnd: HWND) -> CaptureArmReport {
    let failed = |message: String| CaptureArmReport {
        target,
        hwnd,
        outcome: Err(message),
    };

    let device = match d3d11::GraphicsDevice::create() {
        Ok(device) => device,
        Err(message) => return failed(message),
    };
    // The production session, not a copy of it (`P2.01`): an arm that cannot open
    // reports the session's own typed reason, so this probe measures the code the
    // scroll loop will actually use.
    let mut session = match wgc::WgcSession::open(&device, hwnd as isize) {
        Ok(session) => session,
        Err(error) => return failed(error.to_string()),
    };

    let mut summary = CaptureSummary {
        item_size: session.size(),
        ..CaptureSummary::default()
    };
    // Recorded, not propagated: see the module documentation.
    summary
        .option_errors
        .extend(session.diagnostics().iter().cloned());

    let mut consecutive_idle = 0usize;
    for index in 0..CAPTURE_FRAMES {
        let wait_started = Instant::now();
        let frame = match session.next_frame(CAPTURE_FRAME_TIMEOUT) {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                // Nothing changed inside the slot. `WgcSession` calls this idle; in
                // production the scroll loop is what ends it, by scrolling.
                consecutive_idle += 1;
                summary.frames.push(CaptureFrameReport {
                    index,
                    wait_ms: wait_started.elapsed().as_secs_f64() * 1000.0,
                    readback_ms: 0.0,
                    variance: 0.0,
                    kind: FrameKind::Idle,
                    content_size: session.size(),
                });
                if consecutive_idle >= CAPTURE_MAX_CONSECUTIVE_IDLE {
                    break;
                }
                continue;
            }
            Err(error) => return failed(format!("frame {index}: {error}")),
        };
        consecutive_idle = 0;
        let wait_ms = wait_started.elapsed().as_secs_f64() * 1000.0;
        let content = frame.size();

        let readback_started = Instant::now();
        let pixels = match device.read_back_bgra(frame.texture()) {
            Ok(pixels) => pixels,
            Err(message) => return failed(format!("frame {index}: {message}")),
        };
        let readback_ms = readback_started.elapsed().as_secs_f64() * 1000.0;

        let variance = frame_luma_variance(
            &pixels,
            content.0.max(0) as u32,
            content.1.max(0) as u32,
        );
        summary.frames.push(CaptureFrameReport {
            index,
            wait_ms,
            readback_ms,
            variance,
            kind: if variance < BLACK_FRAME_VARIANCE {
                FrameKind::Black
            } else {
                FrameKind::Content
            },
            content_size: content,
        });
    }

    // Counted by the session itself, so "one pool served the whole session" is
    // answered by the same `PoolSizing` the production path uses.
    summary.recreates = session.recreations();
    CaptureArmReport {
        target,
        hwnd,
        outcome: Ok(summary),
    }
}

/// Candidate Electron executables. An Electron app shares Chromium's window class
/// but not its host structure, which is exactly why `docs/30` §25.3 lists it as a
/// target class of its own.
/// Electron applications to try, in order.
///
/// `ELECTRON_PATH` comes first so a reviewer can measure a specific application
/// without editing the probe. The rest is a machine-local list on purpose: the usual
/// "apps every developer has" list was *wrong* here — none of VS Code, Slack, Discord,
/// Notion, Obsidian or Postman is installed, so the old list reported "no Electron
/// application found" while four Electron applications were running.
fn electron_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(explicit) = std::env::var_os("ELECTRON_PATH") {
        candidates.push(PathBuf::from(explicit));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        for suffix in [
            r"Programs\Microsoft VS Code\Code.exe",
            r"Programs\cursor\Cursor.exe",
            r"Programs\Notion\Notion.exe",
            r"Obsidian\Obsidian.exe",
            r"Programs\Postman\Postman.exe",
            r"Programs\slack\slack.exe",
            r"Discord\app-0.0.0\Discord.exe",
        ] {
            candidates.push(local.join(suffix));
        }
    }
    for path in [
        r"C:\A_Softwares\Typora\Typora.exe",
        r"C:\A_Softwares\Qoder IDE\Qoder IDE.exe",
        r"C:\A_Softwares\Xiaomi-MiMo-AI\Xiaomi MiMo AI\Xiaomi MiMo AI.exe",
    ] {
        candidates.push(PathBuf::from(path));
    }
    candidates
}

/// An already-running Electron window, matched by the executable that owns it.
///
/// Matching on the owning process rather than on the window title is what makes this
/// correct: `Chrome_WidgetWin_1` is shared, and an Electron window's title is the
/// document it happens to have open.
///
/// A process usually owns several of these windows, and the first one found is often not
/// the content window: `Qoder IDE` parked a `Quest Window` at `(-31989, -32000)`, which is
/// where Windows puts a minimized window. `IsWindowVisible` is true for a minimized window
/// and `BitBlt` of that rectangle returns a flat block, so the arm would have measured a
/// piece of the desktop that no one can see. The largest non-minimized window that has a
/// client area is the one a user would point at.
fn find_running_electron(executable_name: &str) -> Option<(WindowInfo, PathBuf)> {
    let mut best: Option<(WindowInfo, PathBuf, i64)> = None;
    for window in visible_windows() {
        if window.class != CHROMIUM_WINDOW_CLASS {
            continue;
        }
        let Some(path) = process_image_path(window.hwnd) else {
            continue;
        };
        let matches = path
            .file_name()
            .map(|name| name.to_string_lossy().eq_ignore_ascii_case(executable_name))
            .unwrap_or(false);
        if !matches {
            continue;
        }
        if unsafe { IsIconic(window.hwnd) } != FALSE {
            continue;
        }
        let Some(area) = client_area(window.hwnd) else {
            continue;
        };
        let better = match &best {
            Some((_, _, best_area)) => area > *best_area,
            None => true,
        };
        if better {
            best = Some((window, path, area));
        }
    }
    best.map(|(window, path, _)| (window, path))
}

/// Electron, played in the order this machine actually requires.
///
/// 1. Attach to a running instance. These applications are single-instance: starting
///    one again only raises the window that is already open, so a launch-and-wait arm
///    times out against an application that is sitting right there.
/// 2. Otherwise launch an installed one and wait for its new window.
/// 3. Otherwise report the specific reason, listing how many paths were probed.
///
/// An attached window is never killed: the user may be working in it.
fn attach_or_launch_electron(known: &[HWND]) -> Result<LaunchedTarget, String> {
    let candidates = electron_candidates();
    for candidate in &candidates {
        let Some(name) = candidate
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            continue;
        };
        if let Some((window, path)) = find_running_electron(&name) {
            return Ok(LaunchedTarget {
                window: window.hwnd,
                class: window.class,
                kill_pid: None,
                process: None,
                note: format!(
                    "attached to running {} (owned by {})",
                    candidate.display(),
                    path.display()
                ),
            });
        }
    }

    let installed: Vec<&PathBuf> = candidates.iter().filter(|path| path.is_file()).collect();
    let Some(executable) = installed.first().copied() else {
        return Err(format!(
            "未取得：no Electron application is installed and none is running \
             ({} candidate paths probed)",
            candidates.len()
        ));
    };
    let process = Command::new(executable)
        .spawn()
        .map_err(|error| format!("could not start {}: {error}", executable.display()))?;
    let window = wait_for_new_window(known, WindowMatch::Any, Duration::from_secs(30)).ok_or_else(
        || {
            format!(
                "{} started but no new top-level window appeared within 30s",
                executable.display()
            )
        },
    )?;
    Ok(LaunchedTarget {
        kill_pid: pid_of_window(window.hwnd),
        window: window.hwnd,
        class: window.class,
        process: Some(process),
        note: executable.display().to_string(),
    })
}

/// Installed WebView2 runtime versions.
///
/// A WebView2 window can only be reached through a *host* application, so finding
/// the runtime is not finding a target — but it separates "no WebView2 on this
/// machine at all" from "runtime installed, no host to launch", and those two point
/// at different follow-ups.
fn webview2_runtime_versions() -> Vec<String> {
    let root = Path::new(r"C:\Program Files (x86)\Microsoft\EdgeWebView\Application");
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut versions: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().join("msedgewebview2.exe").is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    versions.sort();
    versions
}

/// Start a Chromium browser on the repository's demo page.
///
/// `anchor` is appended to the file URL. It exists because the demo's first screen is its
/// text block, and a shift measured against text measures the 19 px line carrier rather
/// than the page: P0.06 read 10–25 px there for a scroll that had really moved 800 px, and
/// the same arms read 800 px once they were aimed at the texture region. The scroll probes
/// jump to the texture; the capture probe asks for the page as delivered and passes `""`.
fn launch_chromium_target(
    kind: CaptureTargetKind,
    scratch: &Path,
    token: &str,
    anchor: &str,
) -> Result<LaunchedTarget, String> {
    let known = window_handles();
    let label = kind.label();
    let executable = match kind {
        CaptureTargetKind::Chrome => find_chrome(),
        _ => find_edge(),
    }
    .ok_or_else(|| format!("{label} is not installed: no executable at any known path"))?;
    let dir = scratch.join(label);
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("could not create scratch dir {dir:?}: {error}"))?;
    let title = format!("snapclip-probe-{token}-{label}");
    let html = write_chromium_fixture(&dir, &title)?;
    let profile = dir.join("profile");
    let url = format!("{}{anchor}", file_url(&html));
    let process = launch_chromium(&executable, &url, &profile)?;
    let window = wait_for_new_window(
        &known,
        WindowMatch::TitleContains(&title),
        Duration::from_secs(30),
    )
    .ok_or_else(|| {
        format!("{label} started but no window titled {title:?} appeared within 30s")
    })?;
    Ok(LaunchedTarget {
        kill_pid: pid_of_window(window.hwnd),
        window: window.hwnd,
        class: window.class,
        process: Some(process),
        note: format!("{} on {url}", executable.display()),
    })
}

fn launch_capture_target(
    kind: CaptureTargetKind,
    scratch: &Path,
    token: &str,
) -> Result<LaunchedTarget, String> {
    let known = window_handles();
    match kind {
        CaptureTargetKind::Chrome | CaptureTargetKind::Edge => {
            launch_chromium_target(kind, scratch, token, "")
        }
        CaptureTargetKind::Notepad => {
            // The absolute path is used on purpose: `notepad.exe` is subject to Image
            // File Execution Options, and on this machine that key redirects it to
            // Notepad3 — which is why the first run of this probe reported a window
            // class of `Notepad3U`. Naming the path does not avoid the redirect (the
            // key is keyed on the image name), but it does make the report say which
            // binary was asked for.
            let executable =
                Path::new(&std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string()))
                    .join(r"System32\notepad.exe");
            let (document, document_note) = notepad_document(scratch);
            let process = Command::new(&executable)
                .arg(&document)
                .spawn()
                .map_err(|error| format!("could not start {}: {error}", executable.display()))?;
            let window =
                wait_for_new_window(&known, WindowMatch::Any, Duration::from_secs(20)).ok_or_else(
                    || {
                        format!(
                            "{} started but no new top-level window appeared within 20s",
                            executable.display()
                        )
                    },
                )?;
            Ok(LaunchedTarget {
                kill_pid: pid_of_window(window.hwnd),
                window: window.hwnd,
                class: window.class,
                process: Some(process),
                note: format!("{} on {document_note}", executable.display()),
            })
        }
        CaptureTargetKind::Electron => attach_or_launch_electron(&known),
        CaptureTargetKind::WinUi3 => {
            // A packaged app is started through the shell, which is also why the child
            // handle cannot be used for cleanup: `explorer.exe` hands the request to a
            // running instance and exits.
            let app = r"shell:AppsFolder\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App";
            let process = Command::new("explorer.exe")
                .arg(app)
                .spawn()
                .map_err(|error| format!("could not ask explorer.exe to start {app}: {error}"))?;
            let window = wait_for_new_window(
                &known,
                WindowMatch::Class(PACKAGED_APP_WINDOW_CLASS),
                Duration::from_secs(30),
            )
            .ok_or_else(|| {
                format!(
                    "no window of class {PACKAGED_APP_WINDOW_CLASS} appeared within 30s: {}",
                    new_window_classes(&known)
                )
            })?;
            Ok(LaunchedTarget {
                kill_pid: pid_of_window(window.hwnd),
                window: window.hwnd,
                class: window.class,
                process: Some(process),
                note: "Microsoft.WindowsCalculator via explorer.exe shell:AppsFolder".to_string(),
            })
        }
        CaptureTargetKind::WebView2 => {
            let versions = webview2_runtime_versions();
            Err(if versions.is_empty() {
                "未取得：no WebView2 runtime and no host application on this machine".to_string()
            } else {
                format!(
                    "未取得：the WebView2 runtime is installed ({versions:?}) but a WebView2 window \
                     can only be captured through a host application, and none could be launched. \
                     There is no WebView2 window class: the content is composed into the host's \
                     own top-level window, so \"capture WebView2\" is \"capture the host\". SnapClip \
                     itself is no longer a host either — the Tauri shell was removed, and the \
                     %LOCALAPPDATA% WebView2 data directory left behind is a remnant of it."
                )
            })
        }
    }
}

fn print_capture_table(reports: &[CaptureArmReport]) {
    eprintln!();
    eprintln!(
        "{:<11} {:<12} {:>12} {:>6} {:>5} {:>5} {:>9} {:>9} {:>5}  verdict",
        "target", "hwnd", "item", "frames", "black", "idle", "first_ms", "worst_rb", "recr"
    );
    for report in reports {
        match &report.outcome {
            Ok(summary) => {
                eprintln!(
                    "{:<11} {:<12} {:>5}x{:<6} {:>6} {:>5} {:>5} {:>9.1} {:>9.1} {:>5}  {}",
                    report.target,
                    format!("{:p}", report.hwnd),
                    summary.item_size.0,
                    summary.item_size.1,
                    summary.frames.len(),
                    summary.black_frames(),
                    summary.idle_frames(),
                    summary.first_frame_ms(),
                    summary.worst_readback_ms(),
                    summary.recreates,
                    if summary.option_errors.is_empty() {
                        "captured"
                    } else {
                        "captured (with option errors)"
                    }
                );
                for error in &summary.option_errors {
                    eprintln!("{:13}option error: {error}", "");
                }
                for frame in &summary.frames {
                    // An idle slot is the pool saying "nothing changed", which is the
                    // ordinary state of a still window: it gets one line, not a row of
                    // zeros that would drown the delivered frames.
                    if frame.kind == FrameKind::Idle {
                        eprintln!(
                            "{:13}slot {:>2}: idle after {:>7.2}ms",
                            "", frame.index, frame.wait_ms
                        );
                        continue;
                    }
                    eprintln!(
                        "{:13}slot {:>2}: wait {:>7.2}ms readback {:>7.2}ms variance {:>10.3} {:?} {}x{}",
                        "",
                        frame.index,
                        frame.wait_ms,
                        frame.readback_ms,
                        frame.variance,
                        frame.kind,
                        frame.content_size.0,
                        frame.content_size.1
                    );
                }
            }
            Err(reason) => eprintln!(
                "{:<11} {:<12} {:>12} {:>6} {:>5} {:>5} {:>9} {:>9} {:>5}  UNAVAILABLE: {reason}",
                report.target,
                format!("{:p}", report.hwnd),
                "-",
                "-",
                "-",
                "-",
                "-",
                "-",
                "-"
            ),
        }
    }
    eprintln!();
}

/// `docs/31` `P0.05`: is `CreateForWindow` plus one three-buffer free-threaded pool
/// good enough for the window-level capture path `docs/30` §24.2 commits to?
///
/// Run it with a real interactive desktop — it opens six targets and closes the ones it
/// opened (an Electron window it merely attached to is left alone):
///
/// ```text
/// cargo test -p snapclip-capture --lib capture_probe -- --ignored --nocapture
/// ```
///
/// Two environment variables change what it measures, and both exist because this
/// machine's inventory is not the repository's business:
///
/// * `P0.05_PROBE_TEXT` — the document the Notepad arm opens (default:
///   `docs/Temp/p0-05-notepad.txt`, otherwise generated filler).
/// * `ELECTRON_PATH` — the Electron application to prefer.
///
/// A window that has stopped changing yields idle slots rather than frames, so the exit
/// condition is "at least one delivered frame", not "ten frames": Web Graphics Capture
/// produces a frame only when the content changes, and a still page is not a failure.
#[test]
#[ignore = "P0.05: needs a real interactive desktop and at least one installed target"]
fn capture_probe() {
    let _ = monitor::set_per_monitor_v2_awareness();

    // Self-check first, on a handle that is not a window: if this does not come back
    // as a recorded reason, every "unavailable" row below would be indistinguishable
    // from a crash.
    let invalid = capture_window_arm("invalid-handle", ptr::null_mut());
    assert!(
        invalid.outcome.is_err(),
        "an invalid handle must be reported as unavailable, not captured"
    );

    let token = format!(
        "{:08x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    );
    let scratch = std::env::temp_dir().join(format!("snapclip-p005-{token}"));
    std::fs::create_dir_all(&scratch).expect("creating the probe scratch dir");

    let mut reports = vec![invalid];
    for kind in CaptureTargetKind::ALL {
        match launch_capture_target(kind, &scratch, &token) {
            Ok(mut launched) => {
                eprintln!(
                    "[P0.05] {}: {} (window class {})",
                    kind.label(),
                    launched.note,
                    launched.class
                );
                let report = capture_window_arm(kind.label(), launched.window);
                reports.push(report);
                shutdown_target(&mut launched);
            }
            Err(reason) => reports.push(CaptureArmReport {
                target: kind.label(),
                hwnd: ptr::null_mut(),
                outcome: Err(reason),
            }),
        }
    }
    print_capture_table(&reports);
    let _ = std::fs::remove_dir_all(&scratch);

    // The exit conditions of `docs/31` P0.05: every target either produced content
    // frames, or carries the specific reason it could not.
    for report in &reports {
        match &report.outcome {
            Ok(summary) => {
                assert!(
                    summary.delivered_frames() > 0,
                    "{} delivered no frame carrying content (all {} slots were idle)",
                    report.target,
                    summary.frames.len()
                );
                assert_eq!(
                    summary.black_frames(),
                    0,
                    "{} returned {} flat frames out of {}: the pool handed out uncomposed textures",
                    report.target,
                    summary.black_frames(),
                    summary.frames.len()
                );
            }
            Err(reason) => assert!(
                !reason.is_empty(),
                "{} is unavailable without a reason",
                report.target
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// P0.09 / E-INJECT-1 (completion): the third state of an injection
// ---------------------------------------------------------------------------

/// What an injected wheel turned out to be.
///
/// `PostMessageW` returning `TRUE` says one thing only: a thread queue accepted the
/// message. `docs/30 §24.6.2` turns on the difference between "the target refused it",
/// "the target ignored it" and "the target acted on it" — an arm that reports only
/// "no movement" cannot distinguish a target limitation from a transport failure, and
/// the two call for opposite design responses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Delivery {
    /// The message reached a queue and the target acted on it.
    Consumed,
    /// The message reached a queue and the target's behaviour did not change.
    EnqueuedButNotConsumed,
    /// Nothing accepted the message: the post itself was refused.
    Unreached,
}

fn classify_delivery(posted: bool, moved_px: i32) -> Delivery {
    if !posted {
        Delivery::Unreached
    } else if moved_px != 0 {
        Delivery::Consumed
    } else {
        Delivery::EnqueuedButNotConsumed
    }
}

#[test]
fn the_probe_distinguishes_rejected_from_unreached() {
    // Three states, and the matrix is unreadable if any two are collapsed:
    //   Consumed               - the target acted on the wheel
    //   EnqueuedButNotConsumed - `PostMessageW` returned TRUE and the target ignored it
    //   Unreached              - nothing accepted the post at all
    //
    // The live halves are produced by `the_probe_produces_all_three_delivery_states`
    // on a real desktop; this test pins the classification itself.
    assert_eq!(classify_delivery(true, 120), Delivery::Consumed);
    assert_eq!(
        classify_delivery(true, 0),
        Delivery::EnqueuedButNotConsumed,
        "a post that was accepted and changed nothing is not a delivery failure"
    );
    assert_eq!(
        classify_delivery(false, 0),
        Delivery::Unreached,
        "a refused post is not evidence about the target's handling"
    );
    // A refused post cannot have moved anything, so this pair is unreachable by
    // construction — the post is checked first on purpose, and the ordering is asserted
    // rather than left to a reader's assumption.
    assert_eq!(classify_delivery(false, 120), Delivery::Unreached);
}

#[test]
fn a_flat_capture_area_reports_no_variance() {
    // The matrix's zeros are only readable together with this number: an area with no row
    // structure cannot show movement, so its zero is not evidence about injection.
    assert_eq!(signature_variance(&[]), 0.0);
    assert_eq!(signature_variance(&[0.5, 0.5, 0.5, 0.5]), 0.0);
    assert!(
        signature_variance(&[0.0, 1.0, 0.0, 1.0]) > FLAT_SIGNATURE_VARIANCE,
        "alternating full-black and full-white rows are structure, not a flat area"
    );
    assert!(
        signature_variance(&[0.5, 0.5, 0.5, 0.500001]) < FLAT_SIGNATURE_VARIANCE,
        "a page whose rows differ by less than the flatness threshold has nothing to track"
    );
}

/// Produce the three states on real windows.
///
/// `WheelFixture` counts what its own window procedure consumed, and the `EDIT` fixture is
/// a real target that accepts a posted wheel and does not act on it — those two plus a
/// handle that is not a window cover every state the injection matrix reports.
#[test]
#[ignore = "P0.09: needs a real interactive desktop"]
fn the_probe_produces_all_three_delivery_states() {
    let _ = monitor::set_per_monitor_v2_awareness();

    // 1. Consumed: our own window procedure counted the wheel.
    let wheel = WheelFixture::create(Rect::from_origin_size(Point::new(120, 100), 900, 700))
        .expect("creating the wheel fixture");
    bring_to_front(wheel.hwnd, true);
    let client = wheel.client_rect().expect("wheel fixture client rect");
    let center = Point::new(
        (client.left + client.right) / 2,
        (client.top + client.bottom) / 2,
    );
    let received_before = wheel.wheels_received();
    let posted = post_message_notches(wheel.hwnd, center, CoordSpace::Client, 1).is_ok();
    let consumed = wheel.wheels_received().saturating_sub(received_before);
    assert_eq!(
        classify_delivery(posted, i32::try_from(consumed).unwrap_or(i32::MAX)),
        Delivery::Consumed,
        "the fixture received {consumed} wheel messages and the post reported {posted}"
    );

    // 2. EnqueuedButNotConsumed: an `EDIT` control takes the post and ignores the wheel.
    let lines = random_lines(1400, 0x0F09_1234);
    let edit = EditFixture::create(&lines, Rect::from_origin_size(Point::new(80, 60), 900, 700))
        .expect("creating the EDIT fixture");
    bring_to_front(edit.hwnd, true);
    let edit_client = edit.client_rect().expect("EDIT client rect");
    let edit_center = Point::new(
        (edit_client.left + edit_client.right) / 2,
        (edit_client.top + edit_client.bottom) / 2,
    );
    let line_before = edit.first_visible_line();
    let edit_posted = post_message_notches(edit.hwnd, edit_center, CoordSpace::Client, 4).is_ok();
    let line_after = edit.first_visible_line();
    assert_eq!(
        classify_delivery(edit_posted, line_after - line_before),
        Delivery::EnqueuedButNotConsumed,
        "the EDIT control reported line {line_before} -> {line_after} and the post reported {edit_posted}"
    );

    // 3. Unreached: a handle that is not a window.
    let bogus: HWND = 0xDEAD_BEEF_usize as *mut core::ffi::c_void;
    let refused = post_message_notches(bogus, Point::new(10, 10), CoordSpace::Client, 1);
    assert!(
        refused.is_err(),
        "posting to a handle that is not a window was accepted, so the matrix cannot tell \
         a refused post from an ignored one"
    );
    assert_eq!(
        classify_delivery(false, 0),
        Delivery::Unreached,
        "a post that was refused is Unreached"
    );
}

// ---------------------------------------------------------------------------
// P0.09 / E-INJECT-1: the injection matrix
// ---------------------------------------------------------------------------

/// The scrollable targets the injection matrix runs against.
///
/// Chrome is the reference the other browsers are compared with. Settings stands in for a
/// packaged application because the calculator — P0.05's capture target — has nothing to
/// scroll, and an arm aimed at it could not tell "injection failed" from "there was
/// nothing to scroll".
#[derive(Clone, Copy, Debug)]
enum ScrollTargetKind {
    Chrome,
    Edge,
    PackagedApp,
}

impl ScrollTargetKind {
    const ALL: [Self; 3] = [Self::Chrome, Self::Edge, Self::PackagedApp];

    fn label(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Edge => "edge",
            Self::PackagedApp => "winui3",
        }
    }

    fn capture_kind(self) -> CaptureTargetKind {
        match self {
            Self::Chrome => CaptureTargetKind::Chrome,
            Self::Edge => CaptureTargetKind::Edge,
            Self::PackagedApp => CaptureTargetKind::WinUi3,
        }
    }
}

/// The page the packaged-application scroll arm asks for.
///
/// The home page of Settings is a short list that fits on a 1080p screen, so an arm aimed at
/// it could not tell "injection failed" from "there was nothing to scroll" — the same
/// objection that moved `P0.05`'s capture target off the calculator. The installed-apps list
/// is thousands of pixels long, and a URI navigates the instance that is already running, so
/// the second and later runs of the probe keep pointing at the long page.
const PACKAGED_SETTINGS_PAGE: &str = "ms-settings:appsfeatures";

/// A packaged application window that is already open, if there is one.
///
/// The class alone is not a window: the shell keeps a hidden `ApplicationFrameWindow` for
/// every packaged application it knows about, and P0.05 measured one at `0,0,2560,1392`
/// with `DWMWA_CLOAKED = 2` and an empty title. A frame with a title and a real client
/// area is the application's own.
///
/// Attaching matters as much as finding: starting an application that is already running
/// opens no new window, so a launch-and-wait arm times out against the very window it
/// wants, and the second run of a probe would report "unavailable" for a target that is
/// sitting right there.
fn find_packaged_window() -> Option<WindowInfo> {
    let mut best: Option<(WindowInfo, i64)> = None;
    for window in visible_windows() {
        if window.class != PACKAGED_APP_WINDOW_CLASS {
            continue;
        }
        if unsafe { IsIconic(window.hwnd) } != FALSE || window.title.trim().is_empty() {
            continue;
        }
        let Some(area) = client_area(window.hwnd) else {
            continue;
        };
        if area < MIN_USABLE_CLIENT_AREA {
            continue;
        }
        let better = match &best {
            Some((_, best_area)) => area > *best_area,
            None => true,
        };
        if better {
            best = Some((window, area));
        }
    }
    best.map(|(window, _)| window)
}

fn launch_packaged_scroll_target() -> Result<LaunchedTarget, String> {
    let known = window_handles();
    // The page matters as much as the application: the home page of Settings is a short list
    // that fits on a 1080p screen, so an arm aimed at it could not tell "injection failed"
    // from "there was nothing to scroll" — the same objection that moved P0.05's capture
    // target off the calculator. The installed-apps list is thousands of pixels long, and a
    // URI navigates the instance that is already running, so the second and later runs of
    // this probe keep pointing at the long page instead of a fresh home screen.
    let process = Command::new("explorer.exe")
        .arg(PACKAGED_SETTINGS_PAGE)
        .spawn()
        .map_err(|error| {
            format!("could not ask explorer.exe to open {PACKAGED_SETTINGS_PAGE}: {error}")
        })?;
    // A packaged application is single-instance, so a page request usually opens no new
    // window: wait briefly, then attach to the frame that is already there.
    if let Some(window) = wait_for_new_window(
        &known,
        WindowMatch::Class(PACKAGED_APP_WINDOW_CLASS),
        Duration::from_secs(8),
    ) {
        return Ok(LaunchedTarget {
            kill_pid: pid_of_window(window.hwnd),
            window: window.hwnd,
            class: window.class,
            process: Some(process),
            note: format!("{PACKAGED_SETTINGS_PAGE} in a new window"),
        });
    }
    let window = find_packaged_window().ok_or_else(|| {
        format!(
            "no window of class {PACKAGED_APP_WINDOW_CLASS} appeared within 8s and none was open: {}",
            new_window_classes(&known)
        )
    })?;
    Ok(LaunchedTarget {
        window: window.hwnd,
        class: window.class,
        // Never killed: this is an application the user may be using.
        kill_pid: None,
        process: None,
        note: format!(
            "{PACKAGED_SETTINGS_PAGE} navigated the running instance, title {:?}",
            window.title
        ),
    })
}

/// A label that outlives the call.
///
/// `Outcome::target` is a `&'static str` because every arm in the table has a compile-time
/// name, and widening the report type to `String` for the sake of one arm that is
/// discovered at run time would cost every reader of the table. Leaking a handful of short
/// strings inside an `#[ignore]` probe is cheaper than that.
fn leak_label(text: &str) -> &'static str {
    Box::leak(text.to_owned().into_boxed_str())
}

/// Attach to every Electron application that is running right now.
///
/// Nothing is opened in them on purpose: these are the user's applications, and handing
/// one a document could replace an unsaved file or block on a save prompt. The arms
/// scroll whatever the window already shows, so an application whose content is not
/// scrollable reports zero movement *with its name attached* — a fact about that
/// application, not a silent hole in the matrix.
fn running_electrons() -> Vec<(&'static str, LaunchedTarget)> {
    let mut targets = Vec::new();
    for candidate in electron_candidates() {
        let Some(name) = candidate
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            continue;
        };
        let Some((window, path)) = find_running_electron(&name) else {
            continue;
        };
        let stem = candidate
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.clone());
        targets.push((
            leak_label(&format!("electron-{stem}")),
            LaunchedTarget {
                window: window.hwnd,
                class: window.class,
                kill_pid: None,
                process: None,
                note: format!(
                    "attached to running {} (owned by {})",
                    candidate.display(),
                    path.display()
                ),
            },
        ));
    }
    targets
}

fn launch_scroll_target(
    kind: ScrollTargetKind,
    scratch: &Path,
    token: &str,
) -> Result<LaunchedTarget, String> {
    match kind {
        ScrollTargetKind::Chrome | ScrollTargetKind::Edge => {
            launch_chromium_target(kind.capture_kind(), scratch, token, DEMO_TEXTURE_ANCHOR)
        }
        ScrollTargetKind::PackagedApp => launch_packaged_scroll_target(),
    }
}

/// P0.09's injection matrix.
///
/// Four questions, and each is a row of the report:
///   1. every reachable target under both transports;
///   2. the integrity-level pair — in `uipi_probe`, because it needs a sender this
///      process cannot become;
///   3. `SendInput` while the target is *not* the foreground window, which is what
///      `SPI_GETMOUSEWHEELROUTING` decides;
///   4. a client area smaller than the window's landing point: client vs screen `lParam`.
///
/// Electron arms scroll the windows of applications the operator already has open. Nothing
/// is written to them and nothing is killed; the report names each one.
#[test]
#[ignore = "P0.09: needs a real interactive desktop and installed targets"]
fn inject_matrix_probe() {
    let _ = monitor::set_per_monitor_v2_awareness();

    let token = format!(
        "{:08x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    );
    let scratch = std::env::temp_dir().join(format!("snapclip-p09-{token}"));
    std::fs::create_dir_all(&scratch).expect("creating the probe scratch dir");

    let steps = 8;
    let mut outcomes: Vec<Outcome> = Vec::new();

    // The routing setting decides what a `SendInput` wheel is aimed at, so it is part of
    // the result rather than context around it (P0.07).
    match read_mouse_wheel_routing() {
        Ok(raw) => eprintln!(
            "[P0.09] SPI_GETMOUSEWHEELROUTING = {raw} ({})",
            ROUTING_NAMES.get(raw as usize).copied().unwrap_or("unknown")
        ),
        Err(error) => eprintln!("[P0.09] SPI_GETMOUSEWHEELROUTING could not be read: {error}"),
    }
    match read_wheel_scroll_lines() {
        Ok(raw) => match classify_wheel_lines(raw) {
            WheelLines::Lines(lines) => {
                eprintln!("[P0.09] SPI_GETWHEELSCROLLLINES = {lines} lines per notch")
            }
            WheelLines::Page => eprintln!("[P0.09] SPI_GETWHEELSCROLLLINES = WHEEL_PAGESCROLL"),
            WheelLines::None => eprintln!("[P0.09] SPI_GETWHEELSCROLLLINES = 0 (no scrolling)"),
        },
        Err(error) => eprintln!("[P0.09] SPI_GETWHEELSCROLLLINES could not be read: {error}"),
    }
    match send_input_liveness() {
        Ok(delta) => eprintln!("[P0.09] SendInput liveness: a +40 px move moved {delta} px"),
        Err(error) => panic!("P0.09 environment failure: {error}"),
    }

    // The positive control: a window of our own whose procedure counts what it consumed.
    match WheelFixture::create(Rect::from_origin_size(Point::new(200, 140), 1000, 760)) {
        Ok(fixture) => {
            for transport in [
                Transport::SendInput,
                Transport::PostMessage,
                Transport::ProductSendInput,
                Transport::ProductPost,
            ] {
                bring_to_front(fixture.hwnd, true);
                let client = fixture.client_rect().expect("wheel fixture client rect");
                outcomes.push(run_arm_stepwise(
                    "win32-own",
                    transport,
                    CoordSpace::Client,
                    fixture.hwnd,
                    client,
                    steps,
                    None,
                ));
            }
        }
        Err(error) => panic!("the wheel fixture could not be created: {error}"),
    }

    let mut targets: Vec<(&'static str, LaunchedTarget)> = Vec::new();
    for kind in ScrollTargetKind::ALL {
        match launch_scroll_target(kind, &scratch, &token) {
            Ok(launched) => {
                eprintln!(
                    "[P0.09] {}: {} (class {})",
                    kind.label(),
                    launched.note,
                    launched.class
                );
                targets.push((kind.label(), launched));
            }
            Err(reason) => eprintln!("[P0.09] {}: UNAVAILABLE: {reason}", kind.label()),
        }
    }
    for (label, launched) in running_electrons() {
        eprintln!(
            "[P0.09] {label}: {} (class {})",
            launched.note, launched.class
        );
        targets.push((label, launched));
    }

    for (label, launched) in &targets {
        let label: &'static str = label;
        let root = launched.window;
        let Some(client) = client_rect_of(root) else {
            eprintln!("[P0.09] {label}: the window has no client rect; arm skipped");
            continue;
        };
        bring_to_front(root, false);
        if let Err(error) = describe_scroll_target(label, root, client) {
            eprintln!("[P0.09] {label}: the pre-arm capture failed: {error}");
            continue;
        }
        for transport in [
            Transport::SendInput,
            Transport::PostMessage,
            // `P3.01` exit condition 2: the same matrix, through the shipped actuator.
            Transport::ProductSendInput,
            Transport::ProductPost,
        ] {
            let before = page_scroll_y(root);
            let outcome = run_arm_stepwise(
                label,
                transport,
                CoordSpace::Client,
                root,
                client,
                steps,
                None,
            );
            report_page_delta(label, transport, before, page_scroll_y(root));
            outcomes.push(outcome);
        }
        // The documented coordinate space, as a diagnostic: P0.06 found the two
        // equivalent on Chromium, and `docs/30 §24.6.2` requires that equivalence.
        {
            let before = page_scroll_y(root);
            let outcome = run_arm_stepwise(
                label,
                Transport::PostMessage,
                CoordSpace::Screen,
                root,
                client,
                4,
                None,
            );
            report_page_delta(label, Transport::PostMessage, before, page_scroll_y(root));
            outcomes.push(outcome);
        }
        // The control that separates the two readings of a zero: a window that never
        // receives an injected key either is not receiving our input at all, or is
        // receiving it and refusing the wheel.
        {
            let before = page_scroll_y(root);
            let outcome = run_arm_stepwise(
                label,
                Transport::ArrowDown,
                CoordSpace::Client,
                root,
                client,
                8,
                None,
            );
            report_page_delta(label, Transport::ArrowDown, before, page_scroll_y(root));
            outcomes.push(outcome);
        }
    }

    // 4. A client area smaller than the window's landing point: if the two coordinate
    //    spaces stop agreeing here, the requirement is a hard constraint, not a detail.
    if let Some(entry) = targets.iter().find(|entry| entry.0 == "chrome") {
        let small = Rect::from_origin_size(Point::new(120, 120), 420, 320);
        unsafe {
            SetWindowPos(
                entry.1.window,
                HWND_TOP,
                small.left,
                small.top,
                420,
                320,
                SWP_SHOWWINDOW,
            );
        }
        pump_for(Duration::from_millis(1500));
        match client_rect_of(entry.1.window) {
            Some(client) => {
                eprintln!(
                    "[P0.09] small-window client rect = {client:?} (asked for 420x320 at 120,120)"
                );
                bring_to_front(entry.1.window, false);
                if let Err(error) = describe_scroll_target("chrome-small", entry.1.window, client) {
                    eprintln!("[P0.09] chrome-small: the pre-arm capture failed: {error}");
                }
                // The documented space first, then the one Chromium is handed by the
                // reference implementation, then that one again: if only the first run of a
                // space fails, what failed was the reflow after the resize, not the space.
                for space in [
                    CoordSpace::Screen,
                    CoordSpace::Client,
                    CoordSpace::Client,
                ] {
                    let before = page_scroll_y(entry.1.window);
                    let outcome = run_arm_stepwise(
                        "chrome-small",
                        Transport::PostMessage,
                        space,
                        entry.1.window,
                        client,
                        4,
                        None,
                    );
                    report_page_delta(
                        "chrome-small",
                        Transport::PostMessage,
                        before,
                        page_scroll_y(entry.1.window),
                    );
                    outcomes.push(outcome);
                }
            }
            None => eprintln!("[P0.09] the resized window has no client rect"),
        }
    }

    // 3. `SendInput` while the target is not the foreground window.
    //
    //    `SPI_GETMOUSEWHEELROUTING` decides where an injected wheel goes: `MOUSE_POS`
    //    aims it at the window under the cursor, `FOCUS` at the foreground window. The
    //    arm gives the foreground to a window of our own, parks the cursor over the
    //    target, and reports which of the two moved — that pair is the whole question,
    //    because a target SnapClip cannot focus is reachable only under `MOUSE_POS`.
    if let Some(entry) = targets.iter().find(|entry| entry.0 == "chrome") {
        match WheelFixture::create(Rect::from_origin_size(Point::new(1680, 140), 700, 500)) {
            Ok(fixture) => match client_rect_of(entry.1.window) {
                Some(client) => {
                    bring_to_front(fixture.hwnd, true);
                    let fixture_before = fixture.offset();
                    let page_before = page_scroll_y(entry.1.window);
                    outcomes.push(run_arm_stepwise(
                        "chrome-unfocused",
                        Transport::SendInput,
                        CoordSpace::Client,
                        entry.1.window,
                        client,
                        4,
                        None,
                    ));
                    report_page_delta(
                        "chrome-unfocused",
                        Transport::SendInput,
                        page_before,
                        page_scroll_y(entry.1.window),
                    );
                    eprintln!(
                        "[P0.09] the foreground window of our own consumed {} px while the target \
                         was not focused",
                        fixture.offset() - fixture_before
                    );
                }
                None => eprintln!("[P0.09] the unfocused arm has no client rect to aim at"),
            },
            Err(error) => eprintln!("[P0.09] the unfocused arm needs a fixture: {error}"),
        }
    }

    print_table(&outcomes);

    // The matrix is only readable if the measurement works: an apparatus that cannot move
    // a window of our own would make every zero above meaningless.
    let control_moved = outcomes
        .iter()
        .filter(|outcome| outcome.target == "win32-own" && outcome.moved())
        .count();
    assert_eq!(
        control_moved, 4,
        "the own-window control did not scroll under all four transports (two of them the \
         shipped actuator), so no zero in this table can be read as a statement about a target"
    );
    // And the headline `docs/30 §24.6` depends on, re-measured on the matrix's own targets.
    for transport in ["SendInput", "PostMessageW"] {
        let chromium = outcomes
            .iter()
            .find(|outcome| {
                outcome.target == "chrome"
                    && outcome.transport == transport
                    && outcome.space == CoordSpace::Client.label()
            })
            .expect("a chrome arm exists for every transport");
        assert!(
            chromium.moved(),
            "chrome measured {} px under {transport}: the two-path decision does not hold on \
             this machine any more",
            chromium.measured_px
        );
    }

    // `P3.01` exit condition 2, as an assertion rather than a table: the shipped actuator
    // must reach the same two targets the probe's own wire format reaches. A product path
    // that silently posts to the frame instead of the renderer child would pass every unit
    // test in `scroll_actuator` and fail here.
    for transport in [Transport::ProductSendInput, Transport::ProductPost] {
        assert!(transport.is_product(), "the arm is the product path");
        let own = outcomes
            .iter()
            .find(|outcome| outcome.target == "win32-own" && outcome.transport == transport.label())
            .unwrap_or_else(|| {
                panic!(
                    "the product arm {} never ran against the control window",
                    transport.label()
                )
            });
        assert!(
            own.moved(),
            "the shipped actuator moved our own window {} px under {}: the product path is not \
             wired to the code the probe measured",
            own.measured_px,
            transport.label()
        );
    }
    for transport in [Transport::ProductSendInput, Transport::ProductPost] {
        let Some(chromium) = outcomes.iter().find(|outcome| {
            outcome.target == "chrome" && outcome.transport == transport.label()
        }) else {
            continue;
        };
        assert!(
            chromium.moved(),
            "chrome measured {} px under the product path {}: `docs/30 §24.6`'s two paths hold \
             for the probe's copy of the wire format but not for the shipped one",
            chromium.measured_px,
            transport.label()
        );
    }
}

/// P0.09's integrity-level arm.
///
/// The pair this needs is *low sender → high target*, and on this machine the sender is the
/// side that has to change: `EnableLUA` is 0, so every interactive process here — this harness,
/// the browsers, the editors — runs at High integrity, and there is no higher level for it to
/// be blocked by. The documented route to a lower one, `runas /trustlevel:0x20000`, does not
/// work: the restricted token inherits the parent's mandatory label, and the process reported
/// `Mandatory Label\High Mandatory Level S-1-16-12288`. A genuinely low sender needs the token
/// rewritten, which is what `tools/p009-low-integrity-launch.ps1` does — the child's own
/// `whoami /groups` line reads `Mandatory Label\Low Mandatory Level S-1-16-4096`.
///
/// Three things the operator supplies, or the arm measures the environment instead of the
/// barrier:
///  * a scrollable high-integrity window, named by `SNAPCLIP_UIPI_TARGET` (title substring);
///  * the sender started by that launcher with `SNAPCLIP_UIPI_LOW=1`, and with
///    `CREATE_NO_WINDOW`, because a low-integrity child cannot attach to this console and the
///    one Windows allocates lands on the target's centre — where the wheel is aimed;
///  * `SNAPCLIP_UIPI_NO_AIM=1` once a higher-integrity helper has placed the cursor, because
///    `SetCursorPos` itself fails from a low sender (`FALSE`, last error left at 0). Without
///    that split the arm only learns "this process cannot place the cursor".
///
/// The arm never foregrounds the target, because a low-integrity process cannot raise a
/// high-integrity window and the resulting assertion failure would look like a finding. What it
/// reports is each transport's own outcome, which is the evidence: a `PostMessageW` refused
/// with ERROR_ACCESS_DENIED and a `SendInput` that reports events without moving the target are
/// the loud and silent halves of the same barrier.
#[test]
#[ignore = "P0.09: takes SNAPCLIP_UIPI_TARGET and a low-integrity sender from tools/p009-low-integrity-launch.ps1"]
fn uipi_probe() {
    let _ = monitor::set_per_monitor_v2_awareness();

    let Ok(needle) = std::env::var("SNAPCLIP_UIPI_TARGET") else {
        panic!(
            "set SNAPCLIP_UIPI_TARGET to a substring of the title of a scrollable window, then \
             run this test as the low-integrity sender: runas /trustlevel:0x20000 \
             \"target\\debug\\deps\\snapclip_capture-<hash>.exe\" uipi_probe --ignored --nocapture \
             --test-threads=1"
        );
    };
    let window = find_window_by_title(&needle, Duration::from_secs(5))
        .unwrap_or_else(|| panic!("no visible window whose title contains {needle:?}"));
    let Some(client) = client_rect_of(window.hwnd) else {
        panic!("the window whose title contains {needle:?} has no client rect");
    };
    eprintln!(
        "[P0.09] UIPI arm: target {:?} class {} client {client:?}",
        window_title(window.hwnd),
        window_class(window.hwnd)
    );
    eprintln!(
        "[P0.09] UIPI arm: sender marker SNAPCLIP_UIPI_LOW={:?} (present means the operator \
         launched this process with a restricted token)",
        std::env::var("SNAPCLIP_UIPI_LOW").ok()
    );

    let mut outcomes = Vec::new();
    outcomes.push(run_arm_stepwise(
        "uipi-target",
        Transport::SendInput,
        CoordSpace::Client,
        window.hwnd,
        client,
        4,
        None,
    ));
    outcomes.push(run_arm_stepwise(
        "uipi-target",
        Transport::PostMessage,
        CoordSpace::Client,
        window.hwnd,
        client,
        4,
        None,
    ));
    print_table(&outcomes);

    let send_input = outcomes
        .iter()
        .find(|outcome| outcome.transport == "SendInput")
        .expect("the SendInput arm ran");
    let post = outcomes
        .iter()
        .find(|outcome| outcome.transport == "PostMessageW")
        .expect("the PostMessageW arm ran");
    // The claim under test is asymmetry, so a run in which both work or both fail is a
    // result about the environment, not confirmation of `docs/30 §24.6.2` — and it is
    // reported as such instead of being asserted away.
    eprintln!(
        "[P0.09] UIPI arm: SendInput reported {:?} ({} px), PostMessageW reported {:?} ({} px)",
        send_input.error.as_deref().unwrap_or("accepted"),
        send_input.measured_px,
        post.error.as_deref().unwrap_or("accepted"),
        post.measured_px
    );
}

// --- OQ-2: does `WDA_EXCLUDEFROMCAPTURE` affect display-level WGC? (P2 phase exit ②) ---
//
// `docs/30 §24.5` and `OQ-2` say the same thing from two sides: the overlay already calls
// `SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE)` (`overlay/window_host.rs:228`),
// **MS Learn never mentions WDA in any of the capture pages** (F-22: official silence, not a
// gap in the search), and V2 therefore refuses to treat WDA as a correctness guarantee. It
// is "best effort on the monitor-level fallback path" until somebody measures it.
//
// The measurement needs a window whose presence in a captured frame is a *pixel value*, not
// a judgement call — hence a fixture painted one flat colour that appears nowhere else on a
// normal desktop.

/// COLORREF is `0x00BBGGRR`. This green is chosen to be a colour a desktop does not contain.
const WDA_FIXTURE_COLOUR: u32 = 0x0038_C46A;

/// What one sampled pixel says about whether the fixture is in the captured frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WdaVerdict {
    /// The fixture's own colour: the window is in the frame.
    Visible,
    /// Solid black where the fixture is. Per `F-22` this is what `WDA_MONITOR` does, and
    /// what `WDA_EXCLUDEFROMCAPTURE` degrades to before Win10 2004.
    Black,
    /// Something else: neither the fixture nor black. Reported rather than folded into
    /// either verdict, because "excluded" and "replaced by a different window" differ.
    Other(u8, u8, u8),
}

impl WdaVerdict {
    fn of(pixel: (u8, u8, u8)) -> Self {
        let (blue, green, red) = pixel;
        let expected = (
            (WDA_FIXTURE_COLOUR & 0xFF) as u8,
            ((WDA_FIXTURE_COLOUR >> 8) & 0xFF) as u8,
            ((WDA_FIXTURE_COLOUR >> 16) & 0xFF) as u8,
        );
        // Tolerance for the compositor's colour handling; a flat fill should be exact, but
        // "almost exactly our green" is still our green.
        let near = |a: u8, b: u8| a.abs_diff(b) <= 2;
        if near(red, expected.2) && near(green, expected.1) && near(blue, expected.0) {
            Self::Visible
        } else if red < 8 && green < 8 && blue < 8 {
            Self::Black
        } else {
            Self::Other(red, green, blue)
        }
    }

    fn name(self) -> String {
        match self {
            Self::Visible => "visible".to_string(),
            Self::Black => "black".to_string(),
            Self::Other(r, g, b) => format!("other({r},{g},{b})"),
        }
    }
}

/// A window painted one flat colour, so that "is it in the frame" is one pixel read.
struct WdaFixture {
    hwnd: HWND,
}

impl WdaFixture {
    fn create(rect: Rect) -> Result<Self, String> {
        let class = wide("SnapclipWdaProbeFixture");
        let title = wide("snapclip WDA probe fixture");
        // Leaked for the same reason as the wheel fixture: the class keeps the pointer.
        let class = Box::leak(class.into_boxed_slice());
        let brush = unsafe { CreateSolidBrush(WDA_FIXTURE_COLOUR) };
        unsafe {
            RegisterClassW(&WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(DefWindowProcW),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: ptr::null_mut(),
                hIcon: ptr::null_mut(),
                hCursor: ptr::null_mut(),
                hbrBackground: brush,
                lpszMenuName: ptr::null(),
                lpszClassName: class.as_ptr(),
            });
        }
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                rect.left,
                rect.top,
                rect.width(),
                rect.height(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err("CreateWindowExW(WDA probe fixture) returned null".into());
        }
        Ok(Self { hwnd })
    }

    fn client_rect(&self) -> Result<Rect, String> {
        client_rect_of(self.hwnd).ok_or_else(|| "the WDA fixture has no client rect".to_string())
    }

    /// Sets the affinity and returns what the system reports afterwards.
    ///
    /// The read-back matters: `SetWindowDisplayAffinity` can fail (it only accepts a
    /// top-level window of the calling process, and it conflicts with
    /// `UpdateLayeredWindow`), and a failed call would otherwise look exactly like
    /// "WDA had no effect".
    fn set_affinity(&self, affinity: u32) -> Result<u32, String> {
        let accepted = unsafe { SetWindowDisplayAffinity(self.hwnd, affinity) };
        if accepted == 0 {
            return Err(format!(
                "SetWindowDisplayAffinity(0x{affinity:x}) failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut current = 0u32;
        let read = unsafe { GetWindowDisplayAffinity(self.hwnd, &mut current) };
        if read == 0 {
            return Err(format!(
                "GetWindowDisplayAffinity failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(current)
    }
}

impl Drop for WdaFixture {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}

/// Reads one pixel of a monitor capture.
///
/// The capture is display-level on purpose: that is the path WDA is supposed to protect
/// (`docs/30 §24.5`). The window-level path does not need WDA at all — the overlay is a
/// different top-level window and cannot appear in the target's own frames (§24.2 fact 1).
fn monitor_pixel(
    device: &d3d11::GraphicsDevice,
    layout: &monitor::CapturedMonitor,
    point: Point,
) -> Result<(u8, u8, u8), String> {
    let frame = wgc::capture_monitor(device, layout)?;
    let origin = layout.origin();
    let x = point.x - origin.x;
    let y = point.y - origin.y;
    if x < 0 || y < 0 || x as u32 >= frame.width || y as u32 >= frame.height {
        return Err(format!(
            "({}, {}) is outside the captured monitor {}x{} at ({}, {})",
            point.x,
            point.y,
            frame.width,
            frame.height,
            origin.x,
            origin.y
        ));
    }
    let bgra = device.read_back_bgra(&frame.texture)?;
    let index = ((y as u32 * frame.width + x as u32) * 4) as usize;
    let pixel = bgra
        .get(index..index + 4)
        .ok_or_else(|| format!("the readback is shorter than the frame ({index} + 4)"))?;
    Ok((pixel[0], pixel[1], pixel[2]))
}

/// Returns a description of why the screen cannot be measured right now, or `None`.
///
/// This exists because a locked or disconnected session is **indistinguishable from a working
/// WDA** if you only look at the captured pixels: the desktop frame comes back black, and
/// "the window is not in the frame" is exactly what WDA is supposed to achieve. Reporting
/// that as a WDA result would be the worst kind of wrong answer — a negative one that was
/// never tested. So the environment is checked first, and its absence is reported as an
/// environment failure.
fn interactive_desktop_problem() -> Option<String> {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return Some(
            "GetForegroundWindow() returned NULL, which is what a locked or disconnected \
             session looks like"
                .to_string(),
        );
    }
    // BitBlt is the independent check: on a secure desktop it fails with access denied even
    // though the process is running fine.
    match bitblt::capture_rect(Rect::new(0, 0, 8, 8)) {
        Ok(_) => None,
        Err(error) => Some(format!("BitBlt of the screen failed: {error}")),
    }
}

#[test]
#[ignore = "E-CAP-1 extension for OQ-2: needs a real interactive desktop and paints a window on screen"]
fn wda_probe() {
    let _ = monitor::set_per_monitor_v2_awareness();

    if let Some(problem) = interactive_desktop_problem() {
        // Not a silent skip: this is an explicit, loud "the measurement was not obtained",
        // with the evidence attached, because the alternative is a black frame that reads
        // like a successful WDA exclusion.
        panic!(
            "OQ-2 was NOT measured: {problem}. WDA's effect cannot be told apart from a locked \
             desktop, so this probe refuses to report either answer. Run it again with an \
             unlocked, connected desktop."
        );
    }

    // Put the fixture somewhere a desktop is unlikely to be busy, and make it big enough
    // that the sampled centre is unambiguously inside it.
    let fixture = match WdaFixture::create(Rect::new(240, 240, 240 + 420, 240 + 320)) {
        Ok(fixture) => fixture,
        Err(error) => {
            eprintln!("[OQ-2] no fixture window: {error}");
            return;
        }
    };
    let client = match fixture.client_rect() {
        Ok(client) => client,
        Err(error) => {
            eprintln!("[OQ-2] no client rect: {error}");
            return;
        }
    };
    let probe_point = Point::new(
        client.left + client.width() / 2,
        client.top + client.height() / 2,
    );

    // The fixture needs to be *unoccluded*, not focused: WDA is about whether the window
    // appears in a captured frame, and focus has nothing to do with that. So this does not
    // use `bring_to_front` — that one asserts on `GetForegroundWindow`, which the foreground
    // lock refuses when another process owns the foreground (and this probe has no keyboard
    // claim to make, so the Alt-tap escape would be theatre). The verification is instead
    // "the window at the sampled point is the fixture", which is the property that matters.
    unsafe {
        SetWindowPos(
            fixture.hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
    }
    pump_for(Duration::from_millis(500));
    let at_point = unsafe { WindowFromPoint(POINT { x: probe_point.x, y: probe_point.y }) };
    if at_point != fixture.hwnd {
        // Refuse rather than report: if something else is on top of the sampled point, "the
        // fixture is not in the frame" would be true for a reason that has nothing to do
        // with WDA.
        panic!(
            "the window at the sampled point is {at_point:?}, not the fixture {hwnd:?}: something \
             is occluding it, so this probe would measure the occluder instead of WDA",
            hwnd = fixture.hwnd
        );
    }

    let layout = match monitor::captured_monitor_at(probe_point) {
        Ok(layout) => layout,
        Err(error) => {
            eprintln!("[OQ-2] no monitor at the fixture: {error}");
            return;
        }
    };
    let device = match d3d11::GraphicsDevice::create() {
        Ok(device) => device,
        Err(error) => {
            eprintln!("[OQ-2] no D3D11 device: {error}");
            return;
        }
    };

    /// Captures `FRAMES` times and reports how many of them showed the fixture.
    fn sample(
        device: &d3d11::GraphicsDevice,
        layout: &monitor::CapturedMonitor,
        point: Point,
        frames: usize,
    ) -> Vec<WdaVerdict> {
        let mut verdicts = Vec::with_capacity(frames);
        for _ in 0..frames {
            match monitor_pixel(device, layout, point) {
                Ok(pixel) => verdicts.push(WdaVerdict::of(pixel)),
                Err(error) => {
                    eprintln!("[OQ-2] capture failed: {error}");
                    verdicts.push(WdaVerdict::Other(0, 0, 0));
                }
            }
            pump_for(Duration::from_millis(60));
        }
        verdicts
    }

    fn count(verdicts: &[WdaVerdict], wanted: WdaVerdict) -> usize {
        verdicts.iter().filter(|v| **v == wanted).count()
    }

    const FRAMES: usize = 10;

    // Arm 1: control. WDA off — the fixture must be in the frame, or the probe is measuring
    // nothing and every later number would be meaningless.
    if let Err(error) = fixture.set_affinity(WDA_NONE) {
        eprintln!("[OQ-2] WDA_NONE refused: {error}");
        return;
    }
    pump_for(Duration::from_millis(200));
    let control = sample(&device, &layout, probe_point, FRAMES);

    // Arm 2: the question. `WDA_EXCLUDEFROMCAPTURE` needs Win10 2004; before that it
    // degrades to `WDA_MONITOR`, which paints a black block rather than hiding the window
    // (F-22). Both outcomes are distinguishable from `Visible` by construction.
    let excluded = match fixture.set_affinity(WDA_EXCLUDEFROMCAPTURE) {
        Ok(observed) => {
            pump_for(Duration::from_millis(300));
            let verdicts = sample(&device, &layout, probe_point, FRAMES);
            eprintln!(
                "[OQ-2] arm 2: SetWindowDisplayAffinity(0x11) accepted, GetWindowDisplayAffinity reads back 0x{observed:x}"
            );
            verdicts
        }
        Err(error) => {
            eprintln!("[OQ-2] arm 2: {error}");
            Vec::new()
        }
    };

    // Arm 3: control again, after turning WDA back off. Without it, "the fixture vanished
    // in arm 2" could just as well be "the desktop changed under us".
    if let Err(error) = fixture.set_affinity(WDA_NONE) {
        eprintln!("[OQ-2] WDA_NONE refused on the way back: {error}");
        return;
    }
    pump_for(Duration::from_millis(300));
    let restored = sample(&device, &layout, probe_point, FRAMES);

    let describe = |verdicts: &[WdaVerdict]| {
        if verdicts.is_empty() {
            "not run".to_string()
        } else {
            format!(
                "visible {} / black {} / other {}",
                count(verdicts, WdaVerdict::Visible),
                count(verdicts, WdaVerdict::Black),
                verdicts.len() - count(verdicts, WdaVerdict::Visible) - count(verdicts, WdaVerdict::Black)
            )
        }
    };

    eprintln!("[OQ-2] display-level WGC, {} frames per arm, pixel at ({}, {}):", FRAMES, probe_point.x, probe_point.y);
    eprintln!("  arm 1  WDA_NONE                 : {}", describe(&control));
    eprintln!("  arm 2  WDA_EXCLUDEFROMCAPTURE   : {}", describe(&excluded));
    eprintln!("  arm 3  WDA_NONE (restored)      : {}", describe(&restored));
    eprintln!(
        "[OQ-2] verdict: WDA {} the monitor-level capture path",
        if count(&excluded, WdaVerdict::Visible) == 0 {
            "affects"
        } else {
            "does NOT affect"
        }
    );

    // The only assertion: the controls. Arm 2's outcome is the *finding* — OQ-2 exists
    // because nobody knows it — so it is reported, not asserted into either answer.
    assert!(
        count(&control, WdaVerdict::Visible) >= FRAMES - 1,
        "the fixture was not visible in the control arm ({} of {} frames), so this probe cannot \
         tell WDA's effect from a broken fixture: {}",
        count(&control, WdaVerdict::Visible),
        FRAMES,
        control.iter().map(|v| v.name()).collect::<Vec<_>>().join(", ")
    );
    assert!(
        count(&restored, WdaVerdict::Visible) >= FRAMES - 1,
        "the fixture did not come back after WDA_NONE ({} of {} frames), so arm 2's result \
         cannot be attributed to the affinity: {}",
        count(&restored, WdaVerdict::Visible),
        FRAMES,
        restored.iter().map(|v| v.name()).collect::<Vec<_>>().join(", ")
    );
}

// ---------------------------------------------------------------------------
// P3.10 / Cancel latency: the real-desktop Max
// ---------------------------------------------------------------------------

// The visible rectangle is read exactly once, in production:
// `crate::windows::win::window::frame_bounds` (`P7.03`). This file used to carry its own DWM read
// here plus a sibling `GetWindowRect` helper whose doc claimed the *window* rectangle was what a
// window-level WGC capture delivers — contradicting both the production reader and the measurement
// (Chrome: raw 1200 × 900, client 1184 × 892, delivered frame 1188 × 894). Two readers of one
// rectangle are two chances to build a plan from the wrong one, and
// `windows/win/window.rs::the_visible_frame_has_exactly_one_reader` now forbids the second.

/// The client area's screen origin and size, in physical pixels.
///
/// The actuator's `screen` is where the cursor must be for `SendInput` to route the wheel to the
/// target under `MOUSE_POS` routing, and a cursor is placed over the *client* area — the frame's
/// own dimensions come from `crate::windows::win::window::frame_bounds` (`P7.03`).
fn client_geometry(hwnd: HWND) -> (Point, (u32, u32)) {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let size = if unsafe { GetClientRect(hwnd, &mut rect) } != 0 {
        (
            (rect.right - rect.left).max(0) as u32,
            (rect.bottom - rect.top).max(0) as u32,
        )
    } else {
        (0, 0)
    };
    let mut origin = POINT { x: 0, y: 0 };
    let origin = if unsafe { ClientToScreen(hwnd, &mut origin) } != 0 {
        Point::new(origin.x, origin.y)
    } else {
        Point::new(0, 0)
    };
    (origin, size)
}

/// The real actuator, with a witness for "the wheel is out".
///
/// The witness is the whole point of this wrapper. `Cancel latency` is defined as *user presses
/// cancel* → *the driver confirms it stopped* (§23.2), and the worst trigger is a cancel that
/// lands immediately after an injection: the driver is then inside the settle loop, where the
/// injection it already fired cannot be recalled (§21.4). Without a timestamp taken by the
/// actuator itself, the probe would be guessing when that moment was, and a probe that guesses
/// its own trigger point measures its own reaction time.
struct Win32ScrollActuator {
    target: isize,
    screen: Point,
    axis: Axis,
    choice: scroll_actuator::Choice,
    injections: Arc<AtomicU32>,
    injected_at: Arc<Mutex<Option<Instant>>>,
}

impl ScrollActuator for Win32ScrollActuator {
    type Path = InjectPath;

    fn path(&self) -> InjectPath {
        self.choice.path()
    }

    /// Never switch. The watchdog is not under test here, and a probe that changed transports
    /// mid-measurement would report the latency of a different code path than the one it named.
    fn switch(&mut self, _from: InjectPath) -> Option<InjectPath> {
        None
    }

    fn inject(&mut self, notches: i32) -> InjectOutcome {
        let request = InjectRequest {
            target: self.target,
            screen: self.screen,
            notches,
            axis: self.axis,
            path: self.choice.path(),
            aim: self.choice.aim(),
        };
        let outcome = scroll_actuator::inject(&Win32Injection, &request);
        // Stamped after `inject` returns, because that is the moment the wheel is out: the call
        // itself is atomic and cannot be interrupted, so "during the injection" is not a moment a
        // caller can have.
        *self.injected_at.lock().expect("the witness mutex is never poisoned") = Some(Instant::now());
        self.injections.fetch_add(1, Ordering::Release);
        outcome
    }
}

/// The open attempt's state, shared with the trial so it can wait for it and report it.
const OPEN_PENDING: u32 = 0;
const OPEN_OK: u32 = 1;
const OPEN_FAILED: u32 = 2;

/// The window-level WGC source, opened **on the driver thread** and counted.
///
/// Three things are deliberate here.
///
/// * **Opened by a factory, not handed over.** `ScrollRuntime::start` takes a closure that builds
///   the source on the driver thread, because a real source owns a D3D11 device whose immediate
///   context belongs to exactly one thread (`docs/30` §21.3) and a WGC session whose apartment is
///   established by its first activation (§21.5). A pre-built source would be a context with an
///   owner being used by somebody else.
/// * **Opened eagerly, and the open is signalled.** `WgcSession::open` takes real time (measured
///   below at tens of milliseconds), and it belongs to the session's *assembly*, not to its loop.
///   §23.2 defines the trigger as the worst moment **after an injection**, so folding the open into
///   the latency would report a number for a window the metric does not describe. The `opened` flag
///   is how the `parked` shape cancels *after* the open instead of during it.
/// * **Counted.** The probe refuses to report a latency it did not earn, and "the session never
///   received a frame" is the failure mode that would otherwise look like a perfectly good
///   measurement. The counters are what turn a silent capture failure into a failed assertion.
struct DeferredWgcSource {
    opened: Result<WgcFrameSource, FrameError>,
    viewport: Rect,
    frames: Arc<AtomicU32>,
}

impl FrameSource for DeferredWgcSource {
    fn next(&mut self, timeout: Duration) -> Result<Poll, FrameError> {
        match &mut self.opened {
            Ok(source) => {
                let poll = source.next(timeout)?;
                if matches!(poll, Poll::Frame(_)) {
                    self.frames.fetch_add(1, Ordering::AcqRel);
                }
                Ok(poll)
            }
            // Re-reported on every read rather than latched into an ending: a failed open is a
            // transient by the port's own definition, and the probe asserts on the frame count.
            Err(error) => Err(error.clone()),
        }
    }

    fn viewport(&self) -> Rect {
        self.viewport
    }
}

/// Open the real source, on the calling thread, and announce how the attempt went.
fn open_wgc_source(
    handle: isize,
    axis: Axis,
    viewport: Rect,
    state: Arc<AtomicU32>,
    frames: Arc<AtomicU32>,
) -> DeferredWgcSource {
    let opened = d3d11::GraphicsDevice::create()
        .map_err(|detail| FrameError::Transient {
            context: "GraphicsDevice::create",
            detail,
        })
        .and_then(|device| WgcFrameBackend::open(Arc::new(device), handle))
        .map(|backend| WgcFrameSource::new(Box::new(backend), axis));
    state.store(
        if opened.is_ok() { OPEN_OK } else { OPEN_FAILED },
        Ordering::Release,
    );
    DeferredWgcSource {
        opened,
        viewport,
        frames,
    }
}

/// `SPI_GETMOUSEWHEELROUTING` as the actuator's own vocabulary (`P3.02`).
fn routing_of(raw: u32) -> WheelRouting {
    match raw {
        0 => WheelRouting::Focus,
        1 => WheelRouting::Hybrid,
        2 => WheelRouting::MousePosition,
        _ => WheelRouting::Unknown,
    }
}

/// The real desktop a latency probe needs, assembled once and torn down when it drops.
///
/// Two probes need the same preamble: a running Chrome on the demo page, the rectangles the plan and
/// the capture must agree on, the point the wheel is aimed at, and the transport the routing implies.
/// Assembling it twice would be two places for the geometry to drift, and that drift's failure mode
/// is a driver-thread panic inside `canvas.rs`'s cross-axis invariant rather than a readable probe
/// error — `P3.10` paid for that lesson once already.
struct LatencyArena {
    launched: LaunchedTarget,
    scratch: PathBuf,
    target: isize,
    screen: Point,
    choice: scroll_actuator::Choice,
    frame_width: u32,
    frame_height: u32,
}

impl LatencyArena {
    fn open() -> Self {
        let _ = monitor::set_per_monitor_v2_awareness();

        let token = format!(
            "{:08x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        );
        let scratch = std::env::temp_dir().join(format!("snapclip-latency-{token}"));
        std::fs::create_dir_all(&scratch).expect("creating the probe scratch dir");

        let routing = read_mouse_wheel_routing()
            .map(|raw| routing_of(raw))
            .unwrap_or(WheelRouting::Unknown);
        let lines = read_wheel_scroll_lines().unwrap_or(3);
        eprintln!(
            "[latency] SPI_GETMOUSEWHEELROUTING = {routing:?}, SPI_GETWHEELSCROLLLINES = {lines}"
        );

        let launched = launch_scroll_target(ScrollTargetKind::Chrome, &scratch, &token).expect(
            "the latency probes need Chrome: the demo page is the only scrollable target whose \
             pixel-per-notch gain is already measured (E-INJECT-1: 100 px)",
        );
        // `require_focus: false` — Chrome is another process's window, and `SetFocus` is refused for
        // those. Focus is also not what these probes need: both latencies are about how long the
        // driver takes to *notice* a command, and a step whose wheel lands nowhere still costs the
        // same one injection plus one read-back. Demanding focus would refuse to measure the metric
        // on a machine whose routing hands the wheel to the window under the cursor instead.
        bring_to_front(launched.window, false);

        let target = launched.window as isize;
        let (origin, size) = client_geometry(launched.window);
        // The visible frame has exactly one reader in this crate (`P7.03`): production's
        // `frame_bounds`. It is the rectangle a window-level WGC capture delivers, so it is the
        // only one a plan may be built from (`canvas.rs:1154`, invariant 1).
        let frame = crate::windows::win::window::frame_bounds(target).expect(
            "DWM must report the window's visible bounds: they are the size a window-level WGC \
             capture delivers, and the session's canvas is built from them (canvas.rs:1154)",
        );
        let frame_width = frame.width().max(0) as u32;
        let frame_height = frame.height().max(0) as u32;
        let screen = Point::new(origin.x + size.0 as i32 / 2, origin.y + size.1 as i32 / 2);

        // A self-launched target has the same integrity level as its launcher, so the two fields the
        // choice table reads are equal by construction and the `target_is_elevated &&
        // !self_is_elevated` arm is unreachable. This is not an assumption about the machine —
        // `EnableLUA = 0` here, and the probe does not need to know.
        let probe = TargetProbe {
            target_is_elevated: false,
            self_is_elevated: false,
            target_is_foreground: unsafe { GetForegroundWindow() } == launched.window,
            routing,
        };
        let choice = scroll_actuator::choose(&probe);
        eprintln!(
            "[latency] target 0x{target:x} class {} frame {frame_width}x{frame_height} \
             client ({},{})-({},{}); choice {:?} + {:?} (foreground = {})",
            launched.class,
            origin.x,
            origin.y,
            origin.x + size.0 as i32,
            origin.y + size.1 as i32,
            choice.path(),
            choice.aim(),
            probe.target_is_foreground
        );

        Self {
            launched,
            scratch,
            target,
            screen,
            choice,
            frame_width,
            frame_height,
        }
    }

    /// The plan a session actually gets, and the one calibrated to `E-INJECT-1`'s measured
    /// 100 px/notch for Chromium.
    ///
    /// The default starts at `ĝ₀ = 60 px/notch`, which is what a session actually gets; the
    /// calibrated one matches the 100 px/notch this machine measured, which is what makes the steps
    /// commit at all (R-25 / OQ-22). Built per trial rather than cloned: `ScrollPlan` is moved into
    /// the runtime, and giving it a `Clone` only for a probe would be a production trait bound
    /// earned by a test.
    fn plan(&self, calibrated: bool) -> ScrollPlan {
        let plan = ScrollPlan::new(
            Axis::Vertical,
            self.frame_width as u64,
            self.frame_height,
            MemoryBudget::for_viewport(self.frame_width as u64, self.frame_height as u64),
        );
        if calibrated {
            plan.with_wheel(3, 33)
        } else {
            plan
        }
    }
}

impl Drop for LatencyArena {
    fn drop(&mut self) {
        shutdown_target(&mut self.launched);
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

/// One trial's result, and the only shape the report is allowed to have.
struct CancelTrial {
    shape: &'static str,
    opened: bool,
    frames: u32,
    injections: u32,
    committed: u32,
    stop: Option<StopReason>,
    latency: Option<Duration>,
}

/// `docs/31` `P3.10` / `E-PERF-5`: `Cancel latency`'s Max on a real desktop.
///
/// `P3.07` measured the loop's own contribution against a scripted clock (max 20 ms over seven
/// trigger points) and recorded, as a deviation, that the *real* Max needs the two segments the
/// loop cannot interrupt — one injection and one read-back — whose true cost only a real desktop
/// can produce. This is that measurement, and it is the one performance gate the `P3` stage may
/// not skip.
///
/// Two trigger shapes are run, because they are different claims:
///
/// * **`mid-flight`** — cancel lands immediately after an injection returned. This is the worst
///   trigger the loop can be handed: the step is already out, so the latency includes finishing
///   the settle that step started.
/// * **`parked`** — cancel lands before any injection. The driver is in `await_first_frame` or at
///   the top of its loop, where the only thing between the press and the confirmation is one tick.
///
/// The assertions are the two the metric actually promises (§23.3): `Max ≤ 400 ms` (design) and
/// `Max ≤ 500 ms` (acceptance). `P50 ≤ 60 ms` is reported, not asserted — a distribution with
/// fewer than ten samples has no business claiming a percentile, and the probe says so in its
/// output rather than by quietly rounding up.
#[test]
#[ignore = "P3.10: needs a real interactive desktop and a scrollable target"]
fn cancel_latency_probe() {
    let arena = LatencyArena::open();

    let mut trials: Vec<CancelTrial> = Vec::new();
    for (plan_label, calibrated) in [("default ĝ₀=60", false), ("calibrated ĝ₀=99", true)] {
        for shape in ["mid-flight", "parked"] {
            let trial = run_cancel_trial(&arena, calibrated, shape);
            eprintln!(
                "[P3.10] {plan_label:<16} {shape:<10} opened {} frames {:>2} injections {:>2} \
                 committed {:>2} stop {:?} latency {}",
                trial.opened,
                trial.frames,
                trial.injections,
                trial.committed,
                trial.stop,
                match trial.latency {
                    Some(latency) => format!("{} ms", latency.as_millis()),
                    None => "NOT MEASURED".to_string(),
                }
            );
            trials.push(trial);
        }
    }

    // Tearing the arena down here rather than at the end of the function keeps the numbers the
    // assertions read independent of how long shutting Chrome down takes.
    drop(arena);

    let mut measured: Vec<Duration> = trials.iter().filter_map(|trial| trial.latency).collect();
    let report = || {
        trials
            .iter()
            .map(|trial| {
                format!(
                    "{} opened={} frames={} injections={}",
                    trial.shape, trial.opened, trial.frames, trial.injections
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    // The guard the probe must never drop: a session that never captured would report a latency
    // that measures nothing, and it would look exactly like a fast one. This is checked per shape
    // because the two shapes legitimately differ — `parked` cancels before any injection, so zero
    // frames is its *definition*, while `mid-flight` cannot be claimed unless a wheel actually went
    // out and a frame actually came back.
    assert!(
        trials.iter().all(|trial| trial.opened),
        "every trial must have opened a real WGC session, or the latency it reports belongs to a \
         session that was never capturing: {}",
        report()
    );
    assert!(
        trials
            .iter()
            .filter(|trial| trial.shape == "mid-flight")
            .all(|trial| trial.injections >= 1 && trial.frames >= 1),
        "a `mid-flight` trial must have injected a wheel and received a frame back, or its trigger \
         point was never reached: {}",
        report()
    );
    assert!(
        trials
            .iter()
            .filter(|trial| trial.shape == "parked")
            .all(|trial| trial.injections == 0),
        "a `parked` trial must cancel before the first injection, or it is a `mid-flight` trial \
         wearing the wrong label: {}",
        report()
    );
    assert_eq!(
        measured.len(),
        trials.len(),
        "every cancelled session must carry its latency: {} of {} trials reported one — a `None` \
         here is the metric being absent, not the latency being zero (§23.2)",
        measured.len(),
        trials.len()
    );
    measured.sort();
    let max = *measured.last().expect("at least one trial ran");
    let p50 = measured[measured.len() / 2];

    eprintln!(
        "[P3.10] Cancel latency over {} trials: max = {} ms, p50 = {} ms (thresholds: max ≤ 400 ms \
         design, ≤ 500 ms acceptance; p50 ≤ 60 ms target)",
        measured.len(),
        max.as_millis(),
        p50.as_millis()
    );
    eprintln!(
        "[P3.10] note: {} samples is not a distribution, so p50 is reported and NOT asserted",
        measured.len()
    );

    assert!(
        max <= Duration::from_millis(400),
        "§23.3's design target for Cancel latency is Max ≤ 400 ms; measured {max:?}"
    );
    assert!(
        max <= Duration::from_millis(500),
        "§23.3's acceptance threshold for Cancel latency is Max ≤ 500 ms; measured {max:?}"
    );
}

/// One trial: open a session, wait for the trigger moment, cancel, collect the session.
fn run_cancel_trial(arena: &LatencyArena, calibrated: bool, shape: &'static str) -> CancelTrial {
    let plan = arena.plan(calibrated);
    let cross = plan.cross_len();
    let extent = plan.viewport_extent();
    let viewport = Rect::new(0, 0, cross as i32, extent as i32);
    let frames = Arc::new(AtomicU32::new(0));
    let state = Arc::new(AtomicU32::new(OPEN_PENDING));
    let source_frames = Arc::clone(&frames);
    let source_state = Arc::clone(&state);
    let target = arena.target;
    let make_source =
        move || open_wgc_source(target, Axis::Vertical, viewport, source_state, source_frames);

    let injections = Arc::new(AtomicU32::new(0));
    let injected_at = Arc::new(Mutex::new(None));
    let actuator = Win32ScrollActuator {
        target: arena.target,
        screen: arena.screen,
        axis: Axis::Vertical,
        choice: arena.choice,
        injections: Arc::clone(&injections),
        injected_at: Arc::clone(&injected_at),
    };

    let mut runtime = ScrollRuntime::start(plan, make_source, actuator);

    wait_for_trigger(&state, &injections, shape);
    runtime.controller().cancel();

    let session = runtime
        .teardown()
        .expect("a cancelled driver hands its session back");
    CancelTrial {
        shape,
        opened: state.load(Ordering::Acquire) == OPEN_OK,
        frames: frames.load(Ordering::Acquire),
        injections: injections.load(Ordering::Acquire),
        committed: session.committed(),
        stop: session.stop_reason(),
        latency: session.cancel_latency(),
    }
}

/// Wait for the moment a latency probe wants to press its key at.
///
/// Both shapes spin rather than sleep, and for the same reason: a sleep long enough to be reliable
/// is also long enough to let the thing being measured finish.
///
/// * **`mid-flight`** — the first injection has returned. This is the worst trigger the loop can be
///   handed: the step is already out, so the latency includes finishing the settle that step began.
/// * **`parked`** — the session is up and no injection has gone out. The only thing between the
///   press and the confirmation is one tick. Waiting on the open flag rather than on a fixed sleep is
///   what keeps the session-open cost out of the number: §23.2's trigger is a moment *after* the
///   session is up, not during its assembly.
///
/// The deadline only exists so a broken fixture fails instead of hanging.
fn wait_for_trigger(state: &AtomicU32, injections: &AtomicU32, shape: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    match shape {
        "mid-flight" => {
            while injections.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
                std::hint::spin_loop();
            }
        }
        _ => {
            while state.load(Ordering::Acquire) == OPEN_PENDING && Instant::now() < deadline {
                std::hint::spin_loop();
            }
        }
    }
}

/// One stop trial's result.
struct StopTrial {
    shape: &'static str,
    opened: bool,
    frames: u32,
    injections: u32,
    committed: u32,
    stop: Option<StopReason>,
    /// `disposal()` was `Export` — the promise `stop` makes about the pixels (§20.5).
    exported: bool,
    latency: Option<Duration>,
}

/// `docs/31` `P6.08`: `Stop latency`'s real-desktop number, which §23.3's cell says it is owed.
///
/// ## What the synthetic number left out
///
/// §23.3.4 measured `3.5–52.0 ms` against a scripted page whose `SimulatedPage::next` returns
/// immediately and never blocks for a tick, and the cell says so: that number is the loop's own
/// contribution. A real session pays two more segments the fixture cannot have — one WGC poll and
/// one read-back of the viewport. §23.3.3 measured the read-back alone at 41–44 ms, so the real
/// number should be that much larger; this probe reports whether it is.
///
/// ## The endpoint, and why it is the caller's
///
/// `ScrollController::stop` deliberately does **not** stamp an instant, unlike `cancel`, whose
/// instant exists because §21.4 needs it to detect staleness and §23.2 needs it as the *user's*
/// moment. So this probe times its own press: `stop()` on the probe thread, then `teardown()`
/// returning — the driver thread is gone and the session is back, which is the moment §23.2's
/// "the export can be submitted" becomes true.
///
/// Two costs are inside that interval and are written down rather than smoothed over: the thread
/// join (microseconds) and whatever remained of the step in flight at the moment of the press
/// (usually its settle). The second one is the quantity itself — `stop` cannot recall an injection
/// any more than `cancel` can — so it is not an artifact.
///
/// ## What is asserted
///
/// Only the structure: every trial opened a real WGC session, `mid-flight` pressed *after* an
/// injection and got a frame back, `parked` pressed before any injection, every session reports
/// `UserStopped` and would export, and every latency is present (`None` is the metric being absent,
/// not a fast stop). The comparison against §23.3's `P50 ≤ 20 ms` / `P95 ≤ 60 ms` is **reported**,
/// not asserted: four samples is not a distribution, and the same rule is why `cancel`'s `p50` is
/// reported too. The one ceiling that is asserted is physical — a stop cannot cost more than the
/// step in flight (`STEP_TIMEOUT = 400 ms`) plus the read-back that ends it (44 ms), so 500 ms is
/// the number past which the loop is not merely unlucky but wrong.
#[test]
#[ignore = "P6.08: needs a real interactive desktop and a scrollable target"]
fn stop_latency_probe() {
    let arena = LatencyArena::open();

    let mut trials: Vec<StopTrial> = Vec::new();
    for (plan_label, calibrated) in [("default ĝ₀=60", false), ("calibrated ĝ₀=99", true)] {
        for shape in ["mid-flight", "parked"] {
            let trial = run_stop_trial(&arena, calibrated, shape);
            eprintln!(
                "[P6.08] {plan_label:<16} {shape:<10} opened {} frames {:>2} injections {:>2} \
                 committed {:>2} stop {:?} exported {} latency {}",
                trial.opened,
                trial.frames,
                trial.injections,
                trial.committed,
                trial.stop,
                trial.exported,
                match trial.latency {
                    Some(latency) => format!("{} ms", latency.as_millis()),
                    None => "NOT MEASURED".to_string(),
                }
            );
            trials.push(trial);
        }
    }

    drop(arena);

    let mut measured: Vec<Duration> = trials.iter().filter_map(|trial| trial.latency).collect();
    let report = || {
        trials
            .iter()
            .map(|trial| {
                format!(
                    "{} opened={} frames={} injections={} stop={:?} exported={}",
                    trial.shape,
                    trial.opened,
                    trial.frames,
                    trial.injections,
                    trial.stop,
                    trial.exported
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    };

    assert!(
        trials.iter().all(|trial| trial.opened),
        "every trial must have opened a real WGC session, or the latency it reports belongs to a \
         session that was never capturing: {}",
        report()
    );
    assert!(
        trials
            .iter()
            .filter(|trial| trial.shape == "mid-flight")
            .all(|trial| trial.injections >= 1 && trial.frames >= 1),
        "a `mid-flight` stop must land after a wheel went out and a frame came back, or its trigger \
         point was never reached: {}",
        report()
    );
    assert!(
        trials
            .iter()
            .filter(|trial| trial.shape == "parked")
            .all(|trial| trial.injections == 0),
        "a `parked` stop must land before the first injection, or it is a `mid-flight` trial wearing \
         the wrong label: {}",
        report()
    );
    assert!(
        trials
            .iter()
            .all(|trial| trial.stop == Some(StopReason::UserStopped)),
        "every trial pressed `stop`, so every session must report `UserStopped`: {}",
        report()
    );
    assert!(
        trials.iter().all(|trial| trial.exported),
        "`stop` promises the pixels go to the export path (§20.5), so `disposal()` must be `Export` \
         and never `Discard`: {}",
        report()
    );
    assert_eq!(
        measured.len(),
        trials.len(),
        "every stopped session must carry its latency: {} of {} trials reported one — a `None` here \
         is the metric being absent, not the latency being zero (§23.2)",
        measured.len(),
        trials.len()
    );
    measured.sort();
    let max = *measured.last().expect("at least one trial ran");
    let p50 = measured[measured.len() / 2];
    let share = |limit: u128| (max.as_millis() * 100) / limit;

    eprintln!(
        "[P6.08] Stop latency over {} trials: max = {} ms, p50 = {} ms (targets: P50 ≤ 20 ms, \
         P95 ≤ 60 ms; the synthesised loop-only number was 3.5–52.0 ms, §23.3.4)",
        measured.len(),
        max.as_millis(),
        p50.as_millis()
    );
    if max > Duration::from_millis(60) {
        eprintln!(
            "[P6.08] note: max {} ms is {}% of the P95 ≤ 60 ms threshold — this is the segment the \
             synthesized number could not contain",
            max.as_millis(),
            share(60)
        );
    }
    eprintln!(
        "[P6.08] note: {} samples is not a distribution, so p50 is reported and NOT asserted",
        measured.len()
    );

    assert!(
        max <= Duration::from_millis(500),
        "a stop costs at most the step in flight (STEP_TIMEOUT = 400 ms) plus the read-back that \
         ends it (44 ms, §23.3.3); measured {max:?}"
    );
}

/// One stop trial: open a session, wait for the trigger moment, ask for the result, collect it.
///
/// The latency is timed here rather than read off the session because there is nothing to read:
/// `stop` has no instant of its own (see the probe's doc for why that is deliberate).
fn run_stop_trial(arena: &LatencyArena, calibrated: bool, shape: &'static str) -> StopTrial {
    let plan = arena.plan(calibrated);
    let cross = plan.cross_len();
    let extent = plan.viewport_extent();
    let viewport = Rect::new(0, 0, cross as i32, extent as i32);
    let frames = Arc::new(AtomicU32::new(0));
    let state = Arc::new(AtomicU32::new(OPEN_PENDING));
    let source_frames = Arc::clone(&frames);
    let source_state = Arc::clone(&state);
    let target = arena.target;
    let make_source =
        move || open_wgc_source(target, Axis::Vertical, viewport, source_state, source_frames);

    let injections = Arc::new(AtomicU32::new(0));
    let injected_at = Arc::new(Mutex::new(None));
    let actuator = Win32ScrollActuator {
        target: arena.target,
        screen: arena.screen,
        axis: Axis::Vertical,
        choice: arena.choice,
        injections: Arc::clone(&injections),
        injected_at: Arc::clone(&injected_at),
    };

    let mut runtime = ScrollRuntime::start(plan, make_source, actuator);

    wait_for_trigger(&state, &injections, shape);
    let pressed_at = Instant::now();
    runtime.controller().stop();
    let session = runtime
        .teardown()
        .expect("a stopped driver hands its session back");
    let latency = Instant::now().saturating_duration_since(pressed_at);

    StopTrial {
        shape,
        opened: state.load(Ordering::Acquire) == OPEN_OK,
        frames: frames.load(Ordering::Acquire),
        injections: injections.load(Ordering::Acquire),
        committed: session.committed(),
        stop: session.stop_reason(),
        exported: matches!(session.disposal(), Some(Disposal::Export(_))),
        latency: Some(latency),
    }
}


