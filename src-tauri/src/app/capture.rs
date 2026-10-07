//! Capture feature composition: builds the overlay controller, the frame providers,
//! the artifact service and the Tauri event sink.
//!
//! The event sink is the only place where capture meets Tauri. Everything below it
//! works with domain types.

use std::path::PathBuf;
use std::sync::Arc;

use crate::application::capture_service::PngArtifactEncoder;
use snapclip_capture::artifact::{ArtifactDir, CaptureService};
use snapclip_capture::ports::{CaptureEventSink, ClipboardWriter};
use snapclip_capture::runtime::CaptureRuntime;
use crate::domain::{CaptureArtifact, CaptureState};
use crate::events;

/// Artifact directory: `<app local data>/artifacts/capture`.
pub struct AppArtifactDir {
    root: PathBuf,
}

impl AppArtifactDir {
    pub fn new(app_local_data: impl Into<PathBuf>) -> Self {
        Self {
            root: app_local_data.into(),
        }
    }
}

impl ArtifactDir for AppArtifactDir {
    fn artifact_dir(&self) -> PathBuf {
        self.root.join("artifacts").join("capture")
    }
}

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

    let artifacts = AppArtifactDir::new(app_local_data);
    let service = Arc::new(CaptureService::new(artifacts, PngArtifactEncoder));
    let sink: Arc<dyn CaptureEventSink> = Arc::new(TauriCaptureEventSink::new(app.clone()));
    let clipboard: Arc<dyn ClipboardWriter> =
        Arc::new(crate::app::clipboard_writer::SystemClipboardWriter);

    let runtime = snapclip_capture::windows::overlay::WindowsOverlay::spawn_overlay(
        service,
        sink,
        clipboard,
    )
    .map_err(|message| format!("capture overlay: {message}"))?;
    app.manage(Arc::new(CaptureRuntime::from_platform(Box::new(runtime))));
    Ok(())
}


