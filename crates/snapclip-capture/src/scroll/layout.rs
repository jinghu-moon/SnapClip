//! Where the scroll session's panel goes, in physical pixels of the overlay, and what it says.
//!
//! The panel (`docs/design/overlay-ui-demo.html`, `drawScrolling`) is [`PANEL_WIDTH_DIP`] wide,
//! [`GAP_DIP`] to the right of the selection (to the left when the right side has no room) and
//! bottom-aligned with it. Top to bottom: the thumbnail well, which shows the image's tail and runs
//! from the panel's own top edge to the footer with nothing above or beside it, then a status line
//! and one row of buttons that are also the key hints. The image's size floats on the thumbnail's
//! top-right corner ([`BADGE_INSET_DIP`], [`BADGE_HEIGHT_DIP`]) rather than in a header, so nothing
//! takes a strip of the image's height. The panel grows with the thumbnail, up to the selection's
//! height (at least [`MIN_HEIGHT_DIP`]).
//!
//! Pure arithmetic so the painter, the hit test and the window region agree by construction.

use crate::geometry::Rect;

pub(crate) const PANEL_WIDTH_DIP: i32 = 160;
pub(crate) const GAP_DIP: i32 = 8;
/// The tallest the panel gets when the selection is shorter than this.
pub(crate) const MIN_HEIGHT_DIP: i32 = 120;
pub(crate) const STATUS_DIP: i32 = 20;
pub(crate) const BUTTON_ROW_DIP: i32 = 22;
/// The footer's breathing room: the status line and the buttons keep this far from the panel's
/// sides. The well does not — it is full-bleed, so the image starts at the panel's own edge.
pub(crate) const GUTTER_DIP: i32 = 10;
/// How far the button row and the status line sit above the panel's bottom edge.
const FOOTER_PAD_DIP: i32 = 6;
/// The size badge: how far it floats from the panel's top-right corner, and how tall it is.
const BADGE_INSET_DIP: i32 = 6;
const BADGE_HEIGHT_DIP: i32 = 24;
const BUTTON_SPACING_DIP: i32 = 4;
/// The panel keeps this far from the work area's edges.
const EDGE_DIP: i32 = 4;
/// How far the panel's shadow reaches past it. Less than [`GAP_DIP`], so the shadow never reaches
/// the selection either.
pub(crate) const SHADOW_DIP: i32 = 6;

/// The panel's buttons, left to right. Cancelling is `Esc` alone — it stays a key
/// (`keymap::Action::ScrollCancel`) without taking a slot on the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Button {
    /// Finish and export.
    Finish,
    /// Toggles automatic scrolling.
    Auto,
}

pub(crate) const BUTTONS: [Button; 2] = [Button::Finish, Button::Auto];

impl Button {
    pub(crate) fn action(self) -> crate::keymap::Action {
        use crate::keymap::Action;
        match self {
            Button::Finish => Action::ScrollFinish,
            Button::Auto => Action::ScrollAuto,
        }
    }

    pub(crate) fn label(self, auto: bool) -> &'static str {
        match (self, auto) {
            (Button::Finish, _) => "保存",
            (Button::Auto, false) => "自动",
            (Button::Auto, true) => "停止",
        }
    }
}

/// The header's right side: the image's size so far, `width×rows`.
pub(crate) fn size_text(width: i32, image_rows: u32) -> String {
    format!("{width}×{image_rows}")
}

/// The status line. Empty while everything is going well: the size badge already says how far the
/// capture has come, and a line that repeats it is noise on the panel. Anything the panel has to
/// explain (a failed capture, a lost anchor, the row cap) takes the whole line, because it is the
/// only place the manual buttons leave free.
pub(crate) fn status_text(status: Option<super::Status>, failed: bool) -> String {
    use super::Status;
    match (failed, status) {
        (true, _) => "截图失败，见日志".to_owned(),
        (false, None) | (false, Some(Status::Tracking)) => String::new(),
        (false, Some(Status::Full)) => "已达上限".to_owned(),
        (false, Some(Status::Lost)) => "跟丢了，请往回滚".to_owned(),
    }
}

/// Every string the panel draws apart from key names and digits, for the font subset
/// (`win/d2d/tests.rs`).
#[cfg(test)]
pub(crate) fn drawn_strings() -> Vec<String> {
    use super::Status;
    let mut drawn: Vec<String> = BUTTONS
        .iter()
        .flat_map(|button| [button.label(false), button.label(true)])
        .map(str::to_owned)
        .collect();
    drawn.push(size_text(1234, 56789));
    drawn.push(status_text(None, false));
    drawn.push(status_text(None, true));
    for status in [Status::Tracking, Status::Lost, Status::Full] {
        drawn.push(status_text(Some(status), false));
    }
    drawn
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    pub panel: Rect,
    /// The size badge floating on the thumbnail's top-right corner; it is as wide as the text the
    /// painter measures, so only its right edge and height are settled here.
    pub badge: Rect,
    /// The thumbnail well: the thumbnail is clipped to it, bottom-aligned. It runs from the panel's
    /// top edge to the status line, with no inset on the sides.
    pub well: Rect,
    pub status: Rect,
    pub buttons: [Rect; 2],
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
    px(PANEL_WIDTH_DIP, dpi)
}

/// Everything in the panel that is not the well: the footer and its inset.
fn chrome_height(dpi: u32) -> i32 {
    px(STATUS_DIP, dpi) + px(BUTTON_ROW_DIP, dpi) + px(FOOTER_PAD_DIP, dpi)
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
    let inset = px(FOOTER_PAD_DIP, dpi);
    let gutter = px(GUTTER_DIP, dpi);
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

    let buttons_bottom = panel.bottom - inset;
    let buttons_top = buttons_bottom - px(BUTTON_ROW_DIP, dpi);
    let status = Rect::new(
        left + gutter,
        buttons_top - px(STATUS_DIP, dpi),
        panel.right - gutter,
        buttons_top,
    );
    // The well keeps the panel's full width, so the image reaches the card's own left and right
    // edges the way the loupe's does.
    let well = Rect::new(left, top, panel.right, status.top);
    let badge = Rect::new(
        panel.right - gutter - px(BADGE_INSET_DIP, dpi),
        top + px(BADGE_INSET_DIP, dpi),
        panel.right - gutter - px(BADGE_INSET_DIP, dpi),
        top + px(BADGE_INSET_DIP, dpi) + px(BADGE_HEIGHT_DIP, dpi),
    );

    let spacing = px(BUTTON_SPACING_DIP, dpi);
    let slot = (status.width() - spacing) / 2;
    let mut buttons = [Rect::default(); 2];
    let mut x = status.left;
    for (index, rect) in buttons.iter_mut().enumerate() {
        // The last button takes what the division left over, so the row ends on the status line's
        // right edge.
        let right = if index == 1 { status.right } else { x + slot };
        *rect = Rect::new(x, buttons_top, right, buttons_bottom);
        x = right + spacing;
    }

    let footprint = panel.inflate(px(SHADOW_DIP, dpi));
    Layout {
        panel,
        badge,
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
    use crate::scroll::Status;

    const WORK: Rect = Rect::new(0, 0, 1920, 1040);
    /// Status 20 + buttons 22 + inset 6 at 96 DPI.
    const CHROME: i32 = 48;

    #[test]
    fn the_panel_is_right_of_the_selection_and_bottom_aligned() {
        let selection = Rect::new(400, 200, 1200, 800);
        let placed = layout(selection, WORK, 300, 96);
        assert_eq!(
            placed.panel,
            Rect::new(1208, 800 - 300 - CHROME, 1208 + 160, 800)
        );
        // The well is full-bleed: the panel's own left and right edges, and its top edge too.
        assert_eq!(placed.well, Rect::new(1208, 800 - 348, 1368, 800 - 48));
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
        assert_eq!(thumb_width(96), 160);
        assert_eq!(thumb_width(144), 240);
        let placed = layout(Rect::new(100, 100, 900, 900), WORK, 400, 144);
        assert_eq!(placed.panel.left, 900 + px(8, 144));
        assert_eq!(placed.panel.width(), 240);
        assert_eq!(placed.well.width(), thumb_width(144));
        // The badge keeps its distance from the panel's top-right corner.
        assert_eq!(placed.badge.right, placed.panel.right - px(10, 144) - px(6, 144));
        assert_eq!(placed.badge.top, placed.panel.top + px(6, 144));
        assert_eq!(placed.badge.height(), px(24, 144));
    }

    /// The badge floats on the thumbnail without ever taking a strip of the panel.
    #[test]
    fn the_size_badge_floats_inside_the_well() {
        let selection = Rect::new(400, 200, 1200, 800);
        let placed = layout(selection, WORK, 300, 96);
        assert!(placed.well.contains_rect(placed.badge), "{placed:?}");
        assert_eq!(placed.badge.top, placed.well.top + 6);
        assert_eq!(placed.badge.bottom, placed.well.top + 30);
    }

    /// The buttons share the status line's width, in order, and the hit test finds each one.
    #[test]
    fn the_buttons_span_the_bottom_row_in_order() {
        let selection = Rect::new(400, 200, 1200, 800);
        let placed = layout(selection, WORK, 300, 96);
        assert_eq!(placed.buttons[0].left, placed.status.left);
        assert_eq!(placed.buttons[1].right, placed.status.right);
        assert_eq!(placed.buttons[0].bottom, placed.panel.bottom - 6);
        assert!(placed.status.bottom <= placed.buttons[0].top);
        assert!(placed.well.bottom <= placed.status.top);
        assert!(placed.badge.left >= placed.well.left && placed.badge.right <= placed.well.right);
        assert_eq!(placed.buttons[1].left - placed.buttons[0].right, 4);
        let centre = placed.buttons[0].center();
        assert_eq!(placed.button_at(centre.x, centre.y), Some(Button::Finish));
        let centre = placed.buttons[1].center();
        assert_eq!(placed.button_at(centre.x, centre.y), Some(Button::Auto));
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
    fn the_status_line_is_silent_until_something_needs_saying() {
        // A healthy session leaves the line empty: the badge already carries the size, and a second
        // "how far along" line next to it is noise.
        assert_eq!(status_text(None, false), "");
        assert_eq!(status_text(Some(Status::Tracking), false), "");
        assert_eq!(status_text(Some(Status::Lost), false), "跟丢了，请往回滚");
        assert_eq!(status_text(Some(Status::Full), false), "已达上限");
        assert_eq!(status_text(Some(Status::Tracking), true), "截图失败，见日志");
        // Whatever the session is doing, a failure outranks it.
        assert_eq!(status_text(Some(Status::Full), true), "截图失败，见日志");
        assert_eq!(Button::Auto.label(true), "停止");
        assert_eq!(Button::Finish.label(false), "保存");
        assert_eq!(super::size_text(1250, 25826), "1250×25826");
    }
}
