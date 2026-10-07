//! Artifact references and the capture output hand-over (docs/23 T2.1).
//!
//! These two types exist so the three layers can stop guessing about each other:
//!
//! * [`CaptureOutput`] is what a capture session **hands over** — encoded bytes plus the
//!   metadata they were produced from. Nothing in it is a path, so the capture crate
//!   never decides where an artifact lives.
//! * [`ArtifactRef`] is what a store **hands back** after it wrote those bytes — where the
//!   file is, what it is, how big, and a `blake3` fingerprint computed **at write time**
//!   so readers never have to re-read a file to identify it.
//!
//! Since T2.4 the write path is the shell's `ArtifactWriter` port over
//! `snapclip-history`'s `CaptureArtifactStore`: capture hands over pixels, history encodes
//! and writes, and `CaptureArtifact` (the domain result the rest of the app reads) is
//! built from the `ArtifactRef` that comes back.

use std::path::PathBuf;

use crate::geometry::ImageDimensions;

/// Everything about a finished capture that is not the pixels.
///
/// Deliberately the same fields `CaptureArtifact` carries today, minus its payload: the
/// payload is the bytes, and this is what is true about them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureMetadata {
    pub session_id: String,
    pub width: u32,
    pub height: u32,
    pub dpi: u32,
    pub pixel_format: crate::capture::PixelFormat,
    pub captured_at_unix_ms: i64,
    /// Absent when the capture was not tied to one display (kept from today's model).
    pub monitor_device_name: Option<String>,
}

/// What a capture session hands to whoever owns storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureOutput {
    /// Encoded artifact bytes (PNG today).
    pub bytes: Vec<u8>,
    pub metadata: CaptureMetadata,
}

/// Where an artifact lives, plus enough about it to use it without touching the file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRef {
    /// Absolute path of the artifact on disk.
    pub absolute_path: PathBuf,
    /// Media type of the bytes at that path (`image/png` today).
    pub mime: String,
    pub dimensions: ImageDimensions,
    pub byte_len: u64,
    /// `blake3` hex digest of the bytes, computed when they were written.
    ///
    /// Computing it at write time is the whole point: a reader that needs to know whether
    /// two artifacts are the same must not have to read either file.
    pub content_fingerprint: String,
}

#[cfg(test)]
mod tests {
    use super::{ArtifactRef, CaptureMetadata, CaptureOutput};
    use crate::capture::PixelFormat;
    use crate::geometry::ImageDimensions;
    use std::path::PathBuf;

    fn metadata() -> CaptureMetadata {
        CaptureMetadata {
            session_id: "capture-1-1".into(),
            width: 320,
            height: 200,
            dpi: 144,
            pixel_format: PixelFormat::Bgra8Unorm,
            captured_at_unix_ms: 1_700_000_000_000,
            monitor_device_name: Some(r"\\.\DISPLAY1".into()),
        }
    }

    #[test]
    fn capture_output_carries_bytes_and_metadata_and_no_path() {
        let output = CaptureOutput {
            bytes: vec![1, 2, 3],
            metadata: metadata(),
        };
        // The compile-time half of the contract: a capture hand-over cannot name a path,
        // because deciding where artifacts live belongs to the store.
        assert_eq!(output.bytes.len(), 3);
        assert_eq!(output.metadata.width, 320);
        assert_eq!(output.metadata.monitor_device_name.as_deref(), Some(r"\\.\DISPLAY1"));
    }

    #[test]
    fn artifact_ref_survives_a_round_trip_through_json() {
        // The ref crosses into the front end (history rows), so its wire shape is a
        // contract: camelCase field names, and the path as a plain string.
        let reference = ArtifactRef {
            absolute_path: PathBuf::from(r"C:\artifacts\capture-1-1.png"),
            mime: "image/png".into(),
            dimensions: ImageDimensions {
                width: 320,
                height: 200,
            },
            byte_len: 4096,
            content_fingerprint: "b3:0123456789abcdef".into(),
        };
        let json = serde_json::to_string(&reference).unwrap();
        assert!(json.contains("\"absolutePath\""), "{json}");
        assert!(json.contains("\"byteLen\":4096"), "{json}");
        assert!(json.contains("\"contentFingerprint\""), "{json}");
        let back: ArtifactRef = serde_json::from_str(&json).unwrap();
        assert_eq!(back, reference);
    }
}
