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
        ] {
            let bounds = window.find(id).bounds();
            assert!(
                bounds.left() >= px(0.) && bounds.right() <= px(360.),
                "{id} must fit the narrow history toolbar: {bounds:?}"
            );
        }
        window.click("history-open-original", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert!(pins.read(cx).is_open(1)));
}
