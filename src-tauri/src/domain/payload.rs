use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// A semantic payload inside a [`crate::domain::Publication`].
///
/// The reference is storage-facing: it carries identity, size and kind, but never
/// the bytes themselves.
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

/// Payload bytes paired with their reference, produced by platform adapters and
/// consumed by the store writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadData {
    pub payload: PayloadRef,
    pub bytes: Vec<u8>,
}

impl PayloadData {
    pub fn new(payload: PayloadRef, bytes: Vec<u8>) -> Self {
        Self { payload, bytes }
    }
}

pub const MIME_TEXT: &str = "text/plain;charset=utf-8";
pub const MIME_HTML: &str = "text/html";
pub const MIME_RTF: &str = "text/rtf";
pub const MIME_FILES: &str = "application/json";
pub const MIME_PNG: &str = "image/png";

impl PayloadKind {
    pub fn default_mime_type(&self) -> &'static str {
        match self {
            Self::Text => MIME_TEXT,
            Self::Html => MIME_HTML,
            Self::Rtf => MIME_RTF,
            Self::Image => MIME_PNG,
            Self::Files => MIME_FILES,
            Self::Other => "application/octet-stream",
        }
    }
}
