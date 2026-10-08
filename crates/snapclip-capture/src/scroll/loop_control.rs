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
}
