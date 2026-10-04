//! Persistent export worker, decoupled from the overlay UI thread.
//!
//! The Phase 3 contract (docs/11 §Phase 3, docs/12 阶段交付) is that confirming a
//! selection must not block the message pump: encoding a large PNG takes tens to
//! hundreds of milliseconds, during which `Esc`, `WM_PAINT` and window messages have
//! to keep being answered.
//!
//! What stays on the overlay thread is exactly one step: [`FrozenFrame::read_region`].
//! It is the only part of the pipeline that touches the D3D11 immediate context, and
//! that context is single-threaded — the overlay's Direct2D rendering uses the same
//! one. Everything after it (encode, atomic write) is GPU-free and runs here.
//!
//! The protocol mirrors [`super::capture_worker`] deliberately: capacity-1 mailbox, one
//! monotonic generation shared with cancellation, stale results dropped rather than
//! joined. A stale export additionally deletes the file it already wrote, so a
//! cancelled session cannot leave an orphan artifact behind.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Instant;

use windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW;

use crate::application::capture_service::SelectionPixels;
use crate::capture::{CaptureError, CaptureResult};
use crate::domain::CaptureArtifact;

/// `WM_APP`-based notification posted when an export result is ready.
pub const EXPORT_READY_MESSAGE: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 19;

/// Encode and write one confirmed selection.
///
/// Supplied by the overlay because the capture service is generic over its artifact
/// directory and its encoder, and the worker below must not become generic as well:
/// it is the piece that has to stay a plain thread with a mailbox. The closure is
/// GPU-free by construction — it only ever receives the selection that was already
/// read back on the overlay thread.
pub type ExportExecutor =
    Box<dyn Fn(&ExportJob) -> CaptureResult<CaptureArtifact> + Send + Sync + 'static>;

/// One confirmed selection to be exported.
///
/// It carries [`SelectionPixels`] — the bytes the overlay already read back on its
/// own thread — rather than the frozen frame: the D3D11 texture is single-threaded
/// and belongs to the overlay, and nothing after readback needs it. The encode and
/// write the worker performs are GPU-free, so the export survives the overlay
/// releasing its frame (Esc, display change) while the encode is in flight.
pub struct ExportJob {
    /// Stamped by [`ExportWorker::submit`]. A job whose generation is no longer
    /// current is dropped, and its artifact deleted.
    pub generation: u64,
    pub session_id: String,
    /// The validated, region-sized pixels to encode, read back on the overlay thread.
    pub prepared: SelectionPixels,
    pub dpi: u32,
    pub monitor_device_name: Option<String>,
    /// Overlay UI thread id to wake with [`EXPORT_READY_MESSAGE`].
    pub notify_thread: u32,
    pub executor: ExportExecutor,
}

/// An export that failed, with the stage that produced it.
#[derive(Debug)]
pub struct ExportFailure {
    pub generation: u64,
    pub error: CaptureError,
    pub stage: &'static str,
}

/// A finished export.
#[derive(Debug)]
pub struct CompletedExport {
    pub generation: u64,
    pub artifact: CaptureArtifact,
}

enum WorkerEvent {
    Ready(Result<CompletedExport, ExportFailure>),
}

struct Mailbox {
    inner: Mutex<MailboxInner>,
    request_available: Condvar,
    current_generation: AtomicU64,
    stopping: AtomicBool,
}

#[derive(Default)]
struct MailboxInner {
    /// Capacity 1: confirming twice before the worker wakes up exports only the last.
    pending_job: Option<Arc<ExportJob>>,
    /// The in-flight job, kept so the mailbox owns a reference while it runs.
    active_job: Option<Arc<ExportJob>>,
    /// Capacity 1: only the newest result is retained.
    result: Option<WorkerEvent>,
}

impl Mailbox {
    fn new() -> Self {
        Self {
            inner: Mutex::new(MailboxInner::default()),
            request_available: Condvar::new(),
            current_generation: AtomicU64::new(0),
            stopping: AtomicBool::new(false),
        }
    }

    fn next_generation(&self) -> u64 {
        self.current_generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn current_generation(&self) -> u64 {
        self.current_generation.load(Ordering::Acquire)
    }

    fn is_stale(&self, generation: u64) -> bool {
        generation != self.current_generation() || self.stopping.load(Ordering::Acquire)
    }

    /// Stamp `job` with a fresh generation and queue it, returning that generation.
    /// The return value of a *previous*, still-queued job is reported as superseded.
    fn submit(&self, job: ExportJob) -> Option<u64> {
        if self.stopping.load(Ordering::Acquire) {
            return None;
        }
        let generation = self.next_generation();
        let job = ExportJob { generation, ..job };
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let superseded = inner.pending_job.replace(Arc::new(job));
        if let Some(stale) = superseded {
            eprintln!(
                "[snapclip][capture] export request superseded generation={}",
                stale.generation
            );
        }
        drop(inner);
        self.request_available.notify_one();
        Some(generation)
    }

    /// Mark every unconsumed result stale. Never waits for the worker.
    fn invalidate(&self) -> u64 {
        self.next_generation()
    }

    /// Block until the next job to export. `None` means the worker must exit.
    /// Stale queued jobs are discarded here, so a cancelled export never runs.
    fn wait_job(&self) -> Option<Arc<ExportJob>> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            while inner.pending_job.is_none() && !self.stopping.load(Ordering::Acquire) {
                inner = self
                    .request_available
                    .wait(inner)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            if self.stopping.load(Ordering::Acquire) {
                return None;
            }
            let job = inner.pending_job.take().expect("checked non-empty");
            if job.generation != self.current_generation() {
                continue;
            }
            inner.active_job = Some(job.clone());
            return Some(job);
        }
    }

    /// Publish a result, unless the session was cancelled while the worker was busy —
    /// in which case the artifact it already wrote is deleted right here.
    fn complete(&self, job: &ExportJob, event: WorkerEvent) {
        let written = match &event {
            WorkerEvent::Ready(Ok(completed)) => {
                completed.artifact.png_path().map(Path::to_path_buf)
            }
            WorkerEvent::Ready(Err(_)) => None,
        };
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.active_job = None;
        if self.is_stale(job.generation) {
            drop(inner);
            if let Some(path) = written {
                eprintln!(
                    "[snapclip][capture] export discarded session={} generation={} (cancelled)",
                    job.session_id, job.generation
                );
                delete_quietly(&path);
            }
            return;
        }
        inner.result = Some(event);
        drop(inner);
        // Same tolerance as the capture worker: a lost post is drained by the next
        // message, and the overlay re-checks the mailbox on every wake-up.
        unsafe {
            PostThreadMessageW(job.notify_thread, EXPORT_READY_MESSAGE, 0, 0);
        }
    }

    fn take_result(&self) -> Option<WorkerEvent> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.result.take()
    }
}

/// The worker handle owned by the overlay controller.
pub struct ExportWorker {
    mailbox: Arc<Mailbox>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl ExportWorker {
    pub fn new() -> Self {
        Self {
            mailbox: Arc::new(Mailbox::new()),
            thread: Mutex::new(None),
        }
    }

    /// Queue one export, starting the thread on first use.
    ///
    /// `Ok(None)` means the worker is shutting down and refused the job; the caller
    /// reports that itself, because nothing will ever post back.
    pub fn submit(&self, job: ExportJob) -> Result<Option<u64>, String> {
        self.ensure_running()?;
        Ok(self.mailbox.submit(job))
    }

    fn ensure_running(&self) -> Result<(), String> {
        let mut slot = self
            .thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.as_ref().map(|handle| !handle.is_finished()).unwrap_or(false) {
            return Ok(());
        }
        // A finished (panicked) handle from a previous run is reaped here.
        if let Some(handle) = slot.take() {
            let _ = handle.join();
        }
        let mailbox = self.mailbox.clone();
        let thread = thread::Builder::new()
            .name("snapclip-export-worker".into())
            .spawn(move || worker_main(mailbox))
            .map_err(|error| format!("spawn export worker failed: {error}"))?;
        *slot = Some(thread);
        Ok(())
    }

    /// Mark any in-flight or queued export stale, so its result is dropped and its
    /// file deleted instead of being delivered. Driven by every cancellation path.
    pub fn cancel(&self) {
        self.mailbox.invalidate();
    }

    /// Drain one ready result. Results whose generation is no longer current are
    /// dropped here and reported as `None`.
    pub fn take_ready(&self) -> Option<Result<CompletedExport, ExportFailure>> {
        let event = self.mailbox.take_result()?;
        match event {
            WorkerEvent::Ready(result) => {
                let generation = match &result {
                    Ok(completed) => completed.generation,
                    Err(failure) => failure.generation,
                };
                if self.mailbox.is_stale(generation) {
                    eprintln!("[snapclip][capture] export result discarded generation mismatch");
                    None
                } else {
                    Some(result)
                }
            }
        }
    }

    /// Stop the thread and release the last frozen-frame reference it holds. Joined,
    /// because shutdown must not leave a GPU reference alive past the overlay.
    pub fn shutdown(&self) {
        self.mailbox.invalidate();
        self.mailbox.stopping.store(true, Ordering::Release);
        // Notify while holding the lock: a parked `wait_job` re-checks `stopping` when
        // it wakes, and a worker about to wait cannot miss the flag because it inspects
        // it under the same lock.
        let _guard = self
            .mailbox
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.mailbox.request_available.notify_all();
        drop(_guard);
        let handle = self
            .thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

fn worker_main(mailbox: Arc<Mailbox>) {
    loop {
        let Some(job) = mailbox.wait_job() else {
            return;
        };
        let started_at = Instant::now();
        let event = match (job.executor)(&job) {
            Ok(artifact) => {
                eprintln!(
                    "[snapclip][bench] stage=export session={} path={} size={}x{} encode_write_ms={}",
                    job.session_id,
                    artifact
                        .png_path()
                        .map(|path| path.to_string_lossy())
                        .unwrap_or_default(),
                    artifact.width,
                    artifact.height,
                    started_at.elapsed().as_millis()
                );
                WorkerEvent::Ready(Ok(CompletedExport {
                    generation: job.generation,
                    artifact,
                }))
            }
            Err(error) => {
                eprintln!(
                    "[snapclip][capture] export failed session={} generation={} elapsed_ms={} error={}",
                    job.session_id,
                    job.generation,
                    started_at.elapsed().as_millis(),
                    error
                );
                let stage = if matches!(error, CaptureError::DeviceRemoved(_)) {
                    "device"
                } else {
                    "export"
                };
                WorkerEvent::Ready(Err(ExportFailure {
                    generation: job.generation,
                    error,
                    stage,
                }))
            }
        };
        mailbox.complete(&job, event);
    }
}

/// Remove a discarded artifact, retrying once: Windows can still hold a last handle
/// reference for a moment right after the writer closed the file.
fn delete_quietly(path: &Path) {
    if std::fs::remove_file(path).is_ok() || std::fs::remove_file(path).is_ok() {
        return;
    }
    eprintln!(
        "[snapclip][capture] warning: cancelled artifact left behind: {}",
        path.to_string_lossy()
    );
}

#[cfg(test)]
mod tests {
    use super::{
        CompletedExport, ExportFailure, ExportJob, ExportWorker, Mailbox, WorkerEvent,
        EXPORT_READY_MESSAGE,
    };
    use crate::application::capture_service::SelectionPixels;
    use crate::capture::geometry::Rect;
    use crate::capture::session::CapturedFrame;
    use crate::capture::{CaptureError, CaptureResult};
    use crate::domain::{CaptureArtifact, CapturePayload, PixelFormat};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A 4x4 selection of an 8x8 frame. The worker never inspects the bytes, so a
    /// flat buffer is enough to exercise the mailbox and the executor hand-off.
    fn test_selection_pixels() -> SelectionPixels {
        SelectionPixels {
            frame: CapturedFrame {
                width: 8,
                height: 8,
                pixel_format: PixelFormat::Bgra8Unorm,
                captured_at_unix_ms: 1,
                provider: "test",
            },
            region: Rect::new(0, 0, 4, 4),
            bgra: vec![0u8; 4 * 4 * 4],
        }
    }

    fn artifact(path: PathBuf, session_id: &str) -> CaptureArtifact {
        CaptureArtifact {
            session_id: session_id.to_string(),
            width: 4,
            height: 4,
            dpi: 96,
            pixel_format: PixelFormat::Bgra8Unorm,
            captured_at_unix_ms: 1,
            monitor_device_name: None,
            payload: CapturePayload::PngFile { path },
        }
    }

    fn artifact_path(tag: &str, generation: u64) -> PathBuf {
        std::env::temp_dir().join(format!(
            "snapclip-export-{}-{tag}-{generation}.png",
            std::process::id()
        ))
    }

    /// A job whose executor is a function pointer; tests that need to capture state
    /// build the job literal directly instead, because a capturing closure cannot name
    /// the generic `E` of this helper.
    fn job(
        executor: fn(&ExportJob) -> CaptureResult<CaptureArtifact>,
    ) -> ExportJob {
        new_job(executor)
    }

    fn new_job<E>(executor: E) -> ExportJob
    where
        E: Fn(&ExportJob) -> CaptureResult<CaptureArtifact> + Send + Sync + 'static,
    {
        ExportJob {
            generation: 0,
            session_id: "session-test".into(),
            prepared: test_selection_pixels(),
            dpi: 96,
            monitor_device_name: Some(r"\\.\DISPLAY1".into()),
            notify_thread: unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() },
            executor: Box::new(executor),
        }
    }

    /// An executor that writes a real file, so staleness has something to delete.
    fn writing(runs: Arc<AtomicUsize>) -> impl Fn(&ExportJob) -> CaptureResult<CaptureArtifact> + Send + Sync + 'static {
        move |job| {
            runs.fetch_add(1, Ordering::SeqCst);
            let path = artifact_path("write", job.generation);
            std::fs::write(&path, b"fake png")
                .map_err(|error| CaptureError::EncodeFailed(error.to_string()))?;
            Ok(artifact(path, &job.session_id))
        }
    }

    /// Poll `take_ready` the way the message pump would, without a real pump.
    fn drain(worker: &ExportWorker) -> Option<Result<CompletedExport, ExportFailure>> {
        for _ in 0..300 {
            if let Some(result) = worker.take_ready() {
                return Some(result);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        None
    }

    fn always_fails(_job: &ExportJob) -> CaptureResult<CaptureArtifact> {
        Err(CaptureError::InvalidState("never runs".into()))
    }

    #[test]
    fn submit_stamps_a_fresh_generation_and_supersedes_a_queued_job() {
        let mailbox = Mailbox::new();
        let runs = Arc::new(AtomicUsize::new(0));
        assert_eq!(mailbox.submit(new_job(writing(runs.clone()))), Some(1));
        assert_eq!(mailbox.current_generation(), 1);
        // Capacity 1: the queued job is replaced, and the new generation is returned.
        assert_eq!(mailbox.submit(new_job(writing(runs))), Some(2));
        assert_eq!(mailbox.current_generation(), 2);
    }

    #[test]
    fn a_cancelled_queued_job_is_never_handed_to_the_worker() {
        let mailbox = Mailbox::new();
        let runs = Arc::new(AtomicUsize::new(0));
        let stale_generation = mailbox
            .submit(new_job(writing(runs.clone())))
            .unwrap();
        mailbox.invalidate(); // Esc before the worker even woke up

        // The stale job is skipped, so `wait_job` blocks; check that by inspecting the
        // mailbox state instead of parking a thread on it.
        assert_ne!(stale_generation, mailbox.current_generation());
        assert!(mailbox.is_stale(stale_generation));

        let fresh_generation = mailbox.submit(new_job(writing(runs.clone()))).unwrap();
        let taken = mailbox.wait_job().expect("the fresh job is current");
        assert_eq!(taken.generation, fresh_generation);
        assert_eq!(
            runs.load(Ordering::SeqCst),
            0,
            "taking a job must not run anything; the worker calls the executor"
        );
    }

    #[test]
    fn a_result_for_the_current_generation_is_delivered_once() {
        let mailbox = Mailbox::new();
        let runs = Arc::new(AtomicUsize::new(0));
        let generation = mailbox.submit(new_job(writing(runs))).unwrap();
        let job = mailbox.wait_job().unwrap();
        assert_eq!(job.generation, generation);
        mailbox.complete(
            &job,
            WorkerEvent::Ready(Ok(CompletedExport {
                generation,
                artifact: artifact(artifact_path("once", generation), "session-test"),
            })),
        );
        assert!(mailbox.take_result().is_some());
        assert!(
            mailbox.take_result().is_none(),
            "the mailbox holds one consumable result"
        );
    }

    #[test]
    fn a_cancelled_export_deletes_the_file_it_already_wrote() {
        let mailbox = Mailbox::new();
        let runs = Arc::new(AtomicUsize::new(0));
        let generation = mailbox.submit(new_job(writing(runs))).unwrap();
        let job = mailbox.wait_job().unwrap();
        let path = artifact_path("discard", generation);
        std::fs::write(&path, b"fake png").unwrap();

        mailbox.invalidate(); // Esc happened while the encoder was running
        mailbox.complete(
            &job,
            WorkerEvent::Ready(Ok(CompletedExport {
                generation,
                artifact: artifact(path.clone(), "session-test"),
            })),
        );

        assert!(!path.exists(), "a cancelled artifact must not be left behind");
        assert!(
            mailbox.take_result().is_none(),
            "and must not be delivered to the overlay"
        );
    }

    #[test]
    fn a_failed_export_is_reported_with_its_stage() {
        let mailbox = Mailbox::new();
        let runs = Arc::new(AtomicUsize::new(0));
        let generation = mailbox.submit(new_job(writing(runs))).unwrap();
        let job = mailbox.wait_job().unwrap();
        mailbox.complete(
            &job,
            WorkerEvent::Ready(Err(ExportFailure {
                generation,
                error: CaptureError::EncodeFailed("encoder exploded".into()),
                stage: "export",
            })),
        );
        let WorkerEvent::Ready(result) = mailbox.take_result().expect("a failure is a result");
        let Err(failure) = result else {
            panic!("expected the failure to be reported")
        };
        assert_eq!(failure.stage, "export");
        assert_eq!(failure.generation, generation);
    }

    #[test]
    fn the_worker_runs_the_executor_without_the_caller_waiting() {
        let worker = ExportWorker::new();
        let runs = Arc::new(AtomicUsize::new(0));
        let generation = worker
            .submit(new_job(writing(runs.clone())))
            .unwrap()
            .expect("the worker accepts a job");
        let ready = drain(&worker).expect("the export completes on its own");
        worker.shutdown();

        let completed = ready.expect("the executor writes a real file");
        assert_eq!(completed.generation, generation);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert!(completed.artifact.png_path().unwrap().exists());
        let _ = std::fs::remove_file(completed.artifact.png_path().unwrap());
    }

    #[test]
    fn cancelling_drops_the_result_and_the_artifact() {
        let worker = ExportWorker::new();
        let runs = Arc::new(AtomicUsize::new(0));
        let generation = worker
            .submit(new_job(writing(runs.clone())))
            .unwrap()
            .unwrap();
        worker.cancel();
        let delivered = drain(&worker);
        worker.shutdown();

        assert!(
            delivered.is_none(),
            "a cancelled export must never reach the overlay"
        );
        assert!(!artifact_path("write", generation).exists(), "and its file is removed");
    }

    #[test]
    fn job_metadata_reaches_the_executor() {
        // Guards the `Arc<ExportJob>` hand-off: losing a field would silently strip
        // the monitor name or the DPI scale factor out of the artifact.
        let seen = Arc::new(std::sync::Mutex::new(None::<(String, i32, i32, u32, Option<String>)>));
        let seen_in_executor = seen.clone();
        let worker = ExportWorker::new();
        let mut job = new_job(move |job: &ExportJob| {
            *seen_in_executor.lock().unwrap() = Some((
                job.session_id.clone(),
                job.prepared.region.left,
                job.prepared.region.right,
                job.dpi,
                job.monitor_device_name.clone(),
            ));
            Err(CaptureError::InvalidState("stop here".into()))
        });
        job.prepared.region = Rect::new(2, 3, 9, 11);
        job.dpi = 144;
        job.monitor_device_name = Some(r"\\.\DISPLAY3".into());
        job.session_id = "session-metadata".into();
        worker.submit(job).unwrap().unwrap();
        let _ = drain(&worker);
        worker.shutdown();

        let observed = seen.lock().unwrap().clone();
        assert_eq!(
            observed,
            Some((
                "session-metadata".into(),
                2,
                9,
                144,
                Some(r"\\.\DISPLAY3".into())
            ))
        );
    }

    #[test]
    fn shutdown_is_idempotent_and_refuses_new_jobs() {
        let worker = ExportWorker::new();
        worker.shutdown();
        worker.shutdown();
        assert!(worker.take_ready().is_none());
        assert!(
            worker
                .submit(job(always_fails))
                .unwrap()
                .is_none(),
            "a shutting-down worker refuses new jobs"
        );
    }

    #[test]
    fn the_export_message_id_does_not_collide_with_the_capture_worker() {
        assert_ne!(
            EXPORT_READY_MESSAGE,
            super::super::capture_worker::FRAME_READY_MESSAGE
        );
    }
}
