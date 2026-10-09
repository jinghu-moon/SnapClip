//! The clipboard pipeline, hosted by the GPUI shell (docs/23 P6).
//!
//! Extracted out of the previous shell. The policy — dedup by sequence number, the
//! delayed-format read window, persistence, event publication — lives in
//! `snapclip_history::ingest`; the platform half (listener, reader, source resolution) lives
//! in `snapclip_history::windows`. What is *here* is the composition: bind the two, publish
//! through the shell's own event bus, and own the thread.
//!
//! Counting the moved clock: this pipeline is why the history screen refreshes by itself. The
//! old shell held it, so a clip copied while the GPUI shell was open only appeared after the
//! window regained focus.

use std::sync::{Arc, Mutex};
use std::thread;

use snapclip_history::ingest::{
    ClipboardEventBridge, ClipboardEventSink, ClipboardIngest, ClipboardIngestHandle,
    ClipboardSignal, ClipboardSource, ClipboardSnapshot, OcrQueue, ReadWindow, SourceAppResolver,
    StopSignal,
};
use snapclip_history::store::Store;
use snapclip_history::windows::{
    ClipboardEvent, ClipboardUpdateListener, formats, reader, source_app,
};

use crate::adapters::ClipboardEvents;
use crate::events::EventBus;

/// Reads snapshots through the Win32 clipboard adapter.
struct Win32ClipboardSource;

impl ClipboardSource for Win32ClipboardSource {
    fn read(&mut self, _signal: ClipboardSignal) -> Result<ClipboardSnapshot, String> {
        let snapshot = reader::read_snapshot(formats::formats())?;
        Ok(ClipboardSnapshot {
            sequence: snapshot.sequence,
            owner: snapshot.owner,
            foreground: snapshot.foreground,
            payloads: snapshot.payloads,
        })
    }
}

/// Bridges `WM_CLIPBOARDUPDATE` notifications into the ingest worker.
///
/// The listener sits behind a shared slot so the stop signal can drop it from any thread.
/// Dropping the listener posts `WM_QUIT`, which ends its thread and closes the channel, so the
/// worker's `recv()` returns instead of blocking forever.
struct Win32EventBridge {
    receiver: std::sync::mpsc::Receiver<ClipboardEvent>,
    listener: Arc<Mutex<Option<ClipboardUpdateListener>>>,
}

impl Win32EventBridge {
    fn start() -> Result<Self, String> {
        let (sender, receiver) = std::sync::mpsc::sync_channel(32);
        let listener = ClipboardUpdateListener::start(move |event| {
            // A full queue means the worker is behind; dropping a notification is correct
            // because the next one carries a newer sequence number.
            let _ = sender.try_send(event);
        })
        .map_err(|error| format!("clipboard listener: {error}"))?;
        Ok(Self {
            receiver,
            listener: Arc::new(Mutex::new(Some(listener))),
        })
    }

    /// Handle that unblocks the ingest worker when it fires.
    fn stop_signal(&self) -> ClipboardStopSignal {
        ClipboardStopSignal {
            listener: Arc::clone(&self.listener),
        }
    }
}

#[derive(Clone)]
struct ClipboardStopSignal {
    listener: Arc<Mutex<Option<ClipboardUpdateListener>>>,
}

impl StopSignal for ClipboardStopSignal {
    fn stop(&mut self) {
        let taken = self.listener.lock().ok().and_then(|mut slot| slot.take());
        // Drop outside the lock: the listener's own `Drop` joins its thread.
        drop(taken);
    }
}

impl ClipboardEventBridge for Win32EventBridge {
    fn next_event(&mut self) -> Option<ClipboardSignal> {
        match self.receiver.recv() {
            Ok(event) => Some(ClipboardSignal {
                sequence: event.sequence,
                owner: event.owner,
                foreground: event.foreground,
            }),
            Err(_) => None,
        }
    }
}

/// Resolves the source application through the Win32 owner/foreground strategy.
struct Win32SourceResolver;

impl SourceAppResolver for Win32SourceResolver {
    fn resolve(&self, signal: ClipboardSignal) -> Option<(String, String)> {
        source_app::resolve_source(source_app::SourceWindowSnapshot {
            owner: signal.owner,
            foreground: signal.foreground,
        })
        .map(|info| (info.display_name, info.exe_path))
    }
}

/// The recognition queue, deliberately empty.
///
/// Decision D2 keeps OCR out of this round: the engine lives in a crate that is still moving
/// fast, so the shell runs the pipeline **without** it. Refusing every job is the honest
/// version of that: the ingest service immediately releases the queued row, so its status goes
/// back to "none" instead of sitting in "queued" forever pretending a worker will come.
///
/// Replacing this with a real queue is the only change P3 needs in this file.
struct NoOcrQueue;

impl OcrQueue for NoOcrQueue {
    fn try_enqueue(&self, _clip_id: &str, _content_hash: &str) -> bool {
        false
    }
}

/// Start the pipeline and return its handle.
///
/// The caller **must** keep the returned handle alive for as long as the pipeline is wanted:
/// dropping it stops the listener and joins the worker.
pub fn start(store: Store, events: EventBus) -> Result<ClipboardIngestHandle, String> {
    let bridge = Win32EventBridge::start()?;
    // Captured before the bridge moves into the worker.
    let stop = bridge.stop_signal();
    let ingest = ClipboardIngest::new(
        store,
        bridge,
        Win32SourceResolver,
        Arc::new(NoOcrQueue),
        Arc::new(ClipboardEvents::new(events)) as Arc<dyn ClipboardEventSink>,
        ReadWindow::default(),
    );
    let thread = thread::Builder::new()
        .name("snapclip-clipboard-ingest".into())
        .spawn(move || ingest.run(Win32ClipboardSource))
        .map_err(|error| format!("clipboard ingest thread: {error}"))?;
    Ok(ClipboardIngestHandle::new(stop, thread))
}

#[cfg(test)]
mod tests {
    use snapclip_history::ingest::OcrQueue;

    /// D2, made checkable: the shell runs the pipeline with recognition switched off, and
    /// "off" means the ingest service's `try_enqueue` returns false so the row is released
    /// rather than left in `queued`.
    #[test]
    fn the_recognition_queue_says_no_so_the_row_is_released() {
        let queue = super::NoOcrQueue;
        assert!(!queue.try_enqueue("clip-1", "hash-1"));
    }

    /// The pipeline can be built, listens and stops without leaving a thread behind.
    ///
    /// Ignored by default: it opens the real clipboard listener (a window and a message loop)
    /// and touches the user's clipboard session. Run it on a desktop session with
    /// `cargo test -p snapclip-app --lib -- --ignored`.
    #[test]
    #[ignore = "opens the real clipboard listener and reads the real clipboard"]
    fn the_pipeline_starts_and_stops() {
        let root = std::env::temp_dir().join(format!(
            "snapclip-app-ingest-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let store = snapclip_history::store::Store::open(&root).expect("open store");
        let mut handle = super::start(store, crate::events::EventBus::new())
            .expect("the clipboard listener should start on a desktop session");
        // Stopping must unblock the worker's `recv()`; if it did not, this call would hang
        // rather than fail, which is exactly the bug the stop signal exists to prevent.
        handle.shutdown();
        let _ = std::fs::remove_dir_all(&root);
    }
}
