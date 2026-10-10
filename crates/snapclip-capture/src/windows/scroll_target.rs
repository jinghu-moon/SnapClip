//! The platform half of target choice (`docs/32` §4.2; task `P7.02`).
//!
//! [`crate::scroll::target`] is platform-free by gate (`docs/30` §28.4), so the two conversions
//! that need the platform happen here:
//!
//! * the window-detection snapshot speaks **virtual-desktop** physical pixels and carries a
//!   `WindowIdentity`; the scroll rule speaks the overlay's **monitor-local** pixels and an
//!   opaque `u64` (`docs/32` ADR-25). `geometry::window_rect_to_local` is the documented
//!   conversion — `window_detection/model.rs:395` says "only the overlay does it" — and this
//!   module is the overlay doing it for the scroll path.
//! * degenerate candidates are dropped here: `WindowCandidate::is_usable` already says a
//!   rectangle with no area can never be a target, and carrying it on would only let it win an
//!   overlap of zero.
//!
//! Nothing in production calls this yet; the caller is the assembly root (`docs/32` `P7.04`).

use crate::geometry::{MonitorLayout, Rect, window_rect_to_local};
use crate::scroll::observation::Axis;
use crate::scroll::target::{TargetCandidate, TargetChoice, choose_target};
use crate::window_detection::model::WindowCandidate;

/// Every usable candidate, converted into the overlay's monitor-local space.
#[allow(dead_code)] // The caller is the assembly root (docs/32 P7.04).
pub(crate) fn candidates(
    snapshot: &[WindowCandidate],
    monitor: &MonitorLayout,
) -> Vec<TargetCandidate> {
    snapshot
        .iter()
        .filter(|candidate| candidate.is_usable())
        .map(|candidate| TargetCandidate {
            window: candidate.identity.hwnd as u64,
            bounds: window_rect_to_local(candidate.screen_bounds, monitor),
            z_order: candidate.z_order,
        })
        .collect()
}

/// Which window a scroll session should read from, given the snapshot the overlay holds.
#[allow(dead_code)] // The caller is the assembly root (docs/32 P7.04).
pub(crate) fn choose_in_snapshot(
    selection: Rect,
    snapshot: &[WindowCandidate],
    monitor: &MonitorLayout,
    axis: Axis,
) -> TargetChoice {
    choose_target(selection, &candidates(snapshot, monitor), axis)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window_detection::model::WindowIdentity;

    /// A second monitor to the right of the primary one: screen coordinates are *not* local
    /// coordinates, which is the whole point of this module.
    fn monitor() -> MonitorLayout {
        MonitorLayout {
            bounds: Rect::new(1920, 0, 3840, 1080),
            work_area: Rect::new(1920, 0, 3840, 1040),
            dpi: 96,
            primary: false,
        }
    }

    fn window_candidate(hwnd: isize, screen_bounds: Rect, z_order: u32) -> WindowCandidate {
        WindowCandidate::new(
            WindowIdentity::new(hwnd, 7, 0xABCD),
            screen_bounds,
            z_order,
            5,
        )
    }

    #[test]
    fn the_snapshot_is_converted_into_the_overlays_local_space_first() {
        let monitor = monitor();
        // A window covering the whole (second) monitor, in screen coordinates.
        let window = window_candidate(0x1234, Rect::new(1920, 0, 3840, 1080), 0);
        // The selection is local, as the overlay's capture session produces it.
        let selection = Rect::new(100, 100, 900, 800);

        match choose_in_snapshot(selection, &[window], &monitor, Axis::Vertical) {
            TargetChoice::Accepted(target) => {
                assert_eq!(target.window(), 0x1234);
                assert_eq!(
                    target.bounds(),
                    Rect::new(0, 0, 1920, 1080),
                    "bounds must be monitor-local, not virtual-desktop"
                );
                assert_eq!(target.crop(), selection);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_degenerate_window_is_not_a_candidate() {
        let monitor = monitor();
        let flat = window_candidate(0x1, Rect::new(2000, 100, 2000, 900), 0);
        assert_eq!(
            choose_in_snapshot(Rect::new(0, 0, 100, 100), &[flat], &monitor, Axis::Vertical),
            TargetChoice::NoWindow
        );
    }

    #[test]
    fn the_frontmost_of_two_equal_windows_wins() {
        let monitor = monitor();
        let bounds = Rect::new(1920, 0, 3840, 1080);
        let front = window_candidate(0xA, bounds, 0);
        let behind = window_candidate(0xB, bounds, 1);
        match choose_in_snapshot(
            Rect::new(10, 10, 500, 900),
            &[front, behind],
            &monitor,
            Axis::Vertical,
        ) {
            TargetChoice::Accepted(target) => assert_eq!(target.window(), 0xA),
            other => panic!("{other:?}"),
        }
    }
}
