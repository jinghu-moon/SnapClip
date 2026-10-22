use super::{SegmentState, segment_outer_radius, segment_radius, segment_slide};
use snapclip_model::geometry::Rect;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT,
    D2D_SIZE_U, D2D_RECT_F,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_INTERPOLATION_MODE, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES,
    D2D1_ELLIPSE, D2D1_ROUNDED_RECT, ID2D1Bitmap, ID2D1DCRenderTarget, ID2D1DeviceContext,
    ID2D1HwndRenderTarget, ID2D1RenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_TEXT_METRICS, IDWriteFactory, IDWriteTextFormat,
};
use windows::core::Result;

#[derive(Clone, Copy)]
pub struct SegmentColors<'a> {
    pub track: &'a ID2D1SolidColorBrush,
    pub selected: &'a ID2D1SolidColorBrush,
    pub hover: &'a ID2D1SolidColorBrush,
    pub focus: &'a ID2D1SolidColorBrush,
    pub text: &'a ID2D1SolidColorBrush,
    pub muted: &'a ID2D1SolidColorBrush,
}

pub trait PaintTarget {
    fn fill_round(&self, rect: &D2D1_ROUNDED_RECT, brush: &ID2D1SolidColorBrush);
    fn stroke_round(&self, rect: &D2D1_ROUNDED_RECT, brush: &ID2D1SolidColorBrush, width: f32);
    fn line(
        &self,
        from: windows_numerics::Vector2,
        to: windows_numerics::Vector2,
        brush: &ID2D1SolidColorBrush,
        width: f32,
    );
    fn fill_circle(&self, ellipse: &D2D1_ELLIPSE, brush: &ID2D1SolidColorBrush);
    fn stroke_circle(&self, ellipse: &D2D1_ELLIPSE, brush: &ID2D1SolidColorBrush, width: f32);
    fn draw_text(
        &self,
        text: &[u16],
        format: &IDWriteTextFormat,
        rect: &D2D_RECT_F,
        brush: &ID2D1SolidColorBrush,
    );
    /// Draw a bitmap with the five argument form every render target shares.
    ///
    /// The device context also has a six argument overload with a perspective transform, which is why
    /// the implementations below call through the base interface instead of on the concrete type:
    /// `ID2D1DeviceContext` derives from `ID2D1RenderTarget`, so its pointer is a valid base pointer
    /// and the shared form is reachable from all four targets.
    fn draw_bitmap(
        &self,
        bitmap: &ID2D1Bitmap,
        dest: Option<&D2D_RECT_F>,
        opacity: f32,
        mode: D2D1_BITMAP_INTERPOLATION_MODE,
        src: Option<&D2D_RECT_F>,
    );
    /// Create a bitmap from tightly packed pixels. Base form, same reason as `draw_bitmap`.
    ///
    /// The alpha mode is a parameter rather than two methods: the preview picture is opaque and
    /// ignores its alpha channel, while icons are white on transparent and are prepared
    /// premultiplied, and the macro that implements this for the four targets should carry one body.
    fn create_bitmap(
        &self,
        width: u32,
        height: u32,
        pixels: &[u8],
        pitch: u32,
        alpha_mode: D2D1_ALPHA_MODE,
    ) -> Result<ID2D1Bitmap>;
}

macro_rules! impl_paint_target {
    ($target:ty) => {
        impl PaintTarget for $target {
            fn fill_round(&self, rect: &D2D1_ROUNDED_RECT, brush: &ID2D1SolidColorBrush) {
                unsafe { self.FillRoundedRectangle(rect, brush) };
            }
            fn stroke_round(
                &self,
                rect: &D2D1_ROUNDED_RECT,
                brush: &ID2D1SolidColorBrush,
                width: f32,
            ) {
                unsafe { self.DrawRoundedRectangle(rect, brush, width, None) };
            }
            fn line(
                &self,
                from: windows_numerics::Vector2,
                to: windows_numerics::Vector2,
                brush: &ID2D1SolidColorBrush,
                width: f32,
            ) {
                unsafe { self.DrawLine(from, to, brush, width, None) };
            }
            fn fill_circle(&self, ellipse: &D2D1_ELLIPSE, brush: &ID2D1SolidColorBrush) {
                unsafe { self.FillEllipse(ellipse, brush) };
            }
            fn stroke_circle(
                &self,
                ellipse: &D2D1_ELLIPSE,
                brush: &ID2D1SolidColorBrush,
                width: f32,
            ) {
                unsafe { self.DrawEllipse(ellipse, brush, width, None) };
            }
            fn draw_text(
                &self,
                text: &[u16],
                format: &IDWriteTextFormat,
                rect: &D2D_RECT_F,
                brush: &ID2D1SolidColorBrush,
            ) {
                unsafe {
                    self.DrawText(
                        text,
                        format,
                        rect,
                        brush,
                        windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_CLIP,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            }
            fn draw_bitmap(
                &self,
                bitmap: &ID2D1Bitmap,
                dest: Option<&D2D_RECT_F>,
                opacity: f32,
                mode: D2D1_BITMAP_INTERPOLATION_MODE,
                src: Option<&D2D_RECT_F>,
            ) {
                // Through the base interface: see the trait. `$target` may be the device context,
                // whose own `DrawBitmap` takes a sixth argument.
                let base: &ID2D1RenderTarget =
                    unsafe { &*(self as *const $target as *const ID2D1RenderTarget) };
                unsafe {
                    base.DrawBitmap(
                        bitmap,
                        dest.map(|r| r as *const D2D_RECT_F),
                        opacity,
                        mode,
                        src.map(|r| r as *const D2D_RECT_F),
                    );
                }
            }
            fn create_bitmap(
                &self,
                width: u32,
                height: u32,
                pixels: &[u8],
                pitch: u32,
                alpha_mode: D2D1_ALPHA_MODE,
            ) -> Result<ID2D1Bitmap> {
                let base: &ID2D1RenderTarget =
                    unsafe { &*(self as *const $target as *const ID2D1RenderTarget) };
                let props = D2D1_BITMAP_PROPERTIES {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: alpha_mode,
                    },
                    dpiX: 96.0,
                    dpiY: 96.0,
                };
                let size = D2D_SIZE_U {
                    width,
                    height,
                };
                unsafe {
                    base.CreateBitmap(size, Some(pixels.as_ptr().cast()), pitch, &props)
                }
            }
        }
    };
}

impl_paint_target!(ID2D1DeviceContext);
impl_paint_target!(ID2D1HwndRenderTarget);
impl_paint_target!(ID2D1RenderTarget);
impl_paint_target!(ID2D1DCRenderTarget);

pub struct PanelPainter<'a> {
    pub context: &'a dyn PaintTarget,
    pub text_factory: &'a IDWriteFactory,
}

impl PanelPainter<'_> {
    pub fn fill_round(&self, rect: Rect, radius: i32, brush: &ID2D1SolidColorBrush) {
        let rounded = rounded(rect, radius);
        self.context.fill_round(&rounded, brush);
    }

    pub fn stroke_round(&self, rect: Rect, radius: i32, brush: &ID2D1SolidColorBrush, width: f32) {
        let rounded = rounded(rect, radius);
        self.context.stroke_round(&rounded, brush, width);
    }

    pub fn line(&self, from: (i32, i32), to: (i32, i32), brush: &ID2D1SolidColorBrush, width: f32) {
        self.context.line(point(from), point(to), brush, width);
    }

    pub fn fill_circle(&self, center: (f32, f32), radius: f32, brush: &ID2D1SolidColorBrush) {
        let ellipse = D2D1_ELLIPSE {
            point: windows_numerics::Vector2 {
                X: center.0,
                Y: center.1,
            },
            radiusX: radius,
            radiusY: radius,
        };
        self.context.fill_circle(&ellipse, brush);
    }

    pub fn stroke_circle(
        &self,
        center: (f32, f32),
        radius: f32,
        brush: &ID2D1SolidColorBrush,
        width: f32,
    ) {
        let ellipse = D2D1_ELLIPSE {
            point: windows_numerics::Vector2 {
                X: center.0,
                Y: center.1,
            },
            radiusX: radius,
            radiusY: radius,
        };
        self.context.stroke_circle(&ellipse, brush, width);
    }

    /// A bitmap from tightly packed BGRA rows, ready to be drawn by [`PanelPainter::draw_bitmap_at`].
    ///
    /// The pixels are treated as opaque, which is what the captured preview picture is.
    pub fn bgra_bitmap(&self, width: u32, height: u32, bgra: &[u8]) -> Result<ID2D1Bitmap> {
        self.context
            .create_bitmap(width, height, bgra, width * 4, D2D1_ALPHA_MODE_IGNORE)
    }

    /// The same, for pixels that carry their own alpha, prepared premultiplied. Icons are white on
    /// transparent and are tinted before they get here, so their alpha has to be honoured.
    pub fn bgra_bitmap_with_alpha(
        &self,
        width: u32,
        height: u32,
        bgra: &[u8],
    ) -> Result<ID2D1Bitmap> {
        self.context.create_bitmap(
            width,
            height,
            bgra,
            width * 4,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
    }

    /// Draw a bitmap into `dest`, scaled with linear interpolation.
    pub fn draw_bitmap_at(&self, bitmap: &ID2D1Bitmap, dest: Rect) {
        let rect = to_d2d(dest);
        self.context.draw_bitmap(
            bitmap,
            Some(&rect),
            1.0,
            D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
            None,
        );
    }

    pub fn draw_text(
        &self,
        text: &str,
        format: &IDWriteTextFormat,
        rect: Rect,
        brush: &ID2D1SolidColorBrush,
    ) {
        self.draw_text_rect(text, format, to_d2d(rect), brush);
    }

    pub fn draw_text_rect(
        &self,
        text: &str,
        format: &IDWriteTextFormat,
        rect: D2D_RECT_F,
        brush: &ID2D1SolidColorBrush,
    ) {
        if text.is_empty() {
            return;
        }
        let wide = text.encode_utf16().collect::<Vec<_>>();
        self.context.draw_text(&wide, format, &rect, brush);
    }

    pub fn measure_text_width(&self, text: &str, format: &IDWriteTextFormat) -> Result<f32> {
        Ok(self.measure_text(text, format)?.width)
    }

    pub fn measure_text(
        &self,
        text: &str,
        format: &IDWriteTextFormat,
    ) -> Result<DWRITE_TEXT_METRICS> {
        let wide = text.encode_utf16().collect::<Vec<_>>();
        let layout = unsafe {
            self.text_factory
                .CreateTextLayout(&wide, format, f32::INFINITY, f32::INFINITY)?
        };
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe { layout.GetMetrics(&mut metrics)? };
        Ok(metrics)
    }

    pub fn segmented(
        &self,
        state: &SegmentState,
        labels: &[&str],
        format: &IDWriteTextFormat,
        colors: SegmentColors<'_>,
        scale: f32,
    ) {
        if state.segments.is_empty() || labels.len() != state.segments.len() {
            return;
        }
        let outer = segment_outer_radius(scale);
        let inset = state.segments[0].rect.left - state.track.left;
        let inner = segment_radius(outer, inset);
        self.fill_round(state.track, outer, colors.track);

        let selected = state.segments.iter().position(|segment| segment.selected);
        if let Some(to) = selected {
            let rect = match state.slide {
                Some((from, t)) if from != to && from < state.segments.len() => {
                    segment_slide(state.segments[from].rect, state.segments[to].rect, t)
                }
                _ => state.segments[to].rect,
            };
            self.fill_round(rect, inner, colors.selected);
        }

        for (index, segment) in state.segments.iter().enumerate() {
            if !segment.disabled && !segment.selected && state.hovered == Some(index) {
                self.fill_round(segment.rect, inner, colors.hover);
            }
            self.draw_text(
                labels[index],
                format,
                segment.rect,
                if segment.disabled {
                    colors.muted
                } else if segment.selected {
                    colors.text
                } else {
                    colors.muted
                },
            );
        }

        if state.focused {
            self.stroke_round(state.track, outer, colors.focus, 2.0);
        }
    }
}

pub fn to_d2d(rect: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: rect.left as f32,
        top: rect.top as f32,
        right: rect.right as f32,
        bottom: rect.bottom as f32,
    }
}

fn rounded(rect: Rect, radius: i32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect: to_d2d(rect),
        radiusX: radius as f32,
        radiusY: radius as f32,
    }
}

fn point(point: (i32, i32)) -> windows_numerics::Vector2 {
    windows_numerics::Vector2 {
        X: point.0 as f32,
        Y: point.1 as f32,
    }
}
