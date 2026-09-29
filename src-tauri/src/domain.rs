use serde::{Deserialize, Serialize};

pub const IPC_SCHEMA_VERSION: u16 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PayloadKind {
    Text,
    Html,
    Rtf,
    Image,
    Files,
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OcrStatus {
    None,
    Queued,
    Running,
    Done,
    Failed,
    Skipped,
}

impl OcrStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "done" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            "skipped" => Some(Self::Skipped),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OcrErrorCode {
    LanguageUnavailable,
    DecodeFailed,
    Timeout,
    Cancelled,
    EngineFailed,
}

impl OcrErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LanguageUnavailable => "language_unavailable",
            Self::DecodeFailed => "decode_failed",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::EngineFailed => "engine_failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "language_unavailable" => Some(Self::LanguageUnavailable),
            "decode_failed" => Some(Self::DecodeFailed),
            "timeout" => Some(Self::Timeout),
            "cancelled" => Some(Self::Cancelled),
            "engine_failed" => Some(Self::EngineFailed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageDimensions {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PayloadRef {
    pub payload_id: String,
    pub content_hash: String,
    pub kind: PayloadKind,
    pub size_bytes: u64,
    pub mime_type: Option<String>,
    pub image_dimensions: Option<ImageDimensions>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardPublication {
    pub publication_id: String,
    pub captured_at_unix_ms: i64,
    pub source_app: Option<String>,
    pub source_exe_path: Option<String>,
    pub payloads: Vec<PayloadRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClipSummary {
    pub id: String,
    pub created_at_unix_ms: i64,
    pub primary_kind: PayloadKind,
    pub preview_text: Option<String>,
    pub source_app: Option<String>,
    pub source_exe_path: Option<String>,
    pub thumbnail: Option<PayloadRef>,
    pub payloads: Vec<PayloadRef>,
    pub ocr_status: OcrStatus,
    pub ocr_text: Option<String>,
    pub ocr_engine: Option<String>,
    pub ocr_updated_at: Option<i64>,
    pub ocr_error_code: Option<OcrErrorCode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    pub items: Vec<ClipSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidArgument,
    NotFound,
    Conflict,
    Unsupported,
    Cancelled,
    Storage,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: ErrorCode,
    pub message: Option<String>,
    pub trace_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{ClipboardPublication, PayloadKind, PayloadRef};
    use serde_json::json;

    #[test]
    fn publication_serializes_with_stable_json_field_names() {
        let publication = ClipboardPublication {
            publication_id: "publication-1".into(),
            captured_at_unix_ms: 42,
            source_app: None,
            source_exe_path: None,
            payloads: vec![PayloadRef {
                payload_id: "payload-1".into(),
                content_hash: "abc123".into(),
                kind: PayloadKind::Image,
                size_bytes: 1024,
                mime_type: Some("image/png".into()),
                image_dimensions: None,
            }],
        };

        let value = serde_json::to_value(publication).unwrap();
        assert_eq!(value["publicationId"], "publication-1");
        assert_eq!(value["capturedAtUnixMs"], 42);
        assert_eq!(value["payloads"][0]["payloadId"], "payload-1");
        assert_eq!(value["payloads"][0]["kind"], "image");
        assert_eq!(value["payloads"][0]["sizeBytes"], json!(1024));
    }
}
