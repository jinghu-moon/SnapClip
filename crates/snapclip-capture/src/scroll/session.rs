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
//! * `undo`'s *effect* — §19.6's span history lives in `ViewportState` and moves onto the session in
//!   `P3.09`. `P3.08` lands only the request side (`ScrollController::undo`), because the overlay
//!   thread has to be able to *ask* before anything can answer.

#![allow(dead_code)] // First consumer is the driver (`P3.04`), then the session assembly (`P3.09`).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
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

/// What happens to the recovered pixels when the session stops (`docs/30` §20.5).
///
/// Stop and cancel are the same event as far as the phase is concerned — both are `Stopped` — and
/// they differ in exactly one thing: whether there is an artifact. `Enter` promises a file,
/// `Esc` promises none, and putting that difference in a value the assembly can match on means
/// neither the export path nor the UI has to re-derive it from the stop reason and get it wrong
/// once. PixPin splits the same decision across two user actions ("stop", then "save"); §20.5 keeps
/// it as one decision inside the session, because looking at a partial image before deciding
/// whether to keep it carries no information.
///
/// `docs/30` §27.1's public boundary type is `ScrollOutcome` (final size plus the artifact handle),
/// which belongs to the session assembly (`P3.09`) — a handle is not something this module has.
/// This is the decision `ScrollOutcome` is built on.
pub(crate) enum Disposal<'a> {
    /// `Enter`: the canvas is the result and goes to the export path, possibly as a partial image.
    Export(&'a RecoveredImage),
    /// `Esc`, or an invariant violation: nothing is exported and the pixels are dropped.
    Discard,
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

    /// What to do with the pixels now, or `None` while the session is still running.
    ///
    /// Derived from the stop reason, the way [`phase`] is derived from the session: there is
    /// exactly one place that knows which reasons throw the canvas away, and it is
    /// [`StopReason::yields_partial`] (§20.5). Asking the session rather than the reason keeps the
    /// caller from having to hold the canvas and the reason at the same time.
    pub(crate) fn disposal(&self) -> Option<Disposal<'_>> {
        let reason = self.stop?;
        Some(if reason.yields_partial() {
            Disposal::Export(&self.canvas)
        } else {
            Disposal::Discard
        })
    }
}

/// The overlay thread's handle on a running session (`docs/30` §27.2; task `P3.08`).
///
/// ## Why commands are sticky bits and state is not
///
/// §27.2 calls this "一处刻意的非对称设计" and the asymmetry is the whole design: a **command** is an
/// intent — dropping one loses something the user did — while a **state** update is the newest
/// snapshot of a value, and dropping the older ones costs nothing. One channel cannot carry both,
/// because a capacity-1 slot is right for exactly one of them. So the commands here are **sticky
/// bits**: writing one is an atomic store, nothing can consume it by accident, and no amount of
/// preview traffic can take its place (the two ports share no storage at all).
///
/// Three mechanisms, one per kind of traffic:
///
/// | traffic | mechanism | may wait? | may be lost? |
/// |---|---|---|---|
/// | `stop` / `cancel` / `undo` / `shutdown` | sticky bit, `compare_exchange` / `fetch_add` | no | **no** |
/// | `set_follow` | capacity-1 slot behind a `Mutex` | yes (nanoseconds) | no |
/// | preview updates | capacity-1 slot behind a `try_lock` (`preview.rs`) | **no** | yes, and `dropped` says so |
///
/// ## Why `stop` and `cancel` are one word
///
/// §27.2's sketch has two flags (`AtomicBool stopped` plus a cancel flag), which cannot express an
/// **order** — and §20.5's "the first promise wins" *is* an order: pressing `Enter` and then `Esc`
/// must export, not discard. One `AtomicU32` written with `compare_exchange` makes the first writer
/// the winner without a lock, and makes the losing write a no-op rather than a silent overwrite.
pub(crate) struct ScrollController {
    /// 0 = nothing requested, 1 = `UserStopped`, 2 = `UserCancelled`. First write wins.
    stop: AtomicU32,
    /// How many `undo` presses the driver has not consumed yet (§19.6).
    undo: AtomicU32,
    /// §19.5's "back to the newest": the one command that is a *value* rather than a request, so it
    /// lives in a slot and may be overwritten. `lock()`, not `try_lock()` — see the table above.
    follow: Mutex<Option<bool>>,
    /// Idempotent, and the driver's cue to unwind (§27.2).
    shutdown: AtomicBool,
}

const STOP_NOTHING: u32 = 0;
const STOP_USER_STOPPED: u32 = 1;
const STOP_USER_CANCELLED: u32 = 2;

impl ScrollController {
    pub(crate) fn new() -> Self {
        Self {
            stop: AtomicU32::new(STOP_NOTHING),
            undo: AtomicU32::new(0),
            follow: Mutex::new(None),
            shutdown: AtomicBool::new(false),
        }
    }

    /// "I want the result now" (`Enter`, §20.5). The canvas may be partial.
    pub(crate) fn stop(&self) {
        let _ = self
            .stop
            .compare_exchange(STOP_NOTHING, STOP_USER_STOPPED, Ordering::AcqRel, Ordering::Acquire);
    }

    /// "I want no result" (`Esc`, §20.5). No file is written.
    pub(crate) fn cancel(&self) {
        let _ = self.stop.compare_exchange(
            STOP_NOTHING,
            STOP_USER_CANCELLED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Undo the last confirmed step (§19.6). Presses accumulate: the UI only exposes "undo one
    /// step", but pressing it five times means five steps.
    pub(crate) fn undo(&self) {
        self.undo.fetch_add(1, Ordering::AcqRel);
    }

    /// §19.5: the user dragged the preview box, so automatic following stops — or they asked to
    /// come back to the newest.
    pub(crate) fn set_follow(&self, follow: bool) {
        let mut slot = self.follow.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some(follow);
    }

    /// The driver is being torn down. Idempotent.
    pub(crate) fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
    }

    /// What the user asked for, if anything. The driver reads this at every interruptible point.
    pub(crate) fn requested_stop(&self) -> Option<StopReason> {
        match self.stop.load(Ordering::Acquire) {
            STOP_USER_STOPPED => Some(StopReason::UserStopped),
            STOP_USER_CANCELLED => Some(StopReason::UserCancelled),
            _ => None,
        }
    }

    /// Consume the pending undo presses. Reading is taking: the driver must not replay an undo it
    /// has already applied.
    pub(crate) fn take_undo_requests(&self) -> u32 {
        self.undo.swap(0, Ordering::AcqRel)
    }

    /// Take the follow request, if one is waiting.
    pub(crate) fn take_follow(&self) -> Option<bool> {
        self.follow
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    pub(crate) fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
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

    /// `docs/30` §20.5: stopping and cancelling are **different promises about the same pixels**.
    ///
    /// `Enter` means "give me the result now" — the canvas is the artifact, and it may be a partial
    /// one. `Esc` means "I want nothing" — there is no artifact to write. Both end the session, so
    /// the difference cannot live in the phase (both are `Stopped`); it lives in what the session
    /// hands over, which is what `disposal` answers.
    #[test]
    fn stop_commits_the_export_and_cancel_discards_it() {
        let frame = || {
            Observation::new(
                vec![7u8; 320 * 4 * 4],
                Rect::new(0, 0, 320, 4),
                1,
                (320, 4),
                Axis::Vertical,
            )
            .expect("a packed observation")
        };

        let mut running = ScrollSession::new(Axis::Vertical, 320, MemoryBudget::with_total(1 << 20));
        running.start(&frame());
        assert!(
            running.disposal().is_none(),
            "a running session has not decided anything about its pixels yet"
        );

        let mut stopped = ScrollSession::new(Axis::Vertical, 320, MemoryBudget::with_total(1 << 20));
        stopped.start(&frame());
        stopped.stop(StopReason::UserStopped);

        let mut cancelled =
            ScrollSession::new(Axis::Vertical, 320, MemoryBudget::with_total(1 << 20));
        cancelled.start(&frame());
        cancelled.stop(StopReason::UserCancelled);

        assert_ne!(
            stopped.stop_reason(),
            cancelled.stop_reason(),
            "one reason for `Enter` and another for `Esc`: the two promises are not one 'the user \
             ended it'"
        );
        assert_eq!(
            (phase(&stopped), phase(&cancelled)),
            (Phase::Stopped, Phase::Stopped),
            "§30.2's cancel row is checked against the phase, and both promises end the session — \
             so the phase cannot be what tells them apart"
        );

        let Some(Disposal::Export(canvas)) = stopped.disposal() else {
            panic!("`Enter` commits the export");
        };
        assert_eq!(
            canvas.primary_len(),
            4,
            "the artifact is the content that was recovered, not a handle to something empty"
        );

        assert!(
            matches!(cancelled.disposal(), Some(Disposal::Discard)),
            "`Esc` discards the pixels: there is no partial file to hand over"
        );

        // The same distinction as the export path will ask it.
        assert!(StopReason::UserStopped.yields_partial());
        assert!(!StopReason::UserCancelled.yields_partial());
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

    /// `P3.08`, §27.2: a command **cannot be lost**, no matter what the state channel is doing.
    ///
    /// The load is real (10^5 publishes through the real `publish` path, most of them dropped) and
    /// so is the concurrency (a second thread pumps while this one issues commands). The assertion
    /// holds under *every* interleaving, which is the property being bought: the two ports share no
    /// storage, so no amount of preview traffic can consume a command slot.
    #[test]
    fn commands_are_sticky_and_cannot_be_lost_under_load() {
        let controller = ScrollController::new();
        let preview = crate::scroll::preview::PreviewStream::new();

        std::thread::scope(|scope| {
            scope.spawn(|| {
                for row in 0..100_000u64 {
                    preview.publish(crate::scroll::preview::PreviewUpdate::Span {
                        primary_len: row,
                        steps: row as u32,
                        discarded: 0,
                    });
                }
            });

            controller.stop();
            controller.cancel();
            controller.undo();
            controller.undo();
            controller.undo();
            controller.set_follow(false);
        });

        assert!(
            preview.dropped() > 0,
            "the state channel is supposed to drop under load: {} of 100000 got through",
            100_000 - preview.dropped()
        );
        assert_eq!(
            controller.requested_stop(),
            Some(StopReason::UserStopped),
            "§20.5: the first promise wins — `cancel` must not overwrite `stop` (and a queue \
             would have had to be drained to see this at all)"
        );
        assert_eq!(controller.take_undo_requests(), 3, "three presses, three undos");
        assert_eq!(
            controller.take_undo_requests(),
            0,
            "reading a sticky counter is consuming it: the driver does not replay old undos"
        );
        assert_eq!(
            controller.take_follow(),
            Some(false),
            "`set_follow` is the one command that is a value rather than a request (§27.2)"
        );
        assert_eq!(controller.take_follow(), None);
        assert!(!controller.is_shutdown());
        controller.shutdown();
        assert!(controller.is_shutdown());
        controller.shutdown();
        assert!(controller.is_shutdown(), "`shutdown` is idempotent (§27.2)");
    }

    /// Exit condition ② of `P3.08`, mechanically: the command side is **sticky bits**, not queue
    /// slots. A `Mutex<Option<ScrollCommand>>` would satisfy the two behavioural tests above for
    /// every interleaving that happens to be tried — this is the one that cannot.
    ///
    /// Only the production half is scanned: the test's own source names these types (the same trap
    /// `P3.02`'s `the_decision_table_is_in_one_place` and `P3.04`'s phase test both fell into).
    #[test]
    fn the_command_side_is_sticky_bits_not_a_queue() {
        let production = include_str!("session.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("the file has a production half");
        let sticky =
            production.matches("AtomicBool").count() + production.matches("AtomicU32").count();
        assert!(
            sticky >= 3,
            "§27.2 asks for sticky bits on `Stop`/`Cancel`/`Undo`/`Shutdown`; found {sticky}"
        );
        assert!(
            !production.contains("Mutex<Option<ScrollCommand>>"),
            "a command queue is exactly what §27.2 rules out: a full slot can swallow a `stop`"
        );
    }

    /// The overlay thread writes, the driver thread reads (§27.3's "覆盖层 → driver" direction for
    /// commands), so this is a contract rather than a convenience.
    #[test]
    fn the_controller_crosses_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ScrollController>();
    }
}
