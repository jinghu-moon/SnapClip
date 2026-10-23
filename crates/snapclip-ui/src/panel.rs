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
    NUMBER_CHARACTERS, NUMBER_FAMILY, NUMBER_FALLBACKS, NUMBER_SUBSET, register_number_font,
};
pub use field::{
    EDIT_FONT_DIP, EDIT_INSET_DIP, EDIT_PADDING_DIP, FieldColours, FieldFace, FieldId, FieldState,
    FieldStates, FORM_ROWS,
    FORM_FIELD_TAB_STOPS, INPUT_RADIUS_DIP, NUMBER_EDIT_FONT_DIP, QUALITY_OPTIONS,
    QUALITY_OPTION_WORDS,
    QUALITY_OPTION_WIDTH_DIP, QUALITY_SEG_INNER_RADIUS_DIP,
    QUALITY_SEG_INSET_DIP, QUALITY_SEG_RADIUS_DIP, QUALITY_SEG_WIDTH_DIP, ROW_DIP, Row, SizeCell,
    SIZE_MODE_BUTTON_HEIGHT_DIP, SIZE_MODE_BUTTON_PADDING_DIP, SIZE_MODE_BUTTON_RADIUS_DIP,
    SIZE_MODE_BORDER_DIP, SIZE_MODE_CHOSEN, SIZE_MODE_GAP_DIP, SIZE_MODE_PADDING_DIP,
    SIZE_MODE_RADIUS_DIP, SIZE_MODE_RESET_DIP, SIZE_MODE_RESET_ICON_DIP, SIZE_MODE_RESET_RADIUS_DIP,
    SIZE_MODE_WORDS, CLOSE_ICON_DIP, close_button_contains, close_icon_rect,
    chosen_quality, edit_font_height, edit_font_height_for, field_colours, field_rect,
    form_drawn_strings, label_rect,
    line_soft_over_surface,
    name_field_rect, path_field_rect, quality_option_rect, quality_pointer, quality_seg_rect,
    quality_segment_state, size_cell_rect,
    size_mode_button_rect, size_mode_button_width, size_mode_rect, size_mode_reset_icon_rect,
    size_mode_reset_rect,
};
pub use frame::{
    CHIP_FONT_DIP, DEMO, DEMO_PALETTE, Frame, FrameBrushes, FrameMetrics, FramePalette, FrameRects,
    NUMBER_FONT_DIP, PanelRenderer, ROW_LABEL_FONT_DIP, TITLE, UNIT_FONT_DIP, chip_format, factory,
    number_format, paint_to_dc, paint_to_target,
    row_label_format, text_format_in, title_format,
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
