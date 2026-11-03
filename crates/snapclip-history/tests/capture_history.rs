use snapclip_history::{
    artifact_store::CaptureArtifactStore, pixel_master::PixelMaster, store::Store,
};
use snapclip_model::{CaptureMetadata, CaptureSource, PublicationOrigin, pixels::*};
use std::path::PathBuf;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self(std::env::temp_dir().join(format!(
            "capture-history-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn source(root: &Directory) -> CaptureSource {
    let descriptor = PixelDescriptor::new(
        snapclip_model::PixelFormat::Rgba16Float,
        PixelEncoding::ScRgb,
        AlphaMode::Straight,
    )
    .unwrap();
    let metadata = CaptureMetadata {
        session_id: "capture-history".into(),
        width: 2,
        height: 1,
        dpi: 144,
        source: PixelSource::new(descriptor, 91),
        captured_at_unix_ms: 42,
        monitor_device_name: Some("display-test".into()),
    };
    let artifact = CaptureArtifactStore::new(root.0.join("artifacts/capture"))
        .write_pixel_master(&metadata, |_, _| {
            Ok(PixelBuffer::from_tight(
                2,
                1,
                descriptor,
                [0x00, 0xb4, 0x40, 0x4a, 0x00, 0x38, 0x00, 0x38].repeat(2),
            )
            .unwrap())
        })
        .unwrap();
    CaptureSource { metadata, artifact }
}
fn thumbnail() -> Vec<u8> {
    snapclip_history::image::encode_rgba_png(&[127, 127, 127, 255], 1, 1).unwrap()
}

#[test]
fn capture_history_roundtrips_original_metadata_and_never_uses_the_thumbnail_as_source() {
    let root = Directory::new();
    let store = Store::open(&root.0).unwrap();
    let source = source(&root);
    store
        .save_capture("capture-1".into(), source.clone(), thumbnail())
        .unwrap();
    let page = store.history_page(None, None).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].origin, PublicationOrigin::Capture);
    assert_eq!(page.items[0].capture_source.as_ref(), Some(&source));
    assert_eq!(
        page.items[0].payloads[0]
            .image_dimensions
            .as_ref()
            .unwrap()
            .width,
        1
    );
    let mut master = PixelMaster::open(&source.artifact).unwrap();
    assert_eq!(master.metadata(), &source.metadata);
    assert_eq!(
        master.read_rows(0, 1).unwrap().as_bytes(),
        [0x00, 0xb4, 0x40, 0x4a, 0x00, 0x38, 0x00, 0x38].repeat(2)
    );
    drop(master);
    // A fresh store must reconstruct the source from SQLite, not a process cache.
    let reopened = Store::open(&root.0).unwrap();
    assert_eq!(reopened.history_page(None, None).unwrap(), page);
}

#[test]
fn deleting_one_reference_keeps_a_shared_master_until_the_last_reference_is_deleted() {
    let root = Directory::new();
    let store = Store::open(&root.0).unwrap();
    let source = source(&root);
    for id in ["capture-1", "capture-2"] {
        store
            .save_capture(id.into(), source.clone(), thumbnail())
            .unwrap();
    }
    assert!(store.delete_publication("capture-1".into()).unwrap());
    assert!(source.artifact.absolute_path.exists());
    assert!(store.delete_publication("capture-2".into()).unwrap());
    assert!(!source.artifact.absolute_path.exists());
    assert!(store.history_page(None, None).unwrap().items.is_empty());
}

#[test]
fn invalid_capture_source_is_rejected_without_a_history_row_or_source_deletion() {
    let root = Directory::new();
    let store = Store::open(&root.0).unwrap();
    let source = source(&root);
    let mut changed = source.clone();
    changed.metadata.source = PixelSource::new(changed.metadata.source.descriptor(), 92);
    assert!(
        store
            .save_capture("bad".into(), changed, thumbnail())
            .is_err()
    );
    assert!(store.history_page(None, None).unwrap().items.is_empty());
    assert!(source.artifact.absolute_path.exists());
    assert!(
        store
            .save_capture("bad".into(), source.clone(), b"fake png".to_vec())
            .is_err()
    );
    assert!(store.history_page(None, None).unwrap().items.is_empty());
}

#[test]
fn external_and_tampered_sources_are_never_registered_or_deleted() {
    let root = Directory::new();
    let external = Directory::new();
    let store = Store::open(&root.0).unwrap();
    let outside = source(&external);
    assert!(
        store
            .save_capture("external".into(), outside.clone(), thumbnail())
            .is_err()
    );
    assert!(outside.artifact.absolute_path.exists());
    let owned = source(&root);
    let mut bytes = std::fs::read(&owned.artifact.absolute_path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&owned.artifact.absolute_path, &bytes).unwrap();
    assert!(
        store
            .save_capture("tampered".into(), owned.clone(), thumbnail())
            .is_err()
    );
    assert!(store.history_page(None, None).unwrap().items.is_empty());
    assert_eq!(std::fs::read(&owned.artifact.absolute_path).unwrap(), bytes);
}

#[test]
fn a_duplicate_id_cannot_replace_the_existing_source_or_publish_partial_rows() {
    let root = Directory::new();
    let store = Store::open(&root.0).unwrap();
    let first = source(&root);
    let second = source(&root);
    store
        .save_capture("same".into(), first.clone(), thumbnail())
        .unwrap();
    let before = store.history_page(None, None).unwrap();
    assert!(
        store
            .save_capture("same".into(), second.clone(), thumbnail())
            .is_err()
    );
    assert_eq!(store.history_page(None, None).unwrap(), before);
    assert!(first.artifact.absolute_path.exists());
    assert!(second.artifact.absolute_path.exists());
    store.delete_publication("same".into()).unwrap();
    assert!(!first.artifact.absolute_path.exists());
    assert!(
        second.artifact.absolute_path.exists(),
        "an unregistered original is not the deleted row's source"
    );
}

#[test]
fn capture_thumbnails_cannot_enter_ocr_but_ordinary_publications_keep_their_origin() {
    use snapclip_history::store::{OcrCandidateFilter, QueueDecision};
    use snapclip_model::{PayloadData, PayloadKind, PayloadRef, Publication};
    let root = Directory::new();
    let store = Store::open(&root.0).unwrap();
    store
        .save_capture("capture".into(), source(&root), thumbnail())
        .unwrap();
    let hash = store.history_page(None, None).unwrap().items[0].payloads[0]
        .content_hash
        .clone();
    assert_eq!(
        store.enqueue_ocr("capture".into(), hash).unwrap(),
        QueueDecision::NotImage
    );
    assert!(
        store
            .list_ocr_candidates(OcrCandidateFilter::None, 20)
            .unwrap()
            .is_empty()
    );
    for (id, origin) in [
        ("ordinary-clipboard", PublicationOrigin::Clipboard),
        ("ordinary-capture", PublicationOrigin::Capture),
    ] {
        let bytes = thumbnail();
        let payload = PayloadRef {
            payload_id: id.into(),
            content_hash: blake3::hash(&bytes).to_hex().to_string(),
            kind: PayloadKind::Image,
            size_bytes: bytes.len() as u64,
            mime_type: Some("image/png".into()),
            image_dimensions: Some(snapclip_model::ImageDimensions {
                width: 1,
                height: 1,
            }),
        };
        store
            .save_publication(
                Publication {
                    publication_id: id.into(),
                    origin,
                    captured_at_unix_ms: 100,
                    source_app: None,
                    source_exe_path: None,
                    payloads: vec![payload.clone()],
                },
                vec![PayloadData { payload, bytes }],
            )
            .unwrap();
        let page = store.history_page(None, None).unwrap();
        let item = page.items.iter().find(|item| item.id == id).unwrap();
        assert_eq!(item.origin, origin);
        assert!(item.capture_source.is_none());
    }
    let candidates = store
        .list_ocr_candidates(OcrCandidateFilter::None, 20)
        .unwrap();
    assert_eq!(candidates.len(), 2);
    assert!(candidates.iter().all(|c| c.clip_id != "capture"));
}
