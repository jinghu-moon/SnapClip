//! A throwaway probe: can the save panel be built out of GPUI components instead of a
//! window with one native control?
//!
//! Nothing here is on a production path. It opens when `SNAPCLIP_GPUI_PROBE` is set, it is the
//! only window in that run, and it exists to answer four questions by eye: does Chinese typing
//! work with the candidate window in place, does `Tab` reach both buttons, are the corners round
//! with a shadow, and is the client area the panel's size. If any answer is no, the migration is
//! not worth its cost and this file is deleted.

use gpui_kit::base::StyledExt as _;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;

/// The probe window's client size, matching the save panel so the eye compares like with like.
pub const PROBE_WIDTH: f32 = 920.0;
/// See [`PROBE_WIDTH`].
pub const PROBE_HEIGHT: f32 = 564.0;
/// The window title, which is what the taskbar and Alt-Tab show.
pub const PROBE_TITLE: &str = "\u{4fdd}\u{5b58}\u{56fe}\u{7247} \u{b7} GPUI \u{63a2}\u{9488}";
/// The name the panel starts with, so the field shows a template rather than a placeholder.
pub const PROBE_NAME: &str = "SnapClip_[\u{65e5}\u{671f}]";
/// The two buttons, named once so the label and the id cannot drift.
pub const PROBE_CANCEL: &str = "\u{53d6}\u{6d88}";
/// See [`PROBE_CANCEL`].
pub const PROBE_SAVE: &str = "\u{4fdd}\u{5b58}";

/// The probe's only state: the name field.
pub struct GpuiProbe {
    name: Entity<InputState>,
}

impl GpuiProbe {
    /// Build the view, seeding the name field with the template the panel uses.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(PROBE_NAME)
                .default_value(PROBE_NAME)
        });
        Self { name }
    }
}

impl Render for GpuiProbe {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .v_flex()
            .size_full()
            .gap_3()
            .p_4()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(div().child(PROBE_TITLE))
            .child(Input::new(&self.name).id("probe-name"))
            .child(
                div()
                    .h_flex()
                    .justify_end()
                    .gap_2()
                    .child(Button::new("probe-cancel").label(PROBE_CANCEL))
                    .child(Button::new("probe-save").primary().label(PROBE_SAVE)),
            )
    }
}
