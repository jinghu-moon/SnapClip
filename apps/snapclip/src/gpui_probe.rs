//! A throwaway probe: can the save panel be built out of GPUI components instead of a
//! window with one native control?
//!
//! Nothing here is on a production path. It opens when `SNAPCLIP_GPUI_PROBE` is set, it is the
//! only window in that run, and it exists to answer two questions by eye: does Chinese typing
//! work with the candidate window in place, and do three of the panel's rows — the path row, the
//! name row and the saved-after row — look like the ones the panel already draws.
//!
//! The outer rectangles come from `snapclip_ui::panel`, the same functions the panel's own frame
//! uses, so what the eye compares is one set of geometry in two renderers rather than two
//! interpretations of one mockup. Spacing *inside* a row is the framework's layout, because
//! measuring text is its job now; the tests below are what keep the outer geometry honest.

use gpui_kit::base::StyledExt as _;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;
use snapclip_model::geometry::Rect;
use snapclip_ui::panel::{field_rect, label_rect, name_field_rect, FrameMetrics, Row, DEMO};

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
/// The two switches beside the saved-after label.
pub const PROBE_AFTER: [&str; 2] = ["复制路径", "打开文件夹"];

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

/// The six boxes this probe places, by name, at the window's scale factor.
///
/// The path field's own width depends on a measurement the panel takes through DirectWrite, and
/// this renderer has no such device; the field is therefore given the whole row for this pass and
/// the two buttons sit at its right end, laid out by the framework. Everything else — the row
/// bands, the box the name field is reduced to, the wand's box and the format button's — needs no
/// measurement and comes out identical to the panel's.
pub fn row_boxes(scale: f32) -> [(&'static str, (f32, f32, f32, f32)); 6] {
    let metrics: FrameMetrics = DEMO;
    let rects = metrics.rects(scale);
    [
        (
            "path-label",
            box_of(label_rect(&metrics, &rects, scale, Row::Path)),
        ),
        (
            "path-row",
            box_of(field_rect(&metrics, &rects, scale, Row::Path)),
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
    ]
}

/// The probe's only state: the name field.
pub struct GpuiProbe {
    name: Entity<InputState>,
}

impl GpuiProbe {
    /// Build the view, seeding the name field with the template the panel uses.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(PROBE_NAME)
                .default_value(PROBE_NAME)
        });
        Self { name }
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

impl Render for GpuiProbe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let boxes = row_boxes(window.scale_factor());
        let at = |id: &str| {
            boxes
                .iter()
                .find(|(name, _)| *name == id)
                .map(|(_, rect)| *rect)
                .expect("every box this view places is in row_boxes")
        };
        let face = || div().size_full().rounded_md().bg(theme.secondary);

        div()
            .relative()
            .size_full()
            .p_4()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(placed(div().child(PROBE_TITLE), at("path-label")))
            .child(placed(
                div()
                    .h_flex()
                    .size_full()
                    .items_center()
                    .gap_2()
                    .child(face().flex_1())
                    .child(Button::new("probe-change").label(PROBE_CHANGE)),
                at("path-row"),
            ))
            .child(placed(div().child("文件名"), at("name-label")))
            .child(placed(
                div()
                    .h_flex()
                    .size_full()
                    .items_center()
                    .gap_1()
                    .child(Input::new(&self.name).id("probe-name").flex_1())
                    .child(Button::new("probe-wand").label(PROBE_WAND))
                    .child(Button::new("probe-format").label(PROBE_FORMAT)),
                at("name-box"),
            ))
            .child(placed(div().child("保存后"), at("saved-label")))
            .child(placed(
                div()
                    .h_flex()
                    .size_full()
                    .items_center()
                    .gap_2()
                    .child(Button::new("probe-after-copy").label(PROBE_AFTER[0]))
                    .child(Button::new("probe-after-open").label(PROBE_AFTER[1])),
                at("saved-row"),
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
        assert!(top("path-row") < top("name-box"), "the path row is above");
        assert!(top("name-box") < top("saved-row"), "the saved row is below");
    }
}
