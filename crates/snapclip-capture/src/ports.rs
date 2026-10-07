//! The contracts the overlay needs from the rest of the process.
//!
//! Nothing here knows about the clipboard, SQLite, OCR or Tauri. Implementations are
//! supplied by the composition root. (The runtime that owns the platform controller is
//! a sibling: [`crate::runtime`].)

use snapclip_model::CaptureState;

use super::annotation::AnnotationCommand;
use super::geometry::MonitorLayout;
use super::{CaptureError, CaptureResult};

pub use crate::artifact::{
    CaptureService, PixelSliceSource, SelectionPixels,
};

/// Receives low-frequency lifecycle events. Deliberately a trait so the capture
/// module never depends on Tauri's emitter.
pub trait CaptureEventSink: Send + Sync + 'static {
    fn on_started(&self, session_id: &str, layout: &MonitorLayout);
    fn on_state(&self, session_id: &str, state: CaptureState, layout: Option<&MonitorLayout>);
    fn on_completed(&self, artifact: &snapclip_model::CaptureArtifact);
    fn on_cancelled(&self, session_id: &str, reason: &str);
    fn on_failed(&self, session_id: Option<&str>, error: &CaptureError, provider: &str);
}

/// A sink that drops everything. Useful for tests and headless runs.
pub struct NullEventSink;

impl CaptureEventSink for NullEventSink {
    fn on_started(&self, _session_id: &str, _layout: &MonitorLayout) {}
    fn on_state(&self, _session_id: &str, _state: CaptureState, _layout: Option<&MonitorLayout>) {}
    fn on_completed(&self, _artifact: &snapclip_model::CaptureArtifact) {}
    fn on_cancelled(&self, _session_id: &str, _reason: &str) {}
    fn on_failed(&self, _session_id: Option<&str>, _error: &CaptureError, _provider: &str) {}
}

/// The overlay/presentation implementation for the current platform.
///
/// `Send + Sync` is required because the rest of the process drives sessions from
/// Tauri command threads; implementations are responsible for marshalling work onto
/// their own UI thread.
pub trait OverlayPlatform: Send + Sync + 'static {
    fn state(&self) -> CaptureState;

    /// `true` when a new session was accepted, `false` when one is already active.
    fn request_start(&self) -> CaptureResult<bool>;

    fn request_cancel(&self) -> CaptureResult<()>;

    fn request_confirm(&self) -> CaptureResult<()>;

    /// Deliver one low-frequency toolbar instruction to the running session's
    /// annotation document. The Vue toolbar emits at most one per click, never on
    /// mouse-move or per pixel (docs/11 §7.1 "工具栏不进入像素管线"). The default
    /// rejects the call so only platforms with a real overlay need to override it.
    fn request_annotation(&self, command: AnnotationCommand) -> CaptureResult<()> {
        let _ = command;
        Err(CaptureError::Unsupported)
    }

    fn shutdown(&self);
}

/// Writes short text to the system clipboard on behalf of the overlay.
///
/// The overlay has exactly one clipboard affordance: pressing `C` copies the colour
/// value under the magnifier. Owning the clipboard is the composition root's job,
/// because whoever writes to it also has to mark the write as "ours" so SnapClip's own
/// clip monitor does not record it. Capture therefore declares the need and the shell
/// implements it — the alternative (capture calling into the clipboard module, as it
/// did before this port existed) is a dependency the seam must not have.
pub trait ClipboardWriter: Send + Sync + 'static {
    /// Copy `text`. Failures are the implementation's to log; they are never fatal.
    fn copy_text(&self, text: &str);
}

/// Turns a prepared selection into a durable artifact.
///
/// Owned by the composition root, because this is where encoding and writing meet: the
/// store that writes PNGs lives in `snapclip-history`, and capture must not depend on it
/// (docs/23 T2.4, and the dependency gate enforces exactly that). Capture's job ends at
/// "here are the pixels the user selected, and the frame they came from".
///
/// Implementations run on the export worker thread, never on the overlay thread.
pub trait ArtifactWriter: Send + Sync + 'static {
    fn write(
        &self,
        session_id: &str,
        prepared: &crate::artifact::SelectionPixels,
        dpi: u32,
        monitor_device_name: Option<String>,
    ) -> crate::CaptureResult<snapclip_model::CaptureArtifact>;
}
