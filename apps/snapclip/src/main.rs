//! SnapClip's GPUI shell (docs/23 P4).
//!
//! T4.2's job is deliberately small: prove that a gpui-kit application boots, that `Root`
//! wraps the window's first view, and that nothing about the Tauri host changes while this
//! grows next to it. The history list, settings, tray and event bridge arrive in T4.3–T4.6.
//!
//! Rules this file already follows, because the Coding Guides make them requirements:
//! `gpui_kit::init` runs first inside `run`, before any component-backed view exists;
//! each window's first level is `Root`; the UI comes from `gpui_kit` alone.

// Both traits are re-exported by `gpui-kit` itself (paths verified in gpui-base-0.7.1's
// lib.rs and gpui-component-0.7.1's lib.rs), which is what the one-dependency rule wants:
// `StyledExt` carries `v_flex`/`h_flex`, `ActiveTheme` carries `cx.theme()`.
use gpui_kit::base::StyledExt as _;
use gpui_kit::component::{ActiveTheme, Root};
use gpui_kit::*;

/// The shell's root view. A placeholder until T4.3 gives it the history list.
struct Shell;

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child("SnapClip")
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("GPUI shell — the history list lands here (docs/23 T4.3)"),
            )
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            // MUST be first: component-backed views need the component runtime.
            gpui_kit::init(cx);

            cx.spawn(async move |cx| {
                cx.open_window(WindowOptions::default(), |window, cx| {
                    let shell = cx.new(|_| Shell);
                    // `Root` is the window's first-level child; it owns overlays,
                    // notifications and modal focus restoration.
                    cx.new(|cx| Root::new(shell, window, cx))
                })
                .expect("failed to open the SnapClip shell window");
            })
            .detach();
        });
}
