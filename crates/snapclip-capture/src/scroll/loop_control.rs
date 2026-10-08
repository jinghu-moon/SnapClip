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
//! * The loop itself — inject, settle, estimate, commit (`P3.04`).
//! * `EndReached`: §20.4's convergence table puts "content stopped changing" under `EndReached`,
//!   not under `ActuatorFailed`. The caller must classify a step as "the page is at its end"
//!   **before** feeding it to this watchdog; a page that is merely at its bottom would otherwise
//!   be reported as a broken actuator. Deciding that needs the scroll-position channel, which is
//!   `P3.04`'s delivery.

#![allow(dead_code)] // First consumer is `P3.04`; the second is the session assembly (`P3.09`).

use std::time::{Duration, Instant};

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
    host.inject(notches).map_err(StepError::Injection)?;
    settle.begin(host.now());

    loop {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        step_ms: u64,
        digests: std::collections::VecDeque<Option<u64>>,
        inject_error: Option<String>,
        injections: Vec<i32>,
    }

    impl ScriptedHost {
        fn new(step_ms: u64, digests: impl IntoIterator<Item = Option<u64>>) -> Self {
            Self {
                clock: Instant::now(),
                step_ms,
                digests: digests.into_iter().collect(),
                inject_error: None,
                injections: Vec::new(),
            }
        }
    }

    impl StepHost for ScriptedHost {
        fn inject(&mut self, notches: i32) -> Result<(), String> {
            self.injections.push(notches);
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
}
