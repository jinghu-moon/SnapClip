//! SnapClip's GPUI shell (docs/23 P4).
//!
//! T4.2's job is deliberately small: prove that a gpui-kit application boots, that `Root`
//! wraps the window's first view, and that nothing about the Tauri host changes while this
//! grows next to it. The history list, settings, tray and event bridge arrive in T4.3–T4.6.
//!
//! Rules this file already follows, because the Coding Guides make them requirements:
//! `gpui_kit::init` runs first inside `run`, before any component-backed view exists;
//! each window's first level is `Root`; the UI comes from `gpui_kit` alone.

// Both traits are re-exported by `gpui-kit` itself (paths verified in gpui-base-0.7.1's
// lib.rs and gpui-component-0.7.1's lib.rs), which is what the one-dependency rule wants:
// `StyledExt` carries `v_flex`/`h_flex`, `ActiveTheme` carries `cx.theme()`.
use gpui_kit::base::StyledExt as _;
use gpui_kit::component::{ActiveTheme, Root};
use gpui_kit::*;

mod history;

use history::icons::SourceIcons;
use history::model::HistoryState;
use history::view::HistoryView;

/// The same database and cache directories the Tauri host writes while both shells exist
/// (docs/23 T4.9: they share only the capability crates and the data).
fn app_data_dir() -> std::path::PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("com.seeyuer.snapclip")
}

/// The shell's root view: the history screen, or the reason it could not open.
///
/// A shell that panics because a database file is busy is worse than one that says so, and
/// the Tauri host may be holding the same file until P6 removes it.
struct Shell {
    history: Result<Entity<HistoryView>, String>,
}

impl Shell {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let data = app_data_dir();
        let history = match HistoryState::open(&data) {
            Ok(state) => match SourceIcons::new(data.join("icons")) {
                Ok(icons) => Ok(cx.new(|cx| HistoryView::new(state, icons, window, cx))),
                Err(error) => Err(format!("icons: {error}")),
            },
            Err(error) => Err(format!("history store: {error}")),
        };
        Self { history }
    }
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.history {
            Ok(view) => view.clone().into_any_element(),
            Err(reason) => div()
                .v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .child("Clipboard history is unavailable")
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(reason.clone()),
                )
                .into_any_element(),
        }
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            // MUST be first: component-backed views need the component runtime.
            gpui_kit::init(cx);

            cx.spawn(async move |cx| {
                cx.open_window(WindowOptions::default(), |window, cx| {
                    let shell = cx.new(|cx| Shell::new(window, cx));
                    // `Root` is the window's first-level child; it owns overlays,
                    // notifications and modal focus restoration.
                    cx.new(|cx| Root::new(shell, window, cx))
                })
                .expect("failed to open the SnapClip shell window");
            })
            .detach();
        });
}
