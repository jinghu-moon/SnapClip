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
