use super::{SegmentState, segment_outer_radius, segment_radius, segment_slide};
use snapclip_model::geometry::Rect;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{
    D2D1_ELLIPSE, D2D1_ROUNDED_RECT, ID2D1DeviceContext, ID2D1HwndRenderTarget,
    ID2D1SolidColorBrush,
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
        }
    };
}

impl_paint_target!(ID2D1DeviceContext);
impl_paint_target!(ID2D1HwndRenderTarget);

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
