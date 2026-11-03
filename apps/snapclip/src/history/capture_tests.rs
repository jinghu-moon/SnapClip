use super::*;
use snapclip_model::{PixelFormat, pixels::*};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self(std::env::temp_dir().join(format!(
            "capture-replay-{}-{}-{}",
            std::process::id(),
            snapclip_model::unix_time_ms(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture(hdr: bool) -> (Directory, Store, CaptureArchive, Arc<PixelBuffer>) {
    let root = Directory::new();
    let store = Store::open(&root.0).unwrap();
    let descriptor = if hdr {
        PixelDescriptor::new(
            PixelFormat::Rgba16Float,
            PixelEncoding::ScRgb,
            AlphaMode::Straight,
        )
        .unwrap()
    } else {
        PixelDescriptor::bgra8()
    };
    let bytes = if hdr {
        [0x00, 0xb4, 0x40, 0x4a, 0x00, 0x38, 0x00, 0x38].repeat(512)
    } else {
        [13, 27, 91, 127].repeat(512)
    };
    let pixels = Arc::new(PixelBuffer::from_tight(256, 2, descriptor, bytes).unwrap());
    let archive = CaptureArchive::new(
        root.0.clone(),
        store.clone(),
        CaptureMetadata {
            session_id: "history-replay".into(),
            width: 256,
            height: 2,
            dpi: 144,
            source: PixelSource::new(descriptor, 91),
            captured_at_unix_ms: 42,
            monitor_device_name: Some("test-output".into()),
        },
    );
    (root, store, archive, pixels)
}

#[test]
fn repeated_panel_saves_archive_once_and_hdr_replay_keeps_bits_generation_and_live_pixels() {
    let (_root, store, archive, pixels) = fixture(true);
    let before = pixels.as_bytes().to_vec();
    let events = crate::events::EventBus::new();
    let picture = crate::save_panel::Picture::from_shared_pixels(pixels.clone())
        .with_archive(archive.with_events(events.clone()));
    picture.archive_original().unwrap();
    picture.clone().archive_original().unwrap();
    assert_eq!(
        events.published(),
        1,
        "archive completion must publish exactly once, after durable storage"
    );
    let page = store.history_page(None, None).unwrap();
    assert_eq!(page.items.len(), 1);
    let source = page.items[0].capture_source.as_ref().unwrap();
    let replay = read_source(source).unwrap();
    assert!(replay.is_hdr());
    assert_eq!(replay.pixel_source(), source.metadata.source);
    assert_eq!(replay.pixels().as_bytes(), before);
    let thumbnail = store
        .read_payload_bytes(
            page.items[0].payloads[0].content_hash.clone(),
            snapclip_model::PayloadKind::Image,
        )
        .unwrap();
    assert_eq!(thumbnail[24], 8);
    assert_eq!(
        snapclip_history::image::png_dimensions(&thumbnail),
        Some((128, 1))
    );
    assert_eq!(pixels.as_bytes(), before);
    let state = crate::history::model::HistoryState::with_store(store.clone()).unwrap();
    assert_eq!(state.selected_capture(), Some(source));
    assert!(
        state.selected_payload_bytes().unwrap().is_err(),
        "thumbnail cannot become a copy source"
    );
    assert!(store.delete_publication(page.items[0].id.clone()).unwrap());
    assert!(!source.artifact.absolute_path.exists());
    assert_eq!(
        replay.pixels().as_bytes(),
        before,
        "a live preview owns its pixels after history deletion"
    );
    assert!(
        read_source(source).is_err(),
        "a missing source must never fall back to the thumbnail"
    );
}

#[test]
fn sdr_replay_reopens_the_original_size_and_alpha_instead_of_the_row_thumbnail() {
    let (_root, store, archive, pixels) = fixture(false);
    archive.persist(&pixels).unwrap();
    let page = store.history_page(None, None).unwrap();
    let source = page.items[0].capture_source.as_ref().unwrap();
    let replay = read_source(source).unwrap();
    assert!(!replay.is_hdr());
    assert_eq!((replay.width(), replay.height()), (256, 2));
    assert_eq!(replay.pixels().as_bytes(), pixels.as_bytes());
    assert_eq!(replay.pixel_source().generation(), 91);
    let mut encoded = Vec::new();
    crate::capture::pixel_export::write_capture_source_png(source, &mut encoded).unwrap();
    assert_eq!(
        encoded,
        std::fs::read(&source.artifact.absolute_path).unwrap(),
        "streamed SDR export preserves original encoded bytes"
    );
}

#[test]
fn streaming_hdr_history_export_uses_the_master_and_keeps_pq_depth_and_alpha() {
    let (_root, store, archive, pixels) = fixture(true);
    archive.persist(&pixels).unwrap();
    let source = store.history_page(None, None).unwrap().items[0]
        .capture_source
        .clone()
        .unwrap();
    let before = std::fs::read(&source.artifact.absolute_path).unwrap();
    let mut output = Vec::new();
    crate::capture::pixel_export::write_capture_source_png(&source, &mut output).unwrap();
    let header = png::Decoder::new(std::io::Cursor::new(&output))
        .read_info()
        .unwrap();
    assert_eq!((header.info().width, header.info().height), (256, 2));
    assert_eq!(header.info().bit_depth, png::BitDepth::Sixteen);
    assert_eq!(
        header
            .info()
            .coding_independent_code_points
            .unwrap()
            .transfer_function,
        16
    );
    let decoded = snapclip_pin::decode_image(&output).unwrap();
    assert!(decoded.is_hdr());
    // These negative scRGB source values remain within the wider Rec.2020
    // gamut. Compare with the same explicit quantized derivative; FP16 bits
    // remain exclusively in the unchanged master.
    let expected = crate::capture::pixel_export::encode_pixels(
        &pixels,
        crate::capture::pixel_export::ColorExport::PreserveHdr,
    )
    .unwrap();
    assert_eq!(output, expected);
    assert_eq!(
        std::fs::read(&source.artifact.absolute_path).unwrap(),
        before
    );
}

#[test]
fn history_export_checks_the_resolved_target_and_never_overwrites_its_source() {
    let (_root, store, archive, pixels) = fixture(false);
    archive.persist(&pixels).unwrap();
    let source = store.history_page(None, None).unwrap().items[0]
        .capture_source
        .clone()
        .unwrap();
    let original = &source.artifact.absolute_path;
    let before = std::fs::read(original).unwrap();
    assert!(
        export_source(&source, original)
            .unwrap_err()
            .contains("不能覆盖")
    );
    let normalized_alias = original.with_file_name(format!(
        "{}. .png",
        original.file_stem().unwrap().to_string_lossy()
    ));
    assert!(
        export_source(&source, &normalized_alias)
            .unwrap_err()
            .contains("不能覆盖")
    );
    assert_eq!(std::fs::read(original).unwrap(), before);
    assert!(!normalized_alias.exists());
}

#[test]
fn corrupt_sdr_and_hdr_exports_preserve_the_target_and_clean_their_own_partial() {
    for hdr in [false, true] {
        let (root, store, archive, pixels) = fixture(hdr);
        archive.persist(&pixels).unwrap();
        let source = store.history_page(None, None).unwrap().items[0]
            .capture_source
            .clone()
            .unwrap();
        // Keep the encoded length so the PNG path fails after it wrote partial
        // output, at the fingerprint check rather than a cheap size check.
        let mut bytes = std::fs::read(&source.artifact.absolute_path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        std::fs::write(&source.artifact.absolute_path, bytes).unwrap();
        let target = root.0.join("keep.png");
        std::fs::write(&target, b"old-target").unwrap();
        assert!(export_source(&source, &target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"old-target");
        assert!(!root.0.join("keep.png.partial").exists());
    }
}

#[test]
fn unrepresentable_hdr_export_fails_without_clipping_or_replacing_the_original() {
    let (root, store, archive, pixels) = fixture(true);
    let bytes = [-1.0_f32, 0., 0., 1.]
        .into_iter()
        .flat_map(|v| half::f16::from_f32(v).to_le_bytes())
        .collect::<Vec<_>>()
        .repeat(512);
    let pixels = PixelBuffer::from_tight(256, 2, pixels.descriptor(), bytes.clone()).unwrap();
    archive.persist(&pixels).unwrap();
    let source = store.history_page(None, None).unwrap().items[0]
        .capture_source
        .clone()
        .unwrap();
    let target = root.0.join("keep.png");
    std::fs::write(&target, b"old-target").unwrap();
    assert!(
        export_source(&source, &target)
            .unwrap_err()
            .contains("outside Rec.2020/PQ range")
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"old-target");
    assert!(!root.0.join("keep.png.partial").exists());
    assert_eq!(read_source(&source).unwrap().pixels().as_bytes(), bytes);
}

#[test]
fn a_failed_archive_can_retry_and_a_corrupt_master_never_becomes_a_thumbnail_replay() {
    let (_root, store, archive, pixels) = fixture(true);
    let wrong = PixelBuffer::from_tight(1, 1, PixelDescriptor::bgra8(), vec![0; 4]).unwrap();
    assert!(archive.persist(&wrong).is_err());
    assert!(store.history_page(None, None).unwrap().items.is_empty());
    archive.persist(&pixels).unwrap();
    let page = store.history_page(None, None).unwrap();
    let source = page.items[0].capture_source.as_ref().unwrap();
    std::fs::write(&source.artifact.absolute_path, b"replaced-file").unwrap();
    assert!(read_source(source).is_err());
    assert!(store.delete_publication(page.items[0].id.clone()).unwrap());
    assert_eq!(
        std::fs::read(&source.artifact.absolute_path).unwrap(),
        b"replaced-file",
        "deleting a history row must not remove a replaced external identity"
    );
}

#[test]
fn oversized_source_is_rejected_before_allocating_or_opening_a_file() {
    let (_root, store, archive, pixels) = fixture(true);
    archive.persist(&pixels).unwrap();
    let mut source = store.history_page(None, None).unwrap().items[0]
        .capture_source
        .clone()
        .unwrap();
    source.metadata.width = u32::MAX;
    source.metadata.height = u32::MAX;
    source.artifact.absolute_path = PathBuf::from("does-not-exist");
    assert!(read_source(&source).unwrap_err().contains("预算"));
}

#[cfg(feature = "test-support")]
#[gpui_kit::test]
fn history_original_button_opens_the_real_pin_view_with_original_pixels(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::{AppContext as _, component::Root, px, size, test::TestWindowExt as _};
    let (root, store, archive, pixels) = fixture(true);
    archive.persist(&pixels).unwrap();
    cx.update(|cx| {
        gpui_kit::init(cx);
        yue_component::init(
            cx,
            yue_component::YueTheme::new(
                yue_component::ThemeMode::Light,
                yue_component::Accent::Azure,
            ),
        )
        .unwrap();
        snapclip_pin::init(cx);
    });
    let pins = cx.new(|_| snapclip_pin::PinManager::new().unwrap().0);
    let state = crate::history::model::HistoryState::with_store(store).unwrap();
    let icons = crate::history::icons::SourceIcons::new(root.0.join("icons")).unwrap();
    let handle = cx.open_window(size(px(360.), px(480.)), |window, cx| {
        let view = cx.new(|cx| {
            crate::history::view::HistoryView::new(
                state,
                icons,
                crate::events::EventBus::new(),
                window,
                cx,
            )
        });
        view.update(cx, |view, _| view.set_pins(pins.downgrade()));
        Root::new(view, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        for id in [
            "history-open-original",
            "history-copy-sdr",
            "history-copy-hdr",
            "history-export-original",
        ] {
            let bounds = window.find(id).bounds();
            assert!(
                bounds.left() >= px(0.) && bounds.right() <= px(360.),
                "{id} must fit the narrow history toolbar: {bounds:?}"
            );
        }
        window.click("history-export-original", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("history-export-original", cx)
    })
    .unwrap();
    let exported = root.0.join("historical-original.png");
    cx.simulate_new_path_selection(|_| Some(exported.clone()));
    cx.run_until_parked();
    let exported = snapclip_pin::decode_image(&std::fs::read(exported).unwrap()).unwrap();
    assert_eq!((exported.width(), exported.height()), (256, 2));
    assert!(exported.is_hdr());
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("history-open-original", cx)
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert!(pins.read(cx).is_open(1)));
}
