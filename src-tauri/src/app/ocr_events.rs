//! Adapter from the OCR worker's status exit to the versioned Tauri event.
//!
//! Keeps `ocr` free of Tauri while preserving the wire contract the front end reads:
//! event name `ocr-status-v1`, payload `OcrStatusChanged`
//! (`clipId`/`status`/`engine`/`errorCode`/`updatedAt`), wrapped in the shared
//! `EventEnvelope` (`schemaVersion` + `generation`). The two names are pinned by
//! `crate::events::ALL_EVENT_NAMES` and mirrored in `src/shared/contracts.ts`.

use crate::events::{OcrStatusChanged, OCR_STATUS_EVENT};
use crate::ocr::OcrEventSink;

/// Publishes OCR status changes as `ocr-status-v1`.
pub struct TauriOcrEventSink {
    app: tauri::AppHandle,
}

impl TauriOcrEventSink {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl OcrEventSink for TauriOcrEventSink {
    fn on_status(&self, clip_id: &str, status: &str, engine: &str, error_code: Option<&str>) {
        let updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        // Routed through the shared emitter so the name and the versioned envelope stay
        // in one place; a literal name here would silently drop when Tauri rejects it.
        crate::events::emit(
            &self.app,
            OCR_STATUS_EVENT,
            OcrStatusChanged {
                clip_id: clip_id.to_string(),
                status: status.to_string(),
                engine: engine.to_string(),
                error_code: error_code.map(str::to_string),
                updated_at,
            },
        );
    }
}
