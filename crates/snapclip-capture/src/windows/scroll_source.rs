//! Window-level frame source for a scroll session (`docs/30` §11.1, §11.2, §27.3).
//!
//! A scroll session needs a **stream**: the same target is captured again and again while we
//! drive its scroll position, and the driver thread has to be able to say three different
//! things about every attempt — "here is a new observation", "nothing changed", and "this is
//! over". `FrozenFrame` cannot express that: it owns a `OnceLock` of pixels, so the type itself
//! forbids a second frame (`docs/30` §2.1 `R-3`). This module is the replacement.
//!
//! The three answers are deliberately not two:
//!
//! * [`Poll::Frame`] — a new observation, **whose displacement may still be zero** (the page
//!   reflowed, a sticky header moved, an animation ran). The estimator gets to look at it.
//! * [`Poll::Idle`] — no new observation within the timeout. Not an error, and **not** a
//!   cancellation: the driver keeps waiting or concludes the target has nothing left to give.
//! * [`Poll::Ended`] — the stream is over, with a reason. Also not an error: a window that was
//!   closed is a normal outcome of a long capture, and the canvas it produced so far is still
//!   worth exporting (`docs/30` §17.6).
//!
//! Collapsing `Idle` into `Ended` is the specific mistake that makes "we reached the bottom" a
//! guess — `docs/30` §11.1 calls it out, and `docs/30` §24.2.1 records the measurement that
//! makes it likely: Windows Graphics Capture only produces a frame when the composited content
//! changes, so a still page looks exactly like a dead one.
//!
//! **This module does not choose its target.** Which window to scroll is the app's decision
//! (`docs/30` §9.2); a frame source that enumerated windows would be doing target selection
//! behind the caller's back, and it would do it on the driver thread with no way to show the
//! user what it picked.
//!
//! Two layers live here on purpose:
//!
//! * [`FrameBackend`] is the seam that touches the platform — one WGC session, or a scripted
//!   stand-in. It answers with raw pixels and never with policy.
//! * [`WgcFrameSource`] owns the policy: the deadline, the "byte-identical means no new
//!   observation" rule, and the mapping from platform errors to [`EndReason`]. It can therefore
//!   be tested without a desktop (`docs/31` `P2.08`).

// Nothing here has a production caller yet: `P2.05`/`P2.06` choose the backend and `P3.09`
// assembles the session that drives it. The rules themselves are exercised by the tests below,
// so this is "not wired yet", not "unused". The allow is scoped to this file on purpose — a
// symbol added here later that nothing calls should be visible, not inherited.
#![allow(dead_code)]

use std::time::Duration;

use crate::geometry::Rect;
use crate::scroll::displacement::line_digest;
use crate::scroll::observation::{Axis, Observation};

use super::providers::{ProviderKind, ScrollFrame};
use super::win::d3d11::GraphicsDevice;
use super::win::wgc::{WgcError, WgcSession};
use super::monitor::{self, CapturedMonitor};
use ::windows::Win32::Foundation::HWND;
use ::windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindow};

/// The scroll path's backend order (`docs/30` §11.5, task `P2.06`).
///
/// **Policy, not a retry.** The window backend is first because a window-level capture
/// cannot contain the overlay or an occluder (§24.2) — that is a statement about what the
/// pixels *are*, so it is decided before anything is attempted rather than after something
/// fails. The monitor backend is the only alternative that still produces pixels when
/// `CreateForWindow` is refused (§24.7), and it is the one path that needs `WDA` (§24.5).
///
/// `BitBlt` is deliberately absent. It is the *ordinary* screenshot path's final fallback
/// (`providers::attempt_order`), and falling back to it here would silently hand back a
/// frame of the desktop that is not the target window's.
///
/// The ordinary path's order is **not** touched by this: §33.5 protects it, and the lesson
/// of §21.3 is that scroll work must not reshape the F5 path. Its behaviour is pinned by
/// `providers::tests::wgc_is_tried_before_the_bitblt_fallback`.
pub(crate) const SCROLL_BACKENDS: [ProviderKind; 2] =
    [ProviderKind::WgcWindow, ProviderKind::Wgc];

/// Why the scroll path moved on from one backend to the next (§11.5: a fallback that is not
/// recorded is a fallback the user cannot see — G12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackendFallback {
    /// The backend that was being used.
    pub from: ProviderKind,
    /// The backend being moved to.
    pub to: ProviderKind,
    /// The failure that caused the move. Carried so the diagnostic can say *why*.
    pub reason: String,
}

impl BackendFallback {
    /// The line the session records. It names both ends **and** the reason, because "we fell
    /// back" without "from what" or "why" is not a diagnostic — that is exactly the Snow
    /// Shot counter-example §11.5 cites (`let _ = session.SetIsCursorCaptureEnabled(false)`).
    pub(crate) fn diagnostic(&self) -> String {
        format!(
            "scroll backend fell back from {} to {}: {}",
            self.from.name(),
            self.to.name(),
            self.reason
        )
    }
}

/// The backend to try after `current`, or `None` when the plan is exhausted.
///
/// Returning `None` for anything outside the plan is deliberate: a backend the scroll path
/// does not own cannot be "advanced past", so asking is a wiring mistake and the answer must
/// not be a plausible-looking next step.
pub(crate) fn next_scroll_backend(current: ProviderKind) -> Option<ProviderKind> {
    let index = SCROLL_BACKENDS.iter().position(|kind| *kind == current)?;
    SCROLL_BACKENDS.get(index + 1).copied()
}

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

/// What a display-topology or window change means for a running scroll session
/// (`docs/30` §24.4, task `P2.05`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TopologyOutcome {
    /// Nothing that can invalidate the canvas changed. The session keeps going.
    Continue,
    /// The target can no longer be a scroll target. The caller **must** make the rows it has
    /// already confirmed available as a `Partial` export before it tears the session down:
    /// that requirement is the whole reason this is not just "cancel" (§24.4, last
    /// paragraph).
    Stop(EndReason),
}

/// The target's own facts, as far as the canvas is concerned.
///
/// Deliberately **not** here: the window's position, and which monitor it is on. A
/// window-level capture takes the window's *content*, so moving the window — including to
/// another monitor with the same scaling — cannot invalidate the rows already written.
/// Only the physical-pixel scale and the content's size are part of the canvas coordinate
/// system, and minimising is the platform telling us there is nothing to capture.
///
/// This absence is load-bearing, so it is pinned by a test: adding a position or a monitor
/// field would make rows 2 and 4 of §24.4's table stop the session, which is exactly the
/// behaviour the table exists to remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TargetGeometry {
    /// The effective DPI of the monitor the target is on. A change here changes the
    /// physical-pixel scale, which is what the canvas is measured in.
    pub dpi: u32,
    /// The target's frame size in physical pixels.
    pub size: (u32, u32),
    /// Whether the target is minimised (`IsIconic`).
    pub minimised: bool,
}

/// Decide what a change means by comparing the target with itself, not by classifying the
/// message that arrived.
///
/// This is the whole point of the function: `WM_DISPLAYCHANGE`, `WM_DPICHANGED` and
/// `WM_DEVICECHANGE` all reach the same call, because the same message can be either
/// "the target's scaling changed" or "an unrelated monitor woke up". Asking the target is
/// the only way to tell them apart (§24.4).
pub(crate) fn topology_outcome(
    before: &TargetGeometry,
    after: &TargetGeometry,
) -> TopologyOutcome {
    if after.minimised {
        return TopologyOutcome::Stop(EndReason::TargetLost);
    }
    if after.dpi != before.dpi {
        return TopologyOutcome::Stop(EndReason::TargetLost);
    }
    if after.size != before.size {
        return TopologyOutcome::Stop(EndReason::TargetLost);
    }
    TopologyOutcome::Continue
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

/// What the platform seam can answer. Raw material only — no policy.
pub(crate) enum BackendPoll {
    /// A frame that has already been read back exactly once (`docs/30` §11.3).
    Frame {
        pixels: Vec<u8>,
        size: (u32, u32),
        qpc: i64,
    },
    /// No new content within the timeout.
    Idle,
    /// The target is gone, or the capture machinery gave up. `detail` says which of the
    /// endings it was (`docs/31` `P2.07`) — `EndReason` alone cannot tell a close from a
    /// resize.
    Ended { reason: EndReason, detail: String },
}

/// The platform side of a [`FrameSource`].
///
/// What the platform says about the target right now, asked before every poll.
///
/// This exists because "the pool had no frame for me" is not evidence about *why*. A closed
/// window, a minimised window and a page that is simply not moving all produce no frames, so
/// without asking, all three become `Idle` forever and the session cannot tell "the user is
/// reading" from "the user closed the tab" (§11.1, §24.4 row 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetLiveness {
    Alive,
    /// `IsWindow` says the handle no longer names a window.
    Closed,
    /// `IsIconic` says the window is minimised.
    Minimised,
}

/// Implemented once for real (WGC) and once by a scripted stand-in, which is what lets the
/// stream rules be tested without a desktop.
pub(crate) trait FrameBackend {
    fn poll_frame(&mut self, timeout: Duration) -> Result<BackendPoll, FrameError>;
    /// The viewport size this backend will deliver, known before the first frame.
    fn size(&self) -> (u32, u32);
    /// Whether the target can still produce frames at all.
    ///
    /// Asked before `poll_frame`, and it must be cheap: it is one `IsWindow`/`IsIconic` pair.
    fn liveness(&mut self) -> TargetLiveness;
}

/// A [`FrameSource`] over a [`FrameBackend`]: owns the deadline, the duplicate rule and the
/// error mapping.
pub(crate) struct WgcFrameSource {
    backend: Box<dyn FrameBackend>,
    axis: Axis,
    size: (u32, u32),
    /// Per-row digests of the last observation we handed out, so "the platform re-delivered the
    /// same pixels" is answered without holding a second copy of the viewport (`docs/30` §11.3).
    last_rows: Option<Vec<u64>>,
    /// Why the stream ended, in words. The session turns this into a `StopReason` detail
    /// (`docs/31` `P2.07`); `EndReason` alone cannot tell a close from a resize.
    detail: Option<String>,
    ended: bool,
}

impl WgcFrameSource {
    pub(crate) fn new(backend: Box<dyn FrameBackend>, axis: Axis) -> Self {
        let size = backend.size();
        Self {
            backend,
            axis,
            size,
            last_rows: None,
            detail: None,
            ended: false,
        }
    }

    /// Why the stream ended, in words, once it has.
    pub(crate) fn last_detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    fn end(&mut self, reason: EndReason, detail: impl Into<String>) -> Poll {
        self.ended = true;
        self.detail = Some(detail.into());
        Poll::Ended(reason)
    }
}

impl FrameSource for WgcFrameSource {
    fn next(&mut self, timeout: Duration) -> Result<Poll, FrameError> {
        if self.ended {
            // An ending is final: the caller's next question is answered from what we already
            // know rather than by polling a window we have established is gone.
            return Ok(Poll::Ended(EndReason::TargetLost));
        }
        let deadline = std::time::Instant::now() + timeout;
        loop {
            // Asked before the poll, not after a timeout: a window that is gone or minimised is
            // a fact we can have now, and waiting out the timeout first would report `Idle` for
            // a target that is never coming back (§24.4 row 5).
            match self.backend.liveness() {
                TargetLiveness::Alive => {}
                TargetLiveness::Closed => {
                    return Ok(self.end(
                        EndReason::TargetLost,
                        "the target window is gone: IsWindow no longer names a window for this \
                         handle",
                    ));
                }
                TargetLiveness::Minimised => {
                    return Ok(self.end(
                        EndReason::TargetLost,
                        "the target window is minimised: a minimised window produces no frames, \
                         which would otherwise be indistinguishable from a page that is not \
                         moving",
                    ));
                }
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match self.backend.poll_frame(remaining)? {
                BackendPoll::Idle => return Ok(Poll::Idle),
                BackendPoll::Ended { reason, detail } => return Ok(self.end(reason, detail)),
                BackendPoll::Frame { pixels, size, qpc } => {
                    if size != self.size {
                        let detail = format!(
                            "the target resized: this session was opened for {}x{} but the \
                             platform delivered {}x{}",
                            self.size.0, self.size.1, size.0, size.1
                        );
                        return Ok(self.end(EndReason::TargetLost, detail));
                    }
                    let rows = row_digests(&pixels, size);
                    if self.last_rows.as_deref() == Some(rows.as_slice()) {
                        // Byte-identical to the last observation: the platform re-delivers
                        // unchanged content (docs/30 §24.2.1 fact 3), and §11.1 says that is
                        // `Idle`, not a new observation.
                        if std::time::Instant::now() >= deadline {
                            return Ok(Poll::Idle);
                        }
                        continue;
                    }
                    self.last_rows = Some(rows);
                    let region = Rect::new(0, 0, size.0 as i32, size.1 as i32);
                    let observation = Observation::new(pixels, region, qpc, size, self.axis)
                        .expect("the source builds the region from the size it delivered");
                    return Ok(Poll::Frame(observation));
                }
            }
        }
    }

    fn viewport(&self) -> Rect {
        Rect::new(0, 0, self.size.0 as i32, self.size.1 as i32)
    }
}

/// Per-row FNV digests, the same ones §11.3 uses to compare frames.
fn row_digests(pixels: &[u8], size: (u32, u32)) -> Vec<u64> {
    let stride = size.0 as usize * 4;
    (0..size.1 as usize)
        .map(|row| line_digest(&pixels[row * stride..(row + 1) * stride]))
        .collect()
}

/// The production backend: one [`WgcSession`] driven as a stream.
///
/// Its first consumer is the session assembly (`P3.09`); until then only the stream rules
/// around it are exercised.
pub(crate) struct WgcFrameBackend {
    device: std::sync::Arc<GraphicsDevice>,
    session: WgcSession,
    size: (u32, u32),
    /// Kept here rather than on `WgcSession` because it is the *stream's* business: the
    /// session only knows about the capture item, and a closed window leaves the item looking
    /// perfectly valid.
    handle: isize,
}

impl WgcFrameBackend {
    pub(crate) fn open(device: std::sync::Arc<GraphicsDevice>, handle: isize) -> Result<Self, FrameError> {
        let session = WgcSession::open(&device, handle).map_err(|error| FrameError::Transient {
            context: "WgcSession::open",
            detail: error.to_string(),
        })?;
        let (width, height) = session.size();
        Ok(Self {
            device,
            session,
            size: (width.max(0) as u32, height.max(0) as u32),
            handle,
        })
    }
}

impl FrameBackend for WgcFrameBackend {
    fn poll_frame(&mut self, timeout: Duration) -> Result<BackendPoll, FrameError> {
        match self.session.next_frame(timeout) {
            Ok(Some(frame)) => {
                let (width, height) = frame.size();
                let (width, height) = (width.max(0) as u32, height.max(0) as u32);
                let mut scroll = ScrollFrame::from_wgc(self.device.clone(), frame);
                let pixels = scroll
                    .read_region(Rect::new(0, 0, width as i32, height as i32))
                    .map_err(|error| FrameError::Transient {
                        context: "ScrollFrame::read_region",
                        detail: error.to_string(),
                    })?;
                Ok(BackendPoll::Frame {
                    pixels,
                    size: (width, height),
                    qpc: qpc_now(),
                })
            }
            Ok(None) => Ok(BackendPoll::Idle),
            Err(WgcError::InvalidTarget { handle, detail }) => {
                let _ = handle;
                Ok(BackendPoll::Ended {
                    reason: EndReason::TargetLost,
                    detail,
                })
            }
            Err(WgcError::Failed { context, detail }) => Ok(BackendPoll::Ended {
                reason: EndReason::CaptureFailed,
                detail: format!("{context}: {detail}"),
            }),
        }
    }

    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn liveness(&mut self) -> TargetLiveness {
        let hwnd = HWND(self.handle as *mut core::ffi::c_void);
        if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            if unsafe { IsIconic(hwnd) }.as_bool() {
                TargetLiveness::Minimised
            } else {
                TargetLiveness::Alive
            }
        } else {
            TargetLiveness::Closed
        }
    }
}


/// `QueryPerformanceCounter`, the clock `Observation::qpc` is measured in (`docs/30` §11.2).
///
/// It is here rather than inline so that the frame source has exactly one clock.
fn qpc_now() -> i64 {
    let mut value = 0i64;
    let _ = unsafe { ::windows::Win32::System::Performance::QueryPerformanceCounter(&mut value) };
    value
}

/// The geometry a scroll session is opened with (`docs/30` §9.2, §11.2).
///
/// It is produced once, from the window, and never re-derived behind the caller's back: the
/// content size comes from the capture item (§24.2.1 fact 7 — the item's size is the content
/// area, **not** the window rectangle), the frame rectangle is where the pointer has to sit for
/// the wheel-routed injection paths (§24.6), and the monitor decides the capture's scale.
/// Its first production consumer is the session assembly (`P3.09`); until then only the
/// geometry it reports is exercised.
#[derive(Clone)]
pub(crate) struct ScrollSourceRuntime {
    handle: isize,
    content: (u32, u32),
    frame: Rect,
    monitor: CapturedMonitor,
}

impl ScrollSourceRuntime {
    pub(crate) fn open(handle: isize, content: (u32, u32)) -> Result<Self, FrameError> {
        let detail = |what: &'static str, message: String| FrameError::Transient {
            context: what,
            detail: message,
        };
        let frame = super::win::window::frame_bounds(handle)
            .ok_or_else(|| detail("frame_bounds", format!("{handle:#x} has no DWM frame bounds")))?;
        let centre = crate::geometry::Point::new(
            frame.left + frame.width() / 2,
            frame.top + frame.height() / 2,
        );
        let monitor = monitor::captured_monitor_at(centre)
            .map_err(|message| detail("captured_monitor_at", message))?;
        Ok(Self {
            handle,
            content,
            frame,
            monitor,
        })
    }

    pub(crate) fn handle(&self) -> isize {
        self.handle
    }

    pub(crate) fn content(&self) -> (u32, u32) {
        self.content
    }

    pub(crate) fn frame(&self) -> Rect {
        self.frame
    }

    pub(crate) fn monitor(&self) -> &CapturedMonitor {
        &self.monitor
    }

    pub(crate) fn viewport(&self) -> Rect {
        Rect::new(0, 0, self.content.0 as i32, self.content.1 as i32)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    use super::{
        BackendFallback, BackendPoll, EndReason, FrameBackend, FrameError, FrameSource, Poll,
        ProviderKind, SCROLL_BACKENDS, TargetGeometry, TargetLiveness, TopologyOutcome,
        WgcFrameSource, next_scroll_backend, topology_outcome,
    };
    use crate::geometry::Rect;
    use crate::scroll::observation::{Axis, Observation};

    /// A backend that hands out a scripted sequence. Nothing here touches the platform, which
    /// is the point: every rule this module owns is testable without a desktop.
    struct ScriptedBackend {
        size: (u32, u32),
        steps: VecDeque<ScriptedStep>,
        last: Option<Vec<u8>>,
        /// Shared with the test so "was the platform asked at all?" is answerable — the
        /// liveness rules are about *not* polling, so the count has to be observable from
        /// outside the box.
        polls: Arc<AtomicU32>,
        timeouts: Vec<Duration>,
        liveness: TargetLiveness,
    }

    enum ScriptedStep {
        /// `None` pixels re-delivers exactly what was delivered last time; `None` size means
        /// "the size this backend declared".
        Frame {
            pixels: Option<Vec<u8>>,
            size: Option<(u32, u32)>,
        },
        Idle,
        Ended(EndReason),
        Failed(FrameError),
    }

    impl ScriptedBackend {
        fn new(size: (u32, u32), steps: Vec<ScriptedStep>) -> Self {
            Self::with_liveness(size, TargetLiveness::Alive, steps, Arc::new(AtomicU32::new(0)))
        }

        fn with_liveness(
            size: (u32, u32),
            liveness: TargetLiveness,
            steps: Vec<ScriptedStep>,
            polls: Arc<AtomicU32>,
        ) -> Self {
            Self {
                size,
                steps: steps.into(),
                last: None,
                polls,
                timeouts: Vec::new(),
                liveness,
            }
        }
    }

    fn solid(size: (u32, u32), level: u8) -> Vec<u8> {
        vec![level; size.0 as usize * size.1 as usize * 4]
    }

    /// The same bytes as the previous delivery.
    fn repeated() -> ScriptedStep {
        ScriptedStep::Frame {
            pixels: None,
            size: None,
        }
    }

    /// A frame of a size the session was not opened for.
    fn resized(size: (u32, u32), level: u8) -> ScriptedStep {
        ScriptedStep::Frame {
            pixels: Some(solid(size, level)),
            size: Some(size),
        }
    }

    impl FrameBackend for ScriptedBackend {
        fn poll_frame(&mut self, timeout: Duration) -> Result<BackendPoll, FrameError> {
            let polls = self.polls.fetch_add(1, Ordering::SeqCst) + 1;
            self.timeouts.push(timeout);
            match self.steps.pop_front() {
                Some(ScriptedStep::Frame { pixels, size }) => {
                    let size = size.unwrap_or(self.size);
                    let pixels = match pixels {
                        Some(pixels) => pixels,
                        None => self
                            .last
                            .clone()
                            .expect("re-delivering requires an earlier delivery"),
                    };
                    self.last = Some(pixels.clone());
                    Ok(BackendPoll::Frame {
                        pixels,
                        size,
                        qpc: polls as i64,
                    })
                }
                Some(ScriptedStep::Idle) | None => Ok(BackendPoll::Idle),
                Some(ScriptedStep::Ended(reason)) => Ok(BackendPoll::Ended {
                    reason,
                    detail: "scripted".to_string(),
                }),
                Some(ScriptedStep::Failed(error)) => Err(error),
            }
        }

        fn size(&self) -> (u32, u32) {
            self.size
        }

        fn liveness(&mut self) -> TargetLiveness {
            self.liveness
        }
    }

    fn source(size: (u32, u32), steps: Vec<ScriptedStep>) -> WgcFrameSource {
        WgcFrameSource::new(Box::new(ScriptedBackend::new(size, steps)), Axis::Vertical)
    }

    /// A source whose target is already gone or already minimised, plus the shared poll counter.
    ///
    /// The script is empty on purpose: if the source asks about liveness the way §24.4 requires,
    /// it answers without ever polling, and an empty script makes "it polled anyway" visible as
    /// a change in the counter rather than as a frame it happened to get.
    fn gone_source(
        size: (u32, u32),
        liveness: TargetLiveness,
    ) -> (WgcFrameSource, Arc<AtomicU32>) {
        let polls = Arc::new(AtomicU32::new(0));
        let backend = ScriptedBackend::with_liveness(size, liveness, Vec::new(), polls.clone());
        (
            WgcFrameSource::new(Box::new(backend), Axis::Vertical),
            polls,
        )
    }

    #[test]
    fn next_returns_idle_instead_of_blocking_forever_when_no_frame_arrives() {
        let size = (8, 4);
        let mut source = source(size, vec![ScriptedStep::Idle]);
        let started = Instant::now();
        let answer = source.next(Duration::from_millis(80));
        assert!(
            matches!(answer, Ok(Poll::Idle)),
            "a backend with nothing to give must produce Idle, not a block and not an ending"
        );
        assert!(
            started.elapsed() < Duration::from_millis(600),
            "Idle must come back on the timeout, not eventually: {answer:?}"
        );
        assert_eq!(source.viewport().width(), size.0 as i32);
        assert_eq!(source.viewport().height(), size.1 as i32);
    }

    #[test]
    fn a_closed_window_ends_the_stream_with_a_reason() {
        let mut source = source(
            (4, 4),
            vec![ScriptedStep::Ended(EndReason::TargetLost)],
        );
        let answer = source.next(Duration::from_millis(50));
        assert!(
            matches!(answer, Ok(Poll::Ended(EndReason::TargetLost))),
            "a closed window is an ending, not an error: {answer:?}"
        );
        assert!(source.last_detail().is_some());
    }

    #[test]
    fn a_transient_failure_is_not_an_ending() {
        let mut source = source(
            (4, 4),
            vec![
                ScriptedStep::Failed(FrameError::DeviceLost {
                    id: "adapter-0".to_string(),
                }),
                ScriptedStep::Failed(FrameError::Transient {
                    context: "poll_frame",
                    detail: "one attempt".to_string(),
                }),
                ScriptedStep::Frame {
                    pixels: Some(solid((4, 4), 3)),
                    size: None,
                },
            ],
        );
        assert!(
            matches!(
                source.next(Duration::from_millis(50)),
                Err(FrameError::DeviceLost { .. })
            ),
            "a lost device must be reported as an error so the caller can decide"
        );
        assert!(
            matches!(
                source.next(Duration::from_millis(50)),
                Err(FrameError::Transient { .. })
            ),
            "a transient failure must stay an error, not become an ending"
        );
        assert!(matches!(
            source.next(Duration::from_millis(50)),
            Ok(Poll::Frame(_))
        ));
    }

    #[test]
    fn a_new_frame_arrives_as_an_observation() {
        let size = (8, 4);
        let mut source = source(
            size,
            vec![ScriptedStep::Frame {
                pixels: Some(solid(size, 42)),
                size: None,
            }],
        );
        let answer = source.next(Duration::from_millis(50));
        let Ok(Poll::Frame(observation)) = answer else {
            panic!("a delivered frame must become an observation: {answer:?}");
        };
        assert_eq!(observation.size(), size);
        assert_eq!(observation.axis(), Axis::Vertical);
        assert_eq!(observation.region(), Rect::new(0, 0, size.0 as i32, size.1 as i32));
        assert_eq!(observation.pixels(), solid(size, 42).as_slice());
        assert_eq!(observation.row(2), &solid(size, 42)[2 * 32..3 * 32]);
        assert_eq!(observation.qpc(), 1);
    }

    #[test]
    fn a_repeated_frame_is_idle_because_nothing_changed() {
        let size = (6, 3);
        let mut source = source(
            size,
            vec![
                ScriptedStep::Frame {
                    pixels: Some(solid(size, 10)),
                    size: None,
                },
                repeated(),
            ],
        );
        assert!(matches!(
            source.next(Duration::from_millis(50)),
            Ok(Poll::Frame(_))
        ));
        assert!(
            matches!(source.next(Duration::from_millis(20)), Ok(Poll::Idle)),
            "the same bytes twice is Idle, not a second observation"
        );
    }

    #[test]
    fn a_resize_ends_the_stream_instead_of_moving_the_viewport() {
        let mut source = source((4, 4), vec![resized((6, 6), 1)]);
        let answer = source.next(Duration::from_millis(50));
        assert!(
            matches!(answer, Ok(Poll::Ended(EndReason::TargetLost))),
            "a session's viewport is invariant, so a resize ends it: {answer:?}"
        );
        let detail = source.last_detail().unwrap_or_default();
        assert!(
            detail.contains("resiz"),
            "the detail must say what happened: {detail}"
        );
    }

    #[test]
    fn an_ending_is_final_and_is_answered_without_polling_again() {
        let mut source = source(
            (4, 4),
            vec![ScriptedStep::Ended(EndReason::CaptureFailed)],
        );
        assert!(matches!(
            source.next(Duration::from_millis(50)),
            Ok(Poll::Ended(EndReason::CaptureFailed))
        ));
        assert!(matches!(
            source.next(Duration::from_millis(50)),
            Ok(Poll::Ended(_))
        ));
    }

    // --- three ways a target ends, and one word for all of them (§11.1, §24.4; task P2.07) ---
    //
    // A closed window and a minimised window both stop producing frames. So does a page that
    // is simply not moving — and today `WgcSession::next_frame` only asks the frame pool, so
    // all three look like `Ok(None)` and become `Idle` forever. That is the gap: "no frames"
    // is not evidence about *why*, so the source has to ask the window, not infer from silence.
    //
    // The three facts stay three facts in `detail`, and stay one `EndReason` in the vocabulary
    // (§20.4 keeps eleven `StopReason` variants; none of them is `Resized` or `DisplayChanged`,
    // because to the user and to the code path they behave identically — §24.4).

    #[test]
    fn closed_minimised_and_resized_are_three_distinct_endings() {
        let size = (8, 4);

        let (mut closed, closed_polls) = gone_source(size, TargetLiveness::Closed);
        let closed_answer = closed.next(Duration::from_millis(50));
        assert!(
            matches!(closed_answer, Ok(Poll::Ended(EndReason::TargetLost))),
            "a closed window must end the stream as TargetLost, not as an error: {closed_answer:?}"
        );
        assert_eq!(
            closed_polls.load(Ordering::SeqCst),
            0,
            "a closed window is a fact about the window, not about the frame pool: asking the \
             pool first would make a close wait out the timeout and then report Idle"
        );
        let closed_detail = closed.last_detail().expect("an ending carries its words");

        let (mut minimised, minimised_polls) = gone_source(size, TargetLiveness::Minimised);
        let minimised_answer = minimised.next(Duration::from_millis(50));
        assert!(
            matches!(minimised_answer, Ok(Poll::Ended(EndReason::TargetLost))),
            "a minimised window must end the stream as TargetLost: {minimised_answer:?}"
        );
        assert_eq!(
            minimised_polls.load(Ordering::SeqCst),
            0,
            "minimisation is asked about, not waited for (§24.4 row 5)"
        );
        let minimised_detail = minimised.last_detail().expect("an ending carries its words");

        let mut resized_source = source(size, vec![resized((16, 4), 200)]);
        let resized_answer = resized_source.next(Duration::from_millis(50));
        assert!(
            matches!(resized_answer, Ok(Poll::Ended(EndReason::TargetLost))),
            "a resize must end the stream as TargetLost: {resized_answer:?}"
        );
        let resized_detail = resized_source
            .last_detail()
            .expect("an ending carries its words");

        assert!(
            closed_detail.contains("gone"),
            "the closed detail must say the window is gone: {closed_detail}"
        );
        assert!(
            minimised_detail.contains("minimis"),
            "the minimised detail must say minimised: {minimised_detail}"
        );
        assert!(
            resized_detail.contains("resiz"),
            "the resized detail must say resized: {resized_detail}"
        );
        assert_ne!(closed_detail, minimised_detail);
        assert_ne!(closed_detail, resized_detail);
        assert_ne!(minimised_detail, resized_detail);
    }

    #[test]
    fn the_ending_vocabulary_stays_four() {
        // An exhaustive match with no `_` arm: adding an `EndReason` variant breaks this
        // build, which is what keeps §20.4's word list from growing one "clearer name" at a
        // time. The three endings above are distinguished by `detail`, not by a variant.
        fn name(reason: EndReason) -> &'static str {
            match reason {
                EndReason::TargetLost => "target-lost",
                EndReason::CaptureFailed => "capture-failed",
                EndReason::DeviceLost => "device-lost",
                EndReason::Timeout => "timeout",
            }
        }
        assert_eq!(name(EndReason::TargetLost), "target-lost");
        assert_eq!(name(EndReason::CaptureFailed), "capture-failed");
        assert_eq!(name(EndReason::DeviceLost), "device-lost");
        assert_eq!(name(EndReason::Timeout), "timeout");
    }

    // --- display topology and window changes (§24.4; task P2.05) ---
    //
    // The five rows of §24.4's table, as five cases through one function. The function
    // deliberately never sees *which message* arrived: it compares the target's own facts
    // before and after. That is the whole design — "was this change about the target?" is
    // answered by looking at the target, not by classifying the message, because the same
    // `WM_DISPLAYCHANGE` can be either.
    //
    // Rows 2 and 4 collapse into the same computation: neither is representable as a change
    // to the target's own facts. That collapse *is* the fix (§24.4: today's code cancels the
    // session for both), so the last test below is a compile-time pin rather than a value.

    fn geometry(dpi: u32, size: (u32, u32), minimised: bool) -> TargetGeometry {
        TargetGeometry {
            dpi,
            size,
            minimised,
        }
    }

    #[test]
    fn a_dpi_change_on_the_targets_monitor_stops_the_session() {
        let before = geometry(144, (1280, 900), false);
        let after = geometry(192, (1280, 900), false);
        assert!(matches!(
            topology_outcome(&before, &after),
            TopologyOutcome::Stop(EndReason::TargetLost)
        ));
    }

    #[test]
    fn a_topology_change_on_another_monitor_leaves_the_session_running() {
        // The caller re-reads the target after any `WM_DISPLAYCHANGE`/`WM_DEVICECHANGE`; when
        // the change was on another monitor the facts come back identical, so there is
        // nothing here to stop for. Another monitor is not represented in `TargetGeometry`
        // because it cannot invalidate this canvas.
        for before in [
            geometry(96, (640, 480), false),
            geometry(144, (1280, 900), false),
            geometry(192, (2560, 1440), false),
        ] {
            let after = before;
            assert!(
                matches!(topology_outcome(&before, &after), TopologyOutcome::Continue),
                "a change that leaves the target at {before:?} must not stop the session"
            );
        }
    }

    #[test]
    fn a_target_resize_stops_the_session() {
        let before = geometry(144, (1280, 900), false);
        let after = geometry(144, (1280, 900), false);
        // Resizing in either direction reflows the page, so the two canvases are not
        // comparable and the rows already written are not extendable.
        for size in [(1280, 901), (1279, 900), (1920, 1080)] {
            let resized = TargetGeometry { size, ..after };
            assert!(
                matches!(
                    topology_outcome(&before, &resized),
                    TopologyOutcome::Stop(EndReason::TargetLost)
                ),
                "resizing to {size:?} must stop the session"
            );
        }
    }

    #[test]
    fn the_target_moving_to_another_monitor_with_the_same_dpi_keeps_running() {
        // A move changes the window's *position* and which monitor it is on. Neither is a
        // field of `TargetGeometry`, and the same DPI means the physical-pixel scale — the
        // thing the canvas coordinate system is made of — is unchanged. The pin below keeps
        // it that way; this test states the consequence.
        let before = geometry(144, (1280, 900), false);
        let after = geometry(144, (1280, 900), false);
        assert!(matches!(
            topology_outcome(&before, &after),
            TopologyOutcome::Continue
        ));
    }

    #[test]
    fn minimising_the_target_stops_the_session() {
        let before = geometry(144, (1280, 900), false);
        let after = geometry(144, (1280, 900), true);
        assert!(matches!(
            topology_outcome(&before, &after),
            TopologyOutcome::Stop(EndReason::TargetLost)
        ));
        // Minimising also wins over a simultaneous resize: the answer is the same either
        // way, and the caller's `Partial` export does not depend on which one it was.
        let resized = TargetGeometry {
            size: (1920, 1080),
            ..after
        };
        assert!(matches!(
            topology_outcome(&before, &resized),
            TopologyOutcome::Stop(EndReason::TargetLost)
        ));
    }

    #[test]
    fn the_target_geometry_holds_only_what_can_invalidate_the_canvas() {
        // Exactly three fields and no `..`: adding a position or a monitor identity makes
        // this fail to compile, which is the point. Rows 2 and 4 of §24.4 are only "continue"
        // because nothing that changed is carried here — a window-level capture takes the
        // window's content, so where the window is cannot invalidate the canvas.
        let TargetGeometry {
            dpi,
            size,
            minimised,
        } = geometry(144, (1280, 900), false);
        let _: u32 = dpi;
        let _: (u32, u32) = size;
        let _: bool = minimised;
    }

    // --- backend policy: window first, monitor as the fallback (§11.5; task P2.06) ---

    #[test]
    fn the_window_backend_is_preferred_and_the_monitor_backend_is_the_fallback() {
        // The order *is* the policy, so this asserts the list rather than the mechanism.
        assert_eq!(
            SCROLL_BACKENDS,
            [ProviderKind::WgcWindow, ProviderKind::Wgc],
            "a window-level capture cannot contain the overlay or an occluder (§24.2), so \
             the window backend is the first entry, not the one we retreat to"
        );
        assert_eq!(
            next_scroll_backend(ProviderKind::WgcWindow),
            Some(ProviderKind::Wgc)
        );
        assert_eq!(next_scroll_backend(ProviderKind::Wgc), None);
        // BitBlt is the *ordinary* path's final fallback. It is deliberately absent here:
        // reading the desktop when the target window refuses to be captured would hand back
        // a frame that is not the target's, silently (`providers.rs` `attempt_order`).
        assert!(
            !SCROLL_BACKENDS.contains(&ProviderKind::BitBlt),
            "the scroll path must not fall back to a frame that is not the target window's"
        );
        assert_eq!(next_scroll_backend(ProviderKind::BitBlt), None);
    }

    #[test]
    fn a_fallback_records_a_diagnostic() {
        let fallback = BackendFallback {
            from: ProviderKind::WgcWindow,
            to: ProviderKind::Wgc,
            reason: "CreateForWindow was refused (0x80070057)".to_string(),
        };
        let line = fallback.diagnostic();
        // Both ends and the reason, because "we fell back" without "from what" or "why" is
        // not a diagnostic — that is the Snow Shot counter-example §11.5 cites.
        assert!(line.contains("wgc-window"), "the line must name the backend it left: {line}");
        assert!(line.contains("wgc"), "the line must name the backend it moved to: {line}");
        assert!(line.contains("0x80070057"), "the line must carry the reason: {line}");
        let other = BackendFallback {
            reason: "the first frame timed out".to_string(),
            ..fallback
        };
        assert_ne!(
            line,
            other.diagnostic(),
            "the diagnostic must say why, not just that it happened"
        );
    }

    // --- the source is testable without a desktop, and stays off the context (§29.2, §21.3;
    //     task P2.08) ---
    //
    // `ScriptedBackend` tests the *source*; this tests the *consumer*. The loop that will read
    // from a `FrameSource` is `P3.09`'s, so what can be proven here is the seam: everything a
    // consumer is allowed to rely on is reachable through the trait object, with no device and
    // no window. That is G9, and it is what makes the P3 loop's decisions testable at all.

    /// The shape a consumer is allowed to branch on, as a value.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Decision {
        Frame,
        Idle,
        Ended,
        Failed,
    }

    /// Runs a consumer-shaped loop over any `FrameSource`. Deliberately takes the trait object:
    /// if a consumer needs something the trait does not have, this function cannot be written.
    fn drain(source: &mut dyn FrameSource, budget: usize, timeout: Duration) -> Vec<Decision> {
        let mut seen = Vec::new();
        for _ in 0..budget {
            seen.push(match source.next(timeout) {
                Ok(Poll::Frame(_)) => Decision::Frame,
                Ok(Poll::Idle) => Decision::Idle,
                Ok(Poll::Ended(_)) => Decision::Ended,
                Err(_) => Decision::Failed,
            });
        }
        seen
    }

    /// A scripted [`FrameSource`] for consumer-side tests (`docs/30` §29.4's D class).
    ///
    /// It is the seam that makes the loop testable: `P3.09`'s loop will take a `FrameSource`,
    /// and this is the one that answers without a device, a window or a desktop. It is *not*
    /// `ScriptedBackend` renamed — that one tests the source's own rules, this one exists so a
    /// consumer's rules can be tested at all.
    struct MockFrameSource {
        viewport: Rect,
        script: VecDeque<MockStep>,
        /// Once an ending is handed out it holds, matching the real source's finality.
        ended: Option<EndReason>,
        asked: u32,
        axis: Axis,
    }

    enum MockStep {
        /// A frame whose bytes are all `level`, so two frames are tellable apart.
        Frame { size: (u32, u32), level: u8 },
        Idle,
        Ended(EndReason),
        Failed(FrameError),
    }

    impl MockFrameSource {
        fn new(viewport: Rect) -> Self {
            Self {
                viewport,
                script: VecDeque::new(),
                ended: None,
                asked: 0,
                axis: Axis::Vertical,
            }
        }

        fn then(mut self, step: MockStep) -> Self {
            self.script.push_back(step);
            self
        }
    }

    impl FrameSource for MockFrameSource {
        fn next(&mut self, _timeout: Duration) -> Result<Poll, FrameError> {
            self.asked += 1;
            if let Some(reason) = self.ended {
                return Ok(Poll::Ended(reason));
            }
            match self.script.pop_front() {
                Some(MockStep::Frame { size, level }) => {
                    let pixels = vec![level; size.0 as usize * size.1 as usize * 4];
                    let region = Rect::new(0, 0, size.0 as i32, size.1 as i32);
                    let observation =
                        Observation::new(pixels, region, self.asked as i64, size, self.axis)
                            .expect("the mock builds the region from its own size");
                    Ok(Poll::Frame(observation))
                }
                Some(MockStep::Idle) => Ok(Poll::Idle),
                Some(MockStep::Ended(reason)) => {
                    self.ended = Some(reason);
                    Ok(Poll::Ended(reason))
                }
                Some(MockStep::Failed(error)) => Err(error),
                // A script that ran out is a source with nothing new, which is what a real
                // source does when the page stops moving — not a panic and not an ending.
                None => Ok(Poll::Idle),
            }
        }

        fn viewport(&self) -> Rect {
            self.viewport
        }
    }

    /// Splits a file that scans itself into "what ships" and "what tests it".
    ///
    /// A self-scanning test has to say which half it reads: the first version of these two
    /// tests scanned the whole file, and so matched the needles written *in their own bodies*
    /// (`.context()` inside the list of needles, `env::var` inside the assertion that counts
    /// it). Both failures were real — a scan that cannot tell code from the code that checks
    /// it would report a violation the moment anyone wrote a test about one.
    fn halves(source: &str) -> (&str, &str) {
        let cut = source
            .find("#[cfg(test)]")
            .expect("this file has a test module; a scan without one is a scan of nothing");
        (&source[..cut], &source[cut..])
    }

    #[test]
    fn the_scroll_source_compiles_and_its_logic_tests_run_without_a_desktop() {
        let viewport = Rect::new(0, 0, 8, 4);
        let mut source = MockFrameSource::new(viewport)
            .then(MockStep::Frame {
                size: (8, 4),
                level: 10,
            })
            .then(MockStep::Idle)
            .then(MockStep::Frame {
                size: (8, 4),
                level: 20,
            })
            .then(MockStep::Ended(EndReason::TargetLost));

        assert_eq!(source.viewport(), viewport);

        let seen = drain(&mut source, 6, Duration::from_millis(20));
        assert_eq!(
            seen,
            vec![
                Decision::Frame,
                Decision::Idle,
                Decision::Frame,
                Decision::Ended,
                // An ending is final, and the mock must not pretend otherwise: a consumer that
                // keeps asking after a stop must not see frames appear again.
                Decision::Ended,
                Decision::Ended,
            ],
            "a consumer must see exactly the script, with the ending holding"
        );
    }

    #[test]
    fn no_real_desktop_test_silently_skips_in_this_module() {
        const SOURCE: &str = include_str!("scroll_source.rs");
        let (production, tests) = halves(SOURCE);

        // D-14: a test either runs everywhere or is `#[ignore]`d with a reason. The mechanical
        // form of "does not decide whether to run by looking at the environment" is that this
        // file never reads the environment at all.
        assert_eq!(
            production.matches("env::var").count(),
            0,
            "this module must not decide whether to run by reading the environment (D-14); the \
             frame source needs no desktop, so nothing here needs a conditional"
        );

        // Line-anchored, not substring: the first version matched the phrase `#[ignore]`d with a
        // reason` in the comment right above it. An attribute is a line; prose about one is not.
        let ignores: Vec<&str> = tests
            .lines()
            .filter(|line| line.trim_start().starts_with("#[ignore"))
            .collect();
        for line in &ignores {
            assert!(
                line.trim_start().starts_with("#[ignore = "),
                "an #[ignore] in this module must carry a reason (D-14): {line}"
            );
        }
        assert!(
            ignores.is_empty(),
            "every test in this module runs everywhere — that is the whole claim of G9 — so a \
             new #[ignore] here is a regression, not a convenience ({} found)",
            ignores.len()
        );
    }

    #[test]
    fn this_module_never_becomes_a_context_user() {
        const SOURCE: &str = include_str!("scroll_source.rs");
        let (production, _) = halves(SOURCE);

        // `T-THREAD-1` (P0.02, docs/30 §21.3) found that "the creating thread is the only user"
        // is false in this codebase: the device is created on the capture worker and used by the
        // overlay and the export path, and the real protocol is "one user at a time, handed
        // over". The scroll driver is not a user at all — §11.2 routes reads through the capture
        // worker — so this file must never reach the immediate context itself. It reaches
        // `providers::ScrollFrame::read_region`, which is the boundary, and that is the point.
        for needle in [
            ".context()",
            "GraphicsDevice::create",
            "read_back_bgra",
            "read_back_region_bgra",
        ] {
            assert!(
                !production.contains(needle),
                "scroll_source.rs must not reach the immediate context directly ({needle}): \
                 T-THREAD-1 (docs/30 §21.3) says the scroll path is not a context user, and \
                 §11.2 is what keeps it that way"
            );
        }
    }
}

