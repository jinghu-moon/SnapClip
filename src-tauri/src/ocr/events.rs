//! Framework-free status exit for the OCR worker.
//!
//! The worker must not know where its status changes go. It publishes through this
//! trait and the composition root decides how they leave the process: today a
//! versioned Tauri event (`ocr-status-v1`, see `app::ocr_events`), later whatever the
//! GPUI shell uses. The event name, the payload fields and the timestamp are the
//! adapter's business, not the worker's — which is what keeps this seam stable when
//! the shell changes.

/// A status change for one clip's recognition job.
///
/// `error_code` is the machine-readable code (`decode_failed`, `timeout`, …) or `None`
/// for a successful or merely skipped job.
pub trait OcrEventSink: Send + Sync + 'static {
    fn on_status(&self, clip_id: &str, status: &str, engine: &str, error_code: Option<&str>);
}

// A `NullOcrEventSink` will come back with its first consumer (docs/23 T3.1, where the
// recognize crate gets headless tests). Until then it would be an unused stub, and this
// project's gates treat warnings as failures.
