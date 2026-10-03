//! Capture session state machine.
//!
//! Pure logic: no Win32, no GPU, no Tauri. The overlay controller feeds it pointer
//! and keyboard events and reads back the geometry it needs to draw.

use super::geometry::{
    MonitorLayout, Point, Rect, ResizeMode, SelectionDrag, SelectionGeometry, SelectionSnapshot,
};
use super::CaptureError;
use crate::domain::{CaptureArtifact, CapturePayload, CaptureState, PixelFormat};

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

/// Outcome of [`CaptureSession::begin_finish`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinishingOutcome {
    /// The session is in `Finishing` and the caller must produce the artifact for
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
        matches!(self.state, CaptureState::Selecting | CaptureState::Selected)
            && !self.selection.is_empty()
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

    /// Left button pressed. Starts a new selection, a move, or a resize.
    pub fn pointer_pressed(&mut self, point: Point) -> SelectionGeometry {
        if !matches!(self.state, CaptureState::Selecting | CaptureState::Selected) {
            return SelectionGeometry::Outside;
        }
        let snapshot = self.snapshot();
        let geometry = snapshot.hit_test(point, self.dpi());
        let starts_new_selection = matches!(
            geometry,
            SelectionGeometry::Create | SelectionGeometry::Outside
        );
        if starts_new_selection {
            // A press outside the current selection begins a new one, anchored at the
            // press point so the drag follows the pointer.
            self.selection = Rect::new(point.x, point.y, point.x, point.y);
            self.state = CaptureState::Selecting;
        }
        let mode = match geometry {
            SelectionGeometry::Move => ResizeMode::Move,
            SelectionGeometry::Resize(handle) => ResizeMode::Handle(handle),
            SelectionGeometry::Create | SelectionGeometry::Outside => {
                ResizeMode::Handle(super::geometry::Handle::BottomRight)
            }
        };
        // The drag anchors on the selection that is current *after* the press was
        // applied, so the first mouse move does not jump.
        self.drag = Some(SelectionDrag::new(self.selection, self.dpi(), point));
        self.mode = Some(mode);
        geometry
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

    /// `Selected -> Finishing`. Returns the clipped selection to resolve.
    pub fn begin_finish(&mut self) -> Result<FinishingOutcome, CaptureError> {
        if !matches!(self.state, CaptureState::Selected) {
            return Err(CaptureError::InvalidState(format!(
                "cannot confirm while {}",
                self.state.as_str()
            )));
        }
        let selection = self.selection.intersect(self.bounds());
        if selection.is_empty() {
            return Ok(FinishingOutcome::Empty);
        }
        self.state = CaptureState::Finishing;
        Ok(FinishingOutcome::Produce { selection })
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
    use super::{CaptureSession, CapturedFrame, FinishingOutcome, OverlayGeometry};
    use crate::capture::geometry::{MonitorLayout, Point, Rect, SelectionGeometry};
    use crate::domain::{CapturePayload, CaptureState, PixelFormat};

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

    #[test]
    fn happy_path_walks_through_every_state() {
        let mut session = CaptureSession::new("session-1");
        assert_eq!(session.state(), CaptureState::Idle);

        session.arm(frame(), &layout()).unwrap();
        assert_eq!(session.state(), CaptureState::Armed);
        assert_eq!(session.geometry().unwrap(), OverlayGeometry::new(&layout()));

        session.overlay_ready().unwrap();
        assert_eq!(session.state(), CaptureState::Selecting);

        session.pointer_pressed(Point::new(100, 100));
        session.pointer_moved(Point::new(400, 300));
        assert_eq!(session.selection(), Rect::new(100, 100, 400, 300));
        assert_eq!(session.pointer_released(), CaptureState::Selected);

        let outcome = session.begin_finish().unwrap();
        assert_eq!(
            outcome,
            FinishingOutcome::Produce {
                selection: Rect::new(100, 100, 400, 300)
            }
        );
        assert_eq!(session.state(), CaptureState::Finishing);

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
        // `Finishing` is resolving an artifact.
        for state in [
            CaptureState::Preparing,
            CaptureState::Armed,
            CaptureState::Selecting,
            CaptureState::Selected,
            CaptureState::Finishing,
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
                    session.pointer_pressed(Point::new(10, 10));
                    session.pointer_moved(Point::new(200, 200));
                }
                CaptureState::Selected => {
                    session.arm(frame(), &layout()).unwrap();
                    session.overlay_ready().unwrap();
                    session.pointer_pressed(Point::new(10, 10));
                    session.pointer_moved(Point::new(200, 200));
                    session.pointer_released();
                }
                CaptureState::Finishing => {
                    session.arm(frame(), &layout()).unwrap();
                    session.overlay_ready().unwrap();
                    session.pointer_pressed(Point::new(10, 10));
                    session.pointer_moved(Point::new(200, 200));
                    session.pointer_released();
                    session.begin_finish().unwrap();
                }
                CaptureState::Idle => unreachable!("idle is not an active state"),
                CaptureState::Adjusting
                | CaptureState::Annotating
                | CaptureState::Exporting => {
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
            crate::capture::error::CaptureErrorCode::InvalidState
        );
        assert_eq!(session.state(), CaptureState::Preparing);
    }

    #[test]
    fn window_destroy_and_device_removal_use_the_same_cleanup() {
        let mut destroyed = armed_session();
        destroyed.pointer_pressed(Point::new(10, 10));
        destroyed.pointer_moved(Point::new(110, 110));
        destroyed.cancel();
        assert_eq!(destroyed.state(), CaptureState::Idle);

        let mut removed = armed_session();
        removed.pointer_pressed(Point::new(10, 10));
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
            crate::capture::error::CaptureErrorCode::InvalidState
        );
        assert_eq!(session.state(), CaptureState::Selecting);
    }

    #[test]
    fn confirming_without_a_selection_is_rejected() {
        let mut session = armed_session();
        let error = session.begin_finish().unwrap_err();
        assert_eq!(
            error.error_code(),
            crate::capture::error::CaptureErrorCode::InvalidState
        );
    }

    #[test]
    fn empty_drag_does_not_enter_selected() {
        let mut session = armed_session();
        session.pointer_pressed(Point::new(50, 50));
        assert_eq!(session.selection(), Rect::new(50, 50, 50, 50));
        assert_eq!(session.pointer_released(), CaptureState::Selecting);
        assert_eq!(session.selection(), Rect::default());
        // Still confirmable-false but not a hard error: state stays Selecting.
        assert!(session.begin_finish().is_err());
    }

    #[test]
    fn pressing_outside_starts_a_new_selection() {
        let mut session = armed_session();
        session.pointer_pressed(Point::new(100, 100));
        session.pointer_moved(Point::new(300, 300));
        session.pointer_released();
        assert_eq!(session.state(), CaptureState::Selected);

        let hit = session.pointer_pressed(Point::new(800, 800));
        assert_eq!(hit, SelectionGeometry::Outside);
        assert_eq!(session.selection(), Rect::new(800, 800, 800, 800));
        assert_eq!(session.state(), CaptureState::Selecting);
    }

    #[test]
    fn dragging_inside_moves_the_existing_selection() {
        let mut session = armed_session();
        session.pointer_pressed(Point::new(100, 100));
        session.pointer_moved(Point::new(300, 300));
        session.pointer_released();

        assert_eq!(
            session.pointer_pressed(Point::new(200, 200)),
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
        session.pointer_pressed(Point::new(100, 100));
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
            crate::capture::error::CaptureErrorCode::InvalidState
        );
    }

    #[test]
    fn chrome_is_hidden_until_a_selection_exists() {
        let mut session = armed_session();
        assert!(!session.shows_chrome());
        session.pointer_pressed(Point::new(10, 10));
        session.pointer_moved(Point::new(110, 110));
        assert!(session.shows_chrome());
        session.pointer_released();
        assert!(session.shows_chrome());
    }
}


