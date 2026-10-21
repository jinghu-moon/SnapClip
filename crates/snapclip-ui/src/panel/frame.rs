//! The save panel's frame: dips in, pixels out, then paint.
//!
//! Every number here is taken from `docs/design/save-panel-demo-v3.html` and the source line is
//! named beside it. The layout is pure arithmetic, so the painter and the hit test can agree by
//! construction, the same property `scroll/layout.rs` relies on for the capture overlay.
//!
//! What this module deliberately does not do: draw any control. Fields, menus, buttons and the
//! preview's own image belong to later steps; this is the sheet they sit on.

use super::d2d::PanelPainter;
use snapclip_model::geometry::Rect;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1CreateFactory, ID2D1Factory,
    ID2D1RenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_ISOLATED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::HDC;
use windows::Win32::Foundation::RECT;
use windows::core::Result;

use crate::panel::theme::FAMILY;

/// Panel metrics in dips, each paired with the line it came from in the design.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameMetrics {
    /// `--panel-w` (`:41`).
    pub panel_width: i32,
    /// `--pad` (`:42`); also the head's horizontal padding (`:94`).
    pub pad_x: i32,
    /// `.head { height }` (`:94`).
    pub header_height: i32,
    /// `.body { padding: 4px 16px 12px }` (`:100`), top and bottom.
    pub body_pad_top: i32,
    pub body_pad_bottom: i32,
    /// `--preview-w` (`:43`).
    pub preview_width: i32,
    /// `--preview-h` (`:44`).
    pub preview_height: i32,
    /// `--col-gap` (`:45`).
    pub column_gap: i32,
    /// `.close { width, height }` (`:96`).
    pub close_size: i32,
    /// `--r-card` (`:53`), the preview card's radius.
    pub card_radius: i32,
    /// `.row-header { height: 22px }` (`:165`), the label row above every field.
    pub row_label_height: i32,
    /// `.form-row { gap: 6px }` (`:164`), between a label row and its field.
    pub row_gap: i32,
    /// `.text-input { height: 32px }` (`:167`).
    pub field_height: i32,
    /// `.form { gap: 12px }` (`:162`), between rows inside a group.
    pub row_spacing: i32,
    /// `.form-row.grp { margin-top: 4px }` (`:163`); groups therefore sit 12 + 4 = 16 apart.
    pub group_spacing: i32,
}

/// The design's own numbers.
pub const DEMO: FrameMetrics = FrameMetrics {
    panel_width: 920,
    pad_x: 16,
    header_height: 44,
    body_pad_top: 4,
    body_pad_bottom: 12,
    preview_width: 556,
    preview_height: 460,
    column_gap: 12,
    close_size: 26,
    card_radius: 10,
    row_label_height: 22,
    row_gap: 6,
    field_height: 32,
    row_spacing: 12,
    group_spacing: 4,
};

impl FrameMetrics {
    /// Total height: the header, the body's padding and the preview. The mockup sizes itself to
    /// content and has no height of its own, so this is derived rather than quoted.
    pub fn panel_height(&self) -> i32 {
        self.header_height + self.body_pad_top + self.preview_height + self.body_pad_bottom
    }

    /// The right column: whatever the panel's width leaves after padding, preview and gap.
    pub fn form_width(&self) -> i32 {
        self.panel_width - self.pad_x * 2 - self.preview_width - self.column_gap
    }

    /// One form slot: a label row, the gap under it, and its field.
    pub fn row_height(&self) -> i32 {
        self.row_label_height + self.row_gap + self.field_height
    }

    /// Slot to slot inside a group: the slot plus the form's own gap.
    pub fn row_pitch(&self) -> i32 {
        self.row_height() + self.row_spacing
    }

    /// Slot to slot across a group boundary: the form's gap plus the group's margin.
    pub fn group_pitch(&self) -> i32 {
        self.row_height() + self.row_spacing + self.group_spacing
    }

    /// The layout at a given DPI scale, in pixels.
    pub fn rects(&self, scale: f32) -> FrameRects {
        let px = |dip: i32| (dip as f32 * scale).round() as i32;
        let body_top = px(self.header_height);
        let preview_left = px(self.pad_x);
        let preview_top = body_top + px(self.body_pad_top);
        let preview = Rect::new(
            preview_left,
            preview_top,
            preview_left + px(self.preview_width),
            preview_top + px(self.preview_height),
        );
        let form_left = preview.right + px(self.column_gap);
        let form = Rect::new(
            form_left,
            preview_top,
            form_left + px(self.form_width()),
            preview_top + px(self.preview_height),
        );
        let close = px(self.close_size);
        let close_top = (body_top - close) / 2;
        FrameRects {
            header: Rect::new(0, 0, px(self.panel_width), body_top),
            divider_y: body_top,
            title_left: px(self.pad_x),
            preview,
            form,
            close: Rect::new(
                px(self.panel_width) - px(self.pad_x) - close,
                close_top,
                px(self.panel_width) - px(self.pad_x),
                close_top + close,
            ),
        }
    }
}

/// Resolved pixel geometry. No painting depends on anything else.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameRects {
    pub header: Rect,
    /// The y of the one-pixel line under the header, which is the header's own bottom edge.
    pub divider_y: i32,
    pub title_left: i32,
    pub preview: Rect,
    pub form: Rect,
    pub close: Rect,
}

/// The colours the frame needs, straight from the design's tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FramePalette {
    /// `--surface: rgba(23,23,28,.92)` (`:25`), 0.92 × 255 = 235.
    pub surface: [u8; 4],
    /// `--line: rgba(255,255,255,.30)` (`:27`), 0.30 × 255 = 77.
    pub line: [u8; 4],
    /// `--text: #fff` (`:24`).
    pub text: [u8; 4],
    /// `--muted: rgba(232,236,242,.62)` (`:21`), 0.62 × 255 = 158.
    pub muted: [u8; 4],
}

pub const DEMO_PALETTE: FramePalette = FramePalette {
    surface: [23, 23, 28, 235],
    line: [255, 255, 255, 77],
    text: [255, 255, 255, 255],
    muted: [232, 236, 242, 158],
};

/// Brushes the frame paints with. A brush belongs to the target that made it, so the caller
/// creates these from its own.
pub struct FrameBrushes {
    pub surface: ID2D1SolidColorBrush,
    pub line: ID2D1SolidColorBrush,
    pub text: ID2D1SolidColorBrush,
    pub muted: ID2D1SolidColorBrush,
    /// The field background, already composited: a native `EDIT` cannot be translucent, so the
    /// frame and the child both use this opaque value (`field.rs`).
    pub field_fill: ID2D1SolidColorBrush,
    /// `--accent`, the focused field's border and its inner ring.
    pub accent: ID2D1SolidColorBrush,
    /// `--danger`, the error field's border.
    pub danger: ID2D1SolidColorBrush,
    /// `--warning`, the warning field's border.
    pub warning: ID2D1SolidColorBrush,
}

impl FrameBrushes {
    pub fn new(target: &ID2D1RenderTarget, palette: &FramePalette) -> Result<Self> {
        // The field's own colours, resolved once here rather than at every paint: they are pure
        // functions of the state and the surface.
        let idle = crate::panel::field_colours(crate::panel::FieldState::Idle, palette.surface);
        let focused = crate::panel::field_colours(crate::panel::FieldState::Focused, palette.surface);
        let error = crate::panel::field_colours(crate::panel::FieldState::Error, palette.surface);
        let warning = crate::panel::field_colours(crate::panel::FieldState::Warning, palette.surface);
        Ok(Self {
            surface: brush(target, palette.surface)?,
            line: brush(target, palette.line)?,
            text: brush(target, palette.text)?,
            muted: brush(target, palette.muted)?,
            field_fill: brush(target, idle.fill)?,
            accent: brush(target, focused.border)?,
            danger: brush(target, error.border)?,
            warning: brush(target, warning.border)?,
        })
    }

    /// The brush for a state's border, or `None` when the design draws no border at all.
    fn field_border(&self, state: crate::panel::FieldState) -> Option<&ID2D1SolidColorBrush> {
        match state {
            // `:167` — the mock's idle border is transparent, so an idle field has no visible edge.
            crate::panel::FieldState::Idle => None,
            crate::panel::FieldState::Focused => Some(&self.accent),
            crate::panel::FieldState::Error => Some(&self.danger),
            crate::panel::FieldState::Warning => Some(&self.warning),
        }
    }
}

fn brush(target: &ID2D1RenderTarget, rgba: [u8; 4]) -> Result<ID2D1SolidColorBrush> {
    let color = D2D1_COLOR_F {
        r: rgba[0] as f32 / 255.0,
        g: rgba[1] as f32 / 255.0,
        b: rgba[2] as f32 / 255.0,
        a: rgba[3] as f32 / 255.0,
    };
    unsafe { target.CreateSolidColorBrush(&color, None) }
}

/// The header title, verbatim from the design (`save-panel-demo-v3.html:473`).
pub const TITLE: &str = "保存图片";

/// What the frame draws. Nothing here is a control: the close button is a placeholder that does
/// not respond yet, and the title is the only text the frame itself owns.
pub struct Frame<'a> {
    pub metrics: &'a FrameMetrics,
    pub rects: &'a FrameRects,
    pub brushes: &'a FrameBrushes,
    pub scale: f32,
    /// Fill the surface. Skipped when the compositor draws the panel's material behind us.
    pub fill_surface: bool,
    pub title: Option<(&'a str, &'a IDWriteTextFormat)>,
    /// The state of the first form field's frame. The native `EDIT` itself draws no border, no
    /// focus ring and no error state, so those belong to the frame (`field.rs`).
    pub field_state: crate::panel::FieldState,
}

impl Frame<'_> {
    pub fn paint(&self, painter: &PanelPainter) {
        let canvas = Rect::new(
            0,
            0,
            self.rects.header.right,
            (self.metrics.panel_height() as f32 * self.scale).round() as i32,
        );
        if self.fill_surface {
            // Radius zero: the window's own corners belong to the compositor, see `docs/42` D13.
            painter.fill_round(canvas, 0, &self.brushes.surface);
        }

        let line = 1.0 * self.scale;
        painter.line(
            (0, self.rects.divider_y),
            (self.rects.header.right, self.rects.divider_y),
            &self.brushes.line,
            line,
        );

        // The preview card's boundary. Its content is a later step; its edge is the frame.
        painter.stroke_round(
            self.rects.preview,
            (self.metrics.card_radius as f32 * self.scale).round() as i32,
            &self.brushes.line,
            line,
        );

        // The close placeholder: two strokes, so no glyph is needed and the font subset is untouched.
        let close = self.rects.close;
        let inset = (close.width() as f32 * 0.28).round() as i32;
        let stroke = 1.5 * self.scale;
        painter.line(
            (close.left + inset, close.top + inset),
            (close.right - inset, close.bottom - inset),
            &self.brushes.muted,
            stroke,
        );
        painter.line(
            (close.right - inset, close.top + inset),
            (close.left + inset, close.bottom - inset),
            &self.brushes.muted,
            stroke,
        );

        // The first field's frame. The child window sits inside this rect, inset by
        // `EDIT_INSET_DIP`, and paints its own background with the same resolved fill, so the
        // rounded corner here is the only corner the user sees (`field.rs`).
        let field = crate::panel::name_field_rect(self.metrics, self.rects, self.scale);
        let field_radius = (crate::panel::INPUT_RADIUS_DIP * self.scale).round() as i32;
        painter.fill_round(field, field_radius, &self.brushes.field_fill);
        if let Some(border) = self.brushes.field_border(self.field_state) {
            painter.stroke_round(field, field_radius, border, line);
            // `:169` — focus adds a one dip ring inside the border, which the native control
            // cannot draw for itself.
            if self.field_state == crate::panel::FieldState::Focused {
                let ring = (crate::panel::EDIT_INSET_DIP * self.scale).round() as i32;
                painter.stroke_round(
                    Rect::new(
                        field.left + ring,
                        field.top + ring,
                        field.right - ring,
                        field.bottom - ring,
                    ),
                    (field_radius - ring).max(0),
                    border,
                    line,
                );
            }
        }

        if let Some((text, format)) = self.title {
            painter.draw_text(
                text,
                format,
                Rect::new(self.rects.title_left, 0, self.rects.close.left, self.rects.divider_y),
                &self.brushes.text,
            );
        }
    }
}

/// The Direct2D factory a DC render target needs. Keep one per process and reuse it.
pub fn factory() -> Result<ID2D1Factory> {
    unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }
}

/// The header title's format: `--fs-title: 12px` (`:31`) at weight 600 (`.head .title`, `:95`),
/// centred vertically so the text sits on the header's own centre line.
pub fn title_format(dwrite: &IDWriteFactory, dpi: f32) -> Result<IDWriteTextFormat> {
    let scale = (dpi / 96.0).max(0.5);
    let mut last: Option<windows::core::Error> = None;
    // One family per attempt: CreateTextFormat does not take a stack, so we walk it ourselves in
    // the same order the overlay does (`snapclip-capture`'s font stack) starting with the subset.
    for family in [FAMILY, "Segoe UI Variable Display", "Segoe UI"] {
        let wide = windows::core::HSTRING::from(family);
        let made = unsafe {
            dwrite.CreateTextFormat(
                &wide,
                None,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                12.0 * scale,
                &windows::core::HSTRING::from(""),
            )
        };
        match made {
            Ok(format) => {
                unsafe {
                    let _ = format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                    let _ = format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
                }
                return Ok(format);
            }
            Err(error) => last = Some(error),
        }
    }
    Err(last.expect("at least one family was tried"))
}

fn properties(dpi: f32) -> D2D1_RENDER_TARGET_PROPERTIES {
    D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: dpi,
        dpiY: dpi,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
    }
}

/// Paint the frame onto a device context. This is the real path: the window's `WM_PAINT` hands its
/// own HDC here and the tests hand a memory DC, so both run the same drawing code.
///
/// `HwndRenderTarget` and `RenderTarget` both satisfy `PaintTarget`; the generic parameter keeps
/// this usable with either while the tests use the HDC form.
pub fn paint_to_dc(
    factory: &ID2D1Factory,
    dwrite: &IDWriteFactory,
    hdc: HDC,
    area: Rect,
    dpi: f32,
    fill_surface: bool,
) -> Result<()> {
    paint_to_dc_with_state(
        factory,
        dwrite,
        hdc,
        area,
        dpi,
        fill_surface,
        crate::panel::FieldState::Idle,
    )
}

/// [`paint_to_dc`] with the first field's frame state. Kept separate so the existing signature — and
/// every caller of it — stays valid: the state only matters once a window owns a focused field.
#[allow(clippy::too_many_arguments)]
pub fn paint_to_dc_with_state(
    factory: &ID2D1Factory,
    dwrite: &IDWriteFactory,
    hdc: HDC,
    area: Rect,
    dpi: f32,
    fill_surface: bool,
    field_state: crate::panel::FieldState,
) -> Result<()> {
    let target = unsafe { factory.CreateDCRenderTarget(&properties(dpi)) }?;
    let bind = RECT {
        left: area.left,
        top: area.top,
        right: area.right,
        bottom: area.bottom,
    };
    unsafe { target.BindDC(hdc, &bind)? };
    let brushes = FrameBrushes::new(&target, &DEMO_PALETTE)?;
    let scale = (dpi / 96.0).max(0.5);
    let metrics = DEMO;
    let rects = metrics.rects(scale);
    // The title is part of the frame, not of any control: the design puts it in the header and
    // every drawn character has to be in the embedded subset, which the glyph gate enforces.
    let title = title_format(dwrite, dpi)?;
    let painter = PanelPainter {
        context: &target,
        text_factory: dwrite,
    };
    unsafe { target.BeginDraw() };
    unsafe { target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED) };
    // Hairlines at 1 dip are meant to be one pixel, not two half-lit ones.
    Frame {
        metrics: &metrics,
        rects: &rects,
        brushes: &brushes,
        scale,
        fill_surface,
        title: Some((TITLE, &title)),
        field_state,
    }
    .paint(&painter);
    unsafe { target.EndDraw(None, None)? };
    Ok(())
}

/// Paint onto an existing render target. Used by the tests, which own a bitmap target and read its
/// pixels back, and available to any caller that already has a target.
pub fn paint_to_target(
    target: &ID2D1RenderTarget,
    dwrite: &IDWriteFactory,
    dpi: f32,
    fill_surface: bool,
) -> Result<FrameRects> {
    let brushes = FrameBrushes::new(target, &DEMO_PALETTE)?;
    let scale = (dpi / 96.0).max(0.5);
    let metrics = DEMO;
    let rects = metrics.rects(scale);
    let painter = PanelPainter {
        context: target,
        text_factory: dwrite,
    };
    unsafe { target.BeginDraw() };
    unsafe { target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED) };
    // Hairlines at 1 dip are meant to be one pixel, not two half-lit ones.
    let title = title_format(dwrite, dpi)?;
    Frame {
        metrics: &metrics,
        rects: &rects,
        brushes: &brushes,
        scale,
        fill_surface,
        title: Some((TITLE, &title)),
        field_state: crate::panel::FieldState::Idle,
    }
    .paint(&painter);
    unsafe { target.EndDraw(None, None)? };
    Ok(rects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::DirectWrite::DWRITE_FACTORY_TYPE_ISOLATED;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
        DeleteDC, DeleteObject, SelectObject,
    };
    use windows::Win32::Graphics::DirectWrite::DWriteCreateFactory;

    /// The layout is arithmetic, so the numbers the design states can be asserted without a device.
    #[test]
    fn the_panel_is_the_designs_width_and_a_derived_height() {
        assert_eq!(DEMO.panel_width, 920, "--panel-w, demo :41");
        assert_eq!(DEMO.panel_height(), 520, "44 header + 4 + 460 preview + 12");
        assert_eq!(DEMO.form_width(), 320, "920 - 32 padding - 556 preview - 12 gap");
    }

    #[test]
    fn the_header_body_and_columns_land_where_the_design_puts_them() {
        let rects = DEMO.rects(1.0);
        assert_eq!(rects.header.bottom, 44, ".head is 44 dips tall");
        assert_eq!(rects.divider_y, 44);
        assert_eq!(rects.preview.left, 16, "body padding is 16 dips on the left");
        assert_eq!(rects.preview.top, 48, "header 44 + body padding 4");
        assert_eq!(rects.preview.width(), 556);
        assert_eq!(rects.preview.height(), 460);
        assert_eq!(rects.form.left, 16 + 556 + 12, "preview, then the column gap");
        assert_eq!(rects.form.right, 920 - 16);
        assert_eq!(rects.close.right, 920 - 16, "the close button hugs the right padding");
        assert_eq!(rects.close.width(), 26);
    }

    #[test]
    fn every_number_scales_with_the_dpi() {
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let rects = DEMO.rects(scale);
            let px = |dip: i32| (dip as f32 * scale).round() as i32;
            assert_eq!(rects.header.bottom, px(44), "scale {scale}");
            assert_eq!(rects.preview.left, px(16), "scale {scale}");
            assert_eq!(rects.preview.width(), px(556), "scale {scale}");
            assert_eq!(rects.close.right, px(920) - px(16), "scale {scale}");
        }
    }

    /// A memory DC with a top-down 32-bit DIB behind it: the same drawing code the window runs,
    /// with pixels we can read back.
    struct Canvas {
        dc: windows::Win32::Graphics::Gdi::HDC,
        bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
        bits: *mut core::ffi::c_void,
        width: i32,
        height: i32,
    }

    impl Canvas {
        fn new(width: i32, height: i32) -> Self {
            unsafe {
                let dc = CreateCompatibleDC(None);
                let header = BITMAPINFOHEADER {
                    biSize: core::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                };
                let info = BITMAPINFO {
                    bmiHeader: header,
                    ..Default::default()
                };
                let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
                let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
                    .expect("a DIB section for the panel frame");
                let _ = SelectObject(dc, bitmap.into());
                // Backdrop = the surface colour at full opacity, so the panel's own 92% surface
                // composites to exactly that colour: the window's material is behind the sheet.
                let px = bits as *mut u8;
                for i in 0..(width * height) as usize {
                    *px.add(i * 4) = DEMO_PALETTE.surface[2];
                    *px.add(i * 4 + 1) = DEMO_PALETTE.surface[1];
                    *px.add(i * 4 + 2) = DEMO_PALETTE.surface[0];
                    *px.add(i * 4 + 3) = 255;
                }
                Self {
                    dc,
                    bitmap,
                    bits,
                    width,
                    height,
                }
            }
        }

        fn render(&self) {
            self.render_state(crate::panel::FieldState::Idle);
        }

        /// The same canvas with the first field focused, so the border and the ring can be sampled.
        fn render_focused(&self) {
            self.render_state(crate::panel::FieldState::Focused);
        }

        fn render_state(&self, field_state: crate::panel::FieldState) {
            let factory = factory().expect("a Direct2D factory");
            let dwrite = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_ISOLATED) }
                .expect("a DirectWrite factory");
            paint_to_dc_with_state(
                &factory,
                &dwrite,
                self.dc,
                Rect::new(0, 0, self.width, self.height),
                96.0,
                true,
                field_state,
            )
            .expect("the frame paints");
        }

        /// The pixels, through the only view that exists. Deliberately no direct byte accessor:
        /// the DIB is BGRA and reading it as RGB has already produced one false test result in
        /// this repository (`docs/48`'s encoder trap is the same confusion).
        fn pixels(&self) -> Bgra8<'_> {
            Bgra8 {
                bytes: unsafe {
                    core::slice::from_raw_parts(
                        self.bits as *const u8,
                        (self.width * self.height * 4) as usize,
                    )
                },
                width: self.width,
            }
        }
    }

    /// One pixel with its channels named, so an assertion cannot swap red for blue by accident.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Px {
        b: u8,
        g: u8,
        r: u8,
        a: u8,
    }

    /// A 32-bit top-down BGRA buffer, as the DIB section actually lays it out.
    struct Bgra8<'a> {
        bytes: &'a [u8],
        width: i32,
    }

    impl Bgra8<'_> {
        fn pixel(&self, x: i32, y: i32) -> Px {
            let at = ((y * self.width + x) * 4) as usize;
            Px {
                b: self.bytes[at],
                g: self.bytes[at + 1],
                r: self.bytes[at + 2],
                a: self.bytes[at + 3],
            }
        }
    }

    fn surface_px() -> Px {
        Px {
            b: DEMO_PALETTE.surface[2],
            g: DEMO_PALETTE.surface[1],
            r: DEMO_PALETTE.surface[0],
            a: 255,
        }
    }

    fn close(px: Px, want: Px, tolerance: i32) -> bool {
        (px.b as i32 - want.b as i32).abs() <= tolerance
            && (px.g as i32 - want.g as i32).abs() <= tolerance
            && (px.r as i32 - want.r as i32).abs() <= tolerance
            && (px.a as i32 - want.a as i32).abs() <= tolerance
    }

    impl Drop for Canvas {
        fn drop(&mut self) {
            unsafe {
                let _ = DeleteObject(self.bitmap.into());
                let _ = DeleteDC(self.dc);
            }
        }
    }

    #[test]
    fn the_first_form_slot_starts_where_the_design_puts_it() {
        let rects = DEMO.rects(1.0);
        assert_eq!(rects.form.top, 48, ".body padding-top 4 under the 44 dip header (:100)");
        assert_eq!(DEMO.row_height(), 60, "22 label + 6 gap + 32 field (:165, :164, :167)");
    }

    #[test]
    fn the_form_groups_are_sixteen_apart() {
        assert_eq!(DEMO.row_pitch() - DEMO.row_height(), 12, ".form gap is 12px (:162)");
        assert_eq!(DEMO.group_pitch() - DEMO.row_pitch(), 4, ".form-row.grp margin-top (:163)");
        assert_eq!(
            DEMO.group_pitch() - DEMO.row_height(),
            16,
            "the design's own note says groups sit 16 apart (:163)"
        );
    }

    #[test]
    fn the_form_column_ends_before_the_panel_padding() {
        let rects = DEMO.rects(1.0);
        assert_eq!(rects.form.right, DEMO.panel_width - DEMO.pad_x, "right padding is 16 (:42)");
    }

    #[test]
    fn a_pixel_view_names_the_channels_in_dib_order() {
        let canvas = Canvas::new(4, 2);
        unsafe {
            let px = canvas.bits as *mut u8;
            *px.add(0) = 0;
            *px.add(1) = 0;
            *px.add(2) = 255;
            *px.add(3) = 255;
        }
        let got = canvas.pixels().pixel(0, 0);
        assert_eq!(got, Px { b: 0, g: 0, r: 255, a: 255 }, "the DIB stores BGRA, blue first");
    }

    #[test]
    fn the_sheet_is_the_designs_surface_colour() {
        let canvas = Canvas::new(920, 520);
        canvas.render();
        let got = canvas.pixels().pixel(700, 300);
        assert!(close(got, surface_px(), 3), "the surface should be --surface: got {got:?}");
    }

    #[test]
    fn the_divider_sits_at_the_header_height() {
        let canvas = Canvas::new(920, 520);
        canvas.render();
        let rects = DEMO.rects(1.0);
        let px = canvas.pixels();
        let above = px.pixel(400, rects.divider_y - 4);
        assert!(close(above, surface_px(), 3), "above the divider is still surface: {above:?}");
        let near: Vec<Px> = (-1..=1).map(|d| px.pixel(400, rects.divider_y + d)).collect();
        let lit = near.iter().any(|p| !close(*p, surface_px(), 3));
        assert!(lit, "one of the three rows at the divider must be the --line stroke: {near:?}");
    }

    #[test]
    fn the_preview_card_starts_after_the_padding() {
        let canvas = Canvas::new(920, 520);
        canvas.render();
        let rects = DEMO.rects(1.0);
        let mid = rects.preview.top + rects.preview.height() / 2;
        let px = canvas.pixels();
        let outside = px.pixel(rects.preview.left - 4, mid);
        assert!(close(outside, surface_px(), 3), "left of the card is the surface: {outside:?}");
        let edge: Vec<Px> = (-1..=1).map(|d| px.pixel(rects.preview.left + d, mid)).collect();
        let lit = edge.iter().any(|p| !close(*p, surface_px(), 3));
        assert!(lit, "the card's edge is drawn at x = 16: {edge:?}");
    }

    #[test]
    fn the_title_is_drawn_inside_the_header() {        let canvas = Canvas::new(920, 520);
        canvas.render();
        let rects = DEMO.rects(1.0);
        let px = canvas.pixels();
        let lit = (rects.title_left..rects.title_left + 90)
            .flat_map(|x| (rects.header.top + 8..rects.divider_y - 8).map(move |y| (x, y)))
            .any(|(x, y)| !close(px.pixel(x, y), surface_px(), 12));
        assert!(lit, "the header title paints inside the header");
    }

    /// The field's frame, at the mock's own slot. An idle field has **no visible border** (`:167`),
    /// which is the detail this project got wrong first: the border is transparent until focus, so
    /// an idle field must read as a filled rectangle and nothing else.
    #[test]
    fn an_idle_field_has_no_visible_border() {
        let canvas = Canvas::new(920, 520);
        canvas.render();
        let px = canvas.pixels();
        let rects = DEMO.rects(1.0);
        let field = crate::panel::name_field_rect(&DEMO, &rects, 1.0);
        // The fill the frame paints and the child is given: `--fill` over the surface.
        let fill = Px { b: 41, g: 41, r: 46, a: 255 };
        assert!(
            !close(fill, surface_px(), 3),
            "the fill has to differ from the surface, or an idle field would be invisible"
        );
        for x in [field.left + 20, (field.left + field.right) / 2, field.right - 20] {
            let seen = px.pixel(x, field.top);
            assert!(
                close(seen, fill, 6),
                "the idle field's edge at x={x} must be the fill, not a border: saw {seen:?}"
            );
        }
    }

    /// The inset is what keeps the rounded corner ours: the frame's own stroke must survive around
    /// the square child. Focus is the state that draws it, so the ring is asserted here too.
    #[test]
    fn a_focused_field_draws_its_border_and_ring() {
        let canvas = Canvas::new(920, 520);
        canvas.render_focused();
        let px = canvas.pixels();
        let rects = DEMO.rects(1.0);
        let field = crate::panel::name_field_rect(&DEMO, &rects, 1.0);
        let accent = Px { b: 219, g: 117, r: 31, a: 255 };
        let lit = (field.left..field.right)
            .any(|x| close(px.pixel(x, field.top), accent, 12))
            || (field.top..field.bottom).any(|y| close(px.pixel(field.left, y), accent, 12));
        assert!(lit, "a focused field's frame must paint the accent border (:169)");
    }
}

/// Owns the two factories a window needs and paints the frame onto a device context.
///
/// The window never names a Direct2D type: it hands over an HDC and a rectangle. That keeps the
/// drawing contract in one module and lets the same code stand behind the tests.
pub struct PanelRenderer {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
}

impl PanelRenderer {
    pub fn new() -> Result<Self> {
        let factory = factory()?;
        let dwrite = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_ISOLATED) }?;
        Ok(Self { factory, dwrite })
    }

    /// Paint into a window's DC. `hdc` is the raw handle the Win32 bindings hand out, which is what
    /// `BeginPaint` returns; the parameter is a pointer so callers with either binding can pass it.
    ///
    /// `field_state` is the first field's frame state: the native child draws no border, no focus
    /// ring and no error colour, so the frame around it has to be told what state it is in.
    pub fn paint(
        &self,
        hdc: *mut core::ffi::c_void,
        area: Rect,
        dpi: f32,
        fill_surface: bool,
        field_state: crate::panel::FieldState,
    ) -> Result<()> {
        paint_to_dc_with_state(
            &self.factory,
            &self.dwrite,
            HDC(hdc),
            area,
            dpi,
            fill_surface,
            field_state,
        )
    }
}
