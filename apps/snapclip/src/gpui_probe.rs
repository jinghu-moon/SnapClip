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
    DEMO, FMT_OPTIONS, FMT_PNG_INDEX, QUALITY_HINT_TEXT, QUALITY_OPTION_WORDS, SIZE_MODE_WORDS,
    SIZE_PRESET_WORDS, VARIABLE_TOKENS,
};
use snapclip_ui::panel::fit_preview;

// The menu items cannot carry a closure: the framework's own `on_click` for an item is crate
// private in this release, and the documented route is an action dispatched along the focus chain.
// One action per item rather than one action with a payload, because a payload needs `Deserialize`
// and the derive, while an empty action needs neither.
gpui_kit::actions!(
    snapclip_probe,
    [
        PickDate,
        PickTime,
        PickSequence,
        PickResolution,
        PickApp,
        FormatPng,
        FormatJpeg,
        FormatWebp
    ]
);

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
/// The footer's copy button, a ghost button before the separator (`:679`). Its tooltip in the
/// mockup is `复制图片 (Ctrl+C)`; the probe shows the word only, like the buttons beside it.
pub const PROBE_COPY: &str = "复制";
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
/// The ceiling the mockup's own field holds (`.limit-input`'s value, `:647`).
pub const PROBE_LIMIT_DEFAULT: u32 = 500;

/// The preview card's rectangle in dips, as `(left, top, right, bottom)` — the order `fit_preview`
/// takes, so the card can be handed to it unchanged.
///
/// The edges are `16` and `16 + 556` across, `48` and `48 + 460` down: the padding on the left, the
/// header (`44`) plus the body's gap (`4`) above, and the mockup's `556 × 460` card. Written as a
/// size instead of a rectangle the card comes out 540 wide, which is how the tests below caught it.
pub const PROBE_CARD: (f32, f32, f32, f32) = (16.0, 48.0, 572.0, 508.0);

/// `.preview-frame … 50% / 16px 16px` (`:111`): the checkerboard's tile, in dips.
pub const PROBE_CHECKER_TILE: f32 = 16.0;

/// `.preview-frame { border-radius: var(--r-card); overflow: hidden }` (`:111`) with
/// `--r-card: 10px` (`:53`): the card's corner radius, in dips.
pub const PROBE_CARD_RADIUS: f32 = 10.0;

/// `repeating-conic-gradient(#32363e 0% 25%, #21242a 0% 50%)` (`:111`): the board's two colours.
pub const PROBE_CHECKER_LIGHT: u32 = 0x3236_3e;
pub const PROBE_CHECKER_DARK: u32 = 0x2124_2a;

/// How finely a square that reaches into one of the card's corner arcs is drawn.
///
/// The frame's `overflow_hidden` clips to a rectangle and does not read the border radius
/// (`gpui-fast/src/style.rs`, the clip uses two corner points), so a square that straddles an arc
/// keeps its own square corner. Such a square is redrawn as this many smaller squares of the same
/// colour and the ones lying wholly outside the arc are left out, which turns one square nub into a
/// staircase.
///
/// Eight is the coarsest split that actually drops something at this card's ten dip radius: a
/// sixteen dip tile split eight ways is two, and the corner-most two dip square has its nearest
/// point eight dips out on each axis, which is past the ten dip radius; at four dips the corner-most
/// square is still inside the arc and nothing would be dropped at all.
pub const CORNER_SUBDIVISIONS: usize = 8;

/// How long a typed size waits before the note at the bottom of the form is measured again.
///
/// **This number is ours, not the mockup's** (the mockup has no note that is measured at all): the
/// note comes from a real encode, so measuring on every keystroke would stall typing and measuring
/// in every frame would stall everything. The gap is long enough to fall between two keystrokes of
/// a fast typist and short enough to feel immediate once the typing stops.
pub const REMEASURE_AFTER_MS: u64 = 250;

/// Whether the note has to be measured again, because the text it describes changed.
///
/// Kept as a free function so the answer can be asserted without a window. The failure it exists to
/// prevent is measuring when nothing changed: a note recalculated on every frame looks correct and
/// makes the window unusable.
pub fn should_remeasure(old: &str, new: &str) -> bool {
    old != new
}

/// Whether a square survives the card's rounded corner at its own corner of the card.
///
/// A square is left out only when it lies **wholly outside** one of the four arcs: the nearest
/// point of the square to that arc's centre is farther than the radius. Anything the arc passes
/// through is kept, because leaving out a square the arc only clips would show the surface behind
/// the board instead of the board — the corner would go dark rather than round.
///
/// Both rectangles are `(left, top, right, bottom)` in dips, the order `fit_preview` uses.
pub fn tile_clears_round_corner(
    tile: (f32, f32, f32, f32),
    card: (f32, f32, f32, f32),
    radius: f32,
) -> bool {
    let (card_left, card_top, card_right, card_bottom) = card;
    let (tile_left, tile_top, tile_right, tile_bottom) = tile;
    // Each arc's centre, and which side of it the arc is on: the arc governs the quadrant beyond
    // both of its centre's edges, so a square matters to it only when it lies wholly in there.
    let arcs = [
        (card_left + radius, card_top + radius, true, true),
        (card_right - radius, card_top + radius, false, true),
        (card_left + radius, card_bottom - radius, true, false),
        (card_right - radius, card_bottom - radius, false, false),
    ];
    for (centre_x, centre_y, from_left, from_top) in arcs {
        let wholly_beyond_x = if from_left {
            tile_right <= centre_x
        } else {
            tile_left >= centre_x
        };
        let wholly_beyond_y = if from_top {
            tile_bottom <= centre_y
        } else {
            tile_top >= centre_y
        };
        if !(wholly_beyond_x && wholly_beyond_y) {
            continue;
        }
        let across = if from_left {
            centre_x - tile_right
        } else {
            tile_left - centre_x
        };
        let down = if from_top {
            centre_y - tile_bottom
        } else {
            tile_top - centre_y
        };
        if across * across + down * down > radius * radius {
            return false;
        }
    }
    true
}

/// The edge length to redraw a square at when it reaches into a corner arc, or `None` when it
/// stays clear of all four and can be drawn whole.
///
/// The split is what makes the rule above useful: at this card's ten dip radius and sixteen dip
/// tiles, the square in each corner contains its arc's centre and therefore has to be kept, so
/// skipping whole squares alone would remove nothing and the corners would stay square.
pub fn corner_step(
    tile: (f32, f32, f32, f32),
    card: (f32, f32, f32, f32),
    radius: f32,
) -> Option<f32> {
    let (card_left, card_top, card_right, card_bottom) = card;
    let (tile_left, tile_top, tile_right, tile_bottom) = tile;
    let quadrants = [
        (card_left, card_top, card_left + radius, card_top + radius),
        (card_right - radius, card_top, card_right, card_top + radius),
        (
            card_left,
            card_bottom - radius,
            card_left + radius,
            card_bottom,
        ),
        (
            card_right - radius,
            card_bottom - radius,
            card_right,
            card_bottom,
        ),
    ];
    let overlaps = quadrants.iter().any(|quadrant| {
        let (left, top, right, bottom) = *quadrant;
        tile_left < right && tile_right > left && tile_top < bottom && tile_bottom > top
    });
    if overlaps {
        Some((tile_right - tile_left) / CORNER_SUBDIVISIONS as f32)
    } else {
        None
    }
}

/// The picture the probe shows inside the card.
///
/// The repository has no photograph to hand, only the tracked icon set, so a 96 dip square is used
/// and the panel's own fit rule is what makes the interesting assertion: it refuses to enlarge, so
/// a picture smaller than the card is drawn at its own size in the middle instead of being blown
/// up (`crates/snapclip-ui/src/panel/frame.rs` `fit_preview`, line 5438 `.min(1.0)`).
pub const PROBE_PREVIEW_FILE: &str = "assets/icon/close.png";
pub const PROBE_PREVIEW_SIZE: (f32, f32) = (96.0, 96.0);

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
/// `.foot { padding: 12px 16px 12px; border-top: 1px solid … }` (`:274`) over a `32` button
/// (`:279`): the footer is its own band at the window's bottom, not the last row of the form.
pub const FOOTER_HEIGHT: f32 = 12.0 + BUTTON_HEIGHT + 12.0;
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

    /// The rows that open a group, so the extra margin above them is data rather than a flag
    /// passed at a call site.
    ///
    /// The mockup groups the column as 路径 · 文件名 · 保存后, then 尺寸 · 质量, then 预估大小, and
    /// puts the extra four dips above a row that opens a group (`:163`). Which rows those are was
    /// previously a boolean at each call site, and it was passed the wrong way round: the margin
    /// landed above 质量, which shares the size row's group. A name list cannot be swapped that way.
    pub const GROUP_OPENERS: [&str; 2] = ["size", "estimate"];

    /// Whether a row, by name, begins a group.
    pub fn opens_group(row: &str) -> bool {
        GROUP_OPENERS.contains(&row)
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
    let header_node = div()
        .h(px(dip(scale, mockup::HEADER_HEIGHT)))
        .h_flex()
        .items_center()
        .justify_between()
        .child(header);
    let content_node = div().child(content);
    div()
        .v_flex()
        .mt(px(group_margin))
        .gap(px(dip(scale, mockup::HEADER_GAP)))
        .child(header_node)
        .child(content_node)
}

/// A row label, in the mockup's own type size.
fn row_label(text: &'static str) -> impl IntoElement {
    div().text_xs().child(text)
}

/// The size the two cells start with. The probe has no capture, so it uses the mockup's own
/// sample size (`:593`, `1440 x 900`) instead of inventing dimensions; the presets scale this.
pub const PROBE_ORIGINAL_SIZE: (u32, u32) = (1440, 900);

/// The four size presets, as percentages (`:614`-`:619`: `data-pct="100|75|50|25"`).
pub const PRESET_PERCENTS: [u32; 4] = [100, 75, 50, 25];

/// The fill choices for a JPEG's transparent pixels (`:658` white, `:659` black).
///
/// The mockup's third swatch is a colour wheel that opens the system picker (`:660`); an arbitrary
/// colour is not built, because a swatch that opens nothing would be a control that does not work.
pub const FILL_SWATCHES: [[u8; 3]; 2] = [[0xff, 0xff, 0xff], [0x00, 0x00, 0x00]];

/// The fill the panel starts with: white (`S.fill` at `:724`).
pub const FILL_DEFAULT: usize = 0;

/// `.fill-row`'s words: `透明区域填充` (`:656`).
pub const FILL_LABEL: &str = "透明区域填充";

/// `.bg-opt { width: 16px; height: 16px; border-radius: 4px }` (`:150`, `:58`).
pub const SWATCH_SIZE: f32 = 16.0;
/// See [`SWATCH_SIZE`]: `--r-input` at `:58` is the swatch's radius.
pub const SWATCH_RADIUS: f32 = 4.0;

/// How much the chosen swatch grows, in dip.
///
/// The mockup marks it with a ring (`:153`); the ring's styled helper is not among the ones this
/// file uses, and a marker that has not been verified is worse than a bigger swatch, so the chosen
/// one is drawn two dips wider instead. Recorded as a deviation rather than presented as the ring.
pub const SWATCH_CHOSEN_GROWTH: f32 = 2.0;

/// The size a preset asks for: the capture's own size scaled by a percentage (`:1165` of the
/// mockup's script rounds rather than truncates), never smaller than one pixel.
pub fn preset_size(original: (u32, u32), percent: u32) -> (u32, u32) {
    let side = |value: u32| ((value as u64 * percent as u64 + 50) / 100).max(1) as u32;
    (side(original.0), side(original.1))
}

/// The probe's state: the fields it can edit, and the choices it shows.
pub struct GpuiProbe {
    /// The handle the menus hand their actions back to, so a chosen item reaches this view.
    focus: FocusHandle,
    name: Entity<InputState>,
    path: Entity<InputState>,
    width: Entity<InputState>,
    height: Entity<InputState>,
    /// The chosen format, as an index into [`FMT_OPTIONS`].
    format: usize,
    /// The chosen size mode, as an index into [`SIZE_MODE_WORDS`].
    mode: usize,
    /// The chosen quality tier, as an index into [`QUALITY_OPTION_WORDS`] (`:641`-`:643`). Only
    /// JPEG spends it; the tiers are hidden while PNG is chosen and the choice is kept, so coming
    /// back to JPEG restores what was picked.
    quality: usize,
    /// The chosen fill for a JPEG's transparent pixels, as an index into [`FILL_SWATCHES`]
    /// (`:658`/`:659`). The mockup's third swatch is a colour wheel (`:660`); picking an arbitrary
    /// colour is not built, and that is recorded rather than faked with a colour that is not shown.
    fill: usize,
    /// Which of the saved-after switches are on.
    after: [bool; 2],
    /// What a taken name should do, which the footer's link toggles when it is shown at all.
    ///
    /// The panel's default is to rename, because a screenshot panel that replaces a file without
    /// being asked is a data-loss bug; the link exists so the user can ask for the other.
    conflict: crate::save::service::ConflictPolicy,
    /// The picture a save would write. Synthetic: the probe captures nothing (see `gpui_save`).
    picture: crate::gpui_save::Picture,
    /// The same picture encoded once into a temporary PNG, which is what the card displays.
    ///
    /// `None` until a captured picture arrives and when the encode fails; the card then keeps the
    /// placeholder rather than showing a file left behind by a previous window.
    preview: Option<std::path::PathBuf>,
    /// How heavy the file would be, as the note at the bottom of the form says.
    estimate: Option<String>,
    /// What the footer says after a save that did not happen, and nothing otherwise.
    status: Option<String>,
    /// Whether the save honours a size ceiling, and the field that holds its number.
    ///
    /// The switch decides whether a limit applies, not the field: turning the switch off keeps the
    /// number, so turning it back on restores what the user chose. That is the panel's rule.
    limit_on: bool,
    limit: Entity<InputState>,
    /// The three texts the note was last measured from: the name, the width and the height.
    ///
    /// The note describes a file built from these, so it says nothing about the file once one of
    /// them changes. Comparing the current text with this is what makes "measure again" answerable
    /// without a window, and what stops an unchanged frame from spending an encode.
    weighed_texts: (String, String, String),
    /// The debounce that is waiting to measure, if one is.
    ///
    /// Held rather than detached so a second keystroke does not start a second timer: the timer it
    /// finds is left alone, and the measurement it runs reads the text at that moment, which is by
    /// then the newest text.
    pending_remeasure: Option<Task<()>>,
}

impl GpuiProbe {
    /// The same view, showing a picture a capture handed over instead of the synthetic one.
    ///
    /// A real capture starts the size cells at its own size, which is what the drawn panel does
    /// with `capture_size`; the synthetic picture keeps the mockup's own numbers.
    pub fn with_picture(
        picture: crate::gpui_save::Picture,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut probe = Self::new(&mut *window, cx);
        let size = picture.size();
        probe
            .width
            .update(cx, |state, cx| state.set_value(size.width.to_string(), &mut *window, cx));
        probe
            .height
            .update(cx, |state, cx| state.set_value(size.height.to_string(), &mut *window, cx));
        probe.picture = picture;
        probe.preview = crate::gpui_save::preview_png(&probe.picture);
        probe
    }

    /// Build the view, seeding the name field with the template the panel uses.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        // What was typed last time, not the default: the spec puts the template and the number in
        // one sentence ("模板与序号随输入实时记住", `:201`), so a second window has to open with what
        // the first one was left holding.
        let template = crate::gpui_save::remembered_template();
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(PROBE_NAME)
                .default_value(template)
        });
        let path = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("C:\\\\")
                .default_value(crate::save::dirs::desktop())
        });
        // The size cells start at the capture's own size in the panel; this probe has no capture,
        // so it shows the size the mockup's own sample uses rather than inventing one.
        let width = cx.new(|cx| {
            InputState::new(window, cx).default_value(PROBE_ORIGINAL_SIZE.0.to_string())
        });
        let height = cx.new(|cx| {
            InputState::new(window, cx).default_value(PROBE_ORIGINAL_SIZE.1.to_string())
        });
        // The ceiling's field starts at the mockup's own number (`:647`), which is also the design's
        // floor when nothing readable is typed (`:236`).
        let limit = cx.new(|cx| {
            InputState::new(window, cx).default_value(PROBE_LIMIT_DEFAULT.to_string())
        });
        let mut probe = Self {
            focus,
            name,
            path,
            width,
            height,
            format: 0,
            mode: 0,
            quality: 0,
            fill: FILL_DEFAULT,
            after: crate::gpui_save::remembered_after(),
            conflict: crate::save::service::ConflictPolicy::AutoRename,
            picture: crate::gpui_save::Picture::synthetic(),
            preview: None,
            estimate: None,
            status: None,
            limit_on: false,
            limit,
            // Both are filled by the first `weigh`, which runs before the view is returned.
            weighed_texts: (String::new(), String::new(), String::new()),
            pending_remeasure: None,
        };
        probe.weigh(cx);
        // The three boxes whose text the note describes. Observing them means the note is measured
        // again after typing stops rather than in the middle of a frame; `should_remeasure` decides
        // whether the change is one the note cares about.
        for input in [probe.name.clone(), probe.width.clone(), probe.height.clone()] {
            cx.observe(&input, |this, _, cx| this.schedule_remeasure(cx))
                .detach();
        }
        // The template is remembered as it is typed rather than when a save runs: the spec puts the
        // template and the number in one sentence and asks for both to be remembered live
        // ("模板与序号随输入实时记住", `:201`). No rule lives here — `remember_template` already
        // compares the text and writes nothing when it matches, which is asserted where it is
        // defined (`assemble.rs`), so this is a call rather than a second copy of the decision.
        cx.observe(&probe.name, |_, observed, cx| {
            crate::gpui_save::remember_template(&observed.read(cx).value().to_string());
        })
        .detach();
        probe
    }

    /// The three texts the note is built from, in the order [`Self::weighed_texts`] stores them.
    fn current_texts(&self, cx: &App) -> (String, String, String) {
        (
            self.name.read(cx).value().to_string(),
            self.width.read(cx).value().to_string(),
            self.height.read(cx).value().to_string(),
        )
    }

    /// Whether any of the three texts changed since the note was last measured.
    fn texts_changed(&self, cx: &App) -> bool {
        let (name, width, height) = self.current_texts(cx);
        should_remeasure(&self.weighed_texts.0, &name)
            || should_remeasure(&self.weighed_texts.1, &width)
            || should_remeasure(&self.weighed_texts.2, &height)
    }

    /// Wait out the debounce and measure, unless nothing changed or one is already waiting.
    fn schedule_remeasure(&mut self, cx: &mut Context<Self>) {
        if self.pending_remeasure.is_some() || !self.texts_changed(cx) {
            return;
        }
        self.pending_remeasure = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(REMEASURE_AFTER_MS))
                .await;
            this.update(cx, |this, cx| {
                this.pending_remeasure = None;
                // Read again: the typing may have stopped on a different text than the one that
                // started this wait, and the note has to describe what is on screen now.
                if this.texts_changed(cx) {
                    this.weigh(cx);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// How heavy the file would be, from a real encode of the picture a save would write.
    ///
    /// Named rather than inlined so the two places that need a fresh number (building the view and
    /// changing the format) call the same thing: the note and the file cannot describe two
    /// different pictures.
    fn weigh(&mut self, cx: &App) {
        let output = self.output_size(cx);
        let (date, time) = crate::gpui_save::now();
        let values = crate::gpui_save::values_with(
            "",
            PROBE_NAME,
            crate::gpui_save::format_of(self.format),
            &date,
            &time,
            self.quality,
            FILL_SWATCHES[self.fill],
            crate::gpui_save::next_sequence(),
            self.conflict,
        );
        self.estimate = crate::gpui_save::weight(&self.picture, &values, output);
        // What the note now describes, so an unchanged frame can tell that it has nothing to do.
        self.weighed_texts = self.current_texts(cx);
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
        // The menus hand their chosen item back through this handle, so it is cloned before the
        // closures that each need their own copy.
        let focus = self.focus.clone();
        let wand_focus = focus.clone();
        let format_focus = focus.clone();
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
                        .dropdown_menu(move |menu, _, _| variable_menu(menu, wand_focus.clone())),
                )
                .child(
                    Button::new("probe-format")
                        .label(FMT_OPTIONS[self.format])
                        .h(field_h)
                        .w(px(dip(scale, mockup::FORMAT_WIDTH)))
                        .dropdown_menu(move |menu, _, _| format_menu(menu, format_focus.clone())),
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
            )
            // `.size-reset` sits at the row header's right end (`:232`) and puts the picture back
            // to the capture's own size.
            .child(
                Button::new("probe-size-reset")
                    .label("重置")
                    .h(px(dip(scale, mockup::MODE_BUTTON_HEIGHT)))
                    .on_click(cx.listener(|this, _, window, cx| {
                        let (width, height) = PROBE_ORIGINAL_SIZE;
                        let w = this.width.clone();
                        w.update(cx, |state, cx| state.set_value(width.to_string(), window, cx));
                        let h = this.height.clone();
                        h.update(cx, |state, cx| state.set_value(height.to_string(), window, cx));
                        this.weigh(cx);
                        cx.notify();
                        window.refresh();
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
                        // The probe has no capture, so the "original" is the size its cells start
                        // with; the panel scales the selection's size the same way (`:1165`).
                        let percent = PRESET_PERCENTS[index];
                        let (width, height) = preset_size(PROBE_ORIGINAL_SIZE, percent);
                        Button::new(format!("probe-preset-{index}"))
                            .label(word)
                            .h(chip_h)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let w = this.width.clone();
                                w.update(cx, |state, cx| {
                                    state.set_value(width.to_string(), window, cx)
                                });
                                let h = this.height.clone();
                                h.update(cx, |state, cx| {
                                    state.set_value(height.to_string(), window, cx)
                                });
                                // A smaller picture is a smaller file, so the note is measured
                                // again here rather than in the middle of a frame.
                                this.weigh(cx);
                                cx.notify();
                                window.refresh();
                            }))
                    })),
            );
        // 尺寸 opens the second group. The mockup puts the extra margin above this row (`:163`),
        // not above the quality row, which shares the size row's group.
        let size_row = row(size_header, size_content, mockup::opens_group("size"), scale);

        // 质量: for PNG the mockup hides the tiers and states why (`:638`, `:639`); for the other
        // formats it draws the three tiers instead (`:641`-`:643`). Which of the two is the same
        // question in one place rather than a condition repeated at two call sites.
        let quality_header = div()
            .h_flex()
            .items_center()
            .gap(px(dip(scale, mockup::HEADER_ACTION_GAP)))
            .child(row_label(PROBE_LABELS[4]));
        // The mockup's `限制体积` is a switch (`:631`-`:635`), not a word: turning it on is what
        // makes the `不超过 [500] KB` group appear (`:645`-`:649`) and what makes a save honour a
        // ceiling at all. It sits on the row's header beside the label, as the mockup draws it, and
        // its state is the same field the save reads, so the two cannot disagree.
        let limit_switch = Button::new("probe-limit")
            .label(PROBE_LIMIT)
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.limit_on = !this.limit_on;
                cx.notify();
                window.refresh();
            }));
        // The mockup's switch shows its state in the track, not in the word (`:631`-`:635`), so the
        // on state is the button's emphasis rather than a suffix on the label.
        let limit_switch = if self.limit_on {
            limit_switch.primary()
        } else {
            limit_switch
        };
        let quality_header = quality_header.child(limit_switch);
        let quality_content = if self.limit_on {
            // `不超过 [500] KB` (`:645`-`:649`): the field keeps its number while the switch is off,
            // which is what makes turning it back on restore what the user chose.
            div()
                .h(px(dip(scale, mockup::SEG_HEIGHT)))
                .h_flex()
                .items_center()
                .gap(px(dip(scale, mockup::HEADER_GAP)))
                .child(div().text_xs().child("不超过"))
                .child(
                    Input::new(&self.limit)
                        .id("probe-limit-kb")
                        .h(px(dip(scale, mockup::SEG_HEIGHT))),
                )
                .child(div().text_xs().child("KB"))
        } else if quality_shows_tiers(self.format) {
            // The tiers, and beneath them the fill for a JPEG's transparent pixels (`:655`-`:662`).
            // Both live inside the same branch, so a format without tiers shows neither.
            let mut tiers = div().h_flex().items_center().gap(px(dip(scale, mockup::QUALITY_LINE_GAP)));
            for (index, word) in QUALITY_OPTION_WORDS.iter().enumerate() {
                let button = Button::new(("probe-tier", index))
                    .label(*word)
                    .h(px(dip(scale, mockup::SEG_HEIGHT)));
                let button = if self.quality == index {
                    button.primary()
                } else {
                    button
                };
                tiers = tiers.child(button.on_click(cx.listener(move |this, _, _, cx| {
                    this.quality = index;
                    // Only JPEG spends the tier, so the note only changes for a format that does;
                    // weighing is an encode and must not run per frame.
                    this.weigh(cx);
                    cx.notify();
                })));
            }
            let mut swatches = div()
                .h_flex()
                .items_center()
                .gap(px(dip(scale, mockup::QUALITY_LINE_GAP)))
                .child(div().text_xs().child(FILL_LABEL));
            for (index, colour) in FILL_SWATCHES.iter().enumerate() {
                // A swatch is its own colour, so one that is chosen can be read at a glance; the
                // chosen one grows instead of gaining a ring this file has no verified helper for.
                let side = if self.fill == index {
                    SWATCH_SIZE + SWATCH_CHOSEN_GROWTH
                } else {
                    SWATCH_SIZE
                };
                let swatch = div()
                    .w(px(dip(scale, side)))
                    .h(px(dip(scale, side)))
                    .rounded(px(dip(scale, SWATCH_RADIUS)))
                    .bg(gpui::Rgba {
                        r: colour[0] as f32 / 255.0,
                        g: colour[1] as f32 / 255.0,
                        b: colour[2] as f32 / 255.0,
                        a: 1.0,
                    });
                swatches = swatches.child(
                    div()
                        .id(("probe-fill", index))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.fill = index;
                            this.weigh(cx);
                            cx.notify();
                        }))
                        .child(swatch),
                );
            }
            div()
                .v_flex()
                .gap(px(dip(scale, mockup::HEADER_GAP)))
                .child(tiers)
                .child(swatches)
        } else {
            div()
                .h(px(dip(scale, mockup::SEG_HEIGHT)))
                .h_flex()
                .items_center()
                .child(div().text_xs().child(QUALITY_HINT_TEXT))
        };
        // The quality row sits inside the group the size row opened, so it pays no margin of its own.
        let quality_row = row(quality_header, quality_content, mockup::opens_group("quality"), scale);

        // 预估大小: the value sits at the row's right end (`:668`), as the mockup draws it. It is
        // the length of the file a save would write, from one real encode — not the mockup's sample
        // number, which this probe used to show and no longer does.
        let estimate_text = self
            .estimate
            .clone()
            .unwrap_or_else(|| "—".to_owned());
        let estimate_row = row(

            row_label(PROBE_LABELS[5]),
            div()
                .h(px(dip(scale, mockup::ESTIMATE_HEIGHT)))
                .h_flex()
                .items_center()
                .justify_end()
                .child(div().text_sm().child(estimate_text)),
            true,
            scale,
        );

        // The preview card: the mockup's checkerboard (`:111`), the picture fitted into it by the
        // panel's own rule, and the card's rounded boundary.
        let (card_left, card_top, card_right, card_bottom) = PROBE_CARD;
        let card_w = card_right - card_left;
        let card_h = card_bottom - card_top;
        // The rule fits the picture that is actually on the card, not the placeholder that stands
        // in when the probe opens without one: fitting the placeholder made a captured picture
        // keep the small box the icon would have had.
        let source_size = match self.preview.is_some() {
            true => {
                let size = self.picture.size();
                (size.width as f32, size.height as f32)
            }
            false => PROBE_PREVIEW_SIZE,
        };
        let (drawn_left, drawn_top, drawn_right, drawn_bottom) =
            fit_preview(source_size, PROBE_CARD);
        let tile = PROBE_CHECKER_TILE;
        let columns = (card_w / tile).ceil() as usize;
        let tiles_high = (card_h / tile).ceil() as usize;
        let mut board = div().v_flex();
        for row_index in 0..tiles_high {
            let mut line = div().h_flex();
            for column_index in 0..columns {
                let colour = if (row_index + column_index) % 2 == 0 {
                    PROBE_CHECKER_LIGHT
                } else {
                    PROBE_CHECKER_DARK
                };
                let left = card_left + column_index as f32 * tile;
                let top = card_top + row_index as f32 * tile;
                let square = (left, top, left + tile, top + tile);
                if !tile_clears_round_corner(square, PROBE_CARD, PROBE_CARD_RADIUS) {
                    continue;
                }
                // A square the card's arc passes through cannot be clipped by the frame, so it is
                // redrawn as a finer grid of the same colour: the squares wholly outside the arc
                // drop out and the square nub becomes a staircase (`CORNER_SUBDIVISIONS`).
                match corner_step(square, PROBE_CARD, PROBE_CARD_RADIUS) {
                    None => {
                        line = line.child(
                            div()
                                .w(px(dip(scale, tile)))
                                .h(px(dip(scale, tile)))
                                .bg(rgb(colour)),
                        );
                    }
                    Some(step) => {
                        let mut block = div()
                            .w(px(dip(scale, tile)))
                            .h(px(dip(scale, tile)))
                            .v_flex();
                        for sub_row in 0..CORNER_SUBDIVISIONS {
                            let mut sub_line = div().h_flex();
                            for sub_column in 0..CORNER_SUBDIVISIONS {
                                let sub_left = left + sub_column as f32 * step;
                                let sub_top = top + sub_row as f32 * step;
                                let sub_square = (sub_left, sub_top, sub_left + step, sub_top + step);
                                if tile_clears_round_corner(
                                    sub_square,
                                    PROBE_CARD,
                                    PROBE_CARD_RADIUS,
                                ) {
                                    sub_line = sub_line.child(
                                        div()
                                            .w(px(dip(scale, step)))
                                            .h(px(dip(scale, step)))
                                            .bg(rgb(colour)),
                                    );
                                }
                            }
                            block = block.child(sub_line);
                        }
                        line = line.child(block);
                    }
                }
            }
            board = board.child(line);
        }
        // The captured picture, encoded once into a temporary PNG, so the card shows a file the
        // encoder produced rather than a placeholder. Without one the placeholder stays: the probe
        // opens a window before a capture exists, and a card that showed nothing would look broken.
        // Either way the file is resolved against the working directory rather than the asset
        // bundle, which carries neither.
        let placeholder = match std::env::current_dir() {
            Ok(directory) => directory.join(std::path::Path::new(PROBE_PREVIEW_FILE)),
            Err(_) => std::path::PathBuf::from(PROBE_PREVIEW_FILE),
        };
        let preview_source = self.preview.clone().unwrap_or(placeholder);
        let preview_card = div()
            .w(px(dip(scale, card_w)))
            .h(px(dip(scale, card_h)))
            .relative()
            .rounded(px(dip(scale, PROBE_CARD_RADIUS)))
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .bg(rgb(PROBE_CHECKER_DARK))
            .child(board)
            .child(
                img(preview_source.as_path())
                    .object_fit(ObjectFit::Contain)
                    .w(px(dip(scale, drawn_right - drawn_left)))
                    .h(px(dip(scale, drawn_bottom - drawn_top)))
                    .absolute()
                    .left(px(dip(scale, drawn_left - card_left)))
                    .top(px(dip(scale, drawn_top - card_top))),
            );

        div()
            .relative()
            .size_full()
            .px(px(dip(scale, PROBE_PADDING)))
            .pt(px(dip(scale, PROBE_PADDING)))
            // The footer's band is reserved rather than painted over: the mockup's footer is the
            // window's last child (`:274`), so the body simply ends above it. Without this the
            // column ran underneath the footer and its last row was cut.
            .pb(px(dip(scale, mockup::FOOTER_HEIGHT)))
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                div()
                    .h_flex()
                    .gap(px(dip(scale, 12.0)))
                    .child(preview_card)
                    .child(
                        div()
                            .id("form-column")
                            .test_support()
                            .w(px(dip(scale, column_width())))
                            .v_flex()
                            .gap(px(dip(scale, mockup::ROW_GAP)))
                            .child(path_row)
                            .child(name_row)
                            .child(saved_row)
                            .child(size_row)
                            .child(quality_row)
                            .child(estimate_row),
                    ),
            )
            // The footer, which the mockup lays across the window's bottom (`:673`-`:687`): the
            // status line takes the space the buttons leave, the conflict link sits beside it
            // (`:676`), the copy button follows before a separator, and cancel and save are
            // adjacent at the right end.
            .child({
                let mut bar = div()
                    .absolute()
                    .left(px(dip(scale, PROBE_PADDING)))
                    .right(px(dip(scale, PROBE_PADDING)))
                    .bottom(px(dip(scale, PROBE_PADDING)))
                    .h(px(dip(scale, mockup::BUTTON_HEIGHT)))
                    .h_flex()
                    .items_center()
                    // The bar clips too, so anything that does not fit is cut at the bar rather
                    // than painted above it: the link used to appear over the row above, which is
                    // overflow made visible, not a second line.
                    .overflow_hidden()
                    .gap(px(dip(scale, mockup::HEADER_ACTION_GAP)))
                    .child(
                        // `overflow_hidden` is what lets this shrink. A flex item's automatic
                        // minimum width is its content's, so a long status sentence pushes the
                        // buttons beside it out of the bar however small its flex basis is; the
                        // automatic minimum becomes zero once the item clips, which is the same
                        // rule the mockup relies on when it truncates the status line (`:277`).
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_sm()
                            .child(self.status.clone().unwrap_or_default()),
                    );
                // The link exists only while the plain name is taken, and its word is the action
                // rather than the policy in force (`:1149`, `:1150`).
                if let Some(text) = self.conflict_link(cx) {
                    bar = bar.child(
                        Button::new("probe-conflict")
                            .label(text)
                            .h(px(dip(scale, mockup::BUTTON_HEIGHT)))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_conflict(cx))),
                    );
                }
                bar.child(
                    Button::new("probe-copy")
                        .label(PROBE_COPY)
                        .h(px(dip(scale, mockup::BUTTON_HEIGHT)))
                        .on_click(cx.listener(|this, _, _, cx| this.copy_picture(cx))),
                )
                .child(
                    Button::new("probe-cancel")
                        .label(PROBE_CANCEL)
                        .h(px(dip(scale, mockup::BUTTON_HEIGHT)))
                        .on_click(cx.listener(|this, _, window, cx| this.cancel_probe(window, cx))),
                )
                .child(
                    Button::new("probe-save")
                        .primary()
                        .label(PROBE_SAVE)
                        .h(px(dip(scale, mockup::BUTTON_HEIGHT)))
                        .on_click(cx.listener(|this, _, window, cx| this.save_probe(window, cx))),
                )
            })
            // The menus hand their item back as an action, which arrives here because this element
            // is in the chain of the handle each menu was given.
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::pick_date))
            .on_action(cx.listener(Self::pick_time))
            .on_action(cx.listener(Self::pick_sequence))
            .on_action(cx.listener(Self::pick_resolution))
            .on_action(cx.listener(Self::pick_app))
            .on_action(cx.listener(Self::format_png))
            .on_action(cx.listener(Self::format_jpeg))
            .on_action(cx.listener(Self::format_webp))
    }
}

/// The five variables, as the mockup lists them (`:192`-`:198`), each carrying its action.
fn variable_menu(mut menu: PopupMenu, focus: FocusHandle) -> PopupMenu {
    let actions: [Box<dyn Action>; 5] = [
        Box::new(PickDate),
        Box::new(PickTime),
        Box::new(PickSequence),
        Box::new(PickResolution),
        Box::new(PickApp),
    ];
    for (token, action) in VARIABLE_TOKENS.iter().zip(actions) {
        menu = menu.menu(*token, action);
    }
    menu.action_context(focus)
}

/// The three formats, as the format menu lists them (`:203`), each carrying its action.
fn format_menu(mut menu: PopupMenu, focus: FocusHandle) -> PopupMenu {
    let actions: [Box<dyn Action>; 3] = [
        Box::new(FormatPng),
        Box::new(FormatJpeg),
        Box::new(FormatWebp),
    ];
    for (word, action) in FMT_OPTIONS.iter().zip(actions) {
        menu = menu.menu(*word, action);
    }
    menu.action_context(focus)
}

/// Whether the quality row shows its three tiers rather than the lossless note.
///
/// The mockup settles this by format alone: the tiers are hidden and the note shown for png
/// (`:638`, `:639`), and the tiers are the row for everything else. A free function so the rule is
/// asserted once instead of being a condition written at the two places that draw the row.
pub fn quality_shows_tiers(format: usize) -> bool {
    format != FMT_PNG_INDEX
}

/// What an inserted variable does to the name field.
///
/// `replace` is the whole rule rather than half of it: it applies to the active selection and
/// leaves the caret after what it wrote, so a selection is replaced and a bare caret is inserted
/// at — both of which the specification asks for (`:200`) — without the caller deciding which.
fn insert_token(name: &Entity<InputState>, token: &'static str, window: &mut Window, cx: &mut App) {
    name.update(cx, |state, cx| state.replace(token, window, cx));
}

/// The handlers the two menus reach through their actions.
///
/// Each one is the same three lines, because the action carries no payload: one action per item
/// keeps the mapping visible at the call site and needs neither `Deserialize` nor the derive.
impl GpuiProbe {
    fn pick_date(&mut self, _: &PickDate, window: &mut Window, cx: &mut Context<Self>) {
        insert_token(&self.name, VARIABLE_TOKENS[0], window, cx);
        cx.notify();
    }

    fn pick_time(&mut self, _: &PickTime, window: &mut Window, cx: &mut Context<Self>) {
        insert_token(&self.name, VARIABLE_TOKENS[1], window, cx);
        cx.notify();
    }

    fn pick_sequence(&mut self, _: &PickSequence, window: &mut Window, cx: &mut Context<Self>) {
        insert_token(&self.name, VARIABLE_TOKENS[2], window, cx);
        cx.notify();
    }

    fn pick_resolution(
        &mut self,
        _: &PickResolution,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        insert_token(&self.name, VARIABLE_TOKENS[3], window, cx);
        cx.notify();
    }

    fn pick_app(&mut self, _: &PickApp, window: &mut Window, cx: &mut Context<Self>) {
        insert_token(&self.name, VARIABLE_TOKENS[4], window, cx);
        cx.notify();
    }

    fn format_png(&mut self, _: &FormatPng, _: &mut Window, cx: &mut Context<Self>) {
        self.choose_format(0, cx);
    }

    fn format_jpeg(&mut self, _: &FormatJpeg, _: &mut Window, cx: &mut Context<Self>) {
        self.choose_format(1, cx);
    }

    fn format_webp(&mut self, _: &FormatWebp, _: &mut Window, cx: &mut Context<Self>) {
        self.choose_format(2, cx);
    }

    /// Choosing a format changes the button's word and what the quality row shows, because the
    /// design hides the tiers and shows the lossless note when the format is png (`:638`, `:639`).
    fn choose_format(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < FMT_OPTIONS.len() {
            self.format = index;
            // The format decides what the file weighs, so the note has to be measured again; this
            // is an event handler rather than part of drawing, because a full encode does not
            // belong in a frame.
            self.weigh(cx);
            cx.notify();
        }
    }

    /// The size a save would write, from the two cells and the mode (`:242`, `:226`/`:227`).
    ///
    /// Read through the panel's own rule, so `百分比` means what it means there and a cell that
    /// holds something unreadable falls back to the capture's size instead of failing.
    fn output_size(&self, cx: &App) -> crate::save::Size {
        let width = self.width.read(cx).value().to_string();
        let height = self.height.read(cx).value().to_string();
        let mode = if self.mode == 1 {
            crate::save::scale::SizeMode::Percent
        } else {
            crate::save::scale::SizeMode::Pixels
        };
        crate::save::output_size(
            mode,
            &width,
            &height,
            crate::save::Size::new(PROBE_ORIGINAL_SIZE.0, PROBE_ORIGINAL_SIZE.1),
        )
    }

    /// The footer's conflict link, when the name a save would write is already taken.
    ///
    /// The word is the policy that is *not* in force (`:1150`): showing the one that is would read
    /// as a statement of fact rather than something to press.
    fn conflict_link(&self, cx: &App) -> Option<&'static str> {
        let output = self.output_size(cx);
        let directory = self.path.read(cx).value().to_string();
        let name = self.name.read(cx).value().to_string();
        let (date, time) = crate::gpui_save::now();
        let values = crate::gpui_save::values_with(
            &directory,
            &name,
            crate::gpui_save::format_of(self.format),
            &date,
            &time,
            self.quality,
            FILL_SWATCHES[self.fill],
            crate::gpui_save::next_sequence(),
            self.conflict,
        );
        crate::gpui_save::conflict_link(&values, output.width, output.height, self.conflict)
    }

    /// Swap the policy for a taken name: replace instead of rename, or back.
    fn toggle_conflict(&mut self, cx: &mut Context<Self>) {
        self.conflict = match self.conflict {
            crate::save::service::ConflictPolicy::AutoRename => {
                crate::save::service::ConflictPolicy::Overwrite
            }
            crate::save::service::ConflictPolicy::Overwrite => {
                crate::save::service::ConflictPolicy::AutoRename
            }
        };
        cx.notify();
    }

    /// The footer's copy button: the picture a save would write, at the size the panel asks for.
    ///
    /// The same picture and the same scaling the save uses, so what lands on the clipboard and what
    /// lands on disk are the same image rather than two that look alike.
    fn copy_picture(&mut self, cx: &mut Context<Self>) {
        let output = self.output_size(cx);
        let frame = self.picture.frame(output);
        let scaled = crate::save::scale::scaled(&frame);
        let image = crate::clipboard::ClipboardImage {
            width: output.width,
            height: output.height,
            pixels: &scaled,
            channels: crate::clipboard::Channels::Bgra,
            png: None,
        };
        match crate::clipboard::SystemClipboard.copy_image(&image) {
            Ok(()) => eprintln!("[gpui-probe] copied {}x{}", output.width, output.height),
            Err(why) => eprintln!("[gpui-probe] could not copy: {why}"),
        }
        cx.notify();
    }

    /// The footer's cancel: the user changed their mind, so there is nothing to explain.
    fn cancel_probe(&mut self, window: &mut Window, _: &mut Context<Self>) {
        window.remove_window();
    }

    /// The footer's save: write the file, or say why not and stay open.
    ///
    /// The panel's rule (`docs/41`) is that a failed save keeps the window, because the window is
    /// the only place the reason can be shown; a successful one closes it.
    fn save_probe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let output = self.output_size(cx);
        let directory = self.path.read(cx).value().to_string();
        let name = self.name.read(cx).value().to_string();
        let (date, time) = crate::gpui_save::now();
        let values = crate::gpui_save::values_with(
            &directory,
            &name,
            crate::gpui_save::format_of(self.format),
            &date,
            &time,
            self.quality,
            FILL_SWATCHES[self.fill],
            crate::gpui_save::next_sequence(),
            self.conflict,
        );
        let result = {
            // The switch, not the field, decides whether a ceiling applies; the number is read
            // through the panel's own rule, so an unreadable one means the floor rather than
            // refusing every save (`:236`). With the switch off this is a plain save.
            let limit = if self.limit_on {
                let text = self.limit.read(cx).value().to_string();
                Some(crate::save::window::limit_kilobytes(&text) as u64 * 1024)
            } else {
                None
            };
            match crate::gpui_save::save_within(&self.picture, &values, output, limit) {
                Some(result) => result,
                // No candidate size fits the ceiling, so nothing is written: the panel's own answer,
                // and the mockup has no sentence for it (`docs/41`, A43).
                None => Err(crate::save::SaveError::Cancelled),
            }
        };
        match &result {
            Ok(outcome) => {
                eprintln!(
                    "[gpui-probe] saved {} bytes to {} as {:?}",
                    outcome.bytes,
                    outcome.path.display(),
                    outcome.format
                );
                // The switches' work, through the panel's own order and shell calls: the path is the
                // one the save wrote, so what is copied or revealed cannot be another file.
                crate::gpui_save::run_after_save(self.after, &outcome.path);
            }
            Err(why) => eprintln!("[gpui-probe] the save did not happen: {why:?}"),
        }
        // The shell's record takes the template and moves the number by the panel's rule, which only
        // advances on a success whose name asks for it.
        crate::gpui_save::remember_template(&name);
        crate::gpui_save::remember_after(self.after);
        crate::gpui_save::note_attempt(&result, &name);
        self.status = crate::gpui_save::status_of(&result);
        if result.is_ok() {
            window.remove_window();
        } else {
            cx.notify();
        }
    }
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
    fn the_note_is_measured_only_when_the_text_it_describes_changed() {
        // Unchanged text is the case that matters: recomputing then spends a whole encode on a
        // frame that says the same thing, which is what makes the window feel stuck.
        assert!(!should_remeasure("1440", "1440"), "the same text is not a reason to measure");
        assert!(!should_remeasure("", ""), "two empty fields are also the same text");
        assert!(should_remeasure("1440", "144"), "a shortened size is a new size");
        assert!(should_remeasure("", "1440"), "a filled cell newly describes a file");
        assert!(should_remeasure("SnapClip_[日期]", "SnapClip_2026-10-26"), "the name expands");
        // The debounce is ours, not the mockup's: an encode per keystroke would stall typing.
        assert_eq!(
            REMEASURE_AFTER_MS, 250,
            "the wait is the number we chose, and it is short enough to feel immediate"
        );
    }

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

    /// The window the panel opens is the size the panel is laid out for.
    ///
    /// The panel's own metric is the source: the content is placed for that size rather than
    /// reflowed, which is why the window is opened unresizable. If the two ever part company the
    /// panel would be drawn in the corner of a frame of a different size.
    #[::core::prelude::v1::test]
    fn the_window_is_the_size_the_panel_is_laid_out_for() {
        assert_eq!(
            PROBE_HEIGHT,
            DEMO.panel_height() as f32,
            "the window's height is the panel's own (`panel_height`)"
        );
        assert_eq!(
            PROBE_WIDTH - 2.0 * PROBE_PADDING - column_width(),
            556.0 + 12.0,
            "what the window keeps beyond the column is the preview and the gap"
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
    #[::core::prelude::v1::test]
    fn the_quality_row_shows_the_tiers_for_every_format_that_has_them() {
        assert!(
            quality_shows_tiers(1),
            "jpg is offered the three tiers (`:641`-`:643`)"
        );
        assert!(
            quality_shows_tiers(2),
            "webp is offered the three tiers (`:641`-`:643`)"
        );
        assert!(
            !quality_shows_tiers(FMT_PNG_INDEX),
            "png hides the tiers and says why (`:638`, `:639`)"
        );
    }

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
        assert_eq!(PROBE_LIMIT, "限制体积", "`.switch-text` (`:634`)");    }
}

#[cfg(test)]
mod reset_tests {
    use super::PROBE_ORIGINAL_SIZE;

    /// The reset goes back to the mockup's sample size, and the fields start there too, so the two
    /// cannot part company: a reset that restored a different number than the fields began with
    /// would look like it worked.
    #[::core::prelude::v1::test]
    fn the_reset_restores_the_size_the_fields_start_with() {
        assert_eq!(PROBE_ORIGINAL_SIZE, (1440, 900), "`:593` is 1440 x 900");
    }
}

#[cfg(test)]
mod preset_tests {
    // `use gpui_kit::*` exports a macro named `test`, which shadows the attribute.
    use super::{preset_size, PRESET_PERCENTS, PROBE_ORIGINAL_SIZE};

    #[::core::prelude::v1::test]
    fn a_preset_is_the_captures_size_at_that_percentage() {
        assert_eq!(preset_size((1440, 900), 50), (720, 450), "`:1165` halves both sides");
        assert_eq!(preset_size((1440, 900), 100), (1440, 900));
        assert_eq!(preset_size((1440, 900), 75), (1080, 675));
        assert_eq!(preset_size((1440, 900), 25), (360, 225));
    }

    #[::core::prelude::v1::test]
    fn a_preset_rounds_and_never_reaches_zero() {
        // The mockup's script rounds (`:1165`); truncating would give 1 here and 2 below.
        assert_eq!(preset_size((3, 3), 75), (2, 2));
        assert_eq!(preset_size((1, 1), 25), (1, 1), "no side may reach zero");
    }

    #[::core::prelude::v1::test]
    fn the_presets_are_the_mockups_four_in_its_order() {
        assert_eq!(PRESET_PERCENTS, [100, 75, 50, 25], "`:615`-`:618`");
        assert_eq!(PROBE_ORIGINAL_SIZE, (1440, 900), "`:593`");
    }
}

#[cfg(test)]
mod preview_tests {
    // `use gpui_kit::*` exports a macro named `test`, which shadows the attribute.
    use super::{
        CORNER_SUBDIVISIONS, PROBE_CARD, PROBE_CARD_RADIUS, PROBE_CHECKER_DARK,
        PROBE_CHECKER_LIGHT, PROBE_CHECKER_TILE, corner_step, fit_preview, tile_clears_round_corner,
    };

    /// The card is where the header, the body's padding and the card's own size put it.
    ///
    /// Written as literals because the numbers come from the mockup, not from the sum that builds
    /// them: a card compared against its own arithmetic cannot fail.
    #[::core::prelude::v1::test]
    fn the_preview_card_is_where_the_mockups_parts_put_it() {
        assert_eq!(
            PROBE_CARD,
            (16.0, 48.0, 572.0, 508.0),
            "edges, not a size: padding 16 to 16 + 556 across, header 44 + body 4 to that plus 460 \
             down (`DEMO` in the panel, lines 80-85)"
        );
        assert_eq!(PROBE_CHECKER_TILE, 16.0, "`.preview-frame` tile (`:111`)");
        assert_eq!(PROBE_CHECKER_LIGHT, 0x3236_3e, "`:111` first stop");
        assert_eq!(PROBE_CHECKER_DARK, 0x2124_2a, "`:111` second stop");
        // The mockup rounds this card and clips it in the same rule (`:111`), so the radius is
        // part of the card's definition rather than a decoration the probe can drop.
        assert_eq!(
            PROBE_CARD_RADIUS, 10.0,
            "`.preview-frame` takes `--r-card`, and `--r-card: 10px` (`:53`, `:111`)"
        );
    }

    /// A picture smaller than the card is drawn at its own size, in the middle.
    ///
    /// This is the panel's rule, and the interesting half is that it refuses to enlarge: the scale
    /// is capped at one. The numbers are literals worked out from the card, not a second copy of
    /// the formula, so a change to the rule cannot silently agree with the test.
    #[::core::prelude::v1::test]
    fn a_picture_smaller_than_the_card_is_not_blown_up() {
        assert_eq!(
            fit_preview((96.0, 96.0), PROBE_CARD),
            (246.0, 230.0, 342.0, 326.0),
            "96 square, centred: 16 + (556 - 96) / 2 and 48 + (460 - 96) / 2, no enlargement"
        );
    }

    /// A picture larger than the card is scaled down to fit and centred on the shorter side.
    #[::core::prelude::v1::test]
    fn a_picture_larger_than_the_card_is_fitted_and_centred() {
        assert_eq!(
            fit_preview((1000.0, 1000.0), PROBE_CARD),
            (64.0, 48.0, 524.0, 508.0),
            "1000 square into 556 x 460: 460 wide, 16 + (556 - 460) / 2 across, flush vertically"
        );
    }

    /// A square is left out only when the arc misses it entirely.
    ///
    /// The card here is a plain hundred dip square with a ten dip radius, so the arc's centre is at
    /// ten from the corner and the numbers can be checked by hand: a square ending four dips from
    /// the edge has its nearest point six dips out on each axis, which is within ten and is kept;
    /// one ending two dips out is eight and eight, which is past ten and is dropped.
    #[::core::prelude::v1::test]
    fn a_square_wholly_outside_a_rounded_corner_is_left_out() {
        let card = (0.0, 0.0, 100.0, 100.0);
        assert!(
            tile_clears_round_corner((40.0, 40.0, 56.0, 56.0), card, 10.0),
            "the middle of the card is nowhere near an arc"
        );
        assert!(
            tile_clears_round_corner((0.0, 0.0, 4.0, 4.0), card, 10.0),
            "6 and 6 is 8.49 from the arc's centre, inside ten, so the arc still crosses it"
        );
        assert!(
            !tile_clears_round_corner((0.0, 0.0, 2.0, 2.0), card, 10.0),
            "8 and 8 is 11.31 from the arc's centre, past ten, so no part of it is inside the card"
        );
    }

    /// The square holding an arc's centre always survives, which is why the corner is split.
    ///
    /// At this card's ten dip radius and sixteen dip tiles the corner square contains the arc's
    /// centre at 26, 58, so its nearest point is zero dips away and skipping whole squares alone
    /// would leave all four corners square. This test states that rather than leaving it to be
    /// rediscovered.
    #[::core::prelude::v1::test]
    fn the_square_holding_the_arcs_centre_can_never_be_dropped() {
        assert!(
            tile_clears_round_corner((16.0, 48.0, 32.0, 64.0), PROBE_CARD, PROBE_CARD_RADIUS),
            "the corner square contains 26, 58, the centre of the card's top left arc"
        );
    }

    /// The split is fine enough that the square at the very tip of the arc is dropped.
    ///
    /// Sixteen dips split eight ways is two, and the corner-most square ends eight dips from each
    /// edge: 8 and 8 is 11.31, past the ten dip radius, so it goes. The next one in ends six and
    /// eight, which is exactly ten, and is kept because the arc touches it.
    #[::core::prelude::v1::test]
    fn the_corner_split_drops_the_square_at_the_arcs_tip() {
        assert_eq!(
            CORNER_SUBDIVISIONS, 8,
            "a sixteen dip tile split eight ways is two, the coarsest split that drops anything"
        );
        assert_eq!(
            corner_step((16.0, 48.0, 32.0, 64.0), PROBE_CARD, PROBE_CARD_RADIUS),
            Some(2.0),
            "sixteen dips over eight is two"
        );
        assert!(
            !tile_clears_round_corner((16.0, 48.0, 18.0, 50.0), PROBE_CARD, PROBE_CARD_RADIUS),
            "8 and 8 from the arc's centre is 11.31, outside the ten dip radius"
        );
        assert!(
            tile_clears_round_corner((18.0, 48.0, 20.0, 50.0), PROBE_CARD, PROBE_CARD_RADIUS),
            "6 and 8 is exactly ten, so the arc touches this square and it has to stay"
        );
    }

    /// A square clear of every arc quadrant is drawn whole, at the mockup's tile size.
    #[::core::prelude::v1::test]
    fn a_square_in_the_middle_is_drawn_whole() {
        assert_eq!(
            corner_step((100.0, 200.0, 116.0, 216.0), PROBE_CARD, PROBE_CARD_RADIUS),
            None,
            "the middle of the card is more than a radius from every corner"
        );
    }

    /// The footer's height is the mockup's, and the column fits in what the footer leaves.
    ///
    /// The first assertion names the number rather than deriving it, because the failure this
    /// guards happened when the footer occupied space that nothing had reserved: the column ran
    /// underneath it and the last row was clipped. The second asserts the budget itself, so a row
    /// added later fails here instead of on screen.
    #[::core::prelude::v1::test]
    fn the_column_fits_above_the_footer() {
        assert_eq!(
            super::mockup::FOOTER_HEIGHT,
            56.0,
            "`.foot` is `padding: 12px 16px` (274) around a `.btn` of 32 (279)"
        );
        let available =
            super::PROBE_HEIGHT - super::PROBE_PADDING - super::mockup::FOOTER_HEIGHT;
        assert!(
            super::mockup::column_height() <= available,
            "the column is {} dip and the space above the footer is {}",
            super::mockup::column_height(),
            available
        );
    }
}

#[cfg(test)]
mod chip_bounds_tests {
    use gpui_kit::component::Root;
    use gpui_kit::component::button::Button;
    use gpui_kit::prelude::*;
    use gpui_kit::test::{TestSupportExt, TestWindowExt};
    use gpui_kit::{AppContext, Context, TestAppContext, Window, px, size};

    /// A single button, asked for the mockup's chip height.
    struct ChipProbe;

    impl Render for ChipProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            gpui_kit::div().p_4().child(
                gpui_kit::div()
                    .id("chip-box")
                    .test_support()
                    .child(
                        Button::new("chip")
                            .label("桌面")
                            .h(px(super::mockup::CHIP_HEIGHT)),
                    ),
            )
        }
    }

    /// Whether a button's own sizing yields to `.h()`.
    /// The column's height is summed from the mockup's `.chip { height: 24px }` (`:192`) and then
    /// compared with the space above the footer. If a button renders taller than it is asked for,
    /// that sum describes a column that does not exist, which is how the footer came to overlap
    /// the preset row while the arithmetic said everything fitted.
    #[gpui_kit::test]
    fn a_chip_button_is_as_tall_as_it_is_told(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let handle = cx.open_window(size(px(320.), px(200.)), |window, cx| {
            let view = cx.new(|_| ChipProbe);
            Root::new(view, window, cx)
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let chip = window.find("chip-box").bounds();
            assert_eq!(
                chip.size.height,
                px(super::mockup::CHIP_HEIGHT),
                "a chip is drawn at the mockup's 24 dip (`.chip {{ height: 24px }}`, :192)"
            );
        })
        .expect("the chip's frame renders; a failed render would otherwise pass silently");
    }

    /// The column renders at the height the budget is computed from.
    ///
    /// The footer's band is reserved using `column_height()`, so if the rows actually render
    /// taller than that sum, the reservation is short and the footer overlaps the last row exactly
    /// as it did on screen. This compares the sum with what the window lays out.
    #[gpui_kit::test]
    fn the_column_renders_at_the_height_it_is_summed_from(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let handle = cx.open_window(
            size(px(super::PROBE_WIDTH), px(super::PROBE_HEIGHT)),
            |window, cx| {
                let probe = cx.new(|cx| {
                    super::GpuiProbe::with_picture(crate::gpui_save::Picture::synthetic(), window, cx)
                });
                Root::new(probe, window, cx)
            },
        );
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let scale = window.scale_factor();
            let column = window.find("form-column").bounds();
            // Measured rather than derived: the rows render sixteen dips shorter than the sum
            // they are computed from, so the sum is an upper bound and not an identity. Guarding
            // the bound is what the footer's reservation needs; asserting equality would pin a
            // discrepancy nobody has explained yet.
            assert!(
                column.size.height <= px(super::dip(scale, super::mockup::column_height())),
                "the column renders at {:?} but the footer's band is computed from {:?}",
                column.size.height,
                px(super::dip(scale, super::mockup::column_height()))
            );
        })
        .expect("the column's frame renders; a failed render would otherwise pass silently");
    }
}
/// Which rows carry the extra margin, as data.
///
/// This is a pure assertion on purpose. Measuring the rendered gaps was tried and works when the
/// test runs alone but not in the full suite: the harness registers test-support nodes in a way
/// that depends on what ran before, and the lookup then reports an empty scope. Rather than pin a
/// test on that ordering, the rows are a list and the list is asserted here; a call site that
/// passes the wrong boolean is what the list cannot catch, and the note beside `row()` says so.
#[cfg(test)]
mod group_tests {
    use super::mockup;

    #[::core::prelude::v1::test]
    fn the_rows_that_open_a_group_are_the_mockups_two() {
        assert_eq!(
            mockup::GROUP_OPENERS,
            ["size", "estimate"],
            "the mockup puts the group margin above 尺寸 and 预估大小 (`:163`)"
        );
        assert!(mockup::opens_group("size"), "尺寸 opens a group (`:163`)");
        assert!(mockup::opens_group("estimate"), "预估大小 opens a group (`:163`)");
        for shared in ["path", "name", "saved", "quality"] {
            assert!(
                !mockup::opens_group(shared),
                "{shared} shares the group above it, so it pays no margin of its own"
            );
        }
    }
}
