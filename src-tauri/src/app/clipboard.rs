//! Composition of the clipboard ingest pipeline.
//!
//! Wires the Win32 listener/reader (platform), the ingest policy (application) and
//! the Tauri event emitter (this layer) together. The platform adapter never sees
//! the store, OCR or Tauri.

use std::sync::{Arc, Mutex};
use std::thread;

use crate::application::clipboard_ingest::{
    ClipboardEventSink, ClipboardIngest, ClipboardIngestHandle, ClipboardSignal, ClipboardSource,
    ClipboardSnapshot, ReadWindow, SourceAppResolver,
};
use crate::domain::Publication;
use crate::events;
use crate::infrastructure::store::Store;
use crate::ocr::OcrEnqueuer;
use crate::platform::windows::clipboard::{
    ClipboardEvent, ClipboardUpdateListener, formats, reader, source_app,
};

/// Reads snapshots through the Win32 clipboard adapter.
pub struct Win32ClipboardSource;

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
/// The listener sits behind a shared slot so the stop signal can drop it from the main
/// thread. Dropping [`ClipboardUpdateListener`] posts `WM_QUIT`, which ends the
/// listener thread and closes the channel, so the worker's `recv()` returns instead of
/// blocking forever.
pub struct Win32EventBridge {
    receiver: std::sync::mpsc::Receiver<ClipboardEvent>,
    listener: Arc<Mutex<Option<ClipboardUpdateListener>>>,
}

impl Win32EventBridge {
    pub fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let (sender, receiver) = std::sync::mpsc::sync_channel(32);
        let listener = ClipboardUpdateListener::start(move |event| {
            // A full queue means the worker is behind; dropping the notification is
            // correct because the next one carries a newer sequence number.
            let _ = sender.try_send(event);
        })?;
        Ok(Self {
            receiver,
            listener: Arc::new(Mutex::new(Some(listener))),
        })
    }

    /// Handle that unblocks the ingest worker when it fires.
    ///
    /// Owned by `ClipboardIngestHandle`, never by the worker itself, so releasing the
    /// app's handle is what stops the pipeline.
    pub fn stop_signal(&self) -> ClipboardStopSignal {
        ClipboardStopSignal {
            listener: self.listener.clone(),
        }
    }
}

/// Releases the clipboard listener so the ingest worker's `recv()` returns.
#[derive(Clone)]
pub struct ClipboardStopSignal {
    listener: Arc<Mutex<Option<ClipboardUpdateListener>>>,
}

impl crate::application::clipboard_ingest::StopSignal for ClipboardStopSignal {
    fn stop(&mut self) {
        let taken = self.listener.lock().ok().and_then(|mut slot| slot.take());
        // Drop outside the lock: the listener's own `Drop` joins the listener thread.
        drop(taken);
    }
}

impl crate::application::clipboard_ingest::ClipboardEventBridge for Win32EventBridge {
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
pub struct Win32SourceResolver;

impl SourceAppResolver for Win32SourceResolver {
    fn resolve(&self, signal: ClipboardSignal) -> Option<(String, String)> {
        source_app::resolve_source(source_app::SourceWindowSnapshot {
            owner: signal.owner,
            foreground: signal.foreground,
        })
        .map(|info| (info.display_name, info.exe_path))
    }
}

/// Publishes a versioned Tauri event after every persisted publication.
pub struct TauriClipboardEventSink {
    app: tauri::AppHandle,
}

impl TauriClipboardEventSink {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl ClipboardEventSink for TauriClipboardEventSink {
    fn on_publication_saved(&self, publication: &Publication) {
        events::emit(
            &self.app,
            events::CLIPBOARD_UPDATED_EVENT,
            ClipboardUpdated {
                publication_id: publication.publication_id.clone(),
                origin: publication.origin.as_str().to_string(),
                payload_kinds: publication
                    .payloads
                    .iter()
                    .map(|payload| payload.kind.clone())
                    .collect(),
            },
        );
    }
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ClipboardUpdated {
    publication_id: String,
    origin: String,
    payload_kinds: Vec<crate::domain::PayloadKind>,
}

/// Start the clipboard pipeline and return its handle.
///
/// The caller **must** keep the returned handle alive for as long as the pipeline is
/// wanted: dropping it stops the pipeline, and dropping it immediately after this call
/// (for example by ignoring the return value) would both kill the listener and block
/// here while joining the worker.
pub fn start(
    app: tauri::AppHandle,
    store: Store,
    ocr: OcrEnqueuer,
) -> Result<ClipboardIngestHandle, Box<dyn std::error::Error>> {
    let bridge = Win32EventBridge::start()?;
    // Captured before the bridge moves into the worker.
    let stop = bridge.stop_signal();
    let ingest = ClipboardIngest::new(
        store,
        bridge,
        Win32SourceResolver,
        Arc::new(ocr),
        Arc::new(TauriClipboardEventSink::new(app)),
        ReadWindow::default(),
    );
    let thread = thread::Builder::new()
        .name("snapclip-clipboard-ingest".into())
        .spawn(move || ingest.run(Win32ClipboardSource))?;
    Ok(ClipboardIngestHandle::new(stop, thread))
}




