//! Direct2D / DirectWrite resources and the L0/L1/L2 draw calls.
//!
//! Layer order, back to front:
//! 1. **L0** the frozen capture frame, drawn 1:1 so the selection stays pixel exact;
//! 2. **L1** a translucent dark mask over everything *except* the selection, drawn
//!    as four rectangles so the selected pixels keep their original brightness;
//! 3. **L2** rounded border, eight grips, the `width × height` label and the
//!    magnifier.
//!
//! Everything here is preview-only: the exported artifact is produced by
//! [`crate::application::capture_service`] from the captured frame, so no mask,
//! label or magnifier can leak into the result.

use windows_numerics::Vector2;
use std::sync::Arc;
use ::windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BORDER_MODE_HARD,
    D2D1_COLOR_F, D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use ::windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1GaussianBlur,
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_OPTIONS_NONE,
    D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_CAP_STYLE_FLAT, D2D1_DASH_STYLE_DASH,
    D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_GAUSSIANBLUR_OPTIMIZATION_BALANCED,
    D2D1_GAUSSIANBLUR_PROP_BORDER_MODE,
    D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION,
    D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
    D2D1_LAYER_PARAMETERS1, D2D1_LINE_JOIN_MITER, D2D1_PROPERTY_TYPE_UNKNOWN, D2D1_ROUNDED_RECT,
    D2D1_STROKE_STYLE_PROPERTIES,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1_ELLIPSE,
    ID2D1Bitmap1, ID2D1DeviceContext, ID2D1Effect, ID2D1Factory, ID2D1Geometry, ID2D1Image,
    ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use ::windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use ::windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_ISOLATED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_METRICS,
    DWRITE_WORD_WRAPPING_NO_WRAP,
    DWriteCreateFactory, IDWriteFactory,
    IDWriteTextFormat,
};
use ::windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use ::windows::Win32::Graphics::Gdi::AddFontMemResourceEx;
use ::windows::Win32::Graphics::Dxgi::IDXGISwapChain1;
use ::windows::core::{Interface, PCWSTR};

use super::d3d11::GraphicsDevice;
use crate::annotation::{AnnotationGeometry, AnnotationItem, AnnotationKind};
// The palette and the light-math live in one place: the paint layer and the A4 ring decision read
// the same constants, so a re-measured mask or wash cannot leave the table describing a world that
// is no longer painted (docs/21 §5.24.11).
use crate::ring_contrast::{
    ACCENT_RGB, CAPTURE_RGB, CHAIN_RING_RGB, CHAIN_RING_SHADOW, HOVER_WASH_ALPHA, MASK_ALPHA,
    MASK_RGB, MASK_TINT_ALPHA, PREVIEW_WASH_ALPHA, PREVIEW_WASH_WALK_SCALE, Rgb, mix,
};
use crate::geometry::{
    Handle, LevelReach, MagnifierConfig, Point, Rect, SizeLabelPlacement,
};
use crate::scroll::panel::{
    CANCEL_TEXT, PANEL_LINE_HEIGHT_DIP, PANEL_MARGIN_DIP, PANEL_RADIUS_DIP, PanelLayout,
    RETURN_TEXT, ScrollPanel, STOP_TEXT, UNADOPTED_RGB, UNDO_TEXT,
};

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

/// The info panel's primary typeface: a subsetted HarmonyOS Sans SC carrying only
/// the Latin/digit/punctuation glyphs and the handful of CJK characters the panel
/// draws (`格式复制坐标`). Embedded verbatim and registered privately for this
/// process at renderer start, so no system font install or admin rights are needed.
const INFO_EMBEDDED_FONT: &[u8] =
    include_bytes!("../../../assets/harmonyos-sans-sc-subset.ttf");
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
/// Row 1: the S-cycled colour value (`#RRGGBB` / `rgb(...)` / `hsl(...)`),
/// left-aligned on the same line as the right-aligned coordinates.
const INFO_PRIMARY_COLOR_FONT_DIP: f32 = 13.0;
/// Row 1: the position string `(x, y)`, right-aligned on the same line as the
/// colour value. The colour value shares this 13 px size.
const INFO_COORD_VALUE_FONT_DIP: f32 = 13.0;
/// Horizontal gap between the colour value and the right-aligned position text.
const INFO_COORD_BLOCK_GAP_DIP: f32 = 14.0;
/// Rows 2–3: four `<kbd>` hint items in a 2×2 grid (key text inside the box +
/// CJK description beside it). One point smaller than the colour/coordinate line.
const INFO_HINT_FONT_DIP: f32 = 12.0;
/// The four `<kbd>` rows, as (key, description).
///
/// A `const` rather than a local array because the font-coverage gate
/// (`the_embedded_subset_covers_the_strings_the_overlay_draws`) has to see exactly what is drawn:
/// the embedded subset is a hard allowlist, and a description that drifts away from it renders in
/// a fallback font with nothing failing anywhere visible.
const INFO_HINTS: [(&str, &str); 4] = [
    ("S", "色值格式"),
    ("C", "复制色值"),
    ("P", "坐标模式"),
    ("Z", "滚轮缩放"),
];
const INFO_KBD_PADDING_DIP: f32 = 3.0;
const INFO_KBD_RADIUS_DIP: f32 = 4.0;
const INFO_KBD_TEXT_GAP_DIP: f32 = 6.0;
/// Vertical extent of one kbd row (box height; the text is vertically centred
/// inside). The four hints form a 2×2 grid.
const INFO_KBD_LINE_HEIGHT_DIP: f32 = 22.0;
/// Horizontal gap between the two kbd columns.
const INFO_HINT_ITEM_GAP_DIP: f32 = 16.0;
/// Vertical gap between the two kbd grid rows.
const INFO_KBD_ROW_GAP_DIP: f32 = 8.0;
/// Gap between the colour/coordinate line and the kbd grid below it.
const INFO_KBD_BLOCK_GAP_DIP: f32 = 12.0;

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
    // ── Scroll capture (docs/30 §19.7) ────────────────────────────────────────
    /// The scroll session's panel, when one is running (docs/30 §19.1/§19.4/§19.7).
    ///
    /// The model owns the eight answers and the box geometry; this layer paints what it says and
    /// decides nothing. `None` for every other capture mode — and, importantly, on the export path:
    /// the artifact is cut from the captured frame, so a panel drawn into it would be a panel burned
    /// into the user's image.
    ///
    /// `pub(crate)` rather than `pub` on purpose: the type is crate-internal (`scroll::panel` is not
    /// a boundary module), and widening it to satisfy a field of a `pub` struct would make it public
    /// API for no reason.
    pub(crate) scroll_panel: Option<crate::scroll::panel::ScrollPanel>,
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
            scroll_panel: None,
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
    border: ID2D1SolidColorBrush,
    handle: ID2D1SolidColorBrush,
    label_background: ID2D1SolidColorBrush,
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
    /// `rgba(20,20,20,0.4)` — the translucent background of the whole strip.
    info_bg_brush: Option<ID2D1SolidColorBrush>,
    /// Pure white — the primary S-cycled colour value (`#RRGGBB` /
    /// `rgb(...)` / `hsl(...)`).
    info_hex_brush: Option<ID2D1SolidColorBrush>,
    /// `#f5f5f5` — coordinate numeric values.
    info_coord_value_brush: Option<ID2D1SolidColorBrush>,
    /// `#f5f5f5` — shortcut-hint description text ("色值格式" / "复制色值" …).
    info_hint_brush: Option<ID2D1SolidColorBrush>,
    /// `#f5f5f5` — the key text drawn inside a `<kbd>` box.
    info_kbd_text_brush: Option<ID2D1SolidColorBrush>,
    /// `rgba(255,255,255,0.1)` — the `<kbd>` box fill.
    info_kbd_fill_brush: Option<ID2D1SolidColorBrush>,
    /// A slightly darker translucent white used as the `<kbd>` box's bottom
    /// edge, faking the box-shadow's 3D lip without a real shadow effect.
    info_kbd_bottom_brush: Option<ID2D1SolidColorBrush>,
    /// Semi-transparent grey-white 1px border around the colour swatch.
    info_swatch_border_brush: Option<ID2D1SolidColorBrush>,
    /// Translucent black inner stroke, faking the swatch's inset shadow.
    info_swatch_inset_brush: Option<ID2D1SolidColorBrush>,
    /// Reusable mutable brush whose `SetColor` is updated to the sampled pixel
    /// every frame, avoiding a fresh brush per repaint.
    info_swatch_fill_brush: Option<ID2D1SolidColorBrush>,
    /// Cached `CLSID_D2D1GaussianBlur` effect that samples the frozen frame into
    /// a real backdrop for the info strip. Created once in `recreate_resources`;
    /// per-frame work is just `SetInput` + `SetValue(std_dev)` + `DrawImage`
    /// restricted to the info rectangle (see docs/15).
    blur_effect: Option<ID2D1Effect>,
    /// `#121214 @ 0.48` — the tint drawn over the blurred slice so text keeps
    /// contrast without hiding the blur texture underneath.
    info_tint_brush: Option<ID2D1SolidColorBrush>,
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
    // ── Scroll panel (docs/30 §19.7) ──────────────────────────────────────────
    /// The panel's backdrop: near-black at high alpha, so the frozen frame behind it cannot make
    /// the status text unreadable.
    scroll_panel_brush: Option<ID2D1SolidColorBrush>,
    /// The thumbnail strip's well — darker than the panel so the strip reads as a window.
    scroll_strip_brush: Option<ID2D1SolidColorBrush>,
    /// The viewport box when the last step was adopted (docs/30 §19.4's `Confirmed` outline) and
    /// the outline of the two buttons.
    scroll_box_brush: Option<ID2D1SolidColorBrush>,
    /// The viewport box when the last step was **not** adopted: §19.4 draws that state dashed, and
    /// the colour has to match the action the user has to take — which is none, so it is a neutral
    /// grey rather than the red PixPin uses for the same reading.
    scroll_unadopted_brush: Option<ID2D1SolidColorBrush>,
    /// Dash pattern for the unadopted viewport box. Built once; rebuilt only with the other
    /// resources, because `CreateStrokeStyle` needs the factory and the factory cannot move between
    /// threads (docs/30 §21.5).
    scroll_dash_style: Option<ID2D1StrokeStyle>,
    metrics: RenderMetrics,
    size: (u32, u32),
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
        let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_ISOLATED) }
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
            info_bg_brush: None,
            info_hex_brush: None,
            info_coord_value_brush: None,
            info_hint_brush: None,
            info_kbd_text_brush: None,
            info_kbd_fill_brush: None,
            info_kbd_bottom_brush: None,
            info_swatch_border_brush: None,
            info_swatch_inset_brush: None,
            info_swatch_fill_brush: None,
            blur_effect: None,
            info_tint_brush: None,
            info_badge_bg_brush: None,
            info_formats: Vec::new(),
            ann_stroke_brush: None,
            ann_fill_brush: None,
            scroll_panel_brush: None,
            scroll_strip_brush: None,
            scroll_box_brush: None,
            scroll_unadopted_brush: None,
            scroll_dash_style: None,
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
        let swap_chain = self
            .device
            .create_composition_swap_chain(width, height)?;
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
            scroll_panel: None,
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
            let clip_rect = if view.selection.is_empty() { view.frame } else { view.selection.intersect(view.frame) };
            unsafe {
                self.d2d.PushAxisAlignedClip(&to_d2d(clip_rect), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
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
                border: self.require_brush(&self.border_brush, "border brush")?,
                handle: self.require_brush(&self.handle_brush, "handle brush")?,
                label_background: self
                    .require_brush(&self.label_background_brush, "label background brush")?,
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

        // L4: the scroll panel. Above the selection chrome because during a scroll capture the
        // panel is the only thing the user is interacting with (docs/30 §19.7), and every mode
        // that is not a scroll capture leaves `scroll_panel` at `None`.
        self.draw_scroll_panel(view, self.metrics)?;
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
                    self.d2d
                        .DrawRectangle(&to_d2d(rect), &border, width, None);
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
                    let mix = self.require_brush(&self.preview_stroke_brush, "preview stroke brush")?;
                    let fill = self.require_brush(&self.preview_fill_brush, "preview fill brush")?;
                    unsafe {
                        let _ = mix.SetColor(&capture_mix_color(walking, appear));
                        let _ = fill.SetColor(&capture_wash_color(walking, appear));
                    }
                    (
                        fill,
                        mix,
                        width * 2.0,
                    )
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
                self.d2d
                    .DrawRectangle(
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

    fn draw_chrome(&mut self, view: &RenderView, resources: &ChromeResources) -> Result<(), String> {
        let selection = view.selection.intersect(view.frame);
        let metrics = self.metrics;

        unsafe {
            self.d2d
                .DrawRectangle(&to_d2d(selection), &resources.border, metrics.border_width, None);

            let radius = metrics.handle_size / 2.0;
            for handle in Handle::ALL {
                let anchor = handle.anchor(selection);
                let grip = D2D1_ELLIPSE {
                    point: vector2(anchor.x as f32, anchor.y as f32),
                    radiusX: radius,
                    radiusY: radius,
                };
                self.d2d.FillEllipse(&grip, &resources.handle);
                // Ring each white grip with the selection border colour, stroked
                // at the same width as the selection rectangle outline.
                self.d2d
                    .DrawEllipse(&grip, &resources.border, metrics.border_width, None);
            }
        }

        self.draw_size_label(selection, view, resources, metrics)?;
        if view.cursor_visible {
            self.draw_magnifier(view, resources, metrics)?;
        }
        Ok(())
    }

    fn draw_size_label(
        &mut self,
        selection: Rect,
        view: &RenderView,
        resources: &ChromeResources,
        metrics: RenderMetrics,
    ) -> Result<(), String> {
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
            self.d2d
                .FillRoundedRectangle(&panel, &resources.label_background);
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
                &resources.label_text,
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

    // ── Scroll capture panel (docs/30 §19.1/§19.4/§19.7) ────────────────────────────────────────

    /// L4: the scroll session's panel.
    ///
    /// The model (`crate::scroll::panel`) owns every number and every word here; this draws what it
    /// says and decides nothing. Three consequences worth naming:
    ///
    /// * the panel is anchored to the **work area**, not the frame, so it stays on screen whatever
    ///   the capture geometry is;
    /// * the pieces come from [`PanelLayout`] in DIP and are scaled once, here — the same layout is
    ///   correct at 100% and at 150%, and nothing in the model has to know the DPI;
    /// * the viewport box is placed by [`ScrollPanel::viewport_box`], which is proportional, so the
    ///   strip's size in pixels is all it needs.
    fn draw_scroll_panel(&mut self, view: &RenderView, metrics: RenderMetrics) -> Result<(), String> {
        let Some(panel) = view.scroll_panel.clone() else {
            return Ok(());
        };
        let scale = (metrics.dpi as f32 / 96.0).max(1.0);
        let dip = |value: i32| (value as f32 * scale).round() as i32;
        let layout = PanelLayout::new();
        let work = if view.work_area.is_empty() {
            view.frame
        } else {
            view.work_area
        };
        let width = dip(layout.panel.width());
        let height = dip(layout.panel.height());
        let margin = dip(PANEL_MARGIN_DIP);
        let left = (work.right - margin - width).max(work.left);
        let top = (work.top + (work.height() - height) / 2).max(work.top);
        let place = |rect: Rect| {
            Rect::new(
                left + dip(rect.left),
                top + dip(rect.top),
                left + dip(rect.right),
                top + dip(rect.bottom),
            )
        };

        let answers = panel.answers();
        let appearance = panel.appearance();
        let backdrop = self.require_brush(&self.scroll_panel_brush, "scroll panel brush")?;
        let well = self.require_brush(&self.scroll_strip_brush, "scroll strip brush")?;
        let text = self.require_brush(&self.label_text_brush, "label text brush")?;

        unsafe {
            let radius = dip(PANEL_RADIUS_DIP) as f32;
            let frame = D2D1_ROUNDED_RECT {
                rect: to_d2d(place(layout.panel)),
                radiusX: radius,
                radiusY: radius,
            };
            self.d2d.FillRoundedRectangle(&frame, &backdrop);
            self.d2d.FillRectangle(&to_d2d(place(layout.strip)), &well);
        }

        // Question 1 and question 3. Question 5 is read off the same status line: it says what the
        // session is doing, and the buttons below say whether it can still be stopped.
        self.draw_panel_text(place(layout.status), &answers.doing, false, &text)?;
        self.draw_panel_text(place(layout.amount), &answers.amount, false, &text)?;

        // Questions 2 and 4 share one mark: where the box is, and what its state is. §19.4's three
        // states differ in dash and colour, never in position — a box that moved when a step was not
        // adopted would say the reading changed when only the confidence did.
        let strip = place(layout.strip);
        let placed = panel.viewport_box(strip);
        let outline = if appearance.dashed {
            self.require_brush(&self.scroll_unadopted_brush, "scroll unadopted brush")?
        } else {
            self.require_brush(&self.scroll_box_brush, "scroll box brush")?
        };
        let dashes = if appearance.dashed {
            self.scroll_dash_style.clone()
        } else {
            None
        };
        unsafe {
            self.d2d.DrawRectangle(
                &to_d2d(placed),
                &outline,
                metrics.border_width,
                dashes.as_ref(),
            );
        }
        if let Some(discarded) = appearance.badge {
            // A dashed box says "this step was not adopted"; the number says how many, which is the
            // part a user cannot reconstruct from the box (docs/30 §19.1, question 4 + G12).
            let badge = ScrollPanel::badge_text(discarded);
            let badge_top = placed.bottom + dip(2);
            let anchor = Rect::new(
                placed.left,
                badge_top,
                strip.right,
                (badge_top + dip(PANEL_LINE_HEIGHT_DIP)).min(strip.bottom),
            );
            if !anchor.is_empty() {
                self.draw_panel_text(anchor, &badge, true, &outline)?;
            }
        }

        // The four buttons, in reading order: the view's two actions and the session's two promises
        // (§19.5, §20.5). They are drawn only while the session is running — an "ended" panel offering
        // a stop button, or a "back to the newest" that nothing is following, would be answering a
        // question nobody can ask any more.
        //
        // "回到最新" is dimmed while it has nothing to do. §19.5's mode is the session's and this is the
        // state in which a press would be a no-op, so it is drawn in the grey §19.4 already uses for
        // "there is nothing here for you" rather than hidden: a control that disappears makes the user
        // wonder whether they are in manual mode, which is the one thing this button exists to answer.
        //
        // "撤销" has no such state, and that is deliberate: the panel cannot tell "the stack is empty"
        // from "the last press was spent" (`steps` counts steps, not stack entries), and a press with
        // nothing behind it is a defined no-op (§19.6 constraint 4, pinned by the loop's test) rather
        // than a mistake worth warning about.
        if answers.continuing {
            let spent = self.require_brush(&self.scroll_unadopted_brush, "scroll unadopted brush")?;
            let following = panel.follow();
            for (index, (rect, label)) in layout
                .buttons
                .iter()
                .zip([RETURN_TEXT, UNDO_TEXT, STOP_TEXT, CANCEL_TEXT])
                .enumerate()
            {
                let dimmed = index == 0 && following;
                let (brush, color) = if dimmed {
                    (&spent, &spent)
                } else {
                    (&outline, &text)
                };
                let rect = place(*rect);
                unsafe {
                    self.d2d
                        .DrawRectangle(&to_d2d(rect), brush, metrics.border_width, None);
                }
                self.draw_panel_text(rect, label, true, color)?;
            }
        }

        // Question 8. The line is reserved whether or not there is anything in it (see `PanelLayout`).
        if let Some(trouble) = answers.trouble.as_deref() {
            self.draw_panel_text(place(layout.trouble), trouble, false, &text)?;
        }
        Ok(())
    }

    /// One line of panel text, vertically centred in `rect`.
    ///
    /// The format comes from the same cache as the size label, whose first family is the embedded
    /// subset — which is what makes the panel's Chinese characters resolve, and what the subset gate
    /// in `tests.rs` is about.
    fn draw_panel_text(
        &mut self,
        rect: Rect,
        text: &str,
        centered: bool,
        brush: &ID2D1SolidColorBrush,
    ) -> Result<(), String> {
        if text.is_empty() || rect.is_empty() {
            return Ok(());
        }
        let (measured, format) = self.measure_label(text)?;
        let box_rect = if centered {
            // `measure_label` already returns the text plus symmetric padding, so it is the width to
            // centre on — no second measurement, and the two cannot disagree.
            let left = rect.left + ((rect.width() - measured.0) / 2).max(0);
            Rect::new(left, rect.top, left + measured.0, rect.bottom)
        } else {
            rect
        };
        let wide = text.encode_utf16().collect::<Vec<u16>>();
        let target = D2D_RECT_F {
            left: box_rect.left as f32,
            top: box_rect.top as f32,
            right: box_rect.right as f32,
            bottom: box_rect.bottom as f32,
        };
        unsafe {
            self.d2d.DrawText(
                &wide,
                &format,
                &target,
                brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
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
        let fill   = self.require_brush(&self.ann_fill_brush, "ann fill brush")?;

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
                    if has_fill { self.d2d.FillRectangle(&r, fill); }
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
                    if has_fill { self.d2d.FillEllipse(&ell, fill); }
                    self.d2d.DrawEllipse(&ell, stroke, sw, None);
                }
                AnnotationGeometry::Line { start, end } => {
                    self.d2d.DrawLine(
                        vector2(start.x as f32, start.y as f32),
                        vector2(end.x as f32, end.y as f32),
                        stroke, sw, None,
                    );
                }
                AnnotationGeometry::Arrow { start, end } => {
                    let sx = start.x as f32; let sy = start.y as f32;
                    let ex = end.x as f32;   let ey = end.y as f32;
                    // Shaft
                    self.d2d.DrawLine(vector2(sx, sy), vector2(ex, ey), stroke, sw, None);
                    // Two-line arrowhead
                    let head = item.style.arrow_head_size;
                    let angle = (ey - sy).atan2(ex - sx);
                    let (cos_a, sin_a) = (angle.cos(), angle.sin());
                    // Left barb
                    let lx = ex - head * (cos_a * (30f32).to_radians().cos() + sin_a * (30f32).to_radians().sin());
                    let ly = ey - head * (sin_a * (30f32).to_radians().cos() - cos_a * (30f32).to_radians().sin());
                    // Right barb
                    let rx = ex - head * (cos_a * (-30f32).to_radians().cos() + sin_a * (-30f32).to_radians().sin());
                    let ry = ey - head * (sin_a * (-30f32).to_radians().cos() - cos_a * (-30f32).to_radians().sin());
                    self.d2d.DrawLine(vector2(ex, ey), vector2(lx, ly), stroke, sw, None);
                    self.d2d.DrawLine(vector2(ex, ey), vector2(rx, ry), stroke, sw, None);
                }
                AnnotationGeometry::Freehand { points }
                | AnnotationGeometry::Highlight { points } => {
                    if points.len() < 2 { return Ok(()); }
                    let alpha_mult = if matches!(item.kind, AnnotationKind::Highlight) { 0.35f32 } else { 1.0 };
                    let hl_width = if matches!(item.kind, AnnotationKind::Highlight) { sw * 3.0 } else { sw };
                    let base = item.style.stroke_color;
                    if alpha_mult < 1.0 {
                        let hl_color = color(base[0], base[1], base[2], base[3] * alpha_mult * opacity);
                        let _ = stroke.SetColor(&hl_color);
                    }
                    for w in points.windows(2) {
                        self.d2d.DrawLine(
                            vector2(w[0].x as f32, w[0].y as f32),
                            vector2(w[1].x as f32, w[1].y as f32),
                            stroke, hl_width, None,
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
        if content.is_empty() { return Ok(()); }
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
                (b.left, b.top), (b.right, b.top),
                (b.left, b.bottom), (b.right, b.bottom),
            ] {
                let grip = D2D1_ELLIPSE { point: vector2(cx, cy), radiusX: handle_r, radiusY: handle_r };
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
        let format =
            format.ok_or_else(|| format!("no usable label font family ({last_error})"))?;
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
        let preview_fill = color_of(
            CAPTURE_RGB,
            PREVIEW_WASH_ALPHA * PREVIEW_WASH_WALK_SCALE,
        );
        self.preview_fill_brush = Some(self.create_brush(&preview_fill)?);
        self.preview_stroke_brush = Some(self.create_brush(&capture_mix_color(1.0, 1.0))?);
        // The badge's hairline and its dimmed half (v3: rgba(255,255,255,.1–.14) and
        // rgba(200,208,219,.28); folded into one line brush, since the two hairlines differ by four
        // percent of alpha and no one can see it).
        self.badge_line_brush = Some(self.create_brush(&color(1.0, 1.0, 1.0, 0.13))?);
        self.badge_dim_brush = Some(self.create_brush(&color(
            200.0 / 255.0,
            208.0 / 255.0,
            219.0 / 255.0,
            0.28,
        ))?);
        self.capture_brush = Some(self.create_brush(&color_of(CAPTURE_RGB, 1.0))?);
        // Mutable: `paint_chain_rings` sets each ring's own opacity.
        self.chain_ring_brush = Some(self.create_brush(&chain_ring_color(1.0))?);
        self.chain_shadow_brush = Some(self.create_brush(&color(0.0, 0.0, 0.0, CHAIN_RING_SHADOW))?);
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
        self.magnifier_band_brush =
            Some(self.create_brush(&color(0.30, 0.55, 1.0, 0.45))?);
        self.magnifier_grid_brush = Some(self.create_brush(&grid)?);
        self.magnifier_info_brush = Some(self.create_brush(&info)?);
        // ── Glassmorphism info-panel palette ──
        // The primary path uses a real Gaussian-blur backdrop + `info_tint_brush`.
        // This solid `info_bg_brush` remains the fallback when the effect cannot
        // be constructed; 0.68 keeps the strip legible without going fully opaque.
        self.info_bg_brush = Some(self.create_brush(
            &color(20.0 / 255.0, 20.0 / 255.0, 20.0 / 255.0, 0.68),
        )?);
        self.info_hex_brush = Some(self.create_brush(&color(1.0, 1.0, 1.0, 1.0))?);
        self.info_coord_value_brush = Some(self.create_brush(
            &color(245.0 / 255.0, 245.0 / 255.0, 245.0 / 255.0, 1.0),
        )?);
        self.info_hint_brush = Some(self.create_brush(
            &color(245.0 / 255.0, 245.0 / 255.0, 245.0 / 255.0, 1.0),
        )?);
        self.info_kbd_text_brush = Some(self.create_brush(
            &color(245.0 / 255.0, 245.0 / 255.0, 245.0 / 255.0, 1.0),
        )?);
        self.info_kbd_fill_brush = Some(self.create_brush(
            &color(1.0, 1.0, 1.0, 0.1),
        )?);
        self.info_kbd_bottom_brush = Some(self.create_brush(
            &color(1.0, 1.0, 1.0, 0.06),
        )?);
        self.info_swatch_border_brush = Some(self.create_brush(
            &color(1.0, 1.0, 1.0, 0.35),
        )?);
        self.info_swatch_inset_brush = Some(self.create_brush(
            &color(0.0, 0.0, 0.0, 0.25),
        )?);
        self.info_swatch_fill_brush = Some(self.create_brush(&color(0.0, 0.0, 0.0, 1.0))?);
        // ── Real Gaussian-blur backdrop for the info strip (docs/15) ──
        // windows-rs 0.61 binds ID2D1Properties::SetValue as
        //   (u32, D2D1_PROPERTY_TYPE, &[u8]) -> Result<()>
        // rather than accepting a &PROPVARIANT, so raw little-endian bytes with
        // D2D1_PROPERTY_TYPE_UNKNOWN is the correct call shape. SetInput returns
        // () (no Result), and DrawImage's targetoffset / imagerectangle are
        // Option<*const _> — see the call site in draw_magnifier for how those
        // two differ from the intuitive C++ projection.
        let blur: ID2D1Effect = unsafe { self.d2d.CreateEffect(&CLSID_D2D1GaussianBlur) }
            .map_err(|error| super::hresult("ID2D1DeviceContext::CreateEffect", &error))?;
        unsafe {
            blur.SetValue(
                D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION.0 as u32,
                D2D1_PROPERTY_TYPE_UNKNOWN,
                &(D2D1_GAUSSIANBLUR_OPTIMIZATION_BALANCED.0 as u32).to_ne_bytes(),
            )
            .map_err(|error| {
                super::hresult("ID2D1Effect::SetValue(OPTIMIZATION_MODE)", &error)
            })?;
            // HARD border mode: without it, the kernel would sample outside the
            // info rect into transparent pixels near the frame edge and darken
            // the strip's own edges (a visible halo at screen borders).
            blur.SetValue(
                D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
                D2D1_PROPERTY_TYPE_UNKNOWN,
                &(D2D1_BORDER_MODE_HARD.0 as u32).to_ne_bytes(),
            )
            .map_err(|error| super::hresult("ID2D1Effect::SetValue(BORDER_MODE)", &error))?;
        }
        self.blur_effect = Some(blur);
        self.info_tint_brush = Some(self.create_brush(&color(0.07, 0.07, 0.08, 0.78))?);
        self.info_badge_bg_brush = Some(self.create_brush(&color(
            60.0 / 255.0, 60.0 / 255.0, 60.0 / 255.0, 0.82,
        ))?);
        // Annotation brushes: initialised to accent blue; colour set per-item at draw time.
        self.ann_stroke_brush = Some(self.create_brush(&color(0.0, 0.47, 0.83, 1.0))?);
        self.ann_fill_brush = Some(self.create_brush(&color(0.0, 0.0, 0.0, 0.0))?);
        // ── Scroll panel (docs/30 §19.7) ────────────────────────────────────────────────
        // The backdrop is darker and more opaque than the label panel: the panel sits over a live
        // capture and carries whole sentences, so legibility wins over letting the frame through.
        //
        // The two box colours are the model's (§19.4): the adopted box is the interface accent, and
        // the unadopted box is a neutral grey. The grey is the point — PixPin paints this state red,
        // which reads as "something is wrong, act"; §16.10 continues past the step and the user has
        // nothing to do, so the colour must not ask for an action nobody needs to take.
        self.scroll_panel_brush = Some(self.create_brush(&color(0.04, 0.04, 0.05, 0.96))?);
        self.scroll_strip_brush = Some(self.create_brush(&color(0.0, 0.0, 0.0, 0.55))?);
        self.scroll_box_brush = Some(self.create_brush(&accent)?);
        self.scroll_unadopted_brush = Some(self.create_brush(&color(
            f32::from(UNADOPTED_RGB.r) / 255.0,
            f32::from(UNADOPTED_RGB.g) / 255.0,
            f32::from(UNADOPTED_RGB.b) / 255.0,
            1.0,
        ))?);
        // The dashes come from the factory rather than a hand-rolled pattern of short strokes: a
        // `DrawRectangle` with a stroke style is one call, and the pattern then scales with the DPI
        // the same way the geometry does.
        unsafe {
            let factory: ID2D1Factory = self
                .d2d
                .GetFactory()
                .map_err(|error| super::hresult("ID2D1Resource::GetFactory", &error))?;
            let properties = D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_FLAT,
                endCap: D2D1_CAP_STYLE_FLAT,
                dashCap: D2D1_CAP_STYLE_FLAT,
                lineJoin: D2D1_LINE_JOIN_MITER,
                miterLimit: 10.0,
                dashStyle: D2D1_DASH_STYLE_DASH,
                dashOffset: 0.0,
            };
            self.scroll_dash_style = Some(
                factory
                    .CreateStrokeStyle(&properties, None)
                    .map_err(|error| super::hresult("ID2D1Factory::CreateStrokeStyle", &error))?,
            );
        }
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
        self.info_bg_brush = None;
        self.info_hex_brush = None;
        self.info_coord_value_brush = None;
        self.info_hint_brush = None;
        self.info_kbd_text_brush = None;
        self.info_kbd_fill_brush = None;
        self.info_kbd_bottom_brush = None;
        self.info_swatch_border_brush = None;
        self.info_swatch_inset_brush = None;
        self.info_swatch_fill_brush = None;
        self.blur_effect = None;
        self.info_tint_brush = None;
        self.info_badge_bg_brush = None;
        self.ann_stroke_brush = None;
        self.ann_fill_brush = None;
        self.scroll_panel_brush = None;
        self.scroll_strip_brush = None;
        self.scroll_box_brush = None;
        self.scroll_unadopted_brush = None;
        self.scroll_dash_style = None;
    }
}

use helpers::*;

mod helpers;
mod magnifier_pass;

#[cfg(test)]
mod tests;
