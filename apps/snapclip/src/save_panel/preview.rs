//! App-specific image viewport. Both sides contain real pixels, not a styled mock.
use super::{Export, Picture, SavePanel};
use crate::save::{assemble, scale::Size};
use gpui_kit::*;
use std::sync::Arc;

#[derive(Clone, Copy, Default, PartialEq)]
pub(super) enum PreviewMode {
    #[default]
    Original,
    Split,
    Encoded,
}

#[derive(Default)]
pub(super) struct Preview {
    pub original: Option<Arc<RenderImage>>,
    pub encoded: Option<Arc<RenderImage>>,
    pub mode: PreviewMode,
    pub zoom: Option<f32>,
    pub pan: Point<Pixels>,
    pub split: f32,
    pub bounds: Bounds<Pixels>,
    drag: Option<Drag>,
    pub light: bool,
    pub grid: bool,
}

enum Drag {
    Split,
    Pan {
        start: Point<Pixels>,
        initial: Point<Pixels>,
    },
}

pub(super) struct EncodedPreview {
    pub bytes: u64,
    pub encoded: Arc<RenderImage>,
}

// Ordinary captures, including 4K, fit a 4096² BGRA texture (at most 64 MiB).
// Let the GPU scale those at paint time instead of delaying first display with
// a CPU resampling pass. Larger captures retain the smaller thumbnail budget.
const RAW_TEXTURE_MAX_EDGE: u32 = 4096;
const THUMBNAIL_MAX_EDGE: u32 = 2048;

/// GPUI stores image frames in BGRA even though image::Frame names its buffer RGBA.
fn render_image(mut rgba: image::RgbaImage) -> Arc<RenderImage> {
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(rgba)]))
}

/// The capture is already decoded BGRA. Its first display must not depend on
/// export compression, format support, debounce, or the current output draft.
pub(super) fn original(picture: &Picture) -> Result<Arc<RenderImage>, String> {
    let (pixels, source) = picture.pixels();
    let expected = (source.width as usize)
        .checked_mul(source.height as usize)
        .and_then(|n| n.checked_mul(4));
    if source.width == 0 || source.height == 0 || expected != Some(pixels.len()) {
        return Err("截图像素长度不正确".into());
    }
    let size = if source.width <= RAW_TEXTURE_MAX_EDGE && source.height <= RAW_TEXTURE_MAX_EDGE {
        source
    } else {
        let ratio = (f64::from(THUMBNAIL_MAX_EDGE) / f64::from(source.width))
            .min(f64::from(THUMBNAIL_MAX_EDGE) / f64::from(source.height));
        Size::new(
            (f64::from(source.width) * ratio).round().max(1.) as u32,
            (f64::from(source.height) * ratio).round().max(1.) as u32,
        )
    };
    // Same-size scaling only copies BGRA; no decoding or channel conversion.
    // Larger sources are sampled from borrowed rows, without a full-size clone.
    let bgra = crate::save::scale::scaled(&picture.frame(size));
    let buffer =
        image::RgbaImage::from_raw(size.width, size.height, bgra).ok_or("截图像素长度不正确")?;
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

pub(super) fn encode(export: &Export) -> Result<EncodedPreview, String> {
    super::model::validate_preview_size(export.output).map_err(str::to_owned)?;
    let encoded = assemble::encoded_bytes(&export.values(), &export.picture.frame(export.output))
        .map_err(|e| e.message())?;
    let decoded = image::load_from_memory(&encoded).map_err(|e| format!("无法解码预览：{e}"))?;
    // Limit GPU texture size only. Export pixels and byte counts remain full-resolution.
    let encoded_image = render_image(
        decoded
            .thumbnail(THUMBNAIL_MAX_EDGE, THUMBNAIL_MAX_EDGE)
            .into_rgba8(),
    );
    Ok(EncodedPreview {
        bytes: encoded.len() as u64,
        encoded: encoded_image,
    })
}

pub(super) fn fitted_zoom(source: Size, bounds: Bounds<Pixels>) -> f32 {
    ((f32::from(bounds.size.width) - 32.).max(1.) / source.width as f32)
        .min((f32::from(bounds.size.height) - 32.).max(1.) / source.height as f32)
        .min(1.)
}

pub(super) fn image_bounds(
    source: Size,
    viewport: Bounds<Pixels>,
    zoom: f32,
    pan: Point<Pixels>,
) -> Bounds<Pixels> {
    let size = size(
        px(source.width as f32 * zoom),
        px(source.height as f32 * zoom),
    );
    Bounds::new(
        viewport.origin
            + point(
                (viewport.size.width - size.width) / 2.,
                (viewport.size.height - size.height) / 2.,
            )
            + pan,
        size,
    )
}

impl SavePanel {
    pub(super) fn zoom(&self) -> f32 {
        self.preview
            .zoom
            .unwrap_or_else(|| fitted_zoom(self.picture.size(), self.preview.bounds))
    }

    pub(super) fn zoom_by(
        &mut self,
        factor: f32,
        anchor: Option<Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let old = self.zoom();
        let new = (old * factor).clamp(0.01, 16.);
        let viewport = self.preview.bounds;
        let center = viewport.origin + point(viewport.size.width / 2., viewport.size.height / 2.);
        let anchor = anchor.unwrap_or(center);
        self.preview.pan = (self.preview.pan + center - anchor) * (new / old) - center + anchor;
        self.preview.zoom = Some(new);
        cx.notify();
    }

    pub(super) fn viewport(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let original = self.preview.original.clone();
        let encoded = self.preview.encoded.clone();
        let mode = self.preview.mode;
        let zoom = self.preview.zoom;
        let pan = self.preview.pan;
        let split = self.preview.split;
        let grid = self.preview.grid;
        let source = self.picture.size();
        let theme = yue_component::current_theme(cx);
        let background: Hsla = if self.preview.light {
            rgba(0xffffffff).into()
        } else {
            rgb(theme.surface_component()).into()
        };
        let checker: Hsla = if self.preview.light {
            rgba(0xeeeeeeff).into()
        } else {
            rgb(theme.border_default()).into()
        };
        let line: Hsla = rgb(theme.accent_solid()).into();
        let owner = cx.entity().downgrade();
        div()
            .id("save-preview")
            .test_support()
            .relative()
            .flex_1()
            .w_full()
            .min_h(rems(12.))
            .overflow_hidden()
            .bg(background)
            .border_1()
            .border_color(rgb(theme.border_default()))
            .rounded_lg()
            .cursor(CursorStyle::OpenHand)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    let divider = this.preview.bounds.origin.x
                        + this.preview.bounds.size.width * this.preview.split;
                    this.preview.drag = Some(
                        if this.preview.mode == PreviewMode::Split
                            && (event.position.x - divider).abs() < px(12.)
                        {
                            Drag::Split
                        } else {
                            Drag::Pan {
                                start: event.position,
                                initial: this.preview.pan,
                            }
                        },
                    );
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if !event.dragging() {
                    this.preview.drag = None;
                    return;
                }
                match this.preview.drag.as_ref() {
                    Some(Drag::Split) => {
                        this.preview.split = ((event.position.x - this.preview.bounds.origin.x)
                            / this.preview.bounds.size.width)
                            .clamp(0., 1.)
                    }
                    Some(Drag::Pan { start, initial }) => {
                        this.preview.pan = *initial + event.position - *start
                    }
                    None => return,
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.preview.drag = None;
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.preview.drag = None;
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                let delta = f32::from(event.delta.pixel_delta(px(16.)).y);
                this.zoom_by((-delta * 0.002).exp(), Some(event.position), cx);
                cx.stop_propagation();
            }))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        _ = owner.update(cx, |this, _| this.preview.bounds = bounds);
                        bounds
                    },
                    move |bounds, _, window, _| {
                        // Keep checker cells away from rounded corners; the border remains a quiet solid rim.
                        let inner = Bounds::new(
                            bounds.origin + point(px(8.), px(8.)),
                            bounds.size - size(px(16.), px(16.)),
                        );
                        window.with_content_mask(Some(ContentMask { bounds: inner }), |window| {
                            let columns = (f32::from(inner.size.width) / 12.).ceil() as usize;
                            let rows = (f32::from(inner.size.height) / 12.).ceil() as usize;
                            for y in 0..rows {
                                for x in 0..columns {
                                    if (x + y) % 2 == 0 {
                                        window.paint_quad(fill(
                                            Bounds::new(
                                                inner.origin
                                                    + point(px(x as f32 * 12.), px(y as f32 * 12.)),
                                                size(px(12.), px(12.)),
                                            ),
                                            checker,
                                        ));
                                    }
                                }
                            }
                            let zoom = zoom.unwrap_or_else(|| fitted_zoom(source, bounds));
                            let image = image_bounds(source, bounds, zoom, pan);
                            let render =
                                |window: &mut Window,
                                 clip: Bounds<Pixels>,
                                 data: &Option<Arc<RenderImage>>| {
                                    if let Some(data) = data {
                                        _ = window.paint_image(
                                            clip,
                                            image,
                                            Corners::all(px(0.)),
                                            data.clone(),
                                            0,
                                            false,
                                        );
                                    }
                                };
                            match mode {
                                PreviewMode::Original => render(window, inner, &original),
                                PreviewMode::Encoded => render(window, inner, &encoded),
                                PreviewMode::Split => {
                                    render(window, inner, &original);
                                    let at = bounds.origin.x + bounds.size.width * split;
                                    let clip = Bounds::from_corners(
                                        point(at, inner.origin.y),
                                        inner.bottom_right(),
                                    );
                                    window.with_content_mask(
                                        Some(ContentMask { bounds: clip }),
                                        |window| render(window, inner, &encoded),
                                    );
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(at - px(1.), inner.origin.y),
                                            size(px(2.), inner.size.height),
                                        ),
                                        line,
                                    ));
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(
                                                at - px(4.),
                                                inner.origin.y + inner.size.height / 2. - px(16.),
                                            ),
                                            size(px(8.), px(32.)),
                                        ),
                                        line,
                                    ));
                                }
                            }
                            if grid && zoom >= 8. {
                                let clipped = image.intersect(&inner);
                                let x0 =
                                    ((clipped.origin.x - image.origin.x) / px(zoom)).floor() as u32;
                                let y0 =
                                    ((clipped.origin.y - image.origin.y) / px(zoom)).floor() as u32;
                                let x1 = x0 + (f32::from(clipped.size.width) / zoom).ceil() as u32;
                                let y1 = y0 + (f32::from(clipped.size.height) / zoom).ceil() as u32;
                                for x in x0..=x1 {
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(
                                                image.origin.x + px(x as f32 * zoom),
                                                clipped.origin.y,
                                            ),
                                            size(px(1.), clipped.size.height),
                                        ),
                                        checker,
                                    ));
                                }
                                for y in y0..=y1 {
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(
                                                clipped.origin.x,
                                                image.origin.y + px(y as f32 * zoom),
                                            ),
                                            size(clipped.size.width, px(1.)),
                                        ),
                                        checker,
                                    ));
                                }
                            }
                        });
                    },
                )
                .size_full(),
            )
    }
}
