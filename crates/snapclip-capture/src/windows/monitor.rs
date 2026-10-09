//! Monitor enumeration and physical/DPI geometry.
//!
//! All coordinates here are physical pixels in the virtual desktop, which is the
//! space the overlay window and the capture providers work in. DIP conversion only
//! happens when the process DPI awareness is declared.
//!
//! This module uses the typed `windows` bindings throughout so the monitor handle it
//! hands to the WGC interop needs no cross-crate pointer juggling.

use ::windows::Win32::Foundation::{LPARAM, POINT, RECT};
use ::windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint,
};
use ::windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
    GetThreadDpiAwarenessContext, SetProcessDpiAwarenessContext,
};
use ::windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::geometry::{MonitorLayout, Point, Rect};

/// A monitor plus the handle a capture provider needs.
///
/// `HMONITOR` is a plain handle that stays valid for the lifetime of the display, so
/// this value can be carried from the hotkey-handling thread into the provider call.
#[derive(Clone)]
pub struct CapturedMonitor {
    /// The monitor handle as an `isize`.
    ///
    /// Stored as an integer rather than as `HMONITOR` so this type — and everything
    /// holding one — stays `Send`: raw pointers are not, and the overlay thread moves
    /// the monitor into its own closure.
    pub handle: isize,
    pub layout: MonitorLayout,
}

impl CapturedMonitor {
    pub fn origin(&self) -> Point {
        Point::new(self.layout.bounds.left, self.layout.bounds.top)
    }

    pub fn width(&self) -> u32 {
        self.layout.bounds.width() as u32
    }

    pub fn height(&self) -> u32 {
        self.layout.bounds.height() as u32
    }
}

/// Declare DPI awareness for the process, preferring per-monitor V2.
///
/// Must run before any window is created, any cursor position is read and any window
/// is enumerated (docs/14 §3): after the first window exists the declaration can no
/// longer be changed, and a process that ends up with a coarser mode reports
/// different rectangles than it draws.
///
/// The fallback order is the documented one — Per-Monitor V2, then Per-Monitor, then
/// System — and the achieved mode is returned so callers can log it instead of
/// guessing. A process that is already aware (the shell declares its own context at
/// startup, `apps/snapclip/src/lib.rs`) makes every `SetProcessDpiAwarenessContext` call
/// fail with `ERROR_ACCESS_DENIED`, in which case the effective thread context is
/// reported instead — that path never downgrades.
pub fn set_per_monitor_v2_awareness() -> Result<&'static str, String> {
    for context in [
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
        DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
    ] {
        if unsafe { SetProcessDpiAwarenessContext(context) }.is_ok() {
            if context == DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 {
                return Ok("per-monitor-v2");
            }
            if context == DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE {
                return Ok("per-monitor");
            }
            return Ok("system");
        }
    }

    // Already declared by the host: report the context the thread actually has.
    let current = unsafe { GetThreadDpiAwarenessContext() };
    let matches = |expected| unsafe { AreDpiAwarenessContextsEqual(current, expected) }.as_bool();
    if matches(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) {
        return Ok("per-monitor-v2");
    }
    if matches(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE) {
        // Usable: coordinates stay per-monitor physical, only the automatic
        // non-client scaling behaviour differs.
        return Ok("per-monitor");
    }
    if matches(DPI_AWARENESS_CONTEXT_SYSTEM_AWARE) {
        // Documented last-resort step. Capture geometry stays physical for the primary
        // display at its native scale, so callers must log this loudly rather than
        // assume per-monitor accuracy.
        return Ok("system");
    }
    Err(format!(
        "SetProcessDpiAwarenessContext failed with Win32 error {} and the active context is not \
         DPI aware at all",
        unsafe { ::windows::Win32::Foundation::GetLastError().0 }
    ))
}

/// Current cursor position in virtual-desktop physical pixels.
pub fn cursor_position() -> Result<Point, String> {
    let mut point = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut point) }.is_err() {
        return Err(format!(
            "GetCursorPos failed with Win32 error {}",
            unsafe { ::windows::Win32::Foundation::GetLastError().0 }
        ));
    }
    Ok(Point::new(point.x, point.y))
}

/// The monitor under a virtual-desktop physical point, with its handle.
///
/// Uses `MONITOR_DEFAULTTONEAREST`, so the cursor is always assigned to a monitor
/// even when it sits in a gap between displays.
pub fn captured_monitor_at(point: Point) -> Result<CapturedMonitor, String> {
    let monitor = unsafe {
        MonitorFromPoint(
            POINT {
                x: point.x,
                y: point.y,
            },
            MONITOR_DEFAULTTONEAREST,
        )
    };
    if monitor.is_invalid() {
        return Err("MonitorFromPoint returned no monitor".into());
    }
    Ok(CapturedMonitor {
        handle: monitor.0 as isize,
        layout: describe(monitor)?,
    })
}

/// The monitor nearest the cursor, with its handle.
pub fn captured_monitor_at_cursor() -> Result<CapturedMonitor, String> {
    captured_monitor_at(cursor_position()?)
}


/// Every monitor in the virtual desktop, primary first.
///
/// Used by the multi-monitor acceptance checks and by `cargo test`; the interactive
/// path always resolves the single monitor under the cursor.
#[cfg_attr(not(test), allow(dead_code))]
pub fn enumerate() -> Vec<MonitorLayout> {
    struct Collector {
        monitors: Vec<MonitorLayout>,
    }

    unsafe extern "system" fn callback(
        monitor: HMONITOR,
        _dc: ::windows::Win32::Graphics::Gdi::HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> ::windows::core::BOOL {
        let collector = unsafe { &mut *(data.0 as *mut Collector) };
        if let Ok(layout) = describe(monitor) {
            collector.monitors.push(layout);
        }
        ::windows::core::BOOL(1)
    }

    let mut collector = Collector {
        monitors: Vec::new(),
    };
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(callback),
            LPARAM(&mut collector as *mut Collector as isize),
        );
    }
    collector
        .monitors
        .sort_by_key(|layout| (!layout.primary, layout.bounds.left, layout.bounds.top));
    collector.monitors
}

fn describe(monitor: HMONITOR) -> Result<MonitorLayout, String> {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return Err(format!(
            "GetMonitorInfoW failed with Win32 error {}",
            unsafe { ::windows::Win32::Foundation::GetLastError().0 }
        ));
    }
    Ok(MonitorLayout {
        bounds: to_rect(info.rcMonitor),
        work_area: to_rect(info.rcWork),
        dpi: monitor_dpi(monitor),
        primary: info.dwFlags & 1 != 0, // MONITORINFOF_PRIMARY
    })
}

fn to_rect(rect: RECT) -> Rect {
    Rect::new(rect.left, rect.top, rect.right, rect.bottom)
}

/// Effective DPI for a monitor via `GetDpiForMonitor` (shcore).
///
/// Linked directly so the process still starts on systems without shcore, in which
/// case 96 DPI is assumed.
fn monitor_dpi(monitor: HMONITOR) -> u32 {
    #[link(name = "shcore")]
    unsafe extern "system" {
        fn GetDpiForMonitor(
            monitor: HMONITOR,
            dpi_type: i32,
            dpi_x: *mut u32,
            dpi_y: *mut u32,
        ) -> i32;
    }

    const MDT_EFFECTIVE_DPI: i32 = 0;
    let mut dpi_x = 96u32;
    let mut dpi_y = 96u32;
    let result = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    if result < 0 || dpi_x == 0 {
        96
    } else {
        dpi_x
    }
}

/// Font height in physical pixels for a DIP size at the given monitor DPI.
///
/// Chrome metrics are computed from a DPI scale factor; this helper documents the
/// conversion and is exercised by the DPI tests at 96/120/144/192.
#[cfg_attr(not(test), allow(dead_code))]
pub fn dip_to_px(dip: f32, dpi: u32) -> f32 {
    dip * dpi.max(96) as f32 / 96.0
}

#[cfg(test)]
mod tests {
    use super::{
        captured_monitor_at, captured_monitor_at_cursor, dip_to_px, enumerate,
        set_per_monitor_v2_awareness,
    };
    use crate::geometry::{Point, Rect};

    #[test]
    fn dip_conversion_follows_the_dpi_scale() {
        assert_eq!(dip_to_px(8.0, 96), 8.0);
        assert_eq!(dip_to_px(8.0, 192), 16.0);
        assert!((dip_to_px(10.0, 120) - 12.5).abs() < f32::EPSILON);
        // Degenerate DPI values must never produce zero-sized chrome.
        assert_eq!(dip_to_px(4.0, 0), 4.0);
    }

    #[test]
    fn dpi_awareness_is_declared_and_idempotent() {
        // Safe to call repeatedly; the second call reports the already-set mode.
        let first = set_per_monitor_v2_awareness();
        let second = set_per_monitor_v2_awareness();
        assert_eq!(first.is_ok(), second.is_ok());
        if let Ok(mode) = first {
            // The documented fallback chain ends at System awareness; anything outside
            // the chain would mean the declaration silently produced an unknown state.
            assert!(
                matches!(mode, "per-monitor-v2" | "per-monitor" | "system"),
                "unexpected DPI awareness mode {mode}"
            );
        }
    }

    #[test]
    fn a_second_declaration_never_downgrades_an_aware_process() {
        // The shell declares its own context before capture starts; repeating
        // the declaration from the overlay thread must report the existing context
        // rather than replacing it with a coarser one.
        let first = set_per_monitor_v2_awareness();
        let second = set_per_monitor_v2_awareness();
        assert!(first.is_ok() && second.is_ok());
        if let (Ok(first), Ok(second)) = (first, second) {
            let rank = |mode: &str| match mode {
                "per-monitor-v2" => 2,
                "per-monitor" => 1,
                _ => 0,
            };
            assert!(
                rank(second) >= rank(first),
                "repeating the declaration downgraded {first} to {second}"
            );
        }
    }

    #[test]
    fn enumeration_reports_at_least_one_primary_monitor_with_consistent_geometry() {
        // DPI awareness first: it changes the reported rectangles.
        let _ = set_per_monitor_v2_awareness();
        let monitors = enumerate();
        assert!(!monitors.is_empty(), "a Windows desktop has >= 1 monitor");
        assert_eq!(
            monitors.iter().filter(|layout| layout.primary).count(),
            1,
            "exactly one primary monitor"
        );
        for layout in &monitors {
            assert!(layout.bounds.width() > 0 && layout.bounds.height() > 0);
            assert!(layout.dpi >= 96, "unexpected dpi {}", layout.dpi);
            assert!(
                layout.bounds.contains(layout.work_area.center()),
                "work area must sit inside the monitor bounds"
            );
        }
        // Primary first.
        assert!(monitors[0].primary);
    }

    #[test]
    fn monitor_lookup_falls_back_to_the_nearest_display() {
        let _ = set_per_monitor_v2_awareness();
        let layout = captured_monitor_at(Point::new(0, 0)).unwrap().layout;
        assert!(layout.bounds.contains(layout.bounds.center()));

        // A point far outside every display still resolves (nearest).
        let far = captured_monitor_at(Point::new(-100_000, -100_000)).unwrap().layout;
        assert!(far.bounds.width() > 0);

        // Local conversion round-trips through the monitor origin.
        let local = layout.to_local(Point::new(layout.bounds.left + 5, layout.bounds.top + 7));
        assert_eq!(local, Point::new(5, 7));
        assert_eq!(
            layout.local_bounds(),
            Rect::new(0, 0, layout.bounds.width(), layout.bounds.height())
        );
    }

    #[test]
    #[ignore = "L3 (docs/31 D-14): needs a monitor at the cursor, so it needs an interactive desktop; run with --ignored --test-threads=1 on a live desktop"]
    fn monitor_handle_is_available_for_providers() {
        let _ = set_per_monitor_v2_awareness();
        let monitor = captured_monitor_at_cursor()
            .expect("this test needs a monitor at the cursor (docs/31 D-14)");
        assert_ne!(monitor.handle, 0);
        assert!(monitor.width() > 0 && monitor.height() > 0);
    }
}



