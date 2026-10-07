//! Capture feature composition: builds the overlay controller, the frame providers,
//! the artifact writer and the Tauri event sink.
//!
//! This is the composition root the capture crate talks to through its ports: events go
//! out through the Tauri sink, clipboard writes through the system writer, and finished
//! selections through `HistoryArtifactWriter` (encode + store, both in
//! `snapclip-history`).

use std::sync::Arc;

use crate::app::artifact_writer::HistoryArtifactWriter;
use snapclip_capture::artifact::CaptureService;
use snapclip_capture::ports::{ArtifactWriter, CaptureEventSink, ClipboardWriter};
use snapclip_capture::runtime::CaptureRuntime;
use crate::domain::{CaptureArtifact, CaptureState};
use crate::events;

/// Publishes capture lifecycle events to the front-end.
pub struct TauriCaptureEventSink {
    app: tauri::AppHandle,
}

impl TauriCaptureEventSink {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl CaptureEventSink for TauriCaptureEventSink {
    fn on_started(&self, session_id: &str, layout: &snapclip_capture::geometry::MonitorLayout) {
        events::emit(
            &self.app,
            events::CAPTURE_STARTED_EVENT,
            events::CaptureStarted {
                session_id: session_id.to_string(),
                monitor_left: layout.bounds.left,
                monitor_top: layout.bounds.top,
                monitor_width: layout.bounds.width().max(0) as u32,
                monitor_height: layout.bounds.height().max(0) as u32,
                dpi: layout.dpi,
                // Filled in by the state event once the provider has produced a frame.
                provider: "pending".to_string(),
            },
        );
    }

    fn on_state(
        &self,
        session_id: &str,
        state: CaptureState,
        layout: Option<&snapclip_capture::geometry::MonitorLayout>,
    ) {
        events::emit(
            &self.app,
            events::CAPTURE_STATE_EVENT,
            events::CaptureStateChanged {
                session_id: session_id.to_string(),
                state: state.as_str().to_string(),
                dpi: layout.map(|layout| layout.dpi),
            },
        );
    }

    fn on_completed(&self, artifact: &CaptureArtifact) {
        events::emit(
            &self.app,
            events::CAPTURE_COMPLETED_EVENT,
            events::CaptureCompleted {
                session_id: artifact.session_id.clone(),
                artifact_ref: artifact
                    .png_path()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                width: artifact.width,
                height: artifact.height,
            },
        );
    }

    fn on_cancelled(&self, session_id: &str, reason: &str) {
        events::emit(
            &self.app,
            events::CAPTURE_CANCELLED_EVENT,
            events::CaptureCancelled {
                session_id: session_id.to_string(),
                reason: reason.to_string(),
            },
        );
    }

    fn on_failed(
        &self,
        session_id: Option<&str>,
        error: &snapclip_capture::CaptureError,
        provider: &str,
    ) {
        events::emit(
            &self.app,
            events::CAPTURE_FAILED_EVENT,
            events::CaptureFailed {
                session_id: session_id.map(str::to_string),
                error_code: error.error_code().as_str().to_string(),
                provider: provider.to_string(),
                message: error.to_string(),
            },
        );
    }
}

/// Start the capture feature and manage the runtime so commands can reach it.
pub fn start(
    app: tauri::AppHandle,
    app_local_data: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::Manager;

    // Per-Monitor V2 must be declared before the first window exists. The Tauri
    // window already exists at this point, but the overlay is created later and
    // inherits the process context.
    snapclip_capture::windows::monitor::set_per_monitor_v2_awareness()
        .map_err(|message| format!("DPI awareness: {message}"))?;

    let service = Arc::new(CaptureService::new());
    let sink: Arc<dyn CaptureEventSink> = Arc::new(TauriCaptureEventSink::new(app.clone()));
    let clipboard: Arc<dyn ClipboardWriter> =
        Arc::new(crate::app::clipboard_writer::SystemClipboardWriter);
    // Artifacts land in `<app local data>/artifacts/capture` (docs/11 §8.2); the writer
    // is the only thing that knows how to turn pixels into a file.
    let writer: Arc<dyn ArtifactWriter> = Arc::new(HistoryArtifactWriter::new(
        app_local_data.join("artifacts").join("capture"),
    ));

    let runtime = snapclip_capture::windows::overlay::WindowsOverlay::spawn_overlay(
        service,
        sink,
        clipboard,
        writer,
        // Behaviour is unchanged: `Default` is today's `DEFAULT_ADOPT_TEXT_RUNS`. T4.4's
        // settings channel supplies this value once it exists.
        snapclip_capture::window_detection::DetectionOptions::default(),
    )
    .map_err(|message| format!("capture overlay: {message}"))?;
    app.manage(Arc::new(CaptureRuntime::from_platform(Box::new(runtime))));
    Ok(())
}


