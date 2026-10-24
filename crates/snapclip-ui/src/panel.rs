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
#[path = "panel/field.rs"]
mod field;
#[path = "panel/fonts.rs"]
mod fonts;
#[path = "panel/frame.rs"]
mod frame;
#[path = "panel/feedback.rs"]
mod feedback;
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
#[path = "panel/theme.rs"]
mod theme;
#[path = "panel/toggle.rs"]
mod toggle;

pub use button::{Button, ButtonState};
pub use checkbox::{Checkbox, CheckboxState};
pub use dropdown::{Dropdown, DropdownKey, DropdownTarget};
pub use feedback::{Toast, Tooltip};
pub use fonts::{
    NUMBER_CHARACTERS, NUMBER_FAMILY, NUMBER_FALLBACKS, NUMBER_SUBSET, number_characters,
    register_number_font,
};
pub use field::{
    EDIT_FONT_DIP, EDIT_INSET_DIP, EDIT_PADDING_DIP, FieldColours, FieldFace, FieldId, FieldState,
    FieldStates, FORM_ROWS,
    problem_of, WARNING_RGBA,
    FMT_BUTTON_DIP, FMT_CHECK_DIP, FMT_CHEV_DIP, FMT_GAP_DIP, FMT_ITEM_DIP, FMT_MENU_DIP, FMT_RADIUS_DIP, FMT_RING_DIP,
    FMT_MENU_PAD_DIP, FMT_ITEM_RADIUS_DIP, FMT_MENU_LINE_DIP, FMT_MENU_RADIUS_DIP, FMT_OPTIONS, fmt_button_contains, fmt_button_rect, fmt_check_rect,
    fmt_chev_rect, fmt_item_at, fmt_item_label_rect, fmt_item_rect, fmt_label_rect,
    fmt_menu_contains, fmt_menu_height_dip, fmt_menu_rect,
    FORM_FIELD_TAB_STOPS, INPUT_RADIUS_DIP, NUMBER_EDIT_FONT_DIP, QUALITY_OPTIONS,
    QUALITY_OPTION_WORDS,
    QUALITY_OPTION_WIDTH_DIP, QUALITY_SEG_INNER_RADIUS_DIP,
    QUALITY_SEG_INSET_DIP, QUALITY_SEG_RADIUS_DIP, QUALITY_SEG_WIDTH_DIP, ROW_DIP, Row, SizeCell,
    AFTER_CHIP_GAP_DIP, AFTER_CHIP_HEIGHT_DIP, AFTER_CHIP_PAD_DIP, AFTER_CHIP_WORDS,
    AFTER_LABEL_GAP_DIP, after_chip_rect, after_chip_width,
    SIZE_MODE_BUTTON_HEIGHT_DIP, SIZE_MODE_BUTTON_PADDING_DIP, SIZE_MODE_BUTTON_RADIUS_DIP,
    SIZE_MODE_BORDER_DIP, SIZE_MODE_CHOSEN, SIZE_MODE_GAP_DIP, SIZE_MODE_PADDING_DIP,
    SIZE_MODE_RADIUS_DIP, SIZE_MODE_RESET_DIP, SIZE_MODE_RESET_ICON_DIP, SIZE_MODE_RESET_RADIUS_DIP,
    SIZE_MODE_WORDS, CLOSE_ICON_DIP, close_button_contains, close_icon_rect,
    FOOTER_BUTTON_HEIGHT_DIP, FOOTER_BUTTON_RADIUS_DIP, FOOTER_BUTTONS, FOOTER_GAP_DIP,
    FOOTER_PAD_X_DIP, FooterButton, footer_border_y, footer_button_contains, footer_button_rect,
    footer_button_width, footer_button_is_strong, footer_button_text, footer_copy_icon_rect,
    footer_copy_text_rect, footer_line_over_surface,
    footer_rect, footer_separator_rect, footer_status_rect,
    chosen_quality, edit_font_height, edit_font_height_for, field_colours, field_rect,
    form_drawn_strings, label_rect,
    line_soft_over_surface,
    name_field_rect, path_field_rect, quality_option_rect, quality_pointer, quality_seg_rect,
    quality_segment_state, size_cell_rect,
    FMT_PNG_INDEX, QUALITY_HINT_TEXT, quality_hint_over_surface, quality_hint_rect,
    quality_segment_shown,
    SIZE_PRESET_GAP_DIP, SIZE_PRESET_INDENT_DIP, SIZE_PRESET_MARGIN_DIP, SIZE_PRESET_PCT,
    SIZE_PRESET_WORDS, size_preset_contains, size_preset_rect, size_preset_size,
    size_preset_width, size_presets_rect,
    size_mode_button_rect, size_mode_button_width, size_mode_contains, size_mode_rect,
    size_mode_reset_icon_rect, size_mode_reset_rect, size_reset_contains,
    rect_contains, rects_intersect, path_change_contains, path_open_contains, path_chip_at,
    name_wand_contains, fields_under, insert_variable,
    VARIABLE_LABELS, VARIABLE_TOKENS, VAR_ITEM_H_DIP, VAR_MENU_GAP_DIP, VAR_MENU_OVERHANG_DIP,
    VAR_MENU_PAD_DIP, VAR_MENU_W_DIP, var_item_at, var_item_rect, var_menu_contains, var_menu_rect,
};
pub use frame::{
    CHIP_FONT_DIP, DEMO, DEMO_PALETTE, Frame, FrameBrushes, FrameMetrics, FramePalette, FrameRects,
    NUMBER_FONT_DIP, PaintInput, PanelRenderer, ROW_LABEL_FONT_DIP, TITLE, UNIT_FONT_DIP,
    chip_format, factory, measure_footer_widths, measure_path_chips, measure_size_presets,
    number_format,
    row_label_format, status_format, text_format_in, title_format, var_label_format, VAR_FONT_DIP,
    HINT_FONT_DIP, PRESET_FONT_DIP, hint_format, preset_label_format,
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
