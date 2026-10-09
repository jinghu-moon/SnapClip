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
//! * The invariants of §20.3 that involve the band budget are asserted by `RecoveredImage` itself
//!   (`canvas.rs`, `P1.17`); this module adds the two that are about the *session's* counters.
//! * `undo`'s *effect*: §19.6's span history stays in `ViewportState` (`canvas.rs`), because that is
//!   the only object that knows both the viewport position and the canvas. `P3.08` landed the request
//!   side (`ScrollController::undo`, because the overlay thread has to be able to *ask*), and `P3.09`'s
//!   driver is what answers — it drains the requests and calls `ViewportState::undo_last` per step.
//!   The session itself owns only the request counters, not the history.

// Everything here is `pub(crate)` and consumed by the assembly (`ScrollRuntime`, this file) plus the
// driver (`loop_control.rs`). From the library target alone the chain is unreachable — `ScrollRuntime`
// has no production caller until `P4`/`P5` wire it up — so the allow is the honest way to say "the
// consumer is the binary, not the library" rather than deleting the vocabulary and re-adding it.
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::scroll::canvas::{MemoryBudget, RecoveredImage};
use crate::scroll::loop_control::{ScrollDriver, RENDER_TICK_MS};
use crate::scroll::observation::{Axis, Observation};
use crate::scroll::ports::{FrameSource, ScrollActuator};
use crate::scroll::preview::PreviewStream;

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
    /// How long the driver took to confirm a cancel, once one was asked for (§23.2's
    /// `Cancel latency`).
    ///
    /// It is measured by the loop, not derived from anything here, because only the loop knows when
    /// it regained control: the check points are the loop's, and an injection that has already gone
    /// out cannot be recalled (`docs/30` §21.4). Stored on the session rather than published as a
    /// diagnostic because the number has to survive the session — `P3`'s exit condition is a
    /// *measured* maximum, and a value that only existed while the driver ran could not be checked.
    cancel_latency: Option<Duration>,
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
            cancel_latency: None,
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

    /// Record how long the driver took to confirm a cancel (`docs/30` §23.2's `Cancel latency`).
    ///
    /// The first measurement wins, for the same reason the first stop reason does: a session that has
    /// been cancelled is over, and a second number would be measuring something else (the next loop
    /// iteration noticing a flag that is already set).
    pub(crate) fn record_cancel_latency(&mut self, latency: Duration) {
        if self.cancel_latency.is_none() {
            self.cancel_latency = Some(latency);
        }
    }

    /// The measured `Cancel latency`, if a cancel was ever confirmed.
    ///
    /// `None` is not zero: it means no cancel was asked for, and reporting it as `0 ms` would make
    /// §23.2's maximum look satisfied by sessions that were never cancelled at all.
    pub(crate) fn cancel_latency(&self) -> Option<Duration> {
        self.cancel_latency
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
    /// When the user asked to cancel (§21.4), written by whichever `cancel()` wins the CAS.
    ///
    /// A separate slot from `stop` because the two answer different questions: `stop` is "what
    /// should this session end with", which is sticky and read once; this is "when did the user
    /// ask", which is what §23.2's `Cancel latency` is measured *from*. Deriving the instant from
    /// the loop instead would measure how long the loop took to look, which is precisely the thing
    /// the metric is supposed to expose rather than assume.
    cancel_at: Mutex<Option<Instant>>,
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
            cancel_at: Mutex::new(None),
        }
    }

    /// "I want the result now" (`Enter`, §20.5). The canvas may be partial.
    pub(crate) fn stop(&self) {
        let _ = self
            .stop
            .compare_exchange(STOP_NOTHING, STOP_USER_STOPPED, Ordering::AcqRel, Ordering::Acquire);
    }

    /// "I want no result" (`Esc`, §20.5). No file is written.
    ///
    /// The instant is stamped here, on the **caller's** thread — the overlay's key handler — because
    /// that is the only moment that is the user's rather than the loop's.
    ///
    /// ## Order: the instant becomes visible before the bit does
    ///
    /// The loop reads the sticky bit and then asks for the instant, so a bit that can be seen
    /// before its instant is a cancel with no latency — and `None` is not zero, so §23.2's Max
    /// would be satisfied by the cancels that raced. The first version published the bit with a
    /// `compare_exchange` and only then took the mutex, which is exactly that window; it is
    /// microseconds wide and the parked-driver test hit it.
    ///
    /// So the store happens first, under the lock, and a `compare_exchange` that loses **puts the
    /// previous value back**: only the cancel that won gets an instant, and it is in place before
    /// the bit that advertises it. A second `Esc` therefore cannot move the instant forward and
    /// make the latency look smaller, which is what the CAS was there for.
    pub(crate) fn cancel(&self) {
        let mut slot = self.cancel_at.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = *slot;
        *slot = Some(Instant::now());
        if self
            .stop
            .compare_exchange(
                STOP_NOTHING,
                STOP_USER_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            *slot = previous;
        }
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

    /// The instant the user asked to cancel, or `None` (`docs/30` §21.4).
    ///
    /// Polled by the loop at every interruptible point, and it is the *user's* instant rather than
    /// the loop's on purpose: the difference between the two **is** §23.2's `Cancel latency`, and a
    /// value the loop stamped itself could not measure it.
    pub(crate) fn cancellation(&self) -> Option<Instant> {
        *self
            .cancel_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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

/// Everything a session needs before it has a thread (`docs/30` §20.1's session sketch; task `P3.09`).
///
/// It exists so that the driver's dependencies are all decided **once**, in one place, by the
/// assembly — and so that a test can build a session without a desktop, a window handle or a DPI
/// query. The two ports are *not* in here: they are per-session values that arrive at
/// [`ScrollRuntime::start`], because they are the things a test substitutes.
pub(crate) struct ScrollPlan {
    axis: Axis,
    cross_len: u64,
    viewport_extent: u32,
    budget: MemoryBudget,
    /// The wheel calibration the control law starts from (§13.2): `ĝ₀ = lines_per_notch ×
    /// line_height_px`.
    lines_per_notch: u32,
    line_height_px: u32,
}

impl ScrollPlan {
    /// The defaults are the *measured* ones, not the plausible ones: `lines_per_notch = 3` is this
    /// machine's `SPI_GETWHEELSCROLLLINES` and `line_height_px = 20` is the line height the control
    /// law's worked example uses (§13.2) — `starting_px_per_notch(3, 20) = 60`.
    ///
    /// They are still only a starting point: `ĝ` is learned from confirmed steps, so a wrong default
    /// costs a few steps rather than the session (`E-CTRL-1` measured convergence in ≤ 10 steps for
    /// every starting value within about 2× of the truth). [`Self::with_wheel`] exists for the
    /// assembly, which can do better than a constant by probing the target.
    pub(crate) fn new(axis: Axis, cross_len: u64, viewport_extent: u32, budget: MemoryBudget) -> Self {
        Self {
            axis,
            cross_len,
            viewport_extent,
            budget,
            lines_per_notch: 3,
            line_height_px: 20,
        }
    }

    /// Set the wheel calibration from a probe rather than from a constant (§13.2, `P3.09`'s assembly).
    pub(crate) fn with_wheel(mut self, lines_per_notch: u32, line_height_px: u32) -> Self {
        self.lines_per_notch = lines_per_notch.max(1);
        self.line_height_px = line_height_px.max(1);
        self
    }

    pub(crate) fn axis(&self) -> Axis {
        self.axis
    }

    pub(crate) fn cross_len(&self) -> u64 {
        self.cross_len
    }

    pub(crate) fn viewport_extent(&self) -> u32 {
        self.viewport_extent
    }

    pub(crate) fn budget(&self) -> MemoryBudget {
        self.budget
    }

    pub(crate) fn lines_per_notch(&self) -> u32 {
        self.lines_per_notch
    }

    pub(crate) fn line_height_px(&self) -> u32 {
        self.line_height_px
    }
}

/// A running session: the driver thread, the two ports, and the session it will hand back.
///
/// ## Why the thread is here and not in the driver
///
/// `ScrollDriver` is a pure loop over its two arguments (`scroll/loop_control.rs`), so a test can run
/// it on the test's own thread with a scripted source and actuator. `ScrollRuntime` is the part that
/// needs a real thread, a real desktop and a real teardown order — and keeping that split means the
/// loop's tests do not need any of the three.
///
/// ## Why the session is built before the thread starts
///
/// The canvas owns a spill directory, and the directory is created eagerly (`BandStore::new`,
/// `canvas.rs:383`). Building the session here — on the caller's thread, before `spawn` — is what
/// makes "the session owns exactly one directory" a fact the caller can observe the instant `start`
/// returns. Building it *inside* the thread would make the same statement a race, and every teardown
/// test would be testing the scheduler.
///
/// ## Teardown
///
/// `teardown` sets the shutdown bit and joins. `Drop` calls it and throws the session away. Both are
/// the same code path on purpose: §20.3's audit found the existing workers have no `Drop` at all and
/// their join handles sit in a `Mutex<Option<..>>` that a controller can simply forget to take, which
/// makes "the caller remembered" the resource policy. A session that holds a thread and a directory
/// cannot afford that.
pub(crate) struct ScrollRuntime {
    controller: std::sync::Arc<ScrollController>,
    preview: std::sync::Arc<PreviewStream>,
    /// `None` only after a successful teardown — the handle is taken, not merely inspected, so the
    /// join happens exactly once.
    driver: Option<std::thread::JoinHandle<ScrollSession>>,
}

impl ScrollRuntime {
    /// Start a driver thread and return the handles that talk to it.
    ///
    /// ## Why the frame source arrives as a factory
    ///
    /// `make_source` runs **on the driver thread**, and that is the whole reason it is a closure
    /// rather than a value. A real frame source owns a D3D11 device and a WGC session, neither of
    /// which is `Send`: the device's immediate context belongs to exactly one thread (`docs/30`
    /// §21.3), and the session is opened by whichever thread first activates WinRT on it (§21.5 —
    /// and the driver thread is a plain `std::thread` whose first activation establishes the
    /// implicit MTA, which is the state the rest of the process already relies on). Handing a
    /// pre-built source across the boundary would move a context that has an owner, and the
    /// compiler is right to refuse it. Building it here keeps the two facts — one owner, one
    /// apartment — true by construction instead of by convention.
    ///
    /// The actuator stays a value: it is `SendInput`/`PostMessageW` over plain integers, with no
    /// apartment and no device behind it.
    pub(crate) fn start<F, M, A>(plan: ScrollPlan, make_source: M, actuator: A) -> Self
    where
        M: FnOnce() -> F + Send + 'static,
        F: FrameSource + 'static,
        A: ScrollActuator + Send + 'static,
    {
        let controller = std::sync::Arc::new(ScrollController::new());
        let preview = std::sync::Arc::new(PreviewStream::new());
        let driver = ScrollDriver::new(&plan);

        let thread_controller = std::sync::Arc::clone(&controller);
        let thread_preview = std::sync::Arc::clone(&preview);
        let handle = std::thread::Builder::new()
            // Named so that a hung session is identifiable in a stack dump rather than being one more
            // anonymous thread in a process that already has four workers (§21.1).
            .name("snapclip-scroll-driver".to_owned())
            .spawn(move || {
                let source = make_source();
                driver.run(source, actuator, &thread_controller, &thread_preview)
            })
            .expect("the scroll driver thread is spawnable");

        Self {
            controller,
            preview,
            driver: Some(handle),
        }
    }

    pub(crate) fn controller(&self) -> &std::sync::Arc<ScrollController> {
        &self.controller
    }

    pub(crate) fn preview(&self) -> &std::sync::Arc<PreviewStream> {
        &self.preview
    }

    /// Has the driver thread finished? `false` is "still working", and it is what the teardown test
    /// asserts *before* tearing down: a runtime whose thread never started would satisfy "no threads
    /// leak" trivially.
    pub(crate) fn driver_exited(&self) -> bool {
        self.driver.as_ref().is_none_or(std::thread::JoinHandle::is_finished)
    }

    /// Ask the driver to stop and wait for it, returning the session it produced.
    ///
    /// Returns `None` when called twice: the second call has no thread to join and no session to hand
    /// back, and inventing an empty session would be a lie about what happened.
    pub(crate) fn teardown(&mut self) -> Option<ScrollSession> {
        self.controller.shutdown();
        let handle = self.driver.take()?;
        handle.join().ok()
    }
}

impl Drop for ScrollRuntime {
    fn drop(&mut self) {
        // The result is discarded and that is not a leak: the session owns the spill directory, and
        // dropping it here is what removes the directory (§20.3).
        drop(self.teardown());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;
    use crate::scroll::ports::{
        FrameError, InjectOutcome, InjectPath, InjectStatus, Poll,
    };

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

    /// Every spill directory this process owns right now.
    ///
    /// `BandStore`'s directory name is `snapclip-bands-{pid}-{nanos}` (`canvas.rs`'s
    /// `unique_spill_dir`), so a snapshot of that set is the whole question "did this session clean
    /// up after itself" — and it is asked of the *filesystem*, not of a flag the session sets. A
    /// session that forgot to drop its store would pass any assertion written against its own
    /// bookkeeping.
    fn spill_dirs() -> std::collections::BTreeSet<std::path::PathBuf> {
        let prefix = format!("snapclip-bands-{}-", std::process::id());
        std::fs::read_dir(std::env::temp_dir())
            .expect("the system temp directory is readable")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&prefix))
            })
            .collect()
    }

    /// A source that never has anything new, and never ends.
    ///
    /// It is what makes the teardown test about teardown: the driver has no work to do, so the only
    /// reason it ever leaves its loop is the command port — which is exactly the path under test.
    struct IdleSource {
        viewport: Rect,
    }

    impl FrameSource for IdleSource {
        fn next(&mut self, _timeout: std::time::Duration) -> Result<Poll, FrameError> {
            Ok(Poll::Idle)
        }

        fn viewport(&self) -> Rect {
            self.viewport
        }
    }

    /// An actuator that records what it was asked to do and never moves anything.
    struct SilentActuator {
        injections: u32,
    }

    impl ScrollActuator for SilentActuator {
        type Path = InjectPath;

        fn path(&self) -> InjectPath {
            InjectPath::SendInput
        }

        fn switch(&mut self, _from: InjectPath) -> Option<InjectPath> {
            None
        }

        fn inject(&mut self, _notches: i32) -> InjectOutcome {
            self.injections += 1;
            InjectOutcome::failed(InjectStatus::TargetNotFound)
        }
    }

    fn plan() -> ScrollPlan {
        ScrollPlan::new(
            Axis::Vertical,
            320,
            200,
            MemoryBudget::with_total(1 << 20),
        )
    }

    /// `docs/30` §20.3's "析构即删临时文件" and §21.1's driver thread, asserted as one property:
    /// **a torn-down session leaves nothing behind** — not a thread, not a directory.
    ///
    /// Three things are checked, and each one is a different failure:
    ///
    /// * the driver is *running* before the teardown (a runtime that never started a thread would
    ///   satisfy "no threads leak" trivially);
    /// * the session owns a spill directory while it lives (otherwise "it was removed" is vacuous);
    /// * the directory is gone once the session is dropped, and the thread has been joined.
    ///
    /// The stop reason is `UserCancelled`, and that is a decision rather than a default: a session
    /// torn down without a prior promise produces **no artifact** (§20.5), which is exactly what
    /// `UserCancelled` promises. §4.3.7 froze the vocabulary at eleven reasons, so the honest move is
    /// to reuse the one whose promise matches, not to add a twelfth that only one call site produces.
    #[test]
    fn a_scroll_session_tears_down_without_leaking_bands_or_threads() {
        let before = spill_dirs();

        let mut runtime = ScrollRuntime::start(
            plan(),
            || IdleSource {
                viewport: Rect::new(0, 0, 320, 200),
            },
            SilentActuator { injections: 0 },
        );

        assert!(
            !runtime.driver_exited(),
            "the driver thread is what makes this a session rather than a struct"
        );
        let during = spill_dirs();
        assert_eq!(
            during.len(),
            before.len() + 1,
            "a live session owns exactly one spill directory; before={before:?} during={during:?}"
        );

        let session = runtime
            .teardown()
            .expect("the driver hands its session back");
        assert!(
            runtime.driver_exited(),
            "teardown joins the driver; a session whose thread is still running has not ended"
        );
        assert_eq!(
            session.stop_reason(),
            Some(StopReason::UserCancelled),
            "a teardown promises no artifact (§20.5), which is `UserCancelled`'s promise"
        );

        drop(session);
        assert_eq!(
            spill_dirs(),
            before,
            "the canvas' spill directory outlived the session (§20.3: 析构即删临时文件)"
        );
    }

    /// Teardown is reachable twice, and the second time is `Drop`'s.
    ///
    /// §20.3's audit found the existing workers have **no** `Drop` at all and their `JoinHandle`s
    /// sit in a `Mutex<Option<..>>` that one controller can forget to take (§20.3 item 9). A scroll
    /// session is long-lived and holds a thread and a directory, so "the caller remembered" is not a
    /// resource policy — this is the case that says so.
    #[test]
    fn dropping_a_runtime_tears_the_session_down() {
        let before = spill_dirs();
        {
            let runtime = ScrollRuntime::start(
                plan(),
                || IdleSource {
                    viewport: Rect::new(0, 0, 320, 200),
                },
                SilentActuator { injections: 0 },
            );
            assert!(!runtime.driver_exited());
        }
        assert_eq!(
            spill_dirs(),
            before,
            "`Drop` must do what `teardown` does: no directory, no thread, no residue"
        );
    }

    /// A command that arrives before the loop has anything to do is still honoured: `shutdown` is a
    /// sticky bit, so it cannot be missed by a driver that is parked waiting for frames.
    #[test]
    fn shutdown_ends_a_parked_driver() {
        let mut runtime = ScrollRuntime::start(
            plan(),
            || IdleSource {
                viewport: Rect::new(0, 0, 320, 200),
            },
            SilentActuator { injections: 0 },
        );
        runtime.controller().shutdown();
        let session = runtime
            .teardown()
            .expect("a shutdown driver still returns its session");
        assert_eq!(session.stop_reason(), Some(StopReason::UserCancelled));
        assert!(runtime.driver_exited());
    }

    /// A cancel that arrives while the driver is parked is still a **measured** cancel.
    ///
    /// The loop notices a cancel in two places: inside `inject_and_settle`'s tick check, and at the
    /// top of the loop — which is where a driver that is still waiting for its first frame lives.
    /// Stamping the latency at only the first of them makes `cancel_latency()` read `None` for
    /// exactly the sessions a user is most likely to produce (the screen has not moved yet, so
    /// there is nothing for the settle loop to notice), and `None` is not zero: §23.2's Max would
    /// then be "satisfied" by the sessions that never measured anything. Found while building the
    /// real-desktop instrument (`P3.10`), which cannot report a Max it is unable to collect.
    #[test]
    fn a_cancel_while_the_driver_is_parked_still_reports_a_latency() {
        let mut runtime = ScrollRuntime::start(
            plan(),
            || IdleSource {
                viewport: Rect::new(0, 0, 320, 200),
            },
            SilentActuator { injections: 0 },
        );

        // Parked inside `await_first_frame`: no injection has happened and none ever will, so the
        // only place that can notice is the command poll at the top of the loop.
        std::thread::sleep(Duration::from_millis(5));
        runtime.controller().cancel();

        let session = runtime
            .teardown()
            .expect("a cancelled driver still returns its session");
        assert_eq!(session.stop_reason(), Some(StopReason::UserCancelled));
        let latency = session
            .cancel_latency()
            .expect("a cancelled session must carry the latency it took to notice");
        assert!(
            latency <= Duration::from_millis(RENDER_TICK_MS as u64 * 4),
            "a parked driver notices a cancel on its next tick, not after a step timeout: {latency:?}"
        );
    }

    /// The cancel bit and its instant are published in an order the loop can rely on.
    ///
    /// Single-threaded, so it cannot *reproduce* the race — it pins the invariant instead: whenever
    /// `requested_stop` reports a cancel, `cancellation` already has the instant. The loop reads the
    /// two in that order, and a bit that arrives first is a cancel whose latency reads `None`, which
    /// §23.2 counts as "not measured" and the acceptance gate then treats as "no evidence of a
    /// problem". The cross-thread version of the same claim is
    /// `a_cancel_while_the_driver_is_parked_still_reports_a_latency`.
    #[test]
    fn a_cancel_publishes_its_instant_with_its_bit() {
        let controller = ScrollController::new();
        assert_eq!(controller.requested_stop(), None);
        assert_eq!(controller.cancellation(), None);

        controller.cancel();
        assert_eq!(controller.requested_stop(), Some(StopReason::UserCancelled));
        let first = controller
            .cancellation()
            .expect("the instant must be visible by the time the bit is");

        // A second `Esc` must not move the instant forward and make the latency look smaller.
        controller.cancel();
        assert_eq!(
            controller.cancellation(),
            Some(first),
            "the metric measures the user's *first* press"
        );

        // A cancel that lost to `stop` is not a cancel at all, so it must not invent an instant:
        // a `Some` here would report a latency for a session the user asked to *finish*.
        let stopped = ScrollController::new();
        stopped.stop();
        stopped.cancel();
        assert_eq!(stopped.requested_stop(), Some(StopReason::UserStopped));
        assert_eq!(stopped.cancellation(), None);
    }
}
