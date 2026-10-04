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

/// What the overlay should do after a button press was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressOutcome {
    /// The press landed on a handle or inside the selection: editing starts
    /// immediately, with no drag threshold (the user grabbed something deliberate).
    BeginEdit(ResizeMode),
    /// The press landed anywhere else. The selection is **not** touched; the drag only
    /// starts if the pointer later crosses the drag threshold.
    Pending,
}

/// What the overlay should do after the cursor moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveOutcome {
    /// No button held: hover may update and the dwell timer restarts.
    Hover,
    /// Button held, still inside the drag threshold: nothing changes.
    Pending,
    /// The threshold was crossed by this move; the caller starts this drag.
    ManualDragStarted { press_point: Point },
    /// A drag/edit is already running; the caller forwards the point to the session.
    Dragging,
}

/// What the overlay should do when the button came up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseOutcome {
    /// The press never became a drag. The session's selection is untouched and an
    /// automatic snap is **not** confirmed by releasing the button.
    Click,
    /// A drag/edit ran and must be committed through the session's release path.
    CommitDrag,
}

/// The paint-only automatic-snap preview.
///
/// `selection_before_preview` is what the session showed before the preview appeared;
/// a confirmation that later fails validation restores it (docs/14 §5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapPreview {
    pub target: WindowTarget,
    /// Monitor-local rectangle the overlay paints for the preview.
    pub selection: Rect,
    pub selection_before_preview: Rect,
}

/// Pure pointer-gesture machine (docs/14 §4.2).
///
/// The overlay owns one of these and forwards mouse messages verbatim; all decisions
/// live here so the whole interaction — press, threshold, dwell, release, cancel — is
/// unit testable without a window, a GPU or a desktop.
///
/// **A press never modifies the selection.** That is the behavioural fix this type
/// exists for: the previous implementation turned every button-down into a zero-size
/// selection, which made "click to confirm" and "rest and snap" indistinguishable and
/// destroyed any preview before it could be confirmed.
///
/// The preview is **display state**, not a gesture (docs/14 §4.2): a press must not
/// destroy it, otherwise the primary confirm gesture — a left click on the previewed
/// window — would have nothing left to confirm. Crossing the drag threshold does drop it,
/// because then the user is dragging rather than clicking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GestureState {
    gesture: PointerGesture,
    /// The automatic-snap preview, if one is showing. Survives a press; cleared when the
    /// gesture becomes a drag or when the caller confirms/cancels it.
    preview: Option<SnapPreview>,
    /// System drag distance in physical pixels (`SM_CXDRAG`, DPI scaled).
    drag_threshold: i32,
    /// Cursor-movement generation. A dwell result is only accepted for the generation
    /// it was armed for, so a timer that fires one move late cannot snap to the old
    /// position.
    dwell_generation: u64,
}

impl GestureState {
    pub const fn new(drag_threshold: i32) -> Self {
        Self {
            gesture: PointerGesture::None,
            preview: None,
            drag_threshold,
            dwell_generation: 0,
        }
    }

    pub fn gesture(&self) -> &PointerGesture {
        &self.gesture
    }

    pub fn dwell_generation(&self) -> u64 {
        self.dwell_generation
    }

    /// Record a button press (docs/14 §4.2 `PointerDown`).
    ///
    /// The preview survives the press on purpose: releasing inside the drag threshold is
    /// now the primary way to confirm it (docs/14 §4.3). Only crossing the threshold —
    /// which means the user is dragging, not clicking — discards it.
    pub fn press(
        &mut self,
        point: Point,
        hit: crate::capture::geometry::SelectionGeometry,
        selection: Rect,
    ) -> PressOutcome {
        use crate::capture::geometry::SelectionGeometry;
        self.dwell_generation = self.dwell_generation.wrapping_add(1);
        match hit {
            SelectionGeometry::Resize(handle) => {
                self.gesture = PointerGesture::ResizeSelection;
                PressOutcome::BeginEdit(ResizeMode::Handle(handle))
            }
            SelectionGeometry::Move => {
                self.gesture = PointerGesture::MoveSelection;
                PressOutcome::BeginEdit(ResizeMode::Move)
            }
            SelectionGeometry::Create | SelectionGeometry::Outside => {
                self.gesture = PointerGesture::PendingPointer {
                    press_point: point,
                    selection_before_press: selection,
                };
                PressOutcome::Pending
            }
        }
    }

    /// Record cursor movement (docs/14 §4.2 `PointerMove`).
    pub fn move_cursor(&mut self, point: Point) -> MoveOutcome {
        match self.gesture {
            PointerGesture::PendingPointer { press_point, .. } => {
                if should_start_manual_drag(press_point, point, self.drag_threshold) {
                    // The gesture became a drag: a drag must never confirm the preview.
                    self.preview = None;
                    self.gesture = PointerGesture::ManualDrag {
                        press_point,
                        mode: ResizeMode::Handle(crate::capture::geometry::Handle::BottomRight),
                    };
                    MoveOutcome::ManualDragStarted { press_point }
                } else {
                    MoveOutcome::Pending
                }
            }
            PointerGesture::ManualDrag { .. }
            | PointerGesture::MoveSelection
            | PointerGesture::ResizeSelection => MoveOutcome::Dragging,
            PointerGesture::None => {
                // No button: the cursor position itself is the dwell trigger.
                self.dwell_generation = self.dwell_generation.wrapping_add(1);
                MoveOutcome::Hover
            }
        }
    }

    /// Record the button coming up (docs/14 §4.2 `PointerUp`).
    pub fn release(&mut self) -> ReleaseOutcome {
        let outcome = match self.gesture {
            PointerGesture::ManualDrag { .. }
            | PointerGesture::MoveSelection
            | PointerGesture::ResizeSelection => ReleaseOutcome::CommitDrag,
            PointerGesture::None
            | PointerGesture::PendingPointer { .. } => ReleaseOutcome::Click,
        };
        // The preview deliberately survives: `Click` + `snap_preview().is_some()` is the
        // left-click confirm (docs/14 §4.3).
        self.gesture = PointerGesture::None;
        self.dwell_generation = self.dwell_generation.wrapping_add(1);
        outcome
    }

    /// Accept a dwell result (docs/14 §4.2 `AutoSnapPreview`).
    ///
    /// `generation` must be the value [`Self::dwell_generation`] had when the timer was
    /// armed; a stale timer is rejected, which is what keeps a preview from appearing
    /// for a position the cursor has already left. `None` clears the preview without
    /// touching the session selection.
    pub fn apply_dwell(
        &mut self,
        generation: u64,
        preview: Option<(WindowTarget, Rect, Rect)>,
    ) -> bool {
        if generation != self.dwell_generation || self.gesture.is_pressed() {
            return false;
        }
        match preview {
            Some((target, selection, selection_before_preview)) => {
                if self.preview.is_some_and(|current| current.selection == selection) {
                    return false;
                }
                self.preview = Some(SnapPreview {
                    target,
                    selection,
                    selection_before_preview,
                });
                true
            }
            None => self.clear_preview(),
        }
    }

    /// Forget the preview without changing the session selection.
    pub fn clear_preview(&mut self) -> bool {
        let had = self.preview.take().is_some();
        if had {
            self.dwell_generation = self.dwell_generation.wrapping_add(1);
        }
        had
    }

    /// Drop everything. Used on session teardown so nothing leaks into the next F5.
    pub fn reset(&mut self) {
        self.gesture = PointerGesture::None;
        self.preview = None;
        self.dwell_generation = self.dwell_generation.wrapping_add(1);
    }

    /// The preview the overlay should paint, if any.
    pub fn snap_preview(&self) -> Option<SnapPreview> {
        self.preview
    }
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
    fn a_held_button_suppresses_hover_and_new_previews() {
        assert!(PointerGesture::None.allows_hover());
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
        }
    }

    #[test]
    fn the_preview_is_display_state_not_a_pointer_gesture() {
        let mut state = GestureState::new(4);
        let generation = state.dwell_generation();
        assert!(state.apply_dwell(
            generation,
            Some((snap_target(), selection(100, 100, 300, 200), Rect::default()))
        ));
        // A preview never changes what the pointer is doing...
        assert_eq!(*state.gesture(), PointerGesture::None);
        assert_eq!(
            state.snap_preview().map(|preview| preview.target.identity().hwnd),
            Some(snap_target().identity().hwnd)
        );
        // ...and pressing does not destroy it, because the click that follows is the
        // confirm gesture (docs/14 §4.3).
        state.press(Point::new(150, 150), SelectionGeometry::Create, Rect::default());
        assert!(state.snap_preview().is_some());
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

    // --- The gesture machine (docs/14 §4.2) -------------------------------------

    use crate::capture::geometry::{Handle, SelectionGeometry};

    fn selection(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect::new(left, top, right, bottom)
    }

    /// The target used by the dwell tests.
    fn snap_target() -> WindowTarget {
        target(3)
    }

    #[test]
    fn a_plain_press_never_creates_or_moves_a_selection() {
        let mut state = GestureState::new(4);
        let before = selection(10, 10, 200, 150);
        // Press far outside the selection: this is exactly the case that used to
        // create a zero-size rectangle at the press point.
        let outcome = state.press(Point::new(900, 900), SelectionGeometry::Outside, before);
        assert_eq!(outcome, PressOutcome::Pending);
        assert!(matches!(
            state.gesture(),
            PointerGesture::PendingPointer { press_point, selection_before_press }
                if *press_point == Point::new(900, 900) && *selection_before_press == before
        ));
        // Releasing without moving is a click: nothing is committed.
        assert_eq!(state.release(), ReleaseOutcome::Click);
        assert_eq!(*state.gesture(), PointerGesture::None);
    }

    #[test]
    fn a_press_on_a_handle_or_inside_starts_the_edit_immediately() {
        let mut state = GestureState::new(4);
        let current = selection(10, 10, 200, 150);
        assert_eq!(
            state.press(Point::new(10, 10), SelectionGeometry::Resize(Handle::TopLeft), current),
            PressOutcome::BeginEdit(ResizeMode::Handle(Handle::TopLeft))
        );
        assert_eq!(state.gesture(), &PointerGesture::ResizeSelection);
        assert_eq!(state.release(), ReleaseOutcome::CommitDrag);

        assert_eq!(
            state.press(Point::new(100, 80), SelectionGeometry::Move, current),
            PressOutcome::BeginEdit(ResizeMode::Move)
        );
        assert_eq!(state.gesture(), &PointerGesture::MoveSelection);
        assert_eq!(state.release(), ReleaseOutcome::CommitDrag);
    }

    #[test]
    fn a_tiny_jitter_stays_pending_and_a_real_move_starts_the_drag() {
        let mut state = GestureState::new(4);
        let before = selection(10, 10, 200, 150);
        state.press(Point::new(500, 500), SelectionGeometry::Outside, before);

        // 2 px of jitter never becomes a drag.
        assert_eq!(state.move_cursor(Point::new(502, 501)), MoveOutcome::Pending);
        assert_eq!(state.move_cursor(Point::new(501, 501)), MoveOutcome::Pending);
        assert_eq!(
            state.gesture(),
            &PointerGesture::PendingPointer {
                press_point: Point::new(500, 500),
                selection_before_press: before,
            },
            "the pending record survives the jitter unchanged"
        );

        // Crossing the threshold (4 px) converts the press into a manual drag anchored
        // at the original press point, not at the current position.
        assert_eq!(
            state.move_cursor(Point::new(504, 500)),
            MoveOutcome::ManualDragStarted {
                press_point: Point::new(500, 500)
            }
        );
        assert!(matches!(state.gesture(), PointerGesture::ManualDrag { .. }));
        // Further movement just continues the drag.
        assert_eq!(state.move_cursor(Point::new(600, 520)), MoveOutcome::Dragging);
        assert_eq!(state.release(), ReleaseOutcome::CommitDrag);
    }

    #[test]
    fn a_click_confirms_the_preview_but_a_drag_never_does() {
        // Click: press and release inside the drag threshold. The preview must survive so
        // the caller can confirm it — this is the primary confirm gesture (docs/14 §4.3).
        let mut state = GestureState::new(4);
        let generation = state.dwell_generation();
        assert!(state.apply_dwell(
            generation,
            Some((snap_target(), selection(100, 100, 300, 200), Rect::default()))
        ));
        assert_eq!(
            state.press(Point::new(150, 150), SelectionGeometry::Create, Rect::default()),
            PressOutcome::Pending
        );
        assert_eq!(state.move_cursor(Point::new(151, 151)), MoveOutcome::Pending);
        assert_eq!(state.release(), ReleaseOutcome::Click);
        assert!(
            state.snap_preview().is_some(),
            "a click must leave the preview available to confirm"
        );
        assert_eq!(*state.gesture(), PointerGesture::None);

        // Drag: crossing the threshold means the user is dragging, so the preview is
        // discarded and the release commits the free selection instead.
        let mut state = GestureState::new(4);
        let generation = state.dwell_generation();
        assert!(state.apply_dwell(
            generation,
            Some((snap_target(), selection(100, 100, 300, 200), Rect::default()))
        ));
        state.press(Point::new(500, 500), SelectionGeometry::Outside, Rect::default());
        assert_eq!(
            state.move_cursor(Point::new(600, 600)),
            MoveOutcome::ManualDragStarted {
                press_point: Point::new(500, 500)
            }
        );
        assert!(
            state.snap_preview().is_none(),
            "a drag must never be able to confirm a preview"
        );
        assert_eq!(state.release(), ReleaseOutcome::CommitDrag);
        assert!(state.snap_preview().is_none());
    }

    #[test]
    fn a_click_without_a_preview_changes_nothing() {
        let mut state = GestureState::new(4);
        assert_eq!(
            state.press(
                Point::new(10, 10),
                SelectionGeometry::Outside,
                selection(1, 1, 50, 50)
            ),
            PressOutcome::Pending
        );
        assert_eq!(state.release(), ReleaseOutcome::Click);
        assert!(state.snap_preview().is_none());
        assert_eq!(*state.gesture(), PointerGesture::None);
    }

    #[test]
    fn a_stale_dwell_result_never_produces_a_preview() {
        let mut state = GestureState::new(4);
        let armed = state.dwell_generation();
        // The cursor moved before the timer fired: the generation advanced.
        assert_eq!(state.move_cursor(Point::new(10, 10)), MoveOutcome::Hover);
        assert_ne!(state.dwell_generation(), armed);
        assert!(
            !state.apply_dwell(
                armed,
                Some((snap_target(), selection(100, 100, 200, 200), Rect::default()))
            ),
            "a timer armed for an older position must be ignored"
        );
        assert!(state.snap_preview().is_none());
    }

    #[test]
    fn a_held_button_blocks_the_dwell_preview() {
        let mut state = GestureState::new(4);
        state.press(Point::new(10, 10), SelectionGeometry::Outside, Rect::default());
        let generation = state.dwell_generation();
        assert!(
            !state.apply_dwell(
                generation,
                Some((snap_target(), selection(1, 1, 2, 2), Rect::default()))
            ),
            "a preview must never appear under a held button"
        );
    }

    #[test]
    fn dwell_replaces_a_preview_only_when_the_rectangle_changes() {
        let mut state = GestureState::new(4);
        let generation = state.dwell_generation();
        assert!(state.apply_dwell(
            generation,
            Some((snap_target(), selection(100, 100, 300, 200), Rect::default()))
        ));
        // Same rectangle: no change, so the overlay can skip the repaint.
        assert!(!state.apply_dwell(
            generation,
            Some((snap_target(), selection(100, 100, 300, 200), Rect::default()))
        ));
        // Another candidate: replaced.
        assert!(state.apply_dwell(
            generation,
            Some((target(4), selection(400, 400, 500, 500), Rect::default()))
        ));
        assert_eq!(state.snap_preview().unwrap().selection, selection(400, 400, 500, 500));
        // Leaving the snap radius clears it.
        assert!(state.apply_dwell(generation, None));
        assert!(state.snap_preview().is_none());
        assert!(!state.apply_dwell(generation, None), "clearing twice is a no-op");
    }

    #[test]
    fn the_preview_carries_the_selection_it_must_be_able_to_restore() {
        let mut state = GestureState::new(4);
        let confirmed = selection(20, 20, 120, 120);
        let generation = state.dwell_generation();
        assert!(state.apply_dwell(
            generation,
            Some((snap_target(), selection(300, 300, 500, 400), confirmed))
        ));
        let preview = state.snap_preview().unwrap();
        assert_eq!(preview.selection, selection(300, 300, 500, 400));
        assert_eq!(
            preview.selection_before_preview, confirmed,
            "a failed confirmation restores exactly this rectangle"
        );
    }

    #[test]
    fn reset_drops_the_preview_and_the_pending_press() {
        let mut state = GestureState::new(4);
        let generation = state.dwell_generation();
        state.apply_dwell(
            generation,
            Some((snap_target(), selection(1, 1, 2, 2), Rect::default())),
        );
        state.press(Point::new(5, 5), SelectionGeometry::Outside, Rect::default());
        state.reset();
        assert_eq!(*state.gesture(), PointerGesture::None);
        assert!(state.snap_preview().is_none());
        // The generation advanced, so a timer armed before the reset cannot fire into
        // the new session.
        assert_ne!(state.dwell_generation(), generation);
    }
}
