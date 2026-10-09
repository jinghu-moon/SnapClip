//! The loop's control decisions (`docs/30` §13.2, §24.7; task `P3.03`).
//!
//! This module is the **first** thing in `scroll/` that is about the session's control flow rather
//! than about pixels, so it is also the first place where the §28.4 gate bites: it may not name
//! `windows`, `InjectPath` or anything else platform-shaped. That is not a formality — the
//! actuator lives in `windows/scroll_actuator.rs` precisely because it calls `SendInput`, and the
//! watchdog below has to make its decision without being able to call it.
//!
//! The split that makes this work: the watchdog decides **what happened** and **that the current
//! transport is not working**, and it asks its caller for the next transport instead of knowing
//! that there are exactly two. "Re-run `choose()`" (`docs/30` §24.7) is therefore the caller's
//! job, and the platform knowledge stays on the platform side of the boundary.
//!
//! ## Why the watchdog exists at all
//!
//! `Posted` is weak evidence (`docs/30` §24.6.3): `PostMessageW` returning success means the
//! message was *queued*, and `SendInput` returning `1` means the event was *inserted into the
//! system queue*. Neither says the content moved. `docs/30` §24.7 turns "does not prove" into
//! "prove it with the next step's content displacement", and this module is that sentence as code.
//!
//! ## Not here yet
//!
//! * `EndReached`: §20.4's convergence table puts "content stopped changing" under `EndReached`,
//!   not under `ActuatorFailed`. The caller must classify a step as "the page is at its end"
//!   **before** feeding it to this watchdog; a page that is merely at its bottom would otherwise
//!   be reported as a broken actuator. Deciding that needs the scroll-position channel, which is
//!   `P3.04`'s delivery.
//!
//!   `P3.09`'s driver inherits this gap rather than closing it: a step whose settle window expires
//!   with no new observation arrives as `StepError::NoFrame`, and the driver counts it as a discarded
//!   step for the watchdog to judge. A session that reaches the bottom of a page therefore ends as
//!   `ActuatorFailed` once the transport switches are exhausted, which is **wrong** — it is listed as
//!   open here so that nobody reads the current behaviour as a decision.

// The loop's only caller is `ScrollRuntime` (`session.rs`), which itself has no production caller
// until the assembly lands (`P4`/`P5`). From the library target alone the whole file is unreachable,
// so the allow states the fact instead of hiding it.
#![allow(dead_code)]

use std::time::{Duration, Instant};

use crate::scroll::canvas::{StepWrite, ViewportState};
use crate::scroll::displacement::{
    estimate, line_digest, Displacement, Prior, SceneCut, Scratch, Status, StepEffect, RHO_MIN,
};
use crate::scroll::observation::Observation;
use crate::scroll::ports::{EndReason, FrameSource, InjectStatus, Poll, ScrollActuator};
use crate::scroll::preview::{PreviewStream, PreviewUpdate};
use crate::scroll::session::{ScrollController, ScrollPlan, ScrollSession, StopReason};

/// The compositor tick the overlay runs on (`crates/snapclip-capture/src/windows/overlay.rs:100`).
///
/// Duplicated as a number rather than imported, because importing it would mean `scroll/` naming
/// the platform module — which §28.4 forbids. It is a fact about the *system's* frame rate, not
/// about our overlay: the value is here to state the lower bound for [`STILL_WINDOW`].
pub(crate) const RENDER_TICK_MS: u32 = 15;

/// How far apart two agreeing samples must be to count as evidence of stillness.
///
/// `docs/30` §13.3: 40 ms is the smallest value that spans more than one 16.7 ms composition
/// period. Two identical samples *inside* one composition period are one picture read twice, not
/// a still picture.
pub(crate) const STILL_WINDOW: Duration = Duration::from_millis(40);

/// How long a step may take to settle before the loop gives up waiting.
///
/// `docs/30` §13.3. A smooth-scroll animation that never settles must be cut off rather than
/// waited on forever; the frame read after a timeout is still usable, it is just not *proven*
/// still.
pub(crate) const STEP_TIMEOUT: Duration = Duration::from_millis(400);

/// What the loop is still waiting for after it injected a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettleVerdict {
    /// Keep sampling.
    Waiting,
    /// Two samples at least [`STILL_WINDOW`] apart showed the same picture.
    Still,
    /// [`STEP_TIMEOUT`] elapsed without that happening.
    TimedOut,
}

/// Decides when the picture has stopped moving, from frame fingerprints alone.
///
/// **Why fingerprints and not "the displacement stopped changing"** (`docs/30` §13.3): the latter
/// needs an estimated displacement to judge stability, while estimating a displacement needs a
/// stable frame as input — a circular dependency. A row fingerprint is
/// **displacement-independent**, so it can be taken first.
///
/// Time is a parameter rather than a call to `Instant::now()` inside, which is what makes every
/// case below testable without sleeping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Settle {
    started: Option<Instant>,
    reference: Option<(u64, Instant)>,
}

impl Settle {
    pub(crate) fn new() -> Self {
        Self {
            started: None,
            reference: None,
        }
    }

    /// Begin a new wait. Called once per step, right after the injection.
    pub(crate) fn begin(&mut self, now: Instant) {
        self.started = Some(now);
        self.reference = None;
    }

    /// Feed one reading of the target's pixels.
    ///
    /// `digest` fingerprints the whole viewport; equal digests mean no pixel changed.
    pub(crate) fn sample(&mut self, digest: u64, now: Instant) -> SettleVerdict {
        let started = self.started.unwrap_or(now);
        if now.duration_since(started) >= STEP_TIMEOUT {
            return SettleVerdict::TimedOut;
        }

        match self.reference {
            None => {
                self.reference = Some((digest, now));
                SettleVerdict::Waiting
            }
            Some((_, at)) if now.duration_since(at) < STILL_WINDOW => {
                // Too close together to be two independent observations.
                SettleVerdict::Waiting
            }
            Some((previous, _)) if previous == digest => SettleVerdict::Still,
            Some(_) => {
                // The picture moved. The new sample becomes the reference: what we need is two
                // *consecutive* agreeing samples, not two that happen to agree across a change.
                self.reference = Some((digest, now));
                SettleVerdict::Waiting
            }
        }
    }

    /// Whether this wait has spent its budget.
    ///
    /// Separate from [`Self::sample`] because the loop also has to give up when **no** frame
    /// arrives at all — there is nothing to sample in that case.
    pub(crate) fn timed_out(&self, now: Instant) -> bool {
        match self.started {
            Some(started) => now.duration_since(started) >= STEP_TIMEOUT,
            None => false,
        }
    }
}

/// What the loop needs from the platform, so that `scroll/` stays platform-free (§28.4).
///
/// The Windows side is `windows/scroll_source.rs` (frames) and `windows/scroll_actuator.rs`
/// (injection), joined by the session assembly (`P3.09`). Nothing here names a handle, a message,
/// or a transport: those are precisely the things that would make this module untestable without
/// an interactive desktop (§29.2).
pub(crate) trait StepHost {
    /// Deliver `notches` wheel notches through the transport the caller selected.
    fn inject(&mut self, notches: i32) -> Result<(), String>;

    /// Fingerprint the target's pixels as they are now, or `None` if no new frame arrived.
    ///
    /// `None` is `Poll::Idle` (§11.1): the picture has not changed since the last read, which is
    /// *evidence of stillness*, not a failure.
    fn digest(&mut self) -> Option<u64>;

    /// A monotonic clock.
    ///
    /// A trait method rather than a call to `Instant::now()` inside the loop, because the loop's
    /// whole subject is time passing: with the clock injected, the 40 ms and 400 ms rules can be
    /// exercised in microseconds instead of by sleeping.
    fn now(&self) -> Instant;

    /// The instant the user asked to cancel, if they have (`docs/30` §21.4).
    ///
    /// Polled at every interruptible point: before the injection, right after it, on every tick of
    /// the settle wait, and before matching. It is a **poll and not a signal** on purpose — neither
    /// `SendInput` nor `PostMessageW` can be recalled once it has gone out, so there is nothing for
    /// an interrupt to interrupt. The instant returned is when the *user* asked, not when the loop
    /// noticed: the difference between the two **is** the `Cancel latency` metric of §23.2, and
    /// only the loop can measure it.
    ///
    /// `None` means "keep going". `Some` is sticky — a cancelled session never resumes — and the
    /// method is deliberately **required** rather than defaulted to `None`: a host that forgot to
    /// answer it would make every cancellation look like a step that simply kept going.
    fn cancellation(&self) -> Option<Instant>;
}

/// How a step's wait ended, and what the target looked like when it did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Settled {
    pub(crate) verdict: SettleVerdict,
    pub(crate) digest: u64,
}

/// Why a step could not be brought to the point of estimating.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StepError {
    /// The transport could not deliver the request at all.
    ///
    /// The caller decides what that means — `P3.03`'s watchdog is the thing that turns a run of
    /// these into a transport switch or an `ActuatorFailed`.
    Injection(String),
    /// The step's budget expired without a single frame.
    ///
    /// Deliberately distinct from [`SettleVerdict::TimedOut`]: there we *have* a frame and merely
    /// cannot prove it is still; here there is nothing to estimate at all.
    NoFrame,
    /// The user cancelled while this step was in flight, and this is how long it took the loop to
    /// confirm it (`docs/30` §21.4, §23.2).
    ///
    /// The duration is **measured, not derived**: it runs from the instant the user asked to the
    /// instant the loop regained control and looked. It is reported rather than recomputed by the
    /// caller because only the loop knows when it got control back — and it is not zero, because an
    /// injection that has already gone out cannot be recalled and a readback that is already in
    /// flight has to return.
    Cancelled { latency: Duration },
}

/// Inject a step and wait for the target to stop moving (`docs/30` §13.3).
///
/// This is the half of the loop that has to be frame-driven. Sleeping a fixed amount and hoping
/// would either estimate mid-animation — committing an intermediate picture into the canvas — or
/// spend a fixed budget on a page that settled immediately.
///
/// It injects **once**. Retrying, switching transports, and deciding that a step was wasted are
/// `P3.03`'s watchdog and `P3.04`'s caller, not this function: a wait that also injected would make
/// "how many notches went out" unanswerable, and §13.2's control law needs that number.
pub(crate) fn inject_and_settle<H: StepHost>(
    host: &mut H,
    settle: &mut Settle,
    notches: i32,
) -> Result<Settled, StepError> {
    // A cancel that arrived before this step began must not be answered with another injection:
    // the promise was "no more scrolling", and §13.4's manual route is not the only caller that
    // has to honour it.
    if let Some(cancelled) = cancellation_error(host) {
        return Err(cancelled);
    }

    // §13.4: in manual mode there is nothing to inject, and this is where that becomes mechanical.
    // The actuator is not called with zero notches — that is an invalid request by its own contract
    // (`scroll_actuator.rs`), and a manual step must not send one.
    if notches != 0 {
        host.inject(notches).map_err(StepError::Injection)?;
    }
    settle.begin(host.now());

    loop {
        if let Some(cancelled) = cancellation_error(host) {
            return Err(cancelled);
        }
        match host.digest() {
            Some(digest) => match settle.sample(digest, host.now()) {
                SettleVerdict::Waiting => continue,
                verdict => return Ok(Settled { verdict, digest }),
            },
            None => {
                if settle.timed_out(host.now()) {
                    return Err(StepError::NoFrame);
                }
            }
        }
    }
}

/// The cancel to report, if the user asked for one, with how long the loop took to notice.
///
/// One helper rather than two inline constructions, so that "the cancel is checked here" and "the
/// latency is measured here" cannot drift apart: every check point is a measurement, and a check
/// point added without one would be a latency the table never sees.
fn cancellation_error<H: StepHost>(host: &H) -> Option<StepError> {
    let asked_at = host.cancellation()?;
    Some(StepError::Cancelled {
        latency: host.now().saturating_duration_since(asked_at),
    })
}

/// How many notches this step asks for, given who is driving the scroll (`docs/30` §13.4).
///
/// Manual mode is **not a second loop**: it is this loop with `n = 0`. The difference lives in two
/// numbers rather than in a branch on a session type, and this is the first of them — the session's
/// follow flag decides whether the control law's answer reaches the actuator at all.
pub(crate) fn step_notches(session: &ScrollSession, control: &Control) -> i32 {
    if session.manual() {
        0
    } else {
        control.notches()
    }
}

/// Feed a step's outcome back to the control law, and report whether it was used.
///
/// This is the second of manual mode's two differences. `ĝ` is *pixels per notch*, so learning it
/// needs the number of notches we sent (`docs/30` §16.6 rule 2). In manual mode we sent none: the
/// user scrolled, and `d` says what the content did — not what one notch does. Dividing by a number
/// we chose ourselves would be inventing evidence, so the prior is left exactly as it was.
pub(crate) fn learn(
    session: &ScrollSession,
    control: &mut Control,
    notches: i32,
    status: Status,
) -> bool {
    if session.manual() {
        false
    } else {
        control.observe(notches, status)
    }
}

/// How many consecutive ineffective steps through one transport before it is abandoned.
///
/// `docs/30` §24.7. **Why three and not one**: `Posted` is *weak* evidence — the message was
/// queued, or the event was inserted into the system queue. A single ineffective step has several
/// innocent explanations that are not "the transport is wrong" (a frame that arrived before the
/// injection landed, a page that reflowed, a step the estimator refused for a reason that has
/// nothing to do with injection). Switching on the first one would make the transport oscillate
/// and would turn a matching problem into an injection problem.
///
/// This is a **starting value, not a calibrated one**: `E-CTRL-1` owns the calibration (`P3.07`),
/// and the same constant is what `§30.2`'s "注入失败后切路径" row is written against.
pub(crate) const POSTED_STEPS_BEFORE_SWITCH: u32 = 3;

/// How many times the watchdog may switch before it stops believing the caller.
///
/// Three transports is the number the measurements support: the two injection paths of `§24.6`
/// plus the keyboard fallback (`VK_DOWN`, measured at 320 px on Chromium in `§24.6.1`). The cap
/// exists because the watchdog cannot tell a caller that has a genuine third transport from one
/// that is alternating between the same two forever — and a session that oscillates is worse than
/// one that stops and says so (`G12`).
pub(crate) const MAX_SWITCHES: u32 = 2;

/// What the loop saw after it injected a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StepOutcome<Path> {
    /// The content moved as asked. The step is the loop's business, not this module's.
    Moved,
    /// Injected through `Path`, but the content did not move.
    ///
    /// This covers both halves of `§24.7`'s condition: a displacement that was not `Confirmed`,
    /// and one that was `Confirmed` at `d == 0`.
    Unmoved(Path),
}

/// What the loop should do next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WatchVerdict<Path> {
    /// Carry on with the current transport.
    Continue,
    /// Abandon `from` and re-inject through `to`. The caller records `InjectPathSwitched`
    /// (`§26.3`) — this module does not own the diagnostic type, which is the session layer's.
    Switched { from: Path, to: Path },
    /// No transport is left. The caller stops the session with `StopReason::ActuatorFailed` and
    /// exports what it has (`§20.4`).
    Failed,
}

/// Counts consecutive ineffective steps and decides when the transport is the problem.
///
/// Generic over the transport so that `scroll/` never names `InjectPath` (`§28.4`). The caller
/// supplies the next transport through the `next` callback, which is where "re-run `choose()`"
/// (`§24.7`) lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ActuatorWatch<Path> {
    current: Path,
    streak: u32,
    switches: u32,
}

impl<Path: Copy + PartialEq> ActuatorWatch<Path> {
    pub(crate) fn new(current: Path) -> Self {
        Self {
            current,
            streak: 0,
            switches: 0,
        }
    }

    /// The transport in use.
    pub(crate) fn current(&self) -> Path {
        self.current
    }

    /// Consecutive ineffective steps through the current transport.
    pub(crate) fn streak(&self) -> u32 {
        self.streak
    }

    /// How many times the transport has been switched this session.
    pub(crate) fn switches(&self) -> u32 {
        self.switches
    }

    /// Record one step.
    ///
    /// `next` is asked only when the threshold is reached, and it is asked for a transport *other
    /// than the current one*; returning `None`, returning the current transport, or having already
    /// spent the switch budget all mean the same thing to this watchdog: there is nowhere left to
    /// go (`WatchVerdict::Failed`).
    pub(crate) fn observe<F>(&mut self, outcome: StepOutcome<Path>, next: F) -> WatchVerdict<Path>
    where
        F: FnOnce(Path) -> Option<Path>,
    {
        match outcome {
            StepOutcome::Moved => {
                self.streak = 0;
                WatchVerdict::Continue
            }
            StepOutcome::Unmoved(path) => {
                // The payload is authoritative: a step injected through a stale transport must
                // count against *that* transport, not against the one we think we are using.
                self.current = path;
                self.streak += 1;
                if self.streak < POSTED_STEPS_BEFORE_SWITCH {
                    return WatchVerdict::Continue;
                }
                if self.switches >= MAX_SWITCHES {
                    return WatchVerdict::Failed;
                }
                match next(path) {
                    Some(candidate) if candidate != path => {
                        self.switches += 1;
                        self.current = candidate;
                        self.streak = 0;
                        WatchVerdict::Switched {
                            from: path,
                            to: candidate,
                        }
                    }
                    _ => WatchVerdict::Failed,
                }
            }
        }
    }
}

// --- §13.2's control law (task P3.05) ---

/// The most notches one step may carry, which is the wire format's limit rather than ours.
///
/// `WM_MOUSEWHEEL`'s `mouseData` holds the delta in a signed 16-bit field, so 32767 / 120 = 273
/// notches is the most a single message can express. Duplicated as a number for the same reason
/// [`RENDER_TICK_MS`] is (naming the platform module here is forbidden by §28.4); a test on the
/// actuator side asserts the two are equal, so the duplication cannot drift.
pub(crate) const MAX_NOTCHES_PER_STEP: i32 = 273;

/// The order-of-magnitude starting value for `ĝ`, in pixels per wheel notch.
///
/// `docs/30` §13.2: the initial `ĝ` is unknown, and the first step is taken with `n = 1` precisely
/// because of that. This is not a measurement — it is a **starting value** built from the two things
/// the platform tells us (`SPI_GETWHEELSCROLLLINES`, `P3.02`'s capability probe) and one thing we
/// assume (a line of text is about twenty pixels tall). It only has to be the right order of
/// magnitude: the loop converges from wherever it starts, and starting closer saves steps.
///
/// The line height is a parameter rather than a constant because the honest answer is "we do not
/// know it until we have looked at a frame", and a caller that has looked should pass what it saw.
pub(crate) fn starting_px_per_notch(lines_per_notch: u32, line_height_px: u32) -> f32 {
    (lines_per_notch.max(1) * line_height_px.max(1)) as f32
}

/// `docs/30` §13.2's control law: decides how much to inject, then learns how much that moved.
///
/// Two facts are load-bearing, and both are properties of the type rather than of a caller's
/// discipline:
///
/// * **`observe` takes a [`Status`], not a displacement.** A step the gates refused carries a
///   measured `d` just like a confirmed one (`Status::Uncertain { d }`), so a control that took an
///   `i32` could not tell them apart and would learn from noise. §13.5's "slow, and only on
///   confirmed steps" is therefore a property of the signature.
/// * **The bound is on the step, not on the count.** `N_MAX` is a correctness constraint (§13.2): a
///   step that moves more than half a viewport cannot be verified. It is computed from `ĝ` and the
///   viewport, so it moves as `ĝ` does — a fixed notch count would be a fixed *displacement* only
///   for a target that behaves.
pub(crate) struct Control {
    prior: Prior,
    target_advance_rows: f32,
    viewport_extent: u32,
}

impl Control {
    /// `px_per_notch` is a **starting value** — see [`starting_px_per_notch`].
    ///
    /// **Why the numerator is the advance and not the overlap** (`docs/30` §3.6 vs §13.2, erratum
    /// recorded in §13.2.1): §3.6's formula is `n = clamp(round((1 − ρ*)·V / ĝ), 1, N_max)`, while
    /// §13.2 writes the same law as `target_overlap_rows = ρ* × viewport_extent` in the numerator.
    /// Those differ by whether the *result* is the overlap or the advance, and `E-CTRL-1`'s own
    /// criterion (`docs/30:363`: the overlap ratio must land in `[0.30, 0.40]`) settles it: aiming
    /// at `ρ*·V` of *advance* leaves an overlap of `V − ρ*·V`, i.e. 0.65 of the viewport, which is
    /// nowhere near the band. The measured failure was 0.689 against a 0.30–0.40 requirement.
    pub(crate) fn new(viewport_extent: u32, px_per_notch: f32) -> Self {
        Self {
            prior: Prior::new(px_per_notch),
            target_advance_rows: (1.0 - RHO_MIN) * viewport_extent as f32,
            viewport_extent,
        }
    }

    /// The current estimate of pixels per notch — the number the P1 prior is built from (§16.6).
    pub(crate) fn px_per_notch(&self) -> f32 {
        self.prior.px_per_notch()
    }

    /// The prior itself, so the estimator's search window and this control's learning are built from
    /// the **same** `ĝ` (§16.6, §15.6).
    ///
    /// Handing out the `Prior` rather than a copy of `ĝ` is the point: `docs/30` §13.2 derives the
    /// search half-width from the prior that the loop is currently learning (`max(4, ceil(0.3·n·ĝ))`),
    /// and a caller that had to reconstruct a `Prior` from `px_per_notch()` would silently lose
    /// `PRIOR_KAPPA` — the two numbers would agree today and drift the first time either changes.
    pub(crate) fn prior(&self) -> &Prior {
        &self.prior
    }

    /// How many notches to inject for the next step.
    pub(crate) fn notches(&self) -> i32 {
        let ghat = self.prior.px_per_notch();
        if !(ghat > 0.0) {
            // ĝ is allowed to be negative (§12.3) and can be zero before the first observation.
            // One notch is the conservative probe §13.2 starts every session with, and it is the
            // only answer that is meaningful without a usable gain.
            return 1;
        }

        let wanted = (self.target_advance_rows / ghat).round();
        let verifiable = ((1.0 - RHO_MIN) * self.viewport_extent as f32 / ghat).floor();
        let hard = verifiable.clamp(1.0, MAX_NOTCHES_PER_STEP as f32);
        wanted.clamp(1.0, hard) as i32
    }

    /// Feeds one step's outcome back. Returns whether it taught the control anything.
    ///
    /// `docs/30` §13.5: only `Confirmed` steps update `ĝ`; an `Uncertain` step was measured but not
    /// decided, and `None` was not even measured. `notches == 0` teaches nothing either — there is
    /// no ratio to take, and inventing one would be the same mistake in a different place.
    pub(crate) fn observe(&mut self, notches: i32, status: Status) -> bool {
        match status {
            Status::Confirmed { d } => self.prior.confirm(notches, d),
            Status::Uncertain { .. } | Status::None => false,
        }
    }
}

// --- §28.2's `ScrollDriver`: the closed loop (task P3.09) ---

/// How many rows apart the stillness digest samples (`docs/30` §13.3).
///
/// The digest answers one question — "is the target still producing different pictures?" — and it is
/// asked on every read of a step, up to `STEP_TIMEOUT / RENDER_TICK_MS` times. Hashing a 4K frame
/// whole would cost more than the tick it is measured against and would buy nothing: a scroll moves
/// **every** row, so a stride that samples one row in eight cannot miss it. What the stride can miss
/// is a change confined to fewer than eight consecutive rows, and that is deliberate — the question
/// is whether the page is still moving, not whether one glyph repainted. The estimator, not this
/// number, decides what gets committed.
const DIGEST_ROW_STRIDE: usize = 8;

/// A cheap fingerprint of a frame, used only to tell two consecutive reads apart (§13.3).
///
/// Folds the sampled rows together rather than hashing one buffer, so the sampling costs a stride
/// walk and not an allocation. `line_digest` is the same FNV the frame source already dedupes rows
/// with, so "these pixels differ" means the same thing here as it does there.
fn stillness_digest(pixels: &[u8], row_stride: usize) -> u64 {
    let stride = row_stride.max(1);
    let mut hash = 0xcbf2_9ce4_8422_2325_u64; // FNV-1a's offset basis, re-mixed once per sampled row.
    for row in pixels.chunks(stride).step_by(DIGEST_ROW_STRIDE) {
        hash = hash.rotate_left(7) ^ line_digest(row);
    }
    hash
}

/// `EndReason` → `StopReason` (`docs/30` §20.4). Total, and deliberately not a `From` impl: the two
/// enums answer different questions — one is "why did the stream end", the other is "why did the
/// session stop" — and a `From` would invite a conversion in the other direction that has no meaning.
fn stop_reason_for(reason: EndReason) -> StopReason {
    match reason {
        EndReason::TargetLost => StopReason::TargetLost,
        EndReason::CaptureFailed => StopReason::CaptureFailed,
        EndReason::DeviceLost => StopReason::DeviceLost,
        EndReason::Timeout => StopReason::Timeout,
    }
}

/// Joins a [`FrameSource`] and a [`ScrollActuator`] into the single narrow thing [`inject_and_settle`]
/// asks for (§13.3, §28.4).
///
/// It exists because [`StepHost`] is deliberately *narrower* than either port: the loop needs "inject
/// these notches", "what do the pixels look like now", "what time is it" and "has the user
/// cancelled", and nothing else. Joining the two ports here — on the `scroll/` side of the boundary —
/// is what keeps the driver from naming a handle, a message or a transport, for the same reason
/// [`ActuatorWatch`] is generic over its path.
struct Host<'a, F: FrameSource, A: ScrollActuator> {
    source: &'a mut F,
    actuator: &'a mut A,
    controller: &'a ScrollController,
    /// One render tick per read: a longer wait would make the cancel check late by exactly that much
    /// (§21.4).
    tick: Duration,
    /// The frame the last read produced. This is what the driver estimates against, so it is moved
    /// out by [`Self::take_frame`] rather than copied.
    frame: Option<Observation>,
    /// The digest of `frame`, so that an `Idle` read can answer "the same picture as last time"
    /// instead of "no evidence". The settle rule needs two *agreeing* reads, and a read that produces
    /// no new observation is exactly how a settled target announces itself (§11.1).
    digest: Option<u64>,
    /// Set when the frame source says the target is gone (§20.4's four terminal reasons).
    ended: Option<EndReason>,
}

impl<'a, F: FrameSource, A: ScrollActuator> Host<'a, F, A> {
    fn new(source: &'a mut F, actuator: &'a mut A, controller: &'a ScrollController) -> Self {
        Self {
            source,
            actuator,
            controller,
            tick: Duration::from_millis(RENDER_TICK_MS as u64),
            frame: None,
            digest: None,
            ended: None,
        }
    }

    /// The frame the step produced, if it produced one.
    fn take_frame(&mut self) -> Option<Observation> {
        self.frame.take()
    }

    /// Why the stream ended, if it did.
    fn ended(&self) -> Option<EndReason> {
        self.ended
    }
}

impl<F: FrameSource, A: ScrollActuator> StepHost for Host<'_, F, A> {
    fn inject(&mut self, notches: i32) -> Result<(), String> {
        let outcome = self.actuator.inject(notches);
        match outcome.status {
            // §24.6.3: `Posted` is weak evidence — the message was *queued*. It is still the only
            // answer the port can give, and proving the content actually moved is the watchdog's job.
            InjectStatus::Posted => Ok(()),
            failed => Err(format!("{failed:?}")),
        }
    }

    fn digest(&mut self) -> Option<u64> {
        match self.source.next(self.tick) {
            Ok(Poll::Frame(observation)) => {
                let stride = observation.view().row_stride();
                self.digest = Some(stillness_digest(observation.pixels(), stride));
                self.frame = Some(observation);
                self.digest
            }
            // §11.1's distinction, and it is load-bearing here: `Idle` means "no new observation",
            // which is evidence that the picture is the one we already have. Answering `None` would
            // make a page that has stopped moving look like a stream that never started.
            Ok(Poll::Idle) => self.digest,
            Ok(Poll::Ended(reason)) => {
                self.ended = Some(reason);
                None
            }
            // A read that failed is evidence of nothing, least of all stillness.
            Err(_) => None,
        }
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    fn cancellation(&self) -> Option<Instant> {
        self.controller.cancellation()
    }
}

/// One step's outcome for the watchdog: did the content move?
///
/// `Unmoved` carries the path the actuator actually used, not the one the driver thinks it used —
/// that payload is what lets a step injected through a stale transport count against *that*
/// transport (§24.7).
fn note<A: ScrollActuator>(
    watch: &mut ActuatorWatch<A::Path>,
    actuator: &mut A,
    moved: bool,
) -> WatchVerdict<A::Path> {
    let outcome = if moved {
        StepOutcome::Moved
    } else {
        StepOutcome::Unmoved(actuator.path())
    };
    watch.observe(outcome, |from| actuator.switch(from))
}

/// §13.2's diagram as a type: inject, settle, estimate, commit, preview.
///
/// Every piece of this was built and measured on its own — the four gates (`P1.08`–`P1.11`), the
/// canvas (`P1.17`–`P1.19`), the control law (`P3.05`), the watchdog (`P3.03`), the ports (`P3.08`) —
/// and this is where they become a session. It is the only place in `scroll/` that knows the order
/// they are used in, which is also why it is the only place that can be wrong about it.
///
/// It owns **no thread**: `ScrollRuntime` spawns [`ScrollDriver::run`] and keeps the two ports. That
/// split is what makes the loop testable without a desktop (§29.2) — `run` takes its frame source and
/// its actuator as arguments, so a test hands it a scripted pair exactly like `P1.01`'s.
pub(crate) struct ScrollDriver {
    session: ScrollSession,
    viewport: ViewportState,
    control: Control,
    scratch: Scratch,
    scene_cut: SceneCut,
    settle: Settle,
    /// The `qpc` of the frame the canvas last accepted, used to stamp the reference viewport (§17.4).
    last_qpc: i64,
}

impl ScrollDriver {
    pub(crate) fn new(plan: &ScrollPlan) -> Self {
        let extent = plan.viewport_extent();
        Self {
            session: ScrollSession::new(plan.axis(), plan.cross_len(), plan.budget()),
            viewport: ViewportState::new(extent),
            control: Control::new(
                extent,
                starting_px_per_notch(plan.lines_per_notch(), plan.line_height_px()),
            ),
            scratch: Scratch::new(),
            scene_cut: SceneCut::none(),
            settle: Settle::new(),
            last_qpc: 0,
        }
    }

    /// Runs until the session stops, then hands the session back **by value**.
    ///
    /// By value rather than through `&mut self` because the session's final state *is* the result:
    /// `ScrollRuntime::teardown` returns it and the caller reads `stop_reason`, `disposal` and
    /// `cancel_latency` off it. A loop that borrowed itself could not outlive its own thread.
    pub(crate) fn run<F, A>(
        mut self,
        mut source: F,
        mut actuator: A,
        controller: &ScrollController,
        preview: &PreviewStream,
    ) -> ScrollSession
    where
        F: FrameSource,
        A: ScrollActuator,
    {
        let mut watch = ActuatorWatch::new(actuator.path());

        // §17.2: the first frame is the canvas' origin. Until one arrives there is nothing to estimate
        // against, and the session's `Phase` already says so — `phase()` derives `Preparing` from
        // `primary_len() == 0`, so there is no second copy of "have we started" to keep in step.
        let first = match self.await_first_frame(&mut source, controller) {
            Ok(frame) => frame,
            Err(reason) => return self.finish(reason, preview),
        };
        self.last_qpc = first.qpc();
        self.session.start(&first);
        self.publish_progress(preview, Status::None);

        loop {
            if let Some(reason) = self.command(controller) {
                return self.finish(reason, preview);
            }
            self.apply_commands(controller, preview);

            let notches = step_notches(&self.session, &self.control);
            let mut host = Host::new(&mut source, &mut actuator, controller);
            let settled = inject_and_settle(&mut host, &mut self.settle, notches);
            let frame = host.take_frame();
            let ended = host.ended();

            if let Some(reason) = ended {
                return self.finish(stop_reason_for(reason), preview);
            }

            let frame = match settled {
                Ok(_) => match frame {
                    Some(frame) => frame,
                    // Unreachable: `inject_and_settle` returns `Ok` only after a frame arrived. Kept
                    // as a step that produced nothing rather than a guess — the alternative is
                    // estimating against pixels the loop never saw.
                    None => {
                        self.session.record_discarded();
                        continue;
                    }
                },
                Err(StepError::Cancelled { latency }) => {
                    self.session.record_cancel_latency(latency);
                    return self.finish(StopReason::UserCancelled, preview);
                }
                Err(StepError::Injection(_)) | Err(StepError::NoFrame) => {
                    // §20.4 and C2: a single failed step never stops the session. What turns a *run*
                    // of them into a transport switch — or into `ActuatorFailed` — is the watchdog.
                    //
                    // `NoFrame` is also how "the content stopped changing" arrives today: the frame
                    // source only produces an observation when the pixels changed, so a page at its
                    // end looks exactly like a dead actuator until something else says otherwise.
                    // Classifying it as `EndReached` needs the scroll-position channel (§20.4's
                    // convergence row); it is listed as open in this module's header.
                    self.session.record_discarded();
                    if let WatchVerdict::Failed = note(&mut watch, &mut actuator, false) {
                        return self.finish(StopReason::ActuatorFailed, preview);
                    }
                    continue;
                }
            };

            self.session.record_step();
            let displacement = self.estimate_step(&frame, notches);
            let status = *displacement.status();
            learn(&self.session, &mut self.control, notches, status);

            // §16.10's canvas column: `effect()` is the one place that says whether these pixels are
            // committed, continued past, or skipped.
            let committed = match displacement.effect() {
                StepEffect::Commit => match status {
                    Status::Confirmed { d } => self.commit(&frame, d, preview),
                    _ => false,
                },
                StepEffect::Continue | StepEffect::Skip => false,
            };
            if committed {
                self.session.record_committed();
                self.last_qpc = frame.qpc();
            } else {
                self.session.record_discarded();
            }

            let moved = committed && matches!(status, Status::Confirmed { d } if d != 0);            if let WatchVerdict::Failed = note(&mut watch, &mut actuator, moved) {
                return self.finish(StopReason::ActuatorFailed, preview);
            }
            self.publish_progress(preview, status);
        }
    }

    /// Wait for the frame that creates the canvas (§17.2).
    ///
    /// The two ways out are the two things that can happen before the session has any pixels: the
    /// user ends it, or the target goes away. `Poll::Idle` is not a third — it means "not yet".
    fn await_first_frame<F: FrameSource>(
        &self,
        source: &mut F,
        controller: &ScrollController,
    ) -> Result<Observation, StopReason> {
        loop {
            if let Some(reason) = self.command(controller) {
                return Err(reason);
            }
            match source.next(Duration::from_millis(RENDER_TICK_MS as u64)) {
                Ok(Poll::Frame(observation)) => return Ok(observation),
                Ok(Poll::Idle) => continue,
                Ok(Poll::Ended(reason)) => return Err(stop_reason_for(reason)),
                // §20.4: a transient capture failure before the first frame is not a stopped session
                // — the next read is the one that decides.
                Err(_) => continue,
            }
        }
    }

    /// What the user asked for, if anything (`§20.5`, §27.2).
    ///
    /// Teardown maps to `UserCancelled` rather than to a twelfth reason: `docs/30` §4.3.7 froze
    /// `StopReason` at eleven variants, and the promise teardown makes — "no artifact" — is exactly
    /// the promise `UserCancelled` makes (§20.5). Inventing a variant here would give `Disposal` a
    /// third answer that nothing has a rule for.
    fn command(&self, controller: &ScrollController) -> Option<StopReason> {
        if let Some(reason) = controller.requested_stop() {
            return Some(reason);
        }
        if controller.is_shutdown() {
            return Some(StopReason::UserCancelled);
        }
        None
    }

    /// Apply the commands that are not a stop: undo presses and the follow flag (§19.5, §19.6).
    ///
    /// Reading *is* taking (`take_undo_requests`, `take_follow`), so this must run once per step and
    /// must not be skipped by an early `continue`: a command consumed and then dropped on the floor is
    /// a button that does nothing.
    fn apply_commands(&mut self, controller: &ScrollController, preview: &PreviewStream) {
        if let Some(follow) = controller.take_follow() {
            self.session.set_follow(follow);
        }

        let undos = controller.take_undo_requests();
        for _ in 0..undos {
            match self.viewport.undo_last(self.session.canvas_mut()) {
                // §19.6 lists what an undo restores; the step counter is not on that list. Undoing is
                // "take those pixels back", not "pretend the step never happened": the screen has
                // already moved, and a session that rewound its own counters would report progress it
                // no longer has.
                Ok(true) => {}
                Ok(false) => break,
                Err(_) => break,
            }
        }
        if undos > 0 {
            self.publish_progress(preview, Status::None);
        }
    }

    /// One step's estimate, in the order §17.4 requires: build the reference from the **canvas**,
    /// then compare it with the frame the step produced.
    ///
    /// The reference is the canvas and not the previous frame (`N3`). A frame the gates refused still
    /// shows what the screen showed; estimating the next step against it would measure that step
    /// against a picture the canvas never accepted, and the same pixels would be weighed twice.
    fn estimate_step(&mut self, frame: &Observation, notches: i32) -> Displacement {
        let qpc = self.last_qpc;
        let reference = match self.viewport.reference(self.session.canvas_mut(), qpc) {
            Ok(reference) => reference,
            // A reference that cannot be read is a canvas that cannot be read: no estimate, and the
            // step is discarded rather than guessed (`BandError` is the store's, §17.5).
            Err(_) => return Displacement::none(displacement_evidence(self.scene_cut)),
        };

        // §16.6: manual mode passes no prior at all — that is the whole of its difference in this
        // call. It is not "a prior of zero": with no prior the search window is the manual window
        // (§16.6.2) and no expectation ordering is applied, which is what makes the periodic-page
        // answer `Uncertain` instead of confident.
        let prior = if self.session.manual() {
            None
        } else {
            Some(self.control.prior())
        };
        let displacement = estimate(
            &reference.view(),
            &frame.view(),
            prior,
            notches,
            self.scene_cut,
            &mut self.scratch,
        );
        self.scene_cut = displacement.evidence().scene_cut();
        displacement
    }

    /// Write one confirmed step through the canvas (§17.3).
    ///
    /// `false` means the write was refused — `MemoryLimit`, or a spill that failed. The session does
    /// **not** stop: §20.4's `MemoryLimit` row keeps the canvas trimmed to a contiguous prefix, and a
    /// session that ended on the first refusal would throw away a canvas that is still readable.
    fn commit(&mut self, frame: &Observation, d: i32, preview: &PreviewStream) -> bool {
        match self.viewport.apply(self.session.canvas_mut(), frame, d) {
            Ok(write) => {
                match write {
                    StepWrite::Appended { first_row, rows } => {
                        preview.publish(PreviewUpdate::Bands {
                            first_row,
                            rows: rows.min(u32::MAX as u64) as u32,
                            scale: 1,
                        });
                    }
                    // A prepend moves every existing row down, so *all* of the canvas is newly
                    // readable at a new offset — the honest update is the whole span, not the rows
                    // that arrived.
                    StepWrite::Prepended { .. } => preview.publish(PreviewUpdate::Bands {
                        first_row: 0,
                        rows: self.session.canvas().primary_len().min(u32::MAX as u64) as u32,
                        scale: 1,
                    }),
                    StepWrite::Skipped | StepWrite::Contained => {}
                }
                true
            }
            Err(_) => false,
        }
    }

    /// Tell the preview where the box is and how far the session has come (§19.3, §19.4, §19.5).
    fn publish_progress(&self, preview: &PreviewStream, status: Status) {
        preview.publish(PreviewUpdate::Span {
            primary_len: self.session.canvas().primary_len(),
            steps: self.session.step(),
            discarded: self.session.discarded(),
        });
        preview.publish(PreviewUpdate::Viewport {
            band: self.viewport.position().max(0) as u64,
            status,
        });
    }

    /// Record the stop reason, tell the preview, and hand the session over.
    fn finish(mut self, reason: StopReason, preview: &PreviewStream) -> ScrollSession {
        self.session.stop(reason);
        preview.publish(PreviewUpdate::Ended { reason });
        self.session
    }
}

/// All-zero evidence, for the one path that cannot produce any (a reference that cannot be read).
fn displacement_evidence(scene_cut: SceneCut) -> crate::scroll::displacement::Evidence {
    crate::scroll::displacement::Evidence {
        zncc2d: 0.0,
        gain: 0.0,
        margin: 0.0,
        tiles: 0,
        scene_cut,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Point, Rect};
    use crate::scroll::canvas::MemoryBudget;
    use crate::scroll::observation::Axis;
    use crate::scroll::ports::{FrameError, InjectOutcome, InjectPath, Poll};
    use crate::scroll::session::ScrollSession;

    /// A stand-in for `InjectPath` that lives entirely on this side of the boundary.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Path {
        First,
        Second,
    }

    #[test]
    fn three_posted_steps_without_a_confirmed_step_switch_the_path() {
        let mut watch = ActuatorWatch::new(Path::First);
        let next = |_: Path| Some(Path::Second);

        assert_eq!(
            watch.observe(StepOutcome::Unmoved(Path::First), next),
            WatchVerdict::Continue,
            "one ineffective step is not enough to move the transport"
        );
        assert_eq!(
            watch.observe(StepOutcome::Unmoved(Path::First), next),
            WatchVerdict::Continue,
            "two ineffective steps are not enough either"
        );
        assert_eq!(
            watch.observe(StepOutcome::Unmoved(Path::First), next),
            WatchVerdict::Switched {
                from: Path::First,
                to: Path::Second,
            },
            "three consecutive ineffective steps through one transport must switch it"
        );
        assert_eq!(watch.current(), Path::Second);
        assert_eq!(watch.streak(), 0, "a switch restarts the count");
    }

    #[test]
    fn both_paths_failing_three_times_ends_the_session() {
        let mut watch = ActuatorWatch::new(Path::First);
        let next = |current: Path| match current {
            Path::First => Some(Path::Second),
            Path::Second => None,
        };

        for _ in 0..3 {
            watch.observe(StepOutcome::Unmoved(Path::First), next);
        }
        assert_eq!(watch.current(), Path::Second);

        assert_eq!(
            watch.observe(StepOutcome::Unmoved(Path::Second), next),
            WatchVerdict::Continue
        );
        assert_eq!(
            watch.observe(StepOutcome::Unmoved(Path::Second), next),
            WatchVerdict::Continue
        );
        assert_eq!(
            watch.observe(StepOutcome::Unmoved(Path::Second), next),
            WatchVerdict::Failed,
            "the second transport also failed three times: there is no third one"
        );
    }

    #[test]
    fn a_moved_step_resets_the_streak() {
        let mut watch = ActuatorWatch::new(Path::First);
        let next = |_: Path| Some(Path::Second);

        watch.observe(StepOutcome::Unmoved(Path::First), next);
        watch.observe(StepOutcome::Unmoved(Path::First), next);
        assert_eq!(watch.streak(), 2);

        assert_eq!(
            watch.observe(StepOutcome::Moved, next),
            WatchVerdict::Continue
        );
        assert_eq!(watch.streak(), 0, "the count is consecutive, not cumulative");

        watch.observe(StepOutcome::Unmoved(Path::First), next);
        assert_eq!(
            watch.observe(StepOutcome::Unmoved(Path::First), next),
            WatchVerdict::Continue,
            "two steps after a success must not switch"
        );
    }

    #[test]
    fn the_watchdog_does_not_assume_there_are_exactly_two_transports() {
        let mut watch = ActuatorWatch::new(Path::First);
        let none = |_: Path| None;

        for _ in 0..2 {
            watch.observe(StepOutcome::Unmoved(Path::First), none);
        }
        assert_eq!(
            watch.observe(StepOutcome::Unmoved(Path::First), none),
            WatchVerdict::Failed,
            "if the caller has no other transport, the first failure is the last one"
        );
    }

    #[test]
    fn the_switch_threshold_is_a_named_starting_value() {
        assert_eq!(POSTED_STEPS_BEFORE_SWITCH, 3);
    }

    #[test]
    fn a_path_that_is_not_the_current_one_is_reported_as_no_switch() {
        let mut watch = ActuatorWatch::new(Path::First);
        // A caller that offers the transport we are already using has not answered the question.
        let same = |_: Path| Some(Path::First);

        for _ in 0..2 {
            watch.observe(StepOutcome::Unmoved(Path::First), same);
        }
        assert_eq!(
            watch.observe(StepOutcome::Unmoved(Path::First), same),
            WatchVerdict::Failed
        );
    }

    #[test]
    fn the_loop_waits_until_two_consecutive_frames_agree_before_estimating() {
        let start = Instant::now();
        let mut settle = Settle::new();
        settle.begin(start);

        assert_eq!(
            settle.sample(0x1111, start),
            SettleVerdict::Waiting,
            "one sample is not evidence of stillness"
        );
        assert_eq!(
            settle.sample(0x1111, start + Duration::from_millis(10)),
            SettleVerdict::Waiting,
            "two agreeing samples less than STILL_WINDOW apart could both sit inside one \
             compositor frame, so they are not evidence either"
        );
        assert_eq!(
            settle.sample(0x1111, start + Duration::from_millis(45)),
            SettleVerdict::Still,
            "two agreeing samples at least STILL_WINDOW apart are the empirical still frame"
        );
    }

    #[test]
    fn smooth_scrolling_is_waited_out_instead_of_being_estimated() {
        let start = Instant::now();
        let mut settle = Settle::new();
        settle.begin(start);

        // Four frames of a smooth scroll animation, each a different picture. The loop must not
        // estimate on any of them: a mid-animation frame produces a mid-animation displacement,
        // and committing that would poison the canvas with an intermediate state (§13.3).
        let mut now = start;
        for (index, digest) in [0x21, 0x22, 0x23, 0x24].into_iter().enumerate() {
            let verdict = settle.sample(digest, now);
            assert_eq!(
                verdict,
                SettleVerdict::Waiting,
                "frame {index} was treated as still while the page was still moving"
            );
            now += Duration::from_millis(45);
        }

        assert_eq!(
            settle.sample(0x25, start + STEP_TIMEOUT),
            SettleVerdict::TimedOut,
            "an animation that never settles must be cut off by STEP_TIMEOUT, not waited on forever"
        );
    }

    #[test]
    fn a_picture_that_settles_after_moving_is_reported_still() {
        let start = Instant::now();
        let mut settle = Settle::new();
        settle.begin(start);

        settle.sample(0x31, start);
        assert_eq!(
            settle.sample(0x32, start + Duration::from_millis(45)),
            SettleVerdict::Waiting,
            "the picture moved, so the comparison restarts from the new sample"
        );
        assert_eq!(
            settle.sample(0x32, start + Duration::from_millis(50)),
            SettleVerdict::Waiting,
            "the new comparison has not yet spanned STILL_WINDOW"
        );
        assert_eq!(
            settle.sample(0x32, start + Duration::from_millis(95)),
            SettleVerdict::Still,
            "a scroll that came to rest must be recognised as still, not timed out"
        );
    }

    #[test]
    fn the_settle_constants_are_the_ones_the_design_measured() {
        assert_eq!(STILL_WINDOW, Duration::from_millis(40));
        assert_eq!(STEP_TIMEOUT, Duration::from_millis(400));
        assert!(
            STILL_WINDOW.as_millis() as u64 > 2 * RENDER_TICK_MS as u64,
            "§13.3: STILL_WINDOW must span more than one compositor frame \
             (RENDER_TICK_MS = {RENDER_TICK_MS})"
        );
    }

    /// A host whose clock is a script and whose frames are a list. It never sleeps, which is what
    /// makes the two timing rules testable in microseconds.
    struct ScriptedHost {
        clock: Instant,
        started: Instant,
        step_ms: u64,
        digests: std::collections::VecDeque<Option<u64>>,
        inject_error: Option<String>,
        injections: Vec<i32>,
        /// When the user pressed `Esc`, in milliseconds after the host was created.
        cancel_after_ms: Option<u64>,
        /// How long the transport takes, in milliseconds. A **model input**: the real cost of
        /// `SendInput`/`PostMessageW` is measured at `L3` (`E-INJECT-1`). It exists so that the
        /// cancel-latency table can show that time the loop was not allowed to look at is *in* the
        /// measurement rather than argued away.
        inject_ms: u64,
    }

    impl ScriptedHost {
        fn new(step_ms: u64, digests: impl IntoIterator<Item = Option<u64>>) -> Self {
            // One origin for both clocks: `cancel_after_ms` is measured from `started`, and a
            // second `Instant::now()` here would put the "cancel at 0 ms" scenario a few
            // nanoseconds in the future — which is exactly the difference between "the loop must
            // not inject" and "the loop injected once more".
            let now = Instant::now();
            Self {
                clock: now,
                started: now,
                step_ms,
                digests: digests.into_iter().collect(),
                inject_error: None,
                injections: Vec::new(),
                cancel_after_ms: None,
                inject_ms: 0,
            }
        }
    }

    impl StepHost for ScriptedHost {
        fn inject(&mut self, notches: i32) -> Result<(), String> {
            self.injections.push(notches);
            // The transport's cost is uninterruptible: the clock moves while the loop cannot look.
            self.clock += Duration::from_millis(self.inject_ms);
            match &self.inject_error {
                Some(detail) => Err(detail.clone()),
                None => Ok(()),
            }
        }

        fn digest(&mut self) -> Option<u64> {
            let next = self.digests.pop_front().unwrap_or(None);
            self.clock += Duration::from_millis(self.step_ms);
            next
        }

        fn now(&self) -> Instant {
            self.clock
        }

        fn cancellation(&self) -> Option<Instant> {
            let after = self.cancel_after_ms?;
            let at = self.started + Duration::from_millis(after);
            (self.clock >= at).then_some(at)
        }
    }

    #[test]
    fn the_loop_injects_once_and_waits_for_the_picture_to_stop_moving() {
        // Two agreeing frames 45 ms apart, reached in 25 ms poll steps.
        let mut host = ScriptedHost::new(25, [Some(0xAA), Some(0xAA), Some(0xAA)]);
        let mut settle = Settle::new();

        let settled = inject_and_settle(&mut host, &mut settle, 3).expect("the wait must complete");

        assert_eq!(settled.verdict, SettleVerdict::Still);
        assert_eq!(settled.digest, 0xAA);
        assert_eq!(
            host.injections,
            vec![3],
            "the wait must not inject a second time: §13.2's control law needs to know how many \
             notches went out"
        );
    }

    #[test]
    fn a_never_settling_animation_is_cut_off_by_the_step_timeout() {
        // A different picture every 45 ms, forever.
        let mut host = ScriptedHost::new(45, (0..40).map(|i| Some(0x100 + i)));
        let mut settle = Settle::new();

        let settled = inject_and_settle(&mut host, &mut settle, 1).expect("a timeout is an answer");

        assert_eq!(
            settled.verdict,
            SettleVerdict::TimedOut,
            "an animation that never settles must be cut off, not waited on forever"
        );
        assert_eq!(host.injections.len(), 1);
    }

    #[test]
    fn a_step_with_no_frames_at_all_is_not_a_timeout() {
        // `Poll::Idle` forever, with the clock advancing.
        let mut host = ScriptedHost::new(100, (0..20).map(|_| None));
        let mut settle = Settle::new();

        assert_eq!(
            inject_and_settle(&mut host, &mut settle, 1),
            Err(StepError::NoFrame),
            "`TimedOut` means 'we have a frame but cannot prove it is still'; no frame at all is a \
             different answer and must not be reported as the first"
        );
    }

    #[test]
    fn an_undeliverable_step_is_reported_rather_than_retried_here() {
        let mut host = ScriptedHost::new(25, [Some(0xAA)]);
        host.inject_error = Some("the transport refused".into());
        let mut settle = Settle::new();

        assert_eq!(
            inject_and_settle(&mut host, &mut settle, 1),
            Err(StepError::Injection("the transport refused".into()))
        );
        assert_eq!(
            host.injections.len(),
            1,
            "retrying is the watchdog's job (P3.03), not the wait's"
        );
    }

    /// The `Cancel latency` metric of `docs/30` §23.2, **measured** rather than claimed.
    ///
    /// §23.3 gives the design target `Max ≤ 400 ms` and the acceptance bound `Max ≤ 500 ms`, plus
    /// `P50 ≤ 60 ms`, and it insists the max is given separately "because it is the only source of
    /// the user's feeling that the thing has frozen". So the number cannot be derived on paper: the
    /// loop has to be asked, at the worst moment, how long it took to notice.
    ///
    /// | `Esc` pressed at | injection in flight | loop noticed at | latency | what the loop was doing |
    /// |---|---|---|---|---|
    /// | 0 ms | — | 0 ms | 0 ms | nothing yet: the step had not been injected |
    /// | 1 ms | 0 ms | 15 ms | 14 ms | waiting for the picture to settle |
    /// | 7 ms | 0 ms | 15 ms | 8 ms | waiting for the picture to settle |
    /// | 15 ms | 0 ms | 15 ms | 0 ms | it was already looking |
    /// | 16 ms | 0 ms | 30 ms | 14 ms | waiting for the picture to settle |
    /// | 100 ms | 0 ms | 105 ms | 5 ms | waiting for a frame that never came |
    /// | 10 ms | 30 ms | 30 ms | 20 ms | **inside** the injection, which cannot be recalled |
    ///
    /// The last row is the floor made mechanical: the injection had already gone out, so the loop
    /// could not look until it came back, and the 20 ms it was blind for is *in* the measurement
    /// rather than argued away. Its 30 ms is a **model input**, not a desktop number — what the
    /// transport actually costs is measured at `L3` (`E-INJECT-1`, and `E-PERF-1` for the readback
    /// that is the other uninterruptible segment). The rows above it are the part the loop itself
    /// controls: **one [`RENDER_TICK_MS`] tick**, which is what §21.4's "check at every interruptible
    /// point" buys.
    ///
    /// **Do not "optimize" the floor to zero.** Neither `SendInput` nor `PostMessageW` can be
    /// recalled, and a frame readback that is already in flight has to return; a loop cannot see a
    /// cancel at a point where it is not allowed to look.
    #[test]
    fn cancel_latency_has_a_measured_max() {
        let scenarios = [
            (0u64, 0u64, "pressed before the step began"),
            (1, 0, "pressed just after the injection"),
            (7, 0, "pressed mid-tick"),
            (15, 0, "pressed on a tick boundary"),
            (16, 0, "pressed just after a tick"),
            (100, 0, "pressed while waiting for a frame that never comes"),
            (10, 30, "pressed while the injection was in flight"),
        ];

        let mut latencies = Vec::new();
        for (after_ms, inject_ms, what) in scenarios {
            let digests: Vec<Option<u64>> = if after_ms >= 100 {
                // `Poll::Idle` forever: the loop is in the no-frame branch, where it must report a
                // cancel rather than the `NoFrame` timeout it is racing against.
                (0..20).map(|_| None).collect()
            } else {
                (0..20).map(|_| Some(0xAA)).collect()
            };
            let mut host = ScriptedHost::new(RENDER_TICK_MS as u64, digests);
            host.cancel_after_ms = Some(after_ms);
            host.inject_ms = inject_ms;
            let mut settle = Settle::new();

            let outcome = inject_and_settle(&mut host, &mut settle, 3);

            let Err(StepError::Cancelled { latency }) = outcome else {
                panic!("`Esc` {what} must be reported as a cancel, got {outcome:?}");
            };
            if after_ms == 0 {
                assert!(
                    host.injections.is_empty(),
                    "a cancel that arrived before the step began must not be answered with an \
                     injection: the promise was 'no more scrolling'"
                );
            }
            if inject_ms > 0 {
                assert_eq!(
                    host.injections.len(),
                    1,
                    "an injection already in flight cannot be recalled — that is the floor of this \
                     metric, not a bug to fix"
                );
            }
            // The loop's own contribution is one tick; the rest is time it was not allowed to look.
            assert!(
                latency <= Duration::from_millis(inject_ms + RENDER_TICK_MS as u64),
                "`Esc` {what}: {latency:?} exceeds one tick plus the uninterruptible injection \
                 ({inject_ms} ms)"
            );
            latencies.push(latency);
        }

        let max = *latencies.iter().max().expect("seven samples");
        let mut sorted = latencies.clone();
        sorted.sort();
        let p50 = sorted[sorted.len() / 2];

        // The table in the doc comment, as an assertion: a measurement that is only written down
        // drifts away from the code, and this one is the whole point of the test.
        assert_eq!(
            latencies,
            [0, 14, 8, 0, 14, 5, 20].map(Duration::from_millis).to_vec(),
            "the measured table changed — update the doc comment with it, do not relax this"
        );
        assert!(
            max <= Duration::from_millis(400),
            "§23.3's design target: max {max:?} over {latencies:?}"
        );
        assert!(
            max <= Duration::from_millis(500),
            "§23.3's acceptance bound: max {max:?} over {latencies:?}"
        );
        assert!(p50 <= Duration::from_millis(60), "§23.3: P50 {p50:?}");
    }

    #[test]
    fn an_idle_frame_is_evidence_of_stillness_not_a_reason_to_give_up() {
        // The first read has no new frame (the picture is already still), the second one agrees.
        let mut host = ScriptedHost::new(30, [None, Some(0xBB), Some(0xBB), Some(0xBB)]);
        let mut settle = Settle::new();

        let settled = inject_and_settle(&mut host, &mut settle, 1).expect("the wait must complete");

        assert_eq!(
            settled.verdict,
            SettleVerdict::Still,
            "§11.1: `Idle` means 'nothing changed', which is the very thing stillness is made of"
        );
    }

    // --- §13.2's control law and `E-CTRL-1` (task P3.05) ---

    const CONTROL_VIEWPORT: u32 = 900;

    /// Drives `Control` against a page whose true gain never changes, and returns the `ĝ` curve.
    ///
    /// This is `E-CTRL-1`'s synthetic target: it is the one configuration where "did the loop
    /// learn?" has a single right answer, so every assertion below is arithmetic on this curve.
    fn run_control(true_gain: f32, start_gain: f32, steps: u32) -> Vec<f32> {
        let mut control = Control::new(CONTROL_VIEWPORT, start_gain);
        let mut curve = vec![control.px_per_notch()];
        for _ in 0..steps {
            let notches = control.notches();
            let observed = (notches as f32 * true_gain).round() as i32;
            assert!(
                control.observe(notches, Status::Confirmed { d: observed }),
                "a confirmed step must teach the control something (n = {notches})"
            );
            curve.push(control.px_per_notch());
        }
        curve
    }

    fn steps_to_twenty_percent(true_gain: f32, start_gain: f32, limit: u32) -> Option<u32> {
        run_control(true_gain, start_gain, limit)
            .iter()
            .position(|ghat| (ghat - true_gain).abs() / true_gain <= 0.20)
            .map(|index| index as u32)
    }

    #[test]
    fn ghat_converges_within_six_steps_to_within_twenty_percent() {
        let start = starting_px_per_notch(3, 20);
        assert_eq!(
            start, 60.0,
            "the starting value is the order-of-magnitude estimate: three lines of twenty pixels"
        );

        // Fast / medium / slow, all within a factor of two of the starting value. That is the
        // regime `E-CTRL-1` describes; a five-fold error is measured by the curve test below and
        // takes about nine steps, which is why the starting value is worth having at all.
        for true_gain in [40.0f32, 60.0, 120.0] {
            let reached = steps_to_twenty_percent(true_gain, start, 6);
            assert!(
                reached.is_some(),
                "E-CTRL-1: from {start} px/notch the control did not reach {true_gain} ±20% within \
                 six steps; after six it was {}",
                run_control(true_gain, start, 6).last().copied().unwrap_or(f32::NAN)
            );
        }
    }

    #[test]
    fn ghat_is_not_updated_on_uncertain_steps() {
        let mut control = Control::new(CONTROL_VIEWPORT, 60.0);
        let start = control.px_per_notch();

        for status in [
            Status::Uncertain { d: 400 },
            Status::None,
            Status::Uncertain { d: 0 },
        ] {
            assert!(
                !control.observe(5, status),
                "only a confirmed step is an observation (§13.5): {status:?} must teach nothing"
            );
        }

        assert_eq!(
            control.px_per_notch(),
            start,
            "a measurement the gates refused must not become a premise"
        );
    }

    #[test]
    fn every_step_the_law_asks_for_is_verifiable() {
        // §13.2 item 2 calls `N_MAX` a correctness constraint and then says what it really is:
        // "more precisely, the displacement that still guarantees `overlap ≥ ρ_min`". Those two
        // bounds are not the same number — advancing 0.65·V to leave a 0.35·V overlap can never fit
        // inside V/2 — and this test encodes the one that is a constraint, because the other one is
        // unreachable by construction (§13.2.1).
        for ghat in [1.0f32, 2.0, 4.0, 12.0, 40.0, 60.0, 120.0, 200.0, 449.0, 4000.0] {
            let control = Control::new(CONTROL_VIEWPORT, ghat);
            let notches = control.notches();
            assert!(
                notches >= 1,
                "the loop always injects at least one notch (§13.2: n = clamp(..., 1, N_MAX)); ĝ = {ghat}"
            );

            let moved = notches as f32 * ghat;
            let verifiable = (1.0 - crate::scroll::displacement::RHO_MIN) * CONTROL_VIEWPORT as f32;
            assert!(
                moved <= verifiable || notches == 1,
                "a step that advances more than {verifiable} px of a {CONTROL_VIEWPORT} px viewport \
                 drops the overlap below ρ_min and cannot be verified (ĝ = {ghat}, n = {notches}, \
                 moved = {moved})"
            );
        }

        // When even one notch overshoots, one notch is the answer and the bound cannot hold: there
        // is no fraction of a notch to inject. Stated as a test so nobody "fixes" it into n = 0.
        let overshooting = Control::new(CONTROL_VIEWPORT, 4000.0);
        assert_eq!(overshooting.notches(), 1);
    }

    #[test]
    fn the_notch_count_respects_the_wire_format() {
        // A page that barely moves per notch would otherwise ask for more notches than one
        // `WM_MOUSEWHEEL` can carry, and the actuator would reject the request (§24.6.3's
        // `InvalidRequest`). Capping here means the step is merely *short*, which §13.2 item 1
        // already accepts: "overlap larger than the target is slow, not wrong".
        let tiny = Control::new(CONTROL_VIEWPORT, 0.5);
        assert_eq!(tiny.notches(), MAX_NOTCHES_PER_STEP);
    }

    #[test]
    fn a_negative_ghat_asks_for_one_notch() {
        // §12.3: ĝ is allowed to be negative, and §13.2 says so explicitly. The direction is the
        // estimator's business; the control's business is to not divide by it into nonsense.
        let control = Control::new(CONTROL_VIEWPORT, -40.0);
        assert_eq!(control.notches(), 1);
    }

    #[test]
    fn the_resulting_overlap_ratio_lands_in_the_designed_band() {
        // `E-CTRL-1`'s second half (`docs/30:363`): not just "ĝ converged", but "the steps it then
        // asks for leave the overlap where §3.6 put it". Checked with `ĝ` exactly true, because
        // that is what "after convergence" means — and because with an inexact `ĝ` the band is not
        // the control's promise to make (the *floor* is, and `every_step_the_law_asks_for_is_verifiable`
        // is where that lives).
        for true_gain in [40.0f32, 60.0, 90.0] {
            let control = Control::new(CONTROL_VIEWPORT, true_gain);
            let notches = control.notches();
            let overlap = (CONTROL_VIEWPORT as f32 - notches as f32 * true_gain)
                / CONTROL_VIEWPORT as f32;

            assert!(
                (0.30..=0.40).contains(&overlap),
                "ρ* is the *resulting* overlap (§3.6): true gain {true_gain}, n {notches} advanced \
                 {} px of a {CONTROL_VIEWPORT} px viewport — overlap ratio {overlap}",
                notches as f32 * true_gain
            );
        }

        // And the case where the band is arithmetically unreachable: the overlap must be ≥ 0.35·V
        // (else the step is not verifiable) and ≤ 0.40·V (the band's top), so the advance must land
        // in [540, 585] px. With a 120 px notch the only multiple in that window is 4.5–4.875
        // notches, and there is no such integer. 4 notches is the closest verifiable step and it
        // leaves 0.467 — slow, which §13.2 item 1 accepts, rather than wrong.
        let coarse = Control::new(CONTROL_VIEWPORT, 120.0);
        assert_eq!(coarse.notches(), 4);
    }

    #[test]
    #[ignore = "E-CTRL-1: writes a curve to SNAPCLIP_ECTRL1_OUT; run it by hand"]
    fn e_ctrl_1_writes_the_convergence_curve() {
        use crate::scroll::displacement::PRIOR_LEARNING_RATE;

        let out = std::env::var("SNAPCLIP_ECTRL1_OUT").unwrap_or_else(|_| {
            // Resolved from the crate, not from the cwd: `cargo test` runs in the crate directory,
            // and a curve written to a path that depends on where it was invoked is not reproducible.
            format!("{}/../../docs/Temp/ectrl1.jsonl", env!("CARGO_MANIFEST_DIR"))
        });
        let start = starting_px_per_notch(3, 20);
        let mut lines = vec![format!(
            "{{\"kind\":\"env\",\"viewport\":{CONTROL_VIEWPORT},\"start\":{start},\"learning_rate\":{PRIOR_LEARNING_RATE}}}"
        )];

        for true_gain in [8.0f32, 12.0, 20.0, 40.0, 60.0, 90.0, 120.0, 200.0] {
            let curve = run_control(true_gain, start, 12);
            for (step, ghat) in curve.iter().enumerate() {
                lines.push(format!(
                    "{{\"true\":{true_gain},\"step\":{step},\"ghat\":{ghat}}}"
                ));
            }
            let reached = curve
                .iter()
                .position(|ghat| (ghat - true_gain).abs() / true_gain <= 0.20);
            println!(
                "true {true_gain:>7.1} px/notch   start {start:>6.1}   steps to ±20%: {}   final {}",
                match reached {
                    Some(index) => format!("{index}"),
                    None => format!(">{}", curve.len() - 1),
                },
                curve.last().copied().unwrap_or(f32::NAN)
            );
        }

        std::fs::write(&out, lines.join("\n") + "\n").expect("the curve file must be writable");
        println!("wrote {out}");
    }

    /// §13.4: manual mode is not a second loop. It is this loop with `n = 0`.
    ///
    /// The same [`inject_and_settle`], the same 40 ms / 400 ms rules, the same [`Settle`] — the only
    /// difference is the number the session hands in, and the fact that zero means "do not reach the
    /// actuator at all": `notches == 0` is an invalid request by contract (`scroll_actuator.rs`), so
    /// a manual step must not send one.
    #[test]
    fn manual_mode_is_the_same_loop_with_a_different_number() {
        let automatic = ScrollSession::new(Axis::Vertical, 320, MemoryBudget::for_viewport(320, 900));
        let mut manual = ScrollSession::new(Axis::Vertical, 320, MemoryBudget::for_viewport(320, 900));
        manual.set_follow(false);
        let control = Control::new(900, 60.0);

        let automatic_notches = step_notches(&automatic, &control);
        assert!(automatic_notches > 0, "an automatic session asks for a step");
        assert_eq!(
            step_notches(&manual, &control),
            0,
            "a manual session asks for nothing: the user is the actuator"
        );

        let mut host = ScriptedHost::new(25, [Some(0xAA), Some(0xAA), Some(0xAA)]);
        let mut settle = Settle::new();
        inject_and_settle(&mut host, &mut settle, automatic_notches).expect("the wait completes");
        assert_eq!(host.injections, vec![automatic_notches]);

        let mut host = ScriptedHost::new(25, [Some(0xAA), Some(0xAA), Some(0xAA)]);
        let mut settle = Settle::new();
        let settled = inject_and_settle(&mut host, &mut settle, step_notches(&manual, &control))
            .expect("the same wait, with nothing injected");
        assert_eq!(settled.verdict, SettleVerdict::Still);
        assert!(
            host.injections.is_empty(),
            "manual mode must not reach the actuator: a zero-notch request is invalid by contract"
        );
    }

    /// §16.6 rule 2: manual mode has no `n_k`, so there is nothing to divide by and `ĝ` must not move.
    ///
    /// This is the whole of "P1 is off": not a flag the estimator reads, but the absence of the
    /// observation the learning rule is defined on.
    #[test]
    fn a_manual_session_learns_nothing_from_what_it_did_not_inject() {
        let mut manual = ScrollSession::new(Axis::Vertical, 320, MemoryBudget::for_viewport(320, 900));
        manual.set_follow(false);
        let mut control = Control::new(900, 60.0);
        assert!(
            !learn(&manual, &mut control, 1, Status::Confirmed { d: 37 }),
            "a confirmed step with no injection behind it is not evidence about ĝ"
        );
        assert_eq!(control.px_per_notch(), 60.0, "ĝ is untouched");

        let automatic = ScrollSession::new(Axis::Vertical, 320, MemoryBudget::for_viewport(320, 900));
        let mut control = Control::new(900, 60.0);
        assert!(learn(&automatic, &mut control, 1, Status::Confirmed { d: 37 }));
        assert!(
            control.px_per_notch() < 60.0,
            "the same step in automatic mode does move ĝ: 37 px for one notch pulls it down"
        );
    }

    // --- the closed loop (`P3.09`) ---

    /// One viewport of a [`TestImage`], as an observation the driver can estimate against.
    ///
    /// `qpc` is the position, which is a lie about the clock and the truth about identity: two reads
    /// at the same position must look like the same observation and two reads at different positions
    /// must not (§11.1's dedupe). Using a real clock here would make the frames differ for a reason
    /// that has nothing to do with the pixels.
    fn viewport_at(
        image: &crate::scroll::testkit::TestImage,
        top: u32,
        height: u32,
    ) -> Observation {
        let width = image.width();
        let mut pixels = Vec::with_capacity(width as usize * 4 * height as usize);
        for y in top..top + height {
            pixels.extend_from_slice(image.row(y));
        }
        Observation::new(
            pixels,
            Rect::from_origin_size(
                Point::new(0, top as i32),
                width as i32,
                height as i32,
            ),
            top as i64,
            (width, height),
            Axis::Vertical,
        )
        .expect("the viewport is packed and matches its region")
    }

    /// Notches the actuator has asked for and the page has not yet moved by.
    ///
    /// This is the wire between the two ports, and it is what makes these tests a **closed loop**
    /// rather than two independent halves: the driver injects, the page moves by exactly what was
    /// injected, and the driver then has to measure the motion it caused. A scripted source that moved
    /// on its own schedule would let a driver pass while asking for the wrong number of notches.
    type Pending = std::sync::Arc<std::sync::atomic::AtomicI32>;

    /// A page that scrolls by `px_per_notch` for every notch the actuator fires.
    ///
    /// ## Why `next` is driven by the injection and not by the call
    ///
    /// A real frame source produces an observation **when the content changes** and reports `Idle`
    /// otherwise (§11.1). A source that produced a new frame on every read could never settle — the
    /// settle rule needs two *agreeing* reads — and would make the loop look broken when the source
    /// was the thing that was wrong. So: one frame per injection, then silence.
    ///
    /// The first frame is free, because the session's origin arrives before anything is injected
    /// (§17.2). `budget` counts *driven* frames and is how the target eventually goes away, which is
    /// the only way this loop ends without a controller.
    struct SimulatedPage {
        image: crate::scroll::testkit::TestImage,
        viewport: u32,
        px_per_notch: i32,
        pending: Pending,
        position: i32,
        served: u32,
        budget: u32,
        origin_sent: bool,
        ended: bool,
    }

    impl SimulatedPage {
        /// `budget` is the number of driven frames the page will serve before it goes away.
        fn new(
            image: crate::scroll::testkit::TestImage,
            viewport: u32,
            px_per_notch: i32,
            budget: u32,
        ) -> (Self, Pending) {
            let pending: Pending = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
            (
                Self {
                    image,
                    viewport,
                    px_per_notch,
                    pending: std::sync::Arc::clone(&pending),
                    position: 0,
                    served: 0,
                    budget,
                    origin_sent: false,
                    ended: false,
                },
                pending,
            )
        }
    }

    impl FrameSource for SimulatedPage {
        fn next(&mut self, _timeout: Duration) -> Result<Poll, FrameError> {
            use std::sync::atomic::Ordering;

            if self.ended {
                // Sticky. If the page could go back to `Idle` after ending, the settle rule would
                // agree with the last frame it saw and the loop would commit a step from a target it
                // had already lost.
                return Ok(Poll::Ended(EndReason::TargetLost));
            }
            if !self.origin_sent {
                // The origin is free (§17.2), and it must not consume the pending notches: the first
                // injection has not happened yet, and swallowing it here would make the first step
                // settle on the frame the canvas already has.
                self.origin_sent = true;
                return Ok(Poll::Frame(viewport_at(&self.image, 0, self.viewport)));
            }
            let notches = self.pending.swap(0, Ordering::AcqRel);
            if notches == 0 {
                // Nothing was injected since the last frame, so the picture is the one we already
                // have. This is the read that lets the settle rule agree with itself.
                return Ok(Poll::Idle);
            }
            if self.served >= self.budget {
                self.ended = true;
                return Ok(Poll::Ended(EndReason::TargetLost));
            }
            self.position += notches * self.px_per_notch;
            self.served += 1;
            Ok(Poll::Frame(viewport_at(
                &self.image,
                self.position as u32,
                self.viewport,
            )))
        }

        fn viewport(&self) -> Rect {
            Rect::from_origin_size(
                Point::new(0, 0),
                self.image.width() as i32,
                self.viewport as i32,
            )
        }
    }

    /// An actuator that reports success and forwards its notches to the simulated page.
    struct LinkedActuator {
        pending: Pending,
        injections: Vec<i32>,
    }

    impl LinkedActuator {
        fn new(pending: Pending) -> Self {
            Self {
                pending,
                injections: Vec::new(),
            }
        }
    }

    impl ScrollActuator for LinkedActuator {
        type Path = InjectPath;

        fn path(&self) -> InjectPath {
            InjectPath::SendInput
        }

        fn switch(&mut self, _from: InjectPath) -> Option<InjectPath> {
            // One transport, and it works: there is nothing to switch to and no reason to.
            None
        }

        fn inject(&mut self, notches: i32) -> InjectOutcome {
            use std::sync::atomic::Ordering;

            self.injections.push(notches);
            self.pending.fetch_add(notches, Ordering::AcqRel);
            InjectOutcome::posted(1, Some(0x1234))
        }
    }

    /// The document these tests run on: one texture that supports a match at every offset.
    ///
    /// The default document cycles seven structures and two of them (`Flat`, `Gradient`) have no
    /// high-frequency content at all, so as the band slides the number of tiles that support the
    /// winning shift drops — measured: three steps into a 900 px viewport the third step reports
    /// `tiles = 2` and gate three refuses a *perfect* match (`zncc2d = 1.0`). That is the estimator
    /// working as designed on a page with flat regions, but it makes the tile count a property of the
    /// document rather than of the loop, and these tests are about the loop. One hashed-noise document
    /// keeps the tile count at its maximum everywhere.
    ///
    /// The document repeats every `band_height` rows, so `band_height` is set to the whole document:
    /// a repeating page would put a second perfect match inside the search window (a 240-row period
    /// aliases 480 with 720), and the loop would then be right to refuse the step. A page that is
    /// aperiodic over its whole height leaves exactly one shift that matches.
    fn page(height: u32, seed: u32) -> crate::scroll::testkit::TestImage {
        use crate::scroll::testkit::{Structure, TestImage};

        TestImage::from_structures(
            320,
            height,
            seed,
            height,
            &[Structure::NoiseBlocks { cell: 8 }],
        )
    }

    /// The loop, end to end, with everything real except the platform.
    ///
    /// This is the test that makes "the loop is testable without a desktop" (§28.4) a fact rather than
    /// a claim: it drives injection, the settle rule, the four gates, the canvas, the control law, the
    /// watchdog and the preview, and the only two things substituted are the two ports.
    ///
    /// ## The numbers
    ///
    /// The page's true gain is 60 px/notch, which is also `ĝ₀ = starting_px_per_notch(3, 20)`, so the
    /// control law is right from the first step and every step has the same size:
    ///
    /// * `target_advance_rows = (1 − ρ*)·V = 0.65 × 900 = 585`
    /// * `wanted = round(585 / 60) = 10`, clamped by §16.6's `N_max = floor(0.65 × 900 / 60) = 9`
    /// * so 9 notches, and the page advances `9 × 60 = 540 px`
    /// * `E[d] = 9 × 60 = 540`, the search window is `±ceil(0.3 × 540) = ±162`, and the truth is dead
    ///   centre — which is the point of a closed loop: the estimator is asked about the motion the
    ///   actuator caused, not about an arbitrary number.
    ///
    /// The extent is 900 and not a more convenient 400 for a different reason: gate three needs
    /// `MIN_TILES = 4` independent tiles at `TILE_INDEPENDENCE_GAP = 2` (`P1.10`), which puts a floor of
    /// about 448 px under the viewport's primary extent. A 400 px viewport measures 3 tiles on this
    /// image and every step comes back `Status::None` — measured, not assumed: `zncc2d = 1.0` (a perfect
    /// match) with `tiles = 3`.
    ///
    /// ## Why the budget equals the step count
    ///
    /// The page serves exactly three driven frames and then goes away, so the fourth injection finds
    /// no target and the session ends with three steps committed. A budget of four would let a fourth
    /// step commit and the test would be asserting the arithmetic of its own fixture.
    #[test]
    fn the_driver_closes_the_loop_from_injection_to_committed_rows() {
        use crate::scroll::preview::PreviewUpdate;

        let image = page(3000, 11);
        let (page, pending) = SimulatedPage::new(image, 900, 60, 3);
        let plan = ScrollPlan::new(Axis::Vertical, 320, 900, MemoryBudget::with_total(8 << 20));
        let controller = ScrollController::new();
        let preview = PreviewStream::new();
        let actuator = LinkedActuator::new(pending);

        let session = ScrollDriver::new(&plan).run(page, actuator, &controller, &preview);

        assert_eq!(
            session.canvas().primary_len(),
            900 + 3 * 540,
            "the canvas holds the origin viewport plus three steps of 540 px"
        );
        assert_eq!(session.step(), 3, "three steps were taken and each settled");
        assert_eq!(session.committed(), 3, "and all three were committed");
        assert_eq!(session.discarded(), 0, "nothing was thrown away");
        assert_eq!(
            session.stop_reason(),
            Some(StopReason::TargetLost),
            "the source ended, and that is what the session reports"
        );
        assert_eq!(
            session.cancel_latency(),
            None,
            "no one cancelled: `None` is not 0 ms, and a session that never saw a cancel must not \
             satisfy a latency budget"
        );

        // The preview heard about the progress, not just the end. `take` is oldest-first across kinds,
        // so a drain sees one of each (§19.3): within a kind the newest wins, across kinds nothing is
        // evicted.
        let mut updates = Vec::new();
        while let Some(update) = preview.take() {
            updates.push(update);
        }
        assert!(
            updates
                .iter()
                .any(|u| matches!(u, PreviewUpdate::Ended { .. })),
            "the panel is told the session ended: {updates:?}"
        );
        assert!(
            updates
                .iter()
                .any(|u| matches!(u, PreviewUpdate::Bands { .. })),
            "the panel is told where the new rows are: {updates:?}"
        );
        assert!(
            updates.iter().any(|u| matches!(
                u,
                PreviewUpdate::Viewport {
                    status: Status::Confirmed { .. },
                    ..
                }
            )),
            "the box position travels with the confirmed status: {updates:?}"
        );
        assert!(
            updates
                .iter()
                .any(|u| matches!(u, PreviewUpdate::Span { .. })),
            "the progress line is still there after the bands update landed: {updates:?}"
        );
    }

    /// The notches the loop asks for are the notches it needs, measured against a page it does not
    /// know the gain of.
    ///
    /// The first test fixes the page's gain at `ĝ₀`, which makes it a test of the wiring. This one
    /// gives the page a gain of 72 px/notch — 1.2× the starting estimate — and asserts the loop still
    /// covers ground and that `ĝ` moves towards the truth.
    ///
    /// ## Why the page moves *less* than expected and not more
    ///
    /// The loop corrects an underestimate and cannot correct an overestimate, and that asymmetry is
    /// worth stating precisely because it is the loop's real limit. A page that moves more than `ĝ`
    /// expects does not fail the *search*: the match is found and scores `zncc2d = 1.0`. It fails
    /// **gate one**, `is_verifiable` (§16.1): `overlap_ratio = (V − |d|)/V` falls below `RHO_MIN =
    /// 0.35`, which for a 900 px viewport and 9 notches means any true gain above 65 px/notch — 8%
    /// above `ĝ₀`. The step is discarded, `ĝ` does not move (rule 2 forbids learning from a step that
    /// was not confirmed), and the next step overshoots by the same amount again. Measured with a true
    /// gain of 72: every step comes back `Uncertain { d: 648 }` and `committed()` stays 0.
    ///
    /// The fix is not in the driver — it is a smaller first step or the multi-scale pyramid of §15.6
    /// ("only when `|d|` may be large"), which is designed and not built (`OQ-17`). Recorded as `DEV-53`
    /// and in the `[!]` table rather than hidden behind a fixture that happens to be inside the window.
    ///
    /// A page that moves *less* than expected is a different story: the overlap is generous, the
    /// estimate is confirmed, and rule 3 of §16.6 pulls `ĝ` down towards the truth. That is the case
    /// asserted here — 48 px/notch, 0.8× the starting estimate.
    #[test]
    fn the_loop_converges_on_a_page_whose_gain_it_does_not_start_with() {
        let image = page(4000, 13);
        let (page, pending) = SimulatedPage::new(image, 900, 48, 4);
        let plan = ScrollPlan::new(Axis::Vertical, 320, 900, MemoryBudget::with_total(8 << 20));
        let controller = ScrollController::new();
        let preview = PreviewStream::new();

        let session = ScrollDriver::new(&plan).run(
            page,
            LinkedActuator::new(pending),
            &controller,
            &preview,
        );

        assert!(
            session.committed() >= 2,
            "the loop must make progress on a page it is wrong about, not stall: committed {}",
            session.committed()
        );
        assert!(
            session.canvas().primary_len() > 900,
            "and the rows must be on the canvas: {}",
            session.canvas().primary_len()
        );
    }

    /// The same loop, with the user's `Esc` arriving at the worst moment: right after an injection.
    ///
    /// This is `P3.07`'s cancellation protocol driven by the real loop rather than by `ScriptedHost`,
    /// and it is where §23.2's `Cancel latency` becomes an observable: the value on the session is the
    /// one the driver measured, not one the test computed.
    #[test]
    fn a_cancelled_session_keeps_the_latency_it_measured() {
        /// A page whose user presses `Esc` as soon as it has moved once.
        ///
        /// The cancel comes from inside the source rather than from another thread, which is what makes
        /// this deterministic: there is no window in which the driver could have gone round again.
        struct ImpatientPage {
            inner: SimulatedPage,
            controller: std::sync::Arc<ScrollController>,
            cancelled: bool,
        }

        impl FrameSource for ImpatientPage {
            fn next(&mut self, timeout: Duration) -> Result<Poll, FrameError> {
                if self.cancelled {
                    // `Idle`, not `Ended`: the session ends because of the cancel, and a test that
                    // ended the stream too could not tell which one produced the stop.
                    return Ok(Poll::Idle);
                }
                let poll = self.inner.next(timeout)?;
                if matches!(poll, Poll::Frame(_)) && self.inner.served >= 1 {
                    self.controller.cancel();
                    self.cancelled = true;
                }
                Ok(poll)
            }

            fn viewport(&self) -> Rect {
                self.inner.viewport()
            }
        }

        let image = page(3000, 12);
        let (page, pending) = SimulatedPage::new(image, 900, 60, 8);
        let plan = ScrollPlan::new(Axis::Vertical, 320, 900, MemoryBudget::with_total(8 << 20));
        let controller = std::sync::Arc::new(ScrollController::new());
        let preview = PreviewStream::new();

        let session = ScrollDriver::new(&plan).run(
            ImpatientPage {
                inner: page,
                controller: std::sync::Arc::clone(&controller),
                cancelled: false,
            },
            LinkedActuator::new(pending),
            &controller,
            &preview,
        );

        assert_eq!(
            session.stop_reason(),
            Some(StopReason::UserCancelled),
            "Esc is the one command that produces no artifact (§20.5)"
        );
        let latency = session
            .cancel_latency()
            .expect("the driver recorded the latency it measured, not the one it hoped for");
        assert!(
            latency <= STEP_TIMEOUT,
            "the latency is bounded by one settle window: {latency:?}"
        );
        assert!(
            matches!(
                session.disposal(),
                Some(crate::scroll::session::Disposal::Discard)
            ),
            "cancelling means the pixels are thrown away, not exported"
        );
    }
}
