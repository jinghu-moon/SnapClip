//! Short-lived, independent feedback for commands without a surviving pin window.
use crate::{PinManager, platform};
use gpui_kit::{
    component::{ActiveTheme, Root},
    prelude::*,
    *,
};
use std::time::Duration;

struct Feedback {
    message: String,
    undo: Option<WeakEntity<PinManager>>,
}
impl Render for Feedback {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = self.undo.clone();
        div()
            .id("pin-read-result")
            .role(Role::Status)
            .aria_label(self.message.clone())
            .size_full()
            .flex()
            .items_center()
            .gap_2()
            .p_3()
            .bg(cx.theme().popover)
            .text_color(cx.theme().popover_foreground)
            .text_sm()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .child(div().flex_1().min_w_0().child(self.message.clone()))
            .when_some(owner, |s, owner| {
                s.child(
                    yue_component::YueButton::text(
                        "pin-undo-close",
                        yue_component::current_theme(cx),
                        "撤销关闭",
                    )
                    .size(yue_component::ButtonSize::Small)
                    .on_click(move |_, window, cx| {
                        let owner = owner.clone();
                        let position = platform::cursor_position();
                        window.remove_window();
                        cx.defer(move |cx| {
                            if let Some(owner) = owner.upgrade()
                                && let Err(error) = PinManager::undo(&owner, position, cx)
                            {
                                crate::show_feedback(error.to_string(), position, cx);
                            }
                        });
                    }),
                )
            })
    }
}
pub(crate) fn show(
    message: String,
    position: (i32, i32),
    undo: Option<WeakEntity<PinManager>>,
    duration: Duration,
    cx: &mut App,
) -> Option<WindowHandle<Root>> {
    let handle = cx
        .open_window(
            WindowOptions {
                titlebar: None,
                focus: false,
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(320.), px(80.)),
                ))),
                ..Default::default()
            },
            |window, cx| {
                platform::configure(window, position, (320., 80.), true);
                let view = cx.new(|_| Feedback { message, undo });
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .ok()?;
    cx.spawn(async move |cx| {
        cx.background_executor().timer(duration).await;
        _ = handle.update(cx, |_, window, _| window.remove_window());
    })
    .detach();
    Some(handle)
}
