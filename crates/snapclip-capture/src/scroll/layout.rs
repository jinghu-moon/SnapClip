//! Where the scroll session's panel goes, in physical pixels of the overlay, and what it says.
//!
//! The panel (`docs/design/overlay-ui-demo.html`, `drawScrolling`) is [`PANEL_WIDTH_DIP`] wide,
//! [`GAP_DIP`] to the right of the selection (to the left when the right side has no room) and
//! bottom-aligned with it. Top to bottom: a header (the title and the image's size), the thumbnail
//! well showing the image's tail, a status line, and one row of buttons that are also the key
//! hints. It grows with the thumbnail, up to the selection's height (at least
//! [`MIN_HEIGHT_DIP`]).
//!
//! Pure arithmetic so the painter, the hit test and the window region agree by construction.

use crate::geometry::Rect;

pub(crate) const PANEL_WIDTH_DIP: i32 = 160;
pub(crate) const GAP_DIP: i32 = 8;
/// The tallest the panel gets when the selection is shorter than this.
pub(crate) const MIN_HEIGHT_DIP: i32 = 120;
pub(crate) const HEADER_DIP: i32 = 26;
pub(crate) const STATUS_DIP: i32 = 20;
pub(crate) const BUTTON_ROW_DIP: i32 = 22;
/// The thumbnail well's and the buttons' distance from the panel's sides and bottom.
pub(crate) const INSET_DIP: i32 = 6;
const BUTTON_SPACING_DIP: i32 = 4;
/// The panel keeps this far from the work area's edges.
const EDGE_DIP: i32 = 4;
/// How far the panel's shadow reaches past it. Less than [`GAP_DIP`], so the shadow never reaches
/// the selection either.
pub(crate) const SHADOW_DIP: i32 = 6;

/// The panel's buttons, left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Button {
    /// Finish and export.
    Finish,
    /// Toggles automatic scrolling.
    Auto,
    /// Cancel without output.
    Cancel,
}

pub(crate) const BUTTONS: [Button; 3] = [Button::Finish, Button::Auto, Button::Cancel];

impl Button {
    pub(crate) fn action(self) -> crate::keymap::Action {
        use crate::keymap::Action;
        match self {
            Button::Finish => Action::ScrollFinish,
            Button::Auto => Action::ScrollAuto,
            Button::Cancel => Action::ScrollCancel,
        }
    }

    pub(crate) fn label(self, auto: bool) -> &'static str {
        match (self, auto) {
            (Button::Finish, _) => "完成",
            (Button::Auto, false) => "自动",
            (Button::Auto, true) => "停止",
            (Button::Cancel, _) => "取消",
        }
    }
}

/// The panel's title.
pub(crate) const TITLE: &str = "长截图";

/// The header's right side: the image's size so far, `width×rows`.
pub(crate) fn size_text(width: i32, image_rows: u32) -> String {
    format!("{width}×{image_rows}")
}

/// The status line. `status` is `None` until the first frame arrived.
pub(crate) fn status_text(
    status: Option<super::Status>,
    auto: super::Auto,
    failed: bool,
) -> &'static str {
    use super::{Auto, Status};
    match (failed, status, auto) {
        (true, ..) => "截图失败，见日志",
        (false, None, _) => "准备中…",
        (false, Some(Status::Full), _) => "已达上限",
        (false, Some(Status::Lost), _) => "跟丢了，请往回滚",
        (false, Some(Status::Tracking), Auto::On) => "自动滚动中",
        (false, Some(Status::Tracking), Auto::Refused) => "窗口拒绝自动，请手动滚",
        (false, Some(Status::Tracking), Auto::Off) => "滚动页面",
    }
}

/// Every string the panel draws apart from key names and digits, for the font subset
/// (`win/d2d/tests.rs`).
#[cfg(test)]
pub(crate) fn drawn_strings() -> Vec<String> {
    use super::{Auto, Status};
    let mut drawn: Vec<String> = BUTTONS
        .iter()
        .flat_map(|button| [button.label(false), button.label(true)])
        .chain([TITLE])
        .map(str::to_owned)
        .collect();
    drawn.push(size_text(1234, 56789));
    drawn.push(status_text(None, Auto::Off, false).to_owned());
    drawn.push(status_text(None, Auto::Off, true).to_owned());
    for status in [Status::Tracking, Status::Lost, Status::Full] {
        for auto in [Auto::Off, Auto::On, Auto::Refused] {
            drawn.push(status_text(Some(status), auto, false).to_owned());
        }
    }
    drawn
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    pub panel: Rect,
    /// The title row; the title and the size are drawn inside its horizontal padding.
    pub header: Rect,
    /// The thumbnail well: the thumbnail is clipped to it, bottom-aligned.
    pub well: Rect,
    pub status: Rect,
    pub buttons: [Rect; 3],
    /// The panel and its shadow: what the window region has to keep.
    pub footprint: Rect,
}

impl Layout {
    pub(crate) fn button_at(&self, x: i32, y: i32) -> Option<Button> {
        let point = crate::geometry::Point::new(x, y);
        BUTTONS
            .iter()
            .zip(self.buttons)
            .find(|(_, rect)| rect.contains(point))
            .map(|(button, _)| *button)
    }
}

/// A DIP length in physical pixels.
pub(crate) fn px(dip: i32, dpi: u32) -> i32 {
    (dip as f32 * dpi.max(96) as f32 / 96.0).round() as i32
}

/// The thumbnail's width in physical pixels: the well's.
pub(crate) fn thumb_width(dpi: u32) -> i32 {
    px(PANEL_WIDTH_DIP, dpi) - 2 * px(INSET_DIP, dpi)
}

/// Everything in the panel that is not the well.
fn chrome_height(dpi: u32) -> i32 {
    px(HEADER_DIP, dpi) + px(STATUS_DIP, dpi) + px(BUTTON_ROW_DIP, dpi) + px(INSET_DIP, dpi)
}

/// The tallest the panel gets for `selection` on `work`.
fn max_height(selection: Rect, work: Rect, dpi: u32) -> i32 {
    selection
        .height()
        .max(px(MIN_HEIGHT_DIP, dpi))
        .min(work.height() - 2 * px(EDGE_DIP, dpi))
        .max(chrome_height(dpi) + 1)
}

/// The most thumbnail rows the well can show: what the capture thread has to publish.
pub(crate) fn well_capacity(selection: Rect, work: Rect, dpi: u32) -> i32 {
    max_height(selection, work, dpi) - chrome_height(dpi)
}

/// `thumb_rows` is the thumbnail's height in pixels; the well shows at most
/// [`well_capacity`] of it. `work` is the monitor's work area in overlay coordinates.
pub(crate) fn layout(selection: Rect, work: Rect, thumb_rows: i32, dpi: u32) -> Layout {
    let gap = px(GAP_DIP, dpi);
    let edge = px(EDGE_DIP, dpi);
    let inset = px(INSET_DIP, dpi);
    let width = px(PANEL_WIDTH_DIP, dpi);
    let chrome = chrome_height(dpi);
    let height = (thumb_rows.max(0) + chrome).min(max_height(selection, work, dpi));

    let right_side = selection.right + gap;
    let left = if right_side + width <= work.right - edge {
        right_side
    } else {
        (selection.left - gap - width).max(work.left)
    };
    let top = (selection.bottom - height)
        .min(work.bottom - edge - height)
        .max(work.top + edge);
    let panel = Rect::new(left, top, left + width, top + height);

    let header = Rect::new(left, top, panel.right, top + px(HEADER_DIP, dpi));
    let buttons_bottom = panel.bottom - inset;
    let buttons_top = buttons_bottom - px(BUTTON_ROW_DIP, dpi);
    let status = Rect::new(
        left + inset,
        buttons_top - px(STATUS_DIP, dpi),
        panel.right - inset,
        buttons_top,
    );
    let well = Rect::new(left + inset, header.bottom, panel.right - inset, status.top);

    let spacing = px(BUTTON_SPACING_DIP, dpi);
    let slot = (well.width() - 2 * spacing) / 3;
    let mut buttons = [Rect::default(); 3];
    let mut x = well.left;
    for (index, rect) in buttons.iter_mut().enumerate() {
        // The last button takes what the division left over, so the row ends on the well's edge.
        let right = if index == 2 { well.right } else { x + slot };
        *rect = Rect::new(x, buttons_top, right, buttons_bottom);
        x = right + spacing;
    }

    let footprint = panel.inflate(px(SHADOW_DIP, dpi));
    Layout {
        panel,
        header,
        well,
        status,
        buttons,
        footprint,
    }
}

#[cfg(test)]
mod tests {
    use super::{Button, layout, px, status_text, thumb_width, well_capacity};
    use crate::geometry::Rect;
    use crate::scroll::{Auto, Status};

    const WORK: Rect = Rect::new(0, 0, 1920, 1040);
    /// Header 26 + status 20 + buttons 22 + inset 6 at 96 DPI.
    const CHROME: i32 = 74;

    #[test]
    fn the_panel_is_right_of_the_selection_and_bottom_aligned() {
        let selection = Rect::new(400, 200, 1200, 800);
        let placed = layout(selection, WORK, 300, 96);
        assert_eq!(
            placed.panel,
            Rect::new(1208, 800 - 300 - CHROME, 1208 + 160, 800)
        );
        assert_eq!(
            placed.well,
            Rect::new(1214, placed.panel.top + 26, 1362, 800 - 48)
        );
        assert_eq!(placed.well.height(), 300);
    }

    #[test]
    fn the_panel_grows_with_the_thumbnail_up_to_the_selection_height() {
        let selection = Rect::new(400, 200, 1200, 800);
        assert_eq!(layout(selection, WORK, 0, 96).panel.height(), CHROME);
        assert_eq!(layout(selection, WORK, 50, 96).panel.height(), 50 + CHROME);
        assert_eq!(layout(selection, WORK, 5000, 96).panel.height(), 600);
        assert_eq!(layout(selection, WORK, 5000, 96).panel.top, 200);
        assert_eq!(well_capacity(selection, WORK, 96), 600 - CHROME);
        assert_eq!(
            layout(selection, WORK, 5000, 96).well.height(),
            well_capacity(selection, WORK, 96)
        );
    }

    /// A short selection still gets a usable panel; it rises above the selection's top.
    #[test]
    fn a_short_selection_gets_the_minimum_height() {
        let selection = Rect::new(400, 500, 1200, 540);
        let placed = layout(selection, WORK, 5000, 96);
        assert_eq!(placed.panel.height(), 120);
        assert_eq!(placed.panel.bottom, 540);
    }

    /// Near the top of the work area the panel is pushed down rather than cut off.
    #[test]
    fn the_panel_stays_inside_the_work_area() {
        let selection = Rect::new(400, 0, 1200, 60);
        let placed = layout(selection, WORK, 5000, 96);
        assert_eq!(placed.panel.top, 4);
        assert_eq!(placed.panel.height(), 120);
        let whole = layout(Rect::new(0, 0, 1700, 1040), WORK, 5000, 96);
        assert!(
            whole.panel.top >= 4 && whole.panel.bottom <= 1036,
            "{whole:?}"
        );
    }

    #[test]
    fn the_panel_flips_left_when_the_right_side_has_no_room() {
        let selection = Rect::new(400, 200, 1800, 800);
        let placed = layout(selection, WORK, 300, 96);
        assert_eq!(placed.panel.right, 392);
        assert_eq!(placed.panel.width(), 160);
    }

    #[test]
    fn the_panel_scales_with_dpi() {
        assert_eq!(thumb_width(96), 148);
        assert_eq!(thumb_width(144), 222);
        let placed = layout(Rect::new(100, 100, 900, 900), WORK, 400, 144);
        assert_eq!(placed.panel.left, 900 + px(8, 144));
        assert_eq!(placed.panel.width(), 240);
        assert_eq!(placed.well.width(), thumb_width(144));
    }

    /// The buttons share the well's width, in order, and the hit test finds each one.
    #[test]
    fn the_buttons_span_the_bottom_row_in_order() {
        let selection = Rect::new(400, 200, 1200, 800);
        let placed = layout(selection, WORK, 300, 96);
        assert_eq!(placed.buttons[0].left, placed.well.left);
        assert_eq!(placed.buttons[2].right, placed.well.right);
        assert_eq!(placed.buttons[0].bottom, placed.panel.bottom - 6);
        assert!(placed.status.bottom <= placed.buttons[0].top);
        assert!(placed.well.bottom <= placed.status.top);
        for pair in placed.buttons.windows(2) {
            assert_eq!(pair[1].left - pair[0].right, 4);
        }
        let centre = placed.buttons[1].center();
        assert_eq!(placed.button_at(centre.x, centre.y), Some(Button::Auto));
        let centre = placed.buttons[2].center();
        assert_eq!(placed.button_at(centre.x, centre.y), Some(Button::Cancel));
        assert_eq!(placed.button_at(0, 0), None);
    }

    /// The shadow stays clear of the selection: it reaches less far than the gap.
    #[test]
    fn the_footprint_is_the_panel_and_its_shadow() {
        let selection = Rect::new(400, 200, 1200, 800);
        for dpi in [96, 144, 192] {
            let placed = layout(selection, WORK, 300, dpi);
            assert!(placed.footprint.contains_rect(placed.panel));
            assert!(placed.footprint.intersect(selection).is_empty(), "{dpi}");
        }
    }

    #[test]
    fn the_status_line_says_what_the_session_is_doing() {
        assert_eq!(status_text(None, Auto::Off, false), "准备中…");
        assert_eq!(
            status_text(Some(Status::Tracking), Auto::Off, false),
            "滚动页面"
        );
        assert_eq!(
            status_text(Some(Status::Tracking), Auto::On, false),
            "自动滚动中"
        );
        assert_eq!(
            status_text(Some(Status::Tracking), Auto::Refused, false),
            "窗口拒绝自动，请手动滚"
        );
        assert_eq!(
            status_text(Some(Status::Lost), Auto::On, false),
            "跟丢了，请往回滚"
        );
        assert_eq!(
            status_text(Some(Status::Full), Auto::Off, false),
            "已达上限"
        );
        assert_eq!(
            status_text(Some(Status::Tracking), Auto::Off, true),
            "截图失败，见日志"
        );
        assert_eq!(Button::Auto.label(true), "停止");
        assert_eq!(super::size_text(1250, 25826), "1250×25826");
    }
}
