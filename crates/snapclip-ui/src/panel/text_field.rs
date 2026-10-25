//! A number entered into a drawn field rather than into a native control.
//!
//! The size cells and the limit's box were native `EDIT`s, and a native control carries its own
//! background, border, font and selection colours into a panel that draws all four itself. The
//! result was a patch of system styling on every numeric field — the digits arrived at the wrong
//! size and the selection arrived in the system's blue-grey — and none of it could be made to
//! match: a plain `EDIT` has no per-control selection colour at all.
//!
//! What made drawing them affordable is that these fields hold **digits**: no composition, no
//! candidate window, no line breaking, no scrolling at seven characters. The expensive half of a
//! text field is the input method, and a numeric field never needed it. The file name keeps its
//! native control for exactly that reason (`docs/41`).
//!
//! This module is the model and the geometry, with no window and no painter: every rule here is a
//! function of the text and the keys, which is what lets the behaviour be asserted without one.

use snapclip_model::geometry::Rect;

/// How many digits a number may hold. The design's largest cell is a four digit size and the
/// limit's own default is three; seven leaves room for a 9999999 pixel capture without letting the
/// text grow past the box it is drawn in.
pub const MAX_DIGITS: usize = 7;

/// The smallest number a size cell can hold: a zero pixel picture cannot be encoded.
pub const MIN_SIZE: u32 = 1;

/// The largest number a size cell can hold, which is `MAX_DIGITS` nines.
pub const MAX_SIZE: u32 = 9_999_999;

/// What a drawn number looks like right now, for the painter.
///
/// The indices are into `text`, which is ASCII digits, so they are also byte offsets — a fact that
/// only holds because [`digits`] throws everything else away.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NumberView<'a> {
    pub text: &'a str,
    /// The selected range, `(start, end)` with `start <= end`, or `None` when the selection is a
    /// caret. A focused field that just received focus has its whole text selected, which is what
    /// makes typing replace rather than append.
    pub selection: Option<(usize, usize)>,
    pub caret: usize,
    /// Whether the caret belongs on screen. A field that does not have the focus draws no caret and
    /// no selection: the point of both is to say where the next keystroke lands.
    pub focused: bool,
}

/// The three drawn numbers, by field. `None` for one that is not on screen: the limit's box is only
/// drawn while the limit's line is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NumberViews<'a> {
    pub width: Option<NumberView<'a>>,
    pub height: Option<NumberView<'a>>,
    pub limit: Option<NumberView<'a>>,
}

/// The digits of `text`, and nothing else.
///
/// One function, so a paste, a typed run and a number written back by a shortcut all pass through
/// the same filter. A numeric field that accepted a letter would produce a size that does not
/// parse, and the save would silently fall back to the capture's size.
pub fn digits(text: &str) -> String {
    text.chars()
        .filter(|character| character.is_ascii_digit())
        .take(MAX_DIGITS)
        .collect()
}

/// A number as a value, or `None` when the text is not one.
pub fn value_of(text: &str) -> Option<u32> {
    let digits = digits(text);
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

/// One editable number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextField {
    text: String,
    caret: usize,
    /// Where the selection started, or `None` for a bare caret. Kept as an anchor rather than as a
    /// range so that extending a selection with the arrow keys has somewhere to be measured from.
    anchor: Option<usize>,
    focused: bool,
}

impl TextField {
    pub fn new(text: &str) -> Self {
        let text = digits(text);
        Self {
            caret: text.len(),
            text,
            anchor: None,
            focused: false,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The number, when the field holds one.
    pub fn value(&self) -> Option<u32> {
        value_of(&self.text)
    }

    pub fn focused(&self) -> bool {
        self.focused
    }

    /// Take or give up the focus. Gaining it selects everything, so the first keystroke replaces
    /// the number instead of inserting into the middle of it; giving it up drops the selection,
    /// because a selection nobody can act on is just a highlight.
    pub fn set_focused(&mut self, focused: bool) {
        if focused == self.focused {
            return;
        }
        self.focused = focused;
        if focused {
            self.select_all();
        } else {
            self.anchor = None;
            self.caret = self.caret.min(self.text.len());
        }
    }

    /// Replace the text from outside — a preset, a reset, a mode switch.
    ///
    /// The caret goes to the end and any selection is dropped, because the text the caller wrote is
    /// the whole truth about the field now.
    pub fn set_text(&mut self, text: &str) {
        self.text = digits(text);
        self.caret = self.text.len();
        self.anchor = None;
    }

    pub fn select_all(&mut self) {
        self.caret = self.text.len();
        self.anchor = Some(0);
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        if anchor == self.caret {
            None
        } else {
            Some((anchor.min(self.caret), anchor.max(self.caret)))
        }
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    /// The selected text, for a copy.
    pub fn selected_text(&self) -> Option<&str> {
        let (start, end) = self.selection()?;
        Some(&self.text[start..end])
    }

    /// What the painter draws.
    pub fn view(&self) -> NumberView<'_> {
        NumberView {
            text: &self.text,
            selection: self.selection(),
            caret: self.caret,
            focused: self.focused,
        }
    }

    /// Insert one character. Returns whether anything changed.
    ///
    /// Only digits get in, and a full field stays full: `MAX_DIGITS` is what keeps the text inside
    /// the box it is drawn in, so refusing here is the rule rather than an overflow later.
    pub fn insert_char(&mut self, character: char) -> bool {
        if !character.is_ascii_digit() {
            return false;
        }
        self.insert(&character.to_string())
    }

    /// Insert a run of text, keeping its digits. Returns whether anything changed.
    pub fn insert(&mut self, text: &str) -> bool {
        let incoming = digits(text);
        if incoming.is_empty() {
            return false;
        }
        let (start, end) = self.selection().unwrap_or((self.caret, self.caret));
        let room = MAX_DIGITS.saturating_sub(self.text.len() - (end - start));
        let taken: String = incoming.chars().take(room).collect();
        if taken.is_empty() {
            return false;
        }
        self.text.replace_range(start..end, &taken);
        self.caret = start + taken.len();
        self.anchor = None;
        true
    }

    /// Remove the selection, or the character before the caret.
    pub fn backspace(&mut self) -> bool {
        if self.selection().is_some() {
            return self.cut_selection();
        }
        if self.caret == 0 || self.text.is_empty() {
            return false;
        }
        self.caret -= 1;
        self.text.remove(self.caret);
        self.anchor = None;
        true
    }

    /// Remove the selection, or the character after the caret.
    pub fn delete(&mut self) -> bool {
        if self.selection().is_some() {
            return self.cut_selection();
        }
        if self.caret >= self.text.len() {
            return false;
        }
        self.text.remove(self.caret);
        self.anchor = None;
        true
    }

    /// Cut the selection out, for a cut or a replacement.
    fn cut_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            return false;
        };
        self.text.replace_range(start..end, "");
        self.caret = start;
        self.anchor = None;
        true
    }

    pub fn move_left(&mut self) {
        if let Some((start, _)) = self.selection() {
            self.caret = start;
        } else {
            self.caret = self.caret.saturating_sub(1);
        }
        self.anchor = None;
    }

    pub fn move_right(&mut self) {
        if let Some((_, end)) = self.selection() {
            self.caret = end;
        } else {
            self.caret = (self.caret + 1).min(self.text.len());
        }
        self.anchor = None;
    }

    pub fn move_home(&mut self) {
        self.caret = 0;
        self.anchor = None;
    }

    pub fn move_end(&mut self) {
        self.caret = self.text.len();
        self.anchor = None;
    }

    /// Add `delta` to the number, clamped to the field's own range.
    ///
    /// An empty field counts as zero, so one step up from nothing lands on the smallest size rather
    /// than on two: the first press of an arrow key on an empty box has to give something encodable.
    pub fn step(&mut self, delta: i64) {
        let base = i64::from(self.value().unwrap_or(0));
        let next = (base + delta).clamp(i64::from(MIN_SIZE), i64::from(MAX_SIZE));
        self.set_text(&next.to_string());
    }

    /// Bring the number inside `min..=max`, leaving an empty field empty.
    ///
    /// Empty stays empty on purpose: the save reads an empty box as "whatever the capture was", and
    /// filling it in here would turn "I have not decided" into a number nobody chose.
    pub fn clamp_to(&mut self, min: u32, max: u32) {
        let Some(value) = self.value() else {
            return;
        };
        let clamped = value.clamp(min, max);
        if clamped != value {
            self.set_text(&clamped.to_string());
        }
    }
}

/// Where a right aligned number's text starts, inside the box it is drawn in.
///
/// Right alignment is what the design does with all three of these numbers (`:238`, `:243`, the
/// limit's `.limit-input`), and doing it here rather than through a text format's alignment keeps
/// the format shared with the labels that sit left aligned in their own rectangles.
pub fn right_aligned_left(box_rect: Rect, text_width: i32, padding: i32) -> i32 {
    (box_rect.right - padding - text_width).max(box_rect.left)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn focused(text: &str) -> TextField {
        let mut field = TextField::new(text);
        field.set_focused(true);
        field
    }

    #[test]
    fn a_new_field_keeps_the_digits_and_nothing_else() {
        assert_eq!(TextField::new("1341").text(), "1341");
        assert_eq!(TextField::new(" 13a4b1 ").text(), "1341", "letters and spaces go");
        assert_eq!(TextField::new("").text(), "");
        assert_eq!(
            TextField::new("1234567890").text(),
            "1234567",
            "seven digits is the cap"
        );
    }

    #[test]
    fn gaining_the_focus_selects_everything_so_typing_replaces() {
        let mut field = TextField::new("1341");
        assert_eq!(field.selection(), None, "a new field has no selection");
        field.set_focused(true);
        assert_eq!(field.selection(), Some((0, 4)));
        assert!(field.insert_char('9'));
        assert_eq!(field.text(), "9", "the first keystroke replaces the number");
        assert_eq!(field.selection(), None, "and inserts rather than selects");
    }

    #[test]
    fn losing_the_focus_drops_the_selection() {
        let mut field = focused("500");
        assert_eq!(field.selection(), Some((0, 3)));
        field.set_focused(false);
        assert_eq!(field.selection(), None);
        assert_eq!(field.text(), "500", "losing the focus is not a change");
    }

    #[test]
    fn typing_lands_at_the_caret_and_only_digits_get_in() {
        let mut field = focused("12");
        field.move_end();
        assert!(field.insert_char('3'));
        assert_eq!(field.text(), "123");
        field.move_home();
        assert!(field.insert_char('0'));
        assert_eq!(field.text(), "0123");
        assert!(!field.insert_char('a'), "a letter is not a number");
        assert_eq!(field.text(), "0123");
    }

    #[test]
    fn a_paste_keeps_its_digits_and_respects_the_room_left() {
        let mut field = focused("");
        assert!(field.insert("1a2b3"), "the digits of a paste are taken");
        assert_eq!(field.text(), "123");
        // A field that just gained the focus has everything selected, so a paste replaces: the
        // room it has is the whole seven digits.
        let mut replaced = focused("1234");
        assert!(replaced.insert("56789"));
        assert_eq!(replaced.text(), "56789", "a paste replaces the selection");
        let mut full = TextField::new("1234");
        full.move_end();
        assert!(full.insert("567890"), "only what fits is taken");
        assert_eq!(full.text(), "1234567");
        assert!(!full.insert("8"), "a full field refuses rather than overflows");
    }

    #[test]
    fn backspace_and_delete_remove_the_selection_before_they_remove_a_digit() {
        let mut field = focused("1234");
        assert!(field.backspace());
        assert_eq!(field.text(), "", "the selection goes first");
        let mut field = TextField::new("1234");
        field.move_end();
        assert!(field.backspace());
        assert_eq!(field.text(), "123");
        assert!(!field.delete(), "the caret is at the end");
        field.move_home();
        assert!(field.delete());
        assert_eq!(field.text(), "23");
    }

    #[test]
    fn the_arrows_collapse_a_selection_to_its_edges() {
        let mut field = focused("1234");
        field.move_left();
        assert_eq!(field.selection(), None);
        assert_eq!(field.caret(), 0, "left lands on the selection's start");
        field.select_all();
        field.move_right();
        assert_eq!(field.caret(), 4, "right lands on its end");
    }

    #[test]
    fn a_selection_can_be_read_back_for_a_copy() {
        let mut field = focused("1234");
        assert_eq!(field.selected_text(), Some("1234"));
        field.move_home();
        assert_eq!(field.selected_text(), None);
    }

    #[test]
    fn a_step_moves_by_one_or_by_ten_and_stops_at_the_edges() {
        let mut field = TextField::new("100");
        field.step(1);
        assert_eq!(field.text(), "101");
        field.step(10);
        assert_eq!(field.text(), "111");
        field.step(-10);
        assert_eq!(field.text(), "101");
        let mut at_bottom = TextField::new("3");
        at_bottom.step(-10);
        assert_eq!(at_bottom.text(), "1", "a size cannot go below one pixel");
        let mut empty = TextField::new("");
        empty.step(1);
        assert_eq!(empty.text(), "1", "an empty field steps from the smallest size");
        let mut at_top = TextField::new("9999999");
        at_top.step(10);
        assert_eq!(at_top.text(), "9999999", "and stops at the largest");
    }

    #[test]
    fn a_clamp_leaves_an_empty_field_empty() {
        let mut empty = TextField::new("");
        empty.clamp_to(1, MAX_SIZE);
        assert_eq!(empty.text(), "", "an undecided size stays undecided");
        let mut small = TextField::new("0");
        small.clamp_to(1, MAX_SIZE);
        assert_eq!(small.text(), "1");
        let mut large = TextField::new("9999999");
        large.clamp_to(1, 4096);
        assert_eq!(large.text(), "4096");
        let mut inside = TextField::new("800");
        inside.clamp_to(1, 4096);
        assert_eq!(inside.text(), "800", "a number inside the range is left alone");
    }

    #[test]
    fn a_number_written_in_from_outside_lands_with_the_caret_after_it() {
        let mut field = focused("1341");
        field.set_text("672");
        assert_eq!(field.text(), "672");
        assert_eq!(field.caret(), 3);
        assert_eq!(field.selection(), None, "the whole text is what the caller said");
        assert!(field.focused(), "writing a number is not losing the focus");
    }

    #[test]
    fn a_right_aligned_number_is_placed_from_the_box_and_its_own_width() {
        let box_rect = Rect::new(100, 0, 300, 28);
        assert_eq!(right_aligned_left(box_rect, 40, 10), 250);
        assert_eq!(
            right_aligned_left(box_rect, 400, 10),
            100,
            "a number wider than the box starts at its left edge"
        );
    }

    #[test]
    fn the_view_carries_what_the_painter_needs() {
        let field = focused("12");
        let view = field.view();
        assert_eq!(view.text, "12");
        assert_eq!(view.selection, Some((0, 2)));
        assert_eq!(view.caret, 2);
        assert!(view.focused);
    }
}

impl Default for TextField {
    fn default() -> Self {
        Self::new("")
    }
}
