//! The one owner of screenshot artifacts on disk (docs/23 T2.3).
//!
//! Takes a [`CaptureOutput`] (bytes + metadata), writes it atomically under a naming
//! scheme it owns, computes the `blake3` fingerprint **while writing**, and hands back an
//! [`ArtifactRef`] — so a later reader can identify the artifact without reading it again.
//!
//! Naming is part of the contract (`docs/11 §8.2`): `<session-id>-<sequence>.png`. The
//! sequence is per-process, which is what makes two captures in the same session land in
//! two files.
//!
//! **Not here yet**: cleanup/LRU. Nothing in the current code evicts artifacts, and this
//! task moved what exists rather than inventing a policy; when a retention rule is
//! decided, this store is where it belongs.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use snapclip_model::{ArtifactRef, CaptureOutput, ImageDimensions};

use crate::StoreError;

/// Owns where screenshots live, what they are called and how they get there.
#[derive(Debug, Clone)]
pub struct CaptureArtifactStore {
    root: PathBuf,
}

static ARTIFACT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl CaptureArtifactStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The directory this store writes into.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Write `output` atomically and describe what was written.
    pub fn write(&self, output: &CaptureOutput) -> Result<ArtifactRef, StoreError> {
        let metadata = &output.metadata;
        fs::create_dir_all(&self.root)?;
        let sequence = ARTIFACT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = self.root.join(format!("{}-{sequence}.png", metadata.session_id));
        write_atomically(&path, &output.bytes)?;
        Ok(ArtifactRef {
            absolute_path: path,
            mime: "image/png".to_string(),
            dimensions: ImageDimensions {
                width: metadata.width,
                height: metadata.height,
            },
            byte_len: output.bytes.len() as u64,
            content_fingerprint: blake3::hash(&output.bytes).to_hex().to_string(),
        })
    }

    /// Read an artifact back, checking it still is what the reference promised.
    ///
    /// The check is the reason the fingerprint is stored rather than recomputed from the
    /// reference's other fields: a file that was replaced or truncated is caught here and
    /// not three layers later.
    pub fn read(&self, reference: &ArtifactRef) -> Result<Vec<u8>, StoreError> {
        let bytes = fs::read(&reference.absolute_path)?;
        let actual = blake3::hash(&bytes).to_hex().to_string();
        if actual != reference.content_fingerprint {
            return Err(StoreError::InvalidPublication(format!(
                "artifact {} does not match its recorded fingerprint",
                reference.absolute_path.display()
            )));
        }
        Ok(bytes)
    }
}

/// Write through a temporary file so a crash can never leave a half-written artifact.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let temporary = path.with_extension("png.tmp");
    fs::write(&temporary, bytes)?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CaptureArtifactStore;
    use snapclip_model::{CaptureMetadata, CaptureOutput, PixelFormat};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "snapclip-artifact-{tag}-{}-{}",
            std::process::id(),
            TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        root
    }

    fn output(session: &str, bytes: Vec<u8>) -> CaptureOutput {
        CaptureOutput {
            bytes,
            metadata: CaptureMetadata {
                session_id: session.to_string(),
                width: 640,
                height: 480,
                dpi: 144,
                pixel_format: PixelFormat::Bgra8Unorm,
                captured_at_unix_ms: 1_700_000_000_000,
                monitor_device_name: Some(r"\\.\DISPLAY1".to_string()),
            },
        }
    }

    #[test]
    fn write_names_the_file_and_describes_it_truthfully() {
        let store = CaptureArtifactStore::new(root("write"));
        let payload = b"not really a png, but the store does not care".to_vec();
        let reference = store.write(&output("capture-1-1", payload.clone())).unwrap();

        // Naming contract (docs/11 §8.2) plus a real file on disk.
        let name = reference.absolute_path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("capture-1-1-"), "{name}");
        assert!(name.ends_with(".png"), "{name}");
        assert_eq!(fs::read(&reference.absolute_path).unwrap(), payload);

        assert_eq!(reference.mime, "image/png");
        assert_eq!(reference.byte_len, payload.len() as u64);
        assert_eq!((reference.dimensions.width, reference.dimensions.height), (640, 480));
        // The fingerprint is blake3 of the bytes, computed independently here.
        assert_eq!(
            reference.content_fingerprint,
            blake3::hash(&payload).to_hex().to_string()
        );
        // And no temporary file is left behind.
        let leftovers = fs::read_dir(store.root())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn two_writes_in_one_session_get_two_files() {
        let store = CaptureArtifactStore::new(root("unique"));
        let first = store.write(&output("capture-1-1", b"a".to_vec())).unwrap();
        let second = store.write(&output("capture-1-1", b"b".to_vec())).unwrap();
        assert_ne!(first.absolute_path, second.absolute_path);
        assert_eq!(fs::read(&first.absolute_path).unwrap(), b"a");
        assert_eq!(fs::read(&second.absolute_path).unwrap(), b"b");
        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn read_notices_a_file_that_no_longer_matches_its_fingerprint() {
        let store = CaptureArtifactStore::new(root("tamper"));
        let reference = store.write(&output("capture-1-1", b"original".to_vec())).unwrap();
        assert_eq!(store.read(&reference).unwrap(), b"original");

        fs::write(&reference.absolute_path, b"replaced").unwrap();
        let error = store.read(&reference).unwrap_err();
        assert!(matches!(error, crate::StoreError::InvalidPublication(_)), "{error:?}");
        let _ = fs::remove_dir_all(store.root());
    }
}
