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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use ::windows::Win32::Foundation::{LPARAM, WPARAM};
use ::windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP};

use crate::capture::diagnostics::WindowDetectionMetrics;
use crate::capture::geometry::{Point, Rect};
use crate::capture::window_detection::deep::{
    DeepSelectionProvider, QueryControl, RefinementJob, RefinementOutcome,
};
use crate::capture::window_detection::model::{RequestGate, RequestId, SnapshotEpoch, WindowIdentity};

use super::uia_provider::UiaDeepSelectionProvider;

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
    pub fn new(notify_thread: u32, metrics: WindowDetectionMetrics) -> Self {
        Self::with_provider(
            notify_thread,
            metrics,
            Box::new(|| Box::new(UiaDeepSelectionProvider::new())),
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

    /// Queue a query for `point` inside `window_bounds`. Returns its request id.
    pub fn request(
        &self,
        epoch: SnapshotEpoch,
        window: WindowIdentity,
        point: Point,
        window_bounds: Rect,
    ) -> RequestId {
        let request = self
            .shared
            .requests
            .lock()
            .map(|mut gate| gate.issue())
            .expect("the request gate is never poisoned");
        if let Ok(mut pending) = self.shared.pending.lock() {
            if pending.is_some() {
                self.shared.cancelled.fetch_add(1, Ordering::Relaxed);
            }
            *pending = Some(Job {
                request,
                epoch,
                window,
                point,
                window_bounds,
            });
        }
        self.shared.wake.notify_all();
        request
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
        if is_current(job.request) {
            publish(&shared, job, outcome, started.elapsed());
        }
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
    use crate::capture::window_detection::deep::{DeepTarget, StopReason};
    use crate::capture::window_detection::model::TargetKind;
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
            path: vec![Rect::new(0, 0, 1000, 800), bounds],
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
            _job: &crate::capture::window_detection::deep::RefinementJob,
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
        let request = worker.request(7, window(), Point::new(20, 20), Rect::new(0, 0, 100, 100));
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
    fn a_window_uia_cannot_resolve_degrades_to_unsupported() {
        let worker = RefinementWorker::new(candidate_thread(), WindowDetectionMetrics::new());
        // A fabricated handle: no accessibility tree can be attributed to it, so the worker
        // must report `Unsupported` and the overlay keeps the v1 frame.
        worker.request(1, window(), Point::new(5, 5), Rect::new(0, 0, 100, 100));
        let result = wait_for(&worker, Duration::from_secs(5)).expect("a result");
        assert_eq!(
            result.outcome,
            RefinementOutcome::Empty(StopReason::Unsupported),
            "no geometry may be invented for a window UIA cannot resolve"
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
        let _first = worker.request(1, window(), Point::new(20, 20), Rect::new(0, 0, 100, 100));
        // Let the worker pick the first job up, then supersede it.
        let deadline = Instant::now() + Duration::from_secs(2);
        while calls.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "the first query started");
        let second = worker.request(2, window(), Point::new(21, 21), Rect::new(0, 0, 100, 100));

        // Cancellation is cooperative, so give the traversal a moment to poll it.
        let deadline = Instant::now() + Duration::from_secs(2);
        while !observed.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            observed.load(Ordering::SeqCst),
            "the running query must observe the cancellation"
        );
        // The superseded result is not published; the newest request eventually is.
        let result = wait_for(&worker, Duration::from_secs(5)).expect("a result");
        assert_eq!(result.request, second);
        assert_eq!(result.epoch, 2);
    }

    #[test]
    fn shutdown_is_idempotent_and_leaves_no_thread() {
        let mut worker = RefinementWorker::new(candidate_thread(), WindowDetectionMetrics::new());
        worker.shutdown();
        worker.shutdown();
        // A request after shutdown is never executed and never blocks.
        worker.request(1, window(), Point::new(1, 1), Rect::new(0, 0, 10, 10));
        assert!(worker.take_result().is_none());
    }

    /// The notify thread id is irrelevant in tests: they poll `take_result` directly.
    fn candidate_thread() -> u32 {
        0
    }
}
