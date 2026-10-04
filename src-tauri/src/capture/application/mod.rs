//! Capture application layer: the contracts the overlay needs from the rest of the
//! process, and the runtime that owns the platform controller.
//!
//! Nothing here knows about the clipboard, SQLite, OCR or Tauri. Implementations
//! are supplied by the composition root (see `crate::app`).

pub mod runtime;

use crate::domain::CaptureState;

use super::annotation::AnnotationCommand;
use super::geometry::MonitorLayout;
use super::{CaptureError, CaptureResult};

pub use crate::application::capture_service::{
    ArtifactDir, ArtifactEncoder, CaptureService, PixelSliceSource, PngArtifactEncoder,
    SelectionPixels,
};

/// Receives low-frequency lifecycle events. Deliberately a trait so the capture
/// module never depends on Tauri's emitter.
pub trait CaptureEventSink: Send + Sync + 'static {
    fn on_started(&self, session_id: &str, layout: &MonitorLayout);
    fn on_state(&self, session_id: &str, state: CaptureState, layout: Option<&MonitorLayout>);
    fn on_completed(&self, artifact: &crate::domain::CaptureArtifact);
    fn on_cancelled(&self, session_id: &str, reason: &str);
    fn on_failed(&self, session_id: Option<&str>, error: &CaptureError, provider: &str);
}

/// A sink that drops everything. Useful for tests and headless runs.
pub struct NullEventSink;

impl CaptureEventSink for NullEventSink {
    fn on_started(&self, _session_id: &str, _layout: &MonitorLayout) {}
    fn on_state(&self, _session_id: &str, _state: CaptureState, _layout: Option<&MonitorLayout>) {}
    fn on_completed(&self, _artifact: &crate::domain::CaptureArtifact) {}
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
