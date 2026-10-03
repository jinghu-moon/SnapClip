//! Versioned event envelopes and their directed publication.
//!
//! Every event carries `schemaVersion`, a `sessionId` (or `null`) and a monotonic
//! `generation` so the front-end can drop stale messages. Events never carry image
//! data — only ids, sizes and states.
//!
//! Event names use only `[a-z0-9-]`. Tauri validates event names and rejects anything
//! outside a narrow whitelist, so punctuation such as `.` or a `scheme://` prefix must
//! not appear here: a rejected name makes [`emit`] fail at runtime and the event is
//! simply lost. The same names are duplicated in `src/shared/contracts.ts` and the two
//! copies must stay in sync.

use serde::Serialize;
use tauri::Emitter;

use crate::domain::IPC_SCHEMA_VERSION;

pub const CAPTURE_STARTED_EVENT: &str = "capture-started-v1";
pub const CAPTURE_STATE_EVENT: &str = "capture-state-v1";
pub const CAPTURE_COMPLETED_EVENT: &str = "capture-completed-v1";
pub const CAPTURE_CANCELLED_EVENT: &str = "capture-cancelled-v1";
pub const CAPTURE_FAILED_EVENT: &str = "capture-failed-v1";
pub const CLIPBOARD_UPDATED_EVENT: &str = "clipboard-updated-v1";
pub const OCR_STATUS_EVENT: &str = "ocr-status-v1";

/// Every event name this crate can emit, for the contract test.
#[cfg(test)]
pub const ALL_EVENT_NAMES: &[&str] = &[
    CAPTURE_STARTED_EVENT,
    CAPTURE_STATE_EVENT,
    CAPTURE_COMPLETED_EVENT,
    CAPTURE_CANCELLED_EVENT,
    CAPTURE_FAILED_EVENT,
    CLIPBOARD_UPDATED_EVENT,
    OCR_STATUS_EVENT,
];

/// Monotonic per-process event counter.
fn next_generation() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static GENERATION: AtomicU64 = AtomicU64::new(0);
    GENERATION.fetch_add(1, Ordering::Relaxed) + 1
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventEnvelope<T> {
    pub schema_version: u16,
    pub generation: u64,
    pub payload: T,
}

impl<T> EventEnvelope<T> {
    pub fn new(payload: T) -> Self {
        Self {
            schema_version: IPC_SCHEMA_VERSION,
            generation: next_generation(),
            payload,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStarted {
    pub session_id: String,
    pub monitor_left: i32,
    pub monitor_top: i32,
    pub monitor_width: u32,
    pub monitor_height: u32,
    pub dpi: u32,
    pub provider: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStateChanged {
    pub session_id: String,
    pub state: String,
    pub dpi: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureCompleted {
    pub session_id: String,
    pub artifact_ref: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureCancelled {
    pub session_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureFailed {
    pub session_id: Option<String>,
    pub error_code: String,
    pub provider: String,
    pub message: String,
}

/// OCR progress for one clip.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrStatusChanged {
    pub clip_id: String,
    pub status: String,
    pub engine: String,
    pub error_code: Option<String>,
    pub updated_at: i64,
}

/// Emit a versioned event, ignoring listeners that have already gone away.
///
/// A failure is logged but never propagated: telemetry must not take down the feature
/// that produced it.
pub fn emit<T: Serialize + Clone>(app: &tauri::AppHandle, event: &str, payload: T) {
    let envelope = EventEnvelope::new(payload);
    if let Err(error) = app.emit(event, envelope) {
        eprintln!("[snapclip][events] emit {event} failed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::ALL_EVENT_NAMES;

    /// Tauri rejects event names containing characters outside a narrow whitelist, and
    /// a rejected name silently drops the event — which is exactly how the capture
    /// lifecycle events were lost. This crate is therefore stricter than Tauri: only
    /// lowercase letters, digits and `-` are allowed, which every Tauri 2.x build
    /// accepts.
    #[test]
    fn every_event_name_is_accepted_by_tauri() {
        for name in ALL_EVENT_NAMES {
            assert!(!name.is_empty(), "event name must not be empty");
            for ch in name.chars() {
                assert!(
                    ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-',
                    "event name {name:?} contains {ch:?}; only [a-z0-9-] is allowed"
                );
            }
        }
    }

    /// The back-end and the front-end must agree on the names, or listeners silently
    /// never fire. The TypeScript copy lives in `src/shared/contracts.ts`.
    #[test]
    fn every_event_name_is_declared_in_the_frontend_contracts() {
        let contracts = include_str!("../../../src/shared/contracts.ts");
        for name in ALL_EVENT_NAMES {
            assert!(
                contracts.contains(&format!("\"{name}\"")),
                "event name {name:?} is missing from src/shared/contracts.ts"
            );
        }
    }
}


