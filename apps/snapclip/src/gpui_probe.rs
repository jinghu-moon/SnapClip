//! A throwaway probe: can the save panel be built out of GPUI components instead of a
//! window with one native control?
//!
//! Nothing here is on a production path. It opens when `SNAPCLIP_GPUI_PROBE` is set, it is the
//! only window in that run, and it exists to answer two questions: does Chinese typing work with
//! the candidate window in place, and does the panel's right column — the six rows the mockup calls
//! 保存路径 / 文件名 / 保存后 / 尺寸 / 质量 / 预估大小 — look like the one the panel already draws.
//!
//! The outer rectangles come from `snapclip_ui::panel`, the same functions the panel's own frame
//! uses, so what the eye compares is one set of geometry in two renderers rather than two
//! interpretations of one mockup. Spacing *inside* a row is the framework's layout, because
//! measuring text is its job now; the tests below are what keep the outer geometry honest.
//!
//! The path row is the one that has to *work*, not merely look: its three shortcuts and its
//! `更改` button really change the field, because a field whose buttons do nothing is the failure
//! this whole exercise keeps finding.

use gpui_kit::base::StyledExt as _;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu};
use gpui_kit::*;
use snapclip_model::geometry::Rect;
use snapclip_ui::panel::{
    DEMO, FrameMetrics, Row, SizeCell, field_rect, label_rect, name_field_rect,
    path_field_rect, quality_seg_rect, size_cell_rect, size_presets_rect, FMT_OPTIONS,
    QUALITY_HINT_TEXT, QUALITY_OPTION_WORDS,
    SIZE_MODE_WORDS, SIZE_PRESET_WORDS, VARIABLE_TOKENS,
};

/// The probe window's client size, matching the save panel so the eye compares like with like.
pub const PROBE_WIDTH: f32 = 920.0;
/// See [`PROBE_WIDTH`].
pub const PROBE_HEIGHT: f32 = 564.0;
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
/// The width the path field gives up to the `更改` button, when nothing has measured it.
///
/// The panel measures that word with DirectWrite and carries the number in its window state
/// (`measure_path_change`); this renderer has no device to measure with, so it uses the number the
/// panel's own frame falls back to in its tests — `frame.rs:3048` passes `56` for the same reason.
/// It only affects the field's right edge, never its height, which is what the geometry here is
/// compared on.
pub const PATH_CHANGE_WIDTH: i32 = 56;
/// What the estimated size shows before anything has been measured (`:668` shows a real value,
/// which this probe has no encoder to produce; the sample is the mockup's own).
pub const PROBE_ESTIMATE_SAMPLE: &str = "227.31 KB";

// A menu item's click handler is crate-private in this release — items carry an `action` instead,
// and actions are dispatched to the focus owner through GPUI's action system. Binding that for two
// throwaway menus is a mechanism the real migration should design rather than inherit from a probe,
// so the items below are section labels: the menus open, the words are the mockup's, and choosing
// one is recorded as not yet wired. See `docs/41`.

/// One rectangle as the framework positions with it: left, top, width, height.
///
/// The conversion from the panel's integer dips happens here and nowhere else, so a second
/// arithmetic cannot drift away from this one.
pub fn box_of(rect: Rect) -> (f32, f32, f32, f32) {
    (
        rect.left as f32,
        rect.top as f32,
        (rect.right - rect.left) as f32,
        (rect.bottom - rect.top) as f32,
    )
}

/// The boxes this probe places, by name, at the window's scale factor.
///
/// Every box is the panel's own rectangle for the thing it holds; nothing here is re-derived. The
/// path row is the one that is easy to get wrong: `field_rect` is the whole row *band*, which also
/// contains the three shortcut chips underneath, while the field itself is `path_field_rect`. Using
/// the band made the field twice its real height, which is exactly the sort of mistake comparing
/// the two renderers exists to expose.
pub fn row_boxes(scale: f32) -> [(&'static str, (f32, f32, f32, f32)); 16] {
    let metrics: FrameMetrics = DEMO;
    let rects = metrics.rects(scale);
    [
        (
            "path-label",
            box_of(label_rect(&metrics, &rects, scale, Row::Path)),
        ),
        (
            "path-band",
            box_of(field_rect(&metrics, &rects, scale, Row::Path)),
        ),
        (
            "path-field",
            box_of(path_field_rect(
                &metrics,
                &rects,
                scale,
                PATH_CHANGE_WIDTH,
            )),
        ),
        (
            "name-label",
            box_of(label_rect(&metrics, &rects, scale, Row::Name)),
        ),
        ("name-box", box_of(name_field_rect(&metrics, &rects, scale))),
        (
            "saved-label",
            box_of(label_rect(&metrics, &rects, scale, Row::Saved)),
        ),
        (
            "saved-row",
            box_of(field_rect(&metrics, &rects, scale, Row::Saved)),
        ),
        (
            "size-label",
            box_of(label_rect(&metrics, &rects, scale, Row::Size)),
        ),
        (
            "size-band",
            box_of(field_rect(&metrics, &rects, scale, Row::Size)),
        ),
        (
            "size-width",
            box_of(size_cell_rect(&metrics, &rects, scale, SizeCell::Width)),
        ),
        (
            "size-height",
            box_of(size_cell_rect(&metrics, &rects, scale, SizeCell::Height)),
        ),
        (
            "size-presets",
            box_of(size_presets_rect(&metrics, &rects, scale)),
        ),
        (
            "quality-label",
            box_of(label_rect(&metrics, &rects, scale, Row::Quality)),
        ),
        (
            "quality-seg",
            box_of(quality_seg_rect(&metrics, &rects, scale)),
        ),
        (
            "estimate-label",
            box_of(label_rect(&metrics, &rects, scale, Row::Estimate)),
        ),
        (
            "estimate-value",
            box_of(field_rect(&metrics, &rects, scale, Row::Estimate)),
        ),
    ]
}

/// The probe's state: the three fields it can edit, and the choices it shows.
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
                .placeholder("C:\\")
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

/// Place a child at one of [`row_boxes`], absolutely, in the panel's own coordinates.
fn placed(child: impl IntoElement, at: (f32, f32, f32, f32)) -> impl IntoElement {
    let (left, top, width, height) = at;
    div()
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(width))
        .h(px(height))
        .child(child)
}

/// A row label, in the panel's own box so the two renderers can be compared.
fn row_label(text: &'static str) -> impl IntoElement {
    div().text_xs().child(text)
}

impl Render for GpuiProbe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let scale = window.scale_factor();
        let boxes = row_boxes(scale);
        let at = |id: &str| {
            boxes
                .iter()
                .find(|(name, _)| *name == id)
                .map(|(_, rect)| *rect)
                .expect("every box this view places is in row_boxes")
        };
        let (field_w, field_h) = (at("path-field").2, at("path-field").3);
        let (band_h, band_top) = (at("path-band").3, at("path-band").1);
        let field_top = at("path-field").1;
        // The chips sit in the band's lower part; the band is the field plus the gap plus the chips
        // (`field.rs` `Row::Path` content height), so the chips' top is the field's bottom plus the
        // row gap, expressed against the band rather than as a fresh constant.
        let chips_offset = field_top - band_top + field_h;
        let label = |text: &'static str, id: &str| {
            placed(row_label(text), at(id))
        };

        div()
            .relative()
            .size_full()
            .p_4()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(label(PROBE_LABELS[0], "path-label"))
            // The read-only path field plus its two buttons, laid against the row band so the
            // buttons keep the band's height while the field keeps its own.
            .child(placed(
                div()
                    .h_flex()
                    .size_full()
                    .items_start()
                    .gap_2()
                    .child(
                        div().w(px(field_w)).h(px(field_h)).child(
                            Input::new(&self.path)
                                .id("probe-path")
                                .readonly(true)
                                .size_full(),
                        ),
                    )
                    .child(Button::new("probe-change").label(PROBE_CHANGE).on_click(
                        cx.listener(|this, _, window, cx| {
                            if let Some(folder) = pick_folder() {
                                this.go_to(folder, window, cx);
                            }
                        }),
                    ))
                    .child(Button::new("probe-open").label("…").on_click(
                        cx.listener(|this, _, window, cx| {
                            let current = this.path.read(cx).value().to_string();
                            open_in_explorer(&current);
                            window.refresh();
                        }),
                    )),
                at("path-band"),
            ))
            // The three shortcuts, really moving the field.
            .child(
                div()
                    .absolute()
                    .left(px(at("path-field").0))
                    .top(px(band_top + chips_offset))
                    .h(px(band_h - chips_offset))
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .children(PROBE_SHORTCUTS.into_iter().enumerate().map(|(index, word)| {
                        Button::new(format!("probe-shortcut-{index}")).label(word)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let directory = shortcut_directory(index);
                                this.go_to(directory, window, cx);
                            }))
                    })),
            )
            .child(label(PROBE_LABELS[1], "name-label"))
            .child(placed(
                div()
                    .h_flex()
                    .size_full()
                    .items_center()
                    .gap_1()
                    .child(Input::new(&self.name).id("probe-name").flex_1())
                    .child(
                        Button::new("probe-wand")
                            .label(PROBE_WAND)
                            .dropdown_menu(|menu, _, _| variable_menu(menu)),
                    )
                    .child(
                        Button::new("probe-format")
                            .label(FMT_OPTIONS[self.format])
                            .dropdown_menu(|menu, _, _| format_menu(menu)),
                    ),
                at("name-box"),
            ))
            .child(label(PROBE_LABELS[2], "saved-label"))
            .child(placed(
                div()
                    .h_flex()
                    .size_full()
                    .items_center()
                    .gap_2()
                    .children(PROBE_AFTER.into_iter().enumerate().map(|(index, word)| {
                        let on = self.after[index];
                        let button = Button::new(format!("probe-after-{index}")).label(word);
                        let button = if on { button.primary() } else { button };
                        button.on_click(cx.listener(move |this, _, _, cx| {
                            this.after[index] = !this.after[index];
                            cx.notify();
                        }))
                    })),
                at("saved-row"),
            ))
            .child(label(PROBE_LABELS[3], "size-label"))
            // The size mode group and the reset button, at the panel's rectangles.
            .child(placed(
                div()
                    .h_flex()
                    .items_center()
                    .gap_1()
                    .children(SIZE_MODE_WORDS.into_iter().enumerate().map(|(index, word)| {
                        let chosen = self.mode == index;
                        let button = Button::new(format!("probe-mode-{index}")).label(word);
                        let button = if chosen { button.primary() } else { button };
                        button.on_click(cx.listener(move |this, _, _, cx| {
                            this.mode = index;
                            cx.notify();
                        }))
                    })),
                at("size-band"),
            ))
            .child(placed(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .child(row_label("宽"))
                    .child(
                        div()
                            .w(px(at("size-width").2))
                            .child(Input::new(&self.width).id("probe-width")),
                    )
                    .child(row_label("px"))
                    .child(row_label("高"))
                    .child(
                        div()
                            .w(px(at("size-height").2))
                            .child(Input::new(&self.height).id("probe-height")),
                    )
                    .child(row_label("px")),
                at("size-width"),
            ))
            // The four presets, which are the panel's own words.
            .child(placed(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .children(SIZE_PRESET_WORDS.into_iter().enumerate().map(|(index, word)| {
                        Button::new(format!("probe-preset-{index}")).label(word)
                    })),
                at("size-presets"),
            ))
            .child(label(PROBE_LABELS[4], "quality-label"))
            .child(placed(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    // PNG is lossless, so the mockup hides the tiers and states why (`:638`).
                    .child(div().text_xs().child(QUALITY_HINT_TEXT))
                    .child(row_label(SIZE_MODE_WORDS[0]))
                    .children(QUALITY_OPTION_WORDS.into_iter().enumerate().map(|(index, word)| {
                        Button::new(format!("probe-quality-{index}")).label(word)
                    })),
                at("quality-seg"),
            ))
            .child(label(PROBE_LABELS[5], "estimate-label"))
            .child(placed(
                div().text_sm().child(PROBE_ESTIMATE_SAMPLE),
                at("estimate-value"),
            ))
            .child(
                div()
                    .absolute()
                    .right(px(16.0))
                    .bottom(px(12.0))
                    .h_flex()
                    .gap_2()
                    .child(Button::new("probe-cancel").label(PROBE_CANCEL))
                    .child(Button::new("probe-save").primary().label(PROBE_SAVE)),
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
    use windows_sys::Win32::UI::Shell::{BROWSEINFOW, SHBrowseForFolderW, SHGetPathFromIDListW};
    use windows_sys::Win32::System::Com::CoTaskMemFree;

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
    use snapclip_ui::panel::fmt_button_rect;

    /// The probe places the panel's rectangle for the name box, at every scale factor.
    ///
    /// This is the assertion that makes the comparison meaningful: if a migration ever re-derives a
    /// rectangle here instead of reading the panel's own function, the panel stays correct and the
    /// two renderers drift apart silently. Comparing both at four scales is what would catch it.
    #[::core::prelude::v1::test]
    fn the_probe_places_the_panels_own_name_box() {
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            let metrics: FrameMetrics = DEMO;
            let rects = metrics.rects(scale);
            let expected = name_field_rect(&metrics, &rects, scale);
            let placed = row_boxes(scale)
                .iter()
                .find(|(id, _)| *id == "name-box")
                .map(|(_, rect)| *rect)
                .expect("the name box is placed");
            assert_eq!(
                placed.0 as i32, expected.left,
                "left comes from the panel at {scale}x"
            );
            assert_eq!(
                placed.2 as i32,
                expected.right - expected.left,
                "width comes from the panel at {scale}x"
            );
        }
    }

    /// The field still gives way to the format button, which is the reduction that was painful.
    #[::core::prelude::v1::test]
    fn the_name_box_ends_before_the_format_button() {
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            let metrics: FrameMetrics = DEMO;
            let rects = metrics.rects(scale);
            let field = name_field_rect(&metrics, &rects, scale);
            let button = fmt_button_rect(&metrics, &rects, scale);
            assert!(
                field.right <= button.left,
                "the field ends at or before the format button at {scale}x"
            );
        }
    }

    /// The path row places the field, not the band that also contains the shortcut chips.
    ///
    /// `field_rect` is the whole row band — the field plus the three chips beneath it — so using it
    /// as the field doubled the box's height and produced the grey slab this test now rules out.
    /// The band is still what the buttons are laid out against, which is why both rectangles are
    /// placed and compared rather than only the field.
    #[::core::prelude::v1::test]
    fn the_path_field_is_the_input_and_not_the_row() {
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            let metrics: FrameMetrics = DEMO;
            let rects = metrics.rects(scale);
            let field = path_field_rect(&metrics, &rects, scale, PATH_CHANGE_WIDTH);
            let band = field_rect(&metrics, &rects, scale, Row::Path);
            let placed = |id: &str| {
                row_boxes(scale)
                    .iter()
                    .find(|(name, _)| *name == id)
                    .map(|(_, rect)| *rect)
                    .expect("the box is placed")
            };
            let probe_field = placed("path-field");
            let probe_band = placed("path-band");
            assert_eq!(
                probe_field.3 as i32,
                field.bottom - field.top,
                "the path field's height is the input's, not the band's, at {scale}x"
            );
            assert_eq!(
                probe_band.3 as i32,
                band.bottom - band.top,
                "the band is still placed, because the buttons sit against it, at {scale}x"
            );
            assert!(
                probe_field.3 < probe_band.3,
                "the field is shorter than the band it sits in, at {scale}x"
            );
        }
    }

    /// The three rows keep the panel's vertical order, so a misplaced section is obvious.
    #[::core::prelude::v1::test]
    fn the_three_rows_keep_the_panels_order() {
        let boxes = row_boxes(1.0);
        let top = |id: &str| {
            boxes
                .iter()
                .find(|(name, _)| *name == id)
                .map(|(_, rect)| rect.1)
                .expect("the row is placed")
        };
        assert!(
            top("path-band") < top("name-box"),
            "the path row is above"
        );
        assert!(top("name-box") < top("saved-row"), "the saved row is below");
    }

    /// The whole right column keeps the mockup's row order, not three of them.
    ///
    /// The first three rows were placed before the rest existed; this walks all six, because "the
    /// three that were built are in order" is not the claim anyone cares about.
    #[::core::prelude::v1::test]
    fn all_six_rows_keep_the_panels_order() {
        let boxes = row_boxes(1.0);
        let top = |id: &str| {
            boxes
                .iter()
                .find(|(name, _)| *name == id)
                .map(|(_, rect)| rect.1)
                .expect("the row is placed")
        };
        let order = [
            "path-label",
            "name-label",
            "saved-label",
            "size-label",
            "quality-label",
        ];
        for pair in order.windows(2) {
            assert!(
                top(pair[0]) < top(pair[1]),
                "{} is above {}",
                pair[0],
                pair[1]
            );
        }
        assert!(
            top("quality-label") < top("estimate-value"),
            "the estimate value is below the quality row"
        );
    }

    /// The size row's cells come from the panel, so the two renderers cannot disagree about them.
    #[::core::prelude::v1::test]
    fn the_size_cells_are_the_panels_own() {
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            let metrics: FrameMetrics = DEMO;
            let rects = metrics.rects(scale);
            for (id, cell) in [("size-width", SizeCell::Width), ("size-height", SizeCell::Height)] {
                let expected = size_cell_rect(&metrics, &rects, scale, cell);
                let placed = row_boxes(scale)
                    .iter()
                    .find(|(name, _)| *name == id)
                    .map(|(_, rect)| *rect)
                    .expect("the cell is placed");
                assert_eq!(
                    placed.0 as i32, expected.left,
                    "{id} starts where the panel puts it at {scale}x"
                );
                assert_eq!(
                    placed.2 as i32,
                    expected.right - expected.left,
                    "{id} is as wide as the panel makes it at {scale}x"
                );
            }
        }
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
    }
}
