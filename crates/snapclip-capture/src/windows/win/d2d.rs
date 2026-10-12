//! Direct2D / DirectWrite resources and the L0/L1/L2 draw calls.
//!
//! Layer order, back to front:
//! 1. **L0** the frozen capture frame, drawn 1:1 so the selection stays pixel exact;
//! 2. **L1** a translucent dark mask over everything *except* the selection, drawn
//!    as four rectangles so the selected pixels keep their original brightness;
//! 3. **L2** the selection frame (outline inside the selection's edge, eight grips, the
//!    `width × height` label) and the magnifier.
//!
//! A scroll session draws none of that: only the same selection frame, without its marks, and the
//! scroll panel, on a transparent surface.
//!
//! Everything here is preview-only: the pixels that become the artifact are read
//! back from the frozen frame by [`crate::artifact::CaptureService::prepare_selection`]
//! and encoded by the composition root, so no mask, label or magnifier can leak into
//! the result.

use ::windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED,
    D2D1_BORDER_MODE_HARD, D2D1_COLOR_F, D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_GRADIENT_STOP,
    D2D1_PIXEL_FORMAT,
};
use ::windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1GaussianBlur, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_OPTIONS_NONE,
    D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1, D2D1_DRAW_TEXT_OPTIONS_CLIP,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP, D2D1_GAMMA_2_2,
    D2D1_GAUSSIANBLUR_OPTIMIZATION_BALANCED, D2D1_GAUSSIANBLUR_PROP_BORDER_MODE,
    D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION, D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
    D2D1_LAYER_PARAMETERS1, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_PROPERTY_TYPE_UNKNOWN,
    D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, ID2D1Bitmap1, ID2D1DeviceContext,
    ID2D1Effect, ID2D1Geometry, ID2D1Image, ID2D1LinearGradientBrush, ID2D1RenderTarget,
    ID2D1SolidColorBrush,
};
use ::windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use ::windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_ISOLATED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_METRICS,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
};
use ::windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use ::windows::Win32::Graphics::Dxgi::IDXGISwapChain1;
use ::windows::Win32::Graphics::Gdi::AddFontMemResourceEx;
use ::windows::core::{Interface, PCWSTR};
use std::sync::Arc;
use windows_numerics::Vector2;

use super::d3d11::GraphicsDevice;
use crate::annotation::{AnnotationGeometry, AnnotationItem, AnnotationKind};
// The palette and the light-math live in one place: the paint layer and the A4 ring decision read
// the same constants, so a re-measured mask or wash cannot leave the table describing a world that
// is no longer painted (docs/21 §5.24.11).
use crate::geometry::{Handle, LevelReach, MagnifierConfig, Point, Rect, SizeLabelPlacement};
use crate::keymap::LoupeHint;
use crate::ring_contrast::{
    ACCENT_RGB, CAPTURE_RGB, CHAIN_RING_RGB, CHAIN_RING_SHADOW, HOVER_WASH_ALPHA, MASK_ALPHA,
    MASK_RGB, MASK_TINT_ALPHA, PREVIEW_WASH_ALPHA, PREVIEW_WASH_WALK_SCALE, Rgb, mix,
};
use crate::scroll::layout::{self as scroll_layout, Layout};
pub(crate) use guide_pass::{GUIDE_AVOID_DIP, GuideView};

/// The font family used for the size label, per the tasklist (§6.3).
const LABEL_FONT_FAMILY: &str = "Segoe UI Variable Display";
const LABEL_FONT_FALLBACK: &str = "Segoe UI";
const LABEL_FONT_SIZE_DIP: f32 = 12.0;

/// Stroke width of the selection border in DIP.
const BORDER_WIDTH_DIP: f32 = 1.5;
/// Diameter of each solid circular grip in DIP.
const HANDLE_SIZE_DIP: f32 = 12.0;

/// Monospace family used for every numeric value in the magnifier info panel.
const MONO_FONT_FAMILY: &str = "Consolas";
const MONO_FONT_FALLBACK: &str = "Lucida Console";

/// The overlay's typeface: a subsetted HarmonyOS Sans SC carrying exactly the characters the
/// overlay can draw — every label, hint, guide row and key name, under any binding
/// (`overlay_drawn_strings` in the tests, built by `subfont/subset.ps1`). Embedded verbatim and
/// registered privately for this process at renderer start, so no system font install or admin
/// rights are needed.
const INFO_EMBEDDED_FONT: &[u8] = include_bytes!("../../../assets/harmonyos-sans-sc-subset.ttf");
/// Family name the embedded subset registers under (its name table was reduced to
/// this single English entry during subsetting).
const INFO_FONT_FAMILY: &str = "HarmonyOS Sans SC";

/// Info-panel layout, in DIP at 96 DPI (see `RenderMetrics::for_dpi` for the
/// scaling rule — everything here is multiplied by the same factor).
/// Vertical/horizontal padding of the whole strip.
const INFO_PADDING_V_DIP: f32 = 12.0;
const INFO_PADDING_H_DIP: f32 = 12.0;
/// Row 1: colour swatch size, corner radius and the gap to the value.
const INFO_SWATCH_SIZE_DIP: f32 = 18.0;
const INFO_SWATCH_RADIUS_DIP: f32 = 4.0;
const INFO_SWATCH_GAP_DIP: f32 = 8.0;
/// Row 1: the colour value (semi-bold) and the right-aligned `(x, y)` share one size, which shrinks
/// from the first to the second in half steps until both fit; at the floor the colour value is cut.
const INFO_ROW1_FONT_MAX_DIP: f32 = 13.0;
const INFO_ROW1_FONT_MIN_DIP: f32 = 11.0;
/// Least gap between the colour value and the position text.
const INFO_COORD_BLOCK_GAP_DIP: f32 = 14.0;
/// Row 2: the single hint row (`[S] 格式  [C] 复制  [P] 坐标  [Z] 缩放`), below row 1.
const INFO_HINT_ROW_GAP_DIP: f32 = 12.0;
const INFO_HINT_ROW_HEIGHT_DIP: f32 = 22.0;
/// The label size shrinks from the first to the second in half steps until the row fits with the
/// least gap between items; the key text is one size smaller and semi-bold.
const INFO_HINT_FONT_MAX_DIP: f32 = 12.0;
const INFO_HINT_FONT_MIN_DIP: f32 = 10.0;
const INFO_HINT_ITEM_GAP_MIN_DIP: f32 = 8.0;
/// Key cap: text plus this much, never narrower than it is tall; inset 2 DIP top and bottom.
const INFO_KBD_TEXT_PAD_DIP: f32 = 10.0;
const INFO_KBD_INSET_DIP: f32 = 2.0;
const INFO_KBD_RADIUS_DIP: f32 = 4.0;
const INFO_KBD_LABEL_GAP_DIP: f32 = 5.0;
/// Shrink steps of both ladders.
const INFO_FONT_STEP_DIP: f32 = 0.5;

/// The session's blurred frame (the backdrop of the info strip, the guide and the blurred mask).
const BACKDROP_BLUR_DIP: f32 = 6.0;

/// Sizes the renderer needs, all in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderMetrics {
    pub dpi: u32,
    pub border_width: f32,
    pub handle_size: f32,
    pub hit_slop: f32,
    pub label_font_size: f32,
    pub label_padding_x: f32,
    pub label_padding_y: f32,
    pub label_offset: f32,
    pub label_gap: f32,
}

impl RenderMetrics {
    pub fn for_dpi(dpi: u32) -> Self {
        let scale = dpi.max(96) as f32 / 96.0;
        Self {
            dpi,
            border_width: (BORDER_WIDTH_DIP * scale).max(1.0),
            handle_size: HANDLE_SIZE_DIP * scale,
            hit_slop: 6.0 * scale,
            label_font_size: LABEL_FONT_SIZE_DIP * scale,
            label_padding_x: 10.0 * scale,
            label_padding_y: 4.0 * scale,
            label_offset: 2.0 * scale,
            label_gap: 8.0 * scale,
        }
    }

    /// Displayed text for a selection in virtual-desktop coordinates.
    pub fn label_text_at(selection: Rect, screen_origin: Point) -> String {
        format!(
            "{},{} {}×{} px",
            selection.left + screen_origin.x,
            selection.top + screen_origin.y,
            selection.width(),
            selection.height()
        )
    }

    /// Pixel size of a selection's label, including padding.
    pub fn label_size_at(&self, selection: Rect, screen_origin: Point) -> (i32, i32) {
        let text = Self::label_text_at(selection, screen_origin);
        self.label_size_for_text(&text)
    }

    fn label_size_for_text(&self, text: &str) -> (i32, i32) {
        // Segoe UI digits are close to 0.62 em wide; the label is measured properly
        // by DirectWrite before drawing, this estimate only positions the panel.
        let text_width = text.chars().count() as f32 * self.label_font_size * 0.62;
        (
            (text_width + self.label_padding_x * 2.0).ceil() as i32,
            (self.label_font_size * 1.45 + self.label_padding_y * 2.0).ceil() as i32,
        )
    }
}

/// One ring of the level chain to paint (docs/21 §5.22), in back-buffer coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChainRingView {
    pub rect: Rect,
    /// Deeper than the selection. Inner rings are painted *after* the preview wash (see
    /// [`RenderView::chain_rings`]); outer rings are painted before it.
    pub inner: bool,
    /// Effective opacity for the ring brush — the outer/inner base after the distance ramp.
    pub alpha: f32,
}

/// One L0/L1/L2 composition request.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderView {
    /// Back buffer size in physical pixels.
    pub frame: Rect,
    /// Selection in back-buffer coordinates; empty when nothing is selected yet.
    pub selection: Rect,
    /// Cursor in back-buffer coordinates.
    pub cursor: Point,
    /// Virtual-desktop origin of this monitor, used for the coordinate label.
    pub screen_origin: Point,
    /// Whether the pointer is inside the overlay (drives the magnifier).
    pub cursor_visible: bool,
    /// Whether L2 chrome (border/grips/label) should be drawn.
    pub show_chrome: bool,
    /// Work area used to keep the label and magnifier on screen.
    pub work_area: Rect,
    /// Current color sample as RGB tuple for the info-panel swatch.
    pub magnifier_rgb: Option<(u8, u8, u8)>,
    /// The sampled colour rendered in the current `ColorFormat` (`#RRGGBB` /
    /// `rgb(...)` / `hsl(...)`). `S` cycles the format; there is no secondary
    /// slot any more.
    pub magnifier_color_text: Option<String>,
    /// Show the info-panel coordinate relative to the selection origin instead
    /// of as a global screen position (the P toggle).
    pub magnifier_relative: bool,
    /// Current loupe zoom (`Z` + wheel). `0.1..=40.0`; drives both the panel's
    /// source-window derivation and the numeric badge on the loupe's corner.
    pub magnifier_zoom: f32,
    // ── Annotations ───────────────────────────────────────────────────────────
    /// Committed annotation items to render at L2.
    pub annotation_items: Vec<crate::annotation::AnnotationItem>,
    /// Id of the currently selected annotation (drives selection box drawing).
    pub annotation_selected_id: Option<crate::annotation::AnnotationId>,
    /// Item being actively drawn (draft, not yet in `annotation_items`).
    pub annotation_draft: Option<crate::annotation::AnnotationItem>,
    // ── Window-snap hints (docs/14 §8) ────────────────────────────────────────
    /// Window under the cursor, in back-buffer coordinates. Paint-only: it never
    /// changes the selection.
    pub hover_bounds: Option<Rect>,
    /// Automatic-snap preview, in back-buffer coordinates. Painted as its own layer so a
    /// preview can never be confused with a confirmed selection.
    pub preview_bounds: Option<Rect>,
    /// The level chain as rings to paint, in back-buffer coordinates (docs/21 §5.22).
    ///
    /// Which levels these are — anchors, collapse, merge, the cap of seven — is decided by
    /// `chain_rings`, not here. The selected level is included but painted by the preview code
    /// (capture colour, double stroke), and each ring carries whether it is inside the selection:
    /// **inner rings are painted above the preview wash**, outer ones below it, because a wash
    /// painted over the inner rings is what made them invisible in the prototype.
    pub chain_rings: Vec<ChainRingView>,
    /// Text beside the automatic-snap preview (docs/21 §5.21); `None` = no label.
    pub preview_label: Option<String>,
    /// The previewed box is the whole window rather than an element: neutral wash, thin outline.
    pub preview_is_window: bool,
    /// How visible the preview box is, `0.0..=1.0` (docs/21 §5.24, ②): it eases in when it appears.
    pub preview_alpha: f32,
    /// How "walking" the level chain is right now, `0.0..=1.0` (docs/21 §5.24, A3).
    ///
    /// The capture green is a **walk signal**: at rest the box that would be taken is brand blue,
    /// and it lifts towards the capture colour while the level walk is live. What identifies the box
    /// itself is the mask hole ([`Self::mask_hole`]), so the colour is free to say "you moved"
    /// instead of "this is the one".
    pub capture_green: f32,
    /// How many wheel stops the level walk still has in each direction (docs/21 §5.24, A1); `None`
    /// while the answer itself is selected, which is the state "nothing has been walked".
    pub level_badge: Option<LevelReach>,
    /// One-shot hint in back-buffer coordinates (docs/21 §5.21).
    pub hint: Option<(Point, String)>,
    /// The scroll session's panel, while one runs (`docs/33`). `None` for
    /// every other mode and on the export path, where a panel would be burned into the image.
    pub(crate) scroll: Option<ScrollPanelView>,
    /// Blur the frame outside the mask hole (a setting). Off on the export path.
    pub mask_blur: bool,
    /// The loupe's hint row, with the keys as bound.
    pub loupe_hints: Arc<[LoupeHint]>,
    /// The bottom-left shortcut guide while it is (fading) on screen; `None` otherwise and on the
    /// export path.
    pub(crate) guide: Option<GuideView>,
}

impl RenderView {
    /// A view for an empty session on `frame`.
    ///
    /// The production path always builds the view field by field from the session
    /// state; this is the starting point the composition tests use.
    #[cfg(test)]
    pub fn new(frame: Rect) -> Self {
        Self {
            frame,
            selection: Rect::default(),
            cursor: Point::default(),
            screen_origin: Point::default(),
            cursor_visible: false,
            show_chrome: false,
            work_area: frame,
            magnifier_rgb: None,
            magnifier_color_text: None,
            magnifier_relative: false,
            magnifier_zoom: MagnifierConfig::ZOOM_DEFAULT,
            annotation_items: Vec::new(),
            annotation_selected_id: None,
            annotation_draft: None,
            hover_bounds: None,
            preview_bounds: None,
            chain_rings: Vec::new(),
            preview_label: None,
            preview_is_window: false,
            preview_alpha: 1.0,
            capture_green: 0.0,
            level_badge: None,
            hint: None,
            scroll: None,
            mask_blur: false,
            loupe_hints: crate::keymap::Keymap::default().loupe_hints().into(),
            guide: None,
        }
    }

    /// The region that stays at original brightness — the hole in the mask (docs/21 §5.24, A2).
    ///
    /// It is the box that would be **captured**, and only that: choosing a box means looking at its
    /// content, and content seen through a 45% mask is not the content. Three cases, in order:
    ///
    /// * an element preview punches **its own** hole — the content of the box you are choosing must
    ///   look like itself;
    /// * the whole-window fallback punches nothing: its "box" is the screen, so a hole that size
    ///   would erase the mask and the capture-mode reading with it (only the neutral wash and the
    ///   thin outline mark it, docs/21 §5.21);
    /// * with no preview at all the hole is the settled selection — exactly what it was before.
    ///
    /// Deriving it here rather than storing a second field keeps two invariants free: a preview
    /// confirmed by a click turns into the selection and the hole follows it without a seam, and the
    /// export path — which already clears `preview_bounds` — can never hand an artifact a hole.
    pub fn mask_hole(&self) -> Rect {
        match self.preview_bounds {
            Some(preview) if !self.preview_is_window => preview,
            _ => self.selection,
        }
    }
}

/// Brushes and bitmaps the L2 chrome needs.
///
/// Collected by the caller so the draw helpers never borrow the renderer's fields
/// while also calling `&mut self` methods on it.
struct ChromeResources {
    frame_bitmap: Option<ID2D1Bitmap1>,
    label_text: ID2D1SolidColorBrush,
    crosshair: ID2D1SolidColorBrush,
    /// Light-blue translucent row/column bands through the sampled pixel.
    magnifier_band: ID2D1SolidColorBrush,
    magnifier_grid: ID2D1SolidColorBrush,
    /// Near-black: info-panel background, panel outline and the sampled cell's
    /// counter-stroke.
    magnifier_info: ID2D1SolidColorBrush,
}

/// Overlay renderer: owns the D2D/DirectWrite resources and the swap chain.
pub struct OverlayRenderer {
    device: Arc<GraphicsDevice>,
    d2d: ID2D1DeviceContext,
    dwrite: IDWriteFactory,
    label_formats: Vec<(u32, IDWriteTextFormat)>,
    swap_chain: Option<IDXGISwapChain1>,
    target: Option<ID2D1Bitmap1>,
    frame_bitmap: Option<ID2D1Bitmap1>,
    border_brush: Option<ID2D1SolidColorBrush>,
    mask_brush: Option<ID2D1SolidColorBrush>,
    /// `rgba(255,255,255,0.10)` — lifts the hovered window out of the dark mask.
    hover_fill_brush: Option<ID2D1SolidColorBrush>,
    /// Accent wash at low alpha — marks the automatic-snap preview.
    preview_fill_brush: Option<ID2D1SolidColorBrush>,
    /// The preview's outline, mutable: it carries the walk signal (docs/21 §5.24, A3).
    preview_stroke_brush: Option<ID2D1SolidColorBrush>,
    /// The level badge's hairline: its outline and the divider between the two directions.
    badge_line_brush: Option<ID2D1SolidColorBrush>,
    /// The level badge's dimmed half: the direction with no stops left (docs/21 §5.24, A1).
    badge_dim_brush: Option<ID2D1SolidColorBrush>,
    /// Capture green at full opacity — the outline of the box that will be captured (docs/21 §5.22).
    capture_brush: Option<ID2D1SolidColorBrush>,
    /// Mutable ring brush: the level chain, with each ring's own opacity set per frame.
    chain_ring_brush: Option<ID2D1SolidColorBrush>,
    /// Constant dark underlay for the rings (see [`CHAIN_RING_SHADOW`]).
    chain_shadow_brush: Option<ID2D1SolidColorBrush>,
    /// Theme colour laid over the neutral mask at a few percent (docs/21 §5.22): brand presence,
    /// not darkening — the black mask does that.
    mask_tint_brush: Option<ID2D1SolidColorBrush>,
    handle_brush: Option<ID2D1SolidColorBrush>,
    label_background_brush: Option<ID2D1SolidColorBrush>,
    label_text_brush: Option<ID2D1SolidColorBrush>,
    crosshair_brush: Option<ID2D1SolidColorBrush>,
    magnifier_band_brush: Option<ID2D1SolidColorBrush>,
    magnifier_grid_brush: Option<ID2D1SolidColorBrush>,
    magnifier_info_brush: Option<ID2D1SolidColorBrush>,
    // ── Magnifier info-panel palette (glassmorphism strip) ──
    /// Pure white — the primary S-cycled colour value (`#RRGGBB` /
    /// `rgb(...)` / `hsl(...)`).
    info_hex_brush: Option<ID2D1SolidColorBrush>,
    /// `rgba(232,236,242,.72)` — the position text.
    info_coord_value_brush: Option<ID2D1SolidColorBrush>,
    /// `rgba(232,236,242,.62)` — the hint labels ("格式" / "复制" …).
    info_hint_brush: Option<ID2D1SolidColorBrush>,
    /// `rgba(255,255,255,0.1)` — the key cap fill.
    info_kbd_fill_brush: Option<ID2D1SolidColorBrush>,
    /// `rgba(255,255,255,.55)` — the colour swatch's 1px border.
    info_swatch_border_brush: Option<ID2D1SolidColorBrush>,
    /// Reusable mutable brush whose `SetColor` is updated to the sampled pixel
    /// every frame, avoiding a fresh brush per repaint.
    info_swatch_fill_brush: Option<ID2D1SolidColorBrush>,
    /// `CLSID_D2D1GaussianBlur`, created with the target; it runs once per frozen frame, into
    /// [`Self::blurred_frame`].
    blur_effect: Option<ID2D1Effect>,
    /// The frozen frame blurred once, built on the first draw that needs it and dropped with the
    /// frame. The info strip, the guide and the blurred mask crop from it, so no draw blurs.
    blurred_frame: Option<ID2D1Bitmap1>,
    /// `rgba(18,18,20,.78)` over the info strip's blurred crop.
    info_tint_brush: Option<ID2D1SolidColorBrush>,
    // ── Shortcut guide ──
    /// `rgba(14,16,20,.66)` over the guide's blurred crop.
    guide_tint_brush: Option<ID2D1SolidColorBrush>,
    /// Key cap fill `rgba(236,239,244,.94)` and its text `#15181d`.
    guide_cap_brush: Option<ID2D1SolidColorBrush>,
    guide_cap_text_brush: Option<ID2D1SolidColorBrush>,
    /// Row labels (white at .92), group headers (.42), and the `/` `+` separators (.5).
    guide_label_brush: Option<ID2D1SolidColorBrush>,
    guide_head_brush: Option<ID2D1SolidColorBrush>,
    guide_sep_brush: Option<ID2D1SolidColorBrush>,
    /// The guide's layout for the groups, DPI and frame it was measured for.
    guide_layout: Option<guide_pass::CachedGuide>,
    /// `rgba(60,60,60,0.82)` — background chip behind the zoom badge drawn in
    /// the loupe panel's top-right corner.
    info_badge_bg_brush: Option<ID2D1SolidColorBrush>,
    /// Cache of every info-panel font variant, keyed by
    /// `(family slot, rounded DIP size * 100, weight as u32, alignment as u32)`.
    info_formats: Vec<((u8, u32, u32, u32), IDWriteTextFormat)>,
    /// Mutable stroke brush reused for all annotation items (SetColor per item).
    ann_stroke_brush: Option<ID2D1SolidColorBrush>,
    /// Mutable fill brush reused for filled annotation items.
    ann_fill_brush: Option<ID2D1SolidColorBrush>,
    // ── Scroll panel (docs/33) ────────────────────────────────────────────────
    /// The panel's backdrop.
    scroll_panel_brush: Option<ID2D1SolidColorBrush>,
    /// The viewport outline: the interface accent.
    scroll_box_brush: Option<ID2D1SolidColorBrush>,
    /// One layer of the panel's shadow; three are stacked.
    scroll_shadow_brush: Option<ID2D1SolidColorBrush>,
    /// The title.
    scroll_title_brush: Option<ID2D1SolidColorBrush>,
    /// The size, the status line and the button labels.
    scroll_muted_brush: Option<ID2D1SolidColorBrush>,
    /// The buttons' fill.
    scroll_button_brush: Option<ID2D1SolidColorBrush>,
    /// The panel colour fading out over the well's top when the thumbnail is cut there.
    scroll_fade_brush: Option<ID2D1LinearGradientBrush>,
    /// The thumbnail, uploaded on every report that carries one.
    scroll_preview_bitmap: Option<ID2D1Bitmap1>,
    metrics: RenderMetrics,
    size: (u32, u32),
}

/// What the scroll panel shows, decided by the overlay; the pixels are the last uploaded thumbnail
/// ([`OverlayRenderer::update_scroll_preview`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScrollPanelView {
    pub layout: Layout,
    /// The uploaded thumbnail's `(width, rows)`.
    pub thumb: (u32, u32),
    /// The viewport's thumbnail rows `[start, end)`.
    pub viewport: (u32, u32),
    /// The header's right side: the image's `width×rows`.
    pub size: String,
    pub status: &'static str,
    /// The buttons' texts, in [`scroll_layout::BUTTONS`] order: key and label.
    pub buttons: [String; 3],
}

/// The scroll panel's colour, the demo's `rgba(18,20,24,.92)`.
const SCROLL_PANEL: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 18.0 / 255.0,
    g: 20.0 / 255.0,
    b: 24.0 / 255.0,
    a: 0.92,
};
const SCROLL_PANEL_RADIUS_DIP: f32 = 10.0;
const SCROLL_WELL_RADIUS_DIP: f32 = 6.0;
const SCROLL_BUTTON_RADIUS_DIP: f32 = 5.0;
const SCROLL_BUTTON_PAD_DIP: f32 = 3.0;
/// How far the header's and the status line's text sit from the panel's sides.
const SCROLL_TEXT_PAD_DIP: f32 = 10.0;
/// The status rectangle already starts at the well's inset.
const INSET_TO_TEXT_DIP: f32 = scroll_layout::INSET_DIP as f32;
/// How tall the fade over a cut thumbnail is.
const SCROLL_FADE_DIP: f32 = 28.0;

/// Where the thumbnail lands inside the well: 1:1, centred across, bottom-aligned, and cut to the
/// well's height from the top (the bottom is the newest content).
pub(crate) fn scroll_thumb_rect(well: Rect, thumb: (u32, u32)) -> Rect {
    let width = (thumb.0 as i32).min(well.width());
    let height = (thumb.1 as i32).min(well.height());
    let left = well.left + (well.width() - width) / 2;
    Rect::new(left, well.bottom - height, left + width, well.bottom)
}

/// What the selection frame carries besides its outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameMarks {
    handles: bool,
    size_label: bool,
}

impl FrameMarks {
    /// A selection being made or adjusted.
    const EDITING: Self = Self {
        handles: true,
        size_label: true,
    };
    /// A scroll session's selection: fixed, and its size is in the panel's header.
    const SCROLLING: Self = Self {
        handles: false,
        size_label: false,
    };
}

/// The frame's stroke: `width` wide with its outer edge on `selection`'s, so the outline lies
/// inside the selection (the demo's `strokeRect`).
pub(crate) fn frame_stroke_rect(selection: Rect, width: f32) -> D2D_RECT_F {
    let half = width / 2.0;
    D2D_RECT_F {
        left: selection.left as f32 + half,
        top: selection.top as f32 + half,
        right: selection.right as f32 - half,
        bottom: selection.bottom as f32 - half,
    }
}

/// How many pixels inside the selection's edge the frame paints at `dpi`: the stroke's width,
/// rounded up to cover its antialiased inner edge.
pub(crate) fn frame_band(dpi: u32) -> i32 {
    RenderMetrics::for_dpi(dpi).border_width.ceil() as i32
}

impl OverlayRenderer {
    pub fn new(device: Arc<GraphicsDevice>, dpi: u32) -> Result<Self, String> {
        let d2d = device.create_d2d_context()?;
        // Register the embedded subset *before* the DirectWrite factory is created,
        // then build an isolated factory whose first enumeration already contains
        // "HarmonyOS Sans SC". windows-rs does not bind
        // IDWriteFactory::ReloadSystemFonts, and a shared factory may have been
        // cached earlier by another subsystem, so an isolated factory is what
        // guarantees the private in-memory font resolves by family name.
        register_info_font_once();
        let dwrite: IDWriteFactory =
            unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_ISOLATED) }
                .map_err(|error| super::hresult("DWriteCreateFactory", &error))?;
        Ok(Self {
            device,
            d2d,
            dwrite,
            label_formats: Vec::new(),
            swap_chain: None,
            target: None,
            frame_bitmap: None,
            border_brush: None,
            mask_brush: None,
            hover_fill_brush: None,
            preview_fill_brush: None,
            preview_stroke_brush: None,
            badge_line_brush: None,
            badge_dim_brush: None,
            capture_brush: None,
            chain_ring_brush: None,
            chain_shadow_brush: None,
            mask_tint_brush: None,
            handle_brush: None,
            label_background_brush: None,
            label_text_brush: None,
            crosshair_brush: None,
            magnifier_band_brush: None,
            magnifier_grid_brush: None,
            magnifier_info_brush: None,
            info_hex_brush: None,
            info_coord_value_brush: None,
            info_hint_brush: None,
            info_kbd_fill_brush: None,
            info_swatch_border_brush: None,
            info_swatch_fill_brush: None,
            blur_effect: None,
            blurred_frame: None,
            info_tint_brush: None,
            guide_tint_brush: None,
            guide_cap_brush: None,
            guide_cap_text_brush: None,
            guide_label_brush: None,
            guide_head_brush: None,
            guide_sep_brush: None,
            guide_layout: None,
            info_badge_bg_brush: None,
            info_formats: Vec::new(),
            ann_stroke_brush: None,
            ann_fill_brush: None,
            scroll_panel_brush: None,
            scroll_box_brush: None,
            scroll_shadow_brush: None,
            scroll_title_brush: None,
            scroll_muted_brush: None,
            scroll_button_brush: None,
            scroll_fade_brush: None,
            scroll_preview_bitmap: None,
            metrics: RenderMetrics::for_dpi(dpi),
            size: (0, 0),
        })
    }

    pub fn metrics(&self) -> RenderMetrics {
        self.metrics
    }

    /// (Re)create the swap chain for a monitor-sized back buffer.
    ///
    /// Keeps the D3D11 device alive for the current session: only the swap chain and
    /// its D2D target are recreated when the overlay moves to a different monitor.
    pub fn ensure_back_buffer(&mut self, width: u32, height: u32) -> Result<(), String> {
        if width == 0 || height == 0 {
            return Err("overlay back buffer must have a non-zero size".into());
        }
        if self.size == (width, height) && self.target.is_some() {
            return Ok(());
        }
        self.release_target();
        let swap_chain = self.device.create_composition_swap_chain(width, height)?;
        let target = self.device.create_target_bitmap(&self.d2d, &swap_chain)?;
        unsafe { self.d2d.SetTarget(&target) };
        self.swap_chain = Some(swap_chain);
        self.target = Some(target);
        self.size = (width, height);
        self.recreate_resources()?;
        Ok(())
    }

    pub fn composition_target(
        &self,
        window: ::windows::Win32::Foundation::HWND,
    ) -> Result<super::d3d11::CompositionTarget, String> {
        self.device.create_composition_target(window)
    }

    /// Present the swap chain that DirectComposition is showing.
    pub fn swap_chain(&self) -> Option<&IDXGISwapChain1> {
        self.swap_chain.as_ref()
    }

    pub fn present(&self) -> Result<(), String> {
        let Some(swap_chain) = self.swap_chain.as_ref() else {
            return Ok(());
        };
        self.device.present(swap_chain)
    }

    /// Replace the L0 frame bitmap with a D2D bitmap created **over** an existing GPU
    /// texture.
    ///
    /// This is the zero-copy path used by Windows Graphics Capture: the captured
    /// texture is the L0 layer, so arming a session performs no GPU → CPU transfer and
    /// no CPU → GPU upload.
    pub fn set_frame_texture(
        &mut self,
        width: u32,
        height: u32,
        texture: &ID3D11Texture2D,
    ) -> Result<(), String> {
        let bitmap = super::d3d11::create_bitmap_from_texture(
            &self.d2d,
            texture,
            D2D1_BITMAP_OPTIONS_NONE,
            // Same reasoning as `update_frame`: the captured desktop is opaque and
            // copied verbatim, so its alpha must not participate in source-over.
            D2D1_ALPHA_MODE_IGNORE,
        )
        .map_err(|error| super::hresult("CreateBitmapFromDxgiSurface", &error))?;
        let _ = (width, height);
        self.frame_bitmap = Some(bitmap);
        self.blurred_frame = None;
        Ok(())
    }

    /// Replace the L0 frame bitmap with fresh pixels.
    ///
    /// Only used where CPU pixels are the source of truth (BitBlt at capture time and
    /// the headless visual regression tests).
    pub fn update_frame(&mut self, width: u32, height: u32, bgra: &[u8]) -> Result<(), String> {
        let expected = width as usize * height as usize * 4;
        if bgra.len() != expected {
            return Err(format!(
                "frame upload of {} bytes does not match {width}x{height}",
                bgra.len()
            ));
        }
        let properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                // The captured desktop is opaque and is copied verbatim, so its alpha
                // channel must not participate in the source-over blend: treating it as
                // premultiplied would darken every pixel by its own coverage.
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        let bitmap = unsafe {
            self.d2d.CreateBitmap(
                D2D_SIZE_U { width, height },
                Some(bgra.as_ptr() as *const _),
                width * 4,
                &properties,
            )
        }
        .map_err(|error| super::hresult("ID2D1DeviceContext::CreateBitmap", &error))?;
        self.frame_bitmap = Some(bitmap);
        self.blurred_frame = None;
        Ok(())
    }

    /// Update the DPI-dependent metrics (called on `WM_DPICHANGED`).
    pub fn set_dpi(&mut self, dpi: u32) {
        self.metrics = RenderMetrics::for_dpi(dpi);
        self.label_formats.clear();
        self.info_formats.clear();
    }

    /// Draw one frame of the overlay into the swap chain back buffer.
    pub fn render(&mut self, view: &RenderView) -> Result<(), String> {
        let Some(target) = self.target.clone() else {
            return Err("overlay renderer has no back buffer".into());
        };
        self.draw_to(&target, view)
    }

    /// Render the exportable composition into a fresh frame-sized offscreen target
    /// and crop `view.selection` back to CPU BGRA.
    ///
    /// Reuses the exact [`Self::draw_to`] layer code the preview presents, so the
    /// exported pixels are the same geometry + style path (docs/11 §8.3). The caller
    /// supplies a view with `show_chrome`, the selection box and the draft cleared, so
    /// the crop is the selected region's frozen frame plus its committed annotations —
    /// never the mask, chrome, grips, magnifier or control points, which must not enter
    /// the artifact (docs/11 §8.2/§8.4).
    pub fn render_export(&mut self, view: &RenderView) -> Result<Vec<u8>, String> {
        // Artifact pixels never contain interactive hints. Enforced here — the one place
        // that produces them — so the invariant does not depend on every caller
        // remembering to clear them (docs/14 §8).
        let view = &RenderView {
            hover_bounds: None,
            preview_bounds: None,
            chain_rings: Vec::new(),
            preview_label: None,
            preview_is_window: false,
            level_badge: None,
            hint: None,
            scroll: None,
            mask_blur: false,
            guide: None,
            ..view.clone()
        };
        let (frame_w, frame_h) = self.size;
        if frame_w == 0 || frame_h == 0 {
            return Err("overlay renderer has no back-buffer size for export".into());
        }
        let selection = view.selection.intersect(view.frame);
        if selection.is_empty() {
            return Ok(Vec::new());
        }
        let gpu = self.device.create_render_target_texture(frame_w, frame_h)?;
        let target = super::d3d11::create_bitmap_from_texture(
            &self.d2d,
            &gpu.texture,
            D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
        .map_err(|error| super::hresult("CreateBitmapFromDxgiSurface(export)", &error))?;

        self.draw_to(&target, view)?;

        // draw_to repoints the shared device context at the offscreen target; restore
        // it to the swap chain so any later preview draw is unaffected.
        unsafe {
            match self.target.as_ref() {
                Some(swap_target) => self.d2d.SetTarget(swap_target),
                None => self.d2d.SetTarget(None),
            }
        }

        self.device.read_back_region_bgra(
            &gpu.texture,
            selection.left.max(0) as u32,
            selection.top.max(0) as u32,
            selection.width() as u32,
            selection.height() as u32,
        )
    }

    /// Draw the full L0/L1/L2/L3 composition into an arbitrary D2D target.
    ///
    /// The swap chain path and the visual regression tests share this, so the tested
    /// composition is exactly the one the overlay presents.
    ///
    /// The whole surface is repainted on every present. The swap chain is a
    /// `FLIP_SEQUENTIAL` buffer pair whose back buffer is *not* cleared by `BeginDraw`,
    /// so a pixel left outside a clip keeps the content it had two presents ago and is
    /// flipped back in — exactly the trailing ghost a moving selection, crosshair or
    /// size label would otherwise leave. Repainting the opaque L0 frame over the entire
    /// target makes each presented buffer self-authoritative; the ≤60 Hz input
    /// coalescing (one present per render tick) is what bounds the cost.
    pub fn draw_to(&mut self, target: &ID2D1Bitmap1, view: &RenderView) -> Result<(), String> {
        if needs_backdrop(view) {
            self.ensure_blurred_frame()?;
        }
        unsafe {
            self.d2d.SetTarget(target);
            self.d2d.BeginDraw();
            self.d2d.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.d2d
                .SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        }
        let result = self.draw_layers(view);
        let end_draw = unsafe { self.d2d.EndDraw(None, None) }
            .map_err(|error| super::hresult("ID2D1DeviceContext::EndDraw", &error));
        result?;
        end_draw
    }

    /// Blur the frozen frame into [`Self::blurred_frame`], once per frame.
    ///
    /// σ is 6 DIP. The blur runs into its own target between draws, so the frames that crop from it
    /// cost one `DrawBitmap` each; a per-draw blur cost a full kernel pass over the strip every
    /// present. A 3840×2160 frame keeps about 33 MB of video memory for the session.
    fn ensure_blurred_frame(&mut self) -> Result<(), String> {
        if self.blurred_frame.is_some() {
            return Ok(());
        }
        let (Some(frame), Some(blur)) = (self.frame_bitmap.clone(), self.blur_effect.clone())
        else {
            return Ok(());
        };
        let size = unsafe { frame.GetPixelSize() };
        let properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
            ..Default::default()
        };
        let blurred = unsafe { self.d2d.CreateBitmap(size, None, 0, &properties) }
            .map_err(|error| super::hresult("ID2D1DeviceContext::CreateBitmap(blur)", &error))?;
        let input: ID2D1Image = frame
            .cast()
            .map_err(|error| super::hresult("ID2D1Bitmap1::cast", &error))?;
        let std_dev = BACKDROP_BLUR_DIP * (self.metrics.dpi.max(96) as f32 / 96.0);
        unsafe {
            blur.SetInput(0, &input, true);
            blur.SetValue(
                D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                D2D1_PROPERTY_TYPE_UNKNOWN,
                &std_dev.to_ne_bytes(),
            )
            .map_err(|error| super::hresult("ID2D1Effect::SetValue(STANDARD_DEVIATION)", &error))?;
            let output: ID2D1Image = blur
                .cast()
                .map_err(|error| super::hresult("ID2D1Effect::cast", &error))?;
            self.d2d.SetTarget(&blurred);
            self.d2d.BeginDraw();
            self.d2d.DrawImage(
                &output,
                None,
                None,
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
            let ended = self.d2d.EndDraw(None, None);
            // The effect holds its input; let go of the frame so dropping it frees it.
            blur.SetInput(0, None::<&ID2D1Image>, true);
            ended.map_err(|error| super::hresult("ID2D1DeviceContext::EndDraw(blur)", &error))?;
        }
        self.blurred_frame = Some(blurred);
        Ok(())
    }

    /// Whether an L0 frame bitmap is loaded (diagnostics and tests).
    #[allow(dead_code)]
    pub fn has_frame_bitmap(&self) -> bool {
        self.frame_bitmap.is_some()
    }

    /// The D3D11 device behind this renderer.
    ///
    /// Used by the visual regression tests to read a rendered target back to CPU
    /// pixels; nothing on the interactive path needs it.
    #[allow(dead_code)]
    pub fn device(&self) -> &GraphicsDevice {
        &self.device
    }

    fn draw_layers(&mut self, view: &RenderView) -> Result<(), String> {
        // A scroll session: the page under the selection is live, and the window is cut down to
        // the frame's band and the panel (`overlay/scroll.rs`). There is no frozen frame to show
        // and nothing to mask — the surface is transparent apart from the selection frame, drawn by
        // the same routine as a plain capture's, and the panel.
        if let Some(scroll) = view.scroll.as_ref() {
            unsafe { self.d2d.Clear(Some(&color(0.0, 0.0, 0.0, 0.0))) };
            self.draw_selection_frame(view, FrameMarks::SCROLLING)?;
            return self.draw_scroll_panel(scroll);
        }
        let frame_bitmap = self.frame_bitmap.clone();
        let mask = self.require_brush(&self.mask_brush, "mask brush")?;

        unsafe {
            // L0: the frozen frame, drawn 1:1 so selected pixels stay pixel exact.
            match frame_bitmap.as_ref() {
                Some(bitmap) => self.d2d.DrawBitmap(
                    bitmap,
                    Some(&to_d2d(view.frame)),
                    1.0,
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    None,
                    None,
                ),
                None => {
                    let clear = color(0.0, 0.0, 0.0, 1.0);
                    self.d2d.Clear(Some(&clear));
                }
            }
        }

        // L1: mask everything outside the hole. Using four bands (instead of
        // re-drawing the frame inside the hole) keeps the selected pixels at their
        // original brightness and lets a drag touch only the changed bands.
        //
        // The hole is the box that would be *taken* (docs/21 §5.24, A2), which during a hover is
        // the previewed element rather than the settled selection — see `RenderView::mask_hole`.
        let hole = view.mask_hole();
        // The blurred mask (a setting): the frame blurred outside the hole, under the same mask
        // and tint, so the selection is the one sharp thing on screen.
        if view.mask_blur
            && let Some(blurred) = self.blurred_frame.as_ref()
        {
            for band in outside_bands(view.frame, hole) {
                let band = to_d2d(band);
                unsafe {
                    self.d2d.DrawBitmap(
                        blurred,
                        Some(&band),
                        1.0,
                        D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                        Some(&band),
                        None,
                    )
                };
            }
        }
        fill_outside(&self.d2d, view.frame, hole, &mask);
        // …and a trace of the theme colour on top of it (docs/21 §5.22). Black does the darkening;
        // the tint is only brand presence. The prototype measured why it stays small: at 30% the
        // screen shifts towards the rings' hue and a blue ring drops from 3.1:1 to 1.2–1.8:1,
        // while 10% costs almost nothing.
        if let Some(tint) = self.mask_tint_brush.clone() {
            fill_outside(&self.d2d, view.frame, hole, &tint);
        }

        // L2: annotations (inside selection only, after mask, before chrome).
        if !view.annotation_items.is_empty() || view.annotation_draft.is_some() {
            let clip_rect = if view.selection.is_empty() {
                view.frame
            } else {
                view.selection.intersect(view.frame)
            };
            unsafe {
                self.d2d
                    .PushAxisAlignedClip(&to_d2d(clip_rect), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            }
            self.draw_annotations(view)?;
            unsafe {
                self.d2d.PopAxisAlignedClip();
            }
        }

        // L3: selection chrome.
        // L2.5: window-snap hints. Painted between the annotations and the selection
        // chrome so the preview reads as "this is what would be selected" without ever
        // impersonating the confirmed selection (docs/14 §8).
        self.draw_window_hints(view)?;

        // L3: selection chrome.
        if view.show_chrome && !view.selection.is_empty() {
            let resources = ChromeResources {
                frame_bitmap,
                label_text: self.require_brush(&self.label_text_brush, "label text brush")?,
                crosshair: self.require_brush(&self.crosshair_brush, "crosshair brush")?,
                magnifier_band: self
                    .require_brush(&self.magnifier_band_brush, "magnifier band brush")?,
                magnifier_grid: self
                    .require_brush(&self.magnifier_grid_brush, "magnifier grid brush")?,
                magnifier_info: self
                    .require_brush(&self.magnifier_info_brush, "magnifier info brush")?,
            };
            self.draw_chrome(view, &resources)?;
        }

        // L3.5: the shortcut guide, above the chrome.
        if let Some(guide) = view.guide.as_ref() {
            self.draw_guide(guide, view.frame)?;
        }
        Ok(())
    }

    /// L2.5: the hovered window and the automatic-snap preview (docs/14 §8).
    ///
    /// Hover is a neutral wash with a thin accent outline; the preview adds a stronger
    /// accent wash at double stroke weight so it is unmistakably a preview. Both are
    /// clipped to the back buffer, and neither touches the mask hole or the confirmed
    /// selection: they are independent layers, which is what stops a preview from
    /// silently rewriting a settled selection.
    fn draw_window_hints(&mut self, view: &RenderView) -> Result<(), String> {
        if view.hover_bounds.is_none()
            && view.preview_bounds.is_none()
            && view.chain_rings.is_empty()
            && view.hint.is_none()
        {
            return Ok(());
        }
        let border = self.require_brush(&self.border_brush, "border brush")?;
        let width = self.metrics.border_width;

        // ── The level chain, in two passes around the washes (docs/21 §5.22) ──
        //
        // Outer rings first: they are the context *around* the selection, and the washes below
        // should read as sitting inside them. Inner rings are painted after the preview wash
        // instead — they live inside it, and painting them underneath (the prototype's first
        // version) hides them completely: accent line under accent wash is nearly nothing.
        //
        // One rectangle, one stroke: the hovered window frame is `path[0]`, so its ring is skipped
        // when the hover box is the same rectangle — otherwise the window frame gets two outlines.
        let hovered = view.hover_bounds;
        self.paint_chain_rings(view, false, hovered, None)?;

        if let Some(hover) = view.hover_bounds {
            let rect = hover.intersect(view.frame);
            if !rect.is_empty() {
                let fill = self.require_brush(&self.hover_fill_brush, "hover fill brush")?;
                unsafe {
                    // The wash lifts the hovered window out of the mask — but not out of the *hole*:
                    // the box that would be captured shows its own pixels, and a 10% white veil over
                    // them would be exactly the veil that A2 exists to remove (docs/21 §5.24).
                    fill_outside(&self.d2d, rect, view.mask_hole(), &fill);
                    self.d2d.DrawRectangle(&to_d2d(rect), &border, width, None);
                }
            }
        }

        if let Some(preview) = view.preview_bounds {
            let rect = preview.intersect(view.frame);
            if !rect.is_empty() {
                // A whole-window answer is the v1 fallback, not an element pick: it reads as the
                // neutral hover wash with a thin outline, so "we could not get below the window" is
                // visible instead of looking like a confident element preview (docs/21 §5.21).
                //
                // An element answer is the *capture*, and its colour is the walk signal (docs/21
                // §5.24, A3): brand blue while the chain is at rest, lifting towards the capture
                // green while the level walk is live, with a wash to match (none at rest — the mask
                // hole already shows the content at its own brightness).
                let (fill, stroke, stroke_width) = if view.preview_is_window {
                    (
                        self.require_brush(&self.hover_fill_brush, "hover fill brush")?,
                        border.clone(),
                        width,
                    )
                } else {
                    let walking = view.capture_green.clamp(0.0, 1.0);
                    // ②: the box eases in when it appears; the hole in the mask is already open, so
                    // the content is at its own brightness while the outline is still arriving.
                    let appear = view.preview_alpha.clamp(0.0, 1.0);
                    let mix =
                        self.require_brush(&self.preview_stroke_brush, "preview stroke brush")?;
                    let fill =
                        self.require_brush(&self.preview_fill_brush, "preview fill brush")?;
                    unsafe {
                        let _ = mix.SetColor(&capture_mix_color(walking, appear));
                        let _ = fill.SetColor(&capture_wash_color(walking, appear));
                    }
                    (fill, mix, width * 2.0)
                };
                unsafe {
                    self.d2d.FillRectangle(&to_d2d(rect), &fill);
                    self.d2d
                        .DrawRectangle(&to_d2d(rect), &stroke, stroke_width, None);
                }
            }
        }
        // …and now the inner rings, above the wash that would otherwise cover them.
        self.paint_chain_rings(view, true, hovered, view.preview_bounds)?;
        // The preview's own label: size, kind, level and whether anything answered for this
        // position. Drawn last so it sits above every outline it describes.
        if let (Some(preview), Some(text)) = (view.preview_bounds, view.preview_label.as_deref()) {
            let rect = preview.intersect(view.frame);
            if !rect.is_empty() {
                self.draw_hint_at(rect, text, view, self.metrics)?;
            }
        }
        if let Some((at, text)) = view.hint.as_ref() {
            self.draw_hint_at(Rect::new(at.x, at.y, at.x, at.y), text, view, self.metrics)?;
        }
        // The level badge, last: it sits inside the box, over everything else there (docs/21 §5.22).
        if let (Some(reach), Some(preview)) = (view.level_badge, view.preview_bounds) {
            let rect = preview.intersect(view.frame);
            if !rect.is_empty() {
                self.draw_level_badge(rect, reach, view, self.metrics)?;
            }
        }
        Ok(())
    }

    /// The level badge: how many stops the wheel still has in each direction (docs/21 §5.24, A1).
    ///
    /// Two cells — `↑n` toward the window frame, `↓n` toward the answer — in a fixed 54×18 chip,
    /// split by a hairline. The side with nothing left is dimmed: "you are against the end" is the
    /// one thing the wheel gives no other sign of, and it is exactly when a notch does nothing. The
    /// side that still moves carries the capture green, the same colour the walk puts on the box.
    ///
    /// This replaced the dot strip (v3's `dots` form). That form's virtues still hold — position was
    /// the ordinal and it needed no glyphs — but it could only say *where* you are in the chain, and
    /// after B1 some levels have no ring of their own: `8/9` pointed at a level that may not exist on
    /// screen, while `↓5` is how many notches still land somewhere new.
    fn draw_level_badge(
        &mut self,
        anchor: Rect,
        reach: LevelReach,
        view: &RenderView,
        metrics: RenderMetrics,
    ) -> Result<(), String> {
        use crate::geometry::{LEVEL_BADGE_SIZE, level_badge_placement};

        /// Where a half's arrow and number sit, in logical pixels (the prototype's numbers).
        const PAD: f32 = 6.0;
        const NUMBER_OFFSET: f32 = 10.0;
        /// The second half starts this far in, so the divider has room on both sides of it.
        const SECOND_HALF_INSET: f32 = 2.0;

        let scale = (metrics.dpi as f32 / 96.0).max(1.0);
        let size = (
            (LEVEL_BADGE_SIZE.0 as f32 * scale).round() as i32,
            (LEVEL_BADGE_SIZE.1 as f32 * scale).round() as i32,
        );
        let work_area = if view.work_area.is_empty() {
            view.frame
        } else {
            view.work_area
        };
        let Some((panel, _where)) = level_badge_placement(
            anchor,
            size,
            work_area,
            (metrics.label_gap * scale).round() as i32,
            Some(view.cursor),
        ) else {
            return Ok(());
        };

        let background =
            self.require_brush(&self.label_background_brush, "label background brush")?;
        let text = self.require_brush(&self.label_text_brush, "label text brush")?;
        let dim = self.require_brush(&self.badge_dim_brush, "badge dim brush")?;
        let line = self.require_brush(&self.badge_line_brush, "badge line brush")?;
        let capture = self.require_brush(&self.capture_brush, "capture brush")?;
        // Same font, size and alignment as the size label: the chip and the label are one UI, and
        // `↑` `↓` are already in the embedded subset (the level hint draws them).
        let format = self.label_text_format_mut()?;
        let half = panel.width() as f32 / 2.0;
        let top = panel.top as f32;
        let bottom = panel.bottom as f32;
        let radius = 3.0 * scale;
        let rounded = D2D1_ROUNDED_RECT {
            rect: to_d2d(panel),
            radiusX: radius,
            radiusY: radius,
        };
        unsafe {
            self.d2d.FillRoundedRectangle(&rounded, &background);
            // The hairline the prototype draws: it lifts an opaque chip off bright content, which a
            // dark panel alone does not do, and doubles as the divider between the directions.
            self.d2d.DrawRoundedRectangle(&rounded, &line, 1.0, None);
            let divider = (panel.left as f32 + half).round() as i32;
            let inset = (4.0 * scale).round() as i32;
            self.d2d.FillRectangle(
                &to_d2d(Rect::new(
                    divider,
                    panel.top + inset,
                    divider + 1,
                    panel.bottom - inset,
                )),
                &line,
            );
            // `↑` first: up the chain is toward the window frame, down toward the answer.
            for (index, (arrow, stops)) in
                [("↑", reach.up), ("↓", reach.down)].into_iter().enumerate()
            {
                // Nothing left that way: dim it, so "a notch here does nothing" is visible before
                // the notch is spent rather than after.
                let live = stops > 0;
                let arrow_x = panel.left as f32
                    + index as f32 * half
                    + (if index == 0 { PAD } else { SECOND_HALF_INSET }) * scale;
                let number_x = arrow_x + NUMBER_OFFSET * scale;
                let (arrow_brush, number_brush) = if live {
                    (&capture, &text)
                } else {
                    (&dim, &dim)
                };
                let glyph = arrow.encode_utf16().collect::<Vec<u16>>();
                let number = stops.to_string().encode_utf16().collect::<Vec<u16>>();
                self.d2d.DrawText(
                    &glyph,
                    &format,
                    &D2D_RECT_F {
                        left: arrow_x,
                        top,
                        right: number_x,
                        bottom,
                    },
                    arrow_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
                self.d2d.DrawText(
                    &number,
                    &format,
                    &D2D_RECT_F {
                        left: number_x,
                        top,
                        right: panel.right as f32,
                        bottom,
                    },
                    number_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
        }
        Ok(())
    }

    /// A small panel with `text` anchored to `anchor` (docs/21 §5.21).
    /// Paint one pass of the level chain: `inner == false` for the rings outside the selection,
    /// `inner == true` for the ones inside it (docs/21 §5.22).
    ///
    /// `skip` rectangles are drawn by other layers — the hovered window frame and the preview box
    /// have their own strokes, and a level that coincides with one of them must not be stroked a
    /// second time.
    fn paint_chain_rings(
        &mut self,
        view: &RenderView,
        inner: bool,
        skip: Option<Rect>,
        also_skip: Option<Rect>,
    ) -> Result<(), String> {
        if view.chain_rings.is_empty() {
            return Ok(());
        }
        let brush = self.require_brush(&self.chain_ring_brush, "chain ring brush")?;
        let shadow = self.require_brush(&self.chain_shadow_brush, "chain shadow brush")?;
        // One *logical* pixel, scaled like every other length in the overlay: the first version used
        // a raw physical pixel, which at the user's 144 DPI is 0.67 logical px — thinner than the
        // prototype's line ever was, on a screen that shows it at 1.5 physical px.
        let width = (self.metrics.dpi as f32 / 96.0).max(1.0);
        for ring in view.chain_rings.iter().filter(|ring| ring.inner == inner) {
            let rect = ring.rect.intersect(view.frame);
            if rect.is_empty() {
                continue;
            }
            if skip.is_some_and(|other| other.intersect(view.frame) == rect)
                || also_skip.is_some_and(|other| other.intersect(view.frame) == rect)
            {
                continue;
            }
            // One mutable brush, `SetColor` per ring: at most seven calls a frame and no
            // allocation, the same trick the annotation strokes already use.
            // Dark underlay first (one pixel wider), then the ring itself: the pair carries an edge
            // no screenshot can match in luminance, which a single blue line cannot promise.
            unsafe {
                self.d2d.DrawRectangle(
                    &to_d2d(rect),
                    &shadow,
                    width + CHAIN_RING_SHADOW_WIDTH,
                    None,
                );
                let _ = brush.SetColor(&chain_ring_color(ring.alpha));
                self.d2d.DrawRectangle(&to_d2d(rect), &brush, width, None);
            }
        }
        Ok(())
    }

    /// A small panel with `text` anchored to `anchor` (docs/21 §5.21).
    ///
    /// `anchor` is a rectangle for the preview label (placed beside it, above or below, whichever
    /// fits the work area) or a degenerate point for the one-shot hint. Same brushes, padding and
    /// glyph format as the selection's size label, so the overlay reads as one UI.
    fn draw_hint_at(
        &mut self,
        anchor: Rect,
        text: &str,
        view: &RenderView,
        metrics: RenderMetrics,
    ) -> Result<(), String> {
        let background =
            self.require_brush(&self.label_background_brush, "label background brush")?;
        let foreground = self.require_brush(&self.label_text_brush, "label text brush")?;
        let (measured, format) = self.measure_label(text)?;
        let Some(placement) = self.place_label(anchor, view, measured, metrics) else {
            // No room beside the box: dropping the label beats covering the pixels being chosen.
            return Ok(());
        };
        unsafe {
            let panel = D2D1_ROUNDED_RECT {
                rect: to_d2d(placement.rect),
                radiusX: 3.0,
                radiusY: 3.0,
            };
            self.d2d.FillRoundedRectangle(&panel, &background);
            let wide = text.encode_utf16().collect::<Vec<u16>>();
            let text_rect = D2D_RECT_F {
                left: placement.rect.left as f32 + metrics.label_padding_x,
                top: placement.rect.top as f32 + metrics.label_padding_y,
                right: placement.rect.right as f32 - metrics.label_padding_x,
                bottom: placement.rect.bottom as f32 - metrics.label_padding_y,
            };
            self.d2d.DrawText(
                &wide,
                &format,
                &text_rect,
                &foreground,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }

    fn require_brush(
        &self,
        brush: &Option<ID2D1SolidColorBrush>,
        name: &str,
    ) -> Result<ID2D1SolidColorBrush, String> {
        brush
            .clone()
            .ok_or_else(|| format!("{name} is unavailable"))
    }

    /// L3: the selection frame with its handles and size label, and the loupe.
    fn draw_chrome(
        &mut self,
        view: &RenderView,
        resources: &ChromeResources,
    ) -> Result<(), String> {
        self.draw_selection_frame(view, FrameMarks::EDITING)?;
        if view.cursor_visible {
            self.draw_magnifier(view, resources, self.metrics)?;
        }
        Ok(())
    }

    /// The selection frame: one routine for a selection being made and a scroll session's, which
    /// differ only in `marks`.
    ///
    /// The outline lies just inside the selection's edge ([`frame_stroke_rect`]): it stays whole
    /// against a screen edge, and it is the band a scroll session's window keeps ([`frame_band`]).
    /// The handles are centred on the edge.
    fn draw_selection_frame(&mut self, view: &RenderView, marks: FrameMarks) -> Result<(), String> {
        let selection = view.selection.intersect(view.frame);
        if selection.is_empty() {
            return Ok(());
        }
        let metrics = self.metrics;
        let border = self.require_brush(&self.border_brush, "border brush")?;
        unsafe {
            self.d2d.DrawRectangle(
                &frame_stroke_rect(selection, metrics.border_width),
                &border,
                metrics.border_width,
                None,
            );
        }
        if marks.handles {
            let handle = self.require_brush(&self.handle_brush, "handle brush")?;
            let radius = metrics.handle_size / 2.0;
            for grip in Handle::ALL {
                let anchor = grip.anchor(selection);
                let grip = D2D1_ELLIPSE {
                    point: vector2(anchor.x as f32, anchor.y as f32),
                    radiusX: radius,
                    radiusY: radius,
                };
                unsafe {
                    self.d2d.FillEllipse(&grip, &handle);
                    // Ring each white grip with the frame's colour, at the frame's width.
                    self.d2d
                        .DrawEllipse(&grip, &border, metrics.border_width, None);
                }
            }
        }
        if marks.size_label {
            self.draw_size_label(selection, view, metrics)?;
        }
        Ok(())
    }

    fn draw_size_label(
        &mut self,
        selection: Rect,
        view: &RenderView,
        metrics: RenderMetrics,
    ) -> Result<(), String> {
        let background =
            self.require_brush(&self.label_background_brush, "label background brush")?;
        let foreground = self.require_brush(&self.label_text_brush, "label text brush")?;
        let text = RenderMetrics::label_text_at(selection, view.screen_origin);
        let estimated = metrics.label_size_at(selection, view.screen_origin);

        // Position from an estimate, measure for real, then reposition when the two
        // disagree. Keeps the panel sized to the actual glyph run without adding a
        // frame of latency.
        let Some(first) = self.place_label(selection, view, estimated, metrics) else {
            // No room beside the selection: dropping the label beats covering the
            // pixels the user is choosing.
            return Ok(());
        };
        let (measured, format) = self.measure_label(&text)?;
        let placement = if measured != estimated {
            match self.place_label(selection, view, measured, metrics) {
                Some(placement) => placement,
                None => return Ok(()),
            }
        } else {
            first
        };

        unsafe {
            let panel = D2D1_ROUNDED_RECT {
                rect: to_d2d(placement.rect),
                radiusX: 3.0,
                radiusY: 3.0,
            };
            self.d2d.FillRoundedRectangle(&panel, &background);
            let wide = text.encode_utf16().collect::<Vec<u16>>();
            let text_rect = D2D_RECT_F {
                left: placement.rect.left as f32 + metrics.label_padding_x,
                top: placement.rect.top as f32 + metrics.label_padding_y,
                right: placement.rect.right as f32 - metrics.label_padding_x,
                bottom: placement.rect.bottom as f32 - metrics.label_padding_y,
            };
            self.d2d.DrawText(
                &wide,
                &format,
                &text_rect,
                &foreground,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }

    fn place_label(
        &self,
        selection: Rect,
        view: &RenderView,
        size: (i32, i32),
        metrics: RenderMetrics,
    ) -> Option<SizeLabelPlacement> {
        let work_area = if view.work_area.is_empty() {
            view.frame
        } else {
            view.work_area
        };
        crate::geometry::size_label_placement(
            selection,
            size,
            work_area,
            metrics.label_gap.round() as i32,
            Some(view.cursor),
        )
    }

    // ── Scroll capture panel (docs/33) ──────────────────────────────────────────────────────────

    /// The scroll session's panel (the demo's `drawScrolling`): a shadowed rounded card with the
    /// title and the image's size, the thumbnail's tail in a rounded well with the viewport
    /// outlined, the status line and the buttons. Every rectangle comes from [`Layout`], which the
    /// hit test and the window region use too.
    fn draw_scroll_panel(&mut self, scroll: &ScrollPanelView) -> Result<(), String> {
        let k = self.metrics.dpi.max(96) as f32 / 96.0;
        let backdrop = self.require_brush(&self.scroll_panel_brush, "scroll panel brush")?;
        let accent = self.require_brush(&self.scroll_box_brush, "scroll box brush")?;
        let shadow = self.require_brush(&self.scroll_shadow_brush, "scroll shadow brush")?;
        let title = self.require_brush(&self.scroll_title_brush, "scroll title brush")?;
        let muted = self.require_brush(&self.scroll_muted_brush, "scroll muted brush")?;
        let button = self.require_brush(&self.scroll_button_brush, "scroll button brush")?;
        let fade = self
            .scroll_fade_brush
            .clone()
            .ok_or("scroll fade brush is unavailable")?;
        let layout = &scroll.layout;
        let rounded = |rect: Rect, grow: f32, drop: f32, radius: f32| D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: rect.left as f32 - grow,
                top: rect.top as f32 - grow + drop,
                right: rect.right as f32 + grow,
                bottom: rect.bottom as f32 + grow + drop,
            },
            radiusX: radius + grow,
            radiusY: radius + grow,
        };
        let radius = SCROLL_PANEL_RADIUS_DIP * k;

        // The card: a soft shadow (three stacked layers, inside the layout's footprint), then the
        // card itself.
        unsafe {
            for grow in [1.5, 3.0, 4.0] {
                self.d2d.FillRoundedRectangle(
                    &rounded(layout.panel, grow * k, 2.0 * k, radius),
                    &shadow,
                );
            }
            self.d2d
                .FillRoundedRectangle(&rounded(layout.panel, 0.0, 0.0, radius), &backdrop);
        }

        // The header: the title on the left, the size on the right.
        let pad = SCROLL_TEXT_PAD_DIP * k;
        let header = D2D_RECT_F {
            left: layout.header.left as f32 + pad,
            // The demo sets the line 2 DIP below the row's middle.
            top: layout.header.top as f32 + 4.0 * k,
            right: layout.header.right as f32 - pad,
            bottom: layout.header.bottom as f32,
        };
        let title_format =
            self.info_text_format_mut(false, 12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, false)?;
        self.draw_text_in(scroll_layout::TITLE, &title_format, header, &title)?;
        let size_format =
            self.info_text_format_mut(true, 11.0, DWRITE_FONT_WEIGHT_NORMAL, false)?;
        let size_width = self.measure_text_width_in(&scroll.size, &size_format)?;
        self.draw_text_in(
            &scroll.size,
            &size_format,
            D2D_RECT_F {
                left: header.right - size_width.ceil(),
                ..header
            },
            &muted,
        )?;

        // The well: the thumbnail's newest rows, clipped to a rounded rectangle.
        self.draw_scroll_well(scroll, k, &accent, &fade)?;

        // The status line and the buttons.
        let small = self.info_text_format_mut(false, 11.0, DWRITE_FONT_WEIGHT_NORMAL, false)?;
        let status = to_d2d(layout.status);
        self.draw_text_in(
            scroll.status,
            &small,
            D2D_RECT_F {
                left: status.left + pad - INSET_TO_TEXT_DIP * k,
                ..status
            },
            &muted,
        )?;
        for (rect, label) in layout.buttons.iter().zip(&scroll.buttons) {
            unsafe {
                self.d2d.FillRoundedRectangle(
                    &rounded(*rect, 0.0, 0.0, SCROLL_BUTTON_RADIUS_DIP * k),
                    &button,
                );
            }
            // A rebound key can make a label wider than its button: it steps down to the floor,
            // and is clipped to the button there rather than spilling into the next one.
            let room = rect.width() as f32 - 2.0 * SCROLL_BUTTON_PAD_DIP * k;
            let size = magnifier_pass::shrink_to_fit(11.0, 9.0, |size| {
                let format =
                    self.info_text_format_mut(false, size, DWRITE_FONT_WEIGHT_NORMAL, true)?;
                Ok(self.measure_text_width_in(label, &format)? <= room)
            })?;
            let format = self.info_text_format_mut(false, size, DWRITE_FONT_WEIGHT_NORMAL, true)?;
            self.draw_text_in(label, &format, to_d2d(*rect), &title)?;
        }
        Ok(())
    }

    /// The thumbnail well: the bitmap's last rows bottom-aligned, the panel colour fading in over
    /// the top when older rows are cut there, and the viewport outlined like the selection.
    fn draw_scroll_well(
        &mut self,
        scroll: &ScrollPanelView,
        k: f32,
        accent: &ID2D1SolidColorBrush,
        fade: &ID2D1LinearGradientBrush,
    ) -> Result<(), String> {
        let well = scroll.layout.well;
        if well.is_empty() {
            return Ok(());
        }
        let thumb = scroll_thumb_rect(well, scroll.thumb);
        // The bitmap's last `thumb.height()` rows are shown: the bottom is the newest content.
        let skipped = scroll.thumb.1 as i32 - thumb.height();
        let factory = unsafe { self.d2d.GetFactory() }
            .map_err(|e| super::hresult("ID2D1DeviceContext::GetFactory", &e))?;
        let radius = SCROLL_WELL_RADIUS_DIP * k;
        let mask: ID2D1Geometry = unsafe {
            factory.CreateRoundedRectangleGeometry(&D2D1_ROUNDED_RECT {
                rect: to_d2d(well),
                radiusX: radius,
                radiusY: radius,
            })
        }
        .map_err(|e| super::hresult("CreateRoundedRectangleGeometry", &e))?
        .cast()
        .map_err(|e| super::hresult("cast to ID2D1Geometry", &e))?;
        let layer =
            unsafe { self.d2d.CreateLayer(None) }.map_err(|e| super::hresult("CreateLayer", &e))?;
        let parameters = D2D1_LAYER_PARAMETERS1 {
            contentBounds: to_d2d(well),
            geometricMask: std::mem::ManuallyDrop::new(Some(mask)),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: windows_numerics::Matrix3x2::identity(),
            opacity: 1.0,
            opacityBrush: std::mem::ManuallyDrop::new(None),
            layerOptions: Default::default(),
        };
        unsafe {
            self.d2d.PushLayer(&parameters, &layer);
            if let Some(bitmap) = self.scroll_preview_bitmap.as_ref()
                && !thumb.is_empty()
            {
                let source = D2D_RECT_F {
                    left: 0.0,
                    top: skipped as f32,
                    right: thumb.width() as f32,
                    bottom: scroll.thumb.1 as f32,
                };
                self.d2d.DrawBitmap(
                    bitmap,
                    Some(&to_d2d(thumb)),
                    1.0,
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    Some(&source),
                    None,
                );
            }
            if skipped > 0 {
                let top = well.top as f32;
                let bottom = top + SCROLL_FADE_DIP * k;
                fade.SetStartPoint(vector2(0.0, top));
                fade.SetEndPoint(vector2(0.0, bottom));
                self.d2d.FillRectangle(
                    &D2D_RECT_F {
                        top,
                        bottom,
                        ..to_d2d(well)
                    },
                    fade,
                );
            }
            let top = (thumb.top + scroll.viewport.0 as i32 - skipped).max(thumb.top);
            let bottom = (thumb.top + scroll.viewport.1 as i32 - skipped).min(thumb.bottom);
            if bottom > top {
                let width = self.metrics.border_width;
                self.d2d.DrawRectangle(
                    &frame_stroke_rect(Rect::new(thumb.left, top, thumb.right, bottom), width),
                    accent,
                    width,
                    None,
                );
            }
            self.d2d.PopLayer();
        }
        let _ = std::mem::ManuallyDrop::into_inner(parameters.geometricMask);
        Ok(())
    }

    /// One line of text in `rect`, laid out by `format` (vertically centred, never wrapped).
    fn draw_text_in(
        &self,
        text: &str,
        format: &IDWriteTextFormat,
        rect: D2D_RECT_F,
        brush: &ID2D1SolidColorBrush,
    ) -> Result<(), String> {
        if text.is_empty() {
            return Ok(());
        }
        let wide = text.encode_utf16().collect::<Vec<u16>>();
        unsafe {
            self.d2d.DrawText(
                &wide,
                format,
                &rect,
                brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }

    pub(crate) fn update_scroll_preview(
        &mut self,
        preview: &crate::scroll::Preview,
    ) -> Result<(), String> {
        if preview.width == 0 || preview.rows == 0 {
            self.scroll_preview_bitmap = None;
            return Ok(());
        }
        let expected = preview.width as usize * preview.rows as usize * 4;
        if preview.bytes.len() != expected {
            return Err(format!(
                "scroll preview upload of {} bytes does not match {}x{}",
                preview.bytes.len(),
                preview.width,
                preview.rows
            ));
        }
        let properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        let bitmap = unsafe {
            self.d2d.CreateBitmap(
                D2D_SIZE_U {
                    width: preview.width,
                    height: preview.rows,
                },
                Some(preview.bytes.as_ptr() as *const _),
                preview.width * 4,
                &properties,
            )
        }
        .map_err(|error| {
            super::hresult("ID2D1DeviceContext::CreateBitmap(scroll preview)", &error)
        })?;
        self.scroll_preview_bitmap = Some(bitmap);
        Ok(())
    }

    fn measure_label(&mut self, text: &str) -> Result<((i32, i32), IDWriteTextFormat), String> {
        let metrics = self.metrics;
        let format = self.label_text_format_mut()?;
        let wide = text.encode_utf16().collect::<Vec<u16>>();
        let layout = unsafe {
            self.dwrite
                .CreateTextLayout(&wide, &format, f32::INFINITY, f32::INFINITY)
        }
        .map_err(|error| super::hresult("IDWriteFactory::CreateTextLayout", &error))?;
        let mut text_metrics = DWRITE_TEXT_METRICS::default();
        unsafe { layout.GetMetrics(&mut text_metrics) }
            .map_err(|error| super::hresult("IDWriteTextLayout::GetMetrics", &error))?;
        let width = text_metrics.width.ceil() as i32 + (metrics.label_padding_x * 2.0) as i32;
        let height = text_metrics.height.ceil() as i32 + (metrics.label_padding_y * 2.0) as i32;
        Ok(((width.max(1), height.max(1)), format))
    }

    // ── Annotation rendering ─────────────────────────────────────────────────────────

    fn draw_annotations(&mut self, view: &RenderView) -> Result<(), String> {
        let stroke = self.require_brush(&self.ann_stroke_brush, "ann stroke brush")?;
        let fill = self.require_brush(&self.ann_fill_brush, "ann fill brush")?;

        // L2-a: committed items.
        for item in &view.annotation_items {
            self.draw_annotation_item(item, &stroke, &fill)?;
            if Some(item.id) == view.annotation_selected_id {
                self.draw_selection_box(item, &stroke)?;
            }
        }
        // L2-b: draft (currently being drawn, slightly translucent).
        if let Some(draft) = &view.annotation_draft {
            self.draw_annotation_item(draft, &stroke, &fill)?;
        }
        Ok(())
    }

    fn draw_annotation_item(
        &mut self,
        item: &AnnotationItem,
        stroke: &ID2D1SolidColorBrush,
        fill: &ID2D1SolidColorBrush,
    ) -> Result<(), String> {
        let opacity = item.style.opacity;
        let sc = item.style.stroke_color;
        let stroke_color = color(sc[0], sc[1], sc[2], sc[3] * opacity);
        let _ = unsafe { stroke.SetColor(&stroke_color) };

        if let Some(fc) = item.style.fill_color {
            let fill_color = color(fc[0], fc[1], fc[2], fc[3] * opacity);
            let _ = unsafe { fill.SetColor(&fill_color) };
        }
        let has_fill = item.style.fill_color.is_some();
        let sw = item.style.stroke_width;

        unsafe {
            match &item.geometry {
                AnnotationGeometry::Rect { bounds } => {
                    let r = to_d2d(*bounds);
                    if has_fill {
                        self.d2d.FillRectangle(&r, fill);
                    }
                    self.d2d.DrawRectangle(&r, stroke, sw, None);
                }
                AnnotationGeometry::Ellipse { bounds } => {
                    let cx = (bounds.left + bounds.right) as f32 / 2.0;
                    let cy = (bounds.top + bounds.bottom) as f32 / 2.0;
                    let ell = D2D1_ELLIPSE {
                        point: vector2(cx, cy),
                        radiusX: bounds.width() as f32 / 2.0,
                        radiusY: bounds.height() as f32 / 2.0,
                    };
                    if has_fill {
                        self.d2d.FillEllipse(&ell, fill);
                    }
                    self.d2d.DrawEllipse(&ell, stroke, sw, None);
                }
                AnnotationGeometry::Line { start, end } => {
                    self.d2d.DrawLine(
                        vector2(start.x as f32, start.y as f32),
                        vector2(end.x as f32, end.y as f32),
                        stroke,
                        sw,
                        None,
                    );
                }
                AnnotationGeometry::Arrow { start, end } => {
                    let sx = start.x as f32;
                    let sy = start.y as f32;
                    let ex = end.x as f32;
                    let ey = end.y as f32;
                    // Shaft
                    self.d2d
                        .DrawLine(vector2(sx, sy), vector2(ex, ey), stroke, sw, None);
                    // Two-line arrowhead
                    let head = item.style.arrow_head_size;
                    let angle = (ey - sy).atan2(ex - sx);
                    let (cos_a, sin_a) = (angle.cos(), angle.sin());
                    // Left barb
                    let lx = ex
                        - head
                            * (cos_a * (30f32).to_radians().cos()
                                + sin_a * (30f32).to_radians().sin());
                    let ly = ey
                        - head
                            * (sin_a * (30f32).to_radians().cos()
                                - cos_a * (30f32).to_radians().sin());
                    // Right barb
                    let rx = ex
                        - head
                            * (cos_a * (-30f32).to_radians().cos()
                                + sin_a * (-30f32).to_radians().sin());
                    let ry = ey
                        - head
                            * (sin_a * (-30f32).to_radians().cos()
                                - cos_a * (-30f32).to_radians().sin());
                    self.d2d
                        .DrawLine(vector2(ex, ey), vector2(lx, ly), stroke, sw, None);
                    self.d2d
                        .DrawLine(vector2(ex, ey), vector2(rx, ry), stroke, sw, None);
                }
                AnnotationGeometry::Freehand { points }
                | AnnotationGeometry::Highlight { points } => {
                    if points.len() < 2 {
                        return Ok(());
                    }
                    let alpha_mult = if matches!(item.kind, AnnotationKind::Highlight) {
                        0.35f32
                    } else {
                        1.0
                    };
                    let hl_width = if matches!(item.kind, AnnotationKind::Highlight) {
                        sw * 3.0
                    } else {
                        sw
                    };
                    let base = item.style.stroke_color;
                    if alpha_mult < 1.0 {
                        let hl_color =
                            color(base[0], base[1], base[2], base[3] * alpha_mult * opacity);
                        let _ = stroke.SetColor(&hl_color);
                    }
                    for w in points.windows(2) {
                        self.d2d.DrawLine(
                            vector2(w[0].x as f32, w[0].y as f32),
                            vector2(w[1].x as f32, w[1].y as f32),
                            stroke,
                            hl_width,
                            None,
                        );
                    }
                }
                AnnotationGeometry::Text { position, content } => {
                    self.draw_annotation_text(item, position, content, stroke)?;
                }
            }
        }
        Ok(())
    }

    fn draw_annotation_text(
        &mut self,
        item: &AnnotationItem,
        position: &Point,
        content: &str,
        stroke: &ID2D1SolidColorBrush,
    ) -> Result<(), String> {
        if content.is_empty() {
            return Ok(());
        }
        let format = self.label_text_format_mut()?;
        let wide = content.encode_utf16().collect::<Vec<u16>>();
        let fs = item.style.font_size;
        let approx_w = (content.chars().count() as f32 * fs * 0.55).max(10.0);
        let approx_h = fs * 2.0;
        let text_rect = D2D_RECT_F {
            left: position.x as f32,
            top: position.y as f32,
            right: position.x as f32 + approx_w,
            bottom: position.y as f32 + approx_h,
        };
        unsafe {
            self.d2d.DrawText(
                &wide,
                &format,
                &text_rect,
                stroke,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }

    fn draw_selection_box(
        &mut self,
        item: &AnnotationItem,
        _stroke: &ID2D1SolidColorBrush,
    ) -> Result<(), String> {
        // Use a distinct accent colour for the selection box (not the item's own colour).
        let sel_brush = self.require_brush(&self.border_brush, "border brush")?;
        let b = to_d2d(item.bounds());
        let handle_r = 4.0f32;
        unsafe {
            self.d2d.DrawRectangle(&b, &sel_brush, 1.5, None);
            // Four corner handles.
            for &(cx, cy) in &[
                (b.left, b.top),
                (b.right, b.top),
                (b.left, b.bottom),
                (b.right, b.bottom),
            ] {
                let grip = D2D1_ELLIPSE {
                    point: vector2(cx, cy),
                    radiusX: handle_r,
                    radiusY: handle_r,
                };
                self.d2d.FillEllipse(&grip, &sel_brush);
            }
        }
        Ok(())
    }
    /// Resolve (and cache) the DirectWrite format used by the size label.
    fn label_text_format_mut(&mut self) -> Result<IDWriteTextFormat, String> {
        // Cache by rounded font size: WM_DPICHANGED is the only thing that changes it.
        let key = (self.metrics.label_font_size * 100.0).round() as u32;
        if let Some((_, format)) = self.label_formats.iter().find(|(size, _)| *size == key) {
            return Ok(format.clone());
        }
        let mut format = None;
        let mut last_error = String::new();
        // `localeName` must be a real locale (or an empty string); passing null is
        // rejected with E_INVALIDARG on some DirectWrite versions.
        let locale = to_wide("en-us");
        // The size label uses the same embedded family, then the stock fallbacks.
        for family in [INFO_FONT_FAMILY, LABEL_FONT_FAMILY, LABEL_FONT_FALLBACK] {
            let wide = to_wide(family);
            match unsafe {
                self.dwrite.CreateTextFormat(
                    PCWSTR(wide.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    self.metrics.label_font_size,
                    PCWSTR(locale.as_ptr()),
                )
            } {
                Ok(created) => {
                    format = Some(created);
                    break;
                }
                Err(error) => {
                    last_error = super::hresult(family, &error);
                    continue;
                }
            }
        }
        let format = format.ok_or_else(|| format!("no usable label font family ({last_error})"))?;
        unsafe {
            let _ = format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
            let _ = format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        }
        self.label_formats.push((key, format.clone()));
        Ok(format)
    }

    fn recreate_resources(&mut self) -> Result<(), String> {
        // Premultiplied like a D2D brush wants it: the mask is black at 45 %.
        let (mask_r, mask_g, mask_b) = MASK_RGB.channels_f32();
        let mask = color(
            mask_r * MASK_ALPHA,
            mask_g * MASK_ALPHA,
            mask_b * MASK_ALPHA,
            MASK_ALPHA,
        );
        // Selection border and handle outline: `ACCENT_RGB`.
        let accent = color_of(ACCENT_RGB, 1.0);
        let white = color(1.0, 1.0, 1.0, 1.0);
        let panel = color(0.09, 0.09, 0.11, 0.92);
        let crosshair = color(1.0, 0.75, 0.0, 0.95);
        // Dark charcoal grid lines: keep them legible when the magnified
        // content is light (white-on-white was invisible on pale screens).
        let grid = color(0.12, 0.12, 0.16, 0.55);
        let info = color(0.02, 0.02, 0.025, 0.94);
        self.mask_brush = Some(self.create_brush(&mask)?);
        self.border_brush = Some(self.create_brush(&accent)?);
        // Window-snap hints reuse the existing palette: a neutral wash for the hovered
        // window and a low-alpha accent wash for the preview, so no new colour is
        // introduced (docs/14 §8).
        // The hover veil's alpha is part of the A4 stack (it is what the outer rings sit under), so
        // it comes from the shared palette too.
        let hover_fill = color_of(Rgb::new(255, 255, 255), HOVER_WASH_ALPHA);
        self.hover_fill_brush = Some(self.create_brush(&hover_fill)?);
        // The preview's wash and outline are *mutable*: at rest the box is brand blue with no wash,
        // and both lift towards the capture green while the level walk is live (docs/21 §5.24, A3).
        // The starting colours are the walking ones, so a frame drawn before anything sets them is
        // never an invisible box.
        let preview_fill = color_of(CAPTURE_RGB, PREVIEW_WASH_ALPHA * PREVIEW_WASH_WALK_SCALE);
        self.preview_fill_brush = Some(self.create_brush(&preview_fill)?);
        self.preview_stroke_brush = Some(self.create_brush(&capture_mix_color(1.0, 1.0))?);
        // The badge's hairline and its dimmed half (v3: rgba(255,255,255,.1–.14) and
        // rgba(200,208,219,.28); folded into one line brush, since the two hairlines differ by four
        // percent of alpha and no one can see it).
        self.badge_line_brush = Some(self.create_brush(&color(1.0, 1.0, 1.0, 0.13))?);
        self.badge_dim_brush =
            Some(self.create_brush(&color(200.0 / 255.0, 208.0 / 255.0, 219.0 / 255.0, 0.28))?);
        self.capture_brush = Some(self.create_brush(&color_of(CAPTURE_RGB, 1.0))?);
        // Mutable: `paint_chain_rings` sets each ring's own opacity.
        self.chain_ring_brush = Some(self.create_brush(&chain_ring_color(1.0))?);
        self.chain_shadow_brush =
            Some(self.create_brush(&color(0.0, 0.0, 0.0, CHAIN_RING_SHADOW))?);
        self.mask_tint_brush = Some(self.create_brush(&color(
            accent.r * 1.0,
            accent.g * 1.0,
            accent.b * 1.0,
            MASK_TINT_ALPHA,
        ))?);
        self.handle_brush = Some(self.create_brush(&white)?);
        self.label_background_brush = Some(self.create_brush(&panel)?);
        self.label_text_brush = Some(self.create_brush(&white)?);
        self.crosshair_brush = Some(self.create_brush(&crosshair)?);
        self.magnifier_band_brush = Some(self.create_brush(&color(0.30, 0.55, 1.0, 0.45))?);
        self.magnifier_grid_brush = Some(self.create_brush(&grid)?);
        self.magnifier_info_brush = Some(self.create_brush(&info)?);
        // ── Info strip palette (docs/design/overlay-ui-demo.html) ──
        let soft = |alpha| color(232.0 / 255.0, 236.0 / 255.0, 242.0 / 255.0, alpha);
        self.info_hex_brush = Some(self.create_brush(&white)?);
        self.info_coord_value_brush = Some(self.create_brush(&soft(0.72))?);
        self.info_hint_brush = Some(self.create_brush(&soft(0.62))?);
        self.info_kbd_fill_brush = Some(self.create_brush(&color(1.0, 1.0, 1.0, 0.1))?);
        self.info_swatch_border_brush = Some(self.create_brush(&color(1.0, 1.0, 1.0, 0.55))?);
        self.info_swatch_fill_brush = Some(self.create_brush(&color(0.0, 0.0, 0.0, 1.0))?);
        // ── The backdrop blur ──
        // windows-rs binds ID2D1Properties::SetValue as (u32, D2D1_PROPERTY_TYPE, &[u8]), so the
        // values go in as raw little-endian bytes with D2D1_PROPERTY_TYPE_UNKNOWN.
        let blur: ID2D1Effect = unsafe { self.d2d.CreateEffect(&CLSID_D2D1GaussianBlur) }
            .map_err(|error| super::hresult("ID2D1DeviceContext::CreateEffect", &error))?;
        unsafe {
            blur.SetValue(
                D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION.0 as u32,
                D2D1_PROPERTY_TYPE_UNKNOWN,
                &(D2D1_GAUSSIANBLUR_OPTIMIZATION_BALANCED.0 as u32).to_ne_bytes(),
            )
            .map_err(|error| super::hresult("ID2D1Effect::SetValue(OPTIMIZATION_MODE)", &error))?;
            // HARD border mode: the kernel would otherwise pull in transparent pixels past the
            // frame's edge and darken a band along every screen border.
            blur.SetValue(
                D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
                D2D1_PROPERTY_TYPE_UNKNOWN,
                &(D2D1_BORDER_MODE_HARD.0 as u32).to_ne_bytes(),
            )
            .map_err(|error| super::hresult("ID2D1Effect::SetValue(BORDER_MODE)", &error))?;
        }
        self.blur_effect = Some(blur);
        self.info_tint_brush = Some(self.create_brush(&rgba(18, 18, 20, 0.78))?);
        // ── Shortcut guide ──
        self.guide_tint_brush = Some(self.create_brush(&rgba(14, 16, 20, 0.66))?);
        self.guide_cap_brush = Some(self.create_brush(&rgba(236, 239, 244, 0.94))?);
        self.guide_cap_text_brush = Some(self.create_brush(&rgba(0x15, 0x18, 0x1d, 1.0))?);
        self.guide_label_brush = Some(self.create_brush(&color(1.0, 1.0, 1.0, 0.92))?);
        self.guide_head_brush = Some(self.create_brush(&color(1.0, 1.0, 1.0, 0.42))?);
        self.guide_sep_brush = Some(self.create_brush(&color(1.0, 1.0, 1.0, 0.5))?);
        self.info_badge_bg_brush =
            Some(self.create_brush(&color(60.0 / 255.0, 60.0 / 255.0, 60.0 / 255.0, 0.82))?);
        // Annotation brushes: initialised to accent blue; colour set per-item at draw time.
        self.ann_stroke_brush = Some(self.create_brush(&color(0.0, 0.47, 0.83, 1.0))?);
        self.ann_fill_brush = Some(self.create_brush(&color(0.0, 0.0, 0.0, 0.0))?);
        // ── Scroll panel (docs/33, the demo's `drawScrolling`) ─────────────────────────
        // Nearly opaque: the panel sits over a live page, so legibility wins over letting it through.
        self.scroll_panel_brush = Some(self.create_brush(&SCROLL_PANEL)?);
        self.scroll_box_brush = Some(self.create_brush(&accent)?);
        self.scroll_shadow_brush = Some(self.create_brush(&color(0.0, 0.0, 0.0, 0.1))?);
        self.scroll_title_brush = Some(self.create_brush(&color(1.0, 1.0, 1.0, 1.0))?);
        self.scroll_muted_brush = Some(self.create_brush(&rgba(232, 236, 242, 0.62))?);
        self.scroll_button_brush = Some(self.create_brush(&color(1.0, 1.0, 1.0, 0.08))?);
        let stops = [
            D2D1_GRADIENT_STOP {
                position: 0.0,
                color: D2D1_COLOR_F {
                    a: 0.95,
                    ..SCROLL_PANEL
                },
            },
            D2D1_GRADIENT_STOP {
                position: 1.0,
                color: D2D1_COLOR_F {
                    a: 0.0,
                    ..SCROLL_PANEL
                },
            },
        ];
        // The render target's overload: the device context's own takes straight-alpha stops in a
        // different signature.
        let target: &ID2D1RenderTarget = &self.d2d;
        let fade = unsafe {
            let collection = target
                .CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
                .map_err(|e| super::hresult("CreateGradientStopCollection", &e))?;
            target
                .CreateLinearGradientBrush(
                    &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                        startPoint: vector2(0.0, 0.0),
                        endPoint: vector2(0.0, 1.0),
                    },
                    None,
                    &collection,
                )
                .map_err(|e| super::hresult("CreateLinearGradientBrush", &e))?
        };
        self.scroll_fade_brush = Some(fade);
        Ok(())
    }

    fn create_brush(&self, color: &D2D1_COLOR_F) -> Result<ID2D1SolidColorBrush, String> {
        unsafe { self.d2d.CreateSolidColorBrush(color, None) }
            .map_err(|error| super::hresult("CreateSolidColorBrush", &error))
    }

    /// Drop the swap chain, its render target and the brushes bound to it.
    ///
    /// The L0 frame bitmap is dropped with the renderer at session end. It is only
    /// retained while a capture session is active.
    fn release_target(&mut self) {
        unsafe { self.d2d.SetTarget(None) };
        self.target = None;
        self.swap_chain = None;
        self.size = (0, 0);
        self.border_brush = None;
        self.mask_brush = None;
        self.preview_fill_brush = None;
        self.preview_stroke_brush = None;
        self.badge_line_brush = None;
        self.badge_dim_brush = None;
        self.hover_fill_brush = None;
        self.capture_brush = None;
        self.chain_ring_brush = None;
        self.chain_shadow_brush = None;
        self.mask_tint_brush = None;
        self.handle_brush = None;
        self.label_background_brush = None;
        self.label_text_brush = None;
        self.crosshair_brush = None;
        self.magnifier_band_brush = None;
        self.magnifier_grid_brush = None;
        self.magnifier_info_brush = None;
        self.info_hex_brush = None;
        self.info_coord_value_brush = None;
        self.info_hint_brush = None;
        self.info_kbd_fill_brush = None;
        self.info_swatch_border_brush = None;
        self.info_swatch_fill_brush = None;
        self.blur_effect = None;
        self.blurred_frame = None;
        self.info_tint_brush = None;
        self.guide_tint_brush = None;
        self.guide_cap_brush = None;
        self.guide_cap_text_brush = None;
        self.guide_label_brush = None;
        self.guide_head_brush = None;
        self.guide_sep_brush = None;
        self.info_badge_bg_brush = None;
        self.ann_stroke_brush = None;
        self.ann_fill_brush = None;
        self.scroll_panel_brush = None;
        self.scroll_box_brush = None;
        self.scroll_shadow_brush = None;
        self.scroll_title_brush = None;
        self.scroll_muted_brush = None;
        self.scroll_button_brush = None;
        self.scroll_fade_brush = None;
        self.scroll_preview_bitmap = None;
    }
}

/// Whether a draw of `view` crops from the blurred frame: the blurred mask, the guide, or the
/// magnifier's info strip.
fn needs_backdrop(view: &RenderView) -> bool {
    view.scroll.is_none()
        && (view.mask_blur || view.guide.is_some() || (view.show_chrome && view.cursor_visible))
}

use helpers::*;

mod guide_pass;
mod helpers;
mod magnifier_pass;

#[cfg(test)]
mod tests;
