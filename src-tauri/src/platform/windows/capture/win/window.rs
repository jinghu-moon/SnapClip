//! Win32 window enumeration, attribute reads and DWM frame bounds.
//!
//! Pure FFI: this module answers questions about one window and never decides policy.
//! Whether a window *should* be a snap candidate is
//! `capture::window_detection::snapshot`'s job; keeping the decision out of here means
//! the filter matrix can be unit tested without a desktop, and this module can be
//! tested against fixtures without re-implementing the policy.
//!
//! Two rules come straight from the design (docs/14 §5.3) and are enforced by the
//! shape of the API:
//!
//! * the `EnumWindows` callback performs **only** cheap, user-mode checks — no DWM,
//!   no cross-process compositor round trip;
//! * DWM reads happen in [`read_dwm_batch`], after the callback returns.

use std::mem::size_of;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ::windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use ::windows::core::BOOL;
use ::windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use ::windows::Win32::Graphics::Gdi::ScreenToClient;
use ::windows::Win32::UI::WindowsAndMessaging::{
    CWP_SKIPINVISIBLE, ChildWindowFromPointEx, EnumChildWindows, EnumWindows,
    GA_PARENT, GWL_EXSTYLE, GetAncestor, GetClassNameW, GetDesktopWindow, GetWindowLongPtrW, GetWindowRect,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
    WINDOW_EX_STYLE, WS_EX_LAYERED, WS_EX_TRANSPARENT,
};

use crate::capture::geometry::{Point, Rect};
use crate::capture::window_detection::snapshot::{CheapProbe, DwmRead};

/// Win32 class names are at most 256 characters including the terminator.
const CLASS_NAME_CAPACITY: usize = 256;

/// Whether the extended style marks a genuine click-through window.
///
/// Only `WS_EX_LAYERED` **and** `WS_EX_TRANSPARENT` together pass mouse input to the
/// window below; `WS_EX_TRANSPARENT` alone only affects paint ordering, and an
/// ordinary layered window is still interactive. Treating either bit alone as
/// "click-through" would hide real windows — the reference selector documents the
/// same conclusion, including the shell's full-desktop handwriting canvas
/// (`0x0a08_00a8`), which is both bits plus more.
pub fn is_click_through_layered(extended_style: u32) -> bool {
    WINDOW_EX_STYLE(extended_style).contains(WS_EX_LAYERED | WS_EX_TRANSPARENT)
}

/// Asks a window to answer `WM_NCHITTEST` with `HTTRANSPARENT` while a query runs.
///
/// UI Automation's point hit test has no notion of Z order, and the capture overlay covers the
/// desktop: with the overlay in the way the precision top-up (docs/21 §5.7) was answered by our own
/// window for **every** query, so it did nothing in the product while it worked in the probes —
/// which is also why the app and the probe disagreed for a whole round (docs/21 §5.2, §5.8).
///
/// The levers were measured on a stand-in carrying the overlay's own shape (topmost, full screen,
/// tool window, no redirection bitmap, its own registered class, `browser_element_probe`'s
/// `[overlay]` phase):
///
/// | lever | UIA's answer |
/// | --- | --- |
/// | none (overlay in the way) | the overlay |
/// | `WS_EX_TRANSPARENT` | the overlay — **does not work** |
/// | `WS_EX_LAYERED \| WS_EX_TRANSPARENT` | the page (but a DirectComposition window cannot be layered) |
/// | `HTTRANSPARENT` from `WM_NCHITTEST` | **the page — the lever we use** |
/// | a hole in the window region | the overlay — does not work |
///
/// So the overlay's own window procedure decides, and this flag is the only thing that crosses
/// threads: a plain atomic the procedure reads. The guard is scoped to the single accessibility call
/// it wraps, because while it is set a real mouse click would also fall through to the window below —
/// the exposure is that call (single-digit milliseconds), not the whole query (20–50 ms measured,
/// i.e. long enough to swallow the click a user makes just after the cursor stops).
#[derive(Debug, Clone, Default)]
pub struct HitTestPassThrough(Arc<AtomicBool>);

impl HitTestPassThrough {
    /// Whether the window should currently let the hit test through to what is below it.
    pub fn is_active(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Pass hit tests through until the returned guard is dropped.
    pub fn guard(&self) -> HitTestPassThroughGuard {
        self.0.store(true, Ordering::Relaxed);
        HitTestPassThroughGuard(Arc::clone(&self.0))
    }
}

/// Clears [`HitTestPassThrough`] when dropped.
pub struct HitTestPassThroughGuard(Arc<AtomicBool>);

impl Drop for HitTestPassThroughGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

/// `IsWindow`.
pub fn is_window(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe { IsWindow(Some(to_hwnd(hwnd))).as_bool() }
}

/// `IsWindowVisible`.
pub fn is_window_visible(hwnd: isize) -> bool {
    is_window(hwnd) && unsafe { IsWindowVisible(to_hwnd(hwnd)).as_bool() }
}

/// `IsIconic`.
pub fn is_iconic(hwnd: isize) -> bool {
    is_window(hwnd) && unsafe { IsIconic(to_hwnd(hwnd)).as_bool() }
}

/// `GetWindowThreadProcessId`, or `0` when the window is gone.
pub fn process_id(hwnd: isize) -> u32 {
    if !is_window(hwnd) {
        return 0;
    }
    let mut process_id = 0u32;
    unsafe { GetWindowThreadProcessId(to_hwnd(hwnd), Some(&mut process_id)) };
    process_id
}

/// `GetClassNameW`, or `None` when the window is gone or has no class.
pub fn class_name(hwnd: isize) -> Option<String> {
    if !is_window(hwnd) {
        return None;
    }
    let mut buffer = [0u16; CLASS_NAME_CAPACITY];
    let written = unsafe { GetClassNameW(to_hwnd(hwnd), &mut buffer) };
    if written <= 0 {
        return None;
    }
    let length = (written as usize).min(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..length]))
}

/// `DWMWA_CLOAKED`.
///
/// A cloaked window is one the compositor is not showing — a closed store app, a
/// window on another virtual desktop, the Snap Assist shadow copy. `IsWindowVisible`
/// still returns true for these, so this check is what keeps invisible ghost windows
/// out of the snapshot. A failed DWM call is reported as "not cloaked": an unreadable
/// attribute must not be allowed to look like a rejection the caller can rely on.
pub fn is_cloaked(hwnd: isize) -> bool {
    if !is_window(hwnd) {
        return false;
    }
    let mut cloaked = 0u32;
    let result = unsafe {
        DwmGetWindowAttribute(
            to_hwnd(hwnd),
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut core::ffi::c_void,
            size_of::<u32>() as u32,
        )
    };
    result.is_ok() && cloaked != 0
}

/// The visible window frame, in virtual-desktop physical pixels.
///
/// `DWMWA_EXTENDED_FRAME_BOUNDS` is the frame the user actually sees: it includes the
/// title bar but excludes the invisible resize border that `GetWindowRect` adds on
/// Windows 10/11 (roughly 7-8 px per edge). Using the raw rectangle would make the
/// highlight sit visibly outside the window.
///
/// `GetWindowRect` is a fallback only — when DWM fails or reports an empty rectangle
/// the check is repeated on the fallback value, so a failure cannot produce a
/// zero-size "window" that then fails somewhere else.
pub fn frame_bounds(hwnd: isize) -> Option<Rect> {
    if !is_window(hwnd) {
        return None;
    }
    let window = to_hwnd(hwnd);

    let mut extended = RECT::default();
    let result = unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut extended as *mut RECT as *mut core::ffi::c_void,
            size_of::<RECT>() as u32,
        )
    };
    if result.is_ok() {
        let bounds = to_rect(extended);
        if !bounds.is_empty() {
            return Some(bounds);
        }
    }

    let mut fallback = RECT::default();
    if unsafe { GetWindowRect(window, &mut fallback) }.is_err() {
        return None;
    }
    let bounds = to_rect(fallback);
    (!bounds.is_empty()).then_some(bounds)
}

/// Enumerate this desktop's visible, non-minimised, non-click-through top-level
/// windows in Z order (frontmost first).
///
/// Every check inside the callback is a cheap user-mode call. DWM reads are
/// deliberately deferred to [`read_dwm_batch`]: `DwmGetWindowAttribute` is a
/// synchronous cross-process call, and doing it per window inside the callback makes
/// enumeration time grow with the compositor's response time.
pub fn enumerate_cheap_candidates() -> Result<Vec<CheapProbe>, String> {
    let mut probes: Vec<CheapProbe> = Vec::with_capacity(128);
    let result = unsafe {
        EnumWindows(
            Some(enum_cheap_proc),
            LPARAM(&mut probes as *mut Vec<CheapProbe> as isize),
        )
    };
    result.map_err(|error| super::hresult("EnumWindows", &error))?;
    Ok(probes)
}

unsafe extern "system" fn enum_cheap_proc(window: HWND, lparam: LPARAM) -> BOOL {
    let probes = unsafe { &mut *(lparam.0 as *mut Vec<CheapProbe>) };
    if let Some(probe) = unsafe { probe_if_cheap(window) } {
        probes.push(probe);
    }
    // Always continue: a rejected window is not an error, and stopping on the first
    // rejection would silently truncate the snapshot.
    BOOL(1)
}

/// The cheap checks, plus reading the class name so the caller's policy pass can match
/// shell surfaces exactly.
unsafe fn probe_if_cheap(window: HWND) -> Option<CheapProbe> {
    if window.is_invalid() {
        return None;
    }
    if !unsafe { IsWindow(Some(window)) }.as_bool() {
        return None;
    }
    if !unsafe { IsWindowVisible(window) }.as_bool() {
        return None;
    }
    if unsafe { IsIconic(window) }.as_bool() {
        return None;
    }
    let style = unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) } as u32;
    if is_click_through_layered(style) {
        return None;
    }
    let mut process_id = 0u32;
    unsafe { GetWindowThreadProcessId(window, Some(&mut process_id)) };
    let class_name = class_name_of(window).unwrap_or_default();
    Some(CheapProbe::new(
        window.0 as isize,
        process_id,
        class_name,
        style,
    ))
}

fn class_name_of(window: HWND) -> Option<String> {
    let mut buffer = [0u16; CLASS_NAME_CAPACITY];
    let written = unsafe { GetClassNameW(window, &mut buffer) };
    if written <= 0 {
        return None;
    }
    let length = (written as usize).min(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..length]))
}

/// Visible child-window rectangles of `parent`, clipped to `parent_bounds`.
///
/// Accessibility providers miss older and custom-drawn controls, but their child *windows*
/// are still enumerable. This is the provider-free half of the deep-selection fallback
/// (docs/18 §12.2), so it stays a pure Win32 read with no COM involved.
///
/// Children equal to the parent's own rectangle are dropped (they add no level), as are
/// degenerate and fully-outside rectangles. The result is ordered smallest-first so callers
/// can treat it as "most specific wins".
pub fn visible_child_rects(parent: isize, parent_bounds: Rect) -> Vec<Rect> {
    if !is_window(parent) {
        return Vec::new();
    }
    struct Collector {
        bounds: Rect,
        rects: Vec<Rect>,
    }

    unsafe extern "system" fn collect(window: HWND, lparam: LPARAM) -> BOOL {
        if !unsafe { IsWindowVisible(window) }.as_bool() {
            return BOOL(1);
        }
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(window, &mut rect) }.is_err() {
            return BOOL(1);
        }
        let collector = unsafe { &mut *(lparam.0 as *mut Collector) };
        let clipped = to_rect(rect).intersect(collector.bounds);
        if clipped.is_empty() || clipped == collector.bounds {
            return BOOL(1);
        }
        collector.rects.push(clipped);
        BOOL(1)
    }

    let mut collector = Collector {
        bounds: parent_bounds,
        rects: Vec::new(),
    };
    unsafe {
        let _ = EnumChildWindows(
            Some(to_hwnd(parent)),
            Some(collect),
            LPARAM(&mut collector as *mut Collector as isize),
        );
    }
    collector.rects.sort_unstable_by_key(|rect| {
        (rect.area(), rect.left, rect.top, rect.right, rect.bottom)
    });
    collector.rects.dedup();
    collector.rects
}

/// Whether child window `hwnd` is the one its parent shows at screen `point`.
///
/// Sibling child windows can share a rectangle and all be `WS_VISIBLE` while only the topmost
/// is actually on screen: File Explorer keeps one full-size `ShellTabWindowClass` per tab and
/// shows the active one by z-order alone. `ChildWindowFromPointEx` asks the window manager,
/// which resolves exactly that, so an occluded sibling is told apart from the visible one
/// without guessing from geometry. `WS_EX_TRANSPARENT` alone does not hide a window (see
/// [`is_click_through_layered`]), so only invisible windows are skipped.
pub fn is_shown_child_at(hwnd: isize, point: Point) -> bool {
    if !is_window(hwnd) {
        return false;
    }
    let child = to_hwnd(hwnd);
    let parent = unsafe { GetAncestor(child, GA_PARENT) };
    if parent.is_invalid() {
        return false;
    }
    // Top-level windows (owned popups) are not siblings of anything this check can rank; asking
    // the desktop would answer with whatever top-level window is above, the overlay included.
    if parent == unsafe { GetDesktopWindow() } {
        return true;
    }
    let mut local = POINT {
        x: point.x,
        y: point.y,
    };
    if !unsafe { ScreenToClient(parent, &mut local) }.as_bool() {
        return false;
    }
    let shown = unsafe { ChildWindowFromPointEx(parent, local, CWP_SKIPINVISIBLE) };
    shown == child
}

/// Read the DWM attributes for a batch of candidates.
///
/// Runs after enumeration, on the detection worker. Keeping it a separate entry point
/// is what makes "no DWM inside the callback" checkable in review rather than a
/// convention.
pub fn read_dwm_batch(hwnds: &[isize]) -> Vec<DwmRead> {
    hwnds
        .iter()
        .map(|&hwnd| DwmRead::new(hwnd, is_cloaked(hwnd), frame_bounds(hwnd)))
        .collect()
}

fn to_hwnd(hwnd: isize) -> HWND {
    HWND(hwnd as *mut core::ffi::c_void)
}

fn to_rect(rect: RECT) -> Rect {
    Rect::new(rect.left, rect.top, rect.right, rect.bottom)
}

/// `GWL_EXSTYLE` is re-exported above; this keeps the constant name visible to readers
/// of the tests without importing the whole enum.
#[cfg(test)]
mod tests {
    use super::*;
    use ::windows::Win32::Foundation::POINT;
    use ::windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, GetWindowInfo, HWND_TOP, HWND_TOPMOST, MSG,
        PM_REMOVE, PeekMessageW, SW_HIDE, SW_MINIMIZE, SW_SHOWNA, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SetWindowPos, ShowWindow, TranslateMessage, WINDOWINFO, WS_EX_NOACTIVATE,
        WS_CHILD, WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE,
    };
    use ::windows::core::w;
    use std::time::{Duration, Instant};

    /// Declare DPI awareness once per test binary.
    ///
    /// The application declares it before creating the overlay, and the window
    /// geometry checks below depend on that: an unaware process receives
    /// *virtualised* rectangles from `GetWindowInfo`/`GetWindowRect` while
    /// `DWMWA_EXTENDED_FRAME_BOUNDS` is always in physical pixels, so the two cannot be
    /// compared. Declaring it here makes the test process match production.
    fn ensure_dpi_awareness() -> bool {
        static ONCE: std::sync::Once = std::sync::Once::new();
        static AWARE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        ONCE.call_once(|| {
            let aware = crate::platform::windows::capture::monitor::set_per_monitor_v2_awareness()
                .map(|mode| mode == "per-monitor-v2" || mode == "per-monitor")
                .unwrap_or(false);
            AWARE.store(aware, std::sync::atomic::Ordering::SeqCst);
        });
        AWARE.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Detect whether this test process has a desktop to create windows on. The
    /// capture suite already runs real GPU/window probes, so a failure here means the
    /// environment lacks a window station rather than a code defect.
    fn desktop_available() -> bool {
        ensure_dpi_awareness();
        let probe = TestWindow::create(WINDOW_EX_STYLE(0), WS_POPUP | WS_VISIBLE);
        probe.is_some()
    }

    struct TestWindow {
        window: HWND,
    }

    impl TestWindow {
        fn create(extended_style: WINDOW_EX_STYLE, style: ::windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE) -> Option<Self> {
            let window = unsafe {
                CreateWindowExW(
                    extended_style | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                    w!("STATIC"),
                    w!("SnapClip window detection fixture"),
                    style,
                    120,
                    120,
                    260,
                    180,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .ok()?;
            // SW_SHOWNA: visible without stealing focus from whatever the user is doing.
            let _ = unsafe { ShowWindow(window, SW_SHOWNA) };
            pump(40);
            Some(Self { window })
        }

        fn hwnd(&self) -> HWND {
            self.window
        }

        fn handle(&self) -> isize {
            self.window.0 as isize
        }

        fn bring_to_front(&self) {
            unsafe {
                let _ = SetWindowPos(
                    self.window,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                );
            }
            pump(30);
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            let _ = unsafe { DestroyWindow(self.window) };
            pump(20);
        }
    }

    /// Pump this thread's message queue for `millis`, so the window manager and DWM
    /// can commit the geometry changes the test is about to read.
    fn pump(millis: u64) {
        let deadline = Instant::now() + Duration::from_millis(millis);
        let mut message = MSG::default();
        while Instant::now() < deadline {
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                unsafe {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            std::thread::sleep(Duration::from_millis(4));
        }
    }

    fn contains(rect: Rect, point: POINT) -> bool {
        point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
    }

    #[test]
    fn click_through_requires_both_layered_and_transparent() {
        assert!(!is_click_through_layered(0));
        assert!(
            !is_click_through_layered(WS_EX_LAYERED.0),
            "an ordinary layered window is still interactive"
        );
        assert!(
            !is_click_through_layered(WS_EX_TRANSPARENT.0),
            "WS_EX_TRANSPARENT alone only changes paint ordering"
        );
        assert!(is_click_through_layered((WS_EX_LAYERED | WS_EX_TRANSPARENT).0));
        // The shell's full-desktop handwriting canvas: layered + transparent + more.
        assert!(is_click_through_layered(0x0a08_00a8));
        // Tool window / no-activate are not click-through.
        assert!(!is_click_through_layered((WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE).0));
    }

    /// The pass-through flag is the one thing that crosses threads between the refinement worker
    /// (which sets it around one accessibility call) and the overlay's own window procedure (which
    /// reads it for `WM_NCHITTEST`). It must be clear by default, set for exactly the lifetime of
    /// the guard, and observable from another thread while it is set.
    #[test]
    fn the_hit_test_pass_through_lasts_exactly_as_long_as_its_guard() {
        let flag = HitTestPassThrough::default();
        assert!(!flag.is_active(), "nothing passes through until a query asks");
        {
            let _guard = flag.guard();
            assert!(flag.is_active(), "set for the accessibility call it wraps");
            let other = flag.clone();
            let seen = std::thread::spawn(move || other.is_active()).join().unwrap();
            assert!(seen, "the window procedure and the worker share one flag");
        }
        assert!(!flag.is_active(), "the guard clears it again, on every path");
        // Two guards in sequence must not leave it set (a leaked flag would make the overlay
        // miss real clicks for the rest of the session).
        drop(flag.guard());
        drop(flag.guard());
        assert!(!flag.is_active());
    }

    #[test]
    fn invalid_handles_are_rejected_without_calling_into_win32() {
        assert!(!is_window(0));
        assert!(!is_window_visible(0));
        assert!(!is_iconic(0));
        assert_eq!(process_id(0), 0);
        assert_eq!(class_name(0), None);
        assert_eq!(frame_bounds(0), None);
        assert!(!is_cloaked(0));
    }

    #[test]
    fn a_visible_tool_window_is_enumerated_and_a_hidden_one_is_not() {
        if !desktop_available() {
            eprintln!("skipping: no interactive window station available");
            return;
        }
        let window = TestWindow::create(WINDOW_EX_STYLE(0), WS_POPUP | WS_VISIBLE)
            .expect("desktop is available");
        window.bring_to_front();

        let handle = window.handle();
        let candidates = enumerate_cheap_candidates().expect("EnumWindows succeeds");
        assert!(
            candidates.iter().any(|probe| probe.hwnd == handle),
            "a visible tool window must be a candidate"
        );

        // Hiding it removes it, even though the handle is still valid.
        let _ = unsafe { ShowWindow(window.hwnd(), SW_HIDE) };
        pump(40);
        assert!(is_window(handle), "the handle itself stays valid while hidden");
        let candidates = enumerate_cheap_candidates().expect("EnumWindows succeeds");
        assert!(
            !candidates.iter().any(|probe| probe.hwnd == handle),
            "an invisible window must not be enumerated"
        );
    }

    #[test]
    fn a_minimised_window_is_not_a_candidate() {
        if !desktop_available() {
            eprintln!("skipping: no interactive window station available");
            return;
        }
        let window = TestWindow::create(WINDOW_EX_STYLE(0), WS_OVERLAPPEDWINDOW | WS_VISIBLE)
            .expect("desktop is available");
        window.bring_to_front();
        let handle = window.handle();
        assert!(
            enumerate_cheap_candidates()
                .expect("EnumWindows succeeds")
                .iter()
                .any(|probe| probe.hwnd == handle)
        );

        let _ = unsafe { ShowWindow(window.hwnd(), SW_MINIMIZE) };
        pump(80);
        assert!(
            is_iconic(handle) || !is_window_visible(handle),
            "minimising must make the window iconic or invisible"
        );
        assert!(
            !enumerate_cheap_candidates()
                .expect("EnumWindows succeeds")
                .iter()
                .any(|probe| probe.hwnd == handle),
            "a minimised window must not be enumerated"
        );
    }

    #[test]
    fn a_click_through_layered_window_is_not_a_candidate() {
        if !desktop_available() {
            eprintln!("skipping: no interactive window station available");
            return;
        }
        let overlay = TestWindow::create(WS_EX_LAYERED | WS_EX_TRANSPARENT, WS_POPUP | WS_VISIBLE)
            .expect("desktop is available");
        let ordinary = TestWindow::create(WINDOW_EX_STYLE(0), WS_POPUP | WS_VISIBLE)
            .expect("desktop is available");
        overlay.bring_to_front();
        ordinary.bring_to_front();

        let candidates = enumerate_cheap_candidates().expect("EnumWindows succeeds");
        assert!(
            !candidates.iter().any(|probe| probe.hwnd == overlay.handle()),
            "a click-through overlay must not be a candidate"
        );
        assert!(
            candidates.iter().any(|probe| probe.hwnd == ordinary.handle()),
            "the window under a click-through overlay stays selectable"
        );
    }

    #[test]
    fn frame_bounds_include_the_title_bar_and_exclude_the_invisible_border() {
        if !desktop_available() {
            eprintln!("skipping: no interactive window station available");
            return;
        }
        let window = TestWindow::create(WINDOW_EX_STYLE(0), WS_OVERLAPPEDWINDOW | WS_VISIBLE)
            .expect("desktop is available");
        window.bring_to_front();
        let handle = window.handle();

        // `GetWindowInfo` reports virtualised rectangles to a DPI-unaware caller while
        // DWM always reports physical pixels; without awareness the two spaces cannot
        // be compared, so the assertion below is only meaningful when the process
        // declared the same awareness the application does at startup.
        if !ensure_dpi_awareness() {
            eprintln!(
                "skipping geometry comparison: this test process is not per-monitor DPI aware"
            );
            assert!(frame_bounds(handle).is_some_and(|bounds| !bounds.is_empty()));
            return;
        }

        let mut info = WINDOWINFO {
            cbSize: size_of::<WINDOWINFO>() as u32,
            ..Default::default()
        };
        assert!(unsafe { GetWindowInfo(window.hwnd(), &mut info) }.is_ok());

        let bounds = frame_bounds(handle).expect("a visible window has frame bounds");
        assert!(!bounds.is_empty());

        // The frame starts above the client area: the caption row is part of it.
        assert!(
            bounds.top < info.rcClient.top,
            "frame={bounds:?} client={:?}",
            info.rcClient
        );
        let caption = POINT {
            x: info.rcClient.left + 20,
            y: bounds.top + (info.rcClient.top - bounds.top) / 2,
        };
        assert!(contains(bounds, caption), "the caption belongs to the frame");
        let client = to_rect(info.rcClient);
        assert!(!contains(client, caption), "the caption is not client area");

        // And the frame does not extend past the raw window rectangle: the invisible
        // resize border is excluded, with a generous tolerance for DWM rounding.
        let mut raw = RECT::default();
        assert!(unsafe { GetWindowRect(window.hwnd(), &mut raw) }.is_ok());
        let raw = to_rect(raw);
        const TOLERANCE: i32 = 16;
        assert!(bounds.left >= raw.left - TOLERANCE, "frame={bounds:?} raw={raw:?}");
        assert!(bounds.top >= raw.top - TOLERANCE, "frame={bounds:?} raw={raw:?}");
        assert!(bounds.right <= raw.right + TOLERANCE, "frame={bounds:?} raw={raw:?}");
        assert!(bounds.bottom <= raw.bottom + TOLERANCE, "frame={bounds:?} raw={raw:?}");
        assert!(bounds.width() >= raw.width() / 2);
        assert!(bounds.height() >= raw.height() / 2);
    }

    #[test]
    fn window_attributes_are_stable_for_a_live_window() {
        if !desktop_available() {
            eprintln!("skipping: no interactive window station available");
            return;
        }
        let window = TestWindow::create(WINDOW_EX_STYLE(0), WS_POPUP | WS_VISIBLE)
            .expect("desktop is available");
        let handle = window.handle();
        let class = class_name(handle).expect("a static window has a class name");
        assert_eq!(class, "Static", "CreateWindowExW(w!(\"STATIC\")) resolves to Static");
        assert_eq!(class_name(handle).as_deref(), Some(class.as_str()));
        assert_eq!(process_id(handle), std::process::id());
        assert!(is_window_visible(handle));
        assert!(!is_iconic(handle));
        assert!(!is_cloaked(handle), "an ordinary window is not cloaked");
        // The cheap probe carries the extended style the policy layer will inspect.
        let probe = enumerate_cheap_candidates()
            .expect("EnumWindows succeeds")
            .into_iter()
            .find(|probe| probe.hwnd == handle)
            .expect("the fixture is a candidate");
        assert_ne!(probe.extended_style & WS_EX_NOACTIVATE.0, 0);
    }

    #[test]
    fn dwm_batch_returns_one_read_per_handle() {
        if !desktop_available() {
            eprintln!("skipping: no interactive window station available");
            return;
        }
        let window = TestWindow::create(WINDOW_EX_STYLE(0), WS_POPUP | WS_VISIBLE)
            .expect("desktop is available");
        window.bring_to_front();
        let reads = read_dwm_batch(&[window.handle(), 0]);
        assert_eq!(reads.len(), 2);
        assert_eq!(reads[0].hwnd, window.handle());
        assert!(!reads[0].cloaked);
        assert!(reads[0].frame_bounds.is_some());
        assert_eq!(reads[1].hwnd, 0);
        assert_eq!(reads[1].frame_bounds, None);
    }

    #[test]
    fn only_the_topmost_of_stacked_child_windows_is_shown_at_a_point() {
        if !desktop_available() {
            eprintln!("skipping: no interactive window station available");
            return;
        }
        // File Explorer's tabs: same-size, all `WS_VISIBLE`, only the top one on screen.
        let parent = TestWindow::create(WINDOW_EX_STYLE(0), WS_OVERLAPPEDWINDOW | WS_VISIBLE)
            .expect("desktop is available");
        parent.bring_to_front();
        let stacked = |title| unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("STATIC"),
                title,
                WS_CHILD | WS_VISIBLE,
                20,
                40,
                120,
                60,
                Some(parent.hwnd()),
                None,
                None,
                None,
            )
        }
        .expect("child window creation succeeds");
        let below = stacked(w!("SnapClip hidden tab"));
        let above = stacked(w!("SnapClip active tab"));
        // Explicit z-order: creation order alone does not say which sibling ends up on top.
        unsafe {
            SetWindowPos(above, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE)
        }
        .expect("restack the children");
        pump(60);
        let mut rect = RECT::default();
        unsafe { GetWindowRect(above, &mut rect) }.expect("child rect");
        let inside = Point::new((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
        let outside = Point::new(rect.right + 10, rect.bottom + 10);

        assert!(is_shown_child_at(above.0 as isize, inside));
        assert!(
            !is_shown_child_at(below.0 as isize, inside),
            "a visible child covered by a sibling is not what the user sees"
        );
        assert!(!is_shown_child_at(above.0 as isize, outside));

        unsafe { ShowWindow(above, SW_HIDE) }.ok().ok();
        pump(20);
        assert!(
            is_shown_child_at(below.0 as isize, inside),
            "once the sibling is hidden the lower child is the one on screen"
        );
        let _ = unsafe { DestroyWindow(above) };
        let _ = unsafe { DestroyWindow(below) };
        pump(20);
    }

    #[test]
    fn visible_child_rects_reports_child_windows_inside_the_parent() {
        if !desktop_available() {
            eprintln!("skipping: no interactive window station available");
            return;
        }
        // A classic child-HWND hierarchy: this is the case the provider-free fallback exists
        // for, because such controls never appear in the accessibility tree.
        let parent = TestWindow::create(WINDOW_EX_STYLE(0), WS_OVERLAPPEDWINDOW | WS_VISIBLE)
            .expect("desktop is available");
        parent.bring_to_front();
        let Some(parent_bounds) = frame_bounds(parent.handle()) else {
            return;
        };
        let child = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("SnapClip child fixture"),
                WS_CHILD | WS_VISIBLE,
                20,
                40,
                120,
                60,
                Some(parent.hwnd()),
                None,
                None,
                None,
            )
        }
        .expect("child window creation succeeds");
        pump(60);

        let rects = visible_child_rects(parent.handle(), parent_bounds);
        assert!(
            !rects.is_empty(),
            "a visible child window must be reported inside its parent"
        );
        for rect in &rects {
            assert!(!rect.is_empty());
            assert!(
                rect.intersect(parent_bounds) == *rect,
                "child rectangles are clipped to the parent: {rect:?} vs {parent_bounds:?}"
            );
            assert_ne!(rect, &parent_bounds, "a full-size child adds no level");
        }
        let _ = unsafe { DestroyWindow(child) };
        pump(20);
    }
}
