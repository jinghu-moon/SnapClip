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
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
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
    /// `.body { padding: 4px 16px 12px }` (`:100`); only the top padding is left in the body.
    pub body_pad_top: i32,
    /// The footer strip below the body: `.foot { padding: 12px 16px 12px }` (`:274`) around
    /// `--btn-h: 32px` (`:50`), so `12 + 32 + 12 = 56`.
    ///
    /// This replaces the mock's `12px` body bottom padding rather than adding to it: the footer
    /// draws its own top padding, and the body ends where the footer begins.
    pub footer_height: i32,
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
    footer_height: 56,
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
    /// Total height: the header, the body's top padding, the preview and the footer. The mockup
    /// sizes itself to content and has no height of its own, so this is derived rather than quoted.
    ///
    /// `44 + 4 + 460 + 56 = 564`. The footer's own `12px` top padding is what used to be the body's
    /// bottom padding, which is why the body no longer contributes one.
    pub fn panel_height(&self) -> i32 {
        self.header_height + self.body_pad_top + self.preview_height + self.footer_height
    }

    /// The right column: whatever the panel's width leaves after padding, preview and gap.
    pub fn form_width(&self) -> i32 {
        self.panel_width - self.pad_x * 2 - self.preview_width - self.column_gap
    }

    /// One slot of a row that carries a **text field**: its label row, the gap under it, and the
    /// field. Named for that kind of row rather than for rows in general, because the form also
    /// has rows whose content is `--row` (28, see `field::ROW_DIP`) and the two are not
    /// interchangeable: 22 + 6 + 32 = 60 against 28 (`:165`, `:164`, `:167`, `:47`).
    pub fn text_row_height(&self) -> i32 {
        self.row_label_height + self.row_gap + self.field_height
    }

    /// Slot to slot inside a group, for a text-field row: the slot plus the form's own gap.
    pub fn text_row_pitch(&self) -> i32 {
        self.text_row_height() + self.row_spacing
    }

    /// Slot to slot across a group boundary, for a text-field row: the form's gap plus the
    /// group's margin.
    pub fn text_group_pitch(&self) -> i32 {
        self.text_row_height() + self.row_spacing + self.group_spacing
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
    /// `.row-label { color: rgba(232,236,242,.74) }` (`:166`), 0.74 × 255 = 189. The row labels are
    /// a little stronger than `--muted`, which is what separates them from the hint text.
    pub label: [u8; 4],
    /// `--accent-text: rgb(94,162,245)` (`:29`) — the brighter blue the design uses for small marks
    /// on the dark surface, because the primary accent is only about 4:1 against it.
    pub accent_text: [u8; 4],
    /// `--warning: rgb(240,179,90)` (`:23`) — the colour `.status.is-warning` (`:327`), the conflict
    /// link (`:328`) and `.text-input.is-warn` (`:334`) all use.
    ///
    /// One token, one value: `field_colours` reads the same constant for a warning field's border
    /// (`panel::WARNING_RGBA`), because two yellows for two jobs is how "the warning looks wrong"
    /// starts — and it already had, when the border carried a brighter yellow taken by eye.
    pub warning: [u8; 4],
}

pub const DEMO_PALETTE: FramePalette = FramePalette {
    surface: [23, 23, 28, 235],
    line: [255, 255, 255, 77],
    text: [255, 255, 255, 255],
    muted: [232, 236, 242, 158],
    label: [232, 236, 242, 189],
    accent_text: [94, 162, 245, 255],
    warning: crate::panel::field::WARNING_RGBA,
};

/// Brushes the frame paints with. A brush belongs to the target that made it, so the caller
/// creates these from its own.
pub struct FrameBrushes {
    pub surface: ID2D1SolidColorBrush,
    pub line: ID2D1SolidColorBrush,
    pub text: ID2D1SolidColorBrush,
    pub muted: ID2D1SolidColorBrush,
    /// The row labels' own colour, a little stronger than `--muted` (`:166`).
    pub label: ID2D1SolidColorBrush,
    /// `--line-soft` (`:28`) resolved over the surface, for the size row's mode container.
    pub line_soft: ID2D1SolidColorBrush,
    /// The footer's top hairline: `rgba(255,255,255,.04)` (`:274`), resolved over the surface for
    /// the same reason as `line_soft`. Fainter on purpose — the footer is divided, not edged.
    pub footer_line: ID2D1SolidColorBrush,
    /// The field background, already composited: a native `EDIT` cannot be translucent, so the
    /// frame and the child both use this opaque value (`field.rs`).
    pub field_fill: ID2D1SolidColorBrush,
    /// `--accent`, the focused field's border and its inner ring.
    pub accent: ID2D1SolidColorBrush,
    /// `--danger`, the error field's border.
    pub danger: ID2D1SolidColorBrush,
    /// `--warning`, the warning field's border.
    pub warning: ID2D1SolidColorBrush,
    /// `--accent-text` (`:29`), the locked link hub's icon colour.
    pub accent_text: ID2D1SolidColorBrush,
    /// `--warning` (`:23`), the footer status line's warning colour (`:327`).
    pub warning_text: ID2D1SolidColorBrush,
    /// The same colour as bytes, for the icon's tint, which is arithmetic rather than a brush.
    pub accent_text_rgba: [u8; 4],
    /// `--muted` as bytes, for icons tinted on the processor rather than by a brush.
    pub muted_rgba: [u8; 4],
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
            label: brush(target, palette.label)?,
            // `--line-soft` is a translucent white, and our brushes are opaque, so it is resolved
            // over the surface here rather than left to blend: one place, and no caller has to know.
            line_soft: brush(
                target,
                crate::panel::line_soft_over_surface(palette.surface),
            )?,
            footer_line: brush(
                target,
                crate::panel::footer_line_over_surface(palette.surface),
            )?,
            field_fill: brush(target, idle.fill)?,
            accent: brush(target, focused.border)?,
            danger: brush(target, error.border)?,
            warning: brush(target, warning.border)?,
            accent_text: brush(target, palette.accent_text)?,
            warning_text: brush(target, palette.warning)?,
            accent_text_rgba: palette.accent_text,
            muted_rgba: palette.muted,
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
    /// The row labels' format (`--fs-row: 11px`, `:35`) and the segmented options' (`--fs-chip:
    /// 11px`, `:36`). The two sizes agree and the weights do not — `.row-label` is 500 (`:166`)
    /// while `.seg .opt` inherits the body's — so they are two formats rather than one.
    pub row_label: Option<&'a IDWriteTextFormat>,
    pub chip: Option<&'a IDWriteTextFormat>,
    /// The numeric face's format, for the `px` unit beside each size cell (`--fs-num`'s family,
    /// `.dim-unit`'s size). `None` falls back to [`Self::chip`] so the unit still draws if the
    /// caller did not build one.
    pub number: Option<&'a IDWriteTextFormat>,
    /// The footer buttons' formats: `.btn` is `--fs-btn: 12px` (`:38`, `:279`) at the body weight,
    /// and `.btn.start` is 600 (`:282`), which is the only difference between save and the rest.
    pub button: Option<&'a IDWriteTextFormat>,
    pub button_strong: Option<&'a IDWriteTextFormat>,
    /// The footer buttons' widths in pixels, in the order of [`crate::panel::FOOTER_BUTTONS`].
    ///
    /// A caller that also answers clicks hands them over, so the rectangles the buttons are painted
    /// in and the rectangles that decide whether a click landed are the same numbers rather than two
    /// measurements that happen to agree. `None` measures here, which is what a caller that only
    /// paints — every bitmap test — does.
    pub footer_widths: Option<[i32; 3]>,
    /// The measured width of the path row's `更改` button, in pixels.
    ///
    /// The path input gives up this width so its native control cannot sit under the button, which
    /// means the control and the painting have to be told the same measurement — the same reason the
    /// footer's widths travel. Zero means "paint a button that is no wider than its padding"; the
    /// window always hands a real number.
    pub path_change: i32,
    /// The state of every form field's frame. The native `EDIT` itself draws no border, no focus ring
    /// and no error state, so those belong to the frame (`field.rs`), and each field carries its own
    /// — one shared state is what made a focused path row draw the name row's frame.
    pub fields: crate::panel::FieldStates,
    /// The captured picture and its own pixel size, drawn into the preview card.
    ///
    /// The bitmap is built by the caller because only the caller knows whether the pixels are the
    /// capture or the development entry's synthetic stand-in.
    pub preview: Option<(&'a ID2D1Bitmap, u32, u32)>,
    /// Which quality option the segmented control marks as chosen, by index into the design's three
    /// (`:641`–`:643`). It is the one piece of state the control has so far: the options' words, the
    /// click handling and the setting it writes back belong to later stages.
    pub quality: usize,
    /// The format control's word: `.fmt-btn` and `.fmt-item` both set `font-family: var(--num)` at
    /// twelve dip (`:173`, `:182`), so the label is not the footer's format even though the size
    /// agrees — the face differs.
    pub format_label: Option<&'a IDWriteTextFormat>,
    /// Which format the button shows, by index into [`crate::panel::FMT_OPTIONS`].
    pub format: usize,
    /// Whether the menu is open. Open draws the accent border, turns the chevron over and draws the
    /// menu itself; the message handler owns the flag, and the painter only reads it.
    pub format_open: bool,
    /// The footer's one line of message, with the severity that decides its colour.
    ///
    /// The level comes from the message's own rule (`copy::ValidationCopy::level`), not from the
    /// caller's opinion: a warning that does not block a save and an error that does should not both
    /// be painted the same red. `None` draws nothing, which is the state of a panel that has not
    /// failed at anything yet.
    pub status: Option<(crate::copy::Level, &'a str)>,
    /// The status line's format (`--fs-status: 11px`, `:37`).
    pub status_format: Option<&'a IDWriteTextFormat>,
    /// Whether the token menu is open. The message handler owns the flag and the painter reads it,
    /// the same division as the format menu above.
    pub var_open: bool,
    /// The token menu's label format (`.var-item { font-size: 12px }`, `:219`).
    pub var_label: Option<&'a IDWriteTextFormat>,
    /// The three shortcut chips' widths, measured by the caller.
    ///
    /// Their words are not all the same width, so the chips are not either, and the window has to
    /// hit-test the same rectangles the painter draws. Measuring inside the paint would leave the
    /// click handler with a second measurement — which is the arrangement the footer's widths
    /// already replaced. `None` measures here, for callers that only paint.
    pub path_chips: Option<[i32; 3]>,
}

impl Frame<'_> {
    pub fn paint(&self, painter: &PanelPainter) -> Result<()> {
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

        // The close glyph, from the baked icon rather than from two strokes: the mockup draws a real
        // svg here (`.close svg { width: 14px; height: 14px }`, `:98`) and the placeholder could only
        // approximate it. The colour is the button's own (`color: var(--muted)`, `:96`).
        Self::draw_glyph(
            painter,
            crate::panel::icons::Glyph::Close,
            crate::panel::field::close_icon_rect(self.metrics, self.rects, self.scale),
            self.brushes.muted_rgba,
        )?;

        // Both field frames, in the mockup's order: the path row first and the name row second
        // (`:542`, `:556`), then the size row's two cells (`:235`, `:246`). Each child window sits
        // inside its own rect, inset by `EDIT_INSET_DIP`, and paints its own background with the same
        // resolved fill, so the rounded corner here is the only corner the user sees (`field.rs`).
        //
        // Everything about these frames comes from `field_colours`: which state has a border, which
        // has an inner ring, and what colour each is. The painter holds no opinion of its own, so
        // changing a rule changes the picture and the tests with it. Every field runs this one loop,
        // which is what keeps the second, third and fourth from becoming copies of the first.
        let field_radius = (crate::panel::INPUT_RADIUS_DIP * self.scale).round() as i32;
        // The states come from the field list, one per field: a field that is added later is drawn
        // from its own entry rather than from whatever state the window tracked before.
        for (id, state) in self.fields.iter() {
            let field = id.frame_rect(self.metrics, self.rects, self.scale, self.path_change);
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

        // The naming-variable button, inside the name field's own box (`.name-var { right: 4px;
        // top: 4px; width: 24px }`, `:213`, holding a 16-unit graphic, `:558`).
        //
        // It is painted here, on the parent, because a child window is drawn above its parent: a
        // button placed under the edit control would be invisible. The control therefore gives up
        // this strip (`name_text_rect`) and the frame still fills the whole field box, so the strip
        // reads as part of the field rather than as a gap.
        Self::draw_glyph(
            painter,
            crate::panel::icons::Glyph::Braces,
            crate::panel::field::name_wand_icon_rect(self.metrics, self.rects, self.scale),
            self.brushes.muted_rgba,
        )?;

        // The size cells' own words. Each cell is a wrapper holding a `宽` or `高` label, the number
        // the child window draws, and a `px` unit (`:237`, `:242`, `:244`, `:595`, `:605`). The
        // wrapper is painted by the loop above, which is why the cell's frame rectangle and the
        // control's rectangle are two different things here and the same thing everywhere else.
        if let (Some(word_format), Some(unit_format)) = (self.row_label, self.number.or(self.chip)) {
            for (cell, word) in [
                (crate::panel::SizeCell::Width, crate::panel::field::SIZE_CELL_WORDS[0]),
                (crate::panel::SizeCell::Height, crate::panel::field::SIZE_CELL_WORDS[1]),
            ] {
                let label =
                    crate::panel::field::size_cell_label_rect(self.metrics, self.rects, self.scale, cell);
                painter.draw_text(word, word_format, label, &self.brushes.muted);

                let unit =
                    crate::panel::field::size_cell_unit_rect(self.metrics, self.rects, self.scale, cell);
                painter.draw_text(
                    crate::panel::field::SIZE_UNIT_TEXT,
                    unit_format,
                    unit,
                    &self.brushes.muted,
                );
            }
        }

        // The quality row's segmented control: a track with a block marking the chosen option
        // (`:263`–`:265`). It is not a field — there is no child window and no border rule — so it is
        // painted here rather than inside the field loop above. The options' words are text, and
        // every drawn character has to be in the embedded subset, so they belong to the stage that
        // rebuilds it.
        let seg = crate::panel::quality_seg_rect(self.metrics, self.rects, self.scale);
        let seg_radius = (crate::panel::QUALITY_SEG_RADIUS_DIP * self.scale).round() as i32;
        painter.fill_round(seg, seg_radius, &self.brushes.field_fill);
        if self.quality < crate::panel::QUALITY_OPTIONS {
            let block =
                crate::panel::quality_option_rect(self.metrics, self.rects, self.scale, self.quality);
            let block_radius =
                (crate::panel::QUALITY_SEG_INNER_RADIUS_DIP * self.scale).round() as i32;
            painter.fill_round(block, block_radius, &self.brushes.accent);
        }

        if let Some((text, format)) = self.title {
            painter.draw_text(
                text,
                format,
                Rect::new(self.rects.title_left, 0, self.rects.close.left, self.rects.divider_y),
                &self.brushes.text,
            );
        }

        // The form's own words: one label above each row (`:542`, `:553`, `:582`, `:630`) and one
        // word inside each quality option (`:641`–`:643`). Both come from `snapclip-ui`'s producers
        // rather than from literals here, and every one of those characters is in the embedded
        // subset because the coverage gate's list calls the same producer.
        if let Some(format) = self.row_label {
            for row in crate::panel::field::FORM_ROWS {
                let rect =
                    crate::panel::field::label_rect(self.metrics, self.rects, self.scale, row);
                painter.draw_text(row.label(), format, rect, &self.brushes.label);
            }
        }
        if let Some(format) = self.chip {
            for (index, word) in crate::panel::field::QUALITY_OPTION_WORDS.iter().enumerate() {
                let rect =
                    crate::panel::quality_option_rect(self.metrics, self.rects, self.scale, index);
                // `:266` the options are `--muted`; `:267` the chosen one is `--text`.
                let brush = if index == self.quality {
                    &self.brushes.text
                } else {
                    &self.brushes.muted
                };
                painter.draw_text(word, format, rect, brush);
            }
        }

        // The after-save switches (`:572`–`:575`). That row is a header on its own, so its furniture
        // sits on the label line rather than under it. Both switches start off
        // (`aria-checked="false"`), so the resting look is an edge and `--muted` text with nothing
        // filled. The label's width and each chip's are measured: they are words, and a number
        // invented here would only be right for the font this build happens to load.
        if let (Some(row_format), Some(chip_format)) = (self.row_label, self.chip) {
            let label_width = painter
                .measure_text_width(crate::panel::field::Row::Saved.label(), row_format)?
                .round() as i32;
            let mut widths = [0i32; 2];
            for (index, word) in crate::panel::field::AFTER_CHIP_WORDS.iter().enumerate() {
                let measured = painter.measure_text_width(word, chip_format)?.round() as i32;
                widths[index] = crate::panel::after_chip_width(self.scale, measured);
            }
            // `.chip { border-radius: 999px }` (`:192`): a pill, whose radius is half its height.
            let height = (crate::panel::field::AFTER_CHIP_HEIGHT_DIP as f32 * self.scale).round();
            let radius = (height / 2.0).round() as i32;
            for (index, word) in crate::panel::field::AFTER_CHIP_WORDS.iter().enumerate() {
                let rect = crate::panel::field::after_chip_rect(
                    self.metrics,
                    self.rects,
                    self.scale,
                    label_width,
                    widths,
                    index,
                );
                painter.stroke_round(rect, radius, &self.brushes.line_soft, line);
                painter.draw_text(word, chip_format, rect, &self.brushes.muted);
            }
        }

        // The path row's two buttons and its three shortcuts (`:545`, `:546`, `:548`).
        //
        // The buttons are `.btn-ghost`: a `--fill` face with a transparent edge (`:189`), `--text`
        // for the word and `--muted` for the square one's graphic (`:205`). `--fill` over the
        // surface is the colour the fields already composite, so the ghost borrows that brush
        // instead of inventing a second one for the same value.
        //
        // The shortcuts are the same chip as the after-save switches — same height, same padding,
        // same `--line-soft` edge — because the mockup uses `.chip` for both (`:192`). Each carries
        // the folder from the baked icon set (`:1008`) and its word, so the chip is an icon and a
        // text on one line rather than centred text over a picture.
        if let (Some(_row_format), Some(chip_format)) = (self.row_label, self.chip) {
            let radius = (crate::panel::field::GHOST_RADIUS_DIP as f32 * self.scale).round() as i32;

            let measured = painter
                .measure_text_width(crate::panel::field::GHOST_WORD_CHANGE, chip_format)?
                .round() as i32;
            let change = crate::panel::field::ghost_button_width(self.scale, measured);
            let change_rect = crate::panel::field::path_change_rect(
                self.metrics,
                self.rects,
                self.scale,
                change,
            );
            painter.fill_round(change_rect, radius, &self.brushes.field_fill);
            painter.draw_text(
                crate::panel::field::GHOST_WORD_CHANGE,
                chip_format,
                change_rect,
                &self.brushes.text,
            );

            let open_rect = crate::panel::field::path_open_rect(self.metrics, self.rects, self.scale);
            painter.fill_round(open_rect, radius, &self.brushes.field_fill);
            Self::draw_glyph(
                painter,
                crate::panel::icons::Glyph::FolderOpen,
                crate::panel::field::path_open_icon_rect(self.metrics, self.rects, self.scale),
                self.brushes.muted_rgba,
            )?;

            // The chip widths, measured by the caller when it has a device and by this paint when it
            // does not. The window measures them so that its hit test and this drawing use one set
            // of numbers; a second measurement that happens to agree is a click waiting to land
            // beside its chip.
            let widths = match self.path_chips {
                Some(widths) => widths,
                None => {
                    let mut measured_widths = [0i32; 3];
                    for (index, word) in crate::panel::field::PATH_CHIP_WORDS.iter().enumerate() {
                        let measured = painter.measure_text_width(word, chip_format)?.round() as i32;
                        measured_widths[index] =
                            crate::panel::field::path_chip_width(self.scale, measured);
                    }
                    measured_widths
                }
            };
            let height =
                (crate::panel::field::AFTER_CHIP_HEIGHT_DIP as f32 * self.scale).round();
            let pill = (height / 2.0).round() as i32;
            let icon_gap = (crate::panel::field::PATH_CHIP_ICON_GAP_DIP as f32 * self.scale)
                .round() as i32;
            for (index, word) in crate::panel::field::PATH_CHIP_WORDS.iter().enumerate() {
                let rect = crate::panel::field::path_chip_rect(
                    self.metrics,
                    self.rects,
                    self.scale,
                    &widths,
                    index,
                );
                painter.stroke_round(rect, pill, &self.brushes.line_soft, line);
                let icon = crate::panel::field::path_chip_icon_rect(
                    self.metrics,
                    self.rects,
                    self.scale,
                    &widths,
                    index,
                );
                Self::draw_glyph(
                    painter,
                    crate::panel::icons::Glyph::Folder,
                    icon,
                    self.brushes.muted_rgba,
                )?;
                let text = snapclip_model::geometry::Rect {
                    left: icon.right + icon_gap,
                    top: rect.top,
                    right: rect.right,
                    bottom: rect.bottom,
                };
                painter.draw_text(word, chip_format, text, &self.brushes.muted);
            }
        }


        // is hovered (`:247`), and the icon is `--accent-text` because the hub starts locked (`:248`,
        // `:603`), so the default state is the icon alone. Its rectangle comes from the same place
        // the two cells were laid out around, so the icon cannot drift away from the row.
        //
        // The picture is the baked Tabler file rather than a redrawing of it. `assets/icon/README.md`
        // records that Direct2D's `DrawSvgDocument` paints nothing on this machine, and that redrawing
        // the shapes with lines is by definition not the same picture, so the icon arrives as pixels
        // and is tinted on the way in.
        let icon = crate::panel::field::link_hub_icon_rect(self.metrics, self.rects, self.scale);
        Self::draw_glyph(
            painter,
            crate::panel::icons::Glyph::Link,
            icon,
            self.brushes.accent_text_rgba,
        )?;

        // The size row's mode buttons and its reset (`:583`–`:591`). The header line is
        // `justify-content: space-between` (`:165`), so this group is anchored to the column's right
        // edge and grows leftward. A button is its word plus ten dip on each side (`:226`), and a
        // word's width depends on its glyphs, so it is measured through the device rather than
        // reserved. `数值` carries `is-on` (`:585`) and is drawn at the label weight, which is the
        // only difference the design makes between the two.
        if let (Some(chosen_format), Some(other_format)) = (self.row_label, self.chip) {
            let chosen = crate::panel::SIZE_MODE_CHOSEN;
            let mut widths = [0i32; 2];
            for (index, word) in crate::panel::SIZE_MODE_WORDS.iter().enumerate() {
                let format = if index == chosen { chosen_format } else { other_format };
                let measured = painter.measure_text_width(word, format)?.round() as i32;
                widths[index] = crate::panel::size_mode_button_width(self.scale, measured);
            }
            let radius = (crate::panel::SIZE_MODE_RADIUS_DIP as f32 * self.scale).round() as i32;
            let container = crate::panel::size_mode_rect(
                self.metrics,
                self.rects,
                self.scale,
                widths[0] + widths[1],
            );
            // `:225` the container is `--surface` with a `--line-soft` edge; both are already
            // resolved, because the material behind the panel is the compositor's business.
            painter.fill_round(container, radius, &self.brushes.surface);
            painter.stroke_round(container, radius, &self.brushes.line_soft, line);

            let button_radius =
                (crate::panel::SIZE_MODE_BUTTON_RADIUS_DIP as f32 * self.scale).round() as i32;
            for (index, word) in crate::panel::SIZE_MODE_WORDS.iter().enumerate() {
                let button = crate::panel::size_mode_button_rect(
                    self.metrics,
                    self.rects,
                    self.scale,
                    widths,
                    index,
                );
                // `:227` the chosen button takes the field fill behind its word and the text
                // colour; the other one is transparent and `--muted` (`:226`).
                if index == chosen {
                    painter.fill_round(button, button_radius, &self.brushes.field_fill);
                    painter.draw_text(word, chosen_format, button, &self.brushes.text);
                } else {
                    painter.draw_text(word, other_format, button, &self.brushes.muted);
                }
            }

            // `:229` the reset is a bare `--muted` icon until it is hovered, and its picture is
            // `ICON.reset` (`:589`), which the design's own key table maps to this circular arrow.
            let reset =
                crate::panel::size_mode_reset_icon_rect(self.metrics, self.rects, self.scale);
            Self::draw_glyph(
                painter,
                crate::panel::icons::Glyph::Refresh,
                reset,
                self.brushes.muted_rgba,
            )?;
        }

        // The footer (`:274`–`:288`, `:674`–`:687`): a hairline across the top, then copy, a
        // separator, cancel and save, anchored to the right because the mockup's status text takes
        // the rest of the line with `flex: 1` (`:275`). The status itself is not drawn yet, which
        // `docs/41` records, so nothing occupies the space it will.
        if let (Some(button_format), Some(strong_format)) = (self.button, self.button_strong) {
            let border_y = crate::panel::footer_border_y(self.metrics, self.scale);
            // One dip, rounded up to at least one pixel so the rule never vanishes at low scale.
            let hairline = (self.scale.round() as i32).max(1);
            painter.fill_round(
                Rect::new(0, border_y, self.rects.header.right, border_y + hairline),
                0,
                &self.brushes.footer_line,
            );

            // The widths come from the caller when it also answers clicks, so a click can only land
            // on a button whose rectangle is the one on screen. A caller that only paints measures
            // here; both paths end in `footer_button_width`, so there is one rule for how wide a
            // button is and one place that knows it.
            let widths = match self.footer_widths {
                Some(widths) => widths,
                None => {
                    let mut widths = [0i32; 3];
                    for (index, button) in crate::panel::FOOTER_BUTTONS.iter().enumerate() {
                        let word = crate::panel::footer_button_text(*button);
                        let format = if crate::panel::footer_button_is_strong(*button) {
                            strong_format
                        } else {
                            button_format
                        };
                        let measured = painter.measure_text_width(word, format)?.round() as i32;
                        widths[index] =
                            crate::panel::footer_button_width(self.scale, *button, measured);
                    }
                    widths
                }
            };

            let radius =
                (crate::panel::FOOTER_BUTTON_RADIUS_DIP as f32 * self.scale).round() as i32;
            for button in crate::panel::FOOTER_BUTTONS {
                let rect =
                    crate::panel::footer_button_rect(self.metrics, self.scale, widths, button);
                let word = crate::panel::footer_button_text(button);
                match button {
                    // `:282` save is the one filled button: `--accent` behind white text at 600.
                    crate::panel::FooterButton::Save => {
                        painter.fill_round(rect, radius, &self.brushes.accent);
                        painter.draw_text(word, strong_format, rect, &self.brushes.text);
                    }
                    // `:280` cancel is transparent with a `--line-soft` edge and `--text`.
                    crate::panel::FooterButton::Cancel => {
                        painter.stroke_round(rect, radius, &self.brushes.line_soft, line);
                        painter.draw_text(word, button_format, rect, &self.brushes.text);
                    }
                    // `:286` copy is a ghost: no background, `--muted`, and its word follows the
                    // glyph rather than centring in the button.
                    crate::panel::FooterButton::Copy => {
                        let text = crate::panel::footer_copy_text_rect(self.metrics, self.scale, widths);
                        painter.draw_text(word, button_format, text, &self.brushes.muted);
                    }
                }
            }

            let icon = crate::panel::footer_copy_icon_rect(self.metrics, self.scale, widths);
            Self::draw_glyph(
                painter,
                crate::panel::icons::Glyph::Copy,
                icon,
                self.brushes.muted_rgba,
            )?;

            // `:288` the separator is a one-dip `--line-soft` rule, sixteen dip tall.
            let separator =
                crate::panel::footer_separator_rect(self.metrics, self.scale, widths);
            painter.fill_round(separator, 0, &self.brushes.line_soft);

            // The status line (`:275`, `:675`): the footer's first child, in the space the buttons
            // leave. `.status.is-error` is `--danger` (`:276`) and `.is-warning` is `--warning`
            // (`:327`), and the level travels with the message so the panel cannot pick its own
            // colour for it.
            if let (Some((level, text)), Some(format)) = (self.status, self.status_format) {
                if !text.is_empty() {
                    let rect = crate::panel::footer_status_rect(self.metrics, self.scale, widths);
                    let colour = match level {
                        crate::copy::Level::Error => &self.brushes.danger,
                        // `--warning` (`:23`), not the field border's yellow: `.status.is-warning`
                        // is the token the design uses for a warning *sentence*.
                        crate::copy::Level::Warn => &self.brushes.warning_text,
                    };
                    painter.draw_text(text, format, rect, colour);
                }
            }
        }

        // The format control last, because its menu hangs over everything else. The button shares the
        // name row's band and the menu is positioned against the window's own height, so both need
        // the height the canvas was built with rather than a constant.
        let window_height = canvas.bottom;
        self.paint_format(painter, window_height)?;
        // The token menu, for the same reason and after it: only one of the two can be open, so the
        // order between them is a statement about nothing, and the last thing drawn is the one that
        // would win if both ever were.
        self.paint_variables(painter)?;
        Ok(())
    }

    /// The token menu (`:217`–`:221`, `:690`), drawn only while it is open.
    ///
    /// It hangs under the name field's variable button and covers part of the rows below it. The
    /// frame draws it, but Windows paints the native text boxes afterwards, so the window hides the
    /// boxes it covers for as long as it is open — a menu that appears behind a text field would be
    /// worse than no menu.
    fn paint_variables(&self, painter: &PanelPainter) -> Result<()> {
        if !self.var_open {
            return Ok(());
        }
        let Some(label_format) = self.var_label else {
            return Ok(());
        };
        let scale = self.scale;
        let line = scale.round().max(1.0);
        let menu = crate::panel::var_menu_rect(self.metrics, self.rects, scale);
        // `--r-menu` is the same corner the format menu uses (`:180`), and the one dip inset stands
        // in for the drop shadow a normal window cannot draw.
        let radius = (crate::panel::FMT_MENU_RADIUS_DIP as f32 * scale).round() as i32;
        painter.fill_round(menu, radius, &self.brushes.surface);
        let inset = (crate::panel::FMT_MENU_LINE_DIP as f32 * scale).round() as i32;
        painter.stroke_round(
            Rect::new(
                menu.left + inset / 2,
                menu.top + inset / 2,
                menu.right - inset / 2,
                menu.bottom - inset / 2,
            ),
            (radius - inset).max(0),
            &self.brushes.line,
            line,
        );

        let item_radius = (crate::panel::FMT_ITEM_RADIUS_DIP as f32 * scale).round() as i32;
        let pad = (crate::panel::VAR_MENU_PAD_DIP as f32 * scale).round() as i32;
        for (index, label) in crate::panel::VARIABLE_LABELS.iter().enumerate() {
            let item = crate::panel::var_item_rect(self.metrics, self.rects, scale, index);
            let _ = item_radius;
            // `.var-item { padding: 0 10px; text-align: left }` (`:219`), and `--text` for the word.
            let text = Rect::new(
                item.left + pad,
                item.top,
                item.right - pad,
                item.bottom,
            );
            painter.draw_text(label, label_format, text, &self.brushes.text);
        }
        Ok(())
    }

    /// The format button and, when it is open, its menu (`:173`–`:185`, `:561`, `:696`).
    ///
    /// The two states of the button are the design's: a transparent border when closed (`:173`) and
    /// an accent border with a one dip ring when open (`:175`), the same shape a focused field has
    /// but inset by one dip rather than one and a half. The chevron turns over in the open state
    /// (`:178`), which is done on the glyph's pixels because the panel has no transform.
    fn paint_format(&self, painter: &PanelPainter, window_height: i32) -> Result<()> {
        let Some(label_format) = self.format_label else {
            return Ok(());
        };
        let scale = self.scale;
        let radius = (crate::panel::FMT_RADIUS_DIP as f32 * scale).round() as i32;
        let line = scale.round().max(1.0);
        let button = crate::panel::fmt_button_rect(self.metrics, self.rects, scale);
        let chosen = self.format.min(crate::panel::FMT_OPTIONS.len() - 1);

        painter.fill_round(button, radius, &self.brushes.field_fill);
        if self.format_open {
            painter.stroke_round(button, radius, &self.brushes.accent, line);
            let ring = (crate::panel::FMT_RING_DIP as f32 * scale).round() as i32;
            painter.stroke_round(
                Rect::new(
                    button.left + ring,
                    button.top + ring,
                    button.right - ring,
                    button.bottom - ring,
                ),
                (radius - ring).max(0),
                &self.brushes.accent,
                line,
            );
        }

        // `.fmt-btn { color: var(--text); font-family: var(--num) }` (`:173`), and the word starts at
        // the left padding.
        painter.draw_text(
            crate::panel::FMT_OPTIONS[chosen],
            label_format,
            crate::panel::fmt_label_rect(self.metrics, self.rects, scale),
            &self.brushes.text,
        );
        // The chevron is `--muted` closed and `--accent-text`, turned over, open (`:176`, `:178`).
        let chev = crate::panel::fmt_chev_rect(self.metrics, self.rects, scale);
        if self.format_open {
            Self::draw_glyph_turned(
                painter,
                crate::panel::icons::Glyph::Chevron,
                chev,
                self.brushes.accent_text_rgba,
            )?;
        } else {
            Self::draw_glyph(
                painter,
                crate::panel::icons::Glyph::Chevron,
                chev,
                self.brushes.muted_rgba,
            )?;
        }

        if !self.format_open {
            return Ok(());
        }

        // `.fmt-menu` (`:180`): the surface, a ten dip corner, and a one dip `--line` inset because
        // the drop shadow a normal window cannot draw is what normally separates a menu from below.
        let menu = crate::panel::fmt_menu_rect(self.metrics, self.rects, scale, window_height);
        let menu_radius = (crate::panel::FMT_MENU_RADIUS_DIP as f32 * scale).round() as i32;
        painter.fill_round(menu, menu_radius, &self.brushes.surface);
        let inset = (crate::panel::FMT_MENU_LINE_DIP as f32 * scale).round() as i32;
        painter.stroke_round(
            Rect::new(
                menu.left + inset / 2,
                menu.top + inset / 2,
                menu.right - inset / 2,
                menu.bottom - inset / 2,
            ),
            (menu_radius - inset).max(0),
            &self.brushes.line,
            line,
        );

        let item_radius = (crate::panel::FMT_ITEM_RADIUS_DIP as f32 * scale).round() as i32;
        for index in 0..crate::panel::FMT_OPTIONS.len() {
            let item = crate::panel::fmt_item_rect(self.metrics, self.rects, scale, window_height, index);
            // `.fmt-item.is-selected { background: var(--fill) }` (`:183`), the same fill the fields
            // and the segmented track use.
            if index == chosen {
                painter.fill_round(item, item_radius, &self.brushes.field_fill);
                Self::draw_glyph(
                    painter,
                    crate::panel::icons::Glyph::Check,
                    crate::panel::fmt_check_rect(self.metrics, self.rects, scale, window_height, index),
                    self.brushes.accent_text_rgba,
                )?;
            }
            painter.draw_text(
                crate::panel::FMT_OPTIONS[index],
                label_format,
                crate::panel::fmt_item_label_rect(self.metrics, self.rects, scale, window_height, index),
                &self.brushes.text,
            );
        }
        Ok(())
    }

    /// Draw a baked glyph, tinted to a colour, into `rect`.
    ///
    /// The pixels arrive white on transparent from `icon-bake` and are coloured here, because the
    /// panel paints through a DC render target, which has no colour matrix effect to tint with. The
    /// order of the tint's output and the bitmap's alpha mode are decided in one place, so the next
    /// icon does not have to remember either.
    fn draw_glyph(
        painter: &PanelPainter,
        glyph: crate::panel::icons::Glyph,
        rect: Rect,
        colour: [u8; 4],
    ) -> Result<()> {
        let tinted = crate::panel::icons::tint_rgba_to_premultiplied_bgra(glyph.raw(), colour);
        let bitmap = painter.bgra_bitmap_with_alpha(
            crate::panel::icons::ICON_BAKED_PX,
            crate::panel::icons::ICON_BAKED_PX,
            &tinted,
        )?;
        painter.draw_bitmap_at(&bitmap, rect);
        Ok(())
    }

    /// [`Self::draw_glyph`] with the icon turned by half a turn, for the open format button's
    /// chevron (`:178`). The rotation is done on the pixels because the panel has no transform.
    fn draw_glyph_turned(
        painter: &PanelPainter,
        glyph: crate::panel::icons::Glyph,
        rect: Rect,
        colour: [u8; 4],
    ) -> Result<()> {
        let side = crate::panel::icons::ICON_BAKED_PX as usize;
        let turned = crate::panel::icons::flipped_180(glyph.raw(), side);
        let tinted = crate::panel::icons::tint_rgba_to_premultiplied_bgra(&turned, colour);
        let bitmap =
            painter.bgra_bitmap_with_alpha(side as u32, side as u32, &tinted)?;
        painter.draw_bitmap_at(&bitmap, rect);
        Ok(())
    }
}

/// The Direct2D factory a DC render target needs. Keep one per process and reuse it.
pub fn factory() -> Result<ID2D1Factory> {
    unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }
}

/// A text format in the embedded family at a given size and weight. `CreateTextFormat` does not take
/// a stack, so the families are walked here in the same order the overlay uses, starting with the
/// subset. Every string the panel draws goes through this, so the fallback order exists once.
pub fn text_format(
    dwrite: &IDWriteFactory,
    dpi: f32,
    size_dip: f32,
    weight: DWRITE_FONT_WEIGHT,
) -> Result<IDWriteTextFormat> {
    text_format_in(
        dwrite,
        dpi,
        size_dip,
        weight,
        &[FAMILY, "Segoe UI Variable Display", "Segoe UI"],
    )
}

/// [`text_format`] for a caller that draws in another face. The size row's number and unit are set
/// in the embedded numeric face (`fonts.rs`), which is registered separately and falls back
/// through its own list.
pub fn text_format_in(
    dwrite: &IDWriteFactory,
    dpi: f32,
    size_dip: f32,
    weight: DWRITE_FONT_WEIGHT,
    families: &[&str],
) -> Result<IDWriteTextFormat> {
    let scale = (dpi / 96.0).max(0.5);
    let mut last: Option<windows::core::Error> = None;
    for family in families {
        let wide = windows::core::HSTRING::from(*family);
        let made = unsafe {
            dwrite.CreateTextFormat(
                &wide,
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size_dip * scale,
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

/// The header title's format: `--fs-title: 12px` (`:31`) at weight 600 (`.head .title`, `:95`),
/// centred vertically so the text sits on the header's own centre line.
pub fn title_format(dwrite: &IDWriteFactory, dpi: f32) -> Result<IDWriteTextFormat> {
    text_format(dwrite, dpi, 12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)
}

/// `--fs-row: 11px` (`save-panel-demo-v3.html:35`). The row labels' size.
pub const ROW_LABEL_FONT_DIP: f32 = 11.0;

/// `--fs-chip: 11px` (`save-panel-demo-v3.html:36`). The segmented options' size.
pub const CHIP_FONT_DIP: f32 = 11.0;

/// The row labels: `--fs-row: 11px` (`:35`) at `.row-label`'s weight 500 (`:166`).
pub fn row_label_format(dwrite: &IDWriteFactory, dpi: f32) -> Result<IDWriteTextFormat> {
    text_format(dwrite, dpi, ROW_LABEL_FONT_DIP, DWRITE_FONT_WEIGHT_MEDIUM)
}

/// The segmented options' words: `--fs-chip: 11px` (`:36`). `.seg .opt` sets `font: inherit` and
/// then only the size (`:266`), so their weight is the body's rather than the label's.
pub fn chip_format(dwrite: &IDWriteFactory, dpi: f32) -> Result<IDWriteTextFormat> {
    let format = text_format(dwrite, dpi, CHIP_FONT_DIP, DWRITE_FONT_WEIGHT_NORMAL)?;
    // `.seg .opt { display: grid; place-items: center }` (`:266`) centres the word in its option,
    // so this format is the one that does not lead-align.
    unsafe {
        let _ = format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
    }
    Ok(format)
}

/// `--fs-btn: 12px` (`save-panel-demo-v3.html:38`). The footer buttons' size.
pub const BUTTON_FONT_DIP: f32 = 12.0;

/// The footer's buttons: `--fs-btn` at the body weight, centred because `.btn` is a grid with
/// `place-items: center` (`:279`).
pub fn button_format(dwrite: &IDWriteFactory, dpi: f32) -> Result<IDWriteTextFormat> {
    let format = text_format(dwrite, dpi, BUTTON_FONT_DIP, DWRITE_FONT_WEIGHT_NORMAL)?;
    unsafe {
        let _ = format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
    }
    Ok(format)
}

/// `.btn.start { font-weight: 600 }` (`:282`) — the save button differs from the others in nothing
/// else.
pub fn button_strong_format(dwrite: &IDWriteFactory, dpi: f32) -> Result<IDWriteTextFormat> {
    let format = text_format(dwrite, dpi, BUTTON_FONT_DIP, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
    unsafe {
        let _ = format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
    }
    Ok(format)
}

/// `--fs-status: 11px` (`save-panel-demo-v3.html:37`). The footer's one line of message.
pub const STATUS_FONT_DIP: f32 = 11.0;

/// The footer's status line: `--fs-status` at the body weight, leading-aligned because `.status` is
/// a plain span that starts where its box does (`:275`).
pub fn status_format(dwrite: &IDWriteFactory, dpi: f32) -> Result<IDWriteTextFormat> {
    text_format(dwrite, dpi, STATUS_FONT_DIP, DWRITE_FONT_WEIGHT_NORMAL)
}

/// `.var-item { font-size: 12px }` (`save-panel-demo-v3.html:219`). The token menu's labels.
pub const VAR_FONT_DIP: f32 = 12.0;

/// The token menu's labels: twelve dip at the body weight, leading-aligned because `.var-item` is
/// `text-align: left` (`:219`) rather than centred like a button.
pub fn var_label_format(dwrite: &IDWriteFactory, dpi: f32) -> Result<IDWriteTextFormat> {
    text_format(dwrite, dpi, VAR_FONT_DIP, DWRITE_FONT_WEIGHT_NORMAL)
}

/// The three shortcut chips' widths, measured once for the same reason the footer's are.
///
/// The window needs them to answer a click on a chip and the paint needs them to draw one, and two
/// measurements that happen to agree are not one number used twice.
pub fn measure_path_chips(dwrite: &IDWriteFactory, dpi: f32) -> Result<[i32; 3]> {
    let scale = (dpi / 96.0).max(0.5);
    let format = chip_format(dwrite, dpi)?;
    let mut widths = [0i32; 3];
    for (index, word) in crate::panel::field::PATH_CHIP_WORDS.iter().enumerate() {
        let measured = crate::panel::d2d::measure_text_width(dwrite, word, &format)?;
        widths[index] = crate::panel::field::path_chip_width(scale, measured.round() as i32);
    }
    Ok(widths)
}

/// The path row's `更改` button width, measured the way the painter measures it.
///
/// The button is a word with twelve dip of padding (`:189`), and the input gives up exactly this
/// width so the native control stops short of the button. One measurement, two consumers — the same
/// arrangement as the footer's, and for the same reason: a second measurement that happens to agree
/// is not the same as one number used twice.
pub fn measure_path_change(dwrite: &IDWriteFactory, dpi: f32) -> Result<i32> {
    let scale = (dpi / 96.0).max(0.5);
    let format = chip_format(dwrite, dpi)?;
    let measured = crate::panel::d2d::measure_text_width(
        dwrite,
        crate::panel::field::GHOST_WORD_CHANGE,
        &format,
    )?
    .round() as i32;
    Ok(crate::panel::field::ghost_button_width(scale, measured))
}

/// The footer buttons' widths, measured the way the painter measures them.
///
/// A caller that paints and also answers clicks measures once here and hands the result to both, so
/// the rectangle a button is drawn in and the rectangle a click is tested against are one set of
/// numbers. Nothing about this needs a render target: text layout only needs the factory, which is
/// why the window can do it at startup, before the first paint, and why a click that arrives before
/// anything has been drawn still lands where the button will be.
pub fn measure_footer_widths(dwrite: &IDWriteFactory, dpi: f32) -> Result<[i32; 3]> {
    let scale = (dpi / 96.0).max(0.5);
    let button = button_format(dwrite, dpi)?;
    let strong = button_strong_format(dwrite, dpi)?;
    let mut widths = [0i32; 3];
    for (index, footer) in crate::panel::FOOTER_BUTTONS.iter().enumerate() {
        let format = if crate::panel::footer_button_is_strong(*footer) {
            &strong
        } else {
            &button
        };
        let measured =
            crate::panel::d2d::measure_text_width(dwrite, crate::panel::footer_button_text(*footer), format)?
                .round() as i32;
        widths[index] = crate::panel::footer_button_width(scale, *footer, measured);
    }
    Ok(widths)
}

/// `--fs-num: 16px` (`save-panel-demo-v3.html:33`). The size cells' numbers.
pub const NUMBER_FONT_DIP: f32 = 16.0;

/// `.dim-unit { font-size: 12px }` (`save-panel-demo-v3.html:244`). The unit beside a number.
pub const UNIT_FONT_DIP: f32 = 12.0;

/// The numeric face's size format, used for the `px` unit beside each size cell.
///
/// `.dim-input` and `.dim-unit` both set `font-family: var(--num)` (`:242`, `:244`) — a monospaced
/// face so the digits line up between the two cells and a changing value does not shuffle the text
/// sideways. The face is the embedded subset in `fonts.rs`, registered separately from the UI font,
/// so it carries its own fallback list.
///
/// The number itself is not drawn here: it lives in a native `EDIT`, whose alignment is the control
/// style `ES_RIGHT` rather than a text format.
pub fn number_format(dwrite: &IDWriteFactory, dpi: f32) -> Result<IDWriteTextFormat> {
    let families: Vec<&str> = std::iter::once(crate::panel::NUMBER_FAMILY)
        .chain(crate::panel::NUMBER_FALLBACKS)
        .collect();
    text_format_in(
        dwrite,
        dpi,
        UNIT_FONT_DIP,
        DWRITE_FONT_WEIGHT_MEDIUM,
        &families,
    )
}

/// The format control's word: twelve dip in the numeric face, because `.fmt-btn` and `.fmt-item`
/// set `font-family: var(--num)` (`:173`, `:182`). Leading alignment — both start at their own left
/// padding rather than centring, which is what makes `.PNG` and `.WebP` line up between the button
/// and its menu.
pub fn format_label_format(dwrite: &IDWriteFactory, dpi: f32) -> Result<IDWriteTextFormat> {
    let families: Vec<&str> = std::iter::once(crate::panel::NUMBER_FAMILY)
        .chain(crate::panel::NUMBER_FALLBACKS)
        .collect();
    text_format_in(
        dwrite,
        dpi,
        BUTTON_FONT_DIP,
        DWRITE_FONT_WEIGHT_NORMAL,
        &families,
    )
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

/// Everything the frame paints that is not the canvas itself: what to draw and in which state.
///
/// The fields are named because several of them share a type and mean different things: `quality`
/// and `format` are indices into two different lists, and `fill_surface` and `format_open` are both
/// booleans. Passed positionally a swap compiles and draws the wrong thing without a word, and this
/// panel has already lost time to two such swaps. Named fields make the swap unrepresentable.
pub struct PaintInput<'a> {
    /// The window's real DPI. Sizes the text and drives the layout scale; the target itself is
    /// built at 96 so that `rects(scale)` is not applied twice.
    pub dpi: f32,
    /// Whether to fill the surface. A window with the DWM backdrop behind it says `false`.
    pub fill_surface: bool,
    /// Each form field's frame state: the native children draw no border, no focus ring and no
    /// error colour, so the frames around them have to be told what state each one is in.
    pub fields: crate::panel::FieldStates,
    /// The captured picture -- BGRA, width, height -- for the preview card.
    pub preview: Option<(&'a [u8], u32, u32)>,
    /// Which option the quality segment marks as chosen.
    pub quality: usize,
    /// Which save format the format button shows.
    pub format: usize,
    /// Whether the format menu is open.
    pub format_open: bool,
    /// The footer's button widths, measured by the window so a click is tested against the
    /// rectangles that were drawn. `None` measures here, which is what a caller that only paints
    /// wants.
    pub footer_widths: Option<[i32; 3]>,
    /// The path row's `更改` button width, measured by the window for the same reason: the input's
    /// own rectangle and the button's are one measurement used twice.
    pub path_change: i32,
    /// The footer's one line of message with its level: the window decides whether there is one and
    /// what it says, the frame decides how the two levels look.
    pub status: Option<(crate::copy::Level, &'a str)>,
    /// Whether the token menu is open. Only one menu can be open at a time; the window closes one
    /// when the other opens, so this is a fact about the panel rather than a preference.
    pub var_open: bool,
    /// The three shortcut chips' widths, measured by the window for the same reason as the footer's.
    pub path_chips: Option<[i32; 3]>,
}

impl<'a> PaintInput<'a> {
    /// The least a caller has to decide: a DPI and the fields, with nothing else shown.
    pub fn idle(dpi: f32) -> Self {
        Self {
            dpi,
            fill_surface: true,
            fields: crate::panel::FieldStates::IDLE,
            preview: None,
            quality: 0,
            format: 0,
            format_open: false,
            footer_widths: None,
            path_change: 0,
            status: None,
            var_open: false,
            path_chips: None,
        }
    }
}

/// Paint the frame onto a device context. This is the real path: the window's `WM_PAINT` hands its
/// own HDC here and the tests hand a memory DC, so both run the same drawing code.
pub fn paint_to_dc_with_preview(
    factory: &ID2D1Factory,
    dwrite: &IDWriteFactory,
    hdc: HDC,
    area: Rect,
    input: &PaintInput<'_>,
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
    let scale = (input.dpi / 96.0).max(0.5);
    let metrics = DEMO;
    let rects = metrics.rects(scale);
    // The title is part of the frame, not of any control: the design puts it in the header and
    // every drawn character has to be in the embedded subset, which the glyph gate enforces.
    let title = title_format(dwrite, input.dpi)?;
    // The form's own text, at the two sizes the design gives it (`:35`, `:36`).
    let row_label = row_label_format(dwrite, input.dpi)?;
    let chip = chip_format(dwrite, input.dpi)?;
    // The numbers' face, for the size cells' `px` unit (`:242`, `:244`).
    let number = number_format(dwrite, input.dpi)?;
    // The footer's buttons, at `--fs-btn` (`:38`) and, for save, 600 (`:282`).
    let button = button_format(dwrite, input.dpi)?;
    let button_strong = button_strong_format(dwrite, input.dpi)?;
    // The format control's word, in the numeric face (:173, :182).
    let format_label = format_label_format(dwrite, input.dpi)?;
    // The footer's status line (`--fs-status: 11px`, :37).
    let status_format = status_format(dwrite, input.dpi)?;
    // The token menu's labels (`.var-item`, 12 dip, `:219`).
    let var_label = var_label_format(dwrite, input.dpi)?;
    let painter = PanelPainter {
        context: &target,
        text_factory: dwrite,
    };
    let preview = match input.preview {
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
        fill_surface: input.fill_surface,
        title: Some((TITLE, &title)),
        row_label: Some(&row_label),
        chip: Some(&chip),
        number: Some(&number),
        button: Some(&button),
        button_strong: Some(&button_strong),
        format_label: Some(&format_label),
        format: input.format,
        format_open: input.format_open,
        footer_widths: input.footer_widths,
        path_change: input.path_change,
        status: input.status,
        status_format: Some(&status_format),
        var_open: input.var_open,
        var_label: Some(&var_label),
        path_chips: input.path_chips,
        fields: input.fields,
        preview: preview.as_ref().map(|(b, w, h)| (b, *w, *h)),
        quality: input.quality,
    }
    .paint(&painter)?;
    unsafe { target.EndDraw(None, None)? };
    Ok(())
}

#[cfg(test)]
mod tests {
    impl Canvas {
        /// Paint with a chosen format and, optionally, its menu open.
        ///
        /// The same entry point the window uses, so the pixels these tests read are the pixels the
        /// panel draws; the only thing that differs from the runtime is who decided the two values.
        fn render_format(&self, format: usize, open: bool) {
            let factory = factory().expect("a Direct2D factory");
            let dwrite = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_ISOLATED) }
                .expect("a DirectWrite factory");
            paint_to_dc_with_preview(
                &factory,
                &dwrite,
                self.dc,
                Rect::new(0, 0, self.width, self.height),
                &PaintInput {
                    format,
                    format_open: open,
                    ..PaintInput::idle(self.dpi)
                },
            )
            .expect("the frame paints with a chosen format");
        }
    }

    /// The strongest pixel inside a rectangle, and where it was found. The glyph tests need to know
    /// both: a word's antialiased edge is dimmer than its stroke, so the colour assertion is made on
    /// the darkest-or-brightest ink the rectangle actually contains.
    fn strongest(px: &Bgra8, rect: Rect, colour: Px) -> (Px, i32, i32) {
        let mut best = (Px { b: 0, g: 0, r: 0, a: 255 }, 0, 0);
        let mut best_distance = i32::MAX;
        for y in rect.top..rect.bottom {
            for x in rect.left..rect.right {
                let got = px.pixel(x, y);
                let distance = (i32::from(got.b) - i32::from(colour.b)).abs()
                    + (i32::from(got.g) - i32::from(colour.g)).abs()
                    + (i32::from(got.r) - i32::from(colour.r)).abs();
                if distance < best_distance {
                    best_distance = distance;
                    best = (got, x, y);
                }
            }
        }
        best
    }

    /// The format button draws the word of the format it is set to, in `--text` (`:173`).
    ///
    /// At double scale, because a twelve dip word at unit scale antialiases to a fraction of its
    /// ink and never reaches the colour it was asked for — the same fact that made the border and
    /// the ring indistinguishable at 1×, and the close glyph's test run at 2×.
    #[test]
    fn the_format_button_draws_the_chosen_word_in_the_text_colour() {
        for (format, word) in crate::panel::FMT_OPTIONS.iter().enumerate() {
            let canvas = Canvas::new_at(1840, 1128, 192.0);
            canvas.render_format(format, false);
            let px = canvas.pixels();
            let rects = DEMO.rects(2.0);
            let label = crate::panel::fmt_label_rect(&DEMO, &rects, 2.0);
            let (got, x, y) = strongest(&px, label, fill_rgb(255, 255, 255));
            assert!(
                close(got, fill_rgb(255, 255, 255), 12),
                "{word} must be drawn in --text (:173): the closest pixel in the label's box is \
                 {got:?} at ({x}, {y})"
            );

            // The label is a word, not a block: the box's far corner has to be the fill behind it.
            assert!(
                close(px.pixel(label.right - 1, label.bottom - 1), fill_rgb(41, 41, 46), 12),
                "the label's corner must stay the field's fill, so the ink is text rather than a slab"
            );
        }
    }

    /// The open button's border is `--accent` with a one dip ring (`:175`); the closed one has a
    /// transparent border (`:173`) and shows the fill only.
    #[test]
    fn the_format_button_border_is_accent_only_when_open() {
        let canvas = Canvas::new_at(1840, 1128, 192.0);
        let rects = DEMO.rects(2.0);
        let button = crate::panel::fmt_button_rect(&DEMO, &rects, 2.0);
        let probe = ((button.left + button.right) / 2, button.top + 1);

        canvas.render_format(0, false);
        let closed = canvas.pixels();
        assert!(
            close(closed.pixel(probe.0, probe.1), fill_rgb(41, 41, 46), 12),
            "a closed button's top edge is the fill, because its border is transparent (:173)"
        );

        canvas.render_format(0, true);
        let open = canvas.pixels();
        assert_px(
            &open,
            probe.0,
            probe.1,
            fill_rgb(31, 117, 219),
            12,
            "an open button's top edge is --accent (:175)",
        );
    }

    /// The open menu marks the chosen option with the check glyph in `--accent-text` (`:184`), and
    /// leaves the others without one — which is what makes the menu say which format is in effect.
    #[test]
    fn the_open_format_menu_marks_only_the_chosen_option() {
        let canvas = Canvas::new_at(1840, 1128, 192.0);
        let rects = DEMO.rects(2.0);
        let height = (DEMO.panel_height() as f32 * 2.0).round() as i32;
        for chosen in 0..crate::panel::FMT_OPTIONS.len() {
            canvas.render_format(chosen, true);
            let px = canvas.pixels();
            for index in 0..crate::panel::FMT_OPTIONS.len() {
                let check = crate::panel::fmt_check_rect(&DEMO, &rects, 2.0, height, index);
                let (got, x, y) = strongest(&px, check, fill_rgb(94, 162, 245));
                if index == chosen {
                    assert!(
                        close(got, fill_rgb(94, 162, 245), 14),
                        "the chosen option {index} must carry the check in --accent-text (:184): \
                         closest {got:?} at ({x}, {y})"
                    );
                } else {
                    assert!(
                        !close(got, fill_rgb(94, 162, 245), 14),
                        "option {index} is not chosen and must not be marked: found {got:?} at \
                         ({x}, {y})"
                    );
                }
            }

            // The chosen row is also the one with `--fill` behind it (`:183`), sampled at the item's
            // right end where neither the check nor the word reaches.
            for index in 0..crate::panel::FMT_OPTIONS.len() {
                let item = crate::panel::fmt_item_rect(&DEMO, &rects, 2.0, height, index);
                let probe = (item.right - 6, item.top + 4);
                if index == chosen {
                    assert!(
                        close(px.pixel(probe.0, probe.1), fill_rgb(41, 41, 46), 10),
                        "the chosen row has --fill behind it (:183), index {index}"
                    );
                } else {
                    assert!(
                        !close(px.pixel(probe.0, probe.1), fill_rgb(41, 41, 46), 10),
                        "an unchosen row is the menu's own surface, index {index}"
                    );
                }
            }
        }
    }

    /// The directory row is the one above the name row, and it is taller than a text row: its
    /// content is the input (32) *and* the shortcut chips under it (24 plus the design's 6 gap), so
    /// the name row's field starts 102 below the path row's field rather than one 72 pitch lower.
    #[test]
    fn the_directory_row_sits_above_the_name_row() {
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let rects = DEMO.rects(scale);
            let path = crate::panel::path_field_rect(&DEMO, &rects, scale, 56);
            let name = crate::panel::name_field_rect(&DEMO, &rects, scale);
            assert!(
                path.top < name.top,
                "the directory row must be above the name row at {scale}: path.top={} name.top={}",
                path.top,
                name.top
            );
            // The path row's content (62), the gap to the next row (12), the name row's label (22)
            // and the gap under it (6). Two independently rounded sums may differ by one pixel, so
            // the comparison allows exactly that: `row_label_top` rounds each total once, and 1.25
            // scale puts a half-pixel inside both.
            let expected = ((DEMO.field_height
                + DEMO.row_gap
                + crate::panel::field::AFTER_CHIP_HEIGHT_DIP
                + DEMO.row_spacing
                + DEMO.row_label_height
                + DEMO.row_gap) as f32
                * scale)
                .round() as i32;
            assert!(
                (name.top - path.top - expected).abs() <= 1,
                "the two rows must be one tall row plus a row's label apart at {scale}: got {} expected {expected}",
                name.top - path.top
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

    /// The size row's link hub draws the chain icon in `--accent-text` (`:29`, `:248`).
    ///
    /// Rendered at double scale on purpose. The icon's stroke is 1.4 dip wide, so at unit scale it
    /// lands across two pixels and never reaches its full colour, which would force the tolerance
    /// wide enough to accept almost anything. At 2× there is a solid core to sample.
    ///
    /// The points come from the icon's own rectangle: the mockup's chain has a diagonal through the
    /// middle (`:709`, `M6.5 9.5l3-3`), so the centre and the two points one dip along that diagonal
    /// are on the stroke, and the hub's corners are not. That is what tells an icon from a filled box.
    #[test]
    fn the_close_glyph_sits_centred_in_its_button() {
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            let rects = DEMO.rects(scale);
            let icon = crate::panel::field::close_icon_rect(&DEMO, &rects, scale);
            let button = rects.close;
            assert!(
                icon.left >= button.left
                    && icon.top >= button.top
                    && icon.right <= button.right
                    && icon.bottom <= button.bottom,
                "the glyph must sit inside the button at {scale}: icon {icon:?} button {button:?}"
            );
            // Centred within a pixel, not exactly: the two gaps are integers and their sum is odd
            // whenever the button and the glyph differ by an odd number of pixels, so one side gets
            // the remainder. Demanding equality here was the test being wrong, not the code.
            let left_gap = icon.left - button.left;
            let right_gap = button.right - icon.right;
            assert!(
                (left_gap - right_gap).abs() <= 1,
                "the glyph is centred within a pixel at {scale}: gaps {left_gap} and {right_gap}"
            );
            let top_gap = icon.top - button.top;
            let bottom_gap = button.bottom - icon.bottom;
            assert!(
                (top_gap - bottom_gap).abs() <= 1,
                "the glyph is centred within a pixel vertically at {scale}: gaps {top_gap} and {bottom_gap}"
            );
        }
    }

    #[test]
    fn the_close_button_answers_only_inside_itself() {
        let rects = DEMO.rects(1.0);
        let button = rects.close;
        let middle = ((button.left + button.right) / 2, (button.top + button.bottom) / 2);
        assert!(
            crate::panel::close_button_contains(&rects, 1.0, middle),
            "the middle of the button is a hit"
        );
        assert!(
            crate::panel::close_button_contains(&rects, 1.0, (button.left, button.top)),
            "the top left corner is a hit, because the rect is half open"
        );
        for point in [
            (button.right, middle.1),
            (middle.0, button.bottom),
            (button.left - 1, middle.1),
            (middle.0, button.top - 1),
            (button.left + 40, middle.1),
        ] {
            assert!(
                !crate::panel::close_button_contains(&rects, 1.0, point),
                "{point:?} must not be a hit: the header is not the button"
            );
        }
    }

    /// The footer, at double scale for the same reason the close glyph is: a one dip hairline at
    /// unit scale never reaches full intensity, so the pixel it lands on is a blend.
    #[test]
    fn the_footer_draws_its_hairline_and_its_filled_button() {
        let canvas = Canvas::new_at(1840, 1128, 192.0);
        canvas.render();
        let px = canvas.pixels();

        // The hairline is one dip of `rgba(255,255,255,.04)` over the surface (`:274`).
        let border_y = crate::panel::footer_border_y(&DEMO, 2.0);
        let expected = crate::panel::footer_line_over_surface(DEMO_PALETTE.surface);
        assert_px(
            &px,
            400,
            border_y + 1,
            fill_rgb(expected[0], expected[1], expected[2]),
            4,
            "the footer's top border",
        );

        // Save is the one filled button: `--accent` behind its word (`:282`). The sample is a
        // corner of the button, away from the centred word, so it reads the fill rather than a
        // glyph.
        let widths = [88, 60, 70];
        let save =
            crate::panel::footer_button_rect(&DEMO, 2.0, widths, crate::panel::FooterButton::Save);
        let accent =
            crate::panel::field_colours(crate::panel::FieldState::Focused, DEMO_PALETTE.surface)
                .border;
        assert_px(
            &px,
            save.left + 6,
            save.top + 6,
            fill_rgb(accent[0], accent[1], accent[2]),
            8,
            "the save button's fill",
        );

        // The strip above the border is still the panel's body, not the footer.
        assert_ne!(
            px.pixel(400, border_y - 6),
            px.pixel(400, border_y + 6),
            "the footer must begin at its border"
        );
    }

    /// The footer's status line: the one place a failed save is visible at all.
    ///
    /// At double scale for the same reason the buttons' words are: eleven dip text at unit scale
    /// antialiases to a fraction of its colour, so the assertion would have to accept a blend and
    /// stop testing the colour.
    ///
    /// The two levels are the design's two colours: `.status.is-error` is `--danger` (`:276`) and
    /// `.is-warning` is `--warning` (`:327`). The same sentence is drawn in both, because what
    /// changes is the level, not the wording.
    #[test]
    fn the_footer_status_is_drawn_in_its_levels_colour() {
        // A sentence that is in the embedded subset, so the pixels come from our own face: the
        // glyph gate carries all eleven messages (`overlay_drawn_strings` collects `copy`).
        let text = crate::copy::ValidationCopy::StemEmpty.message();
        for (level, colour, name) in [
            (crate::copy::Level::Error, fill_rgb(255, 123, 123), "--danger"),
            (crate::copy::Level::Warn, fill_rgb(240, 179, 90), "--warning"),
        ] {
            let canvas = Canvas::new_at(1840, 1128, 192.0);
            let widths = canvas.render_status(Some((level, text)));
            let px = canvas.pixels();
            let rect = crate::panel::footer_status_rect(&DEMO, 2.0, widths);
            let (got, x, y) = strongest(&px, rect, colour);
            assert!(
                close(got, colour, 12),
                "{text} at {level:?} must be drawn in {name}: the closest pixel in the status's \
                 own box is {got:?} at ({x}, {y})"
            );
        }
    }

    /// A panel that has not failed at anything draws nothing in the status's space.
    ///
    /// This is what makes the assertion above mean something: if the line were always inked, the
    /// colour test would pass while the message was a decoration rather than an answer.
    #[test]
    fn a_panel_without_a_message_leaves_the_status_space_clean() {
        let canvas = Canvas::new_at(1840, 1128, 192.0);
        let widths = canvas.render_status(None);
        let px = canvas.pixels();
        let rect = crate::panel::footer_status_rect(&DEMO, 2.0, widths);
        let surface = fill_rgb(23, 23, 28);
        // Every pixel in the status's box is the surface: nothing is drawn there.
        for y in rect.top..rect.bottom {
            for x in rect.left..rect.right {
                assert!(
                    close(px.pixel(x, y), surface, 6),
                    "the status space is ink at ({x}, {y}) with no message to show: {:?}",
                    px.pixel(x, y)
                );
            }
        }
    }

    /// The insert-a-variable button is drawn on the parent, in the strip the name control gives up.
    ///
    /// Checked at 2× for the same reason as the close glyph: the graphic is a line drawing, and at
    /// unit scale a one point four dip stroke is spread over two pixels and never reaches its colour.
    #[test]
    fn the_variable_button_is_drawn_inside_the_name_field() {
        let canvas = Canvas::new_at(1840, 1040, 192.0);
        canvas.render();
        let px = canvas.pixels();
        let rects = DEMO.rects(2.0);
        let icon = crate::panel::field::name_wand_icon_rect(&DEMO, &rects, 2.0);
        // The shape, named by where the ink is: braces are two strokes with a gap between them, so
        // the columns that carry ink form exactly two runs. A block, a single stroke, an X (three
        // runs) and a glyph with no ink all fail this, which "there is ink in the box" did not.
        let want = blend(DEMO_PALETTE.muted, [41, 41, 46, 255]);
        // The colour: `--muted` composited over the field's fill, an absolute value rather than the
        // strongest pixel sampled from the same picture. This is the assertion that fails when the
        // tint drops the colour's alpha, which it did until this batch.
        //
        // The shape: at sixteen dip the braces' strokes merge into one band, so what can be said is
        // that the band is centred and does not reach either edge. That rules out a block, an empty
        // box and a glyph drawn at the wrong size; it does **not** tell braces from another icon of
        // similar size, which is a limit of a pixel rule at this resolution rather than something
        // this test checks by accident — `docs/41` records it.
        let mut painted = false;
        let mut middle_ink = false;
        let middle = (icon.left + icon.right) / 2;
        let edge = 4;
        let mut left_ink = false;
        let mut right_ink = false;
        for y in icon.top..icon.bottom {
            for x in icon.left..icon.right {
                if !close(px.pixel(x, y), want, 16) {
                    continue;
                }
                painted = true;
                if (x - middle).abs() <= 2 {
                    middle_ink = true;
                }
                if x < icon.left + edge {
                    left_ink = true;
                }
                if x >= icon.right - edge {
                    right_ink = true;
                }
            }
        }
        assert!(painted, "the wand must be painted in --muted over the field's fill");
        assert!(middle_ink, "the braces' band runs through the middle of their box");
        assert!(
            !left_ink && !right_ink,
            "and it must not reach the box's edges, or the graphic is a block: left {left_ink}, right {right_ink}"
        );
    }

    #[test]
    fn the_close_glyph_is_drawn_in_the_buttons_colour() {
        let canvas = Canvas::new_at(1840, 1040, 192.0);
        canvas.render();
        let px = canvas.pixels();
        let rects = DEMO.rects(2.0);
        let icon = crate::panel::field::close_icon_rect(&DEMO, &rects, 2.0);
        // `color: var(--muted)` on the button (`:96`) over the surface: an absolute colour, not the
        // strongest pixel sampled out of the same picture. The old version compared that pixel with
        // itself at tolerance zero, which could not fail whatever the glyph was.
        let want = blend(DEMO_PALETTE.muted, [23, 23, 28, 255]);
        // The glyph is an X, so its two strokes cross in the middle of its box.
        let middle = ((icon.left + icon.right) / 2, (icon.top + icon.bottom) / 2);
        assert_px(
            &px,
            middle.0,
            middle.1,
            want,
            12,
            "the two strokes of the close glyph cross in the middle of its box",
        );
        // And the box is not filled: its four corners stay the surface, which is what tells a 14 dip
        // glyph apart from a painted block — and from any other icon that fills its frame.
        for (x, y) in [
            (icon.left, icon.top),
            (icon.right - 1, icon.top),
            (icon.left, icon.bottom - 1),
            (icon.right - 1, icon.bottom - 1),
        ] {
            assert_not_any(&px, x, y, &[want], 40, "a corner of the close glyph's box");
        }
    }

    #[test]
    fn the_link_hub_icon_is_drawn_in_accent_text() {
        let canvas = Canvas::new_at(1840, 1040, 192.0);
        canvas.render();
        let px = canvas.pixels();
        let rects = DEMO.rects(2.0);
        let hub = crate::panel::field::link_hub_rect(&DEMO, &rects, 2.0);
        let icon = crate::panel::field::link_hub_icon_rect(&DEMO, &rects, 2.0);
        let want = fill_rgb(94, 162, 245);
        let middle = (icon.left + 16, icon.top + 16);
        for (x, y) in [
            (middle.0, middle.1),
            (middle.0 - 2, middle.1 + 2),
            (middle.0 + 2, middle.1 - 2),
        ] {
            assert_px(&px, x, y, want, 8, "the link hub's chain stroke");
        }
        for (x, y) in [(hub.left + 2, hub.top + 2), (hub.right - 3, hub.bottom - 3)] {
            assert_not_any(&px, x, y, &[want], 8, "a hub corner outside the icon");
        }
    }

    #[test]
    fn the_directory_field_is_filled_and_has_no_border_when_idle() {
        let canvas = Canvas::new(920, DEMO.panel_height());
        canvas.render();
        let px = canvas.pixels();
        let rects = DEMO.rects(1.0);
        let field = crate::panel::path_field_rect(&DEMO, &rects, 1.0, 56);
        assert_idle_field(&px, field, "an idle directory field");
    }

    #[test]
    fn the_size_cells_are_filled_and_have_no_border_when_idle() {
        let canvas = Canvas::new(920, DEMO.panel_height());
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

    /// A size cell is a wrapper holding the number, with its `宽`/`高` word outside the wrapper and
    /// its `px` unit inside it (`:237`, `:242`, `:244`). Both words are drawn, both in `--muted`, and
    /// the samples are named positions with named colours rather than a search for any ink.
    ///
    /// The colours are blends, not the brush value: the text is `--muted` at 62% alpha (`:21`), so
    /// what reaches the buffer is the brush over whatever lies beneath — the surface for the word
    /// outside the wrapper and the field fill for the unit inside it.
    #[test]
    fn the_size_cells_draw_their_word_and_their_unit() {
        let canvas = Canvas::new(920, DEMO.panel_height());
        canvas.render();
        let px = canvas.pixels();
        let rects = DEMO.rects(1.0);
        let surface = DEMO_PALETTE.surface;
        // The fill the painter resolves for an idle field, which is what the unit is drawn over.
        let fill = [41u8, 41, 46, 255];

        // The word is outside the wrapper, so it lands on the panel's surface.
        let muted = DEMO_PALETTE.muted;
        let word_on_surface = blend(muted, surface);
        let unit_on_fill = blend(muted, fill);
        for (cell, word, unit) in [
            (crate::panel::SizeCell::Width, "宽", "px"),
            (crate::panel::SizeCell::Height, "高", "px"),
        ] {
            let _ = (word, unit);
            let label = crate::panel::field::size_cell_label_rect(&DEMO, &rects, 1.0, cell);
            let wrapper = crate::panel::size_cell_rect(&DEMO, &rects, 1.0, cell);
            let unit_rect = crate::panel::field::size_cell_unit_rect(&DEMO, &rects, 1.0, cell);

            assert!(
                label.right <= wrapper.left,
                "the word sits outside the wrapper and before it (:237)"
            );
            assert!(
                unit_rect.left >= wrapper.left && unit_rect.right <= wrapper.right,
                "the unit sits inside the wrapper, after the number (:244)"
            );
            // The words are anti-aliased, so a stroke's core pixel is a rasteriser detail; what is
            // not a detail is which colour the glyph is painted in and which rectangle it lands in.
            assert_ink(&px, label, word_on_surface, 12, "the cell's word is --muted over the surface (:237)");
            assert_ink(&px, unit_rect, unit_on_fill, 12, "the cell's unit is --muted over the field fill (:244)");
        }
    }

    /// The closest pixel in `rect` to `want` must be within `tolerance`, which for anti-aliased text
    /// is the honest form: the glyph's core pixel is wherever the rasteriser put it, but the colour
    /// it was painted in and the rectangle it belongs to are both named here.
    fn assert_ink(px: &Bgra8, rect: Rect, want: Px, tolerance: i32, what: &str) {
        let mut best: Option<(i32, i32, Px)> = None;
        for y in rect.top..rect.bottom {
            for x in rect.left..rect.right {
                let seen = px.pixel(x, y);
                let distance = (i32::from(seen.b) - i32::from(want.b)).abs()
                    + (i32::from(seen.g) - i32::from(want.g)).abs()
                    + (i32::from(seen.r) - i32::from(want.r)).abs();
                if best.is_none_or(|(_, _, previous)| distance < distance_of(previous, want)) {
                    best = Some((x, y, seen));
                }
            }
        }
        let (x, y, seen) = best.expect("the rectangle is not empty");
        assert!(
            close(seen, want, tolerance),
            "{what}: no pixel in {rect:?} is within {tolerance} of {want:?}; the closest is {seen:?} at ({x}, {y})"
        );
    }

    fn distance_of(seen: Px, want: Px) -> i32 {
        (i32::from(seen.b) - i32::from(want.b)).abs()
            + (i32::from(seen.g) - i32::from(want.g)).abs()
            + (i32::from(seen.r) - i32::from(want.r)).abs()
    }

    /// `--muted` (white at 62%) painted over a background, resolved to the pixel it produces.
    fn blend(brush: [u8; 4], under: [u8; 4]) -> Px {
        let a = f32::from(brush[3]) / 255.0;
        let over = |index: usize| {
            (f32::from(brush[index]) * a + f32::from(under[index]) * (1.0 - a)).round() as u8
        };
        Px {
            b: over(2),
            g: over(1),
            r: over(0),
            a: 255,
        }
    }

    /// The segmented control marks the chosen option with an accent block and leaves the others on
    /// the track's fill. Both are named by position and colour, so a control that marked every
    /// option, or marked none, or marked the wrong one, fails here.
    ///
    /// The samples sit in an option's corner rather than at its centre, because the centre is where
    /// the option's word is drawn (`:641`–`:643`): the centre was the right place to look before
    /// there was any text, and pointing at a glyph would make this test about the glyph's ink
    /// instead of about the block behind it.
    #[test]
    fn the_quality_segment_marks_the_chosen_option_with_the_accent() {
        let canvas = Canvas::new(920, DEMO.panel_height());
        canvas.render();
        let px = canvas.pixels();
        let rects = DEMO.rects(1.0);
        let track = crate::panel::quality_seg_rect(&DEMO, &rects, 1.0);
        // Four dips in from the option's own corner: inside the block, clear of the centred word.
        let corner = |rect: snapclip_model::geometry::Rect| (rect.left + 4, rect.top + 4);

        let chosen = crate::panel::quality_option_rect(&DEMO, &rects, 1.0, 0);
        let (cx, cy) = corner(chosen);
        assert_px(&px, cx, cy, fill_rgb(31, 117, 219), 6, "the chosen option is --accent (:265)");

        let other = crate::panel::quality_option_rect(&DEMO, &rects, 1.0, 1);
        let (ox, oy) = corner(other);
        assert_px(&px, ox, oy, fill_rgb(41, 41, 46), 6, "an unchosen option is the track's --fill (:263)");

        // The track itself is filled even where no option is: the last inset's worth of pixels to
        // the right of the third option belongs to the track, not to the surface behind it.
        let last = crate::panel::quality_option_rect(&DEMO, &rects, 1.0, crate::panel::QUALITY_OPTIONS - 1);
        let (lx, ly) = corner(last);
        assert_px(&px, lx, ly, fill_rgb(41, 41, 46), 6, "the third option's cell is track fill (:263)");
        assert!(
            track.right > last.right,
            "the track has to extend past the last option, or the insets are not being applied"
        );
    }
    /// A click and the picture it produces, in one test. The state machine chooses the option and
    /// the painter is told that choice; splitting the two is how a control ends up clickable but
    /// unchanged on screen, which is the one failure of a self drawn control that no return value
    /// reports.
    #[test]
    fn clicking_an_option_moves_the_block_the_painter_draws() {
        let rects = DEMO.rects(1.0);
        let mut state = crate::panel::quality_segment_state(&DEMO, &rects, 1.0, 0);
        let second = crate::panel::quality_option_rect(&DEMO, &rects, 1.0, 1);
        let aim = snapclip_model::geometry::Point::new(
            (second.left + second.right) / 2,
            (second.top + second.bottom) / 2,
        );

        crate::panel::quality_pointer(&mut state, crate::panel::PointerEvent::Down, aim);
        assert_eq!(
            crate::panel::quality_pointer(&mut state, crate::panel::PointerEvent::Up, aim),
            Some(1),
            "the click chooses the second option"
        );

        let canvas = Canvas::new(920, DEMO.panel_height());
        canvas.render_quality(crate::panel::chosen_quality(&state));
        let px = canvas.pixels();
        // Four dips in from each option's own corner: inside the block, clear of the centred word.
        let corner = |rect: snapclip_model::geometry::Rect| (rect.left + 4, rect.top + 4);

        let (sx, sy) = corner(second);
        assert_px(
            &px,
            sx,
            sy,
            fill_rgb(31, 117, 219),
            6,
            "the clicked option carries the accent block (:265)",
        );

        let first = crate::panel::quality_option_rect(&DEMO, &rects, 1.0, 0);
        let (fx, fy) = corner(first);
        assert_px(
            &px,
            fx,
            fy,
            fill_rgb(41, 41, 46),
            6,
            "and the option it left is track fill again (:263)",
        );
    }

    /// Every row's label is drawn in the label row above it, and the row below does not steal its
    /// neighbour's ink. The design puts the label 22 dips tall with a 6 dip gap under it (`:165`,
    /// `:164`), so the box each word is drawn into is arithmetic rather than a guess. A row with no
    /// content — the after-save row (`:569`–`:577`) — has no field to sit above, so for it the label
    /// row is the whole row.
    #[test]
    fn the_row_labels_are_drawn_above_their_rows() {
        let canvas = Canvas::new(920, DEMO.panel_height());
        canvas.render();
        let px = canvas.pixels();
        let rects = DEMO.rects(1.0);
        for row in crate::panel::FORM_ROWS {
            let label = crate::panel::label_rect(&DEMO, &rects, 1.0, row);
            let field = crate::panel::field_rect(&DEMO, &rects, 1.0, row);
            let has_content = row != crate::panel::Row::Saved;
            assert_eq!(
                field.bottom > field.top,
                has_content,
                "{:?}'s content box is present exactly when the row has content",
                row
            );
            if has_content {
                assert_eq!(
                    label.bottom,
                    field.top - DEMO.row_gap,
                    "{:?}'s label sits one gap above its row (:164)",
                    row
                );
            }
            assert_eq!(
                label.bottom - label.top,
                DEMO.row_label_height,
                "{:?}'s label row is 22 dips tall (:165)",
                row
            );
            assert!(
                ink_inside(&px, label, 12) >= 4,
                "{:?}'s label (\"{}\") must put ink in its own row, at {:?}",
                row,
                row.label(),
                label
            );
        }
    }

    /// The chosen option's word is `--text` while the others are `--muted` (`:266`, `:267`), so the
    /// brightest pixel of the chosen box is brighter than the brightest of an unchosen one. Both
    /// boxes are named, and swapping the two brushes — or dropping the word — fails here.
    #[test]
    fn the_chosen_quality_word_is_brighter_than_the_unchosen_ones() {
        let canvas = Canvas::new(920, DEMO.panel_height());
        canvas.render();
        let px = canvas.pixels();
        let rects = DEMO.rects(1.0);
        let option = |index: usize| {
            crate::panel::quality_option_rect(&DEMO, &rects, 1.0, index)
        };
        let chosen = brightest_inside(&px, option(0));
        let muted = brightest_inside(&px, option(1));
        assert!(
            chosen > muted + 20,
            "the chosen word is --text over the others' --muted (:266, :267): chosen={chosen} unchosen={muted}"
        );
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
        // The footer is the last strip: 44 header + 4 body top + 460 preview + 56 footer (:94,
        // :100, :44, :274 with --btn-h: 32 at :50). The mock's 12px body bottom padding is the
        // footer's own top padding, not a fourth term.
        assert_eq!(DEMO.panel_height(), 564, "44 header + 4 + 460 preview + 56 footer");
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
            self.render_fields(crate::panel::FieldStates::name_in(field_state));
        }

        /// Render with each field's own state, which is what the window hands over.
        fn render_fields(&self, fields: crate::panel::FieldStates) {
            let factory = factory().expect("a Direct2D factory");
            let dwrite = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_ISOLATED) }
                .expect("a DirectWrite factory");
            paint_to_dc_with_preview(
                &factory,
                &dwrite,
                self.dc,
                Rect::new(0, 0, self.width, self.height),
                &PaintInput {
                    fields,
                    ..PaintInput::idle(self.dpi)
                },
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
                &PaintInput {
                    preview: Some((bgra, w, h)),
                    ..PaintInput::idle(self.dpi)
                },
            )
            .expect("the frame paints with a preview");
        }

        /// Render with one of the quality options chosen, which is the state a click leaves behind.
        /// Everything else is idle, so the only thing that moves between these renders is the block
        /// the segmented control draws.
        fn render_quality(&self, quality: usize) {
            let factory = factory().expect("a Direct2D factory");
            let dwrite = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_ISOLATED) }
                .expect("a DirectWrite factory");
            paint_to_dc_with_preview(
                &factory,
                &dwrite,
                self.dc,
                Rect::new(0, 0, self.width, self.height),
                &PaintInput {
                    quality,
                    ..PaintInput::idle(self.dpi)
                },
            )
            .expect("the frame paints with a chosen quality");
        }

        /// Render with a footer status line, which is the state a failed save leaves behind.
        ///
        /// The widths are measured here and handed to the paint, so the rectangle the status is
        /// given in the assertions below is the one it was drawn in: the status ends where the copy
        /// button begins, and that number comes from the same rule the paint uses.
        fn render_status(&self, status: Option<(crate::copy::Level, &str)>) -> [i32; 3] {
            let factory = factory().expect("a Direct2D factory");
            let dwrite = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_ISOLATED) }
                .expect("a DirectWrite factory");
            let widths = crate::panel::measure_footer_widths(&dwrite, self.dpi)
                .expect("the footer's widths");
            paint_to_dc_with_preview(
                &factory,
                &dwrite,
                self.dc,
                Rect::new(0, 0, self.width, self.height),
                &PaintInput {
                    footer_widths: Some(widths),
                    status,
                    ..PaintInput::idle(self.dpi)
                },
            )
            .expect("the frame paints with a status");
            widths
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
    /// Whether any pixel inside `rect` is `want` within `tolerance`.
    ///
    /// Some graphics are found by scanning rather than by naming a coordinate: a folder's stroke
    /// need not cross its box's centre, and guessing a coordinate would produce a test that either
    /// fails for the right reason today or passes for the wrong one tomorrow.
    fn any_close(px: &Bgra8, rect: snapclip_model::geometry::Rect, want: Px, tolerance: i32) -> bool {
        for y in rect.top..rect.bottom {
            for x in rect.left..rect.right {
                if close(px.pixel(x, y), want, tolerance) {
                    return true;
                }
            }
        }
        false
    }

    /// The path row's square button is a filled ghost with a folder in it, and its first chip too.
    ///
    /// Both colours are absolute: `--fill` over the surface for the button's face (the same composite
    /// the fields use) and `--muted` over that face for the graphic. The chips start at the column's
    /// left edge, so their first folder is at a fixed place even though each chip's width depends on
    /// its word.
    #[test]
    fn the_path_row_draws_a_filled_button_and_its_chips() {
        let canvas = Canvas::new_at(1840, 1128, 192.0);
        canvas.render();
        let px = canvas.pixels();
        let rects = DEMO.rects(2.0);
        let face = fill_rgb(41, 41, 46);
        let ink = blend(DEMO_PALETTE.muted, [41, 41, 46, 255]);

        let open = crate::panel::field::path_open_rect(&DEMO, &rects, 2.0);
        // Ten pixels in, because the button carries the design's six dip corner (`:54`): a point on
        // the arc is the surface behind the button, not the button.
        assert_px(
            &px,
            open.left + 10,
            open.top + 10,
            face,
            6,
            "the square button's --fill face (:189)",
        );
        let open_icon = crate::panel::field::path_open_icon_rect(&DEMO, &rects, 2.0);
        assert!(
            any_close(&px, open_icon, ink, 16),
            "the square button carries its folder in --muted (:205)"
        );

        let widths = [crate::panel::field::path_chip_width(2.0, 20); 3];
        let chip_icon = crate::panel::field::path_chip_icon_rect(&DEMO, &rects, 2.0, &widths, 0);
        assert!(
            any_close(&px, chip_icon, ink, 16),
            "the first chip carries the folder the mockup puts before its word (:1008)"
        );
        let chip = crate::panel::field::path_chip_rect(&DEMO, &rects, 2.0, &widths, 0);
        assert_eq!(chip.left, rects.form.left, "the chips start at the column's edge (:548)");
    }

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

    /// How many pixels inside `rect` differ from the surface by more than `tolerance`. Text is
    /// anti-aliased, so no single pixel of it has a predictable value; what is predictable is that
    /// a drawn string puts ink inside the box it was given and a missing one puts none. The
    /// assertion still names the box, and a mutation that stops drawing the string makes it zero.
    fn ink_inside(px: &Bgra8, rect: snapclip_model::geometry::Rect, tolerance: i32) -> usize {
        let surface = surface_px();
        let mut ink = 0;
        for y in rect.top.max(0)..rect.bottom.max(0) {
            for x in rect.left.max(0)..rect.right.max(0) {
                if !close(px.pixel(x, y), surface, tolerance) {
                    ink += 1;
                }
            }
        }
        ink
    }

    /// The brightest pixel in a box, as a plain luma. Used where the assertion is about one word
    /// being drawn in a brighter colour than another, which is a relation between two named boxes.
    fn brightest_inside(px: &Bgra8, rect: snapclip_model::geometry::Rect) -> i32 {
        let mut best = -1;
        for y in rect.top.max(0)..rect.bottom.max(0) {
            for x in rect.left.max(0)..rect.right.max(0) {
                let p = px.pixel(x, y);
                let luma = (i32::from(p.r) + i32::from(p.g) + i32::from(p.b)) / 3;
                best = best.max(luma);
            }
        }
        best
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

    /// Every field draws from its own state, not from one state shared by all of them.
    ///
    /// This is the regression for the defect the window had: `WM_CTLCOLOREDIT` and the focus handler
    /// both answered from a single state, so a focused path row painted the name row's frame, and the
    /// two size cells would have been answered the same way. The mutation is to make the painter read
    /// one field's state for every field, which turns the second assertion below red.
    #[test]
    fn each_field_draws_its_own_frame_state() {
        let metrics = DEMO;
        let rects = metrics.rects(1.0);
        let path = crate::panel::FieldId::Path.rect(&metrics, &rects, 1.0, 0);
        let name = crate::panel::FieldId::Name.rect(&metrics, &rects, 1.0, 0);
        let focused =
            crate::panel::field_colours(crate::panel::FieldState::Focused, DEMO_PALETTE.surface);
        // The three states that do paint a border. Idle is not one of them (`:167`), which is why the
        // idle assertion is "none of these" rather than "not this colour".
        let border_px = |c: [u8; 4]| Px { b: c[2], g: c[1], r: c[0], a: 255 };
        let error =
            crate::panel::field_colours(crate::panel::FieldState::Error, DEMO_PALETTE.surface);
        let warning =
            crate::panel::field_colours(crate::panel::FieldState::Warning, DEMO_PALETTE.surface);
        let borders = [
            border_px(focused.border),
            border_px(error.border),
            border_px(warning.border),
        ];
        let accent = border_px(focused.border);

        // The path row focused: it carries the accent, and the name row below it does not.
        let canvas = Canvas::new(920, DEMO.panel_height());
        canvas.render_fields(crate::panel::FieldStates::focused_on(Some(
            crate::panel::FieldId::Path,
        )));
        let px = canvas.pixels();
        assert!(
            (path.top..=path.top + 1).any(|y| close(px.pixel(path.left + 40, y), accent, 12)),
            "the focused path row's own border must be the accent"
        );
        assert_not_any(
            &px,
            name.left + 40,
            name.top,
            &borders,
            12,
            "the name row is not the focused field, so its frame draws no border (:167)",
        );

        // And the other way round, which is the half a single shared state could not express.
        let canvas = Canvas::new(920, DEMO.panel_height());
        canvas.render_fields(crate::panel::FieldStates::focused_on(Some(
            crate::panel::FieldId::Name,
        )));
        let px = canvas.pixels();
        assert!(
            (name.top..=name.top + 1).any(|y| close(px.pixel(name.left + 40, y), accent, 12)),
            "the focused name row's own border must be the accent"
        );
        assert_not_any(
            &px,
            path.left + 40,
            path.top,
            &borders,
            12,
            "the path row is not the focused field, so its frame draws no border (:167)",
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
    fn the_size_row_takes_its_numbers_from_the_design() {
        // The relational test above cannot catch a constant that is simply the wrong size, because
        // every rectangle is derived from the same constants and the relations keep holding. These
        // are the numbers the design states, each with the line it comes from.
        use crate::panel::field::{SIZE_GAP_DIP, SIZE_LABEL_DIP, SIZE_UNIT_MARGIN_DIP};
        assert_eq!(SIZE_LABEL_DIP, 20, ".cell-label width (:237)");
        assert_eq!(SIZE_GAP_DIP, 8, ".values gap (:235)");
        assert_eq!(SIZE_UNIT_MARGIN_DIP, 4, ".dim-unit margin-left (:244)");
        assert_eq!(
            crate::panel::field::LINK_HUB_DIP,
            28,
            ".link-hub width (:246)"
        );
    }

    #[test]
    fn a_size_cell_is_a_label_then_a_number_then_a_unit() {
        // `.values` is a flex row of five children with an 8 dip gap (`:235`): the cell's label, the
        // cell itself, the hub, the other label and the other cell. Inside a cell the wrapper is the
        // filled box (`:238`), the number is right aligned and the unit follows it (`:242`, `:244`).
        use crate::panel::field::{
            SIZE_GAP_DIP, SizeCell, size_cell_input_rect, size_cell_label_rect, size_cell_rect,
            size_cell_unit_rect,
        };
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let rects = DEMO.rects(scale);
            let gap = (SIZE_GAP_DIP as f32 * scale).round() as i32;
            for cell in [SizeCell::Width, SizeCell::Height] {
                let wrapper = size_cell_rect(&DEMO, &rects, scale, cell);
                let label = size_cell_label_rect(&DEMO, &rects, scale, cell);
                let input = size_cell_input_rect(&DEMO, &rects, scale, cell);
                let unit = size_cell_unit_rect(&DEMO, &rects, scale, cell);

                assert_eq!(
                    label.right + gap,
                    wrapper.left,
                    "{cell:?} at {scale}: the label must be one gap before the cell"
                );
                assert!(
                    label.left < label.right && label.right <= input.left,
                    "{cell:?} at {scale}: label {label:?} must end at or before the number {input:?}"
                );
                assert!(
                    input.left < input.right && input.right <= unit.left,
                    "{cell:?} at {scale}: number {input:?} must end at or before the unit {unit:?}"
                );
                assert!(
                    unit.left < unit.right && unit.right <= wrapper.right,
                    "{cell:?} at {scale}: unit {unit:?} must stay inside the cell {wrapper:?}"
                );
                for part in [label, input, unit] {
                    assert_eq!(
                        (part.top, part.bottom),
                        (wrapper.top, wrapper.bottom),
                        "{cell:?} at {scale}: every part is on the cell's line"
                    );
                }
            }
        }
    }

    #[test]
    fn the_two_size_cells_are_separated_by_the_hub() {
        use crate::panel::field::{
            Row, SIZE_GAP_DIP, SizeCell, link_hub_rect, size_cell_label_rect, size_cell_rect,
            size_cell_unit_rect,
        };
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let rects = DEMO.rects(scale);
            let gap = (SIZE_GAP_DIP as f32 * scale).round() as i32;
            let hub = link_hub_rect(&DEMO, &rects, scale);
            let width = size_cell_rect(&DEMO, &rects, scale, SizeCell::Width);
            let height = size_cell_rect(&DEMO, &rects, scale, SizeCell::Height);
            let height_label = size_cell_label_rect(&DEMO, &rects, scale, SizeCell::Height);
            let width_unit = size_cell_unit_rect(&DEMO, &rects, scale, SizeCell::Width);

            assert_eq!(
                width.right + gap,
                hub.left,
                "{scale}: the first cell ends one gap before the hub"
            );
            assert!(
                width_unit.right < width.right,
                "{scale}: the unit sits inside the cell's own padding"
            );
            assert_eq!(
                hub.right + gap,
                height_label.left,
                "{scale}: the second label starts one gap after the hub"
            );
            assert_eq!(
                height.right,
                crate::panel::field::field_rect(&DEMO, &rects, scale, Row::Size).right,
                "{scale}: the second cell must absorb the remainder"
            );
        }
    }

    #[test]
    fn the_size_row_sits_below_the_name_row_and_splits_into_two_cells() {
        use crate::panel::field::{Row, SizeCell, label_rect, size_cell_rect};
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let rects = DEMO.rects(scale);
            let saved = label_rect(&DEMO, &rects, scale, Row::Saved);
            let width = size_cell_rect(&DEMO, &rects, scale, SizeCell::Width);
            let height = size_cell_rect(&DEMO, &rects, scale, SizeCell::Height);
            // From the after-save row's top: that row is a label row on its own, then the row gap,
            // the group margin, the size row's label row and its gap — and the cells start there.
            let expected = ((DEMO.row_label_height
                + DEMO.row_spacing
                + DEMO.group_spacing
                + DEMO.row_label_height
                + DEMO.row_gap) as f32
                * scale)
                .round() as i32;
            // Two independently rounded sums can differ by one pixel: `row_label_top` adds up the
            // rows above and rounds the total once, and at 1.25 scale the two totals straddle a
            // half-pixel. The assertion is therefore "within a pixel", which is what an integer
            // grid can promise — not "exactly equal", which it cannot.
            assert!(
                (width.top - (saved.top + expected)).abs() <= 1,
                "the size row opens the second group below the after-save row (:163, :579): got {} expected {}",
                width.top - saved.top,
                expected
            );
            assert_eq!(height.top, width.top, "both cells sit on the same row");
            assert!(width.right < height.left, "the link hub sits between the two cells (:246)");
            assert_eq!(
                height.right, rects.form.right,
                "the second cell ends at the form's right edge"
            );
        }
    }

    /// The after-save row's label reaches the screen. It is covered by
    /// `the_row_labels_are_drawn_above_their_rows`, which walks `FORM_ROWS` and therefore picks this
    /// row up without being told about it.

    #[test]
    fn the_first_form_slot_starts_where_the_design_puts_it() {
        let rects = DEMO.rects(1.0);
        assert_eq!(rects.form.top, 48, ".body padding-top 4 under the 44 dip header (:100)");
        assert_eq!(DEMO.text_row_height(), 60, "22 label + 6 gap + 32 field (:165, :164, :167)");
    }

    #[test]
    fn the_form_groups_are_sixteen_apart() {
        assert_eq!(DEMO.text_row_pitch() - DEMO.text_row_height(), 12, ".form gap is 12px (:162)");
        assert_eq!(DEMO.text_group_pitch() - DEMO.text_row_pitch(), 4, ".form-row.grp margin-top (:163)");
        assert_eq!(
            DEMO.text_group_pitch() - DEMO.text_row_height(),
            16,
            "the design's own note says groups sit 16 apart (:163)"
        );
    }

    /// Two kinds of row live in this form and they are not the same height: a row carrying a text
    /// field is 60 (label 22 + gap 6 + field 32, `:165`, `:164`, `:167`) while a row whose content
    /// is `--row` is 28 (`:47`, used by the segment `:263` and the estimate `:269`).
    ///
    /// This is asserted rather than assumed because the three metrics that describe a slot used to
    /// be named as if they described **any** row, which invites the next row to borrow a height
    /// that does not belong to it. The names now say which kind of row they mean, and this pins
    /// the difference itself so that collapsing the two heights cannot pass unnoticed.
    #[test]
    fn a_text_row_and_a_row_of_design_height_are_not_interchangeable() {
        assert_eq!(DEMO.text_row_height(), 60, "a row with a text field (:165, :164, :167)");
        assert_eq!(
            crate::panel::ROW_DIP,
            28,
            "--row, the design's own content height for a segment or estimate row (:47)"
        );
        assert_ne!(
            DEMO.text_row_height(),
            crate::panel::ROW_DIP,
            "a text row and a --row row are different heights, so one name cannot cover both"
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
        let canvas = Canvas::new(920, DEMO.panel_height());
        canvas.render();
        // The header band, right of the title: the form column no longer has a bare patch at this
        // height, because the path row now carries a filled button and three chips where its
        // surface used to show.
        let got = canvas.pixels().pixel(700, 12);
        assert!(close(got, surface_px(), 3), "the surface should be --surface: got {got:?}");
    }

    #[test]
    fn the_divider_sits_at_the_header_height() {
        let canvas = Canvas::new(920, DEMO.panel_height());
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
        let canvas = Canvas::new(920, DEMO.panel_height());
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
    fn the_title_is_drawn_inside_the_header() {        let canvas = Canvas::new(920, DEMO.panel_height());
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
        let canvas = Canvas::new(920, DEMO.panel_height());
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
        let canvas = Canvas::new(920, DEMO.panel_height());
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
        let canvas = Canvas::new(920, DEMO.panel_height());
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
        let canvas = Canvas::new(920, DEMO.panel_height());
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
        let canvas = Canvas::new(920, DEMO.panel_height());
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

        let canvas = Canvas::new(920, DEMO.panel_height());
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

    /// [`PanelRenderer::paint`] with a captured picture for the preview card.
    ///
    /// Additive on purpose: `paint` keeps its signature and delegates with no picture, so the
    /// window that has nothing to show and every existing bitmap test are untouched.
    ///
    /// `footer_widths` comes from [`PanelRenderer::measure_footer_widths`] so that the window can
    /// test a click against the same rectangles it paints.
    ///
    /// `status` is the footer's one line of message, with its level: the window decides *whether*
    /// there is a message and *what* it says, and the frame only decides how the two levels look.
    pub fn paint_with_preview(
        &self,
        hdc: *mut core::ffi::c_void,
        area: Rect,
        input: &PaintInput<'_>,
    ) -> Result<()> {
        paint_to_dc_with_preview(&self.factory, &self.dwrite, HDC(hdc), area, input)
    }

    /// The footer buttons' widths, for a caller that has to decide whether a click landed on one.
    ///
    /// Measured with the same formats and the same rule the paint uses, so the two agree by
    /// construction rather than by both being written carefully. Needs no window and no render
    /// target, so the window can call it once at startup and on a dpi change.
    pub fn measure_footer_widths(&self, dpi: f32) -> Result<[i32; 3]> {
        measure_footer_widths(&self.dwrite, dpi)
    }

    /// The path row's `更改` button width, for the same reason and with the same guarantee.
    pub fn measure_path_change(&self, dpi: f32) -> Result<i32> {
        measure_path_change(&self.dwrite, dpi)
    }

    /// The three shortcut chips' widths, for the same reason: the click handler tests the chips the
    /// paint drew rather than a second measurement of the same words.
    pub fn measure_path_chips(&self, dpi: f32) -> Result<[i32; 3]> {
        measure_path_chips(&self.dwrite, dpi)
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
