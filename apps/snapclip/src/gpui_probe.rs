//! A throwaway probe: can the save panel be built out of GPUI components instead of a
//! window with one native control?
//!
//! Nothing here is on a production path. It opens when `SNAPCLIP_GPUI_PROBE` is set, it is the
//! only window in that run, and it exists to answer two questions: does Chinese typing work with
//! the candidate window in place, and does the panel's right column — the six rows the mockup calls
//! 保存路径 / 文件名 / 保存后 / 尺寸 / 质量 / 预估大小 — look like the one the panel already draws.
//!
//! The authority is the mockup (`docs/design/save-panel-demo-v3.html`), not the panel's rectangles.
//! The first version of this view placed the panel's absolute rectangles and reproduced its
//! arithmetic a second time; that can express any misplacement, and it did — rows ran into each
//! other, because a framework control is taller than the box it was dropped into. A flow layout
//! cannot express an overlap, and the numbers it needs are the ones the mockup uses *between*
//! elements, each of which is one constant in [`mockup`] with the line it came from.
//!
//! Two things stay borrowed from the panel: the column's width, which is its own metric rather
//! than a rectangle, and the words, which are the specification's. The path row is the one that
//! has to *work*, not merely look: its three shortcuts and its `更改` button really change the
//! field, because a field whose buttons do nothing is the failure this exercise keeps finding.

use gpui_kit::base::StyledExt as _;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu};
use gpui_kit::*;
use snapclip_ui::panel::{
    DEMO, FMT_OPTIONS, QUALITY_HINT_TEXT, SIZE_MODE_WORDS, SIZE_PRESET_WORDS, VARIABLE_TOKENS,
};

/// The probe window's client size, matching the save panel so the eye compares like with like.
pub const PROBE_WIDTH: f32 = 920.0;
/// See [`PROBE_WIDTH`].
pub const PROBE_HEIGHT: f32 = 564.0;
/// The window's own padding, which the panel also uses around its content (`--pad: 16px`).
pub const PROBE_PADDING: f32 = 16.0;
/// The window title, which is what the taskbar and Alt-Tab show.
pub const PROBE_TITLE: &str = "保存图片 · GPUI 探针";
/// The name the panel starts with, so the field shows a template rather than a placeholder.
pub const PROBE_NAME: &str = "SnapClip_[日期]";
/// The two footer buttons, named once so the label and the id cannot drift.
pub const PROBE_CANCEL: &str = "取消";
/// See [`PROBE_CANCEL`].
pub const PROBE_SAVE: &str = "保存";
/// The path row's button, borrowed from the row that already draws it.
pub const PROBE_CHANGE: &str = "更改";
/// The folder button's glyph, which is all the mockup gives it (`:546` uses an icon).
pub const PROBE_OPEN: &str = "…";
/// The variable button's glyph, which is what the mockup puts in the name row's right end.
pub const PROBE_WAND: &str = "{ }";
/// What the format button shows before its menu is opened.
pub const PROBE_FORMAT: &str = ".PNG";
/// The three shortcuts under the path field (`:739`-`:741`).
pub const PROBE_SHORTCUTS: [&str; 3] = ["桌面", "图片", "下载"];
/// The two switches beside the saved-after label.
pub const PROBE_AFTER: [&str; 2] = ["复制路径", "打开文件夹"];
/// The row labels, in the order the mockup draws them (`:582`, `:599`, `:630`, `:667`).
pub const PROBE_LABELS: [&str; 6] = ["保存路径", "文件名", "保存后", "尺寸", "质量", "预估大小"];
/// The two halves of the size row, which the mockup labels `.cell-label` (`:237`).
pub const PROBE_SIZE_LABELS: [&str; 2] = ["宽", "高"];
/// The unit that follows each size cell (`:243`).
pub const PROBE_UNIT: &str = "px";
/// The quality row's switch (`:631`-`:635`).
pub const PROBE_LIMIT: &str = "限制体积";
/// What the estimated size shows before anything has been measured (`:668` shows a real value,
/// which this probe has no encoder to produce; the sample is the mockup's own).
pub const PROBE_ESTIMATE_SAMPLE: &str = "227.31 KB";

// A menu item's click handler is crate-private in this release — items carry an `action` instead,
// and actions are dispatched to the focus owner through GPUI's action system. Binding that for two
// throwaway menus is a mechanism the real migration should design rather than inherit from a probe,
// so the items below are section labels: the menus open, the words are the mockup's, and choosing
// one is recorded as not yet wired. See `docs/41`.

/// The mockup's own spacing and control sizes, one constant each, with the line it comes from.
///
/// These are the numbers *between* elements — gaps, insets, control heights — which is what a flow
/// layout consumes. They are deliberately not positions: a position list is the panel's arithmetic
/// and copying it is how the first layout managed to overlap its own rows.
pub mod mockup {
    /// `.form { gap: 12px }` (`:162`): between rows, 16 between groups once `.grp` adds its 4.
    pub const ROW_GAP: f32 = 12.0;
    /// `.form-row.grp { margin-top: 4px }` (`:163`), with the comment "组间 16 / 组内 12".
    pub const GROUP_GAP: f32 = 4.0;
    /// `.form-row { gap: 6px }` (`:164`): between a row's header and its content.
    pub const HEADER_GAP: f32 = 6.0;
    /// `.row-header { height: 22px }` (`:165`).
    pub const HEADER_HEIGHT: f32 = 22.0;
    /// `.text-input { height: 32px }` (`:167`).
    pub const FIELD_HEIGHT: f32 = 32.0;
    /// `.chip { height: 24px }` (`:192`).
    pub const CHIP_HEIGHT: f32 = 24.0;
    /// `.seg { height: var(--row) }` (`:263`) with `--row: 28px` (`:47`).
    pub const SEG_HEIGHT: f32 = 28.0;
    /// `.seg.quality { --seg-w: 34px }` (`:264`): one option's width.
    pub const SEG_OPTION_WIDTH: f32 = 34.0;
    /// `.est-size { height: var(--row) }` (`:269`).
    pub const ESTIMATE_HEIGHT: f32 = 28.0;
    /// `.file-row { gap: 8px }` (`:171`): between the name box and its two buttons.
    pub const FILE_GAP: f32 = 8.0;
    /// `.path-row { gap: 8px }` (`:188`): between the path box and its two buttons.
    pub const PATH_GAP: f32 = 8.0;
    /// `.path-chips { gap: 6px }` (`:191`).
    pub const SHORTCUT_GAP: f32 = 6.0;
    /// `.size-presets { padding-left: 28px; gap: 6px; margin-top: 2px }` (`:258`).
    pub const PRESET_INDENT: f32 = 28.0;
    /// See [`PRESET_INDENT`].
    pub const PRESET_GAP: f32 = 6.0;
    /// See [`PRESET_INDENT`].
    pub const PRESET_MARGIN: f32 = 2.0;
    /// `.size-mode { padding: 2px }` (`:225`).
    pub const MODE_PADDING: f32 = 2.0;
    /// `.mode-btn { height: 24px }` (`:106`).
    pub const MODE_BUTTON_HEIGHT: f32 = 24.0;
    /// `.cell-label { width: 20px }` (`:237`).
    pub const CELL_LABEL_WIDTH: f32 = 20.0;
    /// `.cell { gap }` — the mockup spaces the label from its box by the cell's own gap (`:236`).
    pub const CELL_GAP: f32 = 6.0;
    /// `.fmt-btn { width: var(--fmt-w) }` (`:173`) with `--fmt-w: 80px` (`:49`).
    pub const FORMAT_WIDTH: f32 = 80.0;
    /// `.btn-ghost { height: 32px }` (`:189`).
    pub const GHOST_HEIGHT: f32 = 32.0;
    /// `.btn-ghost.icon-only { width: 32px }` (`:205`).
    pub const GHOST_ICON_WIDTH: f32 = 32.0;
    /// `.btn { height: var(--btn-h) }` (`:279`) with `--btn-h: 32px` (`:50`).
    pub const BUTTON_HEIGHT: f32 = 32.0;
    /// `.row-header-actions { gap: 8px }` (`:224`).
    pub const HEADER_ACTION_GAP: f32 = 8.0;
    /// `.quality-line { gap: 12px }` (`:306`).
    pub const QUALITY_LINE_GAP: f32 = 12.0;

    /// The mockup's six rows, each as the parts it holds under its header.
    ///
    /// A row's height is not a number in the mockup; it is the sum of its parts, and writing the
    /// sum here means the layout and the assertion read the same parts rather than two copies of a
    /// number. The path row is the one that is easy to misread: it holds the box *and* the chips
    /// underneath, separated by the row's own gap, which is why that row is sixty two tall while
    /// its box is thirty two. The size row adds the presets' 2 dip margin on top of the same shape.
    pub fn row_heights() -> [(&'static str, f32); 6] {
        let sum = |parts: &[f32]| -> f32 { parts.iter().sum() };
        [
            ("保存路径", sum(&[FIELD_HEIGHT, HEADER_GAP, CHIP_HEIGHT])),
            ("文件名", sum(&[FIELD_HEIGHT])),
            ("保存后", sum(&[CHIP_HEIGHT])),
            (
                "尺寸",
                sum(&[FIELD_HEIGHT, HEADER_GAP, PRESET_MARGIN, CHIP_HEIGHT]),
            ),
            ("质量", sum(&[SEG_HEIGHT])),
            ("预估大小", sum(&[ESTIMATE_HEIGHT])),
        ]
    }

    /// How tall the whole column is: headers, the parts under them, and the gaps between rows.
    ///
    /// Every row pays a header and the gap under it, so those two are counted once per row; the
    /// parts above already include the gaps *inside* a row, which is why they are not counted here.
    /// Two rows open a new group — 尺寸 and 预估大小 — so the column pays [`GROUP_GAP`] twice on
    /// top of the five ordinary gaps.
    pub fn column_height() -> f32 {
        let rows = row_heights();
        let count = rows.len() as f32;
        let content: f32 = rows.iter().map(|(_, height)| *height).sum();
        count * (HEADER_HEIGHT + HEADER_GAP) + content + (count - 1.0) * ROW_GAP + 2.0 * GROUP_GAP
    }
}

/// The right column's width, taken from the panel's own metric rather than from a rectangle.
pub fn column_width() -> f32 {
    DEMO.form_width() as f32
}

/// A dip constant as pixels, converted in one place so the scale factor is applied once.
fn dip(scale: f32, value: f32) -> f32 {
    value * scale
}

/// A row: its header line, the gap under it, and the parts it holds.
///
/// This is the whole reason the first layout overlapped. A row is not placed at a position; it is
/// a column of three parts, and the row beneath it starts where this one ends. Nothing in this
/// shape can express an overlap, so the defect the eye found cannot come back by accident.
fn row(
    header: impl IntoElement,
    content: impl IntoElement,
    opens_group: bool,
    scale: f32,
) -> impl IntoElement {
    let group_margin = if opens_group {
        dip(scale, mockup::GROUP_GAP)
    } else {
        0.0
    };
    div()
        .v_flex()
        .mt(px(group_margin))
        .gap(px(dip(scale, mockup::HEADER_GAP)))
        .child(
            div()
                .h(px(dip(scale, mockup::HEADER_HEIGHT)))
                .h_flex()
                .items_center()
                .justify_between()
                .child(header),
        )
        .child(content)
}

/// A row label, in the mockup's own type size.
fn row_label(text: &'static str) -> impl IntoElement {
    div().text_xs().child(text)
}

/// The probe's state: the fields it can edit, and the choices it shows.
pub struct GpuiProbe {
    name: Entity<InputState>,
    path: Entity<InputState>,
    width: Entity<InputState>,
    height: Entity<InputState>,
    /// The chosen format, as an index into [`FMT_OPTIONS`].
    format: usize,
    /// The chosen size mode, as an index into [`SIZE_MODE_WORDS`].
    mode: usize,
    /// Which of the saved-after switches are on.
    after: [bool; 2],
}

impl GpuiProbe {
    /// Build the view, seeding the name field with the template the panel uses.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(PROBE_NAME)
                .default_value(PROBE_NAME)
        });
        let path = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("C:\\\\")
                .default_value(crate::save::dirs::desktop())
        });
        // The size cells start at the capture's own size in the panel; this probe has no capture,
        // so it shows the size the mockup's own sample uses rather than inventing one.
        let width = cx.new(|cx| InputState::new(window, cx).default_value("1440"));
        let height = cx.new(|cx| InputState::new(window, cx).default_value("900"));
        Self {
            name,
            path,
            width,
            height,
            format: 0,
            mode: 0,
            after: [false, false],
        }
    }

    /// Point the path field at one of the shortcuts' folders.
    fn go_to(&mut self, directory: String, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.path.clone();
        path.update(cx, |state, cx| state.set_value(directory, window, cx));
        cx.notify();
        window.refresh();
    }
}

impl Render for GpuiProbe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let scale = window.scale_factor();
        let field_h = px(dip(scale, mockup::FIELD_HEIGHT));
        let ghost_h = px(dip(scale, mockup::GHOST_HEIGHT));
        let chip_h = px(dip(scale, mockup::CHIP_HEIGHT));

        // 保存路径: a read-only box, its two buttons, then the three shortcuts underneath.
        let path_row = row(
            row_label(PROBE_LABELS[0]),
            div()
                .v_flex()
                .gap(px(dip(scale, mockup::HEADER_GAP)))
                .child(
                    div()
                        .h_flex()
                        .gap(px(dip(scale, mockup::PATH_GAP)))
                        .child(
                            Input::new(&self.path)
                                .id("probe-path")
                                .readonly(true)
                                .h(field_h)
                                .flex_1(),
                        )
                        .child(
                            Button::new("probe-change")
                                .label(PROBE_CHANGE)
                                .h(ghost_h)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if let Some(folder) = pick_folder() {
                                        this.go_to(folder, window, cx);
                                    }
                                })),
                        )
                        .child(
                            Button::new("probe-open")
                                .label(PROBE_OPEN)
                                .h(ghost_h)
                                .w(px(dip(scale, mockup::GHOST_ICON_WIDTH)))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let current = this.path.read(cx).value().to_string();
                                    open_in_explorer(&current);
                                    window.refresh();
                                })),
                        ),
                )
                .child(
                    div()
                        .h_flex()
                        .gap(px(dip(scale, mockup::SHORTCUT_GAP)))
                        .children(PROBE_SHORTCUTS.into_iter().enumerate().map(
                            |(index, word)| {
                                Button::new(format!("probe-shortcut-{index}"))
                                    .label(word)
                                    .h(chip_h)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        let directory = shortcut_directory(index);
                                        this.go_to(directory, window, cx);
                                    }))
                            },
                        )),
                ),
            false,
            scale,
        );

        // 文件名: the box, the variable button and the format button, in that order (`:552`).
        let name_row = row(
            row_label(PROBE_LABELS[1]),
            div()
                .h_flex()
                .gap(px(dip(scale, mockup::FILE_GAP)))
                .child(Input::new(&self.name).id("probe-name").h(field_h).flex_1())
                .child(
                    Button::new("probe-wand")
                        .label(PROBE_WAND)
                        .h(field_h)
                        .dropdown_menu(|menu, _, _| variable_menu(menu)),
                )
                .child(
                    Button::new("probe-format")
                        .label(FMT_OPTIONS[self.format])
                        .h(field_h)
                        .w(px(dip(scale, mockup::FORMAT_WIDTH)))
                        .dropdown_menu(|menu, _, _| format_menu(menu)),
                ),
            false,
            scale,
        );

        // 保存后: the two switches, which really toggle.
        let saved_row = row(
            row_label(PROBE_LABELS[2]),
            div()
                .h_flex()
                .gap(px(dip(scale, mockup::SHORTCUT_GAP)))
                .children(PROBE_AFTER.into_iter().enumerate().map(|(index, word)| {
                    let on = self.after[index];
                    let button = Button::new(format!("probe-after-{index}"))
                        .label(word)
                        .h(chip_h);
                    let button = if on { button.primary() } else { button };
                    button.on_click(cx.listener(move |this, _, _, cx| {
                        this.after[index] = !this.after[index];
                        cx.notify();
                    }))
                })),
            false,
            scale,
        );

        // 尺寸: a two-option mode group in the header's right end (`:605`), then the two cells,
        // then the four presets, which are indented past the cell labels (`:258`).
        let size_header = div()
            .h_flex()
            .items_center()
            .gap(px(dip(scale, mockup::HEADER_ACTION_GAP)))
            .child(row_label(PROBE_LABELS[3]))
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .p(px(dip(scale, mockup::MODE_PADDING)))
                    .gap(px(dip(scale, mockup::MODE_PADDING)))
                    .children(SIZE_MODE_WORDS.into_iter().enumerate().map(|(index, word)| {
                        let button = Button::new(format!("probe-mode-{index}"))
                            .label(word)
                            .h(px(dip(scale, mockup::MODE_BUTTON_HEIGHT)));
                        let button = if self.mode == index {
                            button.primary()
                        } else {
                            button
                        };
                        button.on_click(cx.listener(move |this, _, _, cx| {
                            this.mode = index;
                            cx.notify();
                        }))
                    })),
            );
        let size_content = div()
            .v_flex()
            .gap(px(dip(scale, mockup::HEADER_GAP)))
            .child(
                div()
                    .h_flex()
                    .gap(px(dip(scale, mockup::QUALITY_LINE_GAP)))
                    .child(
                        div()
                            .h_flex()
                            .flex_1()
                            .gap(px(dip(scale, mockup::CELL_GAP)))
                            .child(
                                div()
                                    .w(px(dip(scale, mockup::CELL_LABEL_WIDTH)))
                                    .h_flex()
                                    .items_center()
                                    .child(row_label(PROBE_SIZE_LABELS[0])),
                            )
                            .child(Input::new(&self.width).id("probe-width").h(field_h).flex_1())
                            .child(
                                div()
                                    .h_flex()
                                    .items_center()
                                    .child(row_label(PROBE_UNIT)),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .flex_1()
                            .gap(px(dip(scale, mockup::CELL_GAP)))
                            .child(
                                div()
                                    .w(px(dip(scale, mockup::CELL_LABEL_WIDTH)))
                                    .h_flex()
                                    .items_center()
                                    .child(row_label(PROBE_SIZE_LABELS[1])),
                            )
                            .child(Input::new(&self.height).id("probe-height").h(field_h).flex_1())
                            .child(
                                div()
                                    .h_flex()
                                    .items_center()
                                    .child(row_label(PROBE_UNIT)),
                            ),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .mt(px(dip(scale, mockup::PRESET_MARGIN)))
                    .pl(px(dip(scale, mockup::PRESET_INDENT)))
                    .gap(px(dip(scale, mockup::PRESET_GAP)))
                    .children(SIZE_PRESET_WORDS.into_iter().enumerate().map(|(index, word)| {
                        Button::new(format!("probe-preset-{index}"))
                            .label(word)
                            .h(chip_h)
                    })),
            );
        let size_row = row(size_header, size_content, false, scale);

        // 质量: for PNG the mockup hides the tiers and states why (`:638`), so the row is that
        // sentence and the limit switch, and nothing is a control that would do nothing.
        let quality_header = div()
            .h_flex()
            .items_center()
            .gap(px(dip(scale, mockup::HEADER_ACTION_GAP)))
            .child(row_label(PROBE_LABELS[4]))
            .child(row_label(PROBE_LIMIT));
        let quality_content = div()
            .h(px(dip(scale, mockup::SEG_HEIGHT)))
            .h_flex()
            .items_center()
            .child(div().text_xs().child(QUALITY_HINT_TEXT));
        let quality_row = row(quality_header, quality_content, true, scale);

        // 预估大小: the value sits at the row's right end (`:668`), as the mockup draws it.
        let estimate_row = row(
            row_label(PROBE_LABELS[5]),
            div()
                .h(px(dip(scale, mockup::ESTIMATE_HEIGHT)))
                .h_flex()
                .items_center()
                .justify_end()
                .child(div().text_sm().child(PROBE_ESTIMATE_SAMPLE)),
            true,
            scale,
        );

        div()
            .relative()
            .size_full()
            .p(px(dip(scale, PROBE_PADDING)))
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                div()
                    .w(px(dip(scale, column_width())))
                    .v_flex()
                    .gap(px(dip(scale, mockup::ROW_GAP)))
                    .child(path_row)
                    .child(name_row)
                    .child(saved_row)
                    .child(size_row)
                    .child(quality_row)
                    .child(estimate_row),
            )
            // The footer, which the mockup anchors to the window's bottom right (`:674`).
            .child(
                div()
                    .absolute()
                    .right(px(dip(scale, PROBE_PADDING)))
                    .bottom(px(dip(scale, PROBE_PADDING)))
                    .h_flex()
                    .gap(px(dip(scale, mockup::HEADER_ACTION_GAP)))
                    .child(
                        Button::new("probe-cancel")
                            .label(PROBE_CANCEL)
                            .h(px(dip(scale, mockup::BUTTON_HEIGHT))),
                    )
                    .child(
                        Button::new("probe-save")
                            .primary()
                            .label(PROBE_SAVE)
                            .h(px(dip(scale, mockup::BUTTON_HEIGHT))),
                    ),
            )
    }
}

/// The five variables, as the mockup lists them (`:192`-`:198`).
fn variable_menu(mut menu: PopupMenu) -> PopupMenu {
    for token in VARIABLE_TOKENS {
        menu = menu.label(token);
    }
    menu
}

/// The three formats, as the format menu lists them (`:203`).
fn format_menu(mut menu: PopupMenu) -> PopupMenu {
    for word in FMT_OPTIONS {
        menu = menu.label(word);
    }
    menu
}

/// Which folder a shortcut stands for, by the panel's own order (`:739`-`:741`).
///
/// This is a free function so the mapping can be asserted without a window: a shortcut that points
/// at the wrong folder is exactly the sort of "the button works but does the wrong thing" defect
/// this exercise keeps turning up.
pub fn shortcut_directory(index: usize) -> String {
    match index {
        0 => crate::save::dirs::desktop(),
        1 => crate::save::dirs::pictures(),
        2 => crate::save::dirs::downloads(),
        other => unreachable!("the mockup has three shortcuts, not {other}"),
    }
}

/// Open the folder the path field names in the shell, so the button is not decorative.
fn open_in_explorer(directory: &str) {
    let _ = std::process::Command::new("explorer").arg(directory).spawn();
}

/// Ask the system for a folder.
///
/// The save panel already has this call inside `save::window`, where it is private; the probe
/// cannot borrow it without making the panel wider for a throwaway. Duplicating twenty lines here
/// is the cheaper of the two, and it disappears with the probe.
fn pick_folder() -> Option<String> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{BROWSEINFOW, SHBrowseForFolderW, SHGetPathFromIDListW};

    let mut display = [0u16; 260];
    let mut info: BROWSEINFOW = unsafe { std::mem::zeroed() };
    info.pszDisplayName = display.as_mut_ptr();
    let pidl = unsafe { SHBrowseForFolderW(&info) };
    if pidl.is_null() {
        return None;
    }
    let mut path = [0u16; 260];
    let ok = unsafe { SHGetPathFromIDListW(pidl, path.as_mut_ptr()) };
    unsafe { CoTaskMemFree(pidl as *mut _) };
    if ok == 0 {
        return None;
    }
    let end = path.iter().position(|&unit| unit == 0).unwrap_or(path.len());
    Some(String::from_utf16_lossy(&path[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use snapclip_ui::panel::QUALITY_OPTION_WORDS;

    /// Every number in the layout is the mockup's, and the message says which line.
    ///
    /// This is an absolute assertion on purpose: comparing a constant against the function that
    /// returns it can never fail, and the defect this whole exercise keeps finding is a number that
    /// looks plausible. Reading each value beside the line it came from is what makes a typo show.
    #[::core::prelude::v1::test]
    fn every_number_is_the_mockups() {
        let cases = [
            (mockup::ROW_GAP, 12.0, ".form { gap: 12px } (:162)"),
            (
                mockup::GROUP_GAP,
                4.0,
                ".form-row.grp { margin-top: 4px } (:163) — 组间 16 / 组内 12",
            ),
            (mockup::HEADER_GAP, 6.0, ".form-row { gap: 6px } (:164)"),
            (
                mockup::HEADER_HEIGHT,
                22.0,
                ".row-header { height: 22px } (:165)",
            ),
            (
                mockup::FIELD_HEIGHT,
                32.0,
                ".text-input { height: 32px } (:167)",
            ),
            (mockup::CHIP_HEIGHT, 24.0, ".chip { height: 24px } (:192)"),
            (
                mockup::SEG_HEIGHT,
                28.0,
                ".seg { height: var(--row) } (:263), --row: 28px (:47)",
            ),
            (
                mockup::SEG_OPTION_WIDTH,
                34.0,
                ".seg.quality { --seg-w: 34px } (:264)",
            ),
            (
                mockup::ESTIMATE_HEIGHT,
                28.0,
                ".est-size { height: var(--row) } (:269)",
            ),
            (mockup::FILE_GAP, 8.0, ".file-row { gap: 8px } (:171)"),
            (mockup::PATH_GAP, 8.0, ".path-row { gap: 8px } (:188)"),
            (
                mockup::SHORTCUT_GAP,
                6.0,
                ".path-chips { gap: 6px } (:191)",
            ),
            (
                mockup::PRESET_INDENT,
                28.0,
                ".size-presets { padding-left: 28px } (:258)",
            ),
            (
                mockup::PRESET_GAP,
                6.0,
                ".size-presets { gap: 6px } (:258)",
            ),
            (
                mockup::PRESET_MARGIN,
                2.0,
                ".size-presets { margin-top: 2px } (:258)",
            ),
            (
                mockup::MODE_PADDING,
                2.0,
                ".size-mode { padding: 2px } (:225)",
            ),
            (
                mockup::MODE_BUTTON_HEIGHT,
                24.0,
                ".mode-btn { height: 24px } (:106)",
            ),
            (
                mockup::CELL_LABEL_WIDTH,
                20.0,
                ".cell-label { width: 20px } (:237)",
            ),
            (
                mockup::FORMAT_WIDTH,
                80.0,
                ".fmt-btn { width: var(--fmt-w) } (:173), --fmt-w: 80px (:49)",
            ),
            (
                mockup::GHOST_HEIGHT,
                32.0,
                ".btn-ghost { height: 32px } (:189)",
            ),
            (
                mockup::GHOST_ICON_WIDTH,
                32.0,
                ".btn-ghost.icon-only { width: 32px } (:205)",
            ),
            (
                mockup::BUTTON_HEIGHT,
                32.0,
                ".btn { height: var(--btn-h) } (:279), --btn-h: 32px (:50)",
            ),
            (
                mockup::HEADER_ACTION_GAP,
                8.0,
                ".row-header-actions { gap: 8px } (:224)",
            ),
            (
                mockup::QUALITY_LINE_GAP,
                12.0,
                ".quality-line { gap: 12px } (:306)",
            ),
        ];
        for (value, expected, source) in cases {
            assert_eq!(value, expected, "{source}");
        }
    }

    /// The six rows are the heights the mockup's parts add up to, written as numbers.
    ///
    /// The numbers are literals on purpose. Comparing a row against the same sum that produced it
    /// cannot fail, and a tautology is worse than no assertion because it reads like one that
    /// works: this file's own history has a test that compared a pixel against itself and hid a
    /// defect for a whole batch. Each literal carries the arithmetic that explains it instead.
    #[::core::prelude::v1::test]
    fn every_row_is_the_height_its_parts_add_up_to() {
        let rows = mockup::row_heights();
        assert_eq!(
            rows.map(|(label, _)| label),
            PROBE_LABELS,
            "the rows are the mockup's six, in its order"
        );
        let heights: [f32; 6] = rows.map(|(_, height)| height);
        assert_eq!(
            heights,
            [
                62.0, // 32 box + 6 gap + 24 chips
                32.0, // 32 box
                24.0, // 24 chips
                64.0, // 32 box + 6 gap + 2 margin + 24 chips
                28.0, // 28 line
                28.0, // 28 line
            ],
            "each row is the sum of the parts it holds"
        );
    }

    /// The column is taller than nothing and fits the window it is drawn in.
    ///
    /// The interesting half is the second one: a flow layout cannot overlap its rows, but the whole
    /// column can be too tall for the window, and that has no overlap to give it away. This is also
    /// the assertion a wrong gap or a wrong control height shows up in, because the sum moves.
    #[::core::prelude::v1::test]
    fn the_column_is_tall_enough_to_hold_its_rows_and_fits_the_window() {
        let column = mockup::column_height();
        let rows: f32 = mockup::row_heights().iter().map(|(_, h)| *h).sum();
        assert!(
            column > rows,
            "the column is more than its rows: the headers and gaps are counted too ({column} vs {rows})"
        );
        let used = column + 2.0 * PROBE_PADDING;
        assert!(
            used <= PROBE_HEIGHT,
            "the column plus the window's padding is {used}, which has to fit {PROBE_HEIGHT}"
        );
    }

    /// The column's width is the panel's metric, so the two renderers compare like with like.
    #[::core::prelude::v1::test]
    fn the_column_is_as_wide_as_the_panels() {
        assert_eq!(
            column_width(),
            320.0,
            "the panel's form column is 920 - 32 padding - 556 preview - 12 gap"
        );
        let used = column_width() + 2.0 * PROBE_PADDING;
        assert!(
            used <= PROBE_WIDTH,
            "the column plus padding is {used}, inside a {PROBE_WIDTH} window"
        );
    }

    /// A shortcut points at the folder it names, which is the "works but does the wrong thing" bug.
    ///
    /// The mapping is a free function precisely so this can be asked without a window: the three
    /// shortcuts are the only place in this view where a wrong number would still look right.
    #[::core::prelude::v1::test]
    fn each_shortcut_names_its_own_folder() {
        let desktop = crate::save::dirs::desktop();
        let pictures = crate::save::dirs::pictures();
        let downloads = crate::save::dirs::downloads();
        // The three can legitimately coincide on a machine whose known folders are redirected, so
        // the assertion is against the functions rather than against each other.
        assert_eq!(shortcut_directory(0), desktop, "桌面 is the desktop");
        assert_eq!(shortcut_directory(1), pictures, "图片 is pictures");
        assert_eq!(shortcut_directory(2), downloads, "下载 is downloads");
    }

    /// The menus offer the mockup's words, in its order, and the actions follow them one by one.
    #[::core::prelude::v1::test]
    fn the_menus_offer_the_mockups_words_in_order() {
        assert_eq!(
            VARIABLE_TOKENS,
            ["[日期]", "[时间]", "[序号]", "[分辨率]", "[软件名]"],
            "the five variables are the specification's own words (`:192`-`:198`)"
        );
        assert_eq!(FMT_OPTIONS, [".PNG", ".JPG", ".WebP"], "` :203`");
        assert_eq!(PROBE_SHORTCUTS, ["桌面", "图片", "下载"], "`:739`-`:741`");
        assert_eq!(PROBE_AFTER, ["复制路径", "打开文件夹"], "`:573`/`:574`");
        assert_eq!(SIZE_PRESET_WORDS, ["100%", "75%", "50%", "25%"], "`:614`");
        assert_eq!(QUALITY_OPTION_WORDS, ["高", "中", "低"], "`:641`-`:643`");
        assert_eq!(SIZE_MODE_WORDS, ["数值", "百分比"], "`:226`/`:227`");
        assert_eq!(PROBE_SIZE_LABELS, ["宽", "高"], "`.cell-label` (`:237`)");
        assert_eq!(PROBE_UNIT, "px", "`.dim-unit` (`:243`)");
        assert_eq!(PROBE_LIMIT, "限制体积", "`.switch-text` (`:634`)");
    }
}
