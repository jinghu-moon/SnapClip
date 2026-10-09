//! The platform seams of a scroll session (`docs/30` §27.1, §27.3).
//!
//! # Why these live here and not next to their Windows implementations
//!
//! §28.4's gate is mechanical: nothing under `scroll/` may reference the Windows module, and that
//! includes the test modules. The driver is `scroll/`'s (`loop_control.rs`, §28.2), so every type
//! the driver names has to live on this side of the line — otherwise the loop and the gate cannot
//! both be satisfied, and the usual way out of that (an exception in the gate) would turn §28.4's
//! "the loop is testable without a desktop" from a fact into a claim.
//!
//! So the *vocabulary* is here and the *behaviour* is next door: `windows/scroll_source.rs`
//! implements [`FrameSource`] over WGC, `windows/scroll_actuator.rs` implements [`ScrollActuator`]
//! over `SendInput`/`PostMessageW`. Those modules re-export these names, so their own code and their
//! tests read exactly as they did before the move (`P2.03`/`P3.01`'s evidence stays valid).
//!
//! # The split inside this file
//!
//! * [`Poll`] / [`FrameSource`] — what the driver asks the platform for, and the three answers that
//!   are not the same answer: no new observation ([`Poll::Idle`]), an observation whose displacement
//!   may be zero ([`Poll::Frame`]), and the end of the stream ([`Poll::Ended`]). §11.1's whole point
//!   is that the first two must never be conflated.
//! * [`ScrollActuator`] / [`InjectOutcome`] — what the driver tells the platform to do. The actuator
//!   owns the target and the transport choice (`P3.02`'s `Choice`, in `windows/scroll_actuator.rs`),
//!   because those are decided **once** at session start; a driver that rebuilt them per step would
//!   be re-running the decision table it was handed the answer to.

// The vocabulary is deliberately wider than the driver: it is the contract the *platform* satisfies,
// and two of its corners are read by callers that do not exist yet. `FrameSource::viewport` is §2.1's
// session-invariant check and belongs to the assembly that opens a stream (`P4`); `InjectOutcome`'s
// `delivered`/`target_window` are §24.7's diagnostics, read by the L3 probes and by the degradation
// report; `FrameError::DeviceLost` and `EndReason::{DeviceLost, Timeout}` are produced by the WGC
// backend at runtime, not by anything this crate's tests construct. Narrowing the contract to what
// today's call sites read would make each of those a breaking change later, for no gain now.
#![allow(dead_code)]

use std::time::Duration;

use crate::geometry::Rect;
use crate::scroll::observation::Observation;

/// Why a stream ended. Every one of these is a **normal** outcome; none of them is an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EndReason {
    /// The target stopped being capturable: closed, minimised, or resized away from the
    /// viewport this session was built for. The three are distinguished by the detail the
    /// source records, not by new variants (`docs/31` `P2.07`).
    TargetLost,
    /// The capture machinery failed on something that will not fix itself.
    CaptureFailed,
    /// The GPU device went away.
    DeviceLost,
    /// The session ran out of its own time budget.
    Timeout,
}

/// One attempt to get a new observation.
#[derive(Debug)]
pub(crate) enum Poll {
    /// A new observation. Its displacement may be zero — that is the estimator's business.
    Frame(Observation),
    /// Nothing new within the timeout. The caller keeps waiting or stops for its own reasons.
    Idle,
    /// The stream is over.
    ///
    /// Unlike [`Poll::Idle`], this is final, and it carries an obligation: the caller must hand
    /// out the rows it has already confirmed as a `Partial` before tearing the session down
    /// (`docs/30` §24.4). An ending is never a reason to throw the canvas away — the session may
    /// have been running for minutes when the user closed the tab.
    Ended(EndReason),
}

/// A failure that is **not** an ending: the stream may still be alive next time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FrameError {
    /// The device was lost; `id` names which one so a restarted session can be told apart.
    DeviceLost { id: String },
    /// One attempt failed. Retrying is allowed and is the caller's decision.
    Transient {
        context: &'static str,
        detail: String,
    },
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeviceLost { id } => write!(formatter, "the GPU device {id} was lost"),
            Self::Transient { context, detail } => write!(formatter, "{context}: {detail}"),
        }
    }
}

/// The stream a scroll session reads from (`docs/30` §27.3).
pub(crate) trait FrameSource {
    /// Wait at most `timeout` for a new observation.
    fn next(&mut self, timeout: Duration) -> Result<Poll, FrameError>;
    /// The viewport this session was opened for. Invariant for the session's lifetime
    /// (`docs/30` §2.1 推论 2.6): a target that changes size ends the stream instead.
    fn viewport(&self) -> Rect;
}

/// Which transport fires the wheel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InjectPath {
    /// The system input queue. The routing rule decides where it lands.
    SendInput,
    /// A message addressed to a window we pick.
    PostMessageW,
}

/// Who places the cursor before a `SendInput` notch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Aim {
    /// This process places it. The normal case: under `MOUSE_POS` routing the
    /// wheel follows the cursor, so the aim is part of the injection.
    PlaceCursor,
    /// The caller has already placed it.
    ///
    /// This exists because a sender at a **lower** integrity level than the
    /// foreground window cannot call `SetCursorPos` — it fails and leaves the
    /// last error at 0. The low-integrity arm of `E-INJECT-1` therefore has the
    /// operator aim the cursor and sets this, which leaves only the delivery
    /// step under test instead of reporting "the cursor could not be placed".
    AssumePlaced,
}

/// The outcome of one injection, in the shared vocabulary of `docs/30` §24.6
/// rule 3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InjectStatus {
    /// The transport accepted it. **Not** evidence that the target acted —
    /// `PostMessageW` returning `TRUE` only means the message was queued, and
    /// `SendInput` returning `1` only means the event was inserted. The evidence
    /// is a content displacement, which is `P3.03`'s job (§24.7).
    Posted,
    /// The request cannot be expressed on the wire.
    InvalidRequest,
    /// The handle is not a window any more.
    TargetNotFound,
    /// The point could not be resolved, or the cursor could not be placed.
    /// `code == 0` means no Win32 call failed — the window tree could not be
    /// walked, which is what a failed `ClientToScreen` looks like.
    CoordinateFailure { code: i32 },
    /// The transport refused. For a post that is access or validity, because a
    /// post has no queue slot to fail on.
    PostFailed { code: i32 },
}

/// What one injection produced.
#[derive(Clone, Copy, Debug)]
pub(crate) struct InjectOutcome {
    pub status: InjectStatus,
    /// Events accepted by the transport (1 for a post, `1` per notch for
    /// `SendInput`). Meaningful only when `status == Posted`.
    pub delivered: u32,
    /// The window that actually received the message, when there was one. The
    /// descended child is not the window we were handed, and saying which one it
    /// was is the difference between "the target ignored it" and "we posted to
    /// the wrong window".
    pub target_window: Option<isize>,
}

impl InjectOutcome {
    pub(crate) fn posted(delivered: u32, target_window: Option<isize>) -> Self {
        Self {
            status: InjectStatus::Posted,
            delivered,
            target_window,
        }
    }

    pub(crate) fn failed(status: InjectStatus) -> Self {
        Self {
            status,
            delivered: 0,
            target_window: None,
        }
    }
}

/// The actuator a scroll driver injects through (`docs/30` §27.3).
///
/// # Why `inject` takes only a notch count
///
/// §27.3's sketch is `inject(&mut self, req: &InjectRequest)`, and the request carries the target
/// handle, the screen point, the transport and who aims. Every one of those is the output of
/// `scroll_actuator::choose`, which runs **once**, at session start, against facts that do not
/// change during a session (`SPI_GETMOUSEWHEELROUTING` is read at start too). A driver that rebuilt
/// the request per step would have to re-derive the whole decision table — and the one thing a
/// second copy of a decision table guarantees is that the two copies eventually disagree.
///
/// So the actuator keeps its own target and its own `Choice` (the decision table's output, defined in
/// `windows/scroll_actuator.rs`), and the driver's side of the contract is the notch count. The
/// transport can still change mid-session ([`Self::switch`], `P3.03`'s watchdog), and that change is
/// the actuator's to make.
pub(crate) trait ScrollActuator {
    /// The transport this actuator is using. Opaque to the driver: it is carried, compared and
    /// handed back, never interpreted.
    type Path: Copy + PartialEq + std::fmt::Debug;

    /// The transport currently in use.
    fn path(&self) -> Self::Path;

    /// The transport to try after `from`, or `None` when there is no next one.
    ///
    /// `None` is the honest answer for "the plan is exhausted" and it is what turns `P3.03`'s
    /// watchdog into a stop rather than an oscillation between two transports that both fail.
    fn switch(&mut self, from: Self::Path) -> Option<Self::Path>;

    /// Inject `notches` of wheel movement. Positive advances the document (§13.2).
    ///
    /// Returns an outcome rather than a `Result` for §27.4's reason: a refused injection is a fact
    /// the watchdog classifies, not an error the driver propagates.
    fn inject(&mut self, notches: i32) -> InjectOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two seam types cross a thread boundary: the driver owns them, the overlay thread reads
    /// the preview next to them (`docs/30` §21.1). A trait with a `Send` bound would be the wrong
    /// place to state that — the *implementations* are what move — so this asserts it where it can
    /// be seen: on the outcomes.
    #[test]
    fn the_port_vocabulary_crosses_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<EndReason>();
        assert_send_sync::<FrameError>();
        assert_send_sync::<InjectPath>();
        assert_send_sync::<Aim>();
        assert_send_sync::<InjectStatus>();
        assert_send_sync::<InjectOutcome>();
    }

    /// §11.1: `Idle` and `Frame` are different answers, and the type has to be able to say so.
    /// A source that could only return "a frame" would make "the page is still" and "the target is
    /// gone" the same observation, which is the mistake `P2.03` was written to avoid.
    #[test]
    fn an_ended_stream_is_not_an_idle_one() {
        let idle = Poll::Idle;
        let ended = Poll::Ended(EndReason::TargetLost);
        assert!(matches!(idle, Poll::Idle));
        assert!(!matches!(ended, Poll::Idle));
    }
}
