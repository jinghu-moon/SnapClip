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
    ID2D1Bitmap,
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
    /// `.preview-frame` checkerboard tile: `… 50% / 16px 16px` (`:111`).
    pub checker_tile: i32,
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
    checker_tile: 16,
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
    /// The preview card's checkerboard, the two colours of `:111`.
    pub checker_light: ID2D1SolidColorBrush,
    pub checker_dark: ID2D1SolidColorBrush,
    /// The surface the field rules composite their fill over. Kept so `paint` can ask the pure
    /// rule for colours without being handed the palette again.
    surface_rgba: [u8; 4],
    /// Every colour the field rule can name, paired with its brush. The rule decides *which*
    /// colour a state gets; this only says how to paint it. Both sides are built from the same
    /// `field_colours` call, so a change to a state's colour reaches the painter by construction
    /// rather than by someone remembering to edit a second `match`.
    field_brushes: [([u8; 4], ID2D1SolidColorBrush); 3],
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
            checker_light: brush(target, CHECKER_LIGHT)?,
            checker_dark: brush(target, CHECKER_DARK)?,
            surface_rgba: palette.surface,
            field_brushes: [
                (focused.border, brush(target, focused.border)?),
                (error.border, brush(target, error.border)?),
                (warning.border, brush(target, warning.border)?),
            ],
        })
    }

    /// The surface the field rules composite over.
    pub fn surface_rgba(&self) -> [u8; 4] {
        self.surface_rgba
    }

    /// The brush for a colour the field rule produced, or `None` when the rule asked for nothing
    /// visible. The colour is the *only* thing consulted, so the rule stays authoritative: an
    /// idle field is transparent because `field_colours` says so, not because a second `match`
    /// here happens to agree.
    pub fn field_brush(&self, rgba: [u8; 4]) -> Option<&ID2D1SolidColorBrush> {
        if rgba[3] == 0 {
            return None;
        }
        self.field_brushes
            .iter()
            .find(|(colour, _)| *colour == rgba)
            .map(|(_, brush)| brush)
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

/// `box-shadow: inset 0 0 0 1px var(--line)` (`:92`) — the sheet's own edge.
pub const PANEL_INNER_STROKE_DIP: f32 = 1.0;

/// The radius our own inner stroke is drawn at. The window's real corner belongs to the compositor
/// and is about eight dips (`docs/42` D13); drawing our stroke at the same radius makes it follow
/// the edge the user can actually see, instead of being clipped by it.
pub const PANEL_CORNER_APPROX_DIP: i32 = 8;

/// The preview card's checkerboard colours: `#32363e` and `#21242a` (`:111`).
pub const CHECKER_LIGHT: [u8; 4] = [50, 54, 62, 255];
pub const CHECKER_DARK: [u8; 4] = [33, 36, 42, 255];

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
    /// The captured picture and its own pixel size, drawn into the preview card.
    ///
    /// The bitmap is built by the caller because only the caller knows whether the pixels are the
    /// capture or the development entry's synthetic stand-in.
    pub preview: Option<(&'a ID2D1Bitmap, u32, u32)>,
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

        // The sheet's own edge: `inset 0 0 0 1px var(--line)` (`:92`). Drawn whether or not we fill
        // the surface, because this edge is ours and the material behind it is the compositor's.
        // Without it the panel reads as a rectangle of colour with no boundary, which is what the
        // first screenshot of this window showed.
        let edge = (PANEL_INNER_STROKE_DIP * self.scale).round() as i32;
        let stroke = PANEL_INNER_STROKE_DIP * self.scale;
        painter.stroke_round(
            Rect::new(
                edge,
                edge,
                canvas.right - edge,
                canvas.bottom - edge,
            ),
            (PANEL_CORNER_APPROX_DIP as f32 * self.scale).round() as i32,
            &self.brushes.line,
            stroke,
        );

        let line = 1.0 * self.scale;
        painter.line(
            (0, self.rects.divider_y),
            (self.rects.header.right, self.rects.divider_y),
            &self.brushes.line,
            line,
        );

        // The card's checkerboard: `repeating-conic-gradient(#32363e 0% 25%, #21242a 0% 50%) 50% /
        // 16px 16px` (`:111`). It is what says "this area is a canvas" rather than "this area is
        // unfinished", and it is the single largest difference between our sheet and the design.
        //
        // It goes down **before** the card's own stroke, and that order is the fix for a defect a
        // screenshot caught: the tiles are square and reach the card's edge, so painting them after
        // the stroke erased the rounded border at all four corners.
        {
            let tile = (self.metrics.checker_tile as f32 * self.scale).round() as i32;
            let preview = self.rects.preview;
            if tile > 0 {
                let mut ty = preview.top;
                let mut row = 0i32;
                while ty < preview.bottom {
                    let mut tx = preview.left;
                    let mut col = 0i32;
                    while tx < preview.right {
                        let brush = if (row + col) % 2 == 0 {
                            &self.brushes.checker_light
                        } else {
                            &self.brushes.checker_dark
                        };
                        painter.fill_round(
                            Rect::new(
                                tx,
                                ty,
                                (tx + tile).min(preview.right),
                                (ty + tile).min(preview.bottom),
                            ),
                            0,
                            brush,
                        );
                        tx += tile;
                        col += 1;
                    }
                    ty += tile;
                    row += 1;
                }
            }
        }

        // The captured picture, between the checkerboard and the card's boundary: the board says
        // "canvas", the picture is what the canvas holds, and the boundary stays on top of both.
        if let Some((bitmap, source_w, source_h)) = self.preview {
            let card = self.rects.preview;
            let (x, y, right, bottom) = fit_preview(
                (source_w as f32, source_h as f32),
                (
                    card.left as f32,
                    card.top as f32,
                    card.right as f32,
                    card.bottom as f32,
                ),
            );
            let dest = Rect::new(
                x.round() as i32,
                y.round() as i32,
                right.round() as i32,
                bottom.round() as i32,
            );
            if dest.width() > 0 && dest.height() > 0 {
                painter.draw_bitmap_at(bitmap, dest);
            }
        }

        // The card's boundary, drawn over the checkerboard so its rounded corners stay visible.
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

        // Both field frames, in the mockup's order: the path row first and the name row second
        // (`:542`, `:556`). Each child window sits inside its own rect, inset by `EDIT_INSET_DIP`,
        // and paints its own background with the same resolved fill, so the rounded corner here is
        // the only corner the user sees (`field.rs`).
        //
        // Everything about these frames comes from `field_colours`: which state has a border, which
        // has an inner ring, and what colour each is. The painter holds no opinion of its own, so
        // changing a rule changes the picture and the tests with it. Both rows run this one loop,
        // which is what keeps the second field from becoming a copy of the first.
        let field_radius = (crate::panel::INPUT_RADIUS_DIP * self.scale).round() as i32;
        // Only the name field is wired to focus so far, so the path and size rows are painted idle
        // until the stage that gives them child windows of their own. The size row is two cells
        // rather than one slot (`:235`, `:246`), which is why the loop runs over rects: every field
        // still goes through this one block.
        let size_width = crate::panel::field::size_cell_rect(
            self.metrics,
            self.rects,
            self.scale,
            crate::panel::field::SizeCell::Width,
        );
        let size_height = crate::panel::field::size_cell_rect(
            self.metrics,
            self.rects,
            self.scale,
            crate::panel::field::SizeCell::Height,
        );
        for (field, state) in [
            (
                crate::panel::field_rect(
                    self.metrics,
                    self.rects,
                    self.scale,
                    crate::panel::Row::Path,
                ),
                crate::panel::FieldState::Idle,
            ),
            (
                crate::panel::field_rect(
                    self.metrics,
                    self.rects,
                    self.scale,
                    crate::panel::Row::Name,
                ),
                self.field_state,
            ),
            (size_width, crate::panel::FieldState::Idle),
            (size_height, crate::panel::FieldState::Idle),
        ] {
            let colours = crate::panel::field_colours(state, self.brushes.surface_rgba());
            painter.fill_round(field, field_radius, &self.brushes.field_fill);

            // `:167` idle is transparent, so this branch simply does not run there.
            if let Some(border) = self.brushes.field_brush(colours.border) {
                painter.stroke_round(field, field_radius, border, line);
            }
            // `:169` — focus adds a one dip ring inside the border, which the native control cannot
            // draw for itself.
            if let Some(ring_brush) = colours.inset.and_then(|c| self.brushes.field_brush(c)) {
                let ring = (crate::panel::EDIT_INSET_DIP * self.scale).round() as i32;
                painter.stroke_round(
                    Rect::new(
                        field.left + ring,
                        field.top + ring,
                        field.right - ring,
                        field.bottom - ring,
                    ),
                    (field_radius - ring).max(0),
                    ring_brush,
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
    paint_to_dc_with_preview(factory, dwrite, hdc, area, dpi, fill_surface, field_state, None)
}

/// [`paint_to_dc_with_state`] with a captured picture for the preview card. The old signature
/// delegates here with `None`, so nothing that already calls it changes.
#[allow(clippy::too_many_arguments)]
pub fn paint_to_dc_with_preview(
    factory: &ID2D1Factory,
    dwrite: &IDWriteFactory,
    hdc: HDC,
    area: Rect,
    dpi: f32,
    fill_surface: bool,
    field_state: crate::panel::FieldState,
    preview: Option<(&[u8], u32, u32)>,
) -> Result<()> {
    // DPI 96 on the target, deliberately: `rects(scale)` below already converts every dip to a
    // pixel, so a target carrying the real DPI would scale those pixels a second time. That is
    // exactly what shipped once: at 150% the panel drew at 1.5 x its intended size, the preview
    // card left the window and the form column collapsed to a sliver. Text is unaffected because
    // `title_format` is handed the real `dpi` and sets an already scaled font size.
    let target = unsafe { factory.CreateDCRenderTarget(&properties(96.0)) }?;
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
    let preview = match preview {
        Some((bgra, w, h)) => Some((painter.bgra_bitmap(w, h, bgra)?, w, h)),
        None => None,
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
        preview: preview.as_ref().map(|(b, w, h)| (b, *w, *h)),
    }
    .paint(&painter);
    unsafe { target.EndDraw(None, None)? };
    Ok(())
}

/// Paint onto an existing render target. Used by the tests, which own a bitmap target and read its
/// pixels back, and available to any caller that already has a target.
///
/// The target must be at DPI 96. Geometry is in pixels here — `rects(scale)` has already converted
/// every dip — so a target carrying the real DPI would scale it again; see `paint_to_dc_with_state`.
/// Pass the real `dpi` anyway: it sizes the text and drives the layout scale.
pub fn paint_to_target(
    target: &ID2D1RenderTarget,
    dwrite: &IDWriteFactory,
    dpi: f32,
    fill_surface: bool,
) -> Result<FrameRects> {
    paint_to_target_with_preview(target, dwrite, dpi, fill_surface, None)
}

/// [`paint_to_target`] with a captured picture for the preview card. The old signature delegates
/// here with `None`, so no existing caller or bitmap test has to change.
pub fn paint_to_target_with_preview(
    target: &ID2D1RenderTarget,
    dwrite: &IDWriteFactory,
    dpi: f32,
    fill_surface: bool,
    preview: Option<(&[u8], u32, u32)>,
) -> Result<FrameRects> {
    let brushes = FrameBrushes::new(target, &DEMO_PALETTE)?;
    let scale = (dpi / 96.0).max(0.5);
    let metrics = DEMO;
    let rects = metrics.rects(scale);
    let painter = PanelPainter {
        context: target,
        text_factory: dwrite,
    };
    // The bitmap is made here, on the target that will draw it: the pixels arrive from the caller
    // (the capture, or the development entry's stand-in) but the device object belongs to the target.
    let preview = match preview {
        Some((bgra, w, h)) => Some((painter.bgra_bitmap(w, h, bgra)?, w, h)),
        None => None,
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
        preview: preview.as_ref().map(|(b, w, h)| (b, *w, *h)),
    }
    .paint(&painter);
    unsafe { target.EndDraw(None, None)? };
    Ok(rects)
}

#[cfg(test)]
mod tests {
    /// The directory row is the one above the name row, one row pitch apart, at every scale.
    #[test]
    fn the_directory_row_sits_above_the_name_row() {
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let rects = DEMO.rects(scale);
            let path = crate::panel::path_field_rect(&DEMO, &rects, scale);
            let name = crate::panel::name_field_rect(&DEMO, &rects, scale);
            assert!(
                path.top < name.top,
                "the directory row must be above the name row at {scale}: path.top={} name.top={}",
                path.top,
                name.top
            );
            let expected = (DEMO.row_pitch() as f32 * scale).round() as i32;
            assert_eq!(
                name.top - path.top,
                expected,
                "the two rows must be one row pitch apart at {scale}"
            );
        }
    }

    /// An idle directory field is filled and carries no visible border, by position and colour.
    /// A field slot while idle: filled with `--fill` over the surface, and with no border anywhere on
    /// its top edge or its left edge. The design keeps the border transparent until the field is
    /// focused (`:167`), so an idle field is fill only, and a test that checked only the fill would
    /// pass for a field that had grown a border.
    fn assert_idle_field(px: &Bgra8, field: snapclip_model::geometry::Rect, what: &str) {
        assert_px(
            px,
            field.left + 40,
            field.top + 8,
            fill_rgb(41, 41, 46),
            3,
            what,
        );
        // The three colours a border could be, taken from the rule itself rather than copied.
        let border_px: Vec<Px> = [
            crate::panel::field_colours(crate::panel::FieldState::Focused, DEMO_PALETTE.surface).border,
            crate::panel::field_colours(crate::panel::FieldState::Error, DEMO_PALETTE.surface).border,
            crate::panel::field_colours(crate::panel::FieldState::Warning, DEMO_PALETTE.surface).border,
        ]
        .iter()
        .map(|b| Px { b: b[2], g: b[1], r: b[0], a: b[3] })
        .collect();
        for x in field.left..field.right {
            assert_not_any(px, x, field.top, &border_px, 24, what);
        }
        for y in field.top..field.bottom {
            assert_not_any(px, field.left, y, &border_px, 24, what);
        }
    }

    #[test]
    fn the_directory_field_is_filled_and_has_no_border_when_idle() {
        let canvas = Canvas::new(920, 520);
        canvas.render();
        let px = canvas.pixels();
        let rects = DEMO.rects(1.0);
        let field = crate::panel::path_field_rect(&DEMO, &rects, 1.0);
        assert_idle_field(&px, field, "an idle directory field");
    }

    #[test]
    fn the_size_cells_are_filled_and_have_no_border_when_idle() {
        let canvas = Canvas::new(920, 520);
        canvas.render();
        let px = canvas.pixels();
        let rects = DEMO.rects(1.0);
        for (cell, what) in [
            (crate::panel::SizeCell::Width, "an idle width cell"),
            (crate::panel::SizeCell::Height, "an idle height cell"),
        ] {
            let field = crate::panel::size_cell_rect(&DEMO, &rects, 1.0, cell);
            assert_idle_field(&px, field, what);
        }
    }
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

    /// The regression this panel actually shipped with. At 150% the drawing target carried the
    /// real DPI while the geometry was already in pixels, so everything drew 1.5x too large: the
    /// preview card left the window and the form column collapsed to a sliver. Every other bitmap
    /// test here ran at 96, where the two conversions coincide, which is exactly why they were all
    /// green while the window was wrong.
    #[test]
    fn the_panel_fits_its_window_at_one_hundred_and_fifty_percent() {
        let scale = 1.5f32;
        let px = |dip: i32| (dip as f32 * scale).round() as i32;
        let canvas = Canvas::new_at(px(920), px(520), 144.0);
        canvas.render();
        let view = canvas.pixels();

        // The divider sits at the header's height, not at that height scaled twice.
        let divider = px(44);
        assert!(
            (divider - 2..=divider + 2).any(|y| !close(view.pixel(px(200), y), surface_px(), 6)),
            "the divider must land at {divider} px, not at {}",
            44.0 * scale * scale
        );

        // The preview card's right edge is inside the window, and past it is still surface.
        let card_right = px(16 + 556);
        assert!(
            (card_right - 3..=card_right + 3).any(|x| !close(view.pixel(x, px(300)), surface_px(), 6)),
            "the preview card's right edge must be near {card_right} px"
        );
        assert!(
            close(view.pixel(canvas.width - 6, px(300)), surface_px(), 6),
            "nothing but the sheet's own edge may be drawn at the far right of the window"
        );

        // The form column is present: the first field's fill is drawn between the card and the
        // padding, rather than being pushed off the window.
        let fill = Px {
            b: 46,
            g: 41,
            r: 41,
            a: 255,
        };
        assert!(
            close(
                view.pixel(px(584 + 40), px(44 + 4 + 22 + 6 + 16)),
                fill,
                8
            ),
            "the first field must be drawn in the form column, not collapsed to a sliver"
        );
    }

    /// A memory DC with a top-down 32-bit DIB behind it: the same drawing code the window runs,
    /// with pixels we can read back.
    struct Canvas {
        dc: windows::Win32::Graphics::Gdi::HDC,
        bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
        bits: *mut core::ffi::c_void,
        width: i32,
        height: i32,
        /// The DPI the drawing is told it is at. The target itself is always DPI 96, because the
        /// geometry handed to it is already in pixels; see `paint_to_dc_with_state`.
        dpi: f32,
    }

    impl Canvas {
        fn new(width: i32, height: i32) -> Self {
            Self::new_at(width, height, 96.0)
        }

        fn new_at(width: i32, height: i32, dpi: f32) -> Self {
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
                    dpi,
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
                self.dpi,
                true,
                field_state,
            )
            .expect("the frame paints");
        }

        /// Render with a captured picture in the preview card.
        fn render_preview(&self, bgra: &[u8], w: u32, h: u32) {
            let factory = factory().expect("a Direct2D factory");
            let dwrite = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_ISOLATED) }
                .expect("a DirectWrite factory");
            paint_to_dc_with_preview(
                &factory,
                &dwrite,
                self.dc,
                Rect::new(0, 0, self.width, self.height),
                self.dpi,
                true,
                crate::panel::FieldState::Idle,
                Some((bgra, w, h)),
            )
            .expect("the frame paints with a preview");
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

    /// **A visual assertion must name a position and a colour.**
    ///
    /// Sampling "somewhere near here there is a pixel that is not one of the fills" is what let three
    /// bitmap assertions in this module pass while the thing they were named after was broken. The
    /// rule is that a visual assertion states *where* and *what*, and that a mutation can make it
    /// fail: change the colour, change the order, or delete the element.
    fn assert_px(px: &Bgra8, x: i32, y: i32, want: Px, tolerance: i32, what: &str) {
        let seen = px.pixel(x, y);
        assert!(
            close(seen, want, tolerance),
            "{what}: at ({x}, {y}) expected {want:?}, saw {seen:?}"
        );
    }

    /// The negative form, for "this place must be none of these colours" — the shape the idle field's
    /// border assertion takes, where the edge must not carry any of the three border colours. A
    /// single-colour version of this was written first and had no caller: the assertion that needs it
    /// excludes three colours, not one.
    fn assert_not_any(px: &Bgra8, x: i32, y: i32, unwanted: &[Px], tolerance: i32, what: &str) {
        let seen = px.pixel(x, y);
        assert!(
            !unwanted.iter().any(|w| close(seen, *w, tolerance)),
            "{what}: at ({x}, {y}) must not be any of {unwanted:?}, but it is {seen:?}"
        );
    }

    /// A colour written the way the design writes it.
    ///
    /// The mockup states colours in rgb order and the buffer stores bgra, and this project has
    /// confused the two three times, so the swap happens here once and call sites copy the design's
    /// numbers without having to think about the order.
    fn fill_rgb(r: u8, g: u8, b: u8) -> Px {
        Px { b, g, r, a: 255 }
    }

    #[test]
    fn fill_rgb_takes_the_designs_order_and_stores_the_buffers() {
        let px = fill_rgb(41, 41, 46);
        assert_eq!(
            (px.b, px.g, px.r),
            (46, 41, 41),
            "the design writes rgb and the buffer stores bgra"
        );
    }

    /// A `--line` stroke is white at thirty percent alpha, so over a fill it reads as a blend rather
    /// than as one exact value. Computing the blend is what lets an assertion name the colour it
    /// expects instead of merely excluding the fills that lie beneath.
    fn line_over(fill: [u8; 4]) -> Px {
        let a = f32::from(DEMO_PALETTE.line[3]) / 255.0;
        let over = |under: u8| (255.0 * a + (1.0 - a) * f32::from(under)).round() as u8;
        Px {
            b: over(fill[2]),
            g: over(fill[1]),
            r: over(fill[0]),
            a: 255,
        }
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
    fn the_size_row_sits_below_the_name_row_and_splits_into_two_cells() {
        use crate::panel::field::{Row, SizeCell, field_rect, size_cell_rect};
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let rects = DEMO.rects(scale);
            let name = field_rect(&DEMO, &rects, scale, Row::Name);
            let width = size_cell_rect(&DEMO, &rects, scale, SizeCell::Width);
            let height = size_cell_rect(&DEMO, &rects, scale, SizeCell::Height);
            let one_pitch = (DEMO.row_pitch() as f32 * scale).round() as i32;
            assert_eq!(
                width.top,
                name.top + one_pitch,
                "the size row is one pitch below the name row (:163)"
            );
            assert_eq!(height.top, width.top, "both cells sit on the same row");
            assert!(width.right < height.left, "the link hub sits between the two cells (:246)");
            assert_eq!(
                height.right, rects.form.right,
                "the second cell ends at the form's right edge"
            );
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
        // Deliberately not `fill_rgb`: this is the assertion that pins the channel order, and routing it
        // through the helper would make it compare the helper with itself.
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
    ///
    /// Two exact places are checked, the top edge and the left edge, and every pixel along both of
    /// them: the earlier version sampled three points on one row, which a border on the left edge
    /// would have survived.
    #[test]
    fn an_idle_field_has_no_visible_border() {
        let canvas = Canvas::new(920, 520);
        canvas.render();
        let px = canvas.pixels();
        let rects = DEMO.rects(1.0);
        let field = crate::panel::name_field_rect(&DEMO, &rects, 1.0);
        // The fill the frame paints and the child is given: `--fill` over the surface.
        let fill = fill_rgb(41, 41, 46);
        assert!(
            !close(fill, surface_px(), 3),
            "the fill has to differ from the surface, or an idle field would be invisible"
        );
        // The three colours a border could be, from the rule itself rather than a copy of it.
        let borders = [
            crate::panel::field_colours(crate::panel::FieldState::Focused, DEMO_PALETTE.surface).border,
            crate::panel::field_colours(crate::panel::FieldState::Error, DEMO_PALETTE.surface).border,
            crate::panel::field_colours(crate::panel::FieldState::Warning, DEMO_PALETTE.surface).border,
        ];

        // The top edge, along its whole width.
        let border_px: Vec<Px> = borders
            .iter()
            .map(|b| Px { b: b[2], g: b[1], r: b[0], a: b[3] })
            .collect();
        for x in field.left..field.right {
            assert_not_any(&px, x, field.top, &border_px, 24, "the idle field's top edge");
        }
        // The left edge, down its whole height.
        for y in field.top..field.bottom {
            assert_not_any(&px, field.left, y, &border_px, 24, "the idle field's left edge");
        }
        // And the interior is still the fill, so the test above is not passing because the field
        // was never painted at all.
        let middle = px.pixel((field.left + field.right) / 2, (field.top + field.bottom) / 2);
        assert!(close(middle, fill, 6), "the idle field is a fill: saw {middle:?}");
    }

    /// The inset is what keeps the rounded corner ours: the frame's own stroke must survive around
    /// the square child. Focus is the state that draws it, so the ring is asserted here too.
    ///
    /// The border is checked on the field's edge and the ring at `EDIT_INSET_DIP` inside it, as two
    /// separate facts. The earlier version asked whether accent appeared anywhere on the outline,
    /// which stayed true with the ring deleted, so it proved nothing about the ring.
    #[test]
    fn a_focused_field_draws_its_border_and_ring() {
        let canvas = Canvas::new(920, 520);
        canvas.render_focused();
        let px = canvas.pixels();
        let rects = DEMO.rects(1.0);
        let field = crate::panel::name_field_rect(&DEMO, &rects, 1.0);
        let accent = fill_rgb(31, 117, 219);
        let ring = (crate::panel::EDIT_INSET_DIP * 1.0).round() as i32;
        let mid_x = (field.left + field.right) / 2;

        // The border, on the edge.
        assert!(
            (field.top..=field.top + 1).any(|y| close(px.pixel(mid_x, y), accent, 12)),
            "a focused field must paint the accent border on its edge (:169): saw {:?}",
            px.pixel(mid_x, field.top)
        );
        // The ring, at the inset the child window is placed by.
        assert!(
            (field.top + ring - 1..=field.top + ring + 1)
                .any(|y| close(px.pixel(mid_x, y), accent, 12)),
            "the inset ring must sit {ring} px inside the edge (:169), as the child is placed: saw {:?}",
            px.pixel(mid_x, field.top + ring)
        );
        // Between them is the fill, which is what makes them two strokes rather than one thick one.
        // The exact pixel each lands on depends on how the one dip border and the one and a half dip
        // ring round, so this counts the accent runs rather than naming rows: two runs mean two
        // strokes, and a ring that is not drawn leaves one. The earlier version sampled the row
        // `ring / 2` and called it the gap, which is the border's own row, so it stayed true with the
        // ring deleted and proved nothing.
        let runs = {
            let mut runs = 0usize;
            let mut inside = false;
            for y in field.top - 1..=field.top + 6 {
                let accent_here = close(px.pixel(mid_x, y), accent, 12);
                if accent_here && !inside {
                    runs += 1;
                }
                inside = accent_here;
            }
            runs
        };
        assert!(
            runs >= 2,
            "the focused field draws the border and the ring as two strokes; saw {runs} accent run(s)"
        );

        // The same field at 2x, where the two strokes are three pixels apart and can be told apart.
        let big = Canvas::new_at(1840, 1040, 192.0);
        big.render_focused();
        let big_px = big.pixels();
        let big_rects = DEMO.rects(2.0);
        let big_field = crate::panel::name_field_rect(&DEMO, &big_rects, 2.0);
        let big_ring = (crate::panel::EDIT_INSET_DIP * 2.0).round() as i32;
        let big_mid = (big_field.left + big_field.right) / 2;
        assert!(
            (big_field.top..=big_field.top + 1).any(|y| close(big_px.pixel(big_mid, y), accent, 12)),
            "at 2x the accent border is on the edge"
        );
        assert!(
            (big_field.top + big_ring - 1..=big_field.top + big_ring + 1)
                .any(|y| close(big_px.pixel(big_mid, y), accent, 12)),
            "at 2x the ring sits {big_ring} px inside the edge"
        );
        assert!(
            !close(big_px.pixel(big_mid, big_field.top + big_ring / 2), accent, 12),
            "at 2x the border and the ring are separate strokes, not one thick edge"
        );
    }

    /// The field's font is `--fs-input`, not the field's height. The two were confused once and the
    /// native control's text filled its whole box, which is exactly what a screenshot showed.
    #[test]
    fn the_field_font_is_the_input_size_and_not_the_field_height() {
        let metrics = DEMO;
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let px = |dip: f32| (dip * scale).round() as i32;
            assert_eq!(
                crate::panel::edit_font_height(scale),
                -px(crate::panel::EDIT_FONT_DIP),
                "the font is --fs-input (:39) at scale {scale}"
            );
            assert_ne!(
                crate::panel::edit_font_height(scale),
                -px(metrics.field_height as f32),
                "the font must not be the field's own height at scale {scale}"
            );
        }
    }

    /// The panel's own inner stroke: `box-shadow: inset 0 0 0 1px var(--line)` (`:92`). Without it
    /// the sheet has no edge of its own and reads as a rectangle of colour lying on the desktop,
    /// which is what the first real screenshot showed next to the capture panel.
    ///
    /// The stroke is drawn over the surface, so what it produces is the line colour composited on
    /// the surface and not the raw token; the assertion therefore says "not the surface" on each of
    /// the four edges and "still the surface" in the middle, which is the same shape the divider
    /// test already uses.
    #[test]
    fn the_panel_has_a_one_dip_inner_stroke() {
        let canvas = Canvas::new(920, 520);
        canvas.render();
        let px = canvas.pixels();
        let right = DEMO.panel_width - 1;
        let bottom = DEMO.panel_height() - 1;
        let mid_x = DEMO.panel_width / 2;
        let mid_y = DEMO.panel_height() / 2;

        let edge = |what: &str, samples: Vec<(i32, i32)>| {
            let seen: Vec<Px> = samples.iter().map(|(x, y)| px.pixel(*x, *y)).collect();
            assert!(
                seen.iter().any(|p| !close(*p, surface_px(), 3)),
                "the panel's {what} edge must carry the inner stroke, saw {seen:?}"
            );
        };
        edge("top", (0..=2).map(|d| (mid_x, d)).collect());
        edge("bottom", (0..=2).map(|d| (mid_x, bottom - d)).collect());
        edge("left", (0..=2).map(|d| (d, mid_y)).collect());
        edge("right", (0..=2).map(|d| (right - d, mid_y)).collect());

        // And the stroke is an edge, not a fill: somewhere with no other content must stay surface.
        // The header's own centre line is such a place; the sheet's geometric middle is not, because
        // the preview card and its checkerboard are there.
        let middle = px.pixel(mid_x, DEMO.header_height / 2);
        assert!(
            close(middle, surface_px(), 3),
            "the header is surface, not stroke: saw {middle:?}"
        );
    }

    /// The preview card is the design's checkerboard: a repeating conic gradient of two colours at
    /// sixteen dip (`:111`), which is what tells a user the area is a canvas rather than a hole.
    ///
    /// Sampling adjacent tile centres must differ, and centres a whole tile apart must match again;
    /// that is what makes this bite when nothing is drawn, because three equal samples fail the
    /// first assertion instead of passing quietly.
    #[test]
    fn the_preview_card_is_a_sixteen_dip_checkerboard() {
        let canvas = Canvas::new(920, 520);
        canvas.render();
        let px = canvas.pixels();
        let rects = DEMO.rects(1.0);
        // The design's own number (`:111`), written out rather than read from the metric: a test
        // that samples through the same constant it is guarding cannot fail when that constant
        // changes, which is the mistake the closed-loop cadence test made once already.
        let tile = 16;
        let at = |cx: i32, cy: i32| {
            px.pixel(rects.preview.left + cx * tile + tile / 2, rects.preview.top + cy * tile + tile / 2)
        };
        let light = fill_rgb(50, 54, 62);
        let dark = fill_rgb(33, 36, 42);
        let is_checker = |p: Px| close(p, light, 6) || close(p, dark, 6);

        let a = at(0, 0);
        let b = at(1, 0);
        let c = at(2, 0);
        let d = at(0, 1);
        assert!(is_checker(a), "the first tile is one of the two checker colours: saw {a:?}");
        assert!(is_checker(b), "the second tile is one of the two checker colours: saw {b:?}");
        assert!(
            !close(a, b, 6),
            "adjacent tiles must differ, saw {a:?} and {b:?} — a flat fill means no checkerboard"
        );
        assert!(close(a, c, 6), "a tile two steps along is the same colour again: {a:?} vs {c:?}");
        assert!(!close(a, d, 6), "the tile below is the other colour: {a:?} vs {d:?}");
    }

    /// The card's border must survive its own checkerboard. The tiles are square and reach the card's
    /// edge, so drawing them after the stroke erased the rounded border at all four corners — a
    /// screenshot found it, and this is what stops it coming back.
    ///
    /// Each sample sits just past the corner arc, where the stroke runs straight along the edge; with
    /// the order reversed the sample is a checker colour instead of the line.
    #[test]
    fn the_card_border_is_drawn_over_the_checkerboard() {
        let canvas = Canvas::new(920, 520);
        canvas.render();
        let px = canvas.pixels();
        let rects = DEMO.rects(1.0);
        let card = rects.preview;
        let past = DEMO.card_radius + 2;
        // The two tile colours are read from the card itself rather than copied as literals, so this
        // assertion cannot drift from the painter. What a border must not be is any of the three fills
        // underneath it: the surface, or either tile. The stroke is `--line` at partial alpha, so it
        // reads as a blend and has no single exact value to name; naming the fills it must not be is
        // what makes this bite. Painting the border with a tile brush puts a named fill on the edge,
        // and so does drawing the tiles after the stroke.
        let sample_y = card.top + 24;
        let tile_a = px.pixel(card.left + 8, sample_y);
        let tile_b = px.pixel(card.left + 24, sample_y);
        assert!(
            !close(tile_a, tile_b, 6),
            "the two tiles sampled must differ, or this is not a checkerboard: {tile_a:?} vs {tile_b:?}"
        );
        let samples = [
            ("top, past the left corner", (card.left + past, card.top), 1),
            ("top, past the right corner", (card.right - past, card.top), 1),
            ("bottom, past the left corner", (card.left + past, card.bottom), -1),
            ("bottom, past the right corner", (card.right - past, card.bottom), -1),
        ];
        // What the edge must be, named exactly: `--line` is white at thirty percent alpha, so over a
        // tile it is a blend, and either tile may lie under any given sample. Excluding the fills was
        // not enough: a border painted with a tile brush, or tiles painted after the stroke, still
        // found *some* pixel near the edge that was not a fill, and the assertion passed.
        let line_on_light = line_over(CHECKER_LIGHT);
        let line_on_dark = line_over(CHECKER_DARK);
        for (what, (x, y), inward) in samples {
            let mut stroke_row = None;
            for sy in y - 1..=y + 1 {
                let seen = px.pixel(x, sy);
                if close(seen, line_on_light, 12) || close(seen, line_on_dark, 12) {
                    stroke_row = Some(sy);
                    break;
                }
            }
            let sy = match stroke_row {
                Some(sy) => sy,
                None => panic!(
                    "the card's border at {what} must be --line over a tile; \
                     rows {}-1..={}+1 saw {:?}, {:?}, {:?}",
                    y,
                    y,
                    px.pixel(x, y - 1),
                    px.pixel(x, y),
                    px.pixel(x, y + 1)
                ),
            };
            if close(px.pixel(x, sy), line_on_light, 12) {
                assert_px(&px, x, sy, line_on_light, 12, what);
            } else {
                assert_px(&px, x, sy, line_on_dark, 12, what);
            }
            // Just inside the stroke the card is checkered, so the edge is a stroke and not the fill.
            // Which way is inside depends on which edge this is.
            let inner = px.pixel(x, y + 2 * inward);
            assert!(
                close(inner, tile_a, 6) || close(inner, tile_b, 6),
                "inside the card's border at {what} the checkerboard must show: saw {inner:?}"
            );
        }
    }
    /// The preview must show the captured picture, not a placeholder.
    ///
    /// A four quadrant image, red at the top left and blue at the bottom right, is handed in as the
    /// pixels a capture would produce. The positions are derived from `fit_preview`, which is the
    /// function that decides where the picture goes, and each assertion names a position and a
    /// colour: "some pixel somewhere is red" would pass with the picture drawn in the wrong place.
    #[test]
    fn the_panel_preview_shows_the_captured_picture_and_not_a_placeholder() {
        let (w, h) = (200u32, 100u32);
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let top_left = x < w / 2 && y < h / 2;
                let bottom_right = x >= w / 2 && y >= h / 2;
                let (r, g, b) = if top_left {
                    (255, 0, 0)
                } else if bottom_right {
                    (0, 0, 255)
                } else {
                    (0, 128, 0)
                };
                // The buffer is BGRA, like the capture's.
                bgra[i] = b;
                bgra[i + 1] = g;
                bgra[i + 2] = r;
                bgra[i + 3] = 255;
            }
        }

        let canvas = Canvas::new(920, 520);
        canvas.render_preview(&bgra, w, h);
        let px = canvas.pixels();

        let card = DEMO.rects(1.0).preview;
        let (left, top, ..) = fit_preview(
            (w as f32, h as f32),
            (
                card.left as f32,
                card.top as f32,
                card.right as f32,
                card.bottom as f32,
            ),
        );
        // The picture is not enlarged, so source pixels map one to one from the fitted origin.
        let red = fill_rgb(255, 0, 0);
        let blue = fill_rgb(0, 0, 255);
        let at = |sx: i32, sy: i32| (left.round() as i32 + sx, top.round() as i32 + sy);
        let (rx, ry) = at(20, 20);
        let (bx, by) = at(w as i32 - 20, h as i32 - 20);
        assert_px(&px, rx, ry, red, 12, "the picture's top left quadrant is where the capture put it");
        assert_px(&px, bx, by, blue, 12, "the picture's bottom right quadrant is where the capture put it");
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

    /// [`PanelRenderer::paint`] with a captured picture for the preview card.
    ///
    /// Additive on purpose: `paint` keeps its signature and delegates with no picture, so the
    /// window that has nothing to show and every existing bitmap test are untouched.
    pub fn paint_with_preview(
        &self,
        hdc: *mut core::ffi::c_void,
        area: Rect,
        dpi: f32,
        fill_surface: bool,
        field_state: crate::panel::FieldState,
        preview: Option<(&[u8], u32, u32)>,
    ) -> Result<()> {
        paint_to_dc_with_preview(
            &self.factory,
            &self.dwrite,
            HDC(hdc),
            area,
            dpi,
            fill_surface,
            field_state,
            preview,
        )
    }
}

/// Where an image of `source` (width, height) lands inside `card` (left, top, right, bottom):
/// centred, scaled to fit, and never enlarged past its own size.
///
/// Pure arithmetic, and expressed as plain numbers rather than whichever rectangle type the
/// painter happens to use, so the placement can be checked without a device context. This is the
/// only part of drawing the preview that can be wrong silently: an image drawn at the wrong
/// scale still looks like an image.
pub fn fit_preview(source: (f32, f32), card: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    let (source_w, source_h) = source;
    let (left, top, right, bottom) = card;
    let card_w = (right - left).max(0.0);
    let card_h = (bottom - top).max(0.0);
    if source_w <= 0.0 || source_h <= 0.0 || card_w <= 0.0 || card_h <= 0.0 {
        // Nothing to draw: a degenerate rectangle at the card's origin, so callers need no branch.
        return (left, top, left, top);
    }
    let scale = (card_w / source_w).min(card_h / source_h).min(1.0);
    let drawn_w = source_w * scale;
    let drawn_h = source_h * scale;
    let x = left + (card_w - drawn_w) / 2.0;
    let y = top + (card_h - drawn_h) / 2.0;
    (x, y, x + drawn_w, y + drawn_h)
}

#[cfg(test)]
mod preview_fit_tests {
    use super::fit_preview;

    /// A wide image fills the card's width and is centred vertically.
    #[test]
    fn a_wide_image_touches_the_sides_and_is_centred() {
        let (x, y, right, bottom) = fit_preview((1000.0, 500.0), (0.0, 0.0, 400.0, 300.0));
        assert_eq!((x, right), (0.0, 400.0), "the width is the binding constraint");
        assert_eq!(y, 50.0, "leftover height is split evenly");
        assert_eq!(bottom, 250.0);
    }

    /// A tall image fills the card's height instead.
    #[test]
    fn a_tall_image_touches_the_top_and_bottom() {
        let (x, y, right, bottom) = fit_preview((300.0, 900.0), (0.0, 0.0, 400.0, 300.0));
        assert_eq!((y, bottom), (0.0, 300.0), "the height is the binding constraint");
        assert_eq!((x, right), (150.0, 250.0), "leftover width is split evenly");
    }

    /// An image smaller than the card is not blown up: enlarging a screenshot would misrepresent
    /// what is being saved.
    #[test]
    fn a_small_image_is_centred_at_its_own_size() {
        let (x, y, right, bottom) = fit_preview((100.0, 100.0), (0.0, 0.0, 400.0, 300.0));
        assert_eq!((x, y, right, bottom), (150.0, 100.0, 250.0, 200.0));
    }

    /// The card's origin is respected, not assumed to be zero.
    #[test]
    fn the_placement_is_relative_to_the_card_origin() {
        let (x, y, right, bottom) = fit_preview((200.0, 100.0), (16.0, 48.0, 416.0, 348.0));
        assert_eq!((x, y), (116.0, 148.0), "centred inside a card that starts at its own corner");
        assert_eq!((right, bottom), (316.0, 248.0));
    }

    /// A degenerate source or card yields an empty rectangle rather than a panic or a huge scale.
    #[test]
    fn a_degenerate_input_yields_nothing_to_draw() {
        assert_eq!(fit_preview((0.0, 10.0), (0.0, 0.0, 400.0, 300.0)), (0.0, 0.0, 0.0, 0.0));
        assert_eq!(fit_preview((10.0, 10.0), (5.0, 5.0, 5.0, 5.0)), (5.0, 5.0, 5.0, 5.0));
    }

}
