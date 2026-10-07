//! Window-detection worker (docs/14 §5.5, §10.1).
//!
//! `EnumWindows`, `DwmGetWindowAttribute` and the confirm-time validation are all
//! synchronous calls. Running them on the overlay thread would stall the message loop for
//! as long as the compositor takes to answer — exactly what the design forbids. This
//! worker owns the provider and runs every one of them off the UI thread; the overlay only
//! posts requests and drains results.
//!
//! ## Bounded, latest-only mailbox
//!
//! The mailbox holds **one** pending job. A request superseded before the worker picks it up
//! is overwritten rather than queued: a second refresh produces the same answer twice, and an
//! unbounded queue is what turns a busy desktop into growing latency (docs/14 §5.5).
//!
//! Every job and result carries a request id. The overlay compares it against the request it
//! is still waiting for; anything older is counted and dropped, so a slow worker can never
//! resurrect a superseded snapshot or confirm a stale target.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use ::windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP};

use crate::diagnostics::WindowDetectionMetrics;
use crate::window_detection::model::{
    HoverValidity, RequestGate, RequestId, WindowSnapshot, WindowTarget,
};
use crate::window_detection::provider::{
    Exclusions, WindowDetectionError, WindowTargetProvider,
};

use super::window_detection::TopLevelWindowProvider;

/// Posted to the overlay thread when a worker result is waiting.
pub const DETECTION_READY_MESSAGE: u32 = WM_APP + 43;

/// A task for the worker.
enum Job {
    Refresh {
        request: RequestId,
        exclusions: Exclusions,
    },
    Confirm {
        request: RequestId,
        target: WindowTarget,
    },
    Revalidate {
        request: RequestId,
        target: WindowTarget,
    },
    Shutdown,
}

/// Outcome of one job, tagged with the request that asked for it.
#[derive(Debug)]
pub enum DetectionResult {
    Refreshed {
        request: RequestId,
        snapshot: Result<WindowSnapshot, WindowDetectionError>,
    },
    Confirmed {
        request: RequestId,
        target: WindowTarget,
        valid: bool,
    },
    Revalidated {
        request: RequestId,
        target: WindowTarget,
        validity: HoverValidity,
    },
}

struct Shared {
    notify_thread: u32,
    pending: Mutex<Option<Job>>,
    wake: Condvar,
    result: Mutex<Option<DetectionResult>>,
}

/// Owns the detection thread and the provider.
pub struct DetectionWorker {
    shared: Arc<Shared>,
    requests: Mutex<RequestGate>,
    thread: Option<std::thread::JoinHandle<()>>,
    metrics: WindowDetectionMetrics,
}

impl DetectionWorker {
    /// Start the worker. `notify_thread` is the overlay thread id that receives
    /// [`DETECTION_READY_MESSAGE`].
    pub fn new(notify_thread: u32, metrics: WindowDetectionMetrics) -> Self {
        let shared = Arc::new(Shared {
            notify_thread,
            pending: Mutex::new(None),
            wake: Condvar::new(),
            result: Mutex::new(None),
        });
        let worker_shared = Arc::clone(&shared);
        let worker_metrics = metrics.clone();
        let thread = std::thread::Builder::new()
            .name("snapclip-window-detection".into())
            .spawn(move || run(worker_shared, worker_metrics))
            .ok();
        Self {
            shared,
            requests: Mutex::new(RequestGate::new()),
            thread,
            metrics,
        }
    }

    /// Queue a snapshot refresh. Returns the request id its result will carry.
    pub fn request_refresh(&self, exclusions: &Exclusions) -> RequestId {
        self.enqueue(|request| Job::Refresh {
            request,
            exclusions: exclusions.clone(),
        })
    }

    /// Queue a confirmation check for one target.
    pub fn request_confirm(&self, target: WindowTarget) -> RequestId {
        self.enqueue(|request| Job::Confirm { request, target })
    }

    /// Queue a lightweight re-validation of the hovered window (docs/14 §5.5).
    ///
    /// The overlay keeps this single-flight: only one re-validation is outstanding at a
    /// time, so a fast-moving cursor cannot build a backlog of pointless DWM reads.
    pub fn request_revalidate(&self, target: WindowTarget) -> RequestId {
        self.enqueue(|request| Job::Revalidate { request, target })
    }

    /// Take the pending result, if the worker produced one.
    pub fn take_result(&self) -> Option<DetectionResult> {
        let result = self.shared.result.lock().ok()?.take()?;
        self.metrics.record_worker_dequeued();
        Some(result)
    }

    /// Stop the worker.
    ///
    /// Idempotent. If a Win32 call is already in flight the join waits for that one call
    /// to finish (it cannot be cancelled) and then the thread exits without publishing.
    pub fn shutdown(&mut self) {
        if let Ok(mut pending) = self.shared.pending.lock() {
            *pending = Some(Job::Shutdown);
        }
        self.shared.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    fn enqueue(&self, build: impl FnOnce(RequestId) -> Job) -> RequestId {
        let request = self
            .requests
            .lock()
            .map(|mut gate| gate.issue())
            .expect("the request gate is never poisoned");
        if let Ok(mut pending) = self.shared.pending.lock() {
            if pending.is_some() {
                // Bounded mailbox: the superseded request is dropped, not queued.
                self.metrics.record_worker_stale_result_dropped();
            }
            *pending = Some(build(request));
        }
        self.metrics.record_worker_enqueued();
        self.shared.wake.notify_all();
        request
    }
}

impl Drop for DetectionWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run(shared: Arc<Shared>, metrics: WindowDetectionMetrics) {
    let mut provider = TopLevelWindowProvider::new();
    let mut running = true;
    while running {
        let job = {
            let mut pending = match shared.pending.lock() {
                Ok(pending) => pending,
                Err(_) => return,
            };
            while pending.is_none() {
                pending = match shared.wake.wait(pending) {
                    Ok(guard) => guard,
                    Err(_) => return,
                };
            }
            pending.take().expect("the loop only exits with a job in hand")
        };

        let result = match job {
            Job::Shutdown => {
                // Release the provider's caches before the thread ends so a long-lived
                // application does not keep the last snapshot alive per worker.
                provider.reset();
                running = false;
                None
            }
            Job::Refresh {
                request,
                exclusions,
            } => {
                let started = Instant::now();
                let snapshot = provider.refresh(&exclusions);
                let elapsed = started.elapsed();
                if let Ok(snapshot) = &snapshot {
                    metrics.record_snapshot_refresh(elapsed, snapshot.len());
                }
                Some(DetectionResult::Refreshed { request, snapshot })
            }
            Job::Confirm { request, target } => {
                let started = Instant::now();
                let valid = provider.validate(&target);
                metrics.record_validate(started.elapsed());
                Some(DetectionResult::Confirmed {
                    request,
                    target,
                    valid,
                })
            }
            Job::Revalidate { request, target } => {
                let started = Instant::now();
                let validity = provider.revalidate_hover(&target);
                metrics.record_validate(started.elapsed());
                Some(DetectionResult::Revalidated {
                    request,
                    target,
                    validity,
                })
            }
        };

        if let Some(result) = result {
            publish(&shared, &metrics, result);
        }
    }
}

fn publish(shared: &Arc<Shared>, metrics: &WindowDetectionMetrics, result: DetectionResult) {
    // Only the newest result is ever delivered; an uncollected one is replaced.
    if let Ok(mut slot) = shared.result.lock() {
        if slot.is_some() {
            metrics.record_worker_stale_result_dropped();
        }
        *slot = Some(result);
    }
    // Wake the overlay pump. The message carries no payload — the overlay drains the slot.
    let _ = unsafe {
        PostThreadMessageW(
            shared.notify_thread,
            DETECTION_READY_MESSAGE,
            ::windows::Win32::Foundation::WPARAM(0),
            ::windows::Win32::Foundation::LPARAM(0),
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window_detection::model::WindowCandidate;
    use std::time::Duration;

    fn request_of(result: &DetectionResult) -> RequestId {
        match result {
            DetectionResult::Refreshed { request, .. }
            | DetectionResult::Confirmed { request, .. }
            | DetectionResult::Revalidated { request, .. } => *request,
        }
    }

    fn worker() -> DetectionWorker {
        // The tests never rely on the posted message: they poll `take_result`, so the
        // notify thread id is irrelevant here.
        DetectionWorker::new(0, WindowDetectionMetrics::new())
    }

    fn target() -> WindowTarget {
        WindowTarget::top_level_window_frame(WindowCandidate::new(
            crate::window_detection::model::WindowIdentity::new(0xDEAD, 1, 2),
            crate::geometry::Rect::new(0, 0, 10, 10),
            0,
            1,
        ))
    }

    fn wait_for_result(worker: &DetectionWorker) -> DetectionResult {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(result) = worker.take_result() {
                return result;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the detection worker produced no result");
    }

    #[test]
    fn a_refresh_request_produces_a_snapshot_tagged_with_its_request_id() {
        let worker = worker();
        let request = worker.request_refresh(&Exclusions::new());
        let result = wait_for_result(&worker);
        assert_eq!(request_of(&result), request);
        match result {
            DetectionResult::Refreshed { snapshot, .. } => {
                let snapshot = snapshot.expect("enumerating the desktop succeeds");
                assert!(snapshot.epoch() >= 1);
            }
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }

    #[test]
    fn the_mailbox_keeps_only_the_newest_request() {
        let worker = worker();
        let first = worker.request_refresh(&Exclusions::new());
        let second = worker.request_refresh(&Exclusions::new());
        assert_ne!(first, second);

        // The mailbox is bounded: the superseded job is overwritten, so the worker runs
        // the newest one. Whatever it produces is the second request's result.
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(result) = worker.take_result() {
                assert_eq!(request_of(&result), second);
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the detection worker produced no result");
    }

    #[test]
    fn a_confirm_request_reports_validity_for_a_bogus_target() {
        let worker = worker();
        let request = worker.request_confirm(target());
        let result = wait_for_result(&worker);
        assert_eq!(request_of(&result), request);
        match result {
            DetectionResult::Confirmed { valid, target: t, .. } => {
                assert!(!valid, "a fabricated handle cannot be a live window");
                assert_eq!(t.identity().hwnd, 0xDEAD);
            }
            other => panic!("expected a confirmation, got {other:?}"),
        }
    }

    #[test]
    fn a_revalidate_request_classifies_the_hovered_window() {
        let worker = worker();
        let request = worker.request_revalidate(target());
        let result = wait_for_result(&worker);
        assert_eq!(request_of(&result), request);
        match result {
            DetectionResult::Revalidated { validity, .. } => {
                // A fabricated handle is gone: the overlay must refresh and re-hit.
                assert_eq!(validity, HoverValidity::Invalid);
            }
            other => panic!("expected a re-validation, got {other:?}"),
        }
    }

    #[test]
    fn shutdown_is_idempotent_and_releases_the_thread() {
        let mut worker = worker();
        worker.shutdown();
        worker.shutdown();
        // A request after shutdown is simply never executed; the call must not panic and
        // must not block the caller forever.
        let request = worker.request_refresh(&Exclusions::new());
        assert!(request.get() >= 1);
        assert!(worker.take_result().is_none(), "the worker thread is gone");
    }
}
