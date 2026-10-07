//! The history screen: a search field and a virtualized list of clipboard entries.
//!
//! Design decisions, per the Design Guides' "data-heavy interfaces" section:
//! the row's primary identity (source icon + preview text) stays visually stable, the
//! focused row is visibly distinct from hover, selection is by clip id so filtering cannot
//! move it, the collection is virtualized, and an empty result explains the next step
//! instead of showing a blank pane. Colours come from `cx.theme()`; no raw values.
//!
//! Keyboard path: the search field owns typing, the list owns navigation (↑/↓), `Enter`
//! hands the selection to the owner, `Esc` clears the filter. Critical actions are not
//! hover-only, so nothing here depends on a pointer being present.

use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::base::{StyledExt as _, v_virtual_list};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::ActiveTheme;
use gpui_kit::*;
use snapclip_history::image::decode_to_rgba8;
use snapclip_model::{ClipSummary, PayloadKind};

use crate::clipboard::SystemClipboard;
use super::icons::SourceIcons;
use super::model::HistoryState;

/// Row height in pixels. Fixed so the virtual list can size the scroll range without
/// measuring every row; the row content is one line of text plus one line of metadata.
const ROW_HEIGHT_PX: f32 = 56.0;

pub struct HistoryView {
    state: HistoryState,
    query: Entity<InputState>,
    icons: SourceIcons,
    row_sizes: Rc<Vec<Size<Pixels>>>,
    /// Result of the last command, shown as a status line: the Design Guides require the
    /// result of an action to be visible, and a status line does not depend on hover.
    status: Option<String>,
}

impl HistoryView {
    pub fn new(
        state: HistoryState,
        icons: SourceIcons,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search clipboard history")
        });
        // One unchanged size per row: the list virtualizes by offset, so it needs the
        // sizes up front rather than a measurement pass.
        let row_sizes = Rc::new(vec![size(px(0.0), px(ROW_HEIGHT_PX)); state.items().len()]);
        cx.subscribe(&query, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.apply_query(cx);
            }
        })
        .detach();
        Self {
            state,
            query,
            icons,
            row_sizes,
            status: None,
        }
    }

    /// Re-run the current query and rebuild the row sizes.
    fn apply_query(&mut self, cx: &mut Context<Self>) {
        let next = self.query.read(cx).value().to_string();
        match self.state.set_query(&next) {
            Ok(true) => {
                self.row_sizes = Rc::new(vec![size(px(0.0), px(ROW_HEIGHT_PX)); self.state.items().len()]);
                cx.notify();
            }
            Ok(false) => {}
            Err(error) => eprintln!("[snapclip-app] history query failed: {error}"),
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.state.move_selection(delta);
        cx.notify();
    }

    fn clear_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.query.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        self.apply_query(cx);
    }

    /// `Enter`: copy the selected entry back to the clipboard.
    ///
    /// The write goes through the shell's adapter, which also marks it as ours so the
    /// clipboard monitor does not record our own copy as a new history entry.
    fn copy_selected(&mut self, cx: &mut Context<Self>) {
        let clipboard = SystemClipboard;
        self.status = Some(match self.state.selected_payload_bytes() {
            None => "Nothing selected".to_string(),
            Some(Err(error)) => format!("Could not read the entry: {error}"),
            Some(Ok((_payload, kind, bytes))) => {
                let outcome = match kind {
                    PayloadKind::Image => match decode_to_rgba8(&bytes) {
                        Ok(decoded) => {
                            let (width, height) = (decoded.width(), decoded.height());
                            clipboard.copy_image(width, height, decoded.into_raw())
                        }
                        Err(error) => Err(format!("decode image failed: {error}")),
                    },
                    _ => match String::from_utf8(bytes) {
                        Ok(text) => clipboard.copy_text(&text),
                        Err(_) => Err("the entry is not text".to_string()),
                    },
                };
                match outcome {
                    Ok(()) => match kind {
                        PayloadKind::Image => "Copied image".to_string(),
                        _ => "Copied text".to_string(),
                    },
                    Err(error) => format!("Copy failed: {error}"),
                }
            }
        });
        cx.notify();
    }

    fn render_empty(&self, cx: &Context<Self>) -> impl IntoElement {
        let message = if self.state.items().is_empty() && !self.state.query().trim().is_empty() {
            "No entries match this search".to_string()
        } else {
            // First-run state: say what fills the list rather than leaving it blank.
            "Copied text and images appear here".to_string()
        };
        div()
            .size_full()
            .v_flex()
            .items_center()
            .justify_center()
            .text_color(cx.theme().muted_foreground)
            .child(message)
    }
}

/// `Source app · 14:32 · 图片 · OCR 完成` — the secondary line of a row.
fn row_meta(item: &ClipSummary) -> String {
    let mut parts = Vec::new();
    if let Some(app) = item.source_app.as_deref().filter(|app| !app.is_empty()) {
        parts.push(app.to_string());
    }
    parts.push(format!("{:?}", item.primary_kind));
    if !matches!(item.ocr_status, snapclip_model::OcrStatus::None) {
        parts.push(format!("OCR {:?}", item.ocr_status));
    }
    parts.join(" · ")
}

impl Render for HistoryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = window;
        let theme = cx.theme();
        let count = self.state.items().len();
        let has_rows = count > 0;
        div()
            .v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .key_context("HistoryView")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "up" => this.move_selection(-1, cx),
                    "down" => this.move_selection(1, cx),
                    "escape" => this.clear_query(window, cx),
                    "enter" => this.copy_selected(cx),
                    _ => {}
                }
            }))
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .p_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(Input::new(&self.query))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(match count {
                                0 => "No entries".to_string(),
                                1 => "1 entry".to_string(),
                                n => format!("{n} entries"),
                            }),
                    )
                    .children(self.status.clone().map(|status| {
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(status)
                    })),
            )
            .child(if has_rows {
                v_virtual_list(
                    cx.entity(),
                    "history-rows",
                    self.row_sizes.clone(),
                    |view, range, _window, cx| {
                        // Rows are built here rather than in a helper: a `-> impl IntoElement`
                        // helper would capture `&self` and `&item` under Rust 2024's capture
                        // rules, so the returned element would outlive what it borrows.
                        let theme = cx.theme();
                        let selected_id = view.state.selected_id().map(str::to_string);
                        range
                            .filter_map(|index| {
                                let item = view.state.items().get(index)?.clone();
                                let selected = selected_id.as_deref() == Some(item.id.as_str());
                                let icon = item
                                    .source_exe_path
                                    .as_deref()
                                    .and_then(|exe| view.icons.png_path(exe));
                                let preview = item
                                    .preview_text
                                    .clone()
                                    .unwrap_or_else(|| format!("{:?}", item.primary_kind));
                                let meta = row_meta(&item);
                                let id = item.id.clone();
                                let row = div()
                                    .id(("history-row", index))
                                    .h(px(ROW_HEIGHT_PX))
                                    .w_full()
                                    .px_3()
                                    .gap_3()
                                    .h_flex()
                                    .items_center()
                                    .border_b_1()
                                    .border_color(theme.border)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.select(&id);
                                        cx.notify();
                                    }));
                                // Selected rows get a distinct, stable treatment; ordinary
                                // control flow keeps that readable.
                                let row = if selected { row.bg(theme.accent) } else { row };
                                Some(
                                    row.child(match icon {
                                        Some(path) => img(path).size(px(16.0)).into_any_element(),
                                        // Unknown source: reserve the space so the text
                                        // column stays aligned (a normal state, not an error).
                                        None => div().size(px(16.0)).into_any_element(),
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .v_flex()
                                            .child(div().truncate().child(preview))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(theme.muted_foreground)
                                                    .truncate()
                                                    .child(meta),
                                            ),
                                    ),
                                )
                            })
                            .collect::<Vec<_>>()
                    },
                )
                .flex_1()
                .into_any_element()
            } else {
                self.render_empty(cx).into_any_element()
            })
    }
}

/// Kept for the next slice (Enter copies the selection to the clipboard, per T4.3's
/// acceptance): the set of ids the view has already seen, so a future "new entry" flash
/// does not re-trigger for rows that were merely re-fetched.
#[allow(dead_code)]
fn unseen_ids(items: &[ClipSummary], seen: &HashSet<String>) -> Vec<String> {
    items
        .iter()
        .map(|item| item.id.clone())
        .filter(|id| !seen.contains(id))
        .collect()
}
