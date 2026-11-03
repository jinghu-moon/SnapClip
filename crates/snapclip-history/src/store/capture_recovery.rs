//! A bounded handoff record between an immutable capture file and SQLite.
//! Failed registrations retain both; replay never scans or deletes source files.
use super::*;
use snapclip_model::CaptureSource;
use std::io::{Read, Write};

const MAX_RECORD_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    id: String,
    source: CaptureSource,
    thumbnail: Vec<u8>,
}

pub(super) struct CaptureRecovery {
    directory: PathBuf,
}

impl CaptureRecovery {
    pub(super) fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    fn path(&self, id: &str) -> PathBuf {
        self.directory
            .join(format!("{}.json", blake3::hash(id.as_bytes()).to_hex()))
    }

    fn read(&self, path: &Path) -> Result<Record, StoreError> {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.file_type().is_file() || metadata.len() > MAX_RECORD_BYTES {
            return Err(StoreError::InvalidPublication(
                "capture recovery record is not a bounded regular file".into(),
            ));
        }
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take(MAX_RECORD_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(StoreError::InvalidPublication(
                "capture recovery record exceeds its budget".into(),
            ));
        }
        let record: Record = serde_json::from_slice(&bytes)
            .map_err(|e| StoreError::InvalidPublication(e.to_string()))?;
        if record.version != 1 || self.path(&record.id) != path {
            return Err(StoreError::InvalidPublication(
                "capture recovery version or file identity is invalid".into(),
            ));
        }
        Ok(record)
    }

    pub(super) fn stage(
        &self,
        id: &str,
        source: &CaptureSource,
        thumbnail: &[u8],
    ) -> Result<bool, StoreError> {
        let record = Record {
            version: 1,
            id: id.into(),
            source: source.clone(),
            thumbnail: thumbnail.to_vec(),
        };
        let path = self.path(id);
        if path.try_exists()? {
            if self.read(&path)? != record {
                return Err(StoreError::InvalidPublication(
                    "capture id already has a different pending registration".into(),
                ));
            }
            return Ok(false);
        }
        let bytes = serde_json::to_vec(&record)
            .map_err(|e| StoreError::InvalidPublication(e.to_string()))?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(StoreError::InvalidPublication(
                "capture recovery record exceeds its budget".into(),
            ));
        }
        fs::create_dir_all(&self.directory)?;
        let temporary = path.with_extension("tmp");
        // Only clean a temporary file this call actually created. A stale or
        // concurrent writer's file is evidence, not ours to delete.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let written = (|| {
            file.write_all(&bytes)?;
            file.sync_all()
        })();
        drop(file);
        if let Err(error) = written.and_then(|()| fs::rename(&temporary, &path)) {
            let _ = fs::remove_file(&temporary);
            return Err(error.into());
        }
        Ok(true)
    }

    /// Clear only the matching handoff, after commit or before explicit deletion.
    /// Failure is reported: leaving an unnoticed record could resurrect a row.
    pub(super) fn finish(&self, id: &str, source: &CaptureSource) -> Result<(), StoreError> {
        let path = self.path(id);
        if path.try_exists()? {
            if self.read(&path)?.source != *source {
                return Err(StoreError::InvalidPublication(
                    "capture recovery record conflicts with the registered source".into(),
                ));
            }
            fs::remove_file(path)?;
        }
        Ok(())
    }

    pub(super) fn recover(
        &self,
        connection: &Connection,
        blobs: &BlobStore,
        artifacts: &crate::artifact_store::CaptureArtifactStore,
    ) -> Result<(), StoreError> {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        // One record at a time. Invalid or still-unwritable records remain on
        // disk and do not prevent other captures or ordinary history from opening.
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let result = self.read(&path).and_then(|record| {
                super::capture_repository::insert_capture(
                    connection,
                    blobs,
                    artifacts,
                    self,
                    record.id,
                    record.source,
                    record.thumbnail,
                )
            });
            if let Err(error) = result {
                eprintln!(
                    "[history] retained pending capture {}: {error}",
                    path.display()
                );
            }
        }
        Ok(())
    }
}
