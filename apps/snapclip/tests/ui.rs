//! UI integration tests for the history screen (docs/23 T4.3, step 4).
//!
//! Run with the feature that registers test nodes:
//! `cargo test -p snapclip-app --features test-support`.
//!
//! These tests drive the real view through native input and assert what the *view* shows,
//! not what its internals happen to hold: the guides' rule is that a test which never
//! checks the action's outcome is not evidence.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::component::{Root, WindowExt as _};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, Entity, TestAppContext, px, size};
use snapclip_app::events::EventBus;
use snapclip_app::history::icons::SourceIcons;
use snapclip_app::history::model::HistoryState;
use snapclip_app::history::view::HistoryView;
use snapclip_app::settings::{Settings, SettingsStore, SettingsView};
use snapclip_history::store::Store;
use snapclip_model::{
    AppEvent, ClipboardEvent, PayloadData, PayloadKind, PayloadRef, Publication, PublicationOrigin,
};

static SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_root(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "snapclip-app-ui-{name}-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&path);
    path
}

fn save_text(store: &Store, id: &str, text: &str) {
    let bytes = text.as_bytes().to_vec();
    let payload = PayloadRef {
        payload_id: format!("{id}-0"),
        content_hash: blake3::hash(&bytes).to_hex().to_string(),
        kind: PayloadKind::Text,
        size_bytes: bytes.len() as u64,
        mime_type: Some(PayloadKind::Text.default_mime_type().to_string()),
        image_dimensions: None,
    };
    store
        .save_publication(
            Publication {
                publication_id: id.to_string(),
                origin: PublicationOrigin::Clipboard,
                captured_at_unix_ms: 1_700_000_000_000,
                source_app: Some("Test App".to_string()),
                source_exe_path: None,
                payloads: vec![payload.clone()],
            },
            vec![PayloadData::new(payload, bytes)],
        )
        .expect("save publication");
}

fn save_image(store: &Store, id: &str, bytes: &[u8]) {
    let bytes = bytes.to_vec();
    let payload = PayloadRef {
        payload_id: format!("{id}-image"),
        content_hash: blake3::hash(&bytes).to_hex().to_string(),
        kind: PayloadKind::Image,
        size_bytes: bytes.len() as u64,
        mime_type: Some(PayloadKind::Image.default_mime_type().to_string()),
        image_dimensions: None,
    };
    store
        .save_publication(
            Publication {
                publication_id: id.to_string(),
                origin: PublicationOrigin::Clipboard,
                captured_at_unix_ms: 1_700_000_000_000,
                source_app: Some("Test App".to_string()),
                source_exe_path: None,
                payloads: vec![payload.clone()],
            },
            vec![PayloadData::new(payload, bytes)],
        )
        .expect("save image publication");
}

#[gpui_kit::test]
fn typing_filters_the_list_and_escape_clears_it(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let data = temp_root("filter");
    {
        let store = Store::open(&data).expect("open store");
        save_text(&store, "clip-1", "alpha note");
        save_text(&store, "clip-2", "beta note");
    }
    let state = HistoryState::open(&data).expect("open history");
    let icons = SourceIcons::new(data.join("icons")).expect("icons");

    let mut view: Option<Entity<HistoryView>> = None;
    let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
        let view_entity = cx.new(|cx| HistoryView::new(state, icons, EventBus::new(), window, cx));
        view = Some(view_entity.clone());
        Root::new(view_entity, window, cx)
    });
    let view = view.expect("view constructed");

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        // The search field is reachable and focused by clicking it.
        window.click("history-query", cx);
        assert_eq!(window.find("history-query").focused(), Some(true));

        // A query that matches one entry filters the list to that entry.
        window.input("alpha", cx);
        // Assert the field itself first: if the text never landed there, that is the bug,
        // and the filter assertion below would only report the symptom.
        assert_eq!(window.find("history-query").value(), Some("alpha"));
        assert_eq!(view.read(cx).items().len(), 1);
        assert_eq!(view.read(cx).selected_id(), Some("clip-1"));

        // A query that matches nothing leaves the list empty and nothing selected —
        // the negative case the guides ask for.
        window.input(" nothing", cx);
        assert_eq!(view.read(cx).items().len(), 0);
        assert_eq!(view.read(cx).selected_id(), None);

        // Escape clears the filter and restores the rows.
        window.press("escape", cx);
        assert_eq!(view.read(cx).items().len(), 2);
        assert_eq!(view.read(cx).query(), "");
    })
    .unwrap();

    let _ = std::fs::remove_dir_all(&data);
}

/// The rows must not overlap.
///
/// This is the regression test for the layout the user photographed: a clip whose text is a
/// multi-line code block made its row paint over the next one, because the row's height is
/// fixed while a GPUI div does not clip its children. The fix is two-fold (collapse the text
/// to one line, and clip the row), and this asserts the *outcome* rather than either half.
#[gpui_kit::test]
fn rows_do_not_paint_over_each_other(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let data = temp_root("row-geometry");
    {
        let store = Store::open(&data).expect("open store");
        // The exact shape that broke it: a long, multi-line, whitespace-heavy code block.
        save_text(
            &store,
            "clip-code",
            "项目 A\n  ↓\n打开资源管理器\n  ↓\n找到项目目录\n  ↓\n复制路径\n  ↓\n打开 Windows Terminal\n",
        );
        save_text(&store, "clip-2", "a second entry");
        save_text(&store, "clip-3", "a third entry");
    }
    let state = HistoryState::open(&data).expect("open history");
    let icons = SourceIcons::new(data.join("icons")).expect("icons");

    let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
        let view = cx.new(|cx| HistoryView::new(state, icons, EventBus::new(), window, cx));
        Root::new(view, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let row = |index: usize| window.find(("history-row", index));
        let first = row(0);
        let second = row(1);
        let third = row(2);

        // Each row is one row tall, no matter what its text contains.
        for (index, snapshot) in [first.clone(), second.clone(), third.clone()].into_iter().enumerate() {
            assert!(
                snapshot.bounds().size.height >= px(40.) && snapshot.bounds().size.height <= px(80.),
                "row {index} measured {:?}; a row must stay one row tall",
                snapshot.bounds().size.height
            );
        }
        // And they are stacked, not overlapping.
        assert!(
            second.bounds().top() >= first.bounds().bottom(),
            "row 1 starts before row 0 ends: {:?} vs {:?}",
            second.bounds(),
            first.bounds()
        );
        assert!(
            third.bounds().top() >= second.bounds().bottom(),
            "row 2 starts before row 1 ends: {:?} vs {:?}",
            third.bounds(),
            second.bounds()
        );

        // The strongest half of the assertion: the code block's text itself renders as ONE
        // line. The threshold is deliberately loose (36px against a 60px row) because it only
        // has to distinguish "one line" from "wrapped": the six-line code block in this fixture
        // measured ~78px while it was still wrapping, which is what the row's fixed height could
        // not absorb and how the text ended up drawn over the row below it.
        for index in 0..3usize {
            let preview = window.find(("history-preview", index));
            assert!(
                preview.bounds().size.height <= px(36.),
                "preview {index} measured {:?}; a row's text must be a single line",
                preview.bounds().size.height
            );
        }
    })
    .unwrap();

    let _ = std::fs::remove_dir_all(&data);
}

/// T4.3's remaining acceptance: the type filter, paging, and a delete that asks first.
#[gpui_kit::test]
fn the_filter_the_next_page_and_the_delete_flow_all_reach_the_screen(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let data = temp_root("filter-page-delete");
    {
        let store = Store::open(&data).expect("open store");
        // 60 text rows (more than one page of 50) and one image row.
        for index in 0..60 {
            save_text(&store, &format!("clip-{index:03}"), &format!("entry {index}"));
        }
        save_image(&store, "clip-image", b"not really a png");
    }
    let state = HistoryState::open(&data).expect("open history");
    let icons = SourceIcons::new(data.join("icons")).expect("icons");

    let mut view: Option<Entity<HistoryView>> = None;
    let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
        let view_entity =
            cx.new(|cx| HistoryView::new(state, icons, EventBus::new(), window, cx));
        view = Some(view_entity.clone());
        Root::new(view_entity, window, cx)
    });
    let view = view.expect("view constructed");

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(view.read(cx).items().len(), 50, "first page only");
        assert!(view.read(cx).has_more());

        // The type filter is reachable, and the accessibility tree agrees about which chip
        // is the active one.
        assert_eq!(window.find("history-filter-all").selected(), Some(true));
        window.click("history-filter-image", cx);
        assert_eq!(view.read(cx).items().len(), 1);
        assert_eq!(view.read(cx).items()[0].id, "clip-image");
        assert_eq!(window.find("history-filter-image").selected(), Some(true));
        assert_eq!(window.find("history-filter-all").selected(), Some(false));

        window.click("history-filter-all", cx);
        assert_eq!(view.read(cx).items().len(), 50);

        // Paging: the explicit control does what the scroll trigger does.
        assert_eq!(window.find("history-load-more").label(), Some("加载更多"));
        window.click("history-load-more", cx);
        assert_eq!(view.read(cx).items().len(), 61);
        assert!(!view.read(cx).has_more());
        // With nothing left to fetch, the control is gone rather than lying.
        assert!(window.try_find("history-load-more").is_none());
    })
    .unwrap();

    // The row preview is lazy: it appears because the image row is on screen, and it is read
    // after the frame, not during it (`defer_in`).
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            view.read(cx).loaded_previews() >= 1,
            "the visible image row should have asked for its preview"
        );
    })
    .unwrap();

    // Cancel first: a dismissed confirmation must leave the row alone.
    cx.update_window(handle.into(), |_, window, cx| {
        let doomed = view.read(cx).selected_id().map(str::to_string);
        assert_eq!(doomed.as_deref(), Some("clip-image"), "newest row is selected");
        window.press("delete", cx);
        assert!(window.has_active_dialog(cx), "deleting asks first");
        assert_eq!(view.read(cx).items().len(), 61, "nothing deleted yet");
        window.close_dialog(cx);
        window.render_frame(cx);
        assert!(!window.has_active_dialog(cx));
        assert_eq!(view.read(cx).items().len(), 61);
    })
    .unwrap();

    // Then confirm: the row and its payload are gone.
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("delete", cx);
        assert!(window.has_active_dialog(cx));
        window.within("dialog").click("ok", cx);
        window.render_frame(cx);
        assert_eq!(view.read(cx).items().len(), 60);
        assert!(
            !view
                .read(cx)
                .items()
                .iter()
                .any(|item| item.id == "clip-image"),
            "the confirmed delete must remove that specific row"
        );
        assert_eq!(window.find("history-status").label(), Some("已删除"));
    })
    .unwrap();

    // The store agrees: nothing can read the deleted clip's bytes any more.
    let store = Store::open(&data).expect("open store");
    let page = store.history_page(None, None).expect("re-read history");
    assert_eq!(page.items.len(), 50);
    assert!(!page.items.iter().any(|item| item.id == "clip-image"));
    let _ = std::fs::remove_dir_all(&data);
}

/// T4.7's keyboard layer, plus the negative case that the focus fix exists for: the arrows
/// belong to whichever of the two things owns them, and the wrong one must not react.
#[gpui_kit::test]
fn the_arrows_belong_to_the_list_and_the_field_keeps_them_while_it_types(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let data = temp_root("keyboard-ownership");
    {
        let store = Store::open(&data).expect("open store");
        save_text(&store, "clip-1", "first entry");
        save_text(&store, "clip-2", "second entry");
    }
    let state = HistoryState::open(&data).expect("open history");
    let icons = SourceIcons::new(data.join("icons")).expect("icons");

    let mut view: Option<Entity<HistoryView>> = None;
    let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
        let view_entity =
            cx.new(|cx| HistoryView::new(state, icons, EventBus::new(), window, cx));
        view = Some(view_entity.clone());
        Root::new(view_entity, window, cx)
    });
    let view = view.expect("view constructed");

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        // The list owns the keyboard when the screen opens, so navigation works immediately.
        assert_eq!(view.read(cx).selected_id(), Some("clip-2"));
        window.press("down", cx);
        assert_eq!(view.read(cx).selected_id(), Some("clip-1"));
        window.press("up", cx);
        assert_eq!(view.read(cx).selected_id(), Some("clip-2"));

        // Now the search field takes it: typing must filter, and the arrows must move the
        // caret instead of the selection. Without the focus handle this moved the selection.
        window.click("history-query", cx);
        window.input("entry", cx);
        assert_eq!(window.find("history-query").value(), Some("entry"));
        let before = view.read(cx).selected_id().map(str::to_string);
        window.press("down", cx);
        assert_eq!(
            view.read(cx).selected_id().map(str::to_string),
            before,
            "the field was typing: the list must not steal the arrow keys"
        );
    })
    .unwrap();

    let _ = std::fs::remove_dir_all(&data);
}

/// The settings page's acceptance is "change it, and the change is real": a switch that
/// only looks toggled is the failure mode this catches.
#[gpui_kit::test]
fn toggling_the_setting_writes_it_to_disk(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let data = temp_root("settings-switch");
    let store = SettingsStore::new(&data);
    store.save(&Settings::default()).expect("seed settings");

    let mut view: Option<Entity<SettingsView>> = None;
    let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
        let entity = cx.new(|_| SettingsView::new(SettingsStore::new(&data)));
        view = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let view = view.expect("view constructed");

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            view.read(cx).settings().deep_select_text_runs,
            "the switch starts from the file, which holds the default"
        );

        window.click("deep-select-text-runs", cx);

        assert!(!view.read(cx).settings().deep_select_text_runs);
    })
    .unwrap();

    // The assertion that matters: the file changed, not just the rendered state.
    assert!(!SettingsStore::new(&data).load().deep_select_text_runs);
    let _ = std::fs::remove_dir_all(&data);
}

/// T4.5's acceptance, end to end: a clip stored elsewhere in the process (while the Tauri
/// host still owns the clipboard monitor, it is an entirely different process) reaches the
/// screen through the typed channel, and the screen re-reads the database when it does.
#[gpui_kit::test]
fn a_clipboard_event_refreshes_the_history_screen(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let data = temp_root("event-bridge");
    {
        let store = Store::open(&data).expect("open store");
        save_text(&store, "clip-1", "first entry");
    }
    let state = HistoryState::open(&data).expect("open history");
    let icons = SourceIcons::new(data.join("icons")).expect("icons");
    let bus = EventBus::new();

    let mut view: Option<Entity<HistoryView>> = None;
    let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
        let bus = bus.clone();
        let view_entity = cx.new(|cx| HistoryView::new(state, icons, bus, window, cx));
        view = Some(view_entity.clone());
        Root::new(view_entity, window, cx)
    });
    let view = view.expect("view constructed");

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(view.read(cx).items().len(), 1);
    })
    .unwrap();

    // Another thread (in production, another process) stores a clip and announces it.
    {
        let store = Store::open(&data).expect("open store");
        save_text(&store, "clip-2", "second entry");
    }
    bus.publish(AppEvent::Clipboard(ClipboardEvent {
        clip_id: "clip-2".into(),
        kind: PayloadKind::Text,
        dimensions: None,
        pixel_format: None,
        generation: 0,
    }));
    // The subscriber is an async task, so the publish only becomes a render after the
    // executor runs it.
    cx.run_until_parked();

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            view.read(cx).items().len(),
            2,
            "the screen must show the clip the event announced"
        );
        assert_eq!(view.read(cx).items()[0].id, "clip-2");
    })
    .unwrap();

    let _ = std::fs::remove_dir_all(&data);
}
