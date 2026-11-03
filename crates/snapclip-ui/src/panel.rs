//! Shared panel components.
//!
//! Each component owns only its geometry/state contract. Domain actions stay in the feature that
//! composes the panel, while the Direct2D painter consumes these contracts.

#[path = "panel/button.rs"]
mod button;
#[path = "panel/checkbox.rs"]
mod checkbox;
#[path = "panel/dropdown.rs"]
mod dropdown;
#[path = "panel/elide.rs"]
mod elide;
#[path = "panel/feedback.rs"]
mod feedback;
#[path = "panel/field.rs"]
mod field;
#[path = "panel/fonts.rs"]
mod fonts;
#[path = "panel/frame.rs"]
mod frame;
#[path = "panel/icon_button.rs"]
mod icon_button;
#[path = "panel/icons.rs"]
mod icons;
#[path = "panel/interaction.rs"]
mod interaction;
#[path = "panel/motion.rs"]
mod motion;
#[path = "panel/radio.rs"]
mod radio;
#[path = "panel/segmented.rs"]
mod segmented;
#[path = "panel/slider.rs"]
mod slider;
#[path = "panel/tag.rs"]
mod tag;
mod text_field;
#[path = "panel/theme.rs"]
mod theme;
#[path = "panel/toggle.rs"]
mod toggle;

pub use button::{Button, ButtonState};
pub use checkbox::{Checkbox, CheckboxState};
pub use dropdown::{Dropdown, DropdownKey, DropdownTarget};
pub use elide::elide_middle;
pub use feedback::{Toast, Tooltip};
pub use field::{
    AFTER_CHIP_GAP_DIP,
    AFTER_CHIP_HEIGHT_DIP,
    AFTER_CHIP_PAD_DIP,
    AFTER_CHIP_WORDS,
    AFTER_LABEL_GAP_DIP,
    AUTO_SHRINK_LABEL,
    AfterChips,
    CLOSE_ICON_DIP,
    CONTEXT_ITEM_GAP_DIP,
    CONTEXT_ITEMS,
    CONTEXT_SHORTCUTS,
    CONTEXT_TEXT_PAD_DIP,
    ContextMenuView,
    EDIT_FONT_DIP,
    EDIT_INSET_DIP,
    EDIT_PADDING_DIP,
    ENCODE_STATE_BUSY,
    ENCODE_STATE_DONE,
    // The transparent fill row (`:655`-`:662`), shown by `:1102` for JPG only.
    FILL_BLACK,
    FILL_RING_DIP,
    FILL_RING_OFFSET_DIP,
    FILL_ROW_GAP_DIP,
    FILL_ROW_HEIGHT_DIP,
    FILL_ROW_LABEL,
    FILL_SWATCH_DIP,
    FILL_SWATCH_GAP_DIP,
    FILL_SWATCH_RADIUS_DIP,
    FILL_SWATCHES,
    FILL_WHITE,
    FMT_BUTTON_DIP,
    FMT_CHECK_DIP,
    FMT_CHEV_DIP,
    FMT_GAP_DIP,
    FMT_ITEM_DIP,
    FMT_ITEM_RADIUS_DIP,
    FMT_JPG_INDEX,
    FMT_MENU_DIP,
    FMT_MENU_LINE_DIP,
    FMT_MENU_PAD_DIP,
    FMT_MENU_RADIUS_DIP,
    FMT_OPTIONS,
    FMT_PNG_INDEX,
    FMT_RADIUS_DIP,
    FMT_RING_DIP,
    FOOTER_BUTTON_HEIGHT_DIP,
    FOOTER_BUTTON_RADIUS_DIP,
    FOOTER_BUTTONS,
    FOOTER_GAP_DIP,
    FOOTER_PAD_X_DIP,
    FORM_FIELD_TAB_STOPS,
    FORM_ROWS,
    FieldColours,
    FieldFace,
    FieldId,
    FieldState,
    FieldStates,
    FooterButton,
    INPUT_RADIUS_DIP,
    LIMIT_LABEL,
    LIMIT_NOTE_OK_AFTER,
    LIMIT_NOTE_OK_BEFORE,
    LIMIT_NOTE_OVER_AFTER,
    LIMIT_NOTE_OVER_BEFORE,
    LIMIT_NOTE_OVER_PNG_BEFORE,
    LIMIT_SWITCH_LABEL,
    LIMIT_UNIT,
    NATIVE_FAMILY,
    NUMBER_EDIT_FONT_DIP,
    QUALITY_HINT_TEXT,
    QUALITY_OPTION_WIDTH_DIP,
    QUALITY_OPTION_WORDS,
    QUALITY_OPTIONS,
    QUALITY_SEG_INNER_RADIUS_DIP,
    QUALITY_SEG_INSET_DIP,
    QUALITY_SEG_RADIUS_DIP,
    QUALITY_SEG_WIDTH_DIP,
    QualityLine,
    ROW_DIP,
    Row,
    SELECTION_PERCENT,
    SIZE_MODE_BORDER_DIP,
    SIZE_MODE_BUTTON_HEIGHT_DIP,
    SIZE_MODE_BUTTON_PADDING_DIP,
    SIZE_MODE_BUTTON_RADIUS_DIP,
    SIZE_MODE_CHOSEN,
    SIZE_MODE_GAP_DIP,
    SIZE_MODE_PADDING_DIP,
    SIZE_MODE_RADIUS_DIP,
    SIZE_MODE_RESET_DIP,
    SIZE_MODE_RESET_ICON_DIP,
    SIZE_MODE_RESET_RADIUS_DIP,
    SIZE_MODE_WORDS,
    SIZE_PRESET_GAP_DIP,
    SIZE_PRESET_INDENT_DIP,
    SIZE_PRESET_MARGIN_DIP,
    SIZE_PRESET_PCT,
    SIZE_PRESET_WORDS,
    SizeCell,
    VAR_ITEM_H_DIP,
    VAR_MENU_GAP_DIP,
    VAR_MENU_OVERHANG_DIP,
    VAR_MENU_PAD_DIP,
    VAR_MENU_W_DIP,
    VARIABLE_LABELS,
    VARIABLE_TOKENS,
    WARN_HINT_DIP,
    WARN_HINT_TEXT,
    WARNING_RGBA,
    after_chip_rect,
    after_chip_width,
    auto_shrink_rect,
    chosen_quality,
    close_button_contains,
    close_icon_rect,
    conflict_link_contains,
    conflict_link_rect,
    context_enabled,
    context_item_at,
    context_item_rect,
    context_item_rects,
    context_menu_height_dip,
    context_menu_rect,
    context_menu_width,
    edit_control_rect,
    edit_font_height,
    edit_font_height_for,
    encode_state_rect,
    estimate_label_rect,
    estimate_value_rect,
    field_colours,
    field_rect,
    fields_under,
    fill_row_added_height,
    fill_row_rect,
    fill_row_shown,
    fill_swatch_at,
    fill_swatch_rect,
    fill_swatch_selected,
    fmt_button_contains,
    fmt_button_rect,
    fmt_check_rect,
    fmt_chev_rect,
    fmt_item_at,
    fmt_item_label_rect,
    fmt_item_rect,
    fmt_label_rect,
    fmt_menu_contains,
    fmt_menu_height_dip,
    fmt_menu_rect,
    footer_border_y,
    footer_button_contains,
    footer_button_is_strong,
    footer_button_rect,
    footer_button_text,
    footer_button_width,
    footer_copy_icon_rect,
    footer_copy_text_rect,
    footer_line_over_surface,
    footer_rect,
    footer_separator_rect,
    footer_status_rect,
    form_drawn_strings,
    hidden_fields,
    insert_variable,
    label_rect,
    limit_input_rect,
    limit_knob_rect,
    limit_label_rect,
    limit_note_rect,
    limit_switch_rect,
    limit_track_rect,
    limit_unit_rect,
    line_soft_over_surface,
    minimize_button_contains,
    minimize_icon_rect,
    name_field_rect,
    name_wand_contains,
    path_change_contains,
    path_chip_at,
    path_field_rect,
    path_open_contains,
    problem_of,
    quality_extra_height,
    quality_hint_over_surface,
    quality_hint_rect,
    quality_line,
    quality_option_rect,
    quality_pointer,
    quality_seg_rect,
    quality_segment_shown,
    quality_segment_state,
    rect_contains,
    rects_intersect,
    selection_over_field,
    size_cell_rect,
    size_mode_button_rect,
    size_mode_button_width,
    size_mode_contains,
    size_mode_rect,
    size_mode_reset_icon_rect,
    size_mode_reset_rect,
    size_preset_contains,
    size_preset_rect,
    size_preset_size,
    size_preset_width,
    size_presets_rect,
    size_reset_contains,
    var_item_at,
    var_item_rect,
    var_menu_contains,
    var_menu_rect,
    warn_hint_icon_rect,
    warn_hint_rect,
    warn_hint_shown,
    warn_hint_text_rect,
};
pub use fonts::{
    NUMBER_CHARACTERS, NUMBER_FALLBACKS, NUMBER_FAMILY, NUMBER_SUBSET, number_characters,
    register_number_font,
};
pub use frame::{
    CHIP_FONT_DIP,
    DEMO,
    DEMO_PALETTE,
    Frame,
    FrameBrushes,
    FrameMetrics,
    FramePalette,
    FrameRects,
    HEADER_BUTTON_GAP_DIP,
    HINT_FONT_DIP,
    LimitView,
    LimitWidths,
    NUMBER_FONT_DIP,
    PRESET_FONT_DIP,
    PaintInput,
    PanelRenderer,
    ROW_LABEL_FONT_DIP,
    TITLE,
    UNIT_FONT_DIP,
    VAR_FONT_DIP,
    chip_format,
    factory,
    // The fit rule for the preview card. Re-exported so a second renderer can ask the same function
    // rather than restate the rule: the point of the migration is one geometry, not two.
    fit_preview,
    hint_format,
    measure_footer_widths,
    measure_limit_words,
    measure_path_chips,
    measure_size_presets,
    number_format,
    preset_label_format,
    row_label_format,
    status_format,
    text_format_in,
    title_format,
    var_label_format,
};
pub use icon_button::IconButton;
pub use icons::{Glyph, ICON_BAKED_PX, ICON_COUNT};
pub use interaction::{InteractionState, PointerEvent};
pub use motion::{Easing, FAST, Motion, NORMAL, SEGMENT_SLIDE, SLOW, ease_out};
pub use radio::{RadioGroup, RadioGroupState, RadioOption};
pub use segmented::{
    SEGMENT_PAD_DIP, SEGMENT_RADIUS_DIP, Segment, SegmentState, segment_outer_radius,
    segment_radius, segment_slide,
};
pub use slider::{Slider, SliderKey};
pub use tag::{Tag, TagState};
pub use text_field::{
    MAX_DIGITS, MAX_SIZE, MIN_SIZE, NumberView, NumberViews, TextField, digits, right_aligned_left,
    value_of,
};
pub use theme::{FAMILY, PanelTheme};
pub use toggle::{Switch, SwitchState, Toggle, ToggleState};

#[cfg(windows)]
#[path = "panel/d2d.rs"]
pub mod d2d;

#[cfg(windows)]
pub use button::d2d::ButtonColors;
#[cfg(windows)]
pub use checkbox::d2d::CheckboxColors;
#[cfg(windows)]
pub use d2d::PaintTarget;
#[cfg(windows)]
pub use dropdown::d2d::DropdownColors;
#[cfg(windows)]
pub use feedback::d2d::FeedbackColors;
#[cfg(windows)]
pub use radio::d2d::RadioColors;
#[cfg(windows)]
pub use slider::d2d::SliderColors;
#[cfg(windows)]
pub use tag::d2d::TagColors;
#[cfg(windows)]
#[cfg(windows)]
pub use toggle::d2d::ToggleColors;
