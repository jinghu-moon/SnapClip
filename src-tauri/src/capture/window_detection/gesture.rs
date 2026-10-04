//! Pointer gesture contract for the overlay selection (docs/14 §4.2).
//!
//! ## The root cause this replaces
//!
//! The current overlay calls `CaptureSession::pointer_pressed` on button-down, which
//! immediately rewrites the selection (a press outside the current rectangle creates a
//! zero-size one). That makes three different user intents indistinguishable:
//! confirming an existing selection with a click, waiting for the automatic-snap
//! preview, and starting a fresh drag. Automatic snapping cannot be layered on top of
//! that — a press would destroy the preview before the user could confirm it — so the
//! interaction is rebuilt around an explicit gesture enum where **button-down never
//! changes the selection**.
//!
//! ## Boundary with `Settled`
//!
//! `Settled` is *not* a variant of [`PointerGesture`]. It is a session/display state
//! (selection confirmed, editing allowed, hover and automatic snapping off) owned by
//! the capture session, exactly like `Selecting`/`Selected`/`Annotating`. A pointer
//! gesture is a transient fact about the mouse; mixing the two in one enum is what
//! produces "the confirmed selection was rewritten by a stale hover" bugs. While the
//! session is settled the overlay simply never produces a window target.

use crate::capture::geometry::{Point, Rect, ResizeMode};
use crate::capture::window_detection::model::WindowTarget;

/// The pointer's interaction state as the overlay sees it (docs/14 §4.2).
///
/// `Rect` is the monitor-local selection the overlay paints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerGesture {
    /// No button down, nothing previewed: the cursor only drives hover.
    None,
    /// The cursor rested long enough that a nearby window is previewed.
    ///
    /// `preview_selection` is paint-only; `selection_before_preview` is what the
    /// session had before the preview appeared, and is restored if the target turns
    /// out to be invalid when the user confirms.
    AutoSnapPreview {
        target: WindowTarget,
        preview_selection: Rect,
        selection_before_preview: Rect,
    },
    /// Button is down and has not moved past the drag threshold yet.
    ///
    /// Recorded instead of mutating the selection so a click — including the first
    /// click of a double-click that confirms a settled selection — cannot create a
    /// zero-size rectangle or erase the current one.
    PendingPointer {
        press_point: Point,
        selection_before_press: Rect,
    },
    /// Button moved past the drag threshold and the gesture became a free drag.
    ManualDrag { press_point: Point, mode: ResizeMode },
    /// Dragging an existing selection by its interior.
    MoveSelection,
    /// Dragging one of the eight resize grips.
    ResizeSelection,
}

impl PointerGesture {
    /// Whether the overlay is currently previewing an automatic snap.
    pub fn auto_snap_preview(&self) -> Option<&WindowTarget> {
        match self {
            Self::AutoSnapPreview { target, .. } => Some(target),
            _ => None,
        }
    }

    /// Whether a button is held.
    pub fn is_pressed(&self) -> bool {
        matches!(
            self,
            Self::PendingPointer { .. }
                | Self::ManualDrag { .. }
                | Self::MoveSelection
                | Self::ResizeSelection
        )
    }

    /// Whether hover/dwell may still update while this gesture is active.
    ///
    /// A held button always wins over automatic snapping: the user is doing something
    /// deliberate, and a preview appearing under the cursor would fight the drag.
    pub fn allows_hover(&self) -> bool {
        !self.is_pressed()
    }
}

/// Whether a held pointer has travelled far enough to become a drag (docs/14 §4.2).
///
/// `drag_threshold` is the system drag distance (`SM_CXDRAG`) converted to physical
/// pixels. Comparing squared distances avoids a square root on every mouse move, and
/// `i64` keeps the squares from overflowing at the far end of a virtual desktop.
/// A threshold of `0` (or less) means "any movement starts a drag", matching the
/// reference behaviour in `screenshotintelligentselectionmodel.cpp`.
pub fn should_start_manual_drag(press_point: Point, current: Point, drag_threshold: i32) -> bool {
    if drag_threshold <= 0 {
        return true;
    }
    let dx = i64::from(current.x) - i64::from(press_point.x);
    let dy = i64::from(current.y) - i64::from(press_point.y);
    let threshold = i64::from(drag_threshold);
    dx * dx + dy * dy >= threshold * threshold
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::window_detection::model::{SnapshotEpoch, WindowCandidate, WindowIdentity};

    fn target(epoch: SnapshotEpoch) -> WindowTarget {
        WindowTarget::top_level_window_frame(WindowCandidate::new(
            WindowIdentity::new(0x1234, 7, 0xABC),
            Rect::new(100, 100, 300, 200),
            0,
            epoch,
        ))
    }

    #[test]
    fn a_held_button_suppresses_hover_and_automatic_snapping() {
        assert!(PointerGesture::None.allows_hover());
        assert!(
            PointerGesture::AutoSnapPreview {
                target: target(1),
                preview_selection: Rect::new(10, 10, 20, 20),
                selection_before_preview: Rect::default(),
            }
            .allows_hover()
        );
        for gesture in [
            PointerGesture::PendingPointer {
                press_point: Point::new(5, 5),
                selection_before_press: Rect::default(),
            },
            PointerGesture::ManualDrag {
                press_point: Point::new(5, 5),
                mode: ResizeMode::Move,
            },
            PointerGesture::MoveSelection,
            PointerGesture::ResizeSelection,
        ] {
            assert!(gesture.is_pressed(), "{gesture:?}");
            assert!(!gesture.allows_hover(), "{gesture:?}");
            assert_eq!(gesture.auto_snap_preview(), None, "{gesture:?}");
        }
    }

    #[test]
    fn only_the_preview_variant_exposes_a_window_target() {
        let preview = PointerGesture::AutoSnapPreview {
            target: target(9),
            preview_selection: Rect::new(100, 100, 300, 200),
            selection_before_preview: Rect::default(),
        };
        assert_eq!(preview.auto_snap_preview().map(|t| t.identity().hwnd), Some(0x1234));
        assert!(!preview.is_pressed());
    }

    #[test]
    fn a_tiny_jitter_does_not_start_a_manual_drag() {
        let press = Point::new(1000, 1000);
        // 3 px diagonal-ish movement stays a pending click, not a drag.
        assert!(!should_start_manual_drag(press, Point::new(1002, 1002), 4));
        // Exactly at the threshold starts the drag (>= comparison).
        assert!(should_start_manual_drag(press, Point::new(1004, 1000), 4));
        assert!(should_start_manual_drag(press, Point::new(1003, 1003), 4));
        // A zero threshold treats any movement as a drag.
        assert!(should_start_manual_drag(press, press, 0));
    }

    #[test]
    fn the_drag_test_is_symmetric_and_overflow_safe() {
        let a = Point::new(-900_000, -900_000);
        let b = Point::new(900_000, 900_000);
        // Both directions agree, and the squared distance does not overflow `i32`
        // (which would make a far-off pointer look like a stationary one).
        assert!(should_start_manual_drag(a, b, 4));
        assert!(should_start_manual_drag(b, a, 4));
        assert!(!should_start_manual_drag(a, a, 4));
    }
}
