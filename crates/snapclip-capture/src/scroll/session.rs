//! The scroll session's aggregate root (`docs/30` §20; task `P3.04`).
//!
//! ## No state machine
//!
//! `docs/30` §20.1 decides this explicitly, and the reason is worth restating here because the
//! next person to touch this file will be tempted to add one: a scroll session's non-terminal
//! phases **allow almost the same operations** (stop, cancel, pause, look at the preview, undo).
//! A state machine earns its keep when the set of legal operations differs per state — which is
//! exactly why `CaptureSession` (ordinary screenshots) *is* one: `pointer_released` is legal while
//! selecting and illegal while exporting. Here it would be pseudo-structure: it would freeze a
//! predicate that can be computed from the data into a mode, and then require enter/exit guards,
//! a transition table, and exhaustive transition tests to keep the two in agreement.
//!
//! So: the session holds **data**, and `Phase` is a **pure function** of that data.
//!
//! ## What actually needs to be modal
//!
//! `stop`. A terminal state must **stick** — once stopped, no path leads back to running. That is
//! one `Option<StopReason>` field rather than eight states, because the behaviour after stopping
//! is identical for all eleven reasons (stop capturing, optionally export, drop). If a future
//! requirement appears where some stop reason can be resumed, *that* is the moment a state machine
//! has evidence behind it.
//!
//! ## Not here yet
//!
//! * `target: ScrollTarget` — `docs/30` §20.1's sketch has it, but `target.rs` is `P3.05`.
//! * The invariants of §20.3 that involve the band budget are asserted by `RecoveredImage` itself
//!   (`canvas.rs`, `P1.17`); this module adds the two that are about the *session's* counters.
//! * `undo` — §19.6's span history is `P3.06`.

#![allow(dead_code)] // First consumer is the driver (`P3.04`), then the session assembly (`P3.09`).

use std::time::Instant;

use crate::scroll::canvas::{MemoryBudget, RecoveredImage};
use crate::scroll::observation::{Axis, Observation};

/// Why a session stopped (`docs/30` §20.4, converged to eleven variants by C7).
///
/// The convergence is the point: V1 had a stop reason per failure mode, which meant the UI and the
/// export path each had to know about a growing set. What the eleven have in common is that the
/// behaviour after them is identical — except for two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StopReason {
    /// The user asked for the result (`Enter`).
    UserStopped,
    /// The user asked for nothing (`Esc`). The canvas is **discarded**.
    UserCancelled,
    /// The content stopped changing. §20.4 puts "at the bottom" here, *not* under
    /// [`Self::ActuatorFailed`] — a page that is merely at its end is not a broken actuator.
    EndReached,
    /// The target window is gone or was minimised.
    TargetLost,
    /// Capture failed repeatedly, or every backend failed.
    CaptureFailed,
    /// The graphics device was lost.
    DeviceLost,
    /// Both injection transports failed repeatedly (`P3.03`'s `WatchVerdict::Failed`).
    ActuatorFailed,
    /// The band budget could not be met even after evicting everything evictable.
    MemoryLimit,
    /// The export budget could not be met.
    ExportBudget,
    /// No new frames, or the session ran too long.
    Timeout,
    /// An invariant was violated. The canvas is **discarded** — it is not trustworthy.
    InternalError,
}

impl StopReason {
    /// Whether the canvas survives the stop.
    ///
    /// `UserCancelled` means "I do not want the result", so there is nothing to export.
    /// `InternalError` means the canvas may be internally inconsistent, so exporting it would be
    /// worse than exporting nothing (`G12`: a visible failure beats a silent wrong answer).
    pub(crate) fn yields_partial(self) -> bool {
        !matches!(self, Self::UserCancelled | Self::InternalError)
    }
}

/// Counters for "this is not working" conditions that need to persist across steps.
///
/// `no_progress` is what turns a run of `MatchFailed` steps into a user-visible hint (§20.4: a
/// single failed match never stops the session). `scene_cut` counts consecutive scene cuts, which
/// are handled by §16.8.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Streak {
    pub(crate) no_progress: u8,
    pub(crate) scene_cut: u8,
}

/// What the session is doing, **derived** from its data.
///
/// Three phases, one pure function — versus `docs/19`'s 21-row transition table plus four
/// stale-drop checks. See the module doc for why this is not a state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    /// No confirmed content yet: the first frame has not been committed.
    Preparing,
    /// Content exists and the session has not stopped.
    Running,
    /// A stop reason has been recorded. This phase **sticks**.
    Stopped,
}

/// The scroll session's data. See the module doc.
pub(crate) struct ScrollSession {
    axis: Axis,
    canvas: RecoveredImage,
    step: u32,
    committed: u32,
    discarded: u32,
    streak: Streak,
    last_step_at: Instant,
    stop: Option<StopReason>,
    undo: Vec<u64>,
    follow: bool,
}

impl ScrollSession {
    pub(crate) fn new(axis: Axis, cross_len: u64, budget: MemoryBudget) -> Self {
        Self {
            axis,
            canvas: RecoveredImage::new(axis, cross_len, budget),
            step: 0,
            committed: 0,
            discarded: 0,
            streak: Streak::default(),
            last_step_at: Instant::now(),
            stop: None,
            undo: Vec::new(),
            follow: true,
        }
    }

    /// Whether the session is driving the scroll and the preview shows the newest content (§19.5).
    ///
    /// One flag, two visible consequences, because they are the same question: *is the session
    /// following the page, or is the user looking at something else?* Dragging the preview strip
    /// answers it `false` — and then continuing to inject would move the page under the user, so the
    /// loop stops injecting too. "Back to the newest" answers it `true` again.
    ///
    /// It is **data, not a mode**: nothing branches on a session type, and `phase()` does not read it.
    pub(crate) fn follow(&self) -> bool {
        self.follow
    }

    /// Set the follow flag. `false` is manual mode: `n = 0` and no prior (`docs/30` §13.4).
    pub(crate) fn set_follow(&mut self, follow: bool) {
        self.follow = follow;
    }

    /// Manual mode: the user is the actuator, so we inject nothing and learn nothing (§16.6 rule 2).
    pub(crate) fn manual(&self) -> bool {
        !self.follow
    }

    pub(crate) fn axis(&self) -> Axis {
        self.axis
    }

    pub(crate) fn canvas(&self) -> &RecoveredImage {
        &self.canvas
    }

    pub(crate) fn canvas_mut(&mut self) -> &mut RecoveredImage {
        &mut self.canvas
    }

    pub(crate) fn step(&self) -> u32 {
        self.step
    }

    pub(crate) fn committed(&self) -> u32 {
        self.committed
    }

    pub(crate) fn discarded(&self) -> u32 {
        self.discarded
    }

    pub(crate) fn streak(&self) -> Streak {
        self.streak
    }

    pub(crate) fn streak_mut(&mut self) -> &mut Streak {
        &mut self.streak
    }

    pub(crate) fn last_step_at(&self) -> Instant {
        self.last_step_at
    }

    pub(crate) fn stop_reason(&self) -> Option<StopReason> {
        self.stop
    }

    /// Record the first frame.
    ///
    /// Returns nothing: the first frame cannot be a duplicate of anything, so there is no
    /// `StepWrite` outcome to report. Duplicate detection only becomes meaningful from the second
    /// frame onward, which is `append_confirmed`'s business.
    pub(crate) fn start(&mut self, frame: &Observation) {
        self.last_step_at = Instant::now();
        self.canvas.start(frame);
    }

    /// Count an attempted step.
    pub(crate) fn record_step(&mut self) {
        self.step += 1;
        self.last_step_at = Instant::now();
    }

    /// Count a step whose displacement was accepted.
    pub(crate) fn record_committed(&mut self) {
        self.committed += 1;
        self.streak.no_progress = 0;
    }

    /// Count a step whose displacement was not accepted. **Visible on purpose** (`G12`): a
    /// discarded step is a fact the user can be shown, not something to hide.
    pub(crate) fn record_discarded(&mut self) {
        self.discarded += 1;
        self.streak.no_progress = self.streak.no_progress.saturating_add(1);
    }

    /// Record a stop reason. **The first one wins** — a terminal state is not overwritten.
    pub(crate) fn stop(&mut self, reason: StopReason) {
        if self.stop.is_none() {
            self.stop = Some(reason);
        }
    }
}

/// Derive the phase. Pure; no side effects, no transitions to get out of step with the data.
pub(crate) fn phase(session: &ScrollSession) -> Phase {
    if session.stop.is_some() {
        return Phase::Stopped;
    }
    if session.canvas.primary_len() == 0 {
        // §20.1: `Preparing` means "no confirmed content yet". `primary_len == 0` is that
        // predicate — the canvas has no bands, so it has no rows.
        return Phase::Preparing;
    }
    Phase::Running
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;

    #[test]
    fn phase_is_derived_from_the_data_not_from_a_mode() {
        let mut session = ScrollSession::new(Axis::Vertical, 320, MemoryBudget::with_total(1 << 20));
        assert_eq!(
            phase(&session),
            Phase::Preparing,
            "a session with no confirmed content is preparing"
        );

        session.start(&Observation::new(
            vec![0u8; 320 * 4 * 4],
            Rect::new(0, 0, 320, 4),
            1,
            (320, 4),
            Axis::Vertical,
        ).expect("a packed observation"));

        assert_eq!(
            phase(&session),
            Phase::Running,
            "once the canvas holds content the session is running — no mode change was needed"
        );
    }

    #[test]
    fn the_terminal_phase_sticks() {
        let mut session = ScrollSession::new(Axis::Vertical, 320, MemoryBudget::with_total(1 << 20));
        session.start(&Observation::new(
            vec![0u8; 320 * 4 * 4],
            Rect::new(0, 0, 320, 4),
            1,
            (320, 4),
            Axis::Vertical,
        ).expect("a packed observation"));

        session.stop(StopReason::UserStopped);
        assert_eq!(phase(&session), Phase::Stopped);

        // The canvas still has content, and `phase` still says stopped: the stop is a fact about
        // the session, not something derived from how much was recovered.
        session.stop(StopReason::EndReached);
        assert_eq!(
            phase(&session),
            Phase::Stopped,
            "the first stop reason wins; a terminal state does not get overwritten"
        );
        assert_eq!(session.stop_reason(), Some(StopReason::UserStopped));
    }

    #[test]
    fn only_cancel_and_internal_error_discard_the_canvas() {
        let yields = [
            (StopReason::UserStopped, true),
            (StopReason::UserCancelled, false),
            (StopReason::EndReached, true),
            (StopReason::TargetLost, true),
            (StopReason::CaptureFailed, true),
            (StopReason::DeviceLost, true),
            (StopReason::ActuatorFailed, true),
            (StopReason::MemoryLimit, true),
            (StopReason::ExportBudget, true),
            (StopReason::Timeout, true),
            (StopReason::InternalError, false),
        ];

        assert_eq!(yields.len(), 11, "§20.4 converges on eleven reasons");
        for (reason, expected) in yields {
            assert_eq!(reason.yields_partial(), expected, "{reason:?}");
        }
    }

    #[test]
    fn the_counters_add_up() {
        let mut session = ScrollSession::new(Axis::Vertical, 320, MemoryBudget::with_total(1 << 20));
        assert_eq!(session.step(), 0);

        session.record_step();
        session.record_committed();
        session.record_step();
        session.record_discarded();

        assert_eq!(session.step(), 2);
        assert_eq!(session.committed(), 1);
        assert_eq!(session.discarded(), 1);
        assert_eq!(
            session.committed() + session.discarded(),
            session.step(),
            "§20.3 invariant 5: committed + discarded == step"
        );
    }

    #[test]
    fn the_session_has_one_phase_and_no_state_machine() {
        // The production half only: this test's own source contains the very strings it searches
        // for, so scanning the whole file would count them twice (`P3.02` hit the same thing).
        let production = |source: &'static str| {
            source
                .split("#[cfg(test)]")
                .next()
                .expect("a file always has a first half")
        };
        let source = production(include_str!("session.rs"));
        let driver = production(include_str!("loop_control.rs"));

        let phase_definitions = source.matches("enum Phase").count();
        assert_eq!(
            phase_definitions, 1,
            "the session's phase must be defined exactly once"
        );
        assert_eq!(
            source.matches("enum ScrollState").count() + driver.matches("enum ScrollState").count(),
            0,
            "§20.1: the session holds data and derives its phase; there is no ScrollState"
        );
        assert_eq!(
            source.matches("fn phase").count(),
            1,
            "the phase must be derived in exactly one place"
        );
    }

    /// §19.5: dragging the preview strip stops the session following the newest content; the
    /// "back to the newest" button resumes it.
    ///
    /// The flag is the whole mode. There is no second session type, no second loop and no second
    /// phase: a manual session is this one with `n = 0` and no prior (§13.4).
    #[test]
    fn the_follow_flag_is_the_whole_mode() {
        let mut session = ScrollSession::new(Axis::Vertical, 320, MemoryBudget::with_total(1 << 20));
        assert!(
            session.follow(),
            "a session drives the scroll until the user says otherwise"
        );
        assert!(!session.manual());

        session.set_follow(false);
        assert!(!session.follow());
        assert!(session.manual(), "not following is what manual mode is");
        assert_eq!(
            phase(&session),
            Phase::Preparing,
            "the mode is not a phase: the session is still just starting"
        );

        session.set_follow(true);
        assert!(session.follow(), "\"back to the newest\" resumes the automatic loop");
        assert!(!session.manual());
    }
}
