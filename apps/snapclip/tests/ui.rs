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

use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, Entity, TestAppContext, px, size};
use snapclip_app::history::icons::SourceIcons;
use snapclip_app::history::model::HistoryState;
use snapclip_app::history::view::HistoryView;
use snapclip_app::settings::{Settings, SettingsStore, SettingsView};
use snapclip_history::store::Store;
use snapclip_model::{PayloadData, PayloadKind, PayloadRef, Publication, PublicationOrigin};

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
        let view_entity = cx.new(|cx| HistoryView::new(state, icons, window, cx));
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
