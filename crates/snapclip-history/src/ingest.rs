//! Clipboard ingest: turn a clipboard change notification into a persisted
//! publication, an OCR job and a UI event.
//!
//! This service owns every policy the platform adapter must not know about:
//! sequence deduplication, the delayed-format read window, source-app resolution,
//! persistence, OCR enqueueing and event publication.
//!
//! It is deliberately transport agnostic: the platform supplies a
//! [`ClipboardSource`], the composition root supplies storage/OCR/event adapters,
//! and the whole flow is unit testable with in-memory doubles.

use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use snapclip_model::{PayloadData, PayloadKind, Publication, PublicationOrigin};

use crate::store::{QueueDecision, Store};
use crate::StoreError;

/// A clipboard change notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipboardSignal {
    pub sequence: u32,
    pub owner: usize,
    pub foreground: usize,
}

/// Read one clipboard snapshot.
pub trait ClipboardSource: Send + 'static {
    /// Read the clipboard. Called repeatedly inside the read window.
    fn read(&mut self, signal: ClipboardSignal) -> Result<ClipboardSnapshot, String>;
}

/// One normalised clipboard snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardSnapshot {
    pub sequence: u32,
    pub owner: usize,
    pub foreground: usize,
    pub payloads: Vec<PayloadData>,
}

impl ClipboardSnapshot {
    pub fn is_empty(&self) -> bool {
        self.payloads.is_empty()
    }

    pub fn has_image(&self) -> bool {
        self.payloads
            .iter()
            .any(|payload| payload.payload.kind == PayloadKind::Image)
    }

    pub fn has_rich_text(&self) -> bool {
        self.payloads.iter().any(|payload| {
            matches!(
                payload.payload.kind,
                PayloadKind::Html | PayloadKind::Rtf
            )
        })
    }

    pub fn into_publication(
        self,
        publication_id: String,
        captured_at_unix_ms: i64,
        source_app: Option<String>,
        source_exe_path: Option<String>,
    ) -> Publication {
        Publication {
            publication_id,
            origin: PublicationOrigin::Clipboard,
            captured_at_unix_ms,
            source_app,
            source_exe_path,
            payloads: self
                .payloads
                .iter()
                .map(|payload| payload.payload.clone())
                .collect(),
        }
    }
}

/// Blocks until the next clipboard change arrives.
pub trait ClipboardEventBridge: Send + 'static {
    /// `None` means the bridge was shut down and no further events will arrive.
    fn next_event(&mut self) -> Option<ClipboardSignal>;
}

/// Persistence the ingest service needs from the outside world.
pub trait ClipboardStore: Send + Sync + 'static {
    fn save_publication(
        &self,
        publication: Publication,
        payloads: Vec<PayloadData>,
    ) -> Result<(), StoreError>;

    fn enqueue_ocr(&self, clip_id: String, content_hash: String)
    -> Result<QueueDecision, StoreError>;

    fn release_queued(&self, clip_id: String, attempt: u32) -> Result<bool, StoreError>;
}

/// Worker queues notified after a successful publication.
pub trait OcrQueue: Send + Sync + 'static {
    /// Returns `false` when the queue could not accept the job.
    fn try_enqueue(&self, clip_id: &str, content_hash: &str) -> bool;
}

/// UI notifications. Kept as a trait so the service never depends on Tauri.
pub trait ClipboardEventSink: Send + Sync + 'static {
    fn on_publication_saved(&self, publication: &Publication);
}

/// How long to keep re-reading while a publisher is still filling in delayed
/// formats (Word and several screenshot tools do this).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadWindow {
    pub attempts: u32,
    pub delay: Duration,
}

impl Default for ReadWindow {
    fn default() -> Self {
        Self {
            attempts: 5,
            delay: Duration::from_millis(40),
        }
    }
}

/// Outcome of handling one signal. Returned so tests can assert on the policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestDecision {
    /// Nothing to do (duplicate sequence number).
    Duplicate,
    /// The clipboard produced no supported payload.
    Empty,
    /// The store rejected the publication.
    Rejected,
    /// A publication was persisted.
    Saved,
}

/// Resolves the originating application for a clipboard signal.
pub trait SourceAppResolver: Send + Sync + 'static {
    fn resolve(&self, signal: ClipboardSignal) -> Option<(String, String)>;
}

pub struct ClipboardIngest<S, B, R, Q>
where
    S: ClipboardStore,
    B: ClipboardEventBridge,
    R: SourceAppResolver,
    Q: OcrQueue,
{
    source: S,
    bridge: B,
    resolver: R,
    ocr: Arc<Q>,
    sink: Arc<dyn ClipboardEventSink>,
    window: ReadWindow,
    last_sequence: Option<u32>,
}

impl<S, B, R, Q> ClipboardIngest<S, B, R, Q>
where
    S: ClipboardStore,
    B: ClipboardEventBridge,
    R: SourceAppResolver,
    Q: OcrQueue,
{
    pub fn new(
        source: S,
        bridge: B,
        resolver: R,
        ocr: Arc<Q>,
        sink: Arc<dyn ClipboardEventSink>,
        window: ReadWindow,
    ) -> Self {
        Self {
            source,
            bridge,
            resolver,
            ocr,
            sink,
            window,
            last_sequence: None,
        }
    }

    /// Consume one clipboard signal. `reader` performs the platform read.
    pub fn handle(
        &mut self,
        signal: ClipboardSignal,
        reader: &mut dyn ClipboardSource,
    ) -> IngestDecision {
        if self.last_sequence == Some(signal.sequence) {
            return IngestDecision::Duplicate;
        }

        let Some(snapshot) = self.read_with_window(signal, reader) else {
            return IngestDecision::Empty;
        };
        if self.last_sequence == Some(snapshot.sequence) {
            return IngestDecision::Duplicate;
        }
        if snapshot.is_empty() {
            // Remember the sequence so an empty clipboard does not keep retrying.
            self.last_sequence = Some(snapshot.sequence);
            return IngestDecision::Empty;
        }

        let captured_at_unix_ms = unix_time_ms();
        let publication_id = format!("clipboard-{captured_at_unix_ms}-{}", snapshot.sequence);
        let source = self.resolver.resolve(signal);
        let (source_app, source_exe_path) = match source {
            Some((name, path)) => (Some(name), Some(path)),
            None => (None, None),
        };
        let payloads = snapshot.payloads.clone();
        let publication =
            snapshot.into_publication(publication_id, captured_at_unix_ms, source_app, source_exe_path);

        if self
            .source
            .save_publication(publication.clone(), payloads)
            .is_err()
        {
            return IngestDecision::Rejected;
        }

        self.last_sequence = Some(signal.sequence);
        self.sink.on_publication_saved(&publication);
        self.schedule_ocr(&publication);
        IngestDecision::Saved
    }

    fn schedule_ocr(&self, publication: &Publication) {
        let Some(image) = publication
            .payloads
            .iter()
            .find(|payload| payload.kind == PayloadKind::Image)
        else {
            return;
        };
        let clip_id = publication.publication_id.clone();
        let hash = image.content_hash.clone();
        match self.source.enqueue_ocr(clip_id.clone(), hash.clone()) {
            Ok(QueueDecision::Enqueued { attempt }) => {
                if !self.ocr.try_enqueue(&clip_id, &hash) {
                    let _ = self.source.release_queued(clip_id, attempt);
                }
            }
            Ok(_) | Err(_) => {}
        }
    }

    /// Read until the snapshot looks complete, or the window runs out.
    ///
    /// Policy: an image or rich-text payload ends the window immediately. A
    /// text-only snapshot keeps waiting because a screenshot tool may still be
    /// publishing PNG in a delayed format.
    fn read_with_window(
        &self,
        signal: ClipboardSignal,
        reader: &mut dyn ClipboardSource,
    ) -> Option<ClipboardSnapshot> {
        let mut best: Option<ClipboardSnapshot> = None;
        for attempt in 0..self.window.attempts.max(1) {
            let snapshot = match reader.read(signal) {
                Ok(snapshot) => snapshot,
                Err(_) => {
                    if attempt + 1 < self.window.attempts {
                        thread::sleep(self.window.delay);
                    }
                    continue;
                }
            };
            let complete = snapshot.has_image() || snapshot.has_rich_text() || snapshot.is_empty();
            best = Some(snapshot);
            if complete {
                break;
            }
            if attempt + 1 < self.window.attempts {
                thread::sleep(self.window.delay);
            }
        }
        best
    }

    /// Run until the event bridge shuts down.
    pub fn run(mut self, mut reader: impl ClipboardSource) {
        while let Some(signal) = self.bridge.next_event() {
            let _ = self.handle(signal, &mut reader);
        }
    }
}

/// Owns the ingest thread.
///
/// Dropping this handle stops the pipeline: the caller-supplied [`StopSignal`] must
/// unblock whatever [`ClipboardEventBridge::next_event`] is waiting on, otherwise the
/// `join` below would never return — and because the handle is created during Tauri's
/// `setup`, blocking there freezes the whole application before its window can paint.
pub struct ClipboardIngestHandle {
    stop: Option<Box<dyn StopSignal>>,
    thread: Option<JoinHandle<()>>,
}

/// Releases a [`ClipboardEventBridge`] from its blocking wait.
///
/// `Send + Sync` because the handle lives in shared application state.
pub trait StopSignal: Send + Sync {
    /// Idempotent: called at most once by [`ClipboardIngestHandle`], but implementations
    /// must tolerate repeated calls.
    fn stop(&mut self);
}

impl<F: FnMut() + Send + Sync> StopSignal for F {
    fn stop(&mut self) {
        self()
    }
}

impl ClipboardIngestHandle {
    pub fn new(stop: impl StopSignal + 'static, thread: JoinHandle<()>) -> Self {
        Self {
            stop: Some(Box::new(stop)),
            thread: Some(thread),
        }
    }

    /// Stop the pipeline and wait for the worker to finish.
    ///
    /// Safe to call explicitly; `Drop` then becomes a no-op.
    pub fn shutdown(&mut self) {
        // Signal first so the worker can leave its blocking wait, then join.
        if let Some(mut stop) = self.stop.take() {
            stop.stop();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ClipboardIngestHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Adapter from the concrete [`Store`] to [`ClipboardStore`].
impl ClipboardStore for Store {
    fn save_publication(
        &self,
        publication: Publication,
        payloads: Vec<PayloadData>,
    ) -> Result<(), StoreError> {
        Store::save_publication(self, publication, payloads)
    }

    fn enqueue_ocr(
        &self,
        clip_id: String,
        content_hash: String,
    ) -> Result<QueueDecision, StoreError> {
        Store::enqueue_ocr(self, clip_id, content_hash)
    }

    fn release_queued(&self, clip_id: String, attempt: u32) -> Result<bool, StoreError> {
        Store::release_queued(self, clip_id, attempt)
    }
}

// 过渡期转发（docs/23 T1.4）：实现已搬到 `snapclip-model`，这里保留同名入口，
// 因为本模块内部与测试都按这个名字调用。P2 迁移 `clipboard_ingest` 时一并收敛。
pub use snapclip_model::time::unix_time_ms;

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    };

    use super::*;
    use snapclip_model::{ImageDimensions, PayloadRef};

    fn payload(kind: PayloadKind, bytes: &[u8]) -> PayloadData {
        PayloadData::new(
            PayloadRef {
                payload_id: format!("payload-{}", bytes.len()),
                content_hash: blake3::hash(bytes).to_hex().to_string(),
                kind,
                size_bytes: bytes.len() as u64,
                mime_type: None,
                image_dimensions: Some(ImageDimensions { width: 2, height: 2 }),
            },
            bytes.to_vec(),
        )
    }

    fn signal(sequence: u32) -> ClipboardSignal {
        ClipboardSignal {
            sequence,
            owner: 1,
            foreground: 2,
        }
    }

    #[derive(Default)]
    struct FakeReader {
        calls: u32,
        /// Snapshot returned on each successive call; the last entry repeats.
        snapshots: Vec<Vec<PayloadData>>,
        fail_first: bool,
    }

    impl ClipboardSource for FakeReader {
        fn read(&mut self, signal: ClipboardSignal) -> Result<ClipboardSnapshot, String> {
            self.calls += 1;
            if self.fail_first && self.calls == 1 {
                return Err("clipboard busy".into());
            }
            let index = (self.calls as usize - 1).min(self.snapshots.len() - 1);
            Ok(ClipboardSnapshot {
                // A real clipboard read reports the sequence number of the content
                // currently on the clipboard, which is the signal's sequence.
                sequence: signal.sequence,
                owner: signal.owner,
                foreground: signal.foreground,
                payloads: self.snapshots[index].clone(),
            })
        }
    }

    #[derive(Default)]
    struct FakeBridge {
        events: Vec<ClipboardSignal>,
    }

    impl ClipboardEventBridge for FakeBridge {
        fn next_event(&mut self) -> Option<ClipboardSignal> {
            if self.events.is_empty() {
                None
            } else {
                Some(self.events.remove(0))
            }
        }
    }

    struct FakeResolver;

    impl SourceAppResolver for FakeResolver {
        fn resolve(&self, _signal: ClipboardSignal) -> Option<(String, String)> {
            Some(("Test App".into(), r"C:\Apps\test.exe".into()))
        }
    }

    #[derive(Default)]
    struct FakeStore {
        saved: Mutex<Vec<Publication>>,
        payloads: Mutex<Vec<Vec<PayloadData>>>,
        enqueued: Mutex<Vec<(String, String)>>,
        released: AtomicU32,
        reject: AtomicBool,
    }

    impl ClipboardStore for FakeStore {
        fn save_publication(
            &self,
            publication: Publication,
            payloads: Vec<PayloadData>,
        ) -> Result<(), StoreError> {
            if self.reject.load(Ordering::Relaxed) {
                return Err(StoreError::Internal("rejected".into()));
            }
            self.saved.lock().unwrap().push(publication);
            self.payloads.lock().unwrap().push(payloads);
            Ok(())
        }

        fn enqueue_ocr(
            &self,
            clip_id: String,
            content_hash: String,
        ) -> Result<QueueDecision, StoreError> {
            self.enqueued.lock().unwrap().push((clip_id, content_hash));
            Ok(QueueDecision::Enqueued { attempt: 1 })
        }

        fn release_queued(&self, _clip_id: String, _attempt: u32) -> Result<bool, StoreError> {
            self.released.fetch_add(1, Ordering::Relaxed);
            Ok(true)
        }
    }

    struct FakeOcrQueue {
        accept: bool,
    }

    impl OcrQueue for FakeOcrQueue {
        fn try_enqueue(&self, _clip_id: &str, _content_hash: &str) -> bool {
            self.accept
        }
    }

    #[derive(Default)]
    struct RecordingSink {
        saved: Mutex<Vec<String>>,
    }

    impl ClipboardEventSink for RecordingSink {
        fn on_publication_saved(&self, publication: &Publication) {
            self.saved
                .lock()
                .unwrap()
                .push(publication.publication_id.clone());
        }
    }

    fn ingest<S: ClipboardStore, B: ClipboardEventBridge, Q: OcrQueue>(
        store: S,
        bridge: B,
        ocr: Q,
        sink: Arc<RecordingSink>,
    ) -> ClipboardIngest<S, B, FakeResolver, Q> {
        ClipboardIngest::new(
            store,
            bridge,
            FakeResolver,
            Arc::new(ocr),
            sink,
            ReadWindow {
                attempts: 3,
                delay: Duration::ZERO,
            },
        )
    }

    #[test]
    fn saves_one_publication_per_sequence_and_deduplicates() {
        // The ingest pipeline runs on its own thread in production, and its read
        // window plus the fake reader need more stack than the default 2 MiB test
        // thread provides.
        std::thread::Builder::new()
            .stack_size(4 * 1024 * 1024)
            .spawn(|| {
                let store = FakeStore::default();
                let sink = Arc::new(RecordingSink::default());
                let mut service = ingest(
                    store,
                    FakeBridge::default(),
                    FakeOcrQueue { accept: true },
                    sink.clone(),
                );
                let mut reader = FakeReader {
                    snapshots: vec![vec![payload(PayloadKind::Text, b"hi")]],
                    ..Default::default()
                };

                assert_eq!(service.handle(signal(7), &mut reader), IngestDecision::Saved);
                assert_eq!(
                    service.handle(signal(7), &mut reader),
                    IngestDecision::Duplicate
                );
                // The first call burns the whole read window on a text-only snapshot;
                // the duplicate must not add any further reads.
                assert_eq!(reader.calls, 3, "duplicate must not re-read the clipboard");
                assert_eq!(service.source.saved.lock().unwrap().len(), 1);
                assert_eq!(sink.saved.lock().unwrap().len(), 1);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn text_only_clipboard_uses_the_whole_read_window() {
        let store = FakeStore::default();
        let sink = Arc::new(RecordingSink::default());
        let mut service = ingest(
            store,
            FakeBridge::default(),
            FakeOcrQueue { accept: true },
            sink,
        );
        let mut reader = FakeReader {
            snapshots: vec![vec![payload(PayloadKind::Text, b"hi")]],
            ..Default::default()
        };
        service.handle(signal(1), &mut reader);
        // Text-only snapshots keep the window open for delayed rich formats.
        assert_eq!(reader.calls, 3);
    }

    #[test]
    fn image_payload_ends_the_read_window_immediately() {
        let store = FakeStore::default();
        let sink = Arc::new(RecordingSink::default());
        let mut service = ingest(
            store,
            FakeBridge::default(),
            FakeOcrQueue { accept: true },
            sink,
        );
        let mut reader = FakeReader {
            snapshots: vec![vec![payload(PayloadKind::Image, b"png-bytes")]],
            ..Default::default()
        };
        service.handle(signal(1), &mut reader);
        assert_eq!(reader.calls, 1);
    }

    #[test]
    fn read_failure_is_retried_inside_the_window() {
        let store = FakeStore::default();
        let sink = Arc::new(RecordingSink::default());
        let mut service = ingest(
            store,
            FakeBridge::default(),
            FakeOcrQueue { accept: true },
            sink,
        );
        let mut reader = FakeReader {
            snapshots: vec![vec![payload(PayloadKind::Image, b"png-bytes")]],
            fail_first: true,
            ..Default::default()
        };
        assert_eq!(
            service.handle(signal(1), &mut reader),
            IngestDecision::Saved
        );
        assert_eq!(reader.calls, 2);
    }

    #[test]
    fn empty_clipboard_is_remembered_and_not_retried() {
        let store = FakeStore::default();
        let sink = Arc::new(RecordingSink::default());
        let mut service = ingest(
            store,
            FakeBridge::default(),
            FakeOcrQueue { accept: true },
            sink,
        );
        let mut reader = FakeReader {
            snapshots: vec![vec![]],
            ..Default::default()
        };
        assert_eq!(
            service.handle(signal(3), &mut reader),
            IngestDecision::Empty
        );
        assert_eq!(
            service.handle(signal(3), &mut reader),
            IngestDecision::Duplicate
        );
        assert!(service.source.saved.lock().unwrap().is_empty());
    }

    #[test]
    fn store_failure_does_not_advance_the_sequence() {
        let store = FakeStore::default();
        store.reject.store(true, Ordering::Relaxed);
        let sink = Arc::new(RecordingSink::default());
        let mut service = ingest(
            store,
            FakeBridge::default(),
            FakeOcrQueue { accept: true },
            sink.clone(),
        );
        let mut reader = FakeReader {
            snapshots: vec![vec![payload(PayloadKind::Image, b"png-bytes")]],
            ..Default::default()
        };
        assert_eq!(
            service.handle(signal(5), &mut reader),
            IngestDecision::Rejected
        );
        assert!(sink.saved.lock().unwrap().is_empty());

        service.source.reject.store(false, Ordering::Relaxed);
        assert_eq!(
            service.handle(signal(5), &mut reader),
            IngestDecision::Saved
        );
    }

    #[test]
    fn image_publications_enqueue_ocr_and_release_when_the_queue_is_full() {
        let store = FakeStore::default();
        let sink = Arc::new(RecordingSink::default());
        let mut service = ingest(
            store,
            FakeBridge::default(),
            FakeOcrQueue { accept: false },
            sink,
        );
        let mut reader = FakeReader {
            snapshots: vec![vec![
                payload(PayloadKind::Text, b"caption"),
                payload(PayloadKind::Image, b"png-bytes"),
            ]],
            ..Default::default()
        };
        service.handle(signal(9), &mut reader);
        assert_eq!(service.source.enqueued.lock().unwrap().len(), 1);
        assert_eq!(service.source.released.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn saved_publication_carries_origin_source_and_payloads() {
        let store = FakeStore::default();
        let sink = Arc::new(RecordingSink::default());
        let mut service = ingest(
            store,
            FakeBridge::default(),
            FakeOcrQueue { accept: true },
            sink,
        );
        let mut reader = FakeReader {
            snapshots: vec![vec![payload(PayloadKind::Text, b"hello")]],
            ..Default::default()
        };
        service.handle(signal(11), &mut reader);
        let saved = service.source.saved.lock().unwrap();
        let publication = saved.first().unwrap();
        assert_eq!(publication.origin, PublicationOrigin::Clipboard);
        assert!(publication.publication_id.starts_with("clipboard-"));
        assert_eq!(publication.source_app.as_deref(), Some("Test App"));
        assert_eq!(publication.payloads.len(), 1);
        assert_eq!(
            service.source.payloads.lock().unwrap()[0][0].bytes,
            b"hello"
        );
    }

    #[test]
    fn run_drains_the_bridge_until_shutdown() {
        let store = FakeStore::default();
        let sink = Arc::new(RecordingSink::default());
        let service = ingest(
            store,
            FakeBridge {
                events: vec![signal(1), signal(2)],
            },
            FakeOcrQueue { accept: true },
            sink.clone(),
        );
        let reader = FakeReader {
            snapshots: vec![vec![payload(PayloadKind::Image, b"png-bytes")]],
            ..Default::default()
        };
        service.run(reader);
        assert_eq!(sink.saved.lock().unwrap().len(), 2);
    }
}
