//! The clicks the frame draws and the window answers: hit tests, the token menu, and the caret
//! arithmetic behind inserting a token.
//!
//! Kept beside `field.rs` rather than inside it because these are the rules that decide what a
//! click means, and the geometry next door is long enough already.

use super::*;
use crate::panel::DEMO;

/// Half-open, like every other hit test here: a point on the right or bottom edge belongs to the
/// next rectangle, so two rectangles that share an edge cannot both claim the pixel.
#[test]
fn a_rectangle_owns_its_left_and_top_but_not_its_right_and_bottom() {
    let rect = Rect::new(10, 20, 30, 40);
    assert!(rect_contains(rect, (10, 20)), "the top left corner");
    assert!(rect_contains(rect, (29, 39)), "the last pixel inside");
    assert!(
        !rect_contains(rect, (30, 39)),
        "the right edge belongs to the next box"
    );
    assert!(
        !rect_contains(rect, (29, 40)),
        "so does the bottom edge"
    );
    assert!(!rect_contains(rect, (9, 20)));
}

/// The token goes where the caret is, the selection is replaced rather than kept, and the caret
/// lands after the token — the three things the mockup does (`:200`).
#[test]
fn a_token_lands_where_the_caret_is_and_the_caret_follows_it() {
    let text: Vec<u16> = "Shot_".encode_utf16().collect();
    let (out, caret) = insert_variable(&text, 5, 5, "[日期]");
    assert_eq!(String::from_utf16(&out).unwrap(), "Shot_[日期]");
    assert_eq!(
        caret,
        5 + 4,
        "the caret sits after the token it just inserted"
    );

    let (out, caret) = insert_variable(&text, 0, 5, "[序号]");
    assert_eq!(String::from_utf16(&out).unwrap(), "[序号]");
    assert_eq!(
        caret, 4,
        "a selection is replaced, not kept beside the token"
    );

    let (out, caret) = insert_variable(&[], 0, 0, "[时间]");
    assert_eq!(String::from_utf16(&out).unwrap(), "[时间]");
    assert_eq!(caret, 4);
}

/// The positions are in UTF-16 units, which is what the control counts in. A name with a
/// character outside ASCII has more bytes than units, and mixing the two is how half a character
/// ends up in a file name.
#[test]
fn a_caret_counts_in_units_rather_than_bytes() {
    let text: Vec<u16> = "图_".encode_utf16().collect();
    assert_eq!(text.len(), 2, "two units, more than two bytes in UTF-8");
    let (out, caret) = insert_variable(&text, 1, 1, "[分辨率]");
    assert_eq!(String::from_utf16(&out).unwrap(), "图[分辨率]_");
    assert_eq!(caret, 1 + 5, "five units for `[分辨率]`");
}

/// A caret past the end is clamped rather than refused: the positions and the text can only
/// disagree if the control changed between the two calls, and the useful answer is "insert it".
#[test]
fn a_caret_past_the_end_is_clamped_to_the_end() {
    let text: Vec<u16> = "ab".encode_utf16().collect();
    let (out, caret) = insert_variable(&text, 99, 120, "[序号]");
    assert_eq!(String::from_utf16(&out).unwrap(), "ab[序号]");
    assert_eq!(caret, 6);
    let (out, _) = insert_variable(&text, 2, 0, "[序号]");
    assert_eq!(
        String::from_utf16(&out).unwrap(),
        "ab[序号]",
        "a backwards selection is read as an empty one at the caret"
    );
}

/// The menu hangs under the variable button, overhangs it by four and stays inside the panel
/// (`:915`, `:916`).
#[test]
fn the_token_menu_hangs_under_its_button_and_inside_the_column() {
    for scale in [1.0f32, 1.25, 1.5, 2.0] {
        let rects = DEMO.rects(scale);
        let button = name_wand_rect(&DEMO, &rects, scale);
        let menu = var_menu_rect(&DEMO, &rects, scale);
        let px = |dip: i32| (dip as f32 * scale).round() as i32;
        assert_eq!(
            menu.top,
            button.bottom + px(VAR_MENU_GAP_DIP),
            "the menu hangs eight dip under the button at {scale}"
        );
        assert_eq!(
            menu.right,
            button.right + px(VAR_MENU_OVERHANG_DIP),
            "and overhangs it by four at {scale}"
        );
        assert!(
            menu.right <= rects.header.right,
            "the menu stays inside the panel at {scale}"
        );
        assert_eq!(
            menu.right - menu.left,
            px(VAR_MENU_W_DIP),
            "its width is the design's 184 at {scale}"
        );
    }
}

/// Every token has its own row, the rows tile the menu, and a click in the middle of one answers
/// with that one.
#[test]
fn the_token_rows_tile_the_menu() {
    for scale in [1.0f32, 1.5] {
        let rects = DEMO.rects(scale);
        let menu = var_menu_rect(&DEMO, &rects, scale);
        let first = var_item_rect(&DEMO, &rects, scale, 0);
        let second = var_item_rect(&DEMO, &rects, scale, 1);
        assert_eq!(second.top, first.bottom, "rows are adjacent at {scale}");
        let last = var_item_rect(&DEMO, &rects, scale, VARIABLE_TOKENS.len() - 1);
        assert_eq!(
            last.bottom,
            menu.bottom - (VAR_MENU_PAD_DIP as f32 * scale).round() as i32,
            "the last row ends at the menu's bottom padding at {scale}"
        );
        for index in 0..VARIABLE_TOKENS.len() {
            let row = var_item_rect(&DEMO, &rects, scale, index);
            let centre = ((row.left + row.right) / 2, (row.top + row.bottom) / 2);
            assert_eq!(
                var_item_at(&DEMO, &rects, scale, centre),
                Some(index),
                "the centre of row {index} answers with that row at {scale}"
            );
        }
    }
}

/// A click on a chip answers with that chip and no other: the three words have different lengths,
/// so their rectangles differ.
#[test]
fn each_shortcut_chip_answers_for_itself() {
    let scale = 2.0f32;
    let rects = DEMO.rects(scale);
    let widths = [40, 52, 46];
    for index in 0..3 {
        let chip = path_chip_rect(&DEMO, &rects, scale, &widths, index);
        let centre = ((chip.left + chip.right) / 2, (chip.top + chip.bottom) / 2);
        assert_eq!(
            path_chip_at(&DEMO, &rects, scale, &widths, centre),
            Some(index),
            "chip {index} answers for its own centre"
        );
        let elsewhere = (0..3).any(|other| {
            other != index
                && rect_contains(path_chip_rect(&DEMO, &rects, scale, &widths, other), centre)
        });
        assert!(
            !elsewhere,
            "chip {index}'s centre is outside every other chip"
        );
    }
}

/// The two drawn buttons in the path row answer where they are drawn, by the width the caller
/// measured — the same division as the footer's buttons.
#[test]
fn the_path_buttons_answer_where_they_are_drawn() {
    let scale = 1.5f32;
    let rects = DEMO.rects(scale);
    let change = 60;
    let change_rect = path_change_rect(&DEMO, &rects, scale, change);
    let open_rect = path_open_rect(&DEMO, &rects, scale);
    let centre = |rect: Rect| ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
    assert!(path_change_contains(
        &DEMO,
        &rects,
        scale,
        change,
        centre(change_rect)
    ));
    assert!(path_open_contains(&DEMO, &rects, scale, centre(open_rect)));
    assert!(
        !path_change_contains(&DEMO, &rects, scale, change, centre(open_rect)),
        "the change button does not answer for the folder button"
    );
    assert!(
        !path_open_contains(&DEMO, &rects, scale, centre(change_rect)),
        "nor the other way round"
    );
}

/// The variable button answers inside the strip the name field gave up, and the field's text
/// rectangle stops where the button starts.
#[test]
fn the_variable_button_owns_the_strip_the_field_gave_up() {
    for scale in [1.0f32, 1.5, 2.0] {
        let rects = DEMO.rects(scale);
        let wand = name_wand_rect(&DEMO, &rects, scale);
        let text = name_text_rect(&DEMO, &rects, scale);
        let centre = ((wand.left + wand.right) / 2, (wand.top + wand.bottom) / 2);
        assert!(name_wand_contains(&DEMO, &rects, scale, centre));
        assert!(
            text.right <= wand.left,
            "the text rectangle stops where the button starts, at {scale}"
        );
    }
}

/// A menu covers the boxes it overlaps and no others. The frame cannot draw above a native child,
/// so this list is what the window hides while a menu is open.
#[test]
fn a_menu_covers_only_the_boxes_it_overlaps() {
    let covered = Rect::new(0, 0, 100, 100);
    let fields = [
        Rect::new(10, 10, 20, 20),
        Rect::new(200, 200, 210, 210),
        Rect::new(90, 90, 110, 110),
        Rect::new(0, 100, 50, 150),
    ];
    assert_eq!(
        fields_under(covered, &fields),
        vec![0, 2],
        "an overlap counts; sharing an edge does not"
    );
}

/// The token list is the specification's (`:192`–`:198`), in its order, labels without brackets.
#[test]
fn the_tokens_are_the_ones_the_specification_names() {
    assert_eq!(
        VARIABLE_TOKENS,
        ["[日期]", "[时间]", "[序号]", "[分辨率]", "[软件名]"]
    );
    assert_eq!(
        VARIABLE_LABELS,
        ["日期", "时间", "序号", "分辨率", "软件名"]
    );
}

/// Closing a menu must restore every box it hid, and that invariant belongs to the rule rather than
/// to the handlers: with no menu open, nothing is hidden. Two close paths used to clear the flag and
/// repaint while leaving the boxes hidden, which is what this asserts against.
#[test]
fn a_closed_menu_hides_nothing() {
    let metrics = DEMO;
    let rects = metrics.rects(1.0);
    let boxes: Vec<Rect> = FieldId::ALL
        .iter()
        .map(|id| id.rect(&metrics, &rects, 1.0, 0))
        .collect();
    assert!(
        hidden_fields(None, &boxes).is_empty(),
        "with no menu open every box is shown"
    );
    let menu = fmt_menu_rect(&metrics, &rects, 1.0, metrics.panel_height());
    let covered = hidden_fields(Some(menu), &boxes);
    assert!(!covered.is_empty(), "the format menu covers a field");
    assert!(
        covered.len() < boxes.len(),
        "and the fields it does not cover stay visible"
    );
}

/// The minimize button is the close button's size, one header gap to its left, and it stays inside
/// the header at every density. The gap is absolute rather than derived from the two rectangles, so
/// a wrong constant cannot hide behind the arithmetic that also uses it: it is the number the design
/// gives for two small buttons on one line (`.row-header-actions { gap: 8px }`, `:224`), and it is
/// asserted to be the same token the size row already reads rather than a second 8 that happens to
/// agree today.
#[test]
fn the_minimize_button_sits_one_gap_left_of_the_close() {
    assert_eq!(crate::panel::HEADER_BUTTON_GAP_DIP, 8, ":224");
    assert_eq!(
        crate::panel::HEADER_BUTTON_GAP_DIP,
        SIZE_MODE_GAP_DIP,
        "the header gap and the size row's gap are one token, not two numbers that agree"
    );
    for scale in [1.0_f32, 1.25, 1.5, 2.0] {
        let metrics = DEMO;
        let rects = metrics.rects(scale);
        let px = |dip: i32| (dip as f32 * scale).round() as i32;
        let (close, minimize) = (rects.close, rects.minimize);
        assert_eq!(
            (minimize.width(), minimize.height()),
            (close.width(), close.height()),
            "the two header buttons are the same size at {scale}"
        );
        assert_eq!(
            close.left - minimize.right,
            px(crate::panel::HEADER_BUTTON_GAP_DIP),
            "one header gap between them at {scale}"
        );
        assert_eq!(
            (minimize.top, minimize.bottom),
            (close.top, close.bottom),
            "they share the header's line at {scale}"
        );
        assert!(
            minimize.left >= 0 && minimize.top >= 0 && minimize.bottom <= rects.header.bottom,
            "the minimize button stays inside the header at {scale}: {minimize:?}"
        );
        assert!(
            minimize.right <= close.left,
            "the two buttons cannot overlap at {scale}"
        );
    }
}

/// The picture and the target are different rectangles, the same way they are for the close, so
/// aiming at a corner does not require hitting the 14 dip glyph; and a point on one button is never
/// also a point on the other.
#[test]
fn the_minimize_button_answers_only_inside_itself() {
    let metrics = DEMO;
    for scale in [1.0_f32, 1.25, 1.5, 2.0] {
        let rects = metrics.rects(scale);
        let button = rects.minimize;
        let middle = ((button.left + button.right) / 2, (button.top + button.bottom) / 2);
        assert!(
            minimize_button_contains(&rects, scale, middle),
            "the middle of the button is the button at {scale}"
        );
        assert!(minimize_button_contains(&rects, scale, (button.left, button.top)));
        assert!(
            !minimize_button_contains(&rects, scale, (button.left - 1, middle.1)),
            "one pixel left of the button is not the button at {scale}"
        );
        assert!(
            !minimize_button_contains(&rects, scale, (button.right, middle.1)),
            "the right edge belongs to the gap at {scale}"
        );
        let close_middle = (
            (rects.close.left + rects.close.right) / 2,
            (rects.close.top + rects.close.bottom) / 2,
        );
        assert!(
            minimize_button_contains(&rects, scale, close_middle) == false,
            "a click on the close is not a click on the minimize at {scale}"
        );
    }
}
/// The control's own rectangle: inset horizontally, and only as tall as its text, centred.
///
/// Absolute numbers rather than the constants compared with each other, because the defect this
/// guards against was a control as tall as the frame with its text against the top edge — a
/// relationship the two constants cannot state.
#[test]
fn a_native_control_is_inset_and_centred_at_its_text_height() {
    let frame = Rect::new(100, 200, 320, 248);
    let control = edit_control_rect(frame, 16, 1.0);
    assert_eq!(control.left, 102, "two dip in from the frame at 1x");
    assert_eq!(control.right, 318, "and two dip from the other side");
    assert_eq!(control.bottom - control.top, 16, "only as tall as its text");
    assert_eq!(control.top, 216, "centred in a forty eight dip frame");
    // At one scale the two insets round to the same place, so this is the assertion that tells
    // them apart: three dip at one and a half scale is two dip's inset, where one and a half dip
    // would round to two. Without it the change to clear the corner's arc would be invisible to
    // every test, which is how the square corners survived the first time.
    let scaled = Rect::new(150, 300, 480, 372);
    let wide = edit_control_rect(scaled, 24, 1.5);
    assert_eq!(wide.left, 153, "two dip at one and a half scale is three pixels");
    assert_eq!(wide.right, 477, "and the same on the other side");
    // The height is the font's, so the text is not pushed against an edge: the two gaps above and
    // below are equal save for the rounding an odd remainder forces.
    let above = control.top - frame.top;
    let below = frame.bottom - control.bottom;
    assert!(
        (above - below).abs() <= 1,
        "the text sits in the middle: {above} above, {below} below"
    );
}

/// A native control asks for a system face, not the panel's subset.
///
/// The subset was cut down by pyftsubset, which strips the hinting GDI needs, so a control set in
/// it renders aliased beside the panel's own smooth text. The two names must stay apart.
#[test]
fn a_native_control_asks_for_a_system_face_rather_than_the_embedded_one() {
    assert_ne!(
        NATIVE_FAMILY,
        crate::panel::theme::FAMILY,
        "a native control cannot be set in the subset, which has no hinting for GDI"
    );
    assert!(
        !NATIVE_FAMILY.is_empty(),
        "the system face has to be named for the control to be created at all"
    );
}

/// The two after-save switches are hit where they are drawn, and the gap between them belongs to
/// neither.
#[test]
fn the_after_save_switches_are_hit_where_they_are_drawn() {
    let metrics = DEMO;
    for scale in [1.0f32, 1.25, 1.5, 2.0] {
        let rects = metrics.rects(scale);
        let chips = AfterChips { label: (40.0 * scale).round() as i32, widths: [(52.0 * scale).round() as i32, (64.0 * scale).round() as i32] };
        for index in 0..2 {
            let rect = chips.rect(&metrics, &rects, scale, index);
            let middle = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
            assert_eq!(
                chips.at(&metrics, &rects, scale, middle),
                Some(index),
                "the middle of switch {index} is that switch at {scale}"
            );
        }
        let first = chips.rect(&metrics, &rects, scale, 0);
        let second = chips.rect(&metrics, &rects, scale, 1);
        assert_eq!(
            chips.at(&metrics, &rects, scale, (first.right, (first.top + first.bottom) / 2)),
            None,
            "the right edge is the gap, not the next switch, at {scale}"
        );
        assert_eq!(
            chips.at(&metrics, &rects, scale, (second.left, (second.top + second.bottom) / 2)),
            Some(1),
            "the next switch starts at its own left edge, at {scale}"
        );
        assert_eq!(
            chips.at(&metrics, &rects, scale, (second.right + 8, (second.top + second.bottom) / 2)),
            None,
            "a point past the last switch hits neither at {scale}"
        );
    }
}