//! One observation of user-visible content, and where it came from.

use crate::payload::PayloadRef;

/// Where a publication came from. Keeps clipboard-specific vocabulary out of the storage
/// contract while still allowing per-origin policy in the application layer.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PublicationOrigin {
    Clipboard,
    Capture,
}

impl PublicationOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clipboard => "clipboard",
            Self::Capture => "capture",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "clipboard" => Some(Self::Clipboard),
            "capture" => Some(Self::Capture),
            _ => None,
        }
    }
}

/// A single observation of user-visible content: one clipboard snapshot or one finished
/// capture. Generic on purpose — the store must never learn whether a publication came
/// from the clipboard listener or the capture overlay.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Publication {
    pub publication_id: String,
    pub origin: PublicationOrigin,
    pub captured_at_unix_ms: i64,
    pub source_app: Option<String>,
    pub source_exe_path: Option<String>,
    pub payloads: Vec<PayloadRef>,
}

impl Publication {
    /// The payload that stands for this publication in a one-line summary.
    ///
    /// "First payload wins" is a single shared rule, not a storage detail: the store writes
    /// it into `clips.primary_kind` and the event bridge reports it as the clip's kind, and
    /// a UI that said "image" for a row the store called "text" would be lying to the user.
    pub fn primary_payload(&self) -> Option<&PayloadRef> {
        self.payloads.first()
    }
}

#[cfg(test)]
mod tests {
    use super::{Publication, PublicationOrigin};
    use crate::payload::{PayloadKind, PayloadRef};

    fn sample_publication(id: &str) -> Publication {
        Publication {
            publication_id: id.into(),
            origin: PublicationOrigin::Clipboard,
            captured_at_unix_ms: 42,
            source_app: None,
            source_exe_path: None,
            payloads: vec![PayloadRef {
                payload_id: format!("{id}-0"),
                content_hash: "abc123".into(),
                kind: PayloadKind::Image,
                size_bytes: 1024,
                mime_type: Some("image/png".into()),
                image_dimensions: None,
            }],
        }
    }

    #[test]
    fn publication_serializes_with_stable_json_field_names() {
        let value = serde_json::to_value(sample_publication("publication-1")).unwrap();
        assert_eq!(value["publicationId"], "publication-1");
        assert_eq!(value["origin"], "clipboard");
        assert_eq!(value["capturedAtUnixMs"], 42);
        assert_eq!(value["payloads"][0]["payloadId"], "publication-1-0");
        assert_eq!(value["payloads"][0]["kind"], "image");
        assert_eq!(value["payloads"][0]["sizeBytes"], 1024);
    }

    #[test]
    fn publication_origin_round_trips() {
        for origin in [PublicationOrigin::Clipboard, PublicationOrigin::Capture] {
            assert_eq!(PublicationOrigin::parse(origin.as_str()), Some(origin));
        }
    }
}
