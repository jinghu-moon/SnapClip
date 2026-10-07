//! The history read model: what one row of the history UI needs.

use crate::payload::{PayloadKind, PayloadRef};
use crate::publication::PublicationOrigin;
use crate::recognition::{OcrErrorCode, OcrStatus};

/// One history row as shown to the user.
///
/// It carries everything a row needs, so the front-end never has to compose payload, OCR
/// and source metadata itself. Serialised to the front end and mirrored in
/// `src/shared/contracts.ts`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClipSummary {
    pub id: String,
    pub created_at_unix_ms: i64,
    pub origin: PublicationOrigin,
    pub primary_kind: PayloadKind,
    pub preview_text: Option<String>,
    pub source_app: Option<String>,
    pub source_exe_path: Option<String>,
    pub thumbnail: Option<PayloadRef>,
    pub payloads: Vec<PayloadRef>,
    pub ocr_status: OcrStatus,
    pub ocr_text: Option<String>,
    pub ocr_layout: Option<String>,
    pub ocr_engine: Option<String>,
    pub ocr_updated_at: Option<i64>,
    pub ocr_error_code: Option<OcrErrorCode>,
}

/// One page of history, newest first.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    pub items: Vec<ClipSummary>,
    pub next_cursor: Option<String>,
}
