//! Single OCR worker + in-memory queue.

use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};



use super::engine::{OcrCancel, OcrEngine, OcrInput, OcrText};
use super::events::OcrEventSink;
use crate::domain::OcrErrorCode;
use crate::infrastructure::store::{OcrCandidateFilter, OcrFinishOutcome, QueueDecision, Store};

const QUEUE_CAP: usize = 64;
const COMPENSATE_EVERY: u32 = 8;
const COMPENSATE_INTERVAL: Duration = Duration::from_secs(30);
/// How long the worker waits before its first backlog sweep.
///
/// A sweep can load the OCR model, which must not happen while the window is still being
/// created. Explicitly queued jobs are unaffected and start immediately.
const STARTUP_BACKFILL_DELAY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct OcrJob {
    pub clip_id: String,
    pub content_hash: String,
}

#[derive(Clone)]
pub struct OcrEnqueuer {
    tx: SyncSender<OcrJob>,
    seen: Arc<Mutex<HashSet<String>>>,
}

impl OcrEnqueuer {
    /// Non-blocking enqueue. Queue full → false (status stays `none`).
    pub fn try_enqueue(&self, clip_id: &str, content_hash: &str) -> bool {
        {
            let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
            if !seen.insert(clip_id.to_string()) {
                return true;
            }
        }
        match self.tx.try_send(OcrJob {
            clip_id: clip_id.to_string(),
            content_hash: content_hash.to_string(),
        }) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
                seen.remove(clip_id);
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    pub fn wake(&self) {
        let _ = self.tx.try_send(OcrJob {
            clip_id: String::new(),
            content_hash: String::new(),
        });
    }
}

pub struct OcrServiceHandle {
    enqueuer: OcrEnqueuer,
    stop: Arc<AtomicBool>,
    /// Shared with in-flight jobs; cancelled on drop so shutdown is prompt.
    shutdown_cancel: OcrCancel,
    worker: Option<JoinHandle<()>>,
}

impl OcrServiceHandle {
    pub fn enqueuer(&self) -> OcrEnqueuer {
        self.enqueuer.clone()
    }
}

impl Drop for OcrServiceHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Cancel any recognize() in flight so worker join does not wait for timeout.
        self.shutdown_cancel.cancel();
        let _ = self.enqueuer.wake();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub struct OcrService;

impl OcrService {
    pub fn start(
        store: Store,
        sink: Arc<dyn OcrEventSink>,
        engine: Arc<dyn OcrEngine>,
    ) -> OcrServiceHandle {
        let (tx, rx) = sync_channel::<OcrJob>(QUEUE_CAP);
        let seen = Arc::new(Mutex::new(HashSet::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let shutdown_cancel = OcrCancel::new();
        let worker_seen = seen.clone();
        let worker_stop = stop.clone();
        let worker_cancel = shutdown_cancel.clone();
        let handle = thread::Builder::new()
            .name("snapclip-ocr-worker".into())
            .spawn(move || {
                worker_loop(
                    store,
                    sink,
                    engine,
                    rx,
                    worker_seen,
                    worker_stop,
                    worker_cancel,
                )
            })
            .expect("spawn ocr worker");
        OcrServiceHandle {
            enqueuer: OcrEnqueuer { tx, seen },
            stop,
            shutdown_cancel,
            worker: Some(handle),
        }
    }
}

fn worker_loop(
    store: Store,
    sink: Arc<dyn OcrEventSink>,
    engine: Arc<dyn OcrEngine>,
    rx: Receiver<OcrJob>,
    seen: Arc<Mutex<HashSet<String>>>,
    stop: Arc<AtomicBool>,
    shutdown_cancel: OcrCancel,
) {
    let mut completed: u32 = 0;
    // Startup backfill is deliberately deferred.
    //
    // `compensate` sweeps clips that are already in the database, and a sweep runs the
    // engine — which loads the OCR model — immediately. Doing that during startup put a
    // model load in front of window creation on any machine that had a backlog, and the
    // OS reported the window as unresponsive. Explicit jobs are still processed as soon as
    // they arrive; only the backlog sweep waits.
    let startup = Instant::now();
    let mut last_compensate = startup;

    // Startup recovery: clear stale queued/running.
    let _ = store.reset_stale_ocr_jobs();

    // WinRT/COM apartment belongs to this long-lived OCR thread.
    let com_guard = match super::init_apartment() {
        Ok(guard) => Some(guard),
        Err(error) => {
            eprintln!("[snapclip][ocr] COM init failed: {error}");
            None
        }
    };

    while !stop.load(Ordering::SeqCst) {
        // The first sweep is deferred so the model load never races window creation.
        let backfill_due =
            startup.elapsed() >= STARTUP_BACKFILL_DELAY && last_compensate.elapsed() >= COMPENSATE_INTERVAL;

        let job = match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(job) => job,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if backfill_due {
                    last_compensate = Instant::now();
                    compensate(&store, sink.as_ref(), engine.as_ref(), &seen, &shutdown_cancel);
                }
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };

        if job.clip_id.is_empty() {
            if backfill_due {
                last_compensate = Instant::now();
                compensate(&store, sink.as_ref(), engine.as_ref(), &seen, &shutdown_cancel);
            }
            continue;
        }

        process_job(&store, sink.as_ref(), engine.as_ref(), &job, &shutdown_cancel);
        {
            let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
            seen.remove(&job.clip_id);
        }
        completed = completed.saturating_add(1);
        if completed >= COMPENSATE_EVERY && backfill_due {
            completed = 0;
            last_compensate = Instant::now();
            compensate(&store, sink.as_ref(), engine.as_ref(), &seen, &shutdown_cancel);
        }
    }

    drop(com_guard);
}

fn compensate(
    store: &Store,
    sink: &dyn OcrEventSink,
    engine: &dyn OcrEngine,
    seen: &Arc<Mutex<HashSet<String>>>,
    shutdown_cancel: &OcrCancel,
) {
    if let Ok(candidates) = store.list_ocr_candidates(OcrCandidateFilter::None, 8) {
        for candidate in candidates {
            {
                let mut guard = seen.lock().unwrap_or_else(|e| e.into_inner());
                if !guard.insert(candidate.clip_id.clone()) {
                    continue;
                }
            }
            // none/failed → queued first (claim only accepts queued).
            match store.enqueue_ocr(candidate.clip_id.clone(), candidate.content_hash.clone()) {
                Ok(QueueDecision::Enqueued { .. }) | Ok(QueueDecision::AlreadyPending) => {
                    process_job(
                        store,
                        sink,
                        engine,
                        &OcrJob {
                            clip_id: candidate.clip_id.clone(),
                            content_hash: candidate.content_hash,
                        },
                        shutdown_cancel,
                    );
                }
                _ => {}
            }
            let mut guard = seen.lock().unwrap_or_else(|e| e.into_inner());
            guard.remove(&candidate.clip_id);
        }
    }
}

fn process_job(
    store: &Store,
    sink: &dyn OcrEventSink,
    engine: &dyn OcrEngine,
    job: &OcrJob,
    cancel: &OcrCancel,
) {
    if !engine.is_available() {
        if let Ok(Some(attempt)) =
            store.claim_ocr_job(job.clip_id.clone(), job.content_hash.clone())
        {
            let committed = store
                .finish_ocr_job(
                    job.clip_id.clone(),
                    attempt,
                    OcrFinishOutcome::Skipped {
                        error_code: OcrErrorCode::LanguageUnavailable,
                        engine: Some(engine.name().to_string()),
                    },
                )
                .unwrap_or(false);
            if committed {
                emit_status(
                    sink,
                    &job.clip_id,
                    "skipped",
                    engine.name(),
                    Some("language_unavailable"),
                );
            }
        }
        return;
    }

    let attempt = match store.claim_ocr_job(job.clip_id.clone(), job.content_hash.clone()) {
        Ok(Some(attempt)) => attempt,
        Ok(None) => return,
        Err(_) => return,
    };

    let bytes = match store
        .read_payload_bytes(job.content_hash.clone(), crate::domain::PayloadKind::Image)
    {
        Ok(bytes) => bytes,
        Err(_) => {
            let committed = store
                .finish_ocr_job(
                    job.clip_id.clone(),
                    attempt,
                    OcrFinishOutcome::Failed {
                        error_code: OcrErrorCode::DecodeFailed,
                        engine: Some(engine.name().to_string()),
                    },
                )
                .unwrap_or(false);
            if committed {
                emit_status(
                    sink,
                    &job.clip_id,
                    "failed",
                    engine.name(),
                    Some("decode_failed"),
                );
            }
            return;
        }
    };

    let input = OcrInput::Png(bytes.into());
    let result = engine.recognize(&input, cancel);
    match result {
        Ok(OcrText {
            text,
            layout,
            engine: name,
        }) => {
            let committed = store
                .finish_ocr_job(
                    job.clip_id.clone(),
                    attempt,
                    OcrFinishOutcome::Done {
                        text,
                        layout,
                        engine: name.to_string(),
                    },
                )
                .unwrap_or(false);
            if committed {
                emit_status(sink, &job.clip_id, "done", name, None);
            }
        }
        Err(error) => {
            eprintln!(
                "[snapclip][ocr] recognition failed: clip_id={} code={} detail={}",
                job.clip_id,
                error.code().as_str(),
                error
            );
            let code = error.code();
            let (status, code_str) = match code {
                OcrErrorCode::LanguageUnavailable => ("skipped", "language_unavailable"),
                OcrErrorCode::DecodeFailed => ("failed", "decode_failed"),
                OcrErrorCode::Timeout => ("failed", "timeout"),
                OcrErrorCode::Cancelled => ("failed", "cancelled"),
                OcrErrorCode::EngineFailed => ("failed", "engine_failed"),
            };
            let outcome = if status == "skipped" {
                OcrFinishOutcome::Skipped {
                    error_code: code,
                    engine: Some(engine.name().to_string()),
                }
            } else {
                OcrFinishOutcome::Failed {
                    error_code: code,
                    engine: Some(engine.name().to_string()),
                }
            };
            let committed = store
                .finish_ocr_job(job.clip_id.clone(), attempt, outcome)
                .unwrap_or(false);
            if committed {
                emit_status(sink, &job.clip_id, status, engine.name(), Some(code_str));
            }
        }
    }
}

fn emit_status(
    sink: &dyn OcrEventSink,
    clip_id: &str,
    status: &str,
    engine: &str,
    error: Option<&str>,
) {
    // The envelope, the event name and the timestamp belong to the adapter
    // (`app::ocr_events`), so the worker stays free of both Tauri and the wire format.
    sink.on_status(clip_id, status, engine, error);
}

#[cfg(test)]
mod tests {
    use super::emit_status;
    use crate::ocr::OcrEventSink;
    use std::sync::Mutex;

    /// Records what the worker published, so the seam added in T0.5.2 is pinned.
    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<(String, String, String, Option<String>)>>);

    impl OcrEventSink for RecordingSink {
        fn on_status(&self, clip_id: &str, status: &str, engine: &str, error_code: Option<&str>) {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((
                    clip_id.to_string(),
                    status.to_string(),
                    engine.to_string(),
                    error_code.map(str::to_string),
                ));
        }
    }

    #[test]
    fn emit_status_forwards_every_field_to_the_sink_unchanged() {
        let sink = RecordingSink::default();
        emit_status(&sink, "clip-1", "failed", "windows", Some("decode_failed"));
        emit_status(&sink, "clip-2", "done", "windows", None);

        let recorded = sink.0.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(
            recorded,
            vec![
                (
                    "clip-1".to_string(),
                    "failed".to_string(),
                    "windows".to_string(),
                    Some("decode_failed".to_string())
                ),
                ("clip-2".to_string(), "done".to_string(), "windows".to_string(), None),
            ]
        );
    }
}
