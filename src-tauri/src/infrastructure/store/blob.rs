use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use super::StoreError;

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobRef {
    pub content_hash: String,
    pub size_bytes: u64,
    pub relative_path: String,
}

impl BlobStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn put(&self, bytes: &[u8]) -> Result<BlobRef, StoreError> {
        let content_hash = blake3::hash(bytes).to_hex().to_string();
        let relative_path = format!("{}/{}.blob", &content_hash[..2], content_hash);
        let destination = self.root.join(&relative_path);
        let parent = destination.parent().expect("blob path has a parent");
        fs::create_dir_all(parent)?;

        if !destination.exists() {
            let temporary = parent.join(format!(
                ".{}.tmp-{}-{}",
                content_hash,
                std::process::id(),
                TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            let write_result = write_new_file(&temporary, bytes);
            if let Err(error) = write_result {
                let _ = fs::remove_file(&temporary);
                return Err(error.into());
            }

            if let Err(error) = fs::rename(&temporary, &destination) {
                if destination.exists() {
                    fs::remove_file(&temporary)?;
                } else {
                    let _ = fs::remove_file(&temporary);
                    return Err(error.into());
                }
            }
        }

        let actual = verify_existing(&destination, &content_hash)?;
        if actual != bytes.len() as u64 {
            return Err(StoreError::InvalidPublication(
                "existing content-addressed blob does not match its hash".into(),
            ));
        }
        Ok(BlobRef {
            content_hash,
            size_bytes: actual,
            relative_path,
        })
    }

    pub fn read(&self, content_hash: &str) -> Result<Vec<u8>, StoreError> {
        if content_hash.len() != 64 || !content_hash.bytes().all(|value| value.is_ascii_hexdigit())
        {
            return Err(StoreError::InvalidPublication(
                "invalid BLAKE3 content hash".into(),
            ));
        }
        let content_hash = content_hash.to_ascii_lowercase();
        let path = self
            .root
            .join(&content_hash[..2])
            .join(format!("{content_hash}.blob"));
        let mut file = File::open(path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let actual_hash = blake3::hash(&bytes).to_hex().to_string();
        if actual_hash != content_hash {
            return Err(StoreError::InvalidPublication(
                "content-addressed blob failed hash verification".into(),
            ));
        }
        Ok(bytes)
    }

    pub fn remove_orphans(&self, referenced: &HashSet<String>) -> Result<usize, StoreError> {
        if !self.root.exists() {
            return Ok(0);
        }
        let mut removed = 0;
        for prefix in fs::read_dir(&self.root)? {
            let prefix = prefix?;
            if !prefix.file_type()?.is_dir() {
                continue;
            }
            let prefix_name = prefix.file_name();
            let prefix_name = prefix_name.to_string_lossy();
            if prefix_name.len() != 2 || !prefix_name.bytes().all(|value| value.is_ascii_hexdigit())
            {
                continue;
            }
            for entry in fs::read_dir(prefix.path())? {
                let entry = entry?;
                if !entry.file_type()?.is_file()
                    || entry.path().extension().is_none_or(|ext| ext != "blob")
                {
                    continue;
                }
                let relative_path =
                    format!("{}/{}", prefix_name, entry.file_name().to_string_lossy());
                if !referenced.contains(&relative_path) {
                    fs::remove_file(entry.path())?;
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }
}

fn write_new_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn verify_existing(path: &Path, expected_hash: &str) -> Result<u64, StoreError> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0; 64 * 1024];
    let mut size = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    if hasher.finalize().to_hex().as_str() != expected_hash {
        return Err(StoreError::InvalidPublication(
            "existing content-addressed blob failed hash verification".into(),
        ));
    }
    Ok(size)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::BlobStore;

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn content_is_deduplicated_and_verified_on_read() {
        let root = std::env::temp_dir().join(format!(
            "snapclip-blob-test-{}-{}",
            std::process::id(),
            TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let store = BlobStore::new(&root);
        let first = store.put(b"same bytes").unwrap();
        let second = store.put(b"same bytes").unwrap();
        assert_eq!(first, second);
        assert_eq!(store.read(&first.content_hash).unwrap(), b"same bytes");
        assert_eq!(
            fs::read_dir(root.join(&first.content_hash[..2]))
                .unwrap()
                .count(),
            1
        );
        let _ = fs::remove_dir_all(PathBuf::from(root));
    }

    #[test]
    fn removes_only_unreferenced_blob_files() {
        let root = std::env::temp_dir().join(format!(
            "snapclip-blob-gc-test-{}-{}",
            std::process::id(),
            TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let store = BlobStore::new(&root);
        let keep = store.put(b"referenced").unwrap();
        let orphan = store.put(b"orphan").unwrap();
        let referenced = [keep.relative_path].into_iter().collect();

        assert_eq!(store.remove_orphans(&referenced).unwrap(), 1);
        assert_eq!(store.read(&keep.content_hash).unwrap(), b"referenced");
        assert!(store.read(&orphan.content_hash).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
