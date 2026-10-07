//! v2 refinement worker: runs accessibility deep-selection queries off the UI thread
//! (docs/18 §2, §5).
//!
//! The UIA/MSAA providers are apartment-bound and can block for hundreds of milliseconds;
//! running them on the overlay thread would freeze the message loop, and running them on
//! the v1 detection worker would let a slow accessibility provider stall window snapping.
//! They therefore get their own thread, their own capacity-1 mailbox, and their own
//! request gate.
//!
//! Cancellation is **cooperative**: a COM call cannot be interrupted from outside, so the
//! provider polls [`QueryControl::is_cancelled`] between nodes. The gate is checked from
//! this object and read by the worker thread, which is what makes "the cursor moved, drop
//! what you are doing" observable inside a long traversal.
//!
//! Two invariants this module owes the overlay:
//!
//! * **One id space.** The request id belongs to the scheduler, which is the side that decides
//!   whether an answer is still the current question; the worker *adopts* it rather than
//!   minting a second id. Two counters matched only while both happened to start at 1, so from
//!   the second capture session on every result was rejected as stale and deep selection died
//!   silently (docs/18 §2).
//! * **Every job the worker starts reports back exactly once**, tagged with its own id — even
//!   when a newer point superseded it while it ran. A silently dropped job leaves the overlay's
//!   single-flight slot held with nothing to release it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use ::windows::Win32::Foundation::{LPARAM, WPARAM};
use ::windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP};

use crate::diagnostics::WindowDetectionMetrics;
use crate::geometry::{Point, Rect};
use crate::window_detection::deep::{
    DeepSelectionProvider, DeepTarget, QueryControl, RefinementJob, RefinementOutcome, StopReason,
};
use crate::window_detection::model::{
    PathLevel, RequestGate, RequestId, SnapshotEpoch, TargetKind, WindowIdentity,
};
use crate::window_detection::uia::{
    MAX_PATH_LEN, level_kind_of_msaa_role, should_adopt_msaa_box,
};

use super::msaa_provider::{MsaaDeepSelectionProvider, MsaaHitBox, MsaaHitFailure};
use super::uia_provider::UiaDeepSelectionProvider;
use super::win::window as win32;

/// UIA for the tree walk, MSAA for a second opinion on the point (docs/18 §12.3, docs/21 §5.16).
///
/// The division of labour is measured, not assumed. UIA owns the path/level work and the page's
/// exposed structure; MSAA's `accHitTest` goes through Chromium's *renderer* hit test
/// (`BrowserAccessibilityWin::accHitTest` -> `CachingAsyncHitTest`), which is the only way to see
/// the layout-only boxes UIA never exposes — six fixture points that used to answer the whole page
/// come back with their exact element boxes at ~1 ms (docs/21 §5.11). So MSAA is asked whenever UIA
/// produced a target, and its box is adopted only by the same strict-refinement rule the UIA
/// candidate goes through; when UIA has nothing at all for the window, MSAA is the whole answer,
/// exactly as before.
pub(crate) struct FallbackDeepSelection {
    uia: UiaDeepSelectionProvider,
    msaa: MsaaDeepSelectionProvider,
    metrics: WindowDetectionMetrics,
    /// Whether a bare text run may become the answer (docs/21 §5.19); both transports share it.
    adopt_text_runs: bool,
}

impl FallbackDeepSelection {
    pub(crate) fn new(
        metrics: WindowDetectionMetrics,
        pass_through: win32::HitTestPassThrough,
        adopt_text_runs: bool,
    ) -> Self {
        Self {
            uia: UiaDeepSelectionProvider::new(metrics.clone())
                .with_hit_test_pass_through(pass_through)
                .with_adopt_text_runs(adopt_text_runs),
            msaa: MsaaDeepSelectionProvider::new(metrics.clone()),
            metrics,
            adopt_text_runs,
        }
    }

    /// Fold the MSAA hit into the published target, and report the query's **one** decision.
    ///
    /// One line per query, not one per transport: the forced forensics line has to answer "what did
    /// this query look at, and what did it publish", and two counters per query would make the
    /// session summary unreadable. The note carries both transports' raw facts, including which one
    /// the box came from.
    fn adopt_msaa_hit(
        &self,
        target: &mut DeepTarget,
        hit: Option<&Result<MsaaHitBox, MsaaHitFailure>>,
        uia: Option<&super::uia_provider::HitDecision>,
        job: &RefinementJob,
    ) -> (crate::diagnostics::PrecisionOutcome, String) {
        use crate::diagnostics::PrecisionOutcome;
        let walk = target.screen_bounds;
        let mut note = format!(
            "walk={}x{} at ({},{})",
            walk.width(),
            walk.height(),
            walk.left,
            walk.top
        );
        let uia_adopted = match uia {
            Some(decision) => {
                note.push_str(&format!(
                    " uia=[{}{} {}]",
                    decision_name(decision.outcome),
                    if decision.answered_specifically {
                        ""
                    } else {
                        " unspecific"
                    },
                    decision.note
                ));
                decision.outcome == PrecisionOutcome::Adopted
            }
            None => false,
        };
        let (msaa_usable, msaa_adopted) = match hit {
            // **When is a second opinion wanted?** Not whenever MSAA happens to be finer — that would
            // let a button's *label* outrank the button, because the label is always smaller. MSAA is
            // asked when UIA did not get below the page (a document or window-sized answer, or no
            // answer at all); a transport that named a real element is the platform's own notion of
            // the control and outranks a smaller box inside it (docs/21 §5.19).
            None => {
                note.push_str(" msaa=[not consulted: uia answered a specific element]");
                (false, false)
            }
            Some(Ok(hit)) => {
                let finer = should_adopt_msaa_box(
                    walk,
                    hit.visible,
                    hit.role,
                    hit.parent_role,
                    job.point,
                    self.adopt_text_runs,
                );
                let adopted = finer
                    && push_box(
                        target,
                        PathLevel::new(hit.visible, level_kind_of_msaa_role(hit.role)),
                        &hit.ancestors,
                    );
                note.push_str(&format!(
                    " msaa=[role=0x{:x} name={:?} depth={} {}{}]",
                    hit.role,
                    hit.name.chars().take(24).collect::<String>(),
                    hit.depth,
                    describe_box(hit.raw, hit.visible),
                    if adopted {
                        " ADOPTED"
                    } else if finer {
                        " could not be appended to the path"
                    } else {
                        ""
                    }
                ));
                (true, adopted)
            }
            Some(Err(failure)) => {
                note.push_str(&format!(" msaa=[{}]", failure_name(*failure)));
                (false, false)
            }
        };
        let outcome = if msaa_adopted || uia_adopted {
            PrecisionOutcome::Adopted
        } else if msaa_usable || uia.is_some_and(|decision| decision.outcome != PrecisionOutcome::Unavailable) {
            PrecisionOutcome::NotFiner
        } else {
            PrecisionOutcome::Unavailable
        };
        (outcome, note)
    }
}

/// `adopted` / `not-finer` / `unavailable`, spelled the way the forced line does.
fn decision_name(outcome: crate::diagnostics::PrecisionOutcome) -> &'static str {
    use crate::diagnostics::PrecisionOutcome;
    match outcome {
        PrecisionOutcome::Adopted => "adopted",
        PrecisionOutcome::NotFiner => "not-finer",
        PrecisionOutcome::Unavailable => "unavailable",
    }
}

/// `WxH at (x,y)`, with the visible part when the ancestors trimmed the box.
fn describe_box(raw: Rect, visible: Rect) -> String {
    if raw == visible {
        format!("{}x{} at ({},{})", raw.width(), raw.height(), raw.left, raw.top)
    } else {
        format!(
            "{}x{} at ({},{}) visible={}x{} at ({},{})",
            raw.width(),
            raw.height(),
            raw.left,
            raw.top,
            visible.width(),
            visible.height(),
            visible.left,
            visible.top
        )
    }
}

/// Why MSAA had no box, spelled the way the forced line does.
fn failure_name(failure: MsaaHitFailure) -> &'static str {
    match failure {
        MsaaHitFailure::Quarantined => "quarantined",
        MsaaHitFailure::Busy => "busy",
        MsaaHitFailure::TimedOut => "timeout",
        MsaaHitFailure::Unavailable => "unavailable",
    }
}

/// Publish `bounds` together with **its own** ancestor chain (docs/21 §5.17).
///
/// The box comes from a transport's hit test, so the chain that has to travel with it is the one that
/// transport reported — the walk's levels are another view of the page and are not necessarily above
/// this box. Only levels that actually contain the box survive, which is what makes the published
/// path a containment chain the ancestor walk can step through.
fn push_box(target: &mut DeepTarget, box_level: PathLevel, ancestors: &[Rect]) -> bool {
    let bounds = box_level.rect;
    // The ancestors are rectangles off MSAA's own chain: they say where the box sits, not what it
    // sits in, so they publish as `Unknown` and the label keeps its generic words for them.
    let mut path: Vec<PathLevel> = ancestors
        .iter()
        .copied()
        .filter(|level| level.contains_rect(bounds))
        .map(PathLevel::unknown)
        .collect();
    if path.is_empty() {
        // The chain always starts at the window frame; keep it if a caller ever gets here without one.
        if let Some(frame) = target.path.first().copied() {
            path.push(frame);
        }
    }
    if path.len() >= MAX_PATH_LEN {
        return false;
    }
    if path.last().map(|level| level.rect) != Some(bounds) {
        path.push(box_level);
    }
    target.path = path;
    target.screen_bounds = bounds;
    target.kind = TargetKind::UiElement;
    true
}

impl DeepSelectionProvider for FallbackDeepSelection {
    fn resolve(
        &mut self,
        job: &RefinementJob,
        window_bounds: Rect,
        control: &QueryControl<'_>,
    ) -> RefinementOutcome {
        let outcome = self.uia.resolve(job, window_bounds, control);
        // Whatever the hit test decided belongs to *this* query only.
        let uia = self.uia.take_hit_decision();
        let RefinementOutcome::Target(mut target) = outcome else {
            return match outcome {
                // UIA has nothing for this window at all: MSAA is the whole answer, as before.
                RefinementOutcome::Empty(StopReason::Unsupported) => {
                    self.msaa.resolve(job, window_bounds, control)
                }
                other => other,
            };
        };
        // MSAA is always asked, because it is the authoritative hit test: measured on the fixture,
        // UIA's `ElementFromPoint` answered the *same point* with three different elements across
        // three queries (the text run, then the outer box, then the middle one) — it is Chromium's
        // browser-side approximation, while `accHitTest` runs the renderer's own hit test (docs/21
        // §5.19). UIA's own candidate is still adopted first when it happens to be finer, and by the
        // same rule it can never publish something coarser.
        let hit = Some(self.msaa.hit(job.window.hwnd, job.point, window_bounds));
        let (decision, note) = self.adopt_msaa_hit(&mut target, hit.as_ref(), uia.as_ref(), job);
        self.metrics.record_precision(decision, &note);
        RefinementOutcome::Target(target)
    }

    fn release(&mut self) {
        self.uia.release();
        self.msaa.release();
    }
}

/// Creates the provider **on the refinement thread**.
///
/// A value could not be moved across: COM interfaces are not `Send`, and the design requires
/// apartment-bound objects to be born on the thread that uses them (docs/18 §5).
pub type ProviderFactory = Box<dyn FnOnce() -> Box<dyn DeepSelectionProvider> + Send>;

/// Posted to the overlay thread when a refinement result is waiting.
pub const REFINEMENT_READY_MESSAGE: u32 = WM_APP + 44;

/// One query handed to the worker.
#[derive(Debug, Clone, Copy)]
struct Job {
    request: RequestId,
    epoch: SnapshotEpoch,
    window: WindowIdentity,
    point: Point,
    window_bounds: Rect,
}

/// A finished query.
#[derive(Debug)]
pub struct RefinementResult {
    pub request: RequestId,
    pub epoch: SnapshotEpoch,
    pub outcome: RefinementOutcome,
    /// How long the provider call took. Reported on the session summary so "deep selection
    /// never triggers" can be told apart from "it triggers, but slowly".
    pub elapsed: std::time::Duration,
}

struct Shared {
    notify_thread: u32,
    pending: Mutex<Option<Job>>,
    wake: Condvar,
    result: Mutex<Option<RefinementResult>>,
    /// Request ids are issued by the handle and read by the worker for cancellation, so the
    /// gate is shared rather than owned by either side.
    requests: Mutex<RequestGate>,
    /// Queries dropped because a newer point replaced them (diagnostics only).
    cancelled: AtomicU64,
    /// Explicit shutdown flag.
    ///
    /// The gate alone cannot express "stop": before the first request is issued it has no
    /// latest id either, so inferring shutdown from it made the worker exit at startup and
    /// every submitted query was silently never run.
    shutdown: std::sync::atomic::AtomicBool,
}

/// Owns the refinement thread and the accessibility provider.
pub struct RefinementWorker {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
    metrics: WindowDetectionMetrics,
}

impl RefinementWorker {
    /// Start the worker with the real UIA provider.
    ///
    /// If UI Automation is unavailable the provider reports `Unsupported` per query and the
    /// overlay keeps the v1 whole-window frame.
    /// `pass_through` is the capture overlay's hit-test flag: the UIA point hit test has to fall
    /// through the overlay to see the application underneath (docs/21 §5.7). `default()` is correct
    /// in tests and probes, where nothing of ours covers the desktop.
    ///
    /// `adopt_text_runs` is the product's answer to "may a bare text run be the target?" (docs/21
    /// §5.19) and applies to both transports, so the two do not disagree about it.
    pub fn new(
        notify_thread: u32,
        pass_through: win32::HitTestPassThrough,
        adopt_text_runs: bool,
        metrics: WindowDetectionMetrics,
    ) -> Self {
        let provider_metrics = metrics.clone();
        Self::with_provider(
            notify_thread,
            metrics,
            // The provider logs its per-level forensics through the same verbose gate as the
            // rest of window detection, so it is built on the refinement thread with the
            // shared metrics handle (docs/18 §12.7).
            Box::new(move || {
                Box::new(FallbackDeepSelection::new(
                    provider_metrics,
                    pass_through,
                    adopt_text_runs,
                ))
            }),
        )
    }

    /// Start the worker with an explicit provider factory (tests inject fakes this way).
    pub fn with_provider(
        notify_thread: u32,
        metrics: WindowDetectionMetrics,
        factory: ProviderFactory,
    ) -> Self {
        let shared = Arc::new(Shared {
            notify_thread,
            pending: Mutex::new(None),
            wake: Condvar::new(),
            result: Mutex::new(None),
            requests: Mutex::new(RequestGate::new()),
            cancelled: AtomicU64::new(0),
            shutdown: std::sync::atomic::AtomicBool::new(false),
        });
        let worker_shared = Arc::clone(&shared);
        let worker_metrics = metrics.clone();
        let thread = std::thread::Builder::new()
            .name("snapclip-refinement".into())
            .spawn(move || run(worker_shared, worker_metrics, factory))
            .ok();
        Self {
            shared,
            thread,
            metrics,
        }
    }

    /// Queue a query for the job the scheduler issued.
    ///
    /// The **caller's** request id is the one that travels back with the result
    /// ([`RefinementResult::request`]) and the one the scheduler compares against its own
    /// in-flight question, so the worker adopts it instead of minting a second id space. Two
    /// counters drifted apart as soon as one session ended, and the scheduler then rejected
    /// every later answer as stale — deep selection died silently after the first F5.
    pub fn request(&self, job: RefinementJob, window_bounds: Rect) {
        if let Ok(mut gate) = self.shared.requests.lock() {
            gate.adopt(job.request);
        }
        if let Ok(mut pending) = self.shared.pending.lock() {
            if pending.is_some() {
                self.shared.cancelled.fetch_add(1, Ordering::Relaxed);
            }
            *pending = Some(Job {
                request: job.request,
                epoch: job.epoch,
                window: job.window,
                point: job.point,
                window_bounds,
            });
        }
        self.shared.wake.notify_all();
    }

    /// Take the pending result, if the worker produced one.
    pub fn take_result(&self) -> Option<RefinementResult> {
        self.shared.result.lock().ok()?.take()
    }

    /// Abandon the current query.
    ///
    /// The gate is retired, so the running provider observes `cancelled` on its next poll
    /// and returns a partial/empty result, which is then dropped instead of published.
    pub fn retire(&self) {
        if let Ok(mut gate) = self.shared.requests.lock() {
            gate.retire();
        }
    }

    /// How many queued queries were replaced before they ran (diagnostics).
    pub fn cancelled_queued(&self) -> u64 {
        self.shared.cancelled.load(Ordering::Relaxed)
    }

    /// Stop the worker. Idempotent; a call already inside the provider finishes on its own
    /// thread while this waits, or observes cancellation and returns early.
    pub fn shutdown(&mut self) {
        self.shared
            .shutdown
            .store(true, std::sync::atomic::Ordering::SeqCst);
        // Retire the gate first so an in-flight provider sees `cancelled` and bails out.
        self.retire();
        if let Ok(mut pending) = self.shared.pending.lock() {
            *pending = None;
        }
        self.shared.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.metrics.log_line(
            &format!(
                "refinement stopped cancelled_queued={}",
                self.cancelled_queued()
            ),
            false,
        );
    }
}

impl Drop for RefinementWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run(
    shared: Arc<Shared>,
    metrics: WindowDetectionMetrics,
    factory: ProviderFactory,
) {
    // Born here, on the thread that will use it: this is where COM gets initialised.
    let mut provider = factory();
    loop {
        let job = {
            let mut pending = match shared.pending.lock() {
                Ok(pending) => pending,
                Err(_) => return,
            };
            while pending.is_none() {
                if shared.shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                    provider.release();
                    return;
                }
                pending = match shared.wake.wait_timeout(pending, std::time::Duration::from_millis(50))
                {
                    Ok((guard, _)) => guard,
                    Err(_) => return,
                };
            }
            pending.take().expect("the loop only exits with a job in hand")
        };

        // A query that was superseded before it started is dropped, not run: the whole
        // point of the capacity-1 mailbox.
        let is_current = |request: RequestId| {
            shared
                .requests
                .lock()
                .map(|gate| gate.accepts(request))
                .unwrap_or(false)
        };
        if !is_current(job.request) {
            continue;
        }

        let started = Instant::now();
        // The closure must outlive the control, so it is bound before it.
        let cancelled = || !is_current(job.request);
        let control = QueryControl::refinement(&cancelled);
        let provider_job = RefinementJob {
            request: job.request,
            window: job.window,
            epoch: job.epoch,
            point: job.point,
        };
        let outcome = provider.resolve(&provider_job, job.window_bounds, &control);
        metrics.log_line(
            &format!(
                "refinement hwnd={} point=({},{}) elapsed_us={} reason={:?}",
                job.window.hwnd,
                job.point.x,
                job.point.y,
                started.elapsed().as_micros(),
                match &outcome {
                    RefinementOutcome::Target(target) => target.stop_reason,
                    RefinementOutcome::Empty(reason) => *reason,
                }
            ),
            false,
        );
        // A superseded query is never delivered: the overlay would drop it anyway, and
        // publishing it would evict the newest result from the single slot.
        // **Always** publish, even when the job was superseded while it ran. Dropping it silently
        // left the overlay's single-flight slot held forever (no result, no failure), so the next
        // dwell could never issue a query and refinement stayed dead for the rest of the session.
        // The overlay rejects a superseded result by request id, which is the right place for that
        // decision — it just has to *hear* about it.
        let outcome = if is_current(job.request) {
            outcome
        } else {
            RefinementOutcome::Empty(StopReason::Cancelled)
        };
        publish(&shared, job, outcome, started.elapsed());
    }
}

fn publish(
    shared: &Arc<Shared>,
    job: Job,
    outcome: RefinementOutcome,
    elapsed: std::time::Duration,
) {
    if let Ok(mut slot) = shared.result.lock() {
        *slot = Some(RefinementResult {
            request: job.request,
            epoch: job.epoch,
            outcome,
            elapsed,
        });
    }
    let _ = unsafe {
        PostThreadMessageW(
            shared.notify_thread,
            REFINEMENT_READY_MESSAGE,
            WPARAM(0),
            LPARAM(0),
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window_detection::deep::{DeepTarget, StopReason};
    use crate::window_detection::model::TargetKind;
    use crate::window_detection::RefinementScheduler;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::time::Duration;

    fn window() -> WindowIdentity {
        WindowIdentity::new(0x100, 4242, 0xC0FFEE)
    }

    fn deep(bounds: Rect, reason: StopReason) -> DeepTarget {
        DeepTarget {
            window: window(),
            kind: TargetKind::UiElement,
            screen_bounds: bounds,
            path: vec![
                PathLevel::new(Rect::new(0, 0, 1000, 800), crate::window_detection::LevelKind::Window),
                PathLevel::unknown(bounds),
            ],
            stop_reason: reason,
        }
    }

    /// A provider that cooperates with cancellation, like a real traversal must.
    struct SlowProvider {
        delay: Duration,
        result: Option<DeepTarget>,
        observed_cancel: Arc<AtomicBool>,
        calls: Arc<AtomicU32>,
    }

    impl DeepSelectionProvider for SlowProvider {
        fn resolve(
            &mut self,
            _job: &crate::window_detection::deep::RefinementJob,
            _window_bounds: Rect,
            control: &QueryControl<'_>,
        ) -> RefinementOutcome {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let deadline = Instant::now() + self.delay;
            while Instant::now() < deadline {
                if control.is_cancelled() {
                    self.observed_cancel.store(true, Ordering::SeqCst);
                    return RefinementOutcome::Empty(StopReason::Cancelled);
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            match self.result.take() {
                Some(target) => RefinementOutcome::Target(Box::new(target)),
                None => RefinementOutcome::Empty(StopReason::ProviderFailure),
            }
        }
    }

    fn wait_for(worker: &RefinementWorker, timeout: Duration) -> Option<RefinementResult> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(result) = worker.take_result() {
                return Some(result);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    /// A provider that answers every query with the same rectangle.
    struct FixedProvider {
        bounds: Rect,
    }

    impl DeepSelectionProvider for FixedProvider {
        fn resolve(
            &mut self,
            _job: &RefinementJob,
            _window_bounds: Rect,
            _control: &QueryControl<'_>,
        ) -> RefinementOutcome {
            RefinementOutcome::Target(Box::new(deep(self.bounds, StopReason::Complete)))
        }
    }

    /// A job tagged with a fresh request id, exactly as the scheduler hands it over.
    fn job_for(epoch: SnapshotEpoch, point: Point) -> RefinementJob {
        RefinementJob {
            request: RequestGate::new().issue(),
            window: window(),
            epoch,
            point,
        }
    }

    /// The first delivered outcome tagged with `request`.
    fn wait_for_request(
        worker: &RefinementWorker,
        request: RequestId,
        timeout: Duration,
    ) -> Option<RefinementResult> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(result) = worker.take_result()
                && result.request == request
            {
                return Some(result);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    /// The scheduler owns the refinement id space; the worker only consumes it.
    ///
    /// Regression: the worker used to issue ids from a *second* gate while the scheduler issued
    /// its own. Both counted from 1, so the first session matched by accident — and every later
    /// session was off by however many queries the first one ran, because `begin_window_detection`
    /// resets the scheduler's gate. The worker's result was then rejected as stale, the
    /// single-flight slot was never freed, and deep selection was dead for the rest of the
    /// session (`refinement_submitted=1 refinement_published=0`, "the second F5 only snaps the
    /// window").
    #[test]
    fn a_second_session_still_accepts_the_worker_result() {
        let worker = RefinementWorker::with_provider(
            candidate_thread(),
            WindowDetectionMetrics::new(),
            Box::new(|| {
                Box::new(FixedProvider {
                    bounds: Rect::new(10, 10, 60, 40),
                })
            }),
        );
        let mut scheduler = RefinementScheduler::new();

        // Session one.
        assert!(
            scheduler
                .on_cursor_moved(1, Some(window()), Point::new(20, 20))
                .arm_dwell
        );
        let job = scheduler.on_dwell_due().expect("session one issues a query");
        worker.request(job, Rect::new(0, 0, 100, 100));
        let result = wait_for(&worker, Duration::from_secs(5)).expect("session one answers");
        let RefinementOutcome::Target(target) = result.outcome else {
            panic!("the fixed provider always resolves");
        };
        assert!(
            scheduler.on_result(result.request, result.epoch, *target),
            "session one's answer is the current question"
        );

        // Session teardown and a second F5: both sides start over, and the *scheduler's* ids
        // restart while the worker's mailbox keeps running.
        scheduler.reset();
        worker.retire();

        assert!(
            scheduler
                .on_cursor_moved(2, Some(window()), Point::new(30, 30))
                .arm_dwell
        );
        let job = scheduler.on_dwell_due().expect("session two issues a query");
        worker.request(job, Rect::new(0, 0, 100, 100));
        let result = wait_for(&worker, Duration::from_secs(5)).expect("session two answers");
        let RefinementOutcome::Target(target) = result.outcome else {
            panic!("the fixed provider always resolves");
        };
        assert!(
            scheduler.on_result(result.request, result.epoch, *target),
            "the second session's answer must still be the current question"
        );
        assert!(
            scheduler.cached().is_some(),
            "the second session publishes the deep target"
        );
    }

    #[test]
    fn a_query_produces_a_target_tagged_with_its_request_and_epoch() {
        let observed = Arc::new(AtomicBool::new(false));
        let observed_for_provider = Arc::clone(&observed);
        let worker = RefinementWorker::with_provider(
            candidate_thread(),
            WindowDetectionMetrics::new(),
            Box::new(move || {
                Box::new(SlowProvider {
                    delay: Duration::from_millis(1),
                    result: Some(deep(Rect::new(10, 10, 60, 40), StopReason::Complete)),
                    observed_cancel: observed_for_provider,
                    calls: Arc::new(AtomicU32::new(0)),
                })
            }),
        );
        let job = job_for(7, Point::new(20, 20));
        let request = job.request;
        worker.request(job, Rect::new(0, 0, 100, 100));
        let result = wait_for(&worker, Duration::from_secs(5)).expect("a result");
        assert_eq!(result.request, request);
        assert_eq!(result.epoch, 7);
        match result.outcome {
            RefinementOutcome::Target(target) => {
                assert_eq!(target.screen_bounds, Rect::new(10, 10, 60, 40));
                assert_eq!(target.stop_reason, StopReason::Complete);
            }
            other => panic!("expected a target, got {other:?}"),
        }
        assert!(!observed.load(Ordering::SeqCst), "nobody cancelled this query");
    }

    #[test]
    fn a_window_neither_provider_can_resolve_never_invents_geometry() {
        let worker = RefinementWorker::new(
            candidate_thread(),
            win32::HitTestPassThrough::default(),
            crate::window_detection::DEFAULT_ADOPT_TEXT_RUNS,
            WindowDetectionMetrics::new(),
        );
        // A fabricated handle: no accessibility tree can be attributed to it. UIA reports
        // `Unsupported`, the MSAA fallback is asked next and reports a provider failure; the
        // point is that neither ever invents a rectangle, so the overlay keeps the v1 frame.
        worker.request(job_for(1, Point::new(5, 5)), Rect::new(0, 0, 100, 100));
        let result = wait_for(&worker, Duration::from_secs(5)).expect("a result");
        assert!(
            matches!(
                result.outcome,
                RefinementOutcome::Empty(StopReason::Unsupported | StopReason::ProviderFailure)
            ),
            "no geometry may be invented for a window no provider can resolve, got {:?}",
            result.outcome
        );
    }

    #[test]
    fn a_superseded_query_is_cancelled_cooperatively() {
        let observed = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicU32::new(0));
        let observed_for_provider = Arc::clone(&observed);
        let calls_for_provider = Arc::clone(&calls);
        let worker = RefinementWorker::with_provider(
            candidate_thread(),
            WindowDetectionMetrics::new(),
            Box::new(move || {
                Box::new(SlowProvider {
                    delay: Duration::from_millis(400),
                    result: Some(deep(Rect::new(10, 10, 60, 40), StopReason::Complete)),
                    observed_cancel: observed_for_provider,
                    calls: calls_for_provider,
                })
            }),
        );
        let mut gate = RequestGate::new();
        let first = RefinementJob {
            request: gate.issue(),
            window: window(),
            epoch: 1,
            point: Point::new(20, 20),
        };
        worker.request(first, Rect::new(0, 0, 100, 100));
        // Let the worker pick the first job up, then supersede it.
        let deadline = Instant::now() + Duration::from_secs(2);
        while calls.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "the first query started");
        let second = RefinementJob {
            request: gate.issue(),
            window: window(),
            epoch: 2,
            point: Point::new(21, 21),
        };
        worker.request(second, Rect::new(0, 0, 100, 100));

        // Cancellation is cooperative, so give the traversal a moment to poll it.
        let deadline = Instant::now() + Duration::from_secs(2);
        while !observed.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            observed.load(Ordering::SeqCst),
            "the running query must observe the cancellation"
        );

        // Every job the worker *starts* reports back exactly once, so neither side can wait
        // forever on a query that was replaced. The superseded one reports its cancellation
        // under its own id, which the scheduler then ignores; the newest id carries the target.
        let result = wait_for_request(&worker, second.request, Duration::from_secs(5))
            .expect("the newest query answers");
        assert_eq!(result.request, second.request);
        assert_eq!(result.epoch, 2);
        assert!(matches!(
            result.outcome,
            RefinementOutcome::Target(target) if target.screen_bounds == Rect::new(10, 10, 60, 40)
        ));
    }

    #[test]
    fn shutdown_is_idempotent_and_leaves_no_thread() {
        let mut worker = RefinementWorker::new(
            candidate_thread(),
            win32::HitTestPassThrough::default(),
            crate::window_detection::DEFAULT_ADOPT_TEXT_RUNS,
            WindowDetectionMetrics::new(),
        );
        worker.shutdown();
        worker.shutdown();
        // A request after shutdown is never executed and never blocks.
        worker.request(job_for(1, Point::new(1, 1)), Rect::new(0, 0, 10, 10));
        assert!(worker.take_result().is_none());
    }

    /// The notify thread id is irrelevant in tests: they poll `take_result` directly.
    fn candidate_thread() -> u32 {
        0
    }
}
