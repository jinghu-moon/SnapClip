//! The first text field: where it sits, what its frame looks like, and how focus and errors
//! recolour that frame. The text itself is a native `EDIT` child of the panel window, inset inside
//! this frame so the rounded corner belongs to us and the child's square corner is hidden by a
//! background of the same colour.
//!
//! Every number here is read off `docs/design/save-panel-demo-v3.html`:
//! - `--fill: rgba(255,255,255,.08)` (:26)  the field background
//! - `--r-input: 6px` (:54)                 the frame corner
//! - `--accent: rgb(31,117,219)` (:20)      the focus border
//! - `--danger: rgb(255,123,123)` (:22)     the error border
//! - `--text: #fff` (:24)                   the text and caret colour
//! - `.text-input { border: 1px solid transparent }` (:167)  idle has no visible border
//! - `.text-input:focus { border-color: accent; box-shadow: inset 0 0 0 1px accent }` (:169)
//! - `.text-input.is-error { border-color: danger }` (:333)
//! - `.text-input.is-warn { border-color: warning }` (:334), with `--warning: rgb(240,179,90)` (:23)
//!   shared by `.status.is-warning` (:327) and the conflict link (:328)
//! - `.text-input { padding: 0 10px; height: 32px }` (:167)

use crate::panel::frame::{FrameMetrics, FrameRects};
use crate::panel::interaction::PointerEvent;
use crate::panel::segmented::{Segment, SegmentState};
use snapclip_model::geometry::{Point, Rect};

/// How far the native `EDIT` sits inside its frame, in dip. One and a half, so a one dip border
/// plus the inset leaves the rounded corner visible at every DPI we support without the child
/// painting over it.
pub const EDIT_INSET_DIP: f32 = 1.5;

/// The horizontal padding the design puts between the frame and the text.
pub const EDIT_PADDING_DIP: f32 = 10.0;

/// `--fs-input: 12px` — `save-panel-demo-v3.html:39`. The size of the text **inside** the field.
///
/// This is not the field's height, and the two were confused once: the native control was given a
/// font as tall as its own box, which filled it edge to edge. A field is thirty two dip tall and
/// its text is twelve.
pub const EDIT_FONT_DIP: f32 = 12.0;

/// `--fs-num: 16px` — `save-panel-demo-v3.html:33`. The size cells' numbers, in the numeric face.
pub const NUMBER_EDIT_FONT_DIP: f32 = 16.0;

/// The height GDI wants for a font at this size: `CreateFontW` takes logical units, and the process
/// is per-monitor aware, so a scaled negative height is the pixel size the design asks for.
pub fn edit_font_height_for(scale: f32, font_dip: f32) -> i32 {
    -((f64::from(font_dip) * f64::from(scale)).round() as i32)
}

/// The height GDI wants for a field's font at the UI size.
pub fn edit_font_height(scale: f32) -> i32 {
    edit_font_height_for(scale, EDIT_FONT_DIP)
}

/// `--r-input: 6px` — `save-panel-demo-v3.html:54`. The frame's corner; the native child is inset
/// so this is the corner the user actually sees.
pub const INPUT_RADIUS_DIP: f32 = 6.0;

/// The colour of the field frame, and of the ring inside it when it has focus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FieldColours {
    /// `--fill` composited over the panel surface, so the child can be given the same colour.
    pub fill: [u8; 4],
    /// The frame border.
    pub border: [u8; 4],
    /// The one dip ring drawn inside the border. Only focus has one, which is the whole reason the
    /// native control cannot be trusted to draw this state itself.
    pub inset: Option<[u8; 4]>,
    /// The text colour handed to `WM_CTLCOLOREDIT`.
    pub text: [u8; 4],
}

/// The states the design distinguishes. Idle, focused, and the two message levels that recolour
/// the border. The values are not invented: they are the four rules quoted at the top of the file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FieldState {
    Idle,
    Focused,
    Error,
    Warning,
}

impl FieldState {
    /// Whether this state is a message about the value rather than about the cursor.
    ///
    /// The design lets both apply at once (`:169` focus, `:333`/`:334` the message), and the message
    /// wins because its rules come later. Paint order is therefore not a decision anyone makes here:
    /// a field that has something wrong with it keeps saying so while it is being edited, which is
    /// also when the user needs to read it.
    pub fn is_message(self) -> bool {
        matches!(self, FieldState::Error | FieldState::Warning)
    }
}

/// Which field of the form a native `EDIT` is.
///
/// The window keeps a table of control handle to identity, and every question about a control — what
/// colours its background, which field just took focus — is asked through this identity. That is the
/// whole point of the type: without it the message handlers have to compare handles one field at a
/// time, and the next field added silently gets answered from the previous one's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldId {
    Path,
    Name,
    SizeWidth,
    SizeHeight,
}

impl FieldId {
    /// Every field, in the order they were created. A new field is added here and to the control
    /// table, and both the drawing loop and the message handlers pick it up without a new branch.
    pub const ALL: [FieldId; 4] = [
        FieldId::Path,
        FieldId::Name,
        FieldId::SizeWidth,
        FieldId::SizeHeight,
    ];

    /// Where the child window goes. One formula per row and one per cell, so the control and the
    /// frame around it cannot disagree about where the field is.
    ///
    /// A size cell is not a single box: the design puts a `宽` or `高` label before the number and a
    /// `px` unit after it, all three inside one wrapper (`:237`, `:242`, `:244`). The control is the
    /// number alone, so it takes the input rectangle; the wrapper is painted separately.
    pub fn rect(
        self,
        metrics: &FrameMetrics,
        rects: &FrameRects,
        scale: f32,
    ) -> Rect {
        match self {
            FieldId::Path => field_rect(metrics, rects, scale, Row::Path),
            FieldId::Name => field_rect(metrics, rects, scale, Row::Name),
            FieldId::SizeWidth => size_cell_input_rect(metrics, rects, scale, SizeCell::Width),
            FieldId::SizeHeight => size_cell_input_rect(metrics, rects, scale, SizeCell::Height),
        }
    }

    /// Where the frame is painted. This is the same rectangle as [`FieldId::rect`] for a row, whose
    /// field is one box, and the whole wrapper for a size cell, because the label and the unit share
    /// it with the number.
    pub fn frame_rect(
        self,
        metrics: &FrameMetrics,
        rects: &FrameRects,
        scale: f32,
    ) -> Rect {
        match self {
            FieldId::SizeWidth => size_cell_rect(metrics, rects, scale, SizeCell::Width),
            FieldId::SizeHeight => size_cell_rect(metrics, rects, scale, SizeCell::Height),
            FieldId::Path | FieldId::Name => self.rect(metrics, rects, scale),
        }
    }

    /// The face this field's native control is set in.
    ///
    /// Kept beside the identity rather than at the creation site, so that "which fields are
    /// numeric" has one answer next to the thing it describes — the same reason the rectangles live
    /// here. The mockup sets the size cells' input and unit in `--num` (`:242`, `:244`) and nothing
    /// else in the form.
    pub fn face(self) -> FieldFace {
        match self {
            FieldId::SizeWidth | FieldId::SizeHeight => FieldFace::Number,
            FieldId::Path | FieldId::Name => FieldFace::Text,
        }
    }
}

/// Which typeface a field's native control asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldFace {
    /// The UI font: the path and the file name (`--fs-input: 12px`, `:39`).
    Text,
    /// The embedded numeric face: the size cells (`--fs-num: 16px`, `:33`). `.dim-input` also sets
    /// `text-align: right` (`:242`), so a number is right aligned inside its control.
    Number,
}

impl FieldFace {
    /// The font size this face is set at, in dip.
    pub fn font_dip(self) -> f32 {
        match self {
            FieldFace::Text => EDIT_FONT_DIP,
            FieldFace::Number => NUMBER_EDIT_FONT_DIP,
        }
    }

    /// Whether the control's text is right aligned. `.dim-input { text-align: right }` (`:242`).
    pub fn right_aligned(self) -> bool {
        matches!(self, FieldFace::Number)
    }
}

#[cfg(test)]
mod face_tests {
    use super::*;

    /// Which fields are numeric is a decision, not an accident of a call site: the two size cells
    /// and nothing else. `.dim-input` and `.dim-unit` are the only `--num` users in the form
    /// (`save-panel-demo-v3.html:242`, `:244`).
    #[test]
    fn only_the_size_cells_are_set_in_the_numeric_face() {
        assert_eq!(FieldId::SizeWidth.face(), FieldFace::Number);
        assert_eq!(FieldId::SizeHeight.face(), FieldFace::Number);
        assert_eq!(FieldId::Path.face(), FieldFace::Text);
        assert_eq!(FieldId::Name.face(), FieldFace::Text);
    }

    /// `--fs-num: 16px` (`:33`) against `--fs-input: 12px` (`:39`), and the number is right
    /// aligned while the file name is not. Both come from the face rather than from the call site,
    /// so a control cannot be created at the wrong size or with the wrong alignment.
    #[test]
    fn the_numeric_face_is_larger_and_right_aligned() {
        assert_eq!(FieldFace::Number.font_dip(), NUMBER_EDIT_FONT_DIP);
        assert_eq!(FieldFace::Text.font_dip(), EDIT_FONT_DIP);
        assert!(FieldFace::Number.font_dip() > FieldFace::Text.font_dip());
        assert!(FieldFace::Number.right_aligned());
        assert!(!FieldFace::Text.right_aligned());
    }

    /// GDI takes logical units, and the process is per-monitor aware, so the height is the size in
    /// pixels, negated. A face's size has to survive that conversion at every scale.
    #[test]
    fn a_face_height_is_its_size_at_every_scale() {
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            for face in [FieldFace::Text, FieldFace::Number] {
                let expected = -((f64::from(face.font_dip()) * f64::from(scale)).round() as i32);
                assert_eq!(edit_font_height_for(scale, face.font_dip()), expected);
            }
        }
        assert_eq!(edit_font_height(1.5), edit_font_height_for(1.5, EDIT_FONT_DIP));
    }
}

/// Every field's frame state, together.
///
/// Focus is not stored as "the focused field" plus a per-control copy: the focus notifications are
/// the only writer, and they name the field they are about. So a field that is added later is
/// answered from its own entry rather than from whatever the window happened to track before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldStates {
    path: FieldState,
    name: FieldState,
    size_width: FieldState,
    size_height: FieldState,
}

impl FieldStates {
    /// Nothing focused, nothing wrong.
    pub const IDLE: FieldStates = FieldStates {
        path: FieldState::Idle,
        name: FieldState::Idle,
        size_width: FieldState::Idle,
        size_height: FieldState::Idle,
    };

    /// The state of one field.
    pub fn state_of(self, id: FieldId) -> FieldState {
        match id {
            FieldId::Path => self.path,
            FieldId::Name => self.name,
            FieldId::SizeWidth => self.size_width,
            FieldId::SizeHeight => self.size_height,
        }
    }

    /// Put one field into a state, leaving the others alone.
    pub fn set(&mut self, id: FieldId, state: FieldState) {
        match id {
            FieldId::Path => self.path = state,
            FieldId::Name => self.name = state,
            FieldId::SizeWidth => self.size_width = state,
            FieldId::SizeHeight => self.size_height = state,
        }
    }

    /// Apply a focus change, unless the field is saying something about its value.
    ///
    /// This is the only rule that keeps a validation message alive while the user edits the field
    /// that has it: the notification arrives, the message would be overwritten by `Focused` or
    /// `Idle`, and the border would go quiet exactly when the text is still wrong. On losing focus
    /// the message stays for the same reason. It is cleared where a new attempt is made, which is
    /// the only place that knows the answer changed.
    pub fn set_focus(&mut self, id: FieldId, focused: bool) {
        if self.state_of(id).is_message() {
            return;
        }
        self.set(id, if focused { FieldState::Focused } else { FieldState::Idle });
    }

    /// Put a message on one field. Returns whether anything was there to clear afterwards.
    pub fn mark(&mut self, problem: Option<(FieldId, FieldState)>) {
        if let Some((id, state)) = problem {
            self.set(id, state);
        }
    }

    /// Clear every message, leaving focus alone. Called when a save attempt starts, so a sentence
    /// about the previous attempt cannot outlive it.
    pub fn clear_messages(&mut self) {
        for id in FieldId::ALL {
            if self.state_of(id).is_message() {
                self.set(id, FieldState::Idle);
            }
        }
    }

    /// Exactly one field focused, the rest idle.
    pub fn focused_on(id: Option<FieldId>) -> Self {
        let mut states = FieldStates::IDLE;
        if let Some(id) = id {
            states.set(id, FieldState::Focused);
        }
        states
    }

    /// The name field in `state`, the rest idle.
    ///
    /// The frame's entry points take a single state because that is what most callers and every
    /// bitmap test mean: the first field's frame. Named for the field it describes rather than left
    /// as a bare state so the two cannot be confused later.
    pub fn name_in(state: FieldState) -> Self {
        FieldStates {
            name: state,
            ..FieldStates::IDLE
        }
    }

    /// Every field with its state, in `FieldId::ALL` order, for the drawing loop.
    pub fn iter(self) -> [(FieldId, FieldState); FieldId::ALL.len()] {
        let mut out = [(FieldId::Path, FieldState::Idle); FieldId::ALL.len()];
        for (slot, id) in out.iter_mut().zip(FieldId::ALL) {
            *slot = (id, self.state_of(id));
        }
        out
    }
}

/// `--fill` is white at eight percent over the surface, so the child and the frame agree without
/// making the child translucent, which a `EDIT` cannot be.
fn fill_over_surface(surface: [u8; 4]) -> [u8; 4] {
    let mix = |c: u8| ((u16::from(c) * 92 + 255 * 8) / 100) as u8;
    [mix(surface[0]), mix(surface[1]), mix(surface[2]), 255]
}

/// `--warning: rgb(240,179,90)` — `save-panel-demo-v3.html:23`. The one warning yellow.
///
/// Three rules use it: a warning field's border (`:334`), the status line's warning (`:327`) and the
/// conflict link (`:328`). It was two literals before — the border's was a brighter yellow taken by
/// eye — so the same message was drawn in two colours depending on where you looked.
pub const WARNING_RGBA: [u8; 4] = [240, 179, 90, 255];

/// The colours for a state. Pure, so the rules can be tested without a device.
pub fn field_colours(state: FieldState, surface: [u8; 4]) -> FieldColours {
    const ACCENT: [u8; 4] = [31, 117, 219, 255];
    const DANGER: [u8; 4] = [255, 123, 123, 255];
    const WARNING: [u8; 4] = WARNING_RGBA;
    const TEXT: [u8; 4] = [255, 255, 255, 255];
    const CLEAR: [u8; 4] = [0, 0, 0, 0];

    let fill = fill_over_surface(surface);
    match state {
        FieldState::Idle => FieldColours { fill, border: CLEAR, inset: None, text: TEXT },
        FieldState::Focused => FieldColours {
            fill,
            border: ACCENT,
            inset: Some(ACCENT),
            text: TEXT,
        },
        FieldState::Error => FieldColours { fill, border: DANGER, inset: None, text: TEXT },
        FieldState::Warning => FieldColours { fill, border: WARNING, inset: None, text: TEXT },
    }
}

/// Which field a validation message is about, and how loudly it says it.
///
/// The key travels from the save service, so this is the one place that decides where a sentence
/// appears. A message with no field is still said on the status line — the mapping returning `None`
/// means "the row is fine, the attempt is not", not "say nothing".
pub fn problem_of(
    copy: crate::copy::ValidationCopy,
) -> Option<(FieldId, FieldState)> {
    use crate::copy::{Level, ValidationCopy};
    let field = match copy {
        ValidationCopy::PathEmpty
        | ValidationCopy::PathNotAbsolute
        | ValidationCopy::PathIllegalChars
        | ValidationCopy::NoWritePermission
        | ValidationCopy::NotEnoughSpace
        | ValidationCopy::DirectoryMissing
        | ValidationCopy::PathTooLong => FieldId::Path,
        ValidationCopy::StemEmpty
        | ValidationCopy::StemIllegalChars
        | ValidationCopy::StemReservedName
        | ValidationCopy::StemTrailingDotOrSpace => FieldId::Name,
    };
    let state = match copy.level() {
        Level::Warn => FieldState::Warning,
        Level::Error => FieldState::Error,
    };
    Some((field, state))
}

/// The rect of the first form slot's field, derived from the design's own slot arithmetic rather
/// than from a second set of numbers: the label row, its gap, then the field — less the format
/// button that shares the row (`:554`–`:561`, `.file-row { gap: 8px }`, `:171`).
///
/// The name field is the `.name-wrap` of the mockup (`:555`, `flex: 1`), so it takes the row's width
/// minus the gap and the eighty dip button. That also narrows the native `EDIT` inside it, which is
/// the point: the button must not sit on top of the text the user is typing.
pub fn name_field_rect(metrics: &FrameMetrics, rects: &FrameRects, scale: f32) -> Rect {
    let band = field_rect(metrics, rects, scale, Row::Name);
    fmt_name_field_rect(metrics, rects, scale, band)
}

/// The save path field, the first row of the form.
pub fn path_field_rect(metrics: &FrameMetrics, rects: &FrameRects, scale: f32) -> Rect {
    field_rect(metrics, rects, scale, Row::Path)
}

/// Which row of the form a field sits in. The design puts the save path above the file name
/// (`save-panel-demo-v3.html:542` is the path row, `:556` the name row), so the path is row zero
/// even though the name field existed first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Path,
    Name,
    Saved,
    Size,
    Quality,
    Estimate,
}

/// Every row of the form, top to bottom, in the order the design lays them out: the path row
/// (`:542`), the name row (`:556`), the after-save row (`:568`), the size row (`:235`), the quality
/// row (`:637`) and the estimate row (`:666`). The layout walks this list to add up the rows above a
/// given one, and the painter walks it to draw the labels, so a row missing from it is a row the
/// rest of the form does not know about.
pub const FORM_ROWS: [Row; 6] = [
    Row::Path,
    Row::Name,
    Row::Saved,
    Row::Size,
    Row::Quality,
    Row::Estimate,
];

impl Row {
    fn index(self) -> usize {
        match self {
            Row::Path => 0,
            Row::Name => 1,
            Row::Saved => 2,
            Row::Size => 3,
            Row::Quality => 4,
            Row::Estimate => 5,
        }
    }

    /// Which of the design's three groups the row belongs to. Its own note on the group margin
    /// spells them out — `组间 16 / 组内 12：位置·文件名 | 尺寸·质量 | 预估` (`:163`) — so the path,
    /// name and after-save rows share a group, the size and quality rows share the next, and the
    /// estimate row opens the last one with `.form-row.grp` (`:666`). The after-save row carries no
    /// `.grp` (`:569`), which is why it does not open a group even though it is not a field.
    fn group(self) -> i32 {
        match self {
            Row::Path | Row::Name | Row::Saved => 0,
            Row::Size | Row::Quality => 1,
            Row::Estimate => 2,
        }
    }

    /// Whether the row has a content box under its label. Most rows do, but the after-save row is a
    /// `.row-header` on its own (`:569`–`:577`) — its label and its two chips are the whole row — so
    /// it has no second child and therefore no `.form-row { gap: 6px }` (`:164`) either. Adding the
    /// gap unconditionally would push every row below it down by six.
    fn has_content(self) -> bool {
        !matches!(self, Row::Saved)
    }

    /// The height of the row's content box. It is not the same for every row: a text field is
    /// `height: 32px` (`:167`), while the quality row's content is `.quality-line` and the estimate
    /// row's is `.est-size`, and both of those are `height: var(--row)` — **28** (`:306`, `:269`,
    /// `:47`). The rows below inherit the difference, so it is stated per row rather than assumed to
    /// be the field height. The after-save row has no content box at all, so its height is zero and
    /// callers must ask `has_content` first.
    fn content_height(self, metrics: &FrameMetrics) -> i32 {
        match self {
            Row::Path | Row::Name | Row::Size => metrics.field_height,
            Row::Quality | Row::Estimate => ROW_DIP,
            Row::Saved => 0,
        }
    }

    /// The row's label, one character for one character from the design: the save path
    /// (`save-panel-demo-v3.html:542`), the file name (`:553`), the after-save actions (`:571`), the
    /// size (`:582`), the quality (`:630`) and the estimate (`:667`). They are produced here rather
    /// than copied into the painter so the font subset's list can call this and cannot drift from
    /// what is drawn.
    pub fn label(self) -> &'static str {
        match self {
            Row::Path => "保存路径",
            Row::Name => "文件名",
            Row::Saved => "保存后",
            Row::Size => "尺寸",
            Row::Quality => "质量",
            Row::Estimate => "预估大小",
        }
    }
}

/// The top of a row's label, which is the top of the row itself. Every row starts with its label,
/// and the rows above it are added up: a label row (`.row-header { height: 22px }`, `:165`), the gap
/// under it (`.form-row { gap: 6px }`, `:164`) and the row's content — but only for a row that has
/// content, because the after-save row is a header on its own. Then the gap to the next row
/// (`.form { gap: 12px }`, `:162`), plus the group's own margin when the next row opens a new group
/// (`.form-row.grp { margin-top: 4px }`, `:163`), which is what makes groups sit 16 apart while rows
/// inside one sit 12 apart.
///
/// The rows are added up rather than multiplied by an index, because their content heights differ
/// and a single pitch would put the rows below the quality row in the wrong place.
fn row_label_top(metrics: &FrameMetrics, rects: &FrameRects, scale: f32, row: Row) -> i32 {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let mut above = 0;
    for (index, candidate) in FORM_ROWS.iter().enumerate().take(row.index()) {
        above += metrics.row_label_height;
        if candidate.has_content() {
            above += metrics.row_gap + candidate.content_height(metrics);
        }
        above += metrics.row_spacing;
        if FORM_ROWS[index + 1].group() != candidate.group() {
            above += metrics.group_spacing;
        }
    }
    rects.form.top + px(above)
}

/// The label row above a row's content: `.row-header { height: 22px }` (`:165`) sits `6` (`:164`)
/// above the content it names, and for a row with no content it is the whole row. It spans the
/// column rather than the field, because the design puts a row's other furniture — the size mode
/// buttons, the quality switch, the after-save chips — on the same line (`:583`, `:631`, `:572`).
pub fn label_rect(metrics: &FrameMetrics, rects: &FrameRects, scale: f32, row: Row) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let top = row_label_top(metrics, rects, scale, row);
    Rect {
        left: rects.form.left,
        top,
        right: rects.form.right,
        bottom: top + px(metrics.row_label_height),
    }
}

/// The rect of one row's content box, which sits a label row and a gap below the row's own top. One
/// function for every row, so a row cannot drift away from the others. Rows without content — the
/// after-save row — have no box here; ask `Row::has_content` before using the result.
pub fn field_rect(metrics: &FrameMetrics, rects: &FrameRects, scale: f32, row: Row) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let top = row_label_top(metrics, rects, scale, row) + px(metrics.row_label_height + metrics.row_gap);
    Rect {
        left: rects.form.left,
        top,
        right: rects.form.right,
        bottom: top + px(row.content_height(metrics)),
    }
}

/// The two cells of the size row. The design lays this row out as two flex cells with a link hub
/// between them (`.values { gap: 8px }`, `:235`; `.link-hub { width: 28px }`, `:246`), so the row
/// cannot use the full-width slot the other rows use. It keeps their vertical arithmetic and splits
/// the width itself, so the row still sits on its slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeCell {
    Width,
    Height,
}

/// `.cell-label { width: 20px }` (`:237`) — the `宽` / `高` word before each cell.
pub const SIZE_LABEL_DIP: i32 = 20;

/// `.values { gap: 8px }` (`:235`) — the gap between the row's five children: label, cell, hub,
/// label, cell.
pub const SIZE_GAP_DIP: i32 = 8;

/// `.dim-unit { margin-left: 4px }` (`:244`) — the space between the number and its unit.
pub const SIZE_UNIT_MARGIN_DIP: i32 = 4;

/// The box reserved for the `px` unit after each number.
///
/// The design gives the unit its font size (`--fs-num`'s 12px, `:244`) and its left margin, but not
/// its width: that depends on the glyphs, and this module has no device context to measure text
/// with. The unit is a fixed two-letter string, so the box is reserved rather than measured, and
/// the test only asks that the three parts of a cell do not overlap and stay inside it.
pub const SIZE_UNIT_DIP: i32 = 18;

/// The flex row's fixed width: two labels, four gaps and the hub. What is left over is shared by
/// the two cells.
fn size_row_fixed_dip() -> i32 {
    2 * SIZE_LABEL_DIP + 4 * SIZE_GAP_DIP + LINK_HUB_DIP
}

/// One cell's box, which is the input wrapper: `.input-wrap { flex: 1; height: 32px; background:
/// var(--fill) }` (`:238`) inside `.cell { flex: 1 }` (`:236`). The cell's label sits before it, so
/// this is the cell minus its label and one gap.
pub fn size_cell_rect(
    metrics: &FrameMetrics,
    rects: &FrameRects,
    scale: f32,
    cell: SizeCell,
) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let row = field_rect(metrics, rects, scale, Row::Size);
    let gap = px(SIZE_GAP_DIP);
    let label = px(SIZE_LABEL_DIP);
    let hub = link_hub_rect(metrics, rects, scale);
    let cell_width = (row.right - row.left - px(size_row_fixed_dip())) / 2;
    let left = match cell {
        SizeCell::Width => row.left + label + gap,
        SizeCell::Height => hub.right + gap + label + gap,
    };
    // The design lays the row out with flex, so the second cell takes whatever the first cell, the
    // hub and the labels leave behind. Integer division drops a pixel on odd widths, and the flex
    // behaviour is for the last cell to absorb it rather than for the row to stop short of the form
    // edge.
    let right = match cell {
        SizeCell::Width => left + cell_width,
        SizeCell::Height => row.right,
    };
    Rect {
        left,
        top: row.top,
        right,
        bottom: row.bottom,
    }
}

/// The `宽` / `高` word before a cell: 20 dip wide, on the cell's line (`:237`, `:595`, `:605`).
pub fn size_cell_label_rect(
    metrics: &FrameMetrics,
    rects: &FrameRects,
    scale: f32,
    cell: SizeCell,
) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let wrapper = size_cell_rect(metrics, rects, scale, cell);
    let gap = px(SIZE_GAP_DIP);
    let label = px(SIZE_LABEL_DIP);
    Rect {
        left: wrapper.left - gap - label,
        top: wrapper.top,
        right: wrapper.left - gap,
        bottom: wrapper.bottom,
    }
}

/// The number's own box inside the wrapper: `.dim-input` is right aligned and the unit follows it
/// (`:242`–`:244`), so the number ends where the unit's reserved box begins.
pub fn size_cell_input_rect(
    metrics: &FrameMetrics,
    rects: &FrameRects,
    scale: f32,
    cell: SizeCell,
) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let wrapper = size_cell_rect(metrics, rects, scale, cell);
    let inset = px(EDIT_PADDING_DIP as i32);
    Rect {
        left: wrapper.left + inset,
        top: wrapper.top,
        right: wrapper.right - inset - px(SIZE_UNIT_MARGIN_DIP) - px(SIZE_UNIT_DIP),
        bottom: wrapper.bottom,
    }
}

/// The `px` unit's box at the wrapper's right edge (`:244`).
pub fn size_cell_unit_rect(
    metrics: &FrameMetrics,
    rects: &FrameRects,
    scale: f32,
    cell: SizeCell,
) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let wrapper = size_cell_rect(metrics, rects, scale, cell);
    let inset = px(EDIT_PADDING_DIP as i32);
    Rect {
        left: wrapper.right - inset - px(SIZE_UNIT_DIP),
        top: wrapper.top,
        right: wrapper.right - inset,
        bottom: wrapper.bottom,
    }
}

/// `.link-hub { width: 28px; height: 28px }` (`:246`) — the hub between the two size cells.
pub const LINK_HUB_DIP: i32 = 28;

/// `.link-hub svg { width: 16px; height: 16px }` (`:249`) — the icon's own box, centred in the hub.
pub const LINK_HUB_ICON_DIP: i32 = 16;

/// The hub's rectangle in the size row. One source for it, so the icon is drawn exactly where the
/// two cells were laid out around it: `cell | gap | hub | gap | cell`, with the hub taking the
/// width the flex row leaves and the second cell absorbing the odd pixel (see [`size_cell_rect`]).
pub fn link_hub_rect(metrics: &FrameMetrics, rects: &FrameRects, scale: f32) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let row = field_rect(metrics, rects, scale, Row::Size);
    let gap = px(8);
    let hub = px(LINK_HUB_DIP);
    let left = row.left + (row.right - row.left - 2 * gap - hub) / 2 + gap;
    Rect {
        left,
        top: row.top,
        right: left + hub,
        bottom: row.bottom,
    }
}

/// The icon's box inside the hub: 16 dip square, centred in the 28 dip hub (`:246`, `:249`).
pub fn link_hub_icon_rect(metrics: &FrameMetrics, rects: &FrameRects, scale: f32) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let hub = link_hub_rect(metrics, rects, scale);
    let side = px(LINK_HUB_ICON_DIP);
    let left = hub.left + (hub.right - hub.left - side) / 2;
    let top = hub.top + (hub.bottom - hub.top - side) / 2;
    Rect {
        left,
        top,
        right: left + side,
        bottom: top + side,
    }
}

/// The close glyph's box inside the header button: 14 dip square, centred in the 26 dip button
/// (`.close svg { width: 14px; height: 14px }`, `:98`; the button is `.close { width: 26px }`, `:96`).
pub const CLOSE_ICON_DIP: i32 = 14;

/// The drawn close glyph, centred in `rects.close`.
///
/// The picture and the clickable area are two rectangles on purpose: the mockup's button is 26 dip
/// and the svg inside it is 14 (`:96`, `:98`), and someone aiming at the corner should not have to
/// hit 14 of them. The frame paints this rectangle; the window hit tests the button.
pub fn close_icon_rect(_metrics: &FrameMetrics, rects: &FrameRects, scale: f32) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let button = rects.close;
    let side = px(CLOSE_ICON_DIP);
    let left = button.left + (button.right - button.left - side) / 2;
    let top = button.top + (button.bottom - button.top - side) / 2;
    Rect {
        left,
        top,
        right: left + side,
        bottom: top + side,
    }
}

/// Whether a client-area point is inside the close button.
///
/// Pure on purpose: the window's message handler cannot be tested without a window, and a hit test
/// that only exists inside one is a hit test nothing checks. The button is `rects.close`, which is
/// 26 dip (`:96`) — the glyph inside it is 14 (`:98`).
pub fn close_button_contains(rects: &FrameRects, scale: f32, point: (i32, i32)) -> bool {
    let _ = scale;
    let button = rects.close;
    point.0 >= button.left && point.0 < button.right && point.1 >= button.top && point.1 < button.bottom
}

/// `.size-mode { padding: 2px; border: 1px solid var(--line-soft); border-radius: var(--r-input) }`
/// (`:225`, `:54`) — the two-button container at the end of the size row's header line.
pub const SIZE_MODE_PADDING_DIP: i32 = 2;
pub const SIZE_MODE_BORDER_DIP: i32 = 1;
pub const SIZE_MODE_RADIUS_DIP: i32 = 6;

/// `.size-mode-btn { padding: 0 10px; height: 20px; border-radius: 4px }` (`:226`).
pub const SIZE_MODE_BUTTON_HEIGHT_DIP: i32 = 20;
pub const SIZE_MODE_BUTTON_PADDING_DIP: i32 = 10;
pub const SIZE_MODE_BUTTON_RADIUS_DIP: i32 = 4;

/// `.row-header-actions { gap: 8px }` (`:224`) — between the mode container and the reset button.
pub const SIZE_MODE_GAP_DIP: i32 = 8;

/// `.action { width: 24px; height: 24px; border-radius: var(--r-icon) }` (`:229`, `:60`) and
/// `.action svg { width: 14px; height: 14px }` (`:232`).
pub const SIZE_MODE_RESET_DIP: i32 = 24;
pub const SIZE_MODE_RESET_RADIUS_DIP: i32 = 5;
pub const SIZE_MODE_RESET_ICON_DIP: i32 = 14;

/// The two mode words, verbatim from the design: `数值` (`:585`) and `百分比` (`:586`).
pub const SIZE_MODE_WORDS: [&str; 2] = ["数值", "百分比"];

/// Which word the design marks as on: `数值` carries `is-on` (`:585`). The buttons are painted but
/// do not respond yet, so the choice is a constant rather than state; `docs/41` records the gap.
pub const SIZE_MODE_CHOSEN: usize = 0;

/// `--line-soft` is white at eighteen percent over whatever is behind it (`:28`), resolved here the
/// way `--fill` is, because our brushes are opaque.
pub fn line_soft_over_surface(surface: [u8; 4]) -> [u8; 4] {
    let mix = |c: u8| ((u16::from(c) * 82 + 255 * 18) / 100) as u8;
    [mix(surface[0]), mix(surface[1]), mix(surface[2]), 255]
}

/// The footer's top border: `border-top: 1px solid rgba(255,255,255,.04)` (`:274`), resolved over
/// the surface for the same reason as [`line_soft_over_surface`]. It is deliberately fainter than
/// `--line-soft`: the footer is separated from the body by a hairline, not by an edge.
pub fn footer_line_over_surface(surface: [u8; 4]) -> [u8; 4] {
    let mix = |c: u8| ((u16::from(c) * 96 + 255 * 4) / 100) as u8;
    [mix(surface[0]), mix(surface[1]), mix(surface[2]), 255]
}

/// One mode button's width: its word plus the design's ten dip padding on each side (`:226`).
///
/// The word's own width is measured by the caller and handed in, because it depends on the glyphs
/// and this module has no device context; `PanelPainter::measure_text_width` is the measurement.
pub fn size_mode_button_width(scale: f32, text_width_px: i32) -> i32 {
    text_width_px + 2 * (SIZE_MODE_BUTTON_PADDING_DIP as f32 * scale).round() as i32
}

/// The reset button, at the end of the size row's header line. `.row-header` is
/// `justify-content: space-between` (`:165`) and `.actions` is its last child (`:588`), so the
/// button is anchored to the column's right edge and grows nothing.
pub fn size_mode_reset_rect(metrics: &FrameMetrics, rects: &FrameRects, scale: f32) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let header = label_rect(metrics, rects, scale, Row::Size);
    let side = px(SIZE_MODE_RESET_DIP);
    let top = header.top + (header.bottom - header.top - side) / 2;
    Rect {
        left: header.right - side,
        top,
        right: header.right,
        bottom: top + side,
    }
}

/// The icon's box inside the reset button: fourteen dip, centred (`:232`).
pub fn size_mode_reset_icon_rect(metrics: &FrameMetrics, rects: &FrameRects, scale: f32) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let button = size_mode_reset_rect(metrics, rects, scale);
    let side = px(SIZE_MODE_RESET_ICON_DIP);
    let left = button.left + (button.right - button.left - side) / 2;
    let top = button.top + (button.bottom - button.top - side) / 2;
    Rect {
        left,
        top,
        right: left + side,
        bottom: top + side,
    }
}

/// The mode container: its right edge is one gap left of the reset button (`:224`) and its width is
/// the two buttons plus the container's own padding and border (`:225`). The buttons' widths are
/// measured by the caller, so this stays a pure function of numbers it is handed.
pub fn size_mode_rect(
    metrics: &FrameMetrics,
    rects: &FrameRects,
    scale: f32,
    buttons_width_px: i32,
) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let header = label_rect(metrics, rects, scale, Row::Size);
    let reset = size_mode_reset_rect(metrics, rects, scale);
    let chrome = 2 * (px(SIZE_MODE_PADDING_DIP) + px(SIZE_MODE_BORDER_DIP));
    let width = buttons_width_px + chrome;
    let height = px(SIZE_MODE_BUTTON_HEIGHT_DIP) + chrome;
    let right = reset.left - px(SIZE_MODE_GAP_DIP);
    let top = header.top + (header.bottom - header.top - height) / 2;
    Rect {
        left: right - width,
        top,
        right,
        bottom: top + height,
    }
}

/// One of the two buttons inside the container. `widths` are the measured widths of the two words,
/// so the second starts where the first ends; both sit inside the border and padding.
pub fn size_mode_button_rect(
    metrics: &FrameMetrics,
    rects: &FrameRects,
    scale: f32,
    widths: [i32; 2],
    index: usize,
) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let container = size_mode_rect(metrics, rects, scale, widths[0] + widths[1]);
    let inset = px(SIZE_MODE_PADDING_DIP + SIZE_MODE_BORDER_DIP);
    let left = container.left + inset + if index == 0 { 0 } else { widths[0] };
    Rect {
        left,
        top: container.top + inset,
        right: left + widths[index],
        bottom: container.bottom - inset,
    }
}

pub const QUALITY_SEG_RADIUS_DIP: f32 = 7.0;

/// `--r-seg-inner: 4px` (`:58`) — the selected block's corner, and the options' own.
pub const QUALITY_SEG_INNER_RADIUS_DIP: f32 = 4.0;

/// `.seg { width: 108px }` (`:263`) — the whole track.
pub const QUALITY_SEG_WIDTH_DIP: f32 = 108.0;

/// `.seg.quality { --seg-inset: 3px }` (`:264`) — the track's padding, which the selected block is
/// inset by on all four sides.
pub const QUALITY_SEG_INSET_DIP: f32 = 3.0;

/// `.seg.quality { --seg-w: 34px }` (`:264`) — one option's width, and therefore how far the block
/// slides per step. Three options and the two insets make up the track exactly, which is asserted
/// rather than assumed.
pub const QUALITY_OPTION_WIDTH_DIP: f32 = 34.0;

/// The options the design offers (`:641`–`:643`). The count is geometry; the words are below.
pub const QUALITY_OPTIONS: usize = 3;

/// The three option words, verbatim from the design: `高`, `中`, `低` (`:641`, `:642`, `:643`), in
/// the order the block slides through them.
pub const QUALITY_OPTION_WORDS: [&str; QUALITY_OPTIONS] = ["高", "中", "低"];

/// The two words inside a size cell, verbatim from the design: `宽` (`:595`) and `高` (`:605`), in
/// the order the two cells appear.
pub const SIZE_CELL_WORDS: [&str; 2] = ["宽", "高"];

/// The unit drawn after each number: `.dim-unit` (`:244`) reads `px`.
pub const SIZE_UNIT_TEXT: &str = "px";

/// Every string the panel's form draws. The font subset's list calls this rather than copying the
/// words, so a label added here cannot be drawn without the subset being rebuilt for it.
pub fn form_drawn_strings() -> Vec<&'static str> {
    let mut all: Vec<&'static str> = FORM_ROWS.iter().map(|row| row.label()).collect();
    all.extend(QUALITY_OPTION_WORDS);
    all.extend(SIZE_MODE_WORDS);
    all.extend(SIZE_CELL_WORDS);
    all.push(SIZE_UNIT_TEXT);
    all
}

/// The design's `--row: 28px` (`:47`), the height of a row's content when that content is not a text
/// field. It is named for the design's own variable rather than for one of its users, because two
/// rows share it: the segmented control is `.seg { height: var(--row) }` (`:263`) and the estimate
/// is `.est-size { height: var(--row) }` (`:269`). It is **not** the text field's 32 (`:167`), and a
/// row using it must say so in `Row::content_height`.
pub const ROW_DIP: i32 = 28;

/// The segmented control's track. `.quality-line` is a row with `justify-content: space-between`
/// (`:306`), so the control sits at the end of the form column, and its height is the row's content
/// box, which for this row is `--row`.
pub fn quality_seg_rect(metrics: &FrameMetrics, rects: &FrameRects, scale: f32) -> Rect {
    let px = |dip: f32| (dip * scale).round() as i32;
    let row = field_rect(metrics, rects, scale, Row::Quality);
    let width = px(QUALITY_SEG_WIDTH_DIP);
    Rect {
        left: row.right - width,
        top: row.top,
        right: row.right,
        bottom: row.bottom,
    }
}

/// One option inside the track: the rect it occupies, and — when it is the chosen one — the rect
/// the block is drawn at. The design uses one geometry for both: `.opt { width: var(--seg-w) }`
/// inside the track's `padding: var(--seg-inset)` (`:266`), and `.block { left/top/bottom:
/// var(--seg-inset); width: var(--seg-w); transform: translateX(index * var(--seg-w)) }` (`:265`),
/// which lands on the same rect.
pub fn quality_option_rect(
    metrics: &FrameMetrics,
    rects: &FrameRects,
    scale: f32,
    index: usize,
) -> Rect {
    let px = |dip: f32| (dip * scale).round() as i32;
    let track = quality_seg_rect(metrics, rects, scale);
    let inset = px(QUALITY_SEG_INSET_DIP);
    let width = px(QUALITY_OPTION_WIDTH_DIP);
    let left = track.left + inset + width * index as i32;
    Rect {
        left,
        top: track.top + inset,
        right: left + width,
        bottom: track.bottom - inset,
    }
}

/// The quality control's interaction state, built from the **same rectangles the painter draws**.
///
/// Hit testing and drawing therefore cannot drift apart: both ask this one geometry, so a click
/// lands on the option whose block the user can see. The state machine that owns `selected`,
/// `pressed` and `hovered` is `segmented`'s, not a second one written here.
pub fn quality_segment_state(
    metrics: &FrameMetrics,
    rects: &FrameRects,
    scale: f32,
    chosen: usize,
) -> SegmentState {
    SegmentState {
        track: quality_seg_rect(metrics, rects, scale),
        segments: (0..QUALITY_OPTIONS)
            .map(|index| Segment {
                rect: quality_option_rect(metrics, rects, scale, index),
                selected: index == chosen,
                disabled: false,
            })
            .collect(),
        hovered: None,
        pressed: None,
        focused: false,
        slide: None,
    }
}

/// Which option the state marks as chosen. The selection lives in the state's `selected` flags —
/// written only by `pointer_event` — so this is a read of that one source rather than a second copy
/// of the answer kept beside it.
pub fn chosen_quality(state: &SegmentState) -> usize {
    state
        .segments
        .iter()
        .position(|segment| segment.selected)
        .unwrap_or(0)
}

/// Feed one pointer event to the quality control and report the newly chosen option, if the event
/// changed it. A press that is released on a different option changes nothing, which is what the
/// press-and-release pairing in `segmented` exists to express.
pub fn quality_pointer(state: &mut SegmentState, event: PointerEvent, point: Point) -> Option<usize> {
    let before = chosen_quality(state);
    let activated = state.pointer_event(event, point)?;
    (chosen_quality(state) != before).then_some(activated)
}

/// How many fields take part in tab navigation. The real window keeps this number true by creating
/// exactly that many `WS_TABSTOP` children; the test pins it so adding a field without adding it
/// here fails rather than silently changing the tab order.
///
/// Deliberately a literal rather than `FieldId::ALL.len()`: deriving it would make the guard a
/// tautology, and the point of the number is that a fifth field has to be declared on purpose.
pub const FORM_FIELD_TAB_STOPS: usize = 4;

// -------------------------------------------------------------------------------------------------
// The footer: `:274`-`:288` and `:674`-`:687`
// -------------------------------------------------------------------------------------------------

/// `.foot { padding: 12px 16px 12px; gap: 8px }` (`:274`). The vertical padding is not repeated
/// here: it is part of `FrameMetrics::footer_height`, which is what the layout measures.
pub const FOOTER_PAD_X_DIP: i32 = 16;
pub const FOOTER_GAP_DIP: i32 = 8;

/// `--btn-h: 32px` (`:50`) and `--r-btn: 6px` (`:59`).
pub const FOOTER_BUTTON_HEIGHT_DIP: i32 = 32;
pub const FOOTER_BUTTON_RADIUS_DIP: i32 = 6;

/// `.btn { padding: 0 16px }` (`:279`), `.btn.start { padding: 0 24px }` (`:282`) and
/// `.btn.copy { padding: 0 12px; gap: 6px }` (`:286`).
pub const FOOTER_BUTTON_PAD_DIP: i32 = 16;
pub const FOOTER_SAVE_PAD_DIP: i32 = 24;
pub const FOOTER_COPY_PAD_DIP: i32 = 12;
pub const FOOTER_COPY_GAP_DIP: i32 = 6;

/// `.btn.copy svg { width: 13px; height: 13px }` (`:287`).
pub const FOOTER_COPY_ICON_DIP: i32 = 13;

/// `.foot .sep { width: 1px; height: 16px }` (`:288`), between copy and cancel (`:683`).
pub const FOOTER_SEP_WIDTH_DIP: i32 = 1;
pub const FOOTER_SEP_HEIGHT_DIP: i32 = 16;

/// The three buttons, in the order the mockup writes them (`:679`, `:684`, `:686`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FooterButton {
    Copy,
    Cancel,
    Save,
}

/// Right to left, which is also the order they are laid out in: the status text takes whatever is
/// left (`:275`, `flex: 1`), so everything else is anchored to the right edge.
pub const FOOTER_BUTTONS: [FooterButton; 3] = [
    FooterButton::Save,
    FooterButton::Cancel,
    FooterButton::Copy,
];

/// The word on a footer button, once.
///
/// Both the caller that measures the buttons and the code that paints them need this, and a width is
/// only right if the word it was measured from is the word that gets drawn. Keeping the pairs in one
/// place is what makes those the same word rather than two lists that agree today.
pub fn footer_button_text(button: FooterButton) -> &'static str {
    match button {
        FooterButton::Copy => "复制",
        FooterButton::Cancel => "取消",
        FooterButton::Save => "保存",
    }
}

/// Whether a footer button draws in the strong format. `.btn.start { font-weight: 600 }` (`:282`) is
/// the only difference between save and the other two, and a width measured at the wrong weight is
/// the wrong width.
pub fn footer_button_is_strong(button: FooterButton) -> bool {
    matches!(button, FooterButton::Save)
}

/// The strip below the body: the panel's last `footer_height` dips.
pub fn footer_rect(metrics: &FrameMetrics, scale: f32) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let top = px(metrics.panel_height() - metrics.footer_height);
    Rect {
        left: 0,
        top,
        right: px(metrics.panel_width),
        bottom: px(metrics.panel_height()),
    }
}

/// The y of the footer's top border: `.foot { border-top: 1px solid rgba(255,255,255,.04) }`
/// (`:274`).
pub fn footer_border_y(metrics: &FrameMetrics, scale: f32) -> i32 {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    px(metrics.panel_height() - metrics.footer_height)
}

/// A button's width from its measured text: padding on both sides, plus the icon and its gap for
/// the copy button (`:286`, `:287`).
///
/// The text is measured at paint time because measuring needs a device context, which is the same
/// split the size row's mode buttons use. Nothing here invents a width.
pub fn footer_button_width(scale: f32, button: FooterButton, text_width_px: i32) -> i32 {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let pad = match button {
        FooterButton::Save => FOOTER_SAVE_PAD_DIP,
        FooterButton::Cancel => FOOTER_BUTTON_PAD_DIP,
        FooterButton::Copy => FOOTER_COPY_PAD_DIP,
    };
    let extra = match button {
        FooterButton::Copy => px(FOOTER_COPY_ICON_DIP) + px(FOOTER_COPY_GAP_DIP),
        _ => 0,
    };
    text_width_px + extra + 2 * px(pad)
}

/// The rectangle of one footer button, given every button's width.
///
/// `widths` is in the order of [`FOOTER_BUTTONS`], so a caller that measures them in one pass does
/// not have to know the layout to hand them over.
pub fn footer_button_rect(
    metrics: &FrameMetrics,
    scale: f32,
    widths: [i32; 3],
    button: FooterButton,
) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let strip = footer_rect(metrics, scale);
    let height = px(FOOTER_BUTTON_HEIGHT_DIP);
    let top = strip.top + (strip.bottom - strip.top - height) / 2;
    let mut right = strip.right - px(FOOTER_PAD_X_DIP);
    for (index, candidate) in FOOTER_BUTTONS.iter().enumerate() {
        let width = widths[index];
        let left = right - width;
        if *candidate == button {
            return Rect {
                left,
                top,
                right,
                bottom: top + height,
            };
        }
        // The separator sits between copy and cancel (`:683`), so it counts as part of the gap.
        right = left - px(FOOTER_GAP_DIP);
        if *candidate == FooterButton::Cancel {
            right -= px(FOOTER_SEP_WIDTH_DIP) + px(FOOTER_GAP_DIP);
        }
    }
    Rect {
        left: right,
        top,
        right,
        bottom: top + height,
    }
}

/// The separator between copy and cancel, centred on the strip's line (`:288`).
pub fn footer_separator_rect(metrics: &FrameMetrics, scale: f32, widths: [i32; 3]) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let cancel = footer_button_rect(metrics, scale, widths, FooterButton::Cancel);
    let width = px(FOOTER_SEP_WIDTH_DIP);
    let height = px(FOOTER_SEP_HEIGHT_DIP);
    let right = cancel.left - px(FOOTER_GAP_DIP);
    let strip = footer_rect(metrics, scale);
    let top = strip.top + (strip.bottom - strip.top - height) / 2;
    Rect {
        left: right - width,
        top,
        right,
        bottom: top + height,
    }
}

/// The copy glyph inside the copy button: 13 dip, at the button's own left padding (`:286`, `:287`).
pub fn footer_copy_icon_rect(metrics: &FrameMetrics, scale: f32, widths: [i32; 3]) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let button = footer_button_rect(metrics, scale, widths, FooterButton::Copy);
    let side = px(FOOTER_COPY_ICON_DIP);
    let left = button.left + px(FOOTER_COPY_PAD_DIP);
    let top = button.top + (button.bottom - button.top - side) / 2;
    Rect {
        left,
        top,
        right: left + side,
        bottom: top + side,
    }
}

/// Where the copy button's word goes: `.btn.copy` is a flex row — icon, then `gap: 6px`, then the
/// word (`:286`) — so the word is not centred in the button the way the other two are.
pub fn footer_copy_text_rect(metrics: &FrameMetrics, scale: f32, widths: [i32; 3]) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let button = footer_button_rect(metrics, scale, widths, FooterButton::Copy);
    let icon = footer_copy_icon_rect(metrics, scale, widths);
    Rect {
        left: icon.right + px(FOOTER_COPY_GAP_DIP),
        top: button.top,
        right: button.right - px(FOOTER_COPY_PAD_DIP),
        bottom: button.bottom,
    }
}

/// Whether a client-area point is inside one footer button. Pure for the same reason the close
/// button's test is: a hit test that lives only inside a message handler is never checked.
pub fn footer_button_contains(
    metrics: &FrameMetrics,
    scale: f32,
    widths: [i32; 3],
    button: FooterButton,
    point: (i32, i32),
) -> bool {
    let rect = footer_button_rect(metrics, scale, widths, button);
    point.0 >= rect.left
        && point.0 < rect.right
        && point.1 >= rect.top
        && point.1 < rect.bottom
}

/// The footer's status line: the strip's left padding, up to one gap before the copy button.
///
/// `.status { flex: 1; min-width: 0 }` (`save-panel-demo-v3.html:275`) is the first child of the
/// footer (`:675`), so it takes whatever the three buttons leave. The conflict link that the mockup
/// puts between them is not drawn yet (`docs/41`), and it is only shown when a name is taken, so the
/// status runs to the copy button for now.
///
/// The band is the buttons' own, because `.foot` is `align-items: center` (`:274`) and the status is
/// a single line of eleven dip text: centring it in the same band is what puts it on their middle
/// line rather than on the strip's.
pub fn footer_status_rect(
    metrics: &FrameMetrics,
    scale: f32,
    widths: [i32; 3],
) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let strip = footer_rect(metrics, scale);
    let buttons = footer_button_rect(metrics, scale, widths, FooterButton::Copy);
    let left = strip.left + px(FOOTER_PAD_X_DIP);
    let right = (buttons.left - px(FOOTER_GAP_DIP)).max(left);
    Rect {
        left,
        right,
        top: buttons.top,
        bottom: buttons.bottom,
    }
}

// ---------------------------------------------------------------------------------------------
// The format control: the button at the right of the name row and the menu it opens.
//
// The design puts it beside the name field (`:189`), eighty dip wide, showing the chosen format in
// the numeric face with a chevron after it (`:173`, `:176`). Opening it draws a menu of three
// options, right-aligned to the button, with a check beside the chosen one (`:180`–`:185`, `:894`).
// ---------------------------------------------------------------------------------------------

/// `--fmt-w: 80px` (`save-panel-demo-v3.html:49`). The format button's width.
pub const FMT_BUTTON_DIP: i32 = 80;

/// `.file-row { display: flex; gap: 8px }` (`:171`) — the space between the name field and the
/// button, which is also what the name field gives up.
pub const FMT_GAP_DIP: i32 = 8;

/// `.fmt-btn`'s horizontal padding is `0 8px 0 10px` (`:173`): the label starts ten dip in and the
/// chevron ends eight dip before the right edge.
pub const FMT_PAD_LEFT_DIP: i32 = 10;
pub const FMT_PAD_RIGHT_DIP: i32 = 8;

/// `.fmt-btn { gap: 6px }` (`:173`) — between the label and the chevron, when `space-between` does
/// not already separate them.
pub const FMT_LABEL_GAP_DIP: i32 = 6;

/// `.fmt-btn .chev { width: 12px; height: 12px }` (`:176`).
pub const FMT_CHEV_DIP: i32 = 12;

/// `--r-input: 6px` (`:54`), which `.fmt-btn` shares with the fields.
pub const FMT_RADIUS_DIP: i32 = 6;

/// `box-shadow: inset 0 0 0 1px var(--accent)` (`:175`): the open button's ring sits one dip inside
/// its border, not the one and a half the fields use for their focus ring.
pub const FMT_RING_DIP: i32 = 1;

/// `.fmt-menu { width: 156px; padding: 5px }` (`:180`) and `--r-menu: 10px` (`:55`).
pub const FMT_MENU_DIP: i32 = 156;
pub const FMT_MENU_PAD_DIP: i32 = 5;
pub const FMT_MENU_RADIUS_DIP: i32 = 10;

/// `.fmt-menu`'s inset line: `inset 0 0 0 1px var(--line)` (`:246`'s sibling at `:180`).
pub const FMT_MENU_LINE_DIP: i32 = 1;

/// `.fmt-item { height: 30px }`, `--r-item: 6px`, `padding: 0 10px 0 26px` (`:182`, `:56`).
pub const FMT_ITEM_DIP: i32 = 30;
pub const FMT_ITEM_RADIUS_DIP: i32 = 6;
pub const FMT_ITEM_PAD_LEFT_DIP: i32 = 26;
pub const FMT_ITEM_PAD_RIGHT_DIP: i32 = 10;

/// `.fmt-item .check { left: 7px; width: 14px; height: 14px }` (`:184`).
pub const FMT_CHECK_LEFT_DIP: i32 = 7;
pub const FMT_CHECK_DIP: i32 = 14;

/// `placeFmtMenu` (`:901`): the menu is right-aligned to the button and never further left than six
/// dip from the panel's edge; it opens six dip below the button and flips above it when that would
/// take it past the window's own bottom (`:902`).
pub const FMT_MENU_MIN_LEFT_DIP: i32 = 6;
pub const FMT_MENU_OFFSET_DIP: i32 = 6;

/// `.fmt-menu`'s own height: its two five dip paddings and three thirty dip items (`:180`, `:182`).
/// The mockup's flip test spells out the same `+ 100` (`:902`).
pub fn fmt_menu_height_dip() -> i32 {
    FMT_MENU_PAD_DIP * 2 + FMT_ITEM_DIP * FMT_OPTIONS.len() as i32
}

/// `const FORMATS = ['.PNG', '.JPG', '.WebP']` (`:717`). The dots and the capitals are the design's,
/// and the words are drawn in the numeric face (`:173`, `:182`), so their characters belong to that
/// subset rather than to the panel's.
pub const FMT_OPTIONS: [&str; 3] = [".PNG", ".JPG", ".WebP"];

/// The button's rect: the name row's band, less the gap, at its right end (`:171`, `:561`).
pub fn fmt_button_rect(metrics: &FrameMetrics, rects: &FrameRects, scale: f32) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let band = field_rect(metrics, rects, scale, Row::Name);
    let right = rects.form.right;
    Rect {
        left: right - px(FMT_BUTTON_DIP),
        top: band.top,
        right,
        bottom: band.top + px(metrics.field_height),
    }
}

/// The name field's rect inside a row band: everything left of the gap and the button (`:555`).
fn fmt_name_field_rect(
    metrics: &FrameMetrics,
    rects: &FrameRects,
    scale: f32,
    band: Rect,
) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let button = fmt_button_rect(metrics, rects, scale);
    Rect {
        left: band.left,
        top: band.top,
        right: button.left - px(FMT_GAP_DIP),
        bottom: band.top + px(metrics.field_height),
    }
}

/// The button's label, which starts at its left padding and stops before the chevron (`:173`).
pub fn fmt_label_rect(metrics: &FrameMetrics, rects: &FrameRects, scale: f32) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let button = fmt_button_rect(metrics, rects, scale);
    let chev = fmt_chev_rect(metrics, rects, scale);
    Rect {
        left: button.left + px(FMT_PAD_LEFT_DIP),
        top: button.top,
        right: chev.left - px(FMT_LABEL_GAP_DIP),
        bottom: button.bottom,
    }
}

/// The button's chevron, at the right end of its padding (`:173`, `:176`).
pub fn fmt_chev_rect(metrics: &FrameMetrics, rects: &FrameRects, scale: f32) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let button = fmt_button_rect(metrics, rects, scale);
    let side = px(FMT_CHEV_DIP);
    let right = button.right - px(FMT_PAD_RIGHT_DIP);
    let top = button.top + (button.bottom - button.top - side) / 2;
    Rect {
        left: right - side,
        top,
        right,
        bottom: top + side,
    }
}

/// The menu's rect, right-aligned to the button (`:901`) and flipped above it when it would run past
/// `window_height` (`:902`).
///
/// The window's height is an argument rather than a constant because the mockup asks the window
/// (`window.innerHeight`), and this function has to be answerable without one.
pub fn fmt_menu_rect(
    metrics: &FrameMetrics,
    rects: &FrameRects,
    scale: f32,
    window_height: i32,
) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let button = fmt_button_rect(metrics, rects, scale);
    let width = px(FMT_MENU_DIP);
    // Each part is rounded and then added, rather than rounding the total: the menu's height is its
    // padding and its items, and at 125% rounding the hundred dip sum gives a menu one pixel shorter
    // than the three items inside it. The same rule as the footer's widths.
    let height = px(FMT_MENU_PAD_DIP) * 2 + px(FMT_ITEM_DIP) * FMT_OPTIONS.len() as i32;
    let left = (button.right - width).max(px(FMT_MENU_MIN_LEFT_DIP));
    let offset = px(FMT_MENU_OFFSET_DIP);
    let top = if button.bottom + offset + height > window_height {
        button.top - height - offset
    } else {
        button.bottom + offset
    };
    Rect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// One option's rect inside the menu (`:182`). The rows are the menu's padding plus whole item
/// heights, so the menu's own height is the last item's bottom plus the second padding.
pub fn fmt_item_rect(
    metrics: &FrameMetrics,
    rects: &FrameRects,
    scale: f32,
    window_height: i32,
    index: usize,
) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let menu = fmt_menu_rect(metrics, rects, scale, window_height);
    let top = menu.top + px(FMT_MENU_PAD_DIP) + px(FMT_ITEM_DIP) * index as i32;
    Rect {
        left: menu.left + px(FMT_MENU_PAD_DIP),
        top,
        right: menu.right - px(FMT_MENU_PAD_DIP),
        bottom: top + px(FMT_ITEM_DIP),
    }
}

/// The check beside the chosen option: `left: 7px`, fourteen dip square, centred (`:184`).
pub fn fmt_check_rect(
    metrics: &FrameMetrics,
    rects: &FrameRects,
    scale: f32,
    window_height: i32,
    index: usize,
) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let item = fmt_item_rect(metrics, rects, scale, window_height, index);
    let side = px(FMT_CHECK_DIP);
    let left = item.left + px(FMT_CHECK_LEFT_DIP);
    let top = item.top + (item.bottom - item.top - side) / 2;
    Rect {
        left,
        top,
        right: left + side,
        bottom: top + side,
    }
}

/// An option's word: after the twenty six dip left padding that leaves room for the check (`:182`).
pub fn fmt_item_label_rect(
    metrics: &FrameMetrics,
    rects: &FrameRects,
    scale: f32,
    window_height: i32,
    index: usize,
) -> Rect {
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let item = fmt_item_rect(metrics, rects, scale, window_height, index);
    Rect {
        left: item.left + px(FMT_ITEM_PAD_LEFT_DIP),
        top: item.top,
        right: item.right - px(FMT_ITEM_PAD_RIGHT_DIP),
        bottom: item.bottom,
    }
}

fn fmt_contains(rect: Rect, point: (i32, i32)) -> bool {
    point.0 >= rect.left && point.0 < rect.right && point.1 >= rect.top && point.1 < rect.bottom
}

/// Whether a point is on the format button. Pure, so a click can be answered from a test.
pub fn fmt_button_contains(
    metrics: &FrameMetrics,
    rects: &FrameRects,
    scale: f32,
    point: (i32, i32),
) -> bool {
    fmt_contains(fmt_button_rect(metrics, rects, scale), point)
}

/// Which option a point lands on, if the menu is open. Closed, nothing is hit — the menu is not on
/// screen, and a click that passed through where it would be must not choose a format.
pub fn fmt_item_at(
    metrics: &FrameMetrics,
    rects: &FrameRects,
    scale: f32,
    window_height: i32,
    open: bool,
    point: (i32, i32),
) -> Option<usize> {
    if !open {
        return None;
    }
    (0..FMT_OPTIONS.len())
        .find(|index| fmt_contains(fmt_item_rect(metrics, rects, scale, window_height, *index), point))
}

/// Whether a point is anywhere in the open menu, padding included — the region that keeps a click
/// from counting as "outside the menu" and closing it.
pub fn fmt_menu_contains(
    metrics: &FrameMetrics,
    rects: &FrameRects,
    scale: f32,
    window_height: i32,
    point: (i32, i32),
) -> bool {
    fmt_contains(fmt_menu_rect(metrics, rects, scale, window_height), point)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SURFACE: [u8; 4] = [23, 23, 28, 235];

    /// `--warning` (`:23`) is one value for the border (`:334`), the status line (`:327`) and the
    /// conflict link (`:328`). The border used to carry `255,191,0`, a yellow the mockup does not
    /// contain, so the same message appeared in two colours depending on where you read it.
    #[test]
    fn the_field_warning_is_the_designs_warning() {
        let border = field_colours(FieldState::Warning, SURFACE).border;
        assert_eq!(border, [240, 179, 90, 255], "warning border is --warning (:334, :23)");
        assert_eq!(
            crate::panel::frame::DEMO_PALETTE.warning,
            border,
            "the status line and the field border must be the same yellow (:327, :334)"
        );
        assert_eq!(
            border, WARNING_RGBA,
            "the border reads the one constant, so the two cannot drift apart again"
        );
    }

    /// Every one of the eleven keys names its field, and the level decides how loudly it says so.
    /// A key with no field would mean the sentence appears nowhere but the status line, which is
    /// right for a whole-attempt failure and wrong for a field the user can fix.
    #[test]
    fn every_validation_key_names_the_field_it_is_about() {
        use crate::copy::{Level, ValidationCopy};
        for copy in ValidationCopy::ALL {
            let (field, state) = problem_of(copy).expect("every key names a field");
            let expected_field = match copy {
                ValidationCopy::StemEmpty
                | ValidationCopy::StemIllegalChars
                | ValidationCopy::StemReservedName
                | ValidationCopy::StemTrailingDotOrSpace => FieldId::Name,
                ValidationCopy::PathEmpty
                | ValidationCopy::PathNotAbsolute
                | ValidationCopy::PathIllegalChars
                | ValidationCopy::NoWritePermission
                | ValidationCopy::NotEnoughSpace
                | ValidationCopy::DirectoryMissing
                | ValidationCopy::PathTooLong => FieldId::Path,
            };
            assert_eq!(field, expected_field, "{copy:?} belongs to {expected_field:?}");
            let expected_state = match copy.level() {
                Level::Warn => FieldState::Warning,
                Level::Error => FieldState::Error,
            };
            assert_eq!(state, expected_state, "{copy:?} keeps its level");
        }
    }

    /// The rule that keeps a message readable: focus changes the cursor, not the verdict.
    #[test]
    fn a_message_survives_focus_until_the_next_attempt() {
        let mut states = FieldStates::IDLE;
        states.mark(problem_of(crate::copy::ValidationCopy::StemEmpty));
        assert_eq!(states.state_of(FieldId::Name), FieldState::Error);

        // Clicking into the field must not turn the red border into the focus accent.
        states.set_focus(FieldId::Name, true);
        assert_eq!(
            states.state_of(FieldId::Name),
            FieldState::Error,
            "focus must not overwrite a message about the value (:333 over :169)"
        );

        // Another field is unaffected by both.
        states.set_focus(FieldId::Path, true);
        assert_eq!(states.state_of(FieldId::Path), FieldState::Focused);

        // A new attempt clears the message and nothing else.
        states.clear_messages();
        assert_eq!(states.state_of(FieldId::Name), FieldState::Idle);
        assert_eq!(
            states.state_of(FieldId::Path),
            FieldState::Focused,
            "clearing messages leaves focus alone"
        );
    }

    /// The focus path still works on a field with nothing wrong with it.
    #[test]
    fn focus_still_paints_a_clean_field() {
        let mut states = FieldStates::IDLE;
        states.set_focus(FieldId::Name, true);
        assert_eq!(states.state_of(FieldId::Name), FieldState::Focused);
        states.set_focus(FieldId::Name, false);
        assert_eq!(states.state_of(FieldId::Name), FieldState::Idle);
    }

    /// The four rules quoted at the top of the file, tested without a device.
    #[test]
    fn focus_changes_the_border_and_the_ring_and_nothing_else() {
        let idle = field_colours(FieldState::Idle, SURFACE);
        let focused = field_colours(FieldState::Focused, SURFACE);
        let error = field_colours(FieldState::Error, SURFACE);
        let warning = field_colours(FieldState::Warning, SURFACE);

        // :167 idle has a transparent border, so nothing is drawn.
        assert_eq!(idle.border[3], 0, "idle must not paint a border (:167)");
        // :169 focus is the accent border plus a one dip inset ring.
        assert_eq!(focused.border, [31, 117, 219, 255], "focus border is --accent (:169)");
        assert_eq!(focused.inset, Some([31, 117, 219, 255]), "focus adds the inset ring (:169)");
        // :333 error recolours the border and adds nothing else.
        assert_eq!(error.border, [255, 123, 123, 255], "error border is --danger (:333)");
        assert_eq!(error.inset, None, "error is a border only (:333)");

        // The background and text colour do not depend on the state.
        assert_eq!(idle.fill, focused.fill);
        assert_eq!(idle.fill, error.fill);
        assert_eq!(idle.fill, warning.fill);
        assert_eq!(idle.text, focused.text, "--text does not change with state (:24)");
    }

    /// `--fill` is white at eight percent over the surface (:26), and a native edit cannot be
    /// translucent, so the composite is computed here instead.
    #[test]
    fn the_fill_is_white_at_eight_percent_over_the_surface() {
        let fill = field_colours(FieldState::Idle, SURFACE).fill;
        // 23*0.92 + 255*0.08 = 41.56 -> 41; 28*0.92 + 20.4 = 46.16 -> 46.
        assert_eq!(fill, [41, 41, 46, 255], "eight percent white over the surface (:26)");
        assert_eq!(fill[3], 255, "the child cannot be transparent, so the fill is opaque");
    }

    /// The slot is derived from the design's own arithmetic: label row, its gap, then the field.
    #[test]
    fn the_field_sits_on_its_slot() {
        let metrics = crate::panel::DEMO;
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let rects = metrics.rects(scale);
            let field = name_field_rect(&metrics, &rects, scale);
            let px = |dip: i32| (dip as f32 * scale).round() as i32;
            assert_eq!(field.left, rects.form.left, "the field starts at the form column");
            // The name row shares its width with the format button (`:554`–`:561`): the field is the
            // mockup's `.name-wrap` (`flex: 1`), so it stops one gap short of the button rather than
            // at the column's edge. The button and the field together still fill the column.
            let button = fmt_button_rect(&metrics, &rects, scale);
            assert_eq!(
                field.right,
                button.left - px(FMT_GAP_DIP),
                "the name field gives up the gap and the format button's width (:171, :561)"
            );
            assert_eq!(button.right, rects.form.right, "the button ends at the form's right edge");
            assert_eq!(
                field.top,
                rects.form.top
                    + px(metrics.text_row_pitch())
                    + px(metrics.row_label_height + metrics.row_gap),
                "the directory row takes the first slot; below it, label {} plus gap {} then the field (:165, :164)",
                metrics.row_label_height,
                metrics.row_gap
            );
            assert_eq!(field.bottom - field.top, px(metrics.field_height), "field height 32 (:167)");
        }
    }

    /// Four fields, four tab stops. Adding a field without declaring it here fails rather than
    /// silently changing the tab order.
    #[test]
    fn the_form_declares_one_tab_stop_per_field() {
        assert_eq!(
            FORM_FIELD_TAB_STOPS, 4,
            "the path, the name and the two size cells are focusable, and a fifth added without declaring it here fails"
        );
        assert_eq!(
            FieldId::ALL.len(),
            FORM_FIELD_TAB_STOPS,
            "every field takes part in tab navigation, so the two lists cannot disagree"
        );
    }

    /// The window answers a child control's questions by looking its identity up in a table, so the
    /// thing that must hold is that one field's state never answers for another. This is the rule the
    /// message handler used to break by reading a single state for every control.
    #[test]
    fn each_field_answers_from_its_own_state() {
        let states = FieldStates::focused_on(Some(FieldId::Path));
        assert_eq!(
            states.state_of(FieldId::Path),
            FieldState::Focused,
            "the field that was focused carries the accent"
        );
        for other in FieldId::ALL {
            if other == FieldId::Path {
                continue;
            }
            assert_eq!(
                states.state_of(other),
                FieldState::Idle,
                "{other:?} is not the focused field, so its frame draws no border (:167)"
            );
        }

        // And the same in the other direction: focusing the name must not leave the path focused.
        let states = FieldStates::focused_on(Some(FieldId::Name));
        assert_eq!(states.state_of(FieldId::Path), FieldState::Idle);
        assert_eq!(states.state_of(FieldId::Name), FieldState::Focused);

        // No focus at all is every field idle, which is what a freshly opened panel looks like.
        assert_eq!(FieldStates::focused_on(None), FieldStates::IDLE);
    }

    /// The drawing loop asks for every field's state at once. If this ever returned a single state
    /// repeated, the panel would draw one field's border on all of them.
    #[test]
    fn the_drawing_loop_sees_every_field_with_its_own_state() {
        let states = FieldStates::focused_on(Some(FieldId::SizeHeight));
        let seen = states.iter();
        assert_eq!(seen.len(), FieldId::ALL.len(), "every field is drawn");
        for (id, state) in seen {
            let expected = if id == FieldId::SizeHeight {
                FieldState::Focused
            } else {
                FieldState::Idle
            };
            assert_eq!(state, expected, "{id:?} was drawn in the wrong state");
        }
    }

    /// The size row opens the second group, so it sits one group pitch — 12 + 4 — below the name
    /// row rather than the 12 that separates rows inside a group. The design's own note says groups
    /// are 16 apart (`:163`), and without the margin the size, quality and estimate rows all creep
    /// upwards by 4 dips each.
    #[test]
    fn the_size_row_opens_a_group_and_sits_sixteen_below_the_name_row() {
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let rects = metrics().rects(scale);
            let saved = label_rect(&metrics(), &rects, scale, Row::Saved);
            let size = label_rect(&metrics(), &rects, scale, Row::Size);
            let one_row = (metrics().text_row_pitch() as f32 * scale).round() as i32;
            let one_group = (metrics().text_group_pitch() as f32 * scale).round() as i32;
            // The after-save row is a header with no content, so from its top to the next row's top
            // there is its own label height, the row gap and the group margin — 22 + 12 + 4.
            let expected = ((metrics().row_label_height
                + metrics().row_spacing
                + metrics().group_spacing) as f32
                * scale)
                .round() as i32;
            assert_eq!(
                size.top - saved.top,
                expected,
                "the size row opens a group below the after-save row (:163)"
            );
            assert!(
                one_group > one_row,
                "a group pitch has to be larger than a row pitch or the margin is not being added"
            );
        }
    }

    /// The after-save row is a `.row-header` on its own (`:569`–`:577`): no field, and therefore no
    /// `.form-row { gap: 6px }` (`:164`) either. Its whole row is its label.
    #[test]
    fn the_after_save_row_is_a_header_with_no_content() {
        assert!(!Row::Saved.has_content());
        for row in [Row::Path, Row::Name, Row::Size, Row::Quality] {
            assert!(row.has_content(), "{row:?} has a content box");
        }
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let rects = metrics().rects(scale);
            let label = label_rect(&metrics(), &rects, scale, Row::Saved);
            let expected = (metrics().row_label_height as f32 * scale).round() as i32;
            assert_eq!(
                label.bottom - label.top,
                expected,
                "the after-save row is one label row tall"
            );
        }
    }

    /// The row order the design lays out, checked against the row above and below rather than
    /// against absolute numbers, so a row inserted in the wrong place fails here.
    #[test]
    fn the_after_save_row_sits_between_the_name_row_and_the_size_row() {
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let rects = metrics().rects(scale);
            let name = label_rect(&metrics(), &rects, scale, Row::Name);
            let saved = label_rect(&metrics(), &rects, scale, Row::Saved);
            let size = label_rect(&metrics(), &rects, scale, Row::Size);
            assert!(name.top < saved.top, "the after-save row is below the name row (:568)");
            assert!(saved.top < size.top, "the after-save row is above the size row (:579)");
            let one_row = (metrics().text_row_pitch() as f32 * scale).round() as i32;
            assert_eq!(
                saved.top - name.top,
                one_row,
                "the after-save row is inside the first group, so it is one row pitch down"
            );
        }
    }

    /// The quality row stays inside its group, so it is one ordinary row pitch below the size row.
    #[test]
    fn the_quality_row_stays_inside_its_group() {
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let rects = metrics().rects(scale);
            let size = field_rect(&metrics(), &rects, scale, Row::Size);
            let quality = field_rect(&metrics(), &rects, scale, Row::Quality);
            let one_row = (metrics().text_row_pitch() as f32 * scale).round() as i32;
            assert_eq!(
                quality.top - size.top,
                one_row,
                "size and quality share a group, so only the 12 dip row gap separates them (:162)"
            );
        }
    }

    /// The row's content is not always the field height: the quality row's is `--row`, which is 28.
    /// A row that assumed 32 would put everything below it 4 dips too low.
    #[test]
    fn the_quality_row_is_as_tall_as_the_segment_and_not_as_tall_as_a_field() {
        let rects = metrics().rects(1.0);
        let quality = field_rect(&metrics(), &rects, 1.0, Row::Quality);
        assert_eq!(
            quality.bottom - quality.top,
            ROW_DIP,
            "the quality row's content is .quality-line, whose min-height is --row (:306, :47)"
        );
        assert_ne!(
            ROW_DIP, metrics().field_height,
            "if these ever become equal this test stops meaning anything"
        );
    }

    /// The estimate row is the last row of the form and opens the third group. Its label is
    /// `预估大小` (`:667`) and the design marks it `.form-row.grp` (`:666`), so the gap above it is the
    /// group gap rather than the row gap — the same sixteen the size row gets below the name row.
    /// Its content is `.est-size { height: var(--row) }` (`:269`), which is the same twenty eight the
    /// segmented control uses, so the two rows share one constant rather than two that agree.
    #[test]
    fn the_estimate_row_is_last_and_opens_the_third_group() {
        assert_eq!(
            FORM_ROWS.last(),
            Some(&Row::Estimate),
            "the estimate is the last row of the form (:666)"
        );
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let metrics = metrics();
            let rects = metrics.rects(scale);
            let quality = field_rect(&metrics, &rects, scale, Row::Quality);
            let estimate = label_rect(&metrics, &rects, scale, Row::Estimate);
            let gap = ((metrics.row_spacing + metrics.group_spacing) as f32 * scale).round() as i32;
            assert_eq!(
                estimate.top,
                quality.bottom + gap,
                "the estimate row opens a group, so it is 16 below the quality row, not 12 (:163, :666)"
            );
            assert_eq!(
                estimate.bottom - estimate.top,
                (metrics.row_label_height as f32 * scale).round() as i32,
                "every row starts with its label row (:165)"
            );
        }
        let metrics = metrics();
        let rects = metrics.rects(1.0);
        let estimate = field_rect(&metrics, &rects, 1.0, Row::Estimate);
        assert_eq!(
            estimate.bottom - estimate.top,
            ROW_DIP,
            "the estimate's content is .est-size, whose height is --row (:269, :47)"
        );
    }

    /// Three options and the two insets add up to the track exactly. The design states the three
    /// numbers separately (`:263`, `:264`), so the arithmetic that ties them together is checked
    /// rather than trusted: a wrong option width would leave a gap or overhang at the end of the
    /// track, which is the kind of thing a screenshot shows and a colour assertion does not.
    #[test]
    fn three_options_and_two_insets_make_up_the_track() {
        let metrics = metrics();
        let rects = metrics.rects(1.0);
        let track = quality_seg_rect(&metrics, &rects, 1.0);
        assert_eq!(
            track.right - track.left,
            QUALITY_SEG_WIDTH_DIP as i32,
            ".seg has width 108px (:263)"
        );
        assert_eq!(
            (QUALITY_OPTION_WIDTH_DIP * QUALITY_OPTIONS as f32 + 2.0 * QUALITY_SEG_INSET_DIP) as i32,
            QUALITY_SEG_WIDTH_DIP as i32,
            "3 x 34 + 2 x 3 must be the 108 dip track (:263, :264)"
        );
        let first = quality_option_rect(&metrics, &rects, 1.0, 0);
        let last = quality_option_rect(&metrics, &rects, 1.0, QUALITY_OPTIONS - 1);
        assert_eq!(
            first.left,
            track.left + QUALITY_SEG_INSET_DIP as i32,
            "the first option starts one inset inside the track (:264, :266)"
        );
        assert_eq!(
            last.right,
            track.right - QUALITY_SEG_INSET_DIP as i32,
            "the last option ends one inset before the track does, or the arithmetic above is wrong"
        );
    }

    /// The control sits at the end of the form column: `.quality-line` is a space-between row
    /// (`:306`), so its right edge is the column's.
    #[test]
    fn the_quality_segment_ends_at_the_form_column() {
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let rects = metrics().rects(scale);
            let track = quality_seg_rect(&metrics(), &rects, scale);
            assert_eq!(
                track.right, rects.form.right,
                "the segmented control ends at the form column's right edge at {scale} (:306)"
            );
            assert!(track.left > rects.form.left, "the track is narrower than the column");
        }
    }

    /// The local shorthand, so the tests read like the design rather than like `crate::panel::DEMO`.
    fn metrics() -> FrameMetrics {
        crate::panel::DEMO
    }

    /// The centre of one option, which is a point the user can see and therefore aim at.
    fn option_centre(scale: f32, index: usize) -> Point {
        let metrics = metrics();
        let rects = metrics.rects(scale);
        let rect = quality_option_rect(&metrics, &rects, scale, index);
        Point::new(
            (rect.left + rect.right) / 2,
            (rect.top + rect.bottom) / 2,
        )
    }

    /// The hit rectangles are the drawn rectangles, because the state is built from the same
    /// producer. A second geometry here is how a click would land on the option next to the one the
    /// user aimed at, and it would look like a drawing bug rather than a hit testing one.
    #[test]
    fn the_hit_rectangles_are_the_rectangles_that_are_drawn() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let metrics = metrics();
            let rects = metrics.rects(scale);
            let state = quality_segment_state(&metrics, &rects, scale, 0);

            assert_eq!(
                state.track,
                quality_seg_rect(&metrics, &rects, scale),
                "the track is the drawn track"
            );
            assert_eq!(
                state.segments.len(),
                QUALITY_OPTIONS,
                "one hit rectangle per drawn option (:641-:643)"
            );
            for index in 0..QUALITY_OPTIONS {
                assert_eq!(
                    state.segments[index].rect,
                    quality_option_rect(&metrics, &rects, scale, index),
                    "option {index}'s hit rectangle is the one it is drawn at (:265, :266)"
                );
                assert_eq!(
                    state.target_at(option_centre(scale, index)),
                    Some(index),
                    "the centre of option {index} must hit option {index}"
                );
            }
        }
    }

    /// The chosen option is a press and a release on the same option. This is the control's whole
    /// contract, and it is `segmented`'s state machine that enforces it rather than a rule here.
    #[test]
    fn clicking_the_second_option_selects_it() {
        let metrics = metrics();
        let rects = metrics.rects(1.0);
        let mut state = quality_segment_state(&metrics, &rects, 1.0, 0);
        assert_eq!(chosen_quality(&state), 0, "the panel opens on the first option");

        let second = option_centre(1.0, 1);
        assert_eq!(
            quality_pointer(&mut state, PointerEvent::Down, second),
            None,
            "a press chooses nothing on its own"
        );
        assert_eq!(
            quality_pointer(&mut state, PointerEvent::Up, second),
            Some(1),
            "releasing on the same option chooses it"
        );
        assert_eq!(chosen_quality(&state), 1, "the chosen option moved to the second");
        assert!(
            !state.segments[0].selected,
            "and only one option is chosen at a time"
        );
    }

    /// Pressing one option and releasing over another is not a choice, and neither is a press that
    /// never got a release. Both are the state machine's, and both would be lost by a hit test that
    /// answered on the press alone.
    #[test]
    fn a_press_released_elsewhere_chooses_nothing() {
        let metrics = metrics();
        let rects = metrics.rects(1.0);
        let mut state = quality_segment_state(&metrics, &rects, 1.0, 0);

        quality_pointer(&mut state, PointerEvent::Down, option_centre(1.0, 0));
        assert_eq!(
            quality_pointer(&mut state, PointerEvent::Up, option_centre(1.0, 2)),
            None,
            "a release over a different option chooses nothing"
        );
        assert_eq!(chosen_quality(&state), 0, "and the chosen option does not move");

        quality_pointer(&mut state, PointerEvent::Down, option_centre(1.0, 2));
        assert_eq!(
            quality_pointer(&mut state, PointerEvent::Cancel, option_centre(1.0, 2)),
            None,
            "a cancelled press chooses nothing"
        );
        assert_eq!(chosen_quality(&state), 0);
    }

    /// Every option is reachable, not just the two the other tests use. The design has three
    /// (`:641`-`:643`) and the width arithmetic is what decides whether the last one is inside the
    /// track at all.
    ///
    /// Each round starts on a **different** option, because choosing the option that is already
    /// chosen is not a change and the contract above says so: `quality_pointer` reports a new
    /// choice, not a repeated one.
    #[test]
    fn every_option_can_be_chosen_by_clicking_it() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let metrics = metrics();
            let rects = metrics.rects(scale);

            for index in 0..QUALITY_OPTIONS {
                let start = (index + 1) % QUALITY_OPTIONS;
                let mut state = quality_segment_state(&metrics, &rects, scale, start);
                let point = option_centre(scale, index);

                quality_pointer(&mut state, PointerEvent::Down, point);
                assert_eq!(
                    quality_pointer(&mut state, PointerEvent::Up, point),
                    Some(index),
                    "option {index} must be clickable at {scale}"
                );
                assert_eq!(chosen_quality(&state), index);
            }
        }
    }

    // ---------------------------------------------------------------------------------------------
    // The footer (`:274`-`:288`)
    // ---------------------------------------------------------------------------------------------

    /// Four scales, because every number here is a dip that gets rounded.
    const FOOTER_SCALES: [f32; 4] = [1.0, 1.25, 1.5, 2.0];

    /// Widths the way a caller would measure them: `保存` is the widest word, `复制` carries an
    /// icon. The numbers are inputs, not claims — the layout is what these tests are about.
    const FOOTER_WIDTHS: [i32; 3] = [88, 60, 70];

    #[test]
    fn the_footer_sits_under_the_body_and_ends_the_panel() {
        for scale in FOOTER_SCALES {
            let metrics = crate::panel::DEMO;
            let strip = footer_rect(&metrics, scale);
            let px = |dip: i32| (dip as f32 * scale).round() as i32;
            assert_eq!(strip.top, px(metrics.panel_height() - metrics.footer_height));
            assert_eq!(strip.bottom, px(metrics.panel_height()));
            assert_eq!(strip.left, 0);
            assert_eq!(strip.right, px(metrics.panel_width));
            // The body's preview column ends where the footer begins, so nothing overlaps.
            let preview_bottom = px(metrics.header_height + metrics.body_pad_top + metrics.preview_height);
            assert!(
                preview_bottom <= strip.top,
                "the preview must end above the footer at {scale}"
            );
        }
    }

    #[test]
    fn the_footer_buttons_run_right_to_left_in_the_designs_order() {
        for scale in FOOTER_SCALES {
            let metrics = crate::panel::DEMO;
            let widths = FOOTER_WIDTHS;
            let save = footer_button_rect(&metrics, scale, widths, FooterButton::Save);
            let cancel = footer_button_rect(&metrics, scale, widths, FooterButton::Cancel);
            let copy = footer_button_rect(&metrics, scale, widths, FooterButton::Copy);
            let px = |dip: i32| (dip as f32 * scale).round() as i32;

            assert_eq!(
                save.right,
                px(metrics.panel_width) - px(FOOTER_PAD_X_DIP),
                "save is flush with the footer's right padding"
            );
            assert_eq!(
                cancel.right,
                save.left - px(FOOTER_GAP_DIP),
                "cancel sits one gap left of save"
            );
            // Copy is a further gap and the separator away, because `.foot .sep` (`:288`) stands
            // between them (`:683`).
            assert_eq!(
                copy.right,
                cancel.left - px(FOOTER_GAP_DIP) - px(FOOTER_SEP_WIDTH_DIP) - px(FOOTER_GAP_DIP),
                "the separator stands between copy and cancel"
            );
            for rect in [copy, cancel, save] {
                assert_eq!(
                    rect.bottom - rect.top,
                    px(FOOTER_BUTTON_HEIGHT_DIP),
                    "--btn-h (:50)"
                );
            }
            assert!(copy.left < cancel.left && cancel.left < save.left);
        }
    }

    #[test]
    fn the_footer_status_takes_the_space_the_buttons_leave() {
        // `.status { flex: 1; min-width: 0 }` (`:275`) is the footer's first child (`:675`), so it
        // runs from the strip's padding to one gap before the copy button, and never over a button.
        for scale in FOOTER_SCALES {
            let metrics = crate::panel::DEMO;
            let widths = FOOTER_WIDTHS;
            let strip = footer_rect(&metrics, scale);
            let copy = footer_button_rect(&metrics, scale, widths, FooterButton::Copy);
            let status = footer_status_rect(&metrics, scale, widths);
            let px = |dip: i32| (dip as f32 * scale).round() as i32;

            assert_eq!(
                status.left,
                strip.left + px(FOOTER_PAD_X_DIP),
                ".foot has `padding: 12px 16px 12px` (:274)"
            );
            assert_eq!(
                status.right,
                copy.left - px(FOOTER_GAP_DIP),
                "the status stops one gap before copy"
            );
            assert!(
                status.right <= copy.left,
                "the status must not run under a button at {scale}"
            );
            // `.foot { align-items: center }` (:274): the status shares the buttons' band, so its
            // single line sits on their middle line.
            assert_eq!(status.top, copy.top);
            assert_eq!(status.bottom, copy.bottom);
        }
    }

    #[test]
    fn a_footer_status_that_would_be_negative_is_empty_rather_than_backwards() {
        // A caller that has not measured the buttons yet passes zeros, and a very narrow panel
        // would otherwise ask for a rectangle with its right edge left of its left edge. A backwards
        // rectangle is not an empty one: it would fill the whole strip when it reached a painter.
        let metrics = crate::panel::DEMO;
        let scale = 2.0;
        let zero = footer_status_rect(&metrics, scale, [0; 3]);
        assert!(zero.right >= zero.left, "a status is never backwards");
    }

    #[test]
    fn a_footer_click_is_answered_where_the_button_is_drawn() {
        // The window measures the widths once and hands the same numbers to the paint and to the
        // click handling, so the two rectangles are one set of numbers. This asserts that the two
        // functions agree about it: a click may only land on a button whose rectangle is the one
        // on screen, and a disagreement here is a click answered beside the button it hit.
        for scale in FOOTER_SCALES {
            let metrics = crate::panel::DEMO;
            let widths = FOOTER_WIDTHS;
            for button in FOOTER_BUTTONS {
                let rect = footer_button_rect(&metrics, scale, widths, button);
                let inside = [
                    (rect.left, rect.top),
                    (rect.right - 1, rect.top),
                    (rect.left, rect.bottom - 1),
                    (rect.right - 1, rect.bottom - 1),
                    ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2),
                ];
                for point in inside {
                    assert!(
                        footer_button_contains(&metrics, scale, widths, button, point),
                        "{button:?} is drawn at {rect:?}, so {point:?} is on it"
                    );
                }
                let outside = [
                    (rect.left - 1, rect.top),
                    (rect.right, rect.top),
                    (rect.left, rect.bottom),
                ];
                for point in outside {
                    assert!(
                        !footer_button_contains(&metrics, scale, widths, button, point),
                        "{button:?} must not answer outside {rect:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_footer_keeps_its_absolute_numbers() {
        // Relations between rectangles cannot catch a wrong constant when every rectangle comes
        // from the same constants, which is how the size row's own test learned to check both.
        assert_eq!(FOOTER_PAD_X_DIP, 16, ".foot padding, :274");
        assert_eq!(FOOTER_GAP_DIP, 8, ".foot gap, :274");
        assert_eq!(FOOTER_BUTTON_HEIGHT_DIP, 32, "--btn-h, :50");
        assert_eq!(FOOTER_BUTTON_RADIUS_DIP, 6, "--r-btn, :59");
        assert_eq!(FOOTER_BUTTON_PAD_DIP, 16, ".btn padding, :279");
        assert_eq!(FOOTER_SAVE_PAD_DIP, 24, ".btn.start padding, :282");
        assert_eq!(FOOTER_COPY_PAD_DIP, 12, ".btn.copy padding, :286");
        assert_eq!(FOOTER_COPY_ICON_DIP, 13, ".btn.copy svg, :287");
        assert_eq!(FOOTER_SEP_WIDTH_DIP, 1, ".foot .sep width, :288");
        assert_eq!(FOOTER_SEP_HEIGHT_DIP, 16, ".foot .sep height, :288");
        assert_eq!(
            crate::panel::DEMO.footer_height,
            12 + 32 + 12,
            "padding + button + padding, :274, :50"
        );
    }

    #[test]
    fn the_separator_stands_between_copy_and_cancel() {
        for scale in FOOTER_SCALES {
            let metrics = crate::panel::DEMO;
            let widths = FOOTER_WIDTHS;
            let separator = footer_separator_rect(&metrics, scale, widths);
            let copy = footer_button_rect(&metrics, scale, widths, FooterButton::Copy);
            let cancel = footer_button_rect(&metrics, scale, widths, FooterButton::Cancel);
            assert!(separator.left >= copy.right, "the separator follows copy");
            assert!(separator.right <= cancel.left, "the separator precedes cancel");
            assert_eq!(
                separator.right - separator.left,
                (FOOTER_SEP_WIDTH_DIP as f32 * scale).round() as i32
            );
        }
    }

    #[test]
    fn the_copy_glyph_leads_its_word_inside_the_button() {
        for scale in FOOTER_SCALES {
            let metrics = crate::panel::DEMO;
            let widths = FOOTER_WIDTHS;
            let button = footer_button_rect(&metrics, scale, widths, FooterButton::Copy);
            let icon = footer_copy_icon_rect(&metrics, scale, widths);
            let text = footer_copy_text_rect(&metrics, scale, widths);
            let px = |dip: i32| (dip as f32 * scale).round() as i32;
            assert_eq!(icon.left, button.left + px(FOOTER_COPY_PAD_DIP));
            assert_eq!(text.left, icon.right + px(FOOTER_COPY_GAP_DIP));
            assert_eq!(text.right, button.right - px(FOOTER_COPY_PAD_DIP));
            assert!(
                icon.right <= text.left && text.right <= button.right,
                "the glyph and its word must stay inside the button at {scale}"
            );
        }
    }

    #[test]
    fn each_footer_button_answers_only_inside_itself() {
        let metrics = crate::panel::DEMO;
        for scale in FOOTER_SCALES {
            let widths = FOOTER_WIDTHS;
            for button in FOOTER_BUTTONS {
                let rect = footer_button_rect(&metrics, scale, widths, button);
                let centre = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
                assert!(
                    footer_button_contains(&metrics, scale, widths, button, centre),
                    "{button:?} must answer at its own centre"
                );
                let others = [
                    (rect.left - 2, centre.1),
                    (rect.right + 1, centre.1),
                    (centre.0, rect.top - 2),
                    (centre.0, rect.bottom + 1),
                ];
                for point in others {
                    assert!(
                        !footer_button_contains(&metrics, scale, widths, button, point),
                        "{button:?} must not answer at {point:?} (scale {scale})"
                    );
                }
            }
        }
    }

    /// The format button's four numbers, pinned against the design rather than against each other.
    ///
    /// A related test can only show that the rectangles agree with the constants; a wrong constant
    /// agrees with itself. These are the values from `:49`, `:171` and `:173`.
    #[test]
    fn the_format_button_takes_eighty_dip_and_its_gap_from_the_name_row() {
        let metrics = crate::panel::DEMO;
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let px = |dip: i32| (dip as f32 * scale).round() as i32;
            assert_eq!(FMT_BUTTON_DIP, 80, "--fmt-w (:49)");
            assert_eq!(FMT_GAP_DIP, 8, ".file-row gap (:171)");
            assert_eq!(FMT_PAD_LEFT_DIP, 10, ".fmt-btn padding-left (:173)");
            assert_eq!(FMT_PAD_RIGHT_DIP, 8, ".fmt-btn padding-right (:173)");

            let rects = metrics.rects(scale);
            let button = fmt_button_rect(&metrics, &rects, scale);
            assert_eq!(
                button.right - button.left,
                px(80),
                "the button is eighty dip wide (scale {scale})"
            );
            assert_eq!(
                button.bottom - button.top,
                px(metrics.field_height),
                "and as tall as a field, so the row is one band (scale {scale})"
            );
            assert_eq!(button.top, field_rect(&metrics, &rects, scale, Row::Name).top);

            // The chevron sits at the right padding and the label starts at the left one.
            let chev = fmt_chev_rect(&metrics, &rects, scale);
            assert_eq!(chev.right, button.right - px(FMT_PAD_RIGHT_DIP));
            assert_eq!(chev.right - chev.left, px(FMT_CHEV_DIP), ":176");
            assert_eq!(fmt_label_rect(&metrics, &rects, scale).left, button.left + px(10));
        }
    }

    /// The menu is right-aligned to the button and its three items add up to its own height, which
    /// is the number the mockup's flip test uses (`+ 100`, `:902`).
    #[test]
    fn the_format_menu_hangs_from_the_button_and_holds_three_items() {
        let metrics = crate::panel::DEMO;
        let px = |dip: i32| dip as i32;
        assert_eq!(fmt_menu_height_dip(), 100, "two five dip paddings and three 30 dip items (:180, :182)");
        assert_eq!(FMT_OPTIONS, [".PNG", ".JPG", ".WebP"], "逐字 (:717)");

        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let rects = metrics.rects(scale);
            let window_height = (metrics.panel_height() as f32 * scale).round() as i32;
            let button = fmt_button_rect(&metrics, &rects, scale);
            let menu = fmt_menu_rect(&metrics, &rects, scale, window_height);
            assert_eq!(menu.right, button.right, "right-aligned to the button (:901)");
            assert_eq!(
                menu.right - menu.left,
                (FMT_MENU_DIP as f32 * scale).round() as i32,
                "156 dip wide (:180)"
            );
            assert_eq!(
                menu.top,
                button.bottom + (6.0 * scale).round() as i32,
                "six dip below the button (:902)"
            );
            assert_eq!(
                menu.bottom - menu.top,
                (5.0 * scale).round() as i32 * 2
                    + (30.0 * scale).round() as i32 * FMT_OPTIONS.len() as i32,
                "three items and two paddings, each rounded on its own"
            );

            // The items stack without gaps or overlaps, and the last one plus the padding is the
            // menu's own bottom.
            let first = fmt_item_rect(&metrics, &rects, scale, window_height, 0);
            assert_eq!(first.top, menu.top + (5.0 * scale).round() as i32);
            for index in 1..FMT_OPTIONS.len() {
                let previous = fmt_item_rect(&metrics, &rects, scale, window_height, index - 1);
                let item = fmt_item_rect(&metrics, &rects, scale, window_height, index);
                assert_eq!(item.top, previous.bottom, "items touch (index {index})");
            }
            let last = fmt_item_rect(&metrics, &rects, scale, window_height, FMT_OPTIONS.len() - 1);
            assert_eq!(last.bottom + (5.0 * scale).round() as i32, menu.bottom);
            assert_eq!(px(0), 0);
        }
    }

    /// The flip rule: above the button only when it would otherwise leave the window (`:902`). The
    /// window's height is an argument, so both branches are reachable here.
    #[test]
    fn the_format_menu_flips_above_the_button_only_when_it_would_leave_the_window() {
        let metrics = crate::panel::DEMO;
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let rects = metrics.rects(scale);
            let px = |dip: i32| (dip as f32 * scale).round() as i32;
            let button = fmt_button_rect(&metrics, &rects, scale);
            // The same part-by-part height the menu itself uses, so the boundary tested here is the
            // boundary the function tests.
            let height = px(5) * 2 + px(30) * FMT_OPTIONS.len() as i32;
            let offset = px(6);

            // Room below: hangs below, as the panel it is in always has.
            let roomy = button.bottom + offset + height;
            let below = fmt_menu_rect(&metrics, &rects, scale, roomy);
            assert_eq!(below.top, button.bottom + offset, "room below ⇔ below (scale {scale})");

            // One pixel less: it would leave the window, so it flips above.
            let tight = fmt_menu_rect(&metrics, &rects, scale, roomy - 1);
            assert_eq!(
                tight.top,
                button.top - height - offset,
                "no room below ⇔ above, by the same gap (scale {scale})"
            );
            assert_eq!(tight.bottom, tight.top + height);
        }
    }

    /// Closed, the menu answers nothing: a click that passes through where it would be must not
    /// choose a format. Open, each option answers on its own row and nowhere else.
    #[test]
    fn a_closed_format_menu_chooses_nothing_and_an_open_one_answers_per_row() {
        let metrics = crate::panel::DEMO;
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let rects = metrics.rects(scale);
            let window_height = (metrics.panel_height() as f32 * scale).round() as i32;
            let centre = |index: usize| {
                let item = fmt_item_rect(&metrics, &rects, scale, window_height, index);
                ((item.left + item.right) / 2, (item.top + item.bottom) / 2)
            };

            for index in 0..FMT_OPTIONS.len() {
                assert_eq!(
                    fmt_item_at(&metrics, &rects, scale, window_height, false, centre(index)),
                    None,
                    "a closed menu must not answer at all (index {index}, scale {scale})"
                );
                assert_eq!(
                    fmt_item_at(&metrics, &rects, scale, window_height, true, centre(index)),
                    Some(index),
                    "an open menu answers on its own row (index {index}, scale {scale})"
                );
            }

            // The item's own edges: left and top answer, right and bottom do not.
            let item = fmt_item_rect(&metrics, &rects, scale, window_height, 1);
            assert_eq!(
                fmt_item_at(&metrics, &rects, scale, window_height, true, (item.left, item.top)),
                Some(1),
                "half-open on the leading edge"
            );
            assert_eq!(
                fmt_item_at(&metrics, &rects, scale, window_height, true, (item.right, item.top)),
                None,
                "half-open on the trailing edge"
            );

            // The button answers on its own rectangle and not on the menu's, and the menu's padding
            // counts as the menu so that a click there does not close it.
            let button = fmt_button_rect(&metrics, &rects, scale);
            let menu = fmt_menu_rect(&metrics, &rects, scale, window_height);
            assert!(fmt_button_contains(
                &metrics,
                &rects,
                scale,
                ((button.left + button.right) / 2, (button.top + button.bottom) / 2)
            ));
            assert!(!fmt_button_contains(&metrics, &rects, scale, (menu.left, menu.top)));
            assert!(fmt_menu_contains(&metrics, &rects, scale, window_height, (menu.left, menu.top)));
            assert_eq!(
                fmt_item_at(&metrics, &rects, scale, window_height, true, (menu.left, menu.top)),
                None,
                "the padding is the menu but not an option"
            );
        }
    }
}
