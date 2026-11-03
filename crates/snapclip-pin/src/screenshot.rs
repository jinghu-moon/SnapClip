//! Chrome-free F6 host. Window interaction belongs here; pixels and color
//! rendering use the same Yue resource path as the inspectable F3 viewer.
use crate::{
    CopyTarget, PinContent, PinManager, PinSnapshot, platform, screenshot_geometry as geometry,
};
use gpui_kit::{component::ActiveTheme, prelude::*, *};
use std::{cell::Cell, rc::Rc, sync::Arc};
use yue_component::{ImagePreviewImage, ImagePreviewSampling};

pub(crate) struct ScreenshotView {
    id: u64,
    snapshot: Arc<PinSnapshot>,
    owner: WeakEntity<PinManager>,
    image: Option<ImagePreviewImage>,
    pub(crate) position: (i32, i32),
    pub(crate) zoom: f64,
    radius: i32,
    remainder: i32,
    scale: f32,
    visible: bool,
    receipt_sent: Rc<Cell<bool>>,
    _prepare: Task<()>,
    _bounds: Subscription,
}

pub(crate) fn physical_bounds(
    position: (i32, i32),
    dimensions: (i32, i32),
) -> Bounds<DevicePixels> {
    Bounds::new(
        point(DevicePixels(position.0), DevicePixels(position.1)),
        size(DevicePixels(dimensions.0), DevicePixels(dimensions.1)),
    )
}

impl ScreenshotView {
    pub(crate) fn new(
        id: u64,
        snapshot: Arc<PinSnapshot>,
        owner: WeakEntity<PinManager>,
        position: (i32, i32),
        radius: i32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let PinContent::Image(pixels) = snapshot.content() else {
            unreachable!("validated screenshot")
        };
        let pixels = pixels.clone();
        let prepare = cx.spawn_in(window, async move |this, cx| {
            let prepared = cx
                .background_spawn(async move { crate::image_preview::render(&pixels) })
                .await;
            _ = this.update_in(cx, |this, window, cx| {
                match prepared {
                    Ok(image) => {
                        this.image = Some(image);
                        cx.notify();
                    }
                    Err(error) => {
                        let owner = this.owner.clone();
                        let id = this.id;
                        cx.defer(move |cx| {
                            _ = owner.update(cx, |m, cx| m.screenshot_ready(id, Err(error), cx));
                        });
                    }
                }
                window.refresh();
            });
        });
        let bounds = cx.observe_window_bounds(window, |this, window, cx| {
            if this.scale != window.scale_factor() {
                this.scale = window.scale_factor();
                this.position = platform::origin(window).unwrap_or(this.position);
                // Only the 1-DIP chrome grows with DPI; source pixels retain
                // their physical zoom. Do not redraw on ordinary window moves.
                this.apply_bounds(window);
                cx.notify();
            }
        });
        let this = Self {
            id,
            snapshot,
            owner,
            image: None,
            position,
            zoom: 1.,
            radius,
            remainder: 0,
            scale: window.scale_factor(),
            visible: true,
            receipt_sent: Rc::new(Cell::new(false)),
            _prepare: prepare,
            _bounds: bounds,
        };
        let (width, height) = this.dimensions();
        platform::round_screenshot(
            window,
            geometry::window_size(width, height, 1., this.border()),
            if radius > 0 {
                radius + this.border()
            } else {
                0
            },
        );
        this
    }

    fn dimensions(&self) -> (u32, u32) {
        let PinContent::Image(image) = self.snapshot.content() else {
            unreachable!()
        };
        (image.width(), image.height())
    }

    fn border(&self) -> i32 {
        geometry::border_px((self.scale * 96.).round() as u32)
    }

    fn apply_bounds(&self, window: &mut Window) {
        let (width, height) = self.dimensions();
        let dimensions = geometry::window_size(width, height, self.zoom, self.border());
        platform::round_screenshot(
            window,
            dimensions,
            if self.radius > 0 {
                geometry::scaled_radius(self.radius, self.zoom) + self.border()
            } else {
                0
            },
        );
        if !window.set_physical_bounds(physical_bounds(self.position, dimensions))
            && platform::hwnd(window).is_none()
        {
            // The headless platform has no native origin, but can exercise the
            // same logical image geometry and all production input handlers.
            window.resize(size(
                px(dimensions.0 as f32 / self.scale),
                px(dimensions.1 as f32 / self.scale),
            ));
        }
    }

    pub(crate) fn set_visible(&mut self, visible: bool, window: &mut Window) {
        self.visible = visible;
        if !visible {
            platform::hover_capture(window, false);
        }
        platform::visible(window, visible);
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let delta = platform::wheel_delta(event);
        let (zoom, remainder, notches) = geometry::wheel_zoom(self.zoom, self.remainder, delta);
        self.remainder = remainder;
        if notches == 0 || zoom == self.zoom {
            return;
        }
        self.position = platform::origin(window).unwrap_or(self.position);
        let cursor = (
            self.position.0 + (f32::from(event.position.x) * self.scale).round() as i32,
            self.position.1 + (f32::from(event.position.y) * self.scale).round() as i32,
        );
        let (width, height) = self.dimensions();
        self.position = geometry::zoom_with_cursor(
            self.zoom,
            zoom,
            self.position,
            width,
            height,
            self.border(),
            cursor,
        );
        self.zoom = zoom;
        self.apply_bounds(window);
        cx.notify();
        cx.stop_propagation();
    }

    fn menu(&self, window: &Window, cx: &mut Context<Self>) {
        let Some(hwnd) = platform::hwnd(window) else {
            return;
        };
        let hwnd = hwnd as usize;
        let hdr = matches!(self.snapshot.content(), PinContent::Image(i) if i.is_hdr());
        let text = self.snapshot.text().is_some_and(|t| !t.is_empty());
        // This compact, nonactivating window may be just a few pixels wide.
        // Native menus can extend beyond its HWND; in-window popups cannot.
        cx.spawn(async move |this, cx| {
            let command = platform::image_menu(hwnd, hdr, text);
            _ = this.update(cx, |this, cx| {
                let owner = this.owner.clone();
                let id = this.id;
                match command {
                    1 | 2 => {
                        let PinContent::Image(image) = this.snapshot.content() else {
                            return;
                        };
                        let target = if command == 2 {
                            CopyTarget::HdrPixels(image.clone())
                        } else {
                            CopyTarget::Pixels(image.clone())
                        };
                        _ = owner.update(cx, |m, _| m.copy(id, target));
                    }
                    3 => {
                        _ = owner.update(cx, |m, cx| m.show_text(id, cx));
                    }
                    4 => {
                        _ = owner.update(cx, |m, cx| m.close(id, cx));
                    }
                    _ => {}
                }
            });
        })
        .detach();
    }
}

impl Render for ScreenshotView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (width, height) = self.dimensions();
        let scale = window.scale_factor();
        let border = self.border();
        let image_size = size(
            px(geometry::scaled_dimension(width as f64, self.zoom) as f32 / scale),
            px(geometry::scaled_dimension(height as f64, self.zoom) as f32 / scale),
        );
        let radius = px(geometry::scaled_radius(self.radius, self.zoom) as f32 / scale);
        let inset = px(border as f32 / scale);
        let image = self.image.clone();
        let sent = self.receipt_sent.clone();
        let owner = self.owner.clone();
        let id = self.id;
        let visible = self.visible;
        let surface = div()
            .id("pin-screenshot")
            .map(|s| {
                #[cfg(feature = "headless-tests")]
                let s = s.test_support();
                s
            })
            .relative()
            .size_full()
            .on_hover(|hovered, window, _| platform::hover_capture(window, *hovered))
            .on_mouse_down(MouseButton::Left, |event, window, cx| {
                if event.click_count >= 2 {
                    window.remove_window();
                } else {
                    platform::start_move(window);
                }
                cx.stop_propagation();
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, window, cx| this.menu(window, cx)),
            )
            .on_scroll_wheel(cx.listener(Self::scroll))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        if let Some(image) = &image {
                            // The outer hairline never covers source pixels. Rounded
                            // transparent corners are handled by the native GPU shader.
                            window.paint_quad(quad(
                                bounds,
                                Corners::all(radius + inset),
                                transparent_black(),
                                Edges::all(inset),
                                gpui_kit::component::Theme::global(cx).border,
                                BorderStyle::Solid,
                            ));
                            let image_bounds =
                                Bounds::new(bounds.origin + point(inset, inset), image_size);
                            let painted = image
                                .paint(
                                    bounds,
                                    image_bounds,
                                    radius,
                                    ImagePreviewSampling::Smooth,
                                    window,
                                )
                                .map_err(|e| e.to_string());
                            if visible && !sent.replace(true) {
                                // This is an upload/paint acknowledgement, never an
                                // acknowledgement of an empty window or a decode alone.
                                cx.defer(move |cx| {
                                    _ = owner
                                        .update(cx, |m, cx| m.screenshot_ready(id, painted, cx));
                                });
                            }
                        }
                    },
                )
                .size_full(),
            );
        surface.when(self.zoom != 1., |s| {
            s.child(
                div()
                    .absolute()
                    .right_1()
                    .bottom_1()
                    .rounded_md()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .bg(cx.theme().secondary)
                    .text_color(cx.theme().secondary_foreground)
                    .child(geometry::badge_label(self.zoom)),
            )
        })
    }
}

#[cfg(all(test, feature = "headless-tests"))]
#[path = "screenshot_tests.rs"]
mod tests;

#[cfg(all(test, feature = "headless-tests"))]
#[path = "screenshot_native_tests.rs"]
mod native_tests;
