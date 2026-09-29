use serde::{Deserialize, Serialize};

pub const IPC_SCHEMA_VERSION: u16 = 1;

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
