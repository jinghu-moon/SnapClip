//! Database/file handoff recovery; no window, GPU or system clipboard is used.
use snapclip_history::{
    artifact_store::CaptureArtifactStore, pixel_master::PixelMaster, store::Store,
};
use snapclip_model::{CaptureMetadata, CaptureSource, pixels::*};
use std::{fs, path::PathBuf};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "capture-recovery-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn database(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.0.join("data/snapclip.db")).unwrap()
    }
    fn journal(&self, id: &str) -> PathBuf {
        self.0
            .join("artifacts/pending-captures")
            .join(format!("{}.json", blake3::hash(id.as_bytes()).to_hex()))
    }
    fn fail_registration(&self) {
        self.database()
            .execute_batch(
                "CREATE TRIGGER reject_capture BEFORE INSERT ON capture_sources
            BEGIN SELECT RAISE(ABORT, 'injected registration failure'); END;",
            )
            .unwrap();
    }
    fn allow_registration(&self) {
        self.database()
            .execute_batch("DROP TRIGGER reject_capture;")
            .unwrap();
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn source(root: &Directory, hdr: bool) -> CaptureSource {
    let descriptor = if hdr {
        PixelDescriptor::new(
            snapclip_model::PixelFormat::Rgba16Float,
            PixelEncoding::ScRgb,
            AlphaMode::Straight,
        )
        .unwrap()
    } else {
        PixelDescriptor::bgra8()
    };
    let metadata = CaptureMetadata {
        session_id: "recover".into(),
        width: 2,
        height: 1,
        dpi: 144,
        source: PixelSource::new(descriptor, 91),
        captured_at_unix_ms: 42,
        monitor_device_name: Some("display-test".into()),
    };
    let artifacts = CaptureArtifactStore::new(root.0.join("artifacts/capture"));
    let artifact = if hdr {
        artifacts
            .write_pixel_master(&metadata, |_, _| {
                Ok(PixelBuffer::from_tight(
                    2,
                    1,
                    descriptor,
                    [0x00, 0xb4, 0x40, 0x4a, 0x00, 0x38, 0x00, 0x38].repeat(2),
                )
                .unwrap())
            })
            .unwrap()
    } else {
        artifacts
            .write(&snapclip_model::CaptureOutput {
                metadata: metadata.clone(),
                bytes: snapclip_history::image::encode_rgba_png(&[1, 2, 3, 255].repeat(2), 2, 1)
                    .unwrap(),
            })
            .unwrap()
    };
    CaptureSource { metadata, artifact }
}
fn thumbnail() -> Vec<u8> {
    snapclip_history::image::encode_rgba_png(&[127, 127, 127, 255], 1, 1).unwrap()
}
fn write_intent(root: &Directory, id: &str, source: &CaptureSource) {
    let path = root.journal(id);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        serde_json::to_vec(&serde_json::json!({
            "version": 1, "id": id, "source": source, "thumbnail": thumbnail(),
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn failed_registration_recovers_hdr_and_sdr_on_reopen_without_changing_the_source() {
    for hdr in [false, true] {
        let root = Directory::new();
        let store = Store::open(&root.0).unwrap();
        let source = source(&root, hdr);
        let original = fs::read(&source.artifact.absolute_path).unwrap();
        root.fail_registration();
        let error = store
            .save_capture("recover".into(), source.clone(), thumbnail())
            .unwrap_err();
        assert!(
            error.to_string().contains("injected registration failure"),
            "{error}"
        );
        assert!(store.history_page(None, None).unwrap().items.is_empty());
        assert!(
            root.journal("recover").exists(),
            "failed handoff needs a durable recovery record"
        );
        assert_eq!(fs::read(&source.artifact.absolute_path).unwrap(), original);
        drop(store);
        // A still-failing database must not discard the intent or block unrelated history reads.
        let store = Store::open(&root.0).unwrap();
        assert!(store.history_page(None, None).unwrap().items.is_empty());
        assert!(root.journal("recover").exists());
        drop(store);
        root.allow_registration();
        let store = Store::open(&root.0).unwrap();
        let page = store.history_page(None, None).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].capture_source.as_ref(), Some(&source));
        assert!(!root.journal("recover").exists());
        assert_eq!(fs::read(&source.artifact.absolute_path).unwrap(), original);
        if hdr {
            let mut master = PixelMaster::open(&source.artifact).unwrap();
            assert_eq!(
                master.read_rows(0, 1).unwrap().as_bytes(),
                [0x00, 0xb4, 0x40, 0x4a, 0x00, 0x38, 0x00, 0x38].repeat(2)
            );
        }
        assert!(store.delete_publication("recover".into()).unwrap());
        assert!(!source.artifact.absolute_path.exists());
    }
}

#[test]
fn committed_registration_is_reconciled_without_a_duplicate_or_resurrection() {
    let root = Directory::new();
    let store = Store::open(&root.0).unwrap();
    let source = source(&root, true);
    store
        .save_capture("committed".into(), source.clone(), thumbnail())
        .unwrap();
    assert!(!root.journal("committed").exists());
    let before = store.history_page(None, None).unwrap();
    // Crash after SQLite commit but before removing the intent.
    write_intent(&root, "committed", &source);
    let reopened = Store::open(&root.0).unwrap();
    assert_eq!(reopened.history_page(None, None).unwrap(), before);
    assert!(!root.journal("committed").exists());
    write_intent(&root, "committed", &source);
    assert!(reopened.delete_publication("committed".into()).unwrap());
    assert!(!root.journal("committed").exists());
    let last = Store::open(&root.0).unwrap();
    assert!(last.history_page(None, None).unwrap().items.is_empty());
}

#[test]
fn recovery_never_imports_external_tampered_or_conflicting_originals() {
    let root = Directory::new();
    let external = Directory::new();
    let store = Store::open(&root.0).unwrap();
    let first = source(&root, true);
    store
        .save_capture("same".into(), first.clone(), thumbnail())
        .unwrap();
    let second = source(&root, true);
    let outside = source(&external, true);
    let tampered = source(&root, true);
    let mut changed = fs::read(&tampered.artifact.absolute_path).unwrap();
    *changed.last_mut().unwrap() ^= 1;
    fs::write(&tampered.artifact.absolute_path, &changed).unwrap();
    for (id, source) in [
        ("same", &second),
        ("external", &outside),
        ("tampered", &tampered),
    ] {
        write_intent(&root, id, source);
    }
    let reopened = Store::open(&root.0).unwrap();
    let page = reopened.history_page(None, None).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].capture_source.as_ref(), Some(&first));
    for (id, source) in [
        ("same", &second),
        ("external", &outside),
        ("tampered", &tampered),
    ] {
        assert!(root.journal(id).exists());
        assert!(source.artifact.absolute_path.exists());
    }
    assert_eq!(fs::read(&tampered.artifact.absolute_path).unwrap(), changed);
}

#[test]
fn malformed_oversized_and_wrongly_named_intents_do_not_block_valid_recovery() {
    use std::io::Write;
    let root = Directory::new();
    let store = Store::open(&root.0).unwrap();
    let source = source(&root, true);
    write_intent(&root, "valid", &source);
    let malformed = root.journal("malformed");
    fs::write(&malformed, b"{").unwrap();
    let oversized = root.journal("oversized");
    let mut file = fs::File::create(&oversized).unwrap();
    file.write_all(b"{").unwrap();
    file.set_len(5 * 1024 * 1024 + 1).unwrap();
    drop(file);
    let wrong_name = root.journal("wrong-name");
    fs::copy(root.journal("valid"), &wrong_name).unwrap();
    drop(store);
    let reopened = Store::open(&root.0).unwrap();
    let page = reopened.history_page(None, None).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, "valid");
    assert!(!root.journal("valid").exists());
    for path in [malformed, oversized, wrong_name] {
        assert!(path.exists());
    }
}

#[test]
fn a_failed_intent_write_cannot_publish_a_row_or_remove_the_original() {
    let root = Directory::new();
    let store = Store::open(&root.0).unwrap();
    let source = source(&root, true);
    let directory = root.0.join("artifacts/pending-captures");
    if directory.exists() {
        fs::remove_dir(&directory).unwrap();
    }
    fs::write(&directory, b"not a directory").unwrap();
    assert!(
        store
            .save_capture("no-intent".into(), source.clone(), thumbnail())
            .is_err()
    );
    assert!(store.history_page(None, None).unwrap().items.is_empty());
    assert!(source.artifact.absolute_path.exists());
}

#[test]
fn retries_cannot_replace_a_pending_original_and_identical_retries_are_idempotent() {
    let root = Directory::new();
    let store = Store::open(&root.0).unwrap();
    let first = source(&root, true);
    let second = source(&root, true);
    root.fail_registration();
    assert!(
        store
            .save_capture("retry".into(), first.clone(), thumbnail())
            .is_err()
    );
    let intent = fs::read(root.journal("retry")).unwrap();
    assert!(
        store
            .save_capture("retry".into(), second.clone(), thumbnail())
            .is_err()
    );
    assert_eq!(fs::read(root.journal("retry")).unwrap(), intent);
    root.allow_registration();
    for _ in 0..2 {
        store
            .save_capture("retry".into(), first.clone(), thumbnail())
            .unwrap();
    }
    let page = store.history_page(None, None).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].capture_source.as_ref(), Some(&first));
    assert!(first.artifact.absolute_path.exists());
    assert!(second.artifact.absolute_path.exists());
    assert!(!root.journal("retry").exists());
}

#[test]
fn failed_handoff_cleanup_prevents_deletion_instead_of_resurrecting_the_row_later() {
    let root = Directory::new();
    let store = Store::open(&root.0).unwrap();
    let source = source(&root, true);
    store
        .save_capture("delete".into(), source.clone(), thumbnail())
        .unwrap();
    let before = store.history_page(None, None).unwrap();
    write_intent(&root, "delete", &source);
    let valid = fs::read(root.journal("delete")).unwrap();
    fs::write(root.journal("delete"), b"broken").unwrap();
    assert!(store.delete_publication("delete".into()).is_err());
    assert_eq!(store.history_page(None, None).unwrap(), before);
    assert!(source.artifact.absolute_path.exists());
    fs::write(root.journal("delete"), valid).unwrap();
    assert!(store.delete_publication("delete".into()).unwrap());
    assert!(!source.artifact.absolute_path.exists());
    assert!(!root.journal("delete").exists());
}

#[test]
fn another_writers_temporary_record_is_never_removed_or_used() {
    let root = Directory::new();
    let store = Store::open(&root.0).unwrap();
    let source = source(&root, true);
    let temporary = root.journal("busy").with_extension("tmp");
    fs::create_dir_all(temporary.parent().unwrap()).unwrap();
    fs::write(&temporary, b"another writer's work").unwrap();
    assert!(
        store
            .save_capture("busy".into(), source.clone(), thumbnail())
            .is_err()
    );
    assert_eq!(fs::read(&temporary).unwrap(), b"another writer's work");
    let reopened = Store::open(&root.0).unwrap();
    assert!(reopened.history_page(None, None).unwrap().items.is_empty());
    assert!(source.artifact.absolute_path.exists());
    assert_eq!(fs::read(&temporary).unwrap(), b"another writer's work");
}

#[test]
fn a_database_read_failure_also_retains_an_intent_before_registration() {
    let root = Directory::new();
    let store = Store::open(&root.0).unwrap();
    let source = source(&root, true);
    root.database()
        .execute_batch("ALTER TABLE clips RENAME TO unavailable_clips;")
        .unwrap();
    assert!(
        store
            .save_capture("read-failure".into(), source.clone(), thumbnail())
            .is_err()
    );
    assert!(root.journal("read-failure").exists());
    assert!(source.artifact.absolute_path.exists());
    root.database()
        .execute_batch("ALTER TABLE unavailable_clips RENAME TO clips;")
        .unwrap();
    drop(store);
    let reopened = Store::open(&root.0).unwrap();
    let page = reopened.history_page(None, None).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].capture_source.as_ref(), Some(&source));
    assert!(!root.journal("read-failure").exists());
}
