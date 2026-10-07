//! Capture session state machine.
//!
//! Pure logic: no Win32, no GPU, no Tauri. The overlay controller feeds it pointer
//! and keyboard events and reads back the geometry it needs to draw.

use super::geometry::{
    MonitorLayout, Point, Rect, ResizeMode, SelectionDrag, SelectionGeometry, SelectionSnapshot,
};
use super::CaptureError;
use snapclip_model::{CaptureArtifact, CapturePayload, CaptureState, PixelFormat};

/// One frozen back-buffer frame: exactly what the overlay presents while the user
/// selects. It is captured once, before the overlay becomes visible, so the
/// overlay can never appear inside its own screenshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub pixel_format: PixelFormat,
    pub captured_at_unix_ms: i64,
    pub provider: &'static str,
}

impl CapturedFrame {
    pub fn rect(&self) -> Rect {
        Rect::from_origin_size(Point::new(0, 0), self.width as i32, self.height as i32)
    }
}

/// Per-session overlay constants derived from the monitor and its DPI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayGeometry {
    /// Monitor-local frame the back buffer covers.
    pub frame: Rect,
    /// Monitor-local work area used to keep labels and the magnifier visible.
    pub work_area: Rect,
    pub dpi: u32,
}

impl OverlayGeometry {
    pub fn new(layout: &MonitorLayout) -> Self {
        Self {
            frame: layout.local_bounds(),
            work_area: layout.local_work_area(),
            dpi: layout.dpi,
        }
    }
}

/// Outcome of [`CaptureSession::begin_export`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportOutcome {
    /// The session is in `Exporting` and the caller must produce the artifact for
    /// this clipped selection from the frozen frame.
    Produce { selection: Rect },
    /// Nothing usable is selected; the session returns to `Selected`.
    Empty,
}

/// Lifecycle of a single capture session.
#[derive(Debug, Clone)]
pub struct CaptureSession {
    id: String,
    state: CaptureState,
    frame: Option<CapturedFrame>,
    geometry: Option<OverlayGeometry>,
    selection: Rect,
    drag: Option<SelectionDrag>,
    mode: Option<ResizeMode>,
}

impl CaptureSession {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            state: CaptureState::Idle,
            frame: None,
            geometry: None,
            selection: Rect::default(),
            drag: None,
            mode: None,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn state(&self) -> CaptureState {
        self.state
    }

    pub fn frame(&self) -> Option<&CapturedFrame> {
        self.frame.as_ref()
    }

    pub fn geometry(&self) -> Option<OverlayGeometry> {
        self.geometry.as_ref().copied()
    }

    /// Selection in monitor-local physical pixels; empty when nothing is selected.
    pub fn selection(&self) -> Rect {
        self.selection
    }

    /// Whether the overlay currently paints a selection.
    pub fn has_selection(&self) -> bool {
        matches!(
            self.state,
            CaptureState::Selecting | CaptureState::Selected | CaptureState::Annotating
        ) && !self.selection.is_empty()
    }

    /// Whether the overlay should paint the selection chrome (border, handles,
    /// size label, magnifier). Not painted before the first drag.
    pub fn shows_chrome(&self) -> bool {
        self.has_selection()
    }

    pub fn dpi(&self) -> u32 {
        self.geometry.map(|geometry| geometry.dpi).unwrap_or(96)
    }

    pub fn snapshot(&self) -> SelectionSnapshot {
        SelectionSnapshot::new(self.selection, self.dpi())
    }

    /// What a click at `point` would do, without changing any state.
    pub fn pointer_hit(&self, point: Point) -> SelectionGeometry {
        self.snapshot().hit_test(point, self.dpi())
    }

    /// `Idle -> Preparing`: the hotkey fired and a capture worker request was
    /// submitted. The overlay keeps pumping messages in this state and Esc can
    /// cancel before any frame exists (docs/11 §2.2).
    pub fn preparing(&mut self) -> Result<(), CaptureError> {
        if self.state.is_active() {
            return Err(CaptureError::InvalidState(format!(
                "session {} is already {}",
                self.id,
                self.state.as_str()
            )));
        }
        self.state = CaptureState::Preparing;
        Ok(())
    }

    /// `(Idle | Preparing) -> Armed`. Fails when a session is already running
    /// past `Preparing`.
    pub fn arm(
        &mut self,
        frame: CapturedFrame,
        layout: &MonitorLayout,
    ) -> Result<(), CaptureError> {
        if self.state.is_active() && self.state != CaptureState::Preparing {
            return Err(CaptureError::InvalidState(format!(
                "session {} is already {}",
                self.id,
                self.state.as_str()
            )));
        }
        self.geometry = Some(OverlayGeometry::new(layout));
        self.frame = Some(frame);
        self.selection = Rect::default();
        self.drag = None;
        self.mode = None;
        self.state = CaptureState::Armed;
        Ok(())
    }

    /// `Armed -> Selecting`: the overlay is visible and can accept input.
    pub fn overlay_ready(&mut self) -> Result<(), CaptureError> {
        if self.state != CaptureState::Armed {
            return Err(CaptureError::InvalidState(format!(
                "overlay became ready while {}",
                self.state.as_str()
            )));
        }
        self.state = CaptureState::Selecting;
        Ok(())
    }

    pub fn pointer_moved(&mut self, point: Point) {
        if let (Some(mode), Some(drag)) = (self.mode, self.drag) {
            let bounds = self.bounds();
            let rect = drag.apply(mode, point, bounds);
            if rect != self.selection {
                self.selection = rect;
            }
        }
    }

    /// What a press at `point` would do. **Never mutates the selection.**
    ///
    /// The overlay records a gesture and only calls [`Self::begin_drag`] once the pointer
    /// has actually crossed the drag threshold (docs/14 §4.2). That removes the old
    /// "press creates a zero-size selection" path, which made a click-to-confirm
    /// indistinguishable from the start of a drag and destroyed any snap preview.
    pub fn press(&mut self, point: Point) -> SelectionGeometry {
        if !matches!(self.state, CaptureState::Selecting | CaptureState::Selected) {
            return SelectionGeometry::Outside;
        }
        self.snapshot().hit_test(point, self.dpi())
    }

    /// Start a drag once the gesture has been decided.
    ///
    /// `creating` is true when the press landed outside an existing selection. The new
    /// selection is anchored at the press point *now* — after the threshold, not at press
    /// time — and the drag anchors on the resulting rectangle so the first move does not
    /// jump.
    pub fn begin_drag(&mut self, press_point: Point, mode: ResizeMode, creating: bool) -> bool {
        if !matches!(self.state, CaptureState::Selecting | CaptureState::Selected) {
            return false;
        }
        if creating {
            self.selection =
                Rect::new(press_point.x, press_point.y, press_point.x, press_point.y);
            self.state = CaptureState::Selecting;
        }
        self.drag = Some(SelectionDrag::new(self.selection, self.dpi(), press_point));
        self.mode = Some(mode);
        true
    }

    /// Adopt a selection produced by automatic window snapping (docs/14 §5.4).
    ///
    /// Clipped to the captured monitor; rejected when the result is empty or smaller than
    /// the minimum usable selection, so a window that was mid-close cannot leave the
    /// session with a degenerate rectangle.
    pub fn snap_to(&mut self, rect: Rect) -> bool {
        if !matches!(self.state, CaptureState::Selecting | CaptureState::Selected) {
            return false;
        }
        let clipped = rect.intersect(self.bounds());
        if clipped.is_empty() {
            return false;
        }
        let minimum = SelectionSnapshot::new(clipped, self.dpi()).minimum_size();
        if clipped.width() < minimum || clipped.height() < minimum {
            return false;
        }
        self.selection = clipped;
        self.state = CaptureState::Selected;
        self.drag = None;
        self.mode = None;
        true
    }

    /// Left button released. Non-empty selections move the session to `Selected`.
    pub fn pointer_released(&mut self) -> CaptureState {
        self.drag = None;
        self.mode = None;
        if matches!(self.state, CaptureState::Selecting) {
            if self.selection.is_empty() {
                self.selection = Rect::default();
            } else {
                self.state = CaptureState::Selected;
            }
        }
        self.state
    }

    pub fn pointer_left(&mut self) {
        self.drag = None;
        self.mode = None;
    }

    /// `Selected -> Annotating`: the selection is locked and object annotations can
    /// be created / edited on top of it (docs/11 §8). Requires a non-empty selection.
    pub fn begin_annotating(&mut self) -> Result<(), CaptureError> {
        if self.state != CaptureState::Selected {
            return Err(CaptureError::InvalidState(format!(
                "cannot annotate while {}",
                self.state.as_str()
            )));
        }
        if self.selection.is_empty() {
            return Err(CaptureError::InvalidState("nothing selected".into()));
        }
        // Selection drags are over: drop any in-flight selection drag so the
        // pointer now drives the annotation document instead.
        self.drag = None;
        self.mode = None;
        self.state = CaptureState::Annotating;
        Ok(())
    }

    /// `Selected -> Exporting`. Returns the clipped selection to resolve.
    ///
    /// Entering `Exporting` is the overlay's signal to stop painting: from here the
    /// frozen frame is being handed to the export worker, and a repaint would race the
    /// region readback for the single-threaded D3D11 context.
    pub fn begin_export(&mut self) -> Result<ExportOutcome, CaptureError> {
        if !matches!(self.state, CaptureState::Selected | CaptureState::Annotating) {
            return Err(CaptureError::InvalidState(format!(
                "cannot confirm while {}",
                self.state.as_str()
            )));
        }
        let selection = self.selection.intersect(self.bounds());
        if selection.is_empty() {
            return Ok(ExportOutcome::Empty);
        }
        self.state = CaptureState::Exporting;
        Ok(ExportOutcome::Produce { selection })
    }

    /// Build the domain artifact for a finished session.
    pub fn artifact(
        &self,
        selection: Rect,
        payload: CapturePayload,
    ) -> Result<CaptureArtifact, CaptureError> {
        let frame = self
            .frame
            .as_ref()
            .ok_or_else(|| CaptureError::InvalidState("session has no captured frame".into()))?;
        let clipped = super::geometry::validate_selection(selection, frame.rect())?;
        Ok(CaptureArtifact {
            session_id: self.id.clone(),
            width: clipped.width() as u32,
            height: clipped.height() as u32,
            dpi: self.dpi(),
            pixel_format: frame.pixel_format,
            captured_at_unix_ms: frame.captured_at_unix_ms,
            monitor_device_name: None,
            payload,
        })
    }

    /// Return to `Idle` after the artifact was produced.
    ///
    /// Named for the Phase 3 contract: the artifact is delivered by the export worker,
    /// so completion is a distinct step from entering `Exporting`.
    pub fn complete(&mut self) {
        self.reset();
    }

    /// Return to `Idle` after a failure, keeping the session id for diagnostics.
    pub fn fail(&mut self) {
        self.reset();
    }

    /// `* -> Idle`. Used by Esc, window destruction and device removal alike so all
    /// cancellation paths converge on one cleanup routine.
    pub fn cancel(&mut self) {
        self.reset();
    }

    /// Reject input for a monitor rectangle that this session does not own.
    pub fn bounds(&self) -> Rect {
        self.geometry.map(|geometry| geometry.frame).unwrap_or_default()
    }

    fn reset(&mut self) {
        self.state = CaptureState::Idle;
        self.frame = None;
        self.geometry = None;
        self.selection = Rect::default();
        self.drag = None;
        self.mode = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{CaptureSession, CapturedFrame, ExportOutcome, OverlayGeometry};
    use crate::geometry::{Handle, MonitorLayout, Point, Rect, ResizeMode, SelectionGeometry};
    use snapclip_model::{CapturePayload, CaptureState, PixelFormat};

    fn layout() -> MonitorLayout {
        MonitorLayout {
            bounds: Rect::new(0, 0, 1920, 1080),
            work_area: Rect::new(0, 0, 1920, 1040),
            dpi: 96,
            primary: true,
        }
    }

    fn frame() -> CapturedFrame {
        CapturedFrame {
            width: 1920,
            height: 1080,
            pixel_format: PixelFormat::Bgra8Unorm,
            captured_at_unix_ms: 1_700_000_000_000,
            provider: "test",
        }
    }

    fn armed_session() -> CaptureSession {
        let mut session = CaptureSession::new("session-1");
        session.arm(frame(), &layout()).unwrap();
        session.overlay_ready().unwrap();
        session
    }

    /// Test helper: the production press → threshold → drag sequence in one call.
    ///
    /// A press on its own never changes the selection (that is the point of the gesture
    /// refactor), so tests that expect a drag have to begin it explicitly — exactly what
    /// the overlay does when the pointer crosses the drag threshold.
    fn press(session: &mut CaptureSession, point: Point) -> SelectionGeometry {
        let hit = session.press(point);
        let mode = match hit {
            SelectionGeometry::Move => ResizeMode::Move,
            SelectionGeometry::Resize(handle) => ResizeMode::Handle(handle),
            SelectionGeometry::Create | SelectionGeometry::Outside => {
                ResizeMode::Handle(Handle::BottomRight)
            }
        };
        let creating = matches!(
            hit,
            SelectionGeometry::Create | SelectionGeometry::Outside
        );
        session.begin_drag(point, mode, creating);
        hit
    }

    #[test]
    fn happy_path_walks_through_every_state() {
        let mut session = CaptureSession::new("session-1");
        assert_eq!(session.state(), CaptureState::Idle);

        session.arm(frame(), &layout()).unwrap();
        assert_eq!(session.state(), CaptureState::Armed);
        assert_eq!(session.geometry().unwrap(), OverlayGeometry::new(&layout()));

        session.overlay_ready().unwrap();
        assert_eq!(session.state(), CaptureState::Selecting);

        press(&mut session, Point::new(100, 100));
        session.pointer_moved(Point::new(400, 300));
        assert_eq!(session.selection(), Rect::new(100, 100, 400, 300));
        assert_eq!(session.pointer_released(), CaptureState::Selected);

        let outcome = session.begin_export().unwrap();
        assert_eq!(
            outcome,
            ExportOutcome::Produce {
                selection: Rect::new(100, 100, 400, 300)
            }
        );
        assert_eq!(session.state(), CaptureState::Exporting);

        let artifact = session
            .artifact(
                Rect::new(100, 100, 400, 300),
                CapturePayload::PngFile {
                    path: "artifact.png".into(),
                },
            )
            .unwrap();
        assert_eq!((artifact.width, artifact.height), (300, 200));
        assert_eq!(artifact.session_id, "session-1");
        assert_eq!(artifact.dpi, 96);

        session.complete();
        assert_eq!(session.state(), CaptureState::Idle);
        assert!(session.frame().is_none());
        assert_eq!(session.selection(), Rect::default());
    }

    #[test]
    fn esc_from_every_active_state_returns_to_idle() {
        // Each case builds the session up to the state under test, then cancels.
        // `Preparing` is the worker round trip; `Armed` has no selection yet;
        // `Selecting` is mid-drag; `Selected` has a committed selection;
        // `Exporting` has handed its pixels to the export worker.
        for state in [
            CaptureState::Preparing,
            CaptureState::Armed,
            CaptureState::Selecting,
            CaptureState::Selected,
            CaptureState::Annotating,
            CaptureState::Exporting,
        ] {
            let mut session = CaptureSession::new("session-1");
            session.preparing().unwrap();
            match state {
                CaptureState::Preparing => {}
                CaptureState::Armed => {
                    session.arm(frame(), &layout()).unwrap();
                }
                CaptureState::Selecting => {
                    session.arm(frame(), &layout()).unwrap();
                    session.overlay_ready().unwrap();
                    press(&mut session, Point::new(10, 10));
                    session.pointer_moved(Point::new(200, 200));
                }
                CaptureState::Selected => {
                    session.arm(frame(), &layout()).unwrap();
                    session.overlay_ready().unwrap();
                    press(&mut session, Point::new(10, 10));
                    session.pointer_moved(Point::new(200, 200));
                    session.pointer_released();
                }
                CaptureState::Exporting => {
                    session.arm(frame(), &layout()).unwrap();
                    session.overlay_ready().unwrap();
                    press(&mut session, Point::new(10, 10));
                    session.pointer_moved(Point::new(200, 200));
                    session.pointer_released();
                    session.begin_export().unwrap();
                }
                CaptureState::Idle => unreachable!("idle is not an active state"),
                CaptureState::Annotating => {
                    session.arm(frame(), &layout()).unwrap();
                    session.overlay_ready().unwrap();
                    press(&mut session, Point::new(10, 10));
                    session.pointer_moved(Point::new(200, 200));
                    session.pointer_released();
                    session.begin_annotating().unwrap();
                }
                CaptureState::Adjusting => {
                    unreachable!("{state:?} belongs to the docs/11 contract but is not reachable until its phase lands")
                }
            }
            assert_eq!(session.state(), state);

            session.cancel();
            assert_eq!(session.state(), CaptureState::Idle, "{state:?}");
            assert!(session.geometry().is_none());
            assert!(session.frame().is_none());
            assert_eq!(session.selection(), Rect::default());
        }
    }

    #[test]
    fn preparing_accepts_the_frame_and_arms_the_session() {
        let mut session = CaptureSession::new("session-1");
        session.preparing().unwrap();
        assert_eq!(session.state(), CaptureState::Preparing);
        session.arm(frame(), &layout()).unwrap();
        assert_eq!(session.state(), CaptureState::Armed);
    }

    #[test]
    fn preparing_cannot_be_entered_twice() {
        let mut session = CaptureSession::new("session-1");
        session.preparing().unwrap();
        let error = session.preparing().unwrap_err();
        assert_eq!(
            error.error_code(),
            crate::error::CaptureErrorCode::InvalidState
        );
        assert_eq!(session.state(), CaptureState::Preparing);
    }

    #[test]
    fn window_destroy_and_device_removal_use_the_same_cleanup() {
        let mut destroyed = armed_session();
        press(&mut destroyed, Point::new(10, 10));
        destroyed.pointer_moved(Point::new(110, 110));
        destroyed.cancel();
        assert_eq!(destroyed.state(), CaptureState::Idle);

        let mut removed = armed_session();
        press(&mut removed, Point::new(10, 10));
        removed.pointer_moved(Point::new(110, 110));
        removed.fail();
        assert_eq!(removed.state(), CaptureState::Idle);
        assert!(removed.frame().is_none());
    }

    #[test]
    fn arming_an_active_session_is_rejected() {
        let mut session = armed_session();
        let error = session.arm(frame(), &layout()).unwrap_err();
        assert_eq!(
            error.error_code(),
            crate::error::CaptureErrorCode::InvalidState
        );
        assert_eq!(session.state(), CaptureState::Selecting);
    }

    #[test]
    fn confirming_without_a_selection_is_rejected() {
        let mut session = armed_session();
        let error = session.begin_export().unwrap_err();
        assert_eq!(
            error.error_code(),
            crate::error::CaptureErrorCode::InvalidState
        );
    }

    #[test]
    fn a_press_alone_never_creates_a_zero_size_selection() {
        let mut session = armed_session();
        let hit = session.press(Point::new(50, 50));
        assert_eq!(hit, SelectionGeometry::Create);
        assert_eq!(
            session.selection(),
            Rect::default(),
            "the press must not create a zero-size selection (docs/14 §4.2)"
        );
        assert_eq!(session.state(), CaptureState::Selecting);
        // Releasing without crossing the drag threshold is a click: nothing is committed.
        assert_eq!(session.pointer_released(), CaptureState::Selecting);
        assert_eq!(session.selection(), Rect::default());
        assert!(session.begin_export().is_err());
    }

    #[test]
    fn pressing_outside_keeps_the_selection_until_the_drag_starts() {
        let mut session = armed_session();
        press(&mut session, Point::new(100, 100));
        session.pointer_moved(Point::new(300, 300));
        session.pointer_released();
        assert_eq!(session.state(), CaptureState::Selected);
        let confirmed = Rect::new(100, 100, 300, 300);

        // The press alone changes nothing: a click outside must not erase the confirmed
        // selection (this is the defect the gesture refactor removes).
        let hit = session.press(Point::new(800, 800));
        assert_eq!(hit, SelectionGeometry::Outside);
        assert_eq!(session.selection(), confirmed);

        // Only crossing the drag threshold replaces it, anchored at the press point.
        assert!(session.begin_drag(
            Point::new(800, 800),
            ResizeMode::Handle(Handle::BottomRight),
            true,
        ));
        session.pointer_moved(Point::new(900, 900));
        assert_eq!(session.selection(), Rect::new(800, 800, 900, 900));
        assert_eq!(session.state(), CaptureState::Selecting);
    }

    #[test]
    fn snap_to_adopts_a_usable_rectangle_and_relies_on_the_clip_for_edges() {
        let mut session = armed_session();
        assert!(session.snap_to(Rect::new(100, 100, 400, 300)));
        assert_eq!(session.state(), CaptureState::Selected);
        assert_eq!(session.selection(), Rect::new(100, 100, 400, 300));

        // A window hanging off the captured monitor is clipped, not rejected.
        let mut session = armed_session();
        assert!(session.snap_to(Rect::new(-100, -50, 500, 400)));
        assert_eq!(session.selection(), Rect::new(0, 0, 500, 400));
    }

    #[test]
    fn snap_to_rejects_degenerate_rectangles_and_leaves_the_session_untouched() {
        let mut session = armed_session();
        assert!(!session.snap_to(Rect::default()));
        assert!(!session.snap_to(Rect::new(10, 10, 10, 400)), "zero width");
        assert!(!session.snap_to(Rect::new(10, 10, 400, 10)), "zero height");
        assert!(!session.snap_to(Rect::new(9000, 9000, 9500, 9500)), "off the monitor");
        assert!(!session.snap_to(Rect::new(20, 20, 22, 22)), "below the minimum size");
        assert_eq!(session.state(), CaptureState::Selecting);
        assert_eq!(session.selection(), Rect::default());
    }

    #[test]
    fn dragging_inside_moves_the_existing_selection() {
        let mut session = armed_session();
        press(&mut session, Point::new(100, 100));
        session.pointer_moved(Point::new(300, 300));
        session.pointer_released();

        assert_eq!(
            press(&mut session, Point::new(200, 200)),
            SelectionGeometry::Move
        );
        session.pointer_moved(Point::new(250, 250));
        session.pointer_released();
        assert_eq!(session.selection(), Rect::new(150, 150, 350, 350));
        assert_eq!(session.state(), CaptureState::Selected);
    }

    #[test]
    fn artifact_rejects_a_selection_outside_the_frozen_frame() {
        let mut session = armed_session();
        press(&mut session, Point::new(100, 100));
        session.pointer_moved(Point::new(300, 300));
        session.pointer_released();
        let error = session
            .artifact(
                Rect::new(5000, 5000, 5100, 5100),
                CapturePayload::PngFile {
                    path: "artifact.png".into(),
                },
            )
            .unwrap_err();
        assert_eq!(
            error.error_code(),
            crate::error::CaptureErrorCode::InvalidState
        );
    }

    #[test]
    fn chrome_is_hidden_until_a_selection_exists() {
        let mut session = armed_session();
        assert!(!session.shows_chrome());
        press(&mut session, Point::new(10, 10));
        session.pointer_moved(Point::new(110, 110));
        assert!(session.shows_chrome());
        session.pointer_released();
        assert!(session.shows_chrome());
    }

    #[test]
    fn annotating_locks_selection_and_stays_confirmable() {
        let mut session = armed_session();
        press(&mut session, Point::new(100, 100));
        session.pointer_moved(Point::new(400, 300));
        assert_eq!(session.pointer_released(), CaptureState::Selected);

        session.begin_annotating().unwrap();
        assert_eq!(session.state(), CaptureState::Annotating);
        // Chrome keeps painting and the selection is unchanged.
        assert!(session.shows_chrome());
        assert_eq!(session.selection(), Rect::new(100, 100, 400, 300));
        // A pointer press in Annotating does not re-open a selection drag.
        assert_eq!(press(&mut session, Point::new(500, 500)), SelectionGeometry::Outside);
        assert_eq!(session.selection(), Rect::new(100, 100, 400, 300));

        // Confirm from Annotating produces the same clipped selection.
        let outcome = session.begin_export().unwrap();
        assert_eq!(
            outcome,
            ExportOutcome::Produce {
                selection: Rect::new(100, 100, 400, 300)
            }
        );
    }

    #[test]
    fn annotating_requires_a_selected_session() {
        let mut session = armed_session();
        // Still Selecting (no committed selection) -> rejected.
        assert!(session.begin_annotating().is_err());
        press(&mut session, Point::new(10, 10));
        session.pointer_moved(Point::new(100, 100));
        session.pointer_released();
        assert!(session.begin_annotating().is_ok());
        // Cannot enter twice.
        assert!(session.begin_annotating().is_err());
    }
}


