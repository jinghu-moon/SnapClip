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

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::{ScrollbarHandle as _, StyledExt as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme, Theme, VirtualListScrollHandle, WindowExt as _, v_virtual_list,
};
use gpui_kit::*;
// Only with the `test-support` feature: it registers nodes for `window.find("id")`.
#[cfg(feature = "test-support")]
use gpui_kit::test::TestSupportExt;
use snapclip_history::image::decode_to_rgba8;
use snapclip_model::{AppEvent, ClipSummary, PayloadKind};

use crate::clipboard::SystemClipboard;
use crate::events::EventBus;
use super::icons::SourceIcons;
use super::model::HistoryState;

/// Row height in pixels. Fixed so the virtual list can size the scroll range without
/// measuring every row; the row content is one line of text plus one line of metadata.
const ROW_HEIGHT_PX: f32 = 60.0;

/// The selected row's marker: a bar at the leading edge, not a colour wash alone.
const SELECTION_BAR_PX: f32 = 3.0;

/// Row preview size. Matches the height the row already reserves for an icon column.
const THUMBNAIL_PX: f32 = 32.0;

/// How many decoded previews to keep before dropping the cache.
const THUMBNAIL_CACHE_LIMIT: usize = 256;

/// How close to the bottom counts as "load the next page" (the old front end used 3 rows).
const LOAD_MORE_THRESHOLD_ROWS: f32 = 3.0;

/// How many characters of a clip's text a row shows.
///
/// The store keeps 500 for search and the dialog; a row is one line, so it needs far less.
const PREVIEW_CHARS: usize = 160;

pub struct HistoryView {
    state: HistoryState,
    query: Entity<InputState>,
    icons: SourceIcons,
    row_sizes: Rc<Vec<Size<Pixels>>>,
    /// The list's scroll position, so "near the end" can page in the next batch the same way
    /// the old front end did (it triggered within three rows of the bottom).
    list_scroll: VirtualListScrollHandle,
    /// Decoded row previews, keyed by clip id. `None` records "there is nothing to show" so
    /// a row that cannot be decoded is not retried on every frame.
    thumbnails: HashMap<String, Option<Arc<Image>>>,
    /// Clip ids whose preview is being read right now, so a frame redraw cannot queue the
    /// same read twice.
    pending_thumbnails: HashSet<String>,
    /// Whether a page request is in flight, so the scroll trigger and the button cannot both
    /// fire for the same page.
    loading_more: bool,
    /// Result of the last command, shown as a status line: the Design Guides require the
    /// result of an action to be visible, and a status line does not depend on hover.
    status: Option<String>,
    /// The task that drains the shell's event channel. Dropping it cancels the subscription
    /// with the view, which is why it is held rather than detached.
    _events: Task<()>,
    /// Re-reads the list when the window regains focus (the other shell writes the same
    /// database, and focus is the one moment we know the user is looking at it).
    _activation: Subscription,
    /// Keyboard ownership. The list and the search field are two different things that both
    /// want the arrow keys, so the screen has to say which one is typing: this handle is the
    /// list's, and the handler only navigates or deletes while it holds focus.
    list_focus: FocusHandle,
}

impl HistoryView {
    pub fn new(
        state: HistoryState,
        icons: SourceIcons,
        bus: EventBus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| {
            InputState::new(window, cx).placeholder("搜索剪贴板历史")
        });
        let list_focus = cx.focus_handle();
        // The list is what the arrow keys and Delete are for, so it starts focused; clicking
        // the search field moves that ownership to the field, and clicking any row brings it
        // back.
        list_focus.focus(window, cx);
        // One unchanged size per row: the list virtualizes by offset, so it needs the
        // sizes up front rather than a measurement pass.
        let row_sizes = Rc::new(vec![size(px(0.0), px(ROW_HEIGHT_PX)); state.items().len()]);
        // Re-read the field on any input event rather than matching one variant: the query
        // is whatever the field holds now, and `set_query` is a no-op when it did not change,
        // so this cannot loop.
        cx.subscribe(&query, |this, _, _event: &InputEvent, cx| {
            this.apply_query(cx);
        })
        .detach();
        // A new clip is stored by whoever owns the clipboard monitor, which is another
        // process while the Tauri host is still alive — so the row list cannot be re-read
        // from a local callback. It is re-read from the event channel, and a window that
        // regains focus re-reads too, because the two shells share only the database.
        let mut stream = bus.subscribe();
        let _events = cx.spawn(async move |this, cx| {
            while let Some(event) = stream.next().await {
                if !matches!(event, AppEvent::Clipboard(_)) {
                    continue;
                }
                // The store read happens here and the state change happens inside
                // `Entity::update`, which is the rule that keeps rendering single-threaded.
                if this.update(cx, |view, cx| view.refresh(cx)).is_err() {
                    // The view is gone; stop draining rather than spinning on a closed bus.
                    break;
                }
            }
        });
        let _activation = cx.observe_window_activation(window, move |this, window, cx| {
            if window.is_window_active() {
                this.refresh(cx);
            }
        });
        Self {
            state,
            query,
            icons,
            row_sizes,
            list_scroll: VirtualListScrollHandle::new(),
            thumbnails: HashMap::new(),
            pending_thumbnails: HashSet::new(),
            loading_more: false,
            status: None,
            _events,
            _activation,
            list_focus,
        }
    }

    /// Read-only accessors for the rows the view is showing. Public because the UI
    /// integration tests assert against them (`tests/ui.rs`).
    pub fn items(&self) -> &[ClipSummary] {
        self.state.items()
    }

    pub fn selected_id(&self) -> Option<&str> {
        self.state.selected_id()
    }

    pub fn query(&self) -> &str {
        self.state.query()
    }

    /// Whether the store has more rows behind what is loaded (read by the UI tests).
    pub fn has_more(&self) -> bool {
        self.state.has_more()
    }

    /// How many row previews have been read and decoded (read by the UI tests).
    pub fn loaded_previews(&self) -> usize {
        self.thumbnails.len()
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

    /// Re-read the list, keeping the rows the user had already loaded.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        // An empty query still needs the field's text: the user may have typed while a clip
        // arrived, and the refresh must stay on the same filter the list is showing.
        if let Err(error) = self.state.refresh() {
            eprintln!("[snapclip-app] history refresh failed: {error}");
            return;
        }
        self.row_sizes =
            Rc::new(vec![size(px(0.0), px(ROW_HEIGHT_PX)); self.state.items().len()]);
        cx.notify();
    }

    /// Switch the type filter (`None` = every kind).
    fn apply_kind(&mut self, kind: Option<PayloadKind>, cx: &mut Context<Self>) {
        match self.state.set_kind(kind) {
            Ok(true) => {
                self.resize_rows();
                cx.notify();
            }
            Ok(false) => {}
            Err(error) => eprintln!("[snapclip-app] history filter failed: {error}"),
        }
    }

    fn resize_rows(&mut self) {
        self.row_sizes =
            Rc::new(vec![size(px(0.0), px(ROW_HEIGHT_PX)); self.state.items().len()]);
    }

    /// Append the next page, if there is one.
    fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading_more || !self.state.has_more() {
            return;
        }
        self.loading_more = true;
        match self.state.load_more() {
            Ok(0) => {
                // The cursor ran out without producing rows; stop asking on every frame.
                self.status = Some("没有更多记录了".to_string());
            }
            Ok(added) => {
                self.status = Some(format!("又加载了 {added} 条"));
            }
            Err(error) => {
                self.status = Some(format!("加载更多失败：{error}"));
            }
        }
        self.loading_more = false;
        self.resize_rows();
        cx.notify();
    }

    /// Ask before deleting: destruction is not a hover action, and the Design Guides want the
    /// object named in the confirmation.
    fn request_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.state.selected_id().map(str::to_string) else {
            self.status = Some("没有选中的记录".to_string());
            cx.notify();
            return;
        };
        let label = self
            .state
            .items()
            .iter()
            .find(|item| item.id == id)
            .map(entry_label)
            .unwrap_or_else(|| id.clone());
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            alert
                .title("删除这条记录？")
                .description(format!("将从历史记录中永久删除：{label}"))
                .confirm()
                .ok_text("删除")
                .cancel_text("取消")
                .on_ok(move |_, _, cx| {
                    // `update` on the view is fine here: this callback is owned by the dialog,
                    // not by the view, so nothing is locked while it runs.
                    let _ = view.update(cx, |view, cx| view.delete_selected(cx));
                    true
                })
        });
    }

    fn delete_selected(&mut self, cx: &mut Context<Self>) {
        match self.state.delete_selected() {
            Ok(Some(_)) => {
                self.thumbnails.clear();
                self.pending_thumbnails.clear();
                self.status = Some("已删除".to_string());
            }
            Ok(None) => self.status = Some("没有可删除的记录".to_string()),
            Err(error) => self.status = Some(format!("删除失败：{error}")),
        }
        self.resize_rows();
        cx.notify();
    }

    /// The preview for a row, if the row has an image payload.
    ///
    /// Loading is *lazy* by construction: this is only called for rows the virtual list is
    /// actually rendering, and the read is deferred to the end of the frame so rendering
    /// never blocks on the database.
    fn thumbnail(
        &mut self,
        item: &ClipSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let has_image = item
            .payloads
            .iter()
            .any(|payload| payload.kind == PayloadKind::Image);
        if !has_image {
            return None;
        }
        if !self.thumbnails.contains_key(&item.id) && !self.pending_thumbnails.contains(&item.id) {
            self.pending_thumbnails.insert(item.id.clone());
            let id = item.id.clone();
            cx.defer_in(window, move |this, _window, cx| this.load_thumbnail(&id, cx));
            // Reserve the space while it loads, so the row does not jump.
            return Some(div().size(px(THUMBNAIL_PX)).into_any_element());
        }
        self.thumbnails.get(&item.id).and_then(|loaded| {
            loaded.as_ref().map(|image| {
                img(Arc::clone(image))
                    .size(px(THUMBNAIL_PX))
                    .object_fit(ObjectFit::Contain)
                    .into_any_element()
            })
        })
    }

    fn load_thumbnail(&mut self, clip_id: &str, cx: &mut Context<Self>) {
        self.pending_thumbnails.remove(clip_id);
        // A preview cache that grows with the history is a leak, and the visible set is what
        // matters; dropping everything re-reads only the rows on screen.
        if self.thumbnails.len() > THUMBNAIL_CACHE_LIMIT {
            self.thumbnails.clear();
        }
        let Some(item) = self
            .state
            .items()
            .iter()
            .find(|item| item.id == clip_id)
            .cloned()
        else {
            return;
        };
        let decoded = match self.state.image_bytes(&item) {
            Some(Ok(bytes)) => Some(Arc::new(Image::from_bytes(ImageFormat::Png, bytes))),
            Some(Err(error)) => {
                eprintln!("[snapclip-app] history preview failed for {clip_id}: {error}");
                None
            }
            None => None,
        };
        self.thumbnails.insert(clip_id.to_string(), decoded);
        cx.notify();
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
            None => "没有选中的记录".to_string(),
            Some(Err(error)) => format!("读取失败：{error}"),
            Some(Ok((_payload, kind, bytes))) => {
                let outcome = match kind {
                    PayloadKind::Image => match decode_to_rgba8(&bytes) {
                        Ok(decoded) => {
                            let (width, height) = (decoded.width(), decoded.height());
                            clipboard.copy_image(width, height, decoded.into_raw())
                        }
                        Err(error) => Err(format!("图片解码失败：{error}")),
                    },
                    _ => match String::from_utf8(bytes) {
                        Ok(text) => clipboard.copy_text(&text),
                        Err(_) => Err("这条记录不是文本".to_string()),
                    },
                };
                match outcome {
                    Ok(()) => match kind {
                        PayloadKind::Image => "已复制图片".to_string(),
                        _ => "已复制文本".to_string(),
                    },
                    Err(error) => format!("复制失败：{error}"),
                }
            }
        });
        cx.notify();
    }

    fn render_empty(&self, cx: &Context<Self>) -> impl IntoElement {
        let message = if self.state.items().is_empty() && !self.state.query().trim().is_empty() {
            "没有匹配的记录".to_string()
        } else {
            // First-run state: say what fills the list rather than leaving it blank.
            "复制的文字和图片会出现在这里".to_string()
        };
        div()
            .size_full()
            .v_flex()
            .items_center()
            .justify_center()
            .text_color(cx.theme().muted_foreground)
            .child(message)
    }

    /// Whether the search field currently owns typing.
    fn query_focused(&self, window: &Window, cx: &Context<Self>) -> bool {
        self.query.read(cx).focus_handle(cx).is_focused(window)
    }

    /// The type filter, mirroring the options the old panel offered (all/text/image/files).
    fn filter_bar(&self, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        const OPTIONS: [(&str, &str, Option<PayloadKind>); 4] = [
            ("history-filter-all", "全部", None),
            ("history-filter-text", "文本", Some(PayloadKind::Text)),
            ("history-filter-image", "图片", Some(PayloadKind::Image)),
            ("history-filter-files", "文件", Some(PayloadKind::Files)),
        ];
        let mut bar = div().h_flex().gap_1();
        for (id, label, value) in OPTIONS {
            let active = self.state.kind() == value.as_ref();
            let chip = div()
                .id(id)
                .px_2()
                .py_1()
                .rounded_sm()
                .text_xs()
                .border_1()
                .border_color(if active { theme.primary } else { theme.border })
                .text_color(if active {
                    theme.primary
                } else {
                    theme.muted_foreground
                })
                .on_click(cx.listener(move |this, _, _, cx| this.apply_kind(value.clone(), cx)))
                .child(label);
            #[cfg(feature = "test-support")]
            let chip = chip
                .role(gpui_kit::Role::Button)
                .aria_label(label)
                .aria_selected(active)
                .test_support();
            bar = bar.child(chip);
        }
        bar.into_any_element()
    }

    fn delete_button(&self, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        let enabled = self.state.selected_id().is_some();
        let button = div()
            .id("history-delete")
            .px_2()
            .py_1()
            .rounded_sm()
            .text_xs()
            .border_1()
            .border_color(if enabled { theme.border } else { theme.border })
            .text_color(if enabled {
                theme.foreground
            } else {
                theme.muted_foreground
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if enabled {
                    this.request_delete(window, cx);
                }
            }))
            .child("删除所选");
        #[cfg(feature = "test-support")]
        let button = button
            .role(gpui_kit::Role::Button)
            .aria_label("删除所选")
            .test_support();
        button.into_any_element()
    }
}

/// The words used to name one entry in a confirmation.
///
/// A dialog that says "delete clip-1791288801200-1?" names the database, not the entry; the
/// user recognises the preview.
fn entry_label(item: &ClipSummary) -> String {
    item.preview_text
        .as_deref()
        .map(|text| one_line_preview(text, 48))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| kind_label(&item.primary_kind).to_string())
}

/// Collapse any text into one displayable line of at most `max_chars` characters.
///
/// Clipboard content is frequently a code block: newlines, tabs and long runs of spaces. A
/// row is exactly one line tall, and a GPUI div does not clip its children, so the row is
/// kept honest by making the text one line *before* it reaches the layout rather than by
/// hoping the layout truncates it. Control characters are folded in with the whitespace:
/// they have no width and would otherwise silently eat part of the visible character budget.
fn one_line_preview(text: &str, max_chars: usize) -> String {
    let mut collapsed = String::with_capacity(text.len().min(max_chars * 4));
    let mut pending_space = false;
    let mut chars = text.chars();
    let mut truncated = false;
    loop {
        let Some(character) = chars.next() else { break };
        let is_space = character.is_whitespace() || character.is_control();
        if is_space {
            // A leading space is dropped, and a run of whitespace becomes one separator.
            pending_space = !collapsed.is_empty();
            continue;
        }
        if pending_space {
            if collapsed.chars().count() >= max_chars {
                truncated = true;
                break;
            }
            collapsed.push(' ');
            pending_space = false;
        }
        if collapsed.chars().count() >= max_chars {
            truncated = true;
            break;
        }
        collapsed.push(character);
    }
    if truncated || chars.next().is_some() {
        if collapsed.chars().count() >= max_chars {
            // Make room for the ellipsis so the result still fits the row.
            let keep = max_chars.saturating_sub(1).max(1);
            collapsed = collapsed.chars().take(keep).collect();
        }
        collapsed.push('…');
    }
    collapsed
}

/// `来源程序 · 图片 · OCR 完成` — the secondary line of a row.
///
/// Written in the user's words, not the enum's: `PayloadKind::Text` and
/// `OcrStatus::Done` are internal names, and the Design Guides are explicit that the
/// interface speaks nouns a person uses ("Write each language, do not translate its shape").
fn row_meta(item: &ClipSummary) -> String {
    let mut parts = Vec::new();
    if let Some(app) = item.source_app.as_deref().filter(|app| !app.is_empty()) {
        parts.push(app.to_string());
    }
    parts.push(kind_label(&item.primary_kind).to_string());
    if let Some(ocr) = ocr_label(item.ocr_status) {
        parts.push(ocr.to_string());
    }
    parts.join(" · ")
}

/// The word for a payload kind.
fn kind_label(kind: &PayloadKind) -> &'static str {
    match kind {
        PayloadKind::Text => "文本",
        PayloadKind::Html => "HTML",
        PayloadKind::Rtf => "富文本",
        PayloadKind::Image => "图片",
        PayloadKind::Files => "文件",
        PayloadKind::Other => "其他",
    }
}

/// The word for a recognition state, or `None` when there is nothing worth saying.
///
/// `None` and `Queued` stay silent: a row whose text has simply not been read yet is not a
/// state the user needs to see, and showing "未识别" on every fresh entry is noise. A
/// running job *is* worth showing, because it explains why the text is missing.
fn ocr_label(status: snapclip_model::OcrStatus) -> Option<&'static str> {
    use snapclip_model::OcrStatus;
    match status {
        OcrStatus::None | OcrStatus::Queued => None,
        OcrStatus::Running => Some("识别中"),
        OcrStatus::Done => Some("OCR 完成"),
        OcrStatus::Failed => Some("OCR 失败"),
        OcrStatus::Skipped => Some("OCR 跳过"),
    }
}

impl Render for HistoryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let count = self.state.items().len();
        let has_rows = count > 0;
        let root = div()
            .v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .key_context("HistoryView")
            .track_focus(&self.list_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                if this.query_focused(window, cx) {
                    // While the field is typing, the keys keep their editing meaning. Two
                    // exceptions, because the screen gave them a meaning the field does not
                    // own: Escape clears the filter, Enter copies the selected row.
                    match key {
                        "escape" => this.clear_query(window, cx),
                        "enter" => this.copy_selected(cx),
                        // Pull, do not push: this InputState's text changes are not delivered
                        // as a subscribe-able event in this version (the guides' own example
                        // reads the field inside a handler for the same reason), so the view
                        // re-reads the field on the keystrokes it types.
                        _ => this.apply_query(cx),
                    }
                    return;
                }
                match key {
                    "up" => this.move_selection(-1, cx),
                    "down" => this.move_selection(1, cx),
                    "escape" => this.clear_query(window, cx),
                    "enter" => this.copy_selected(cx),
                    // The list owns this key: the search field never deletes a stored clip.
                    "delete" | "backspace" => this.request_delete(window, cx),
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
                    .child({
                        // Component inputs register themselves by id; only custom native
                        // nodes need `.test_support()` (see the guides' example).
                        Input::new(&self.query).id("history-query")
                    })
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(match count {
                                        0 => "暂无记录".to_string(),
                                        1 => "共 1 条".to_string(),
                                        n => format!("共 {n} 条"),
                                    }),
                            )
                            .child(div().flex_1())
                            .child(self.delete_button(theme, cx)),
                    )
                    .children(self.status.clone().map(|status| {
                        let node = div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(status.clone());
                        #[cfg(feature = "test-support")]
                        let node = node
                            .id("history-status")
                            .role(gpui_kit::Role::Status)
                            .aria_label(status)
                            .test_support();
                        node
                    }))
                    .child(self.filter_bar(theme, cx)),
            )
            .child(if has_rows {
                let list = v_virtual_list(
                    cx.entity(),
                    "history-rows",
                    self.row_sizes.clone(),
                    |view, range, window, cx| {
                        // Rows are built here rather than in a helper: a `-> impl IntoElement`
                        // helper would capture `&self` and `&item` under Rust 2024's capture
                        // rules, so the returned element would outlive what it borrows.
                        // Copy the colours out before the row loop: the loop needs `&mut cx`
                        // (to queue a deferred preview read), and a live `&Theme` borrowed
                        // from `cx` would forbid that.
                        let theme = cx.theme();
                        let row_border = theme.border;
                        let selected_bg = theme.accent;
                        let selection = theme.primary;
                        let muted = theme.muted_foreground;
                        let selected_id = view.state.selected_id().map(str::to_string);
                        range
                            .filter_map(|index| {
                                let item = view.state.items().get(index)?.clone();
                                let selected = selected_id.as_deref() == Some(item.id.as_str());
                                let icon = item
                                    .source_exe_path
                                    .as_deref()
                                    .and_then(|exe| view.icons.png_path(exe));
                                // An entry with no text preview (an image, a file list) still
                                // needs a first line, and it must be a word a person reads —
                                // `format!("{:?}")` would put `Image` on screen.
                                let preview = item
                                    .preview_text
                                    .as_deref()
                                    // Clipboard previews are usually source code: a raw
                                    // multi-line string in a fixed-height row paints over the
                                    // row below it, because a GPUI div does not clip by
                                    // default. Collapsing it to one line is the root fix; the
                                    // row's own `overflow_hidden` below is the belt.
                                    .map(|text| one_line_preview(text, PREVIEW_CHARS))
                                    .filter(|text| !text.is_empty())
                                    .unwrap_or_else(|| kind_label(&item.primary_kind).to_string());
                                let meta = row_meta(&item);
                                let id = item.id.clone();
                                let row = div()
                                    .id(("history-row", index))
                                    .h(px(ROW_HEIGHT_PX))
                                    // Inside the marker wrapper: take the rest of the width and
                                    // be allowed to shrink, so long previews ellipse instead of
                                    // pushing the row wider than the list.
                                    .flex_1()
                                    .min_w_0()
                                    // Nothing may paint outside the row, whatever the text
                                    // metrics turn out to be.
                                    .overflow_hidden()
                                    .px_3()
                                    .gap_3()
                                    .h_flex()
                                    .items_center()
                                    .border_b_1()
                                    .border_color(row_border)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        // Clicking a row hands the keyboard back to the list, so
                                        // ↑/↓/Delete keep working after a mouse click.
                                        this.list_focus.focus(window, cx);
                                        this.state.select(&id);
                                        cx.notify();
                                    }));
                                // Registered for the UI tests, which assert geometry: the bug
                                // this guards against was a row's text painting over the row
                                // below it, and only a bounds comparison catches that.
                                #[cfg(feature = "test-support")]
                                let row = row.test_support();
                                // Selected rows get a distinct, stable treatment; ordinary
                                // control flow keeps that readable.
                                let row = if selected { row.bg(selected_bg) } else { row };
                                // The marker sits *outside* the row's padding, so a selected
                                // row's text does not shift right and the eye can find the
                                // selection while scanning the left edge of the column.
                                let marker = if selected {
                                    div()
                                        .w(px(SELECTION_BAR_PX))
                                        .h_full()
                                        .flex_none()
                                        .bg(selection)
                                } else {
                                    div().w(px(SELECTION_BAR_PX)).h_full().flex_none()
                                };
                                // Only rows the list is actually rendering ask for a preview,
                                // which is what makes the load lazy rather than eager.
                                let thumbnail = view.thumbnail(&item, window, cx);
                                Some(
                                    div()
                                        .h_flex()
                                        .w_full()
                                        .items_center()
                                        .child(marker)
                                        .child(row
                                        // A fixed leading column, so every row's text starts at
                                        // the same x whether or not the source app is known.
                                        .child(
                                            div()
                                                .w(px(16.0))
                                                .flex_none()
                                                .h_flex()
                                                .items_center()
                                                .justify_center()
                                                .child(match icon {
                                                    Some(path) => {
                                                        img(path).size(px(16.0)).into_any_element()
                                                    }
                                                    // Unknown source is a normal state, not an
                                                    // error: the space is reserved, not filled.
                                                    None => div()
                                                        .size(px(16.0))
                                                        .into_any_element(),
                                                }),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .v_flex()
                                                .gap_1()
                                                .child({
                                                    // Denser than body text: this is a list to
                                                    // scan, not a paragraph to read.
                                                    let line =
                                                        div().text_sm().truncate().child(preview);
                                                    // Registered so the UI test can measure the
                                                    // line: a multi-line preview is exactly what
                                                    // made rows paint over each other.
                                                    #[cfg(feature = "test-support")]
                                                    let line = line
                                                        .id(("history-preview", index))
                                                        .test_support();
                                                    line
                                                })
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(muted)
                                                        .truncate()
                                                        .child(meta),
                                                ),
                                        )
                                        // The preview is a fixed-size column on the right; a row
                                        // with no image simply does not have one.
                                        .children(thumbnail.map(|element| {
                                            div().flex_none().child(element)
                                        }))),
                                )
                            })
                            .collect::<Vec<_>>()
                    },
                )
                // Paging follows the user's scroll, like the old panel did, and the explicit
                // button below stays for keyboards and for when the trigger has nothing left
                // to fetch.
                .track_scroll(&self.list_scroll)
                .flex_1();

                div()
                    .v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(list)
                    .children(self.state.has_more().then(|| self.load_more_button(theme, cx)))
                    .into_any_element()
            } else {
                self.render_empty(cx).into_any_element()
            });

        // Reading the scroll handle is the previous frame's layout, which is exactly what
        // "the last rows are on screen" needs; the fetch itself is deferred so it cannot run
        // inside this render (and no theme borrow is alive here).
        if self.state.has_more() && !self.loading_more && self.near_the_end() {
            cx.defer_in(window, |this, _window, cx| this.load_more(cx));
        }
        root
    }
}

impl HistoryView {
    /// Whether the list is within a few rows of the bottom.
    fn near_the_end(&self) -> bool {
        let content = self.list_scroll.content_size().height;
        if content <= px(0.0) {
            // Nothing has been laid out yet; the next frame can decide.
            return false;
        }
        let viewport = self.list_scroll.bounds().size.height;
        let offset = self.list_scroll.offset().y;
        offset + viewport >= content - px(ROW_HEIGHT_PX * LOAD_MORE_THRESHOLD_ROWS)
    }

    fn load_more_button(&self, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        let button = div()
            .id("history-load-more")
            .w_full()
            .py_2()
            .h_flex()
            .justify_center()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.muted_foreground)
            .on_click(cx.listener(|this, _, _, cx| this.load_more(cx)))
            .child("加载更多");
        #[cfg(feature = "test-support")]
        let button = button
            .role(gpui_kit::Role::Button)
            .aria_label("加载更多")
            .test_support();
        button.into_any_element()
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

#[cfg(test)]
mod tests {
    use super::{kind_label, ocr_label, one_line_preview};
    use snapclip_model::{OcrStatus, PayloadKind};

    /// The row is one line tall, so the text has to be one line before layout sees it.
    #[test]
    fn a_multi_line_clip_becomes_one_readable_line() {
        // A code block is the common case, and the one that broke the layout: newlines in a
        // `truncate()`d div still painted over the row below.
        let collapsed = one_line_preview("项目 A\n  ↓\n打开资源管理器\n", 160);
        assert_eq!(collapsed, "项目 A ↓ 打开资源管理器");
        assert!(!collapsed.contains('\n'));

        // Tabs, carriage returns and other control characters fold in with the whitespace.
        assert_eq!(one_line_preview("a\t\tb\r\nc", 160), "a b c");
        // Leading and trailing whitespace do not become visible characters.
        assert_eq!(one_line_preview("   spaced   out   ", 160), "spaced out");
        // Text that is only whitespace has nothing to show.
        assert_eq!(one_line_preview("\n \t\n", 160), "");
    }

    #[test]
    fn a_long_clip_is_cut_to_the_row_budget_and_says_so() {
        let text = "x".repeat(500);
        let preview = one_line_preview(&text, 20);
        assert_eq!(preview.chars().count(), 20);
        assert!(preview.ends_with('…'));

        // Exactly at the budget: nothing is lost, so nothing is announced.
        let exact = one_line_preview(&"y".repeat(20), 20);
        assert_eq!(exact, "y".repeat(20));
        assert!(!exact.ends_with('…'));
    }

    #[test]
    fn the_row_speaks_the_users_words_not_the_enums() {
        for (kind, expected) in [
            (PayloadKind::Text, "文本"),
            (PayloadKind::Html, "HTML"),
            (PayloadKind::Rtf, "富文本"),
            (PayloadKind::Image, "图片"),
            (PayloadKind::Files, "文件"),
            (PayloadKind::Other, "其他"),
        ] {
            assert_eq!(kind_label(&kind), expected);
            // None of these may leak a Rust identifier into the interface.
            assert!(!kind_label(&kind).contains("::"));
        }
    }

    #[test]
    fn a_quiet_recognition_state_says_nothing_and_a_busy_one_explains_itself() {
        // Nothing to say: not recognised yet is the normal state of a fresh entry.
        assert_eq!(ocr_label(OcrStatus::None), None);
        assert_eq!(ocr_label(OcrStatus::Queued), None);
        // Worth saying: these explain why text is missing, or that it arrived.
        assert_eq!(ocr_label(OcrStatus::Running), Some("识别中"));
        assert_eq!(ocr_label(OcrStatus::Done), Some("OCR 完成"));
        assert_eq!(ocr_label(OcrStatus::Failed), Some("OCR 失败"));
        assert_eq!(ocr_label(OcrStatus::Skipped), Some("OCR 跳过"));
    }
}
