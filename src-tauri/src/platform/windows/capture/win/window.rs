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

// The detection provider that consumes this module arrives with the snapshot phase;
// until it is wired in, the only callers are this module's own real-window tests.
// Remove this once `platform::windows::capture::window_detection` uses the surface.
#![cfg_attr(not(test), allow(dead_code))]

use std::mem::size_of;

use ::windows::Win32::Foundation::{HWND, LPARAM, RECT};
use ::windows::core::BOOL;
use ::windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetClassNameW, GetWindowLongPtrW, GetWindowRect,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, WINDOW_EX_STYLE, WS_EX_LAYERED,
    WS_EX_TRANSPARENT,
};

use crate::capture::geometry::Rect;
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

/// `GetWindowLongPtrW(GWL_EXSTYLE)`.
pub fn extended_style(hwnd: isize) -> u32 {
    if !is_window(hwnd) {
        return 0;
    }
    unsafe { GetWindowLongPtrW(to_hwnd(hwnd), GWL_EXSTYLE) as u32 }
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
        CreateWindowExW, DestroyWindow, DispatchMessageW, GetWindowInfo, HWND_TOPMOST, MSG,
        PM_REMOVE, PeekMessageW, SW_HIDE, SW_MINIMIZE, SW_SHOWNA, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SetWindowPos, ShowWindow, TranslateMessage, WINDOWINFO, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE,
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
        assert!(extended_style(handle) & WS_EX_NOACTIVATE.0 != 0);
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
}
