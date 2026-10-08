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

use std::path::{Path, PathBuf};
use std::process::Child;
use std::ptr;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    BOOL, FALSE, HWND, LPARAM, LRESULT, POINT, RECT, TRUE, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect,
    PAINTSTRUCT,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput, SetFocus, VK_DOWN,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CWP_SKIPINVISIBLE, ChildWindowFromPointEx, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, ES_AUTOVSCROLL, ES_MULTILINE, EnumWindows, GetClassNameW, GetClientRect,
    GetCursorPos, GetForegroundWindow, GetWindowTextW, HWND_TOPMOST, IsWindowVisible, MSG,
    PM_REMOVE, PeekMessageW, PostMessageW, RegisterClassW, SB_LINEDOWN, SW_RESTORE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_SHOWWINDOW, SendMessageW, SetCursorPos, SetForegroundWindow, SetWindowPos,
    ShowWindow, TranslateMessage, WM_ERASEBKGND, WM_MOUSEWHEEL, WM_PAINT, WM_VSCROLL, WNDCLASSW,
    WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL,
};

use crate::geometry::{Point, Rect};
use crate::windows::monitor;
use crate::windows::win::bitblt;

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
struct WindowInfo {
    hwnd: HWND,
    title: String,
    class: String,
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

fn visible_windows() -> Vec<WindowInfo> {
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
fn pump_for(duration: Duration) {
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

fn capture_signature(client: Rect) -> Result<Vec<f64>, String> {
    let bitmap = bitblt::capture_rect(client)?;
    Ok(row_signature(&bitmap))
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

fn find_chromium() -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os("CHROME_PATH") {
        let path = PathBuf::from(configured);
        if path.is_file() {
            return Some(path);
        }
    }
    [
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
}

/// The Chromium fixture is a stack of randomly sized, randomly lit bars.
///
/// Text would be more realistic and much worse to measure: a text line is
/// ~13 px of glyphs followed by ~6 px of leading, so the row signal carries a
/// strong carrier at the line height, and a large scroll then reports a
/// confident *spurious* peak on that carrier instead of the true displacement.
/// Bars with random heights (4–40 px) remove the carrier: the row series is a
/// random step function, and a shift of any size is either the truth or
/// nothing.
fn write_chromium_fixture(dir: &Path, token: &str) -> Result<PathBuf, String> {
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
         <title>snapclip-inject-{token}</title>\
         <style>html,body{{margin:0;padding:0;background:#fff}}\
         div{{margin:0;padding:0}}</style>\
         </head><body>{bars}</body></html>"
    );
    let path = dir.join("fixture.html");
    std::fs::write(&path, html).map_err(|error| format!("writing {path:?} failed: {error}"))?;
    Ok(path)
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
        transport: match transport {
            Transport::SendInput => "SendInput",
            Transport::PostMessage => "PostMessageW",
            Transport::ArrowDown => "SendInput(down-key)",
        },
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
            if unsafe { SetCursorPos(center.x, center.y) } == FALSE {
                outcome.error = Some("SetCursorPos failed".into());
                return outcome;
            }
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
                if unsafe { SetCursorPos(center.x, center.y) } == FALSE {
                    outcome.error = Some("SetCursorPos failed".into());
                    return outcome;
                }
                send_input_notches(1)
            }
            Transport::ArrowDown => send_input_keys(VK_DOWN, 1),
            Transport::PostMessage => post_message_notches(root, center, space, 1),
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
        let html = write_chromium_fixture(&scratch, &token).expect("writing the fixture");
        let profile = scratch.join("chromium-profile");
        let child = launch_chromium(executable, &file_url(&html), &profile);
        (child, html)
    });
    match chromium_arm {
        Some((Ok(mut child), html)) => {
            let title_token = format!("snapclip-inject-{token}");
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
        Some((Err(error), _)) => panic!("Chromium could not be launched: {error}"),
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

impl Transport {
    fn label(self) -> &'static str {
        match self {
            Transport::SendInput => "SendInput",
            Transport::PostMessage => "PostMessageW",
            Transport::ArrowDown => "SendInput(down-key)",
        }
    }
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
