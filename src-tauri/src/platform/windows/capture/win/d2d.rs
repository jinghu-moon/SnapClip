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
    D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_GAUSSIANBLUR_OPTIMIZATION_BALANCED,
    D2D1_GAUSSIANBLUR_PROP_BORDER_MODE,
    D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION,
    D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
    D2D1_LAYER_PARAMETERS1, D2D1_PROPERTY_TYPE_UNKNOWN, D2D1_ROUNDED_RECT,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1_ELLIPSE,
    ID2D1Bitmap1, ID2D1DeviceContext, ID2D1Effect, ID2D1Geometry, ID2D1Image,
    ID2D1SolidColorBrush,
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
use crate::capture::annotation::{AnnotationGeometry, AnnotationItem, AnnotationKind};
use crate::capture::geometry::{Handle, MagnifierConfig, Point, Rect, SizeLabelPlacement};

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
    include_bytes!("../../../../../fonts/harmonyos-sans-sc-subset.ttf");
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

/// Mask colour (opaque black at 45% opacity). Configurable in one place so the
/// light/dark-background acceptance can be re-run with a different value.
pub const MASK_ALPHA: f32 = 0.45;
/// How much of the theme colour is laid over the neutral mask (docs/21 §5.22).
///
/// Deliberately small: the prototype measured a 30% blue tint pushing the background towards the
/// rings' hue and dropping a blue ring from 3.1:1 to 1.2–1.8:1, while 10% costs almost nothing.
pub const MASK_TINT_ALPHA: f32 = 0.10;
pub const MASK_RGB: (f32, f32, f32) = (0.0, 0.0, 0.0);

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
    pub annotation_items: Vec<crate::capture::annotation::AnnotationItem>,
    /// Id of the currently selected annotation (drives selection box drawing).
    pub annotation_selected_id: Option<crate::capture::annotation::AnnotationId>,
    /// Item being actively drawn (draft, not yet in `annotation_items`).
    pub annotation_draft: Option<crate::capture::annotation::AnnotationItem>,
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
    /// One-shot hint in back-buffer coordinates (docs/21 §5.21).
    pub hint: Option<(Point, String)>,
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
            hint: None,
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
            hint: None,
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
        // Fill the mask geometry — the frame, minus the selection hole — with `brush`. Four bands
        // instead of re-drawing the frame inside the hole keeps the selected pixels at their
        // original brightness; the same geometry is reused for the theme tint below.
        fn fill_mask(
            context: &ID2D1DeviceContext,
            frame: Rect,
            selection: Rect,
            brush: &ID2D1SolidColorBrush,
        ) {
            unsafe {
                if selection.is_empty() {
                    context.FillRectangle(&to_d2d(frame), brush);
                } else {
                    let hole = selection.intersect(frame);
                    for band in frame.surround(hole) {
                        if band.is_empty() {
                            continue;
                        }
                        context.FillRectangle(&to_d2d(band), brush);
                    }
                }
            }
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

        // L1: mask everything outside the selection. Using four bands (instead of
        // re-drawing the frame inside the hole) keeps the selected pixels at their
        // original brightness and lets a drag touch only the changed bands.
        fill_mask(&self.d2d, view.frame, view.selection, &mask);
        // …and a trace of the theme colour on top of it (docs/21 §5.22). Black does the darkening;
        // the tint is only brand presence. The prototype measured why it stays small: at 30% the
        // screen shifts towards the rings' hue and a blue ring drops from 3.1:1 to 1.2–1.8:1,
        // while 10% costs almost nothing.
        if let Some(tint) = self.mask_tint_brush.clone() {
            fill_mask(&self.d2d, view.frame, view.selection, &tint);
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
        let capture = self.require_brush(&self.capture_brush, "capture brush")?;
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
                    self.d2d.FillRectangle(&to_d2d(rect), &fill);
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
                // An element answer is the *capture*: the green that says "this is what you get",
                // distinct from the blue that means "a level on the chain" (docs/21 §5.22).
                let (fill, stroke, stroke_width) = if view.preview_is_window {
                    (
                        self.require_brush(&self.hover_fill_brush, "hover fill brush")?,
                        border.clone(),
                        width,
                    )
                } else {
                    (
                        self.require_brush(&self.preview_fill_brush, "preview fill brush")?,
                        capture.clone(),
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
        crate::capture::geometry::size_label_placement(
            selection,
            size,
            work_area,
            metrics.label_gap.round() as i32,
        )
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

    fn draw_magnifier(
        &mut self,
        view: &RenderView,
        resources: &ChromeResources,
        metrics: RenderMetrics,
    ) -> Result<(), String> {
        let config = MagnifierConfig::with_zoom(view.magnifier_zoom).scaled(metrics.dpi);
        let geometry = crate::capture::geometry::magnifier_geometry(
            view.cursor,
            config,
            view.frame,
            view.work_area,
        );
        // A missing frame bitmap means there is nothing to magnify yet.
        let Some(bitmap) = resources.frame_bitmap.as_ref() else {
            return Ok(());
        };
        let band = &resources.magnifier_band;
        let crosshair = &resources.crosshair;
        let grid = &resources.magnifier_grid;
        let dark = &resources.magnifier_info;
        let white = &resources.label_text;
        let panel = geometry.panel;
        let zoom = geometry.zoom; // f32: physical pixels per source pixel
        // Convert a source-pixel delta to a panel-pixel delta.
        let z2p = |src_delta: i32| (src_delta as f32 * zoom).round() as i32;

        // Rounded-corner clip: PushLayer with a geometric mask clips all content
        // to a rounded rectangle, giving the panel the soft-cornered look.
        let corner_radius = 8.0 * (metrics.dpi.max(96) as f32 / 96.0);
        let factory = unsafe { self.d2d.GetFactory() }
            .map_err(|e| super::hresult("ID2D1DeviceContext::GetFactory", &e))?;
        let rounded_geo = unsafe {
            factory.CreateRoundedRectangleGeometry(&D2D1_ROUNDED_RECT {
                rect: to_d2d(geometry.bounds),
                radiusX: corner_radius,
                radiusY: corner_radius,
            })
        }
        .map_err(|e| super::hresult("CreateRoundedRectangleGeometry", &e))?;
        let geo_mask: ID2D1Geometry = rounded_geo
            .cast()
            .map_err(|e| super::hresult("cast to ID2D1Geometry", &e))?;
        let layer = unsafe { self.d2d.CreateLayer(None) }
            .map_err(|e| super::hresult("CreateLayer", &e))?;

        unsafe {
            // Push the geometry-masked layer so all content is clipped to the
            // rounded-rect bounds.
            let lp = D2D1_LAYER_PARAMETERS1 {
                contentBounds: to_d2d(geometry.bounds),
                geometricMask: std::mem::ManuallyDrop::new(Some(geo_mask)),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: windows_numerics::Matrix3x2::identity(),
                opacity: 1.0,
                opacityBrush: std::mem::ManuallyDrop::new(None),
                layerOptions: Default::default(),
            };
            self.d2d.PushLayer(&lp, &layer);

            // Clip so the magnified pixels cannot spill out of the panel.
            self.d2d
                .PushAxisAlignedClip(&to_d2d(geometry.panel), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            // `geometry.source` is always exactly cursor-centred and may reach
            // past the frame at a screen edge; fill the whole panel first so the
            // out-of-frame margin shows a clean background instead of stale
            // pixels, then blit only the part of the source that actually exists.
            self.d2d.FillRectangle(&to_d2d(geometry.panel), dark);
            let visible_source = geometry.source.intersect(view.frame);
            if !visible_source.is_empty() {
                let visible_panel = Rect::new(
                    panel.left + z2p(visible_source.left - geometry.source.left),
                    panel.top + z2p(visible_source.top - geometry.source.top),
                    panel.left + z2p(visible_source.right - geometry.source.left),
                    panel.top + z2p(visible_source.bottom - geometry.source.top),
                );
                let interp = if zoom < 1.0 {
                    D2D1_INTERPOLATION_MODE_LINEAR
                } else {
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR
                };
                self.d2d.DrawBitmap(
                    bitmap,
                    Some(&to_d2d(visible_panel)),
                    1.0,
                    interp,
                    Some(&to_d2d(visible_source)),
                    None,
                );
            }

            // The sampled source pixel is `geometry.center` by construction;
            // since the source rect is built cursor-centred it always lands on
            // the panel's own centre cell, never translated aside at an edge.
            let half_w = (geometry.source.width() / 2).max(0);
            let half_h = (geometry.source.height() / 2).max(0);
            let center_x = panel.left + z2p(half_w);
            let center_y = panel.top + z2p(half_h);
            let cell_size = z2p(1).max(1);
            let center_cell = Rect::from_origin_size(Point::new(center_x, center_y), cell_size, cell_size);

            // Light-blue crosshair bands through the sampled pixel's whole row
            // and column: the eye follows them to the cell even before the ring.
            self.d2d.FillRectangle(
                &to_d2d(Rect::new(panel.left, center_y, panel.right, center_y + cell_size)),
                band,
            );
            self.d2d.FillRectangle(
                &to_d2d(Rect::new(center_x, panel.top, center_x + cell_size, panel.bottom)),
                band,
            );

            // Pixel grid: only at zoom >= 4 where each cell is wide enough to
            // render legibly. Integer-aligned to avoid sub-pixel blur.
            if geometry.zoom >= 4.0 {
                let step = geometry.zoom;
                // Vertical separators: one between each pair of columns.
                for index in 1..geometry.source.width() {
                    let x = panel.left as f32 + index as f32 * step + 0.5;
                    self.d2d.DrawLine(
                        vector2(x, panel.top as f32),
                        vector2(x, panel.bottom as f32),
                        grid,
                        1.0,
                        None,
                    );
                }
                // Horizontal separators: one between each pair of rows.
                for index in 1..geometry.source.height() {
                    let y = panel.top as f32 + index as f32 * step + 0.5;
                    self.d2d.DrawLine(
                        vector2(panel.left as f32, y),
                        vector2(panel.right as f32, y),
                        grid,
                        1.0,
                        None,
                    );
                }
            }

            // Mark the sampled cell with its true colour so it agrees with the
            // info-strip swatch. Both read the same async `magnifier_rgb`, so
            // filling (rather than relying on the blit's live-frame pixel) keeps
            // them identical even when sampling lags a frame. A white ring on the
            // cell edge plus a dark ring one pixel outside keep it readable on any
            // background.
            let cell_fill = self.require_brush(&self.info_swatch_fill_brush, "swatch fill")?;
            {
                let (r, g, b) = view.magnifier_rgb.unwrap_or((0, 0, 0));
                let _ = cell_fill.SetColor(&color(
                    r as f32 / 255.0,
                    g as f32 / 255.0,
                    b as f32 / 255.0,
                    1.0,
                ));
            }
            self.d2d.FillRectangle(&to_d2d(center_cell), &cell_fill);
            self.d2d.DrawRectangle(&to_d2d(center_cell), white, 1.0, None);
            let outer_cell = Rect::new(
                center_cell.left - 1,
                center_cell.top - 1,
                center_cell.right + 1,
                center_cell.bottom + 1,
            );
            self.d2d.DrawRectangle(&to_d2d(outer_cell), dark, 1.0, None);

            // ── Zoom badge (top-right corner of the loupe panel) ──
            {
                let badge_scale = metrics.dpi.max(96) as f32 / 96.0;
                let badge_font_sz = 14.0 * badge_scale;
                let badge_pad_h = 8.0 * badge_scale;
                let badge_pad_v = 5.0 * badge_scale;
                let badge_inset = 6.0 * badge_scale;
                let badge_radius = 6.0 * badge_scale;
                let label = if zoom >= 10.0 {
                    format!("{:.0}×", zoom)
                } else if zoom >= 1.0 {
                    let r = zoom.round();
                    if (zoom - r).abs() < 0.01 { format!("{:.0}×", r) } else { format!("{:.1}×", zoom) }
                } else {
                    format!("{:.1}×", zoom)
                };
                let badge_fmt = self.info_text_format_mut(false, 14.0, DWRITE_FONT_WEIGHT_BOLD, true)?;
                let text_w = self.measure_text_width_in(&label, &badge_fmt)?;
                let bw = text_w + badge_pad_h * 2.0;
                let bh = badge_font_sz + badge_pad_v * 2.0;
                let badge_rect = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: panel.right as f32 - badge_inset - bw,
                        top: panel.top as f32 + badge_inset,
                        right: panel.right as f32 - badge_inset,
                        bottom: panel.top as f32 + badge_inset + bh,
                    },
                    radiusX: badge_radius,
                    radiusY: badge_radius,
                };
                let badge_bg = self.require_brush(&self.info_badge_bg_brush, "zoom badge bg")?;
                self.d2d.FillRoundedRectangle(&badge_rect, &badge_bg);
                let wide = label.encode_utf16().collect::<Vec<u16>>();
                self.d2d.DrawText(
                    &wide, &badge_fmt, &badge_rect.rect,
                    white, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            self.d2d.PopAxisAlignedClip();

            // ── Info panel: two-column glass layout ──
            // Left: colour swatch + S-cycled value (row 1) and X/Y coordinates
            // (row 2). Right: three stacked `<kbd>` hint rows for S / C / P.
            let info = geometry.info_panel;
            let bg = self.require_brush(&self.info_bg_brush, "info bg")?;
            let primary_brush = self.require_brush(&self.info_hex_brush, "info primary colour")?;
            let coord_val_brush = self.require_brush(&self.info_coord_value_brush, "info coord")?;
            let hint_brush = self.require_brush(&self.info_hint_brush, "info hint")?;
            let kbd_text_brush = self.require_brush(&self.info_kbd_text_brush, "info kbd text")?;
            let kbd_fill_brush = self.require_brush(&self.info_kbd_fill_brush, "info kbd fill")?;
            let kbd_bottom_brush = self.require_brush(&self.info_kbd_bottom_brush, "info kbd bot")?;
            let swatch_border = self.require_brush(&self.info_swatch_border_brush, "swatch border")?;
            let swatch_inset = self.require_brush(&self.info_swatch_inset_brush, "swatch inset")?;
            let swatch_fill = self.require_brush(&self.info_swatch_fill_brush, "swatch fill")?;

            // Info-strip backdrop: real Gaussian blur restricted to `info`, with
            // a translucent tint on top. If either the effect or the tint brush
            // is missing (or there is no frame to sample), fall back to the old
            // solid `bg` so the strip never disappears.
            let blur_owned = self.blur_effect.clone();
            let tint_owned = self.info_tint_brush.clone();
            if let (Some(fb), Some(blur), Some(tint)) = (
                resources.frame_bitmap.as_ref(),
                blur_owned.as_ref(),
                tint_owned.as_ref(),
            ) {
                // Rebind the effect input every frame: the effect instance is
                // cached across sessions, but `frame_bitmap` is rebuilt per
                // session, so its input slot must be refreshed against the
                // currently frozen frame. SetInput returns (), not Result.
                let fb_img: ID2D1Image = fb
                    .cast()
                    .map_err(|error| super::hresult("ID2D1Bitmap1::cast", &error))?;
                blur.SetInput(0, &fb_img, true);
                // Mode A (context DPI = 96, coordinates in physical pixels):
                // std_dev is expressed in DIP (= physical pixels here); the
                // `* scale` widens the kernel proportionally on high-DPI
                // displays so the perceived blur strength stays constant.
                let std_dev = 12.0 * (metrics.dpi.max(96) as f32 / 96.0);
                blur.SetValue(
                    D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                    D2D1_PROPERTY_TYPE_UNKNOWN,
                    &std_dev.to_ne_bytes(),
                )
                .map_err(|error| {
                    super::hresult("ID2D1Effect::SetValue(STANDARD_DEVIATION)", &error)
                })?;
                // DrawImage is bounded to `info` via the source rectangle, so
                // D2D only evaluates the blur over the region actually needed
                // (info rect + kernel padding). Both optional pointers must be
                // raw pointers in windows-rs 0.61 (`Option<*const _>`).
                let dest = vector2(info.left as f32, info.top as f32);
                let src = to_d2d(info);
                let blur_img: ID2D1Image = blur
                    .cast()
                    .map_err(|error| super::hresult("ID2D1Effect::cast", &error))?;
                self.d2d
                    .DrawImage(
                        &blur_img,
                        Some(&dest as *const Vector2),
                        Some(&src as *const D2D_RECT_F),
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        D2D1_COMPOSITE_MODE_SOURCE_OVER,
                    );
                // 0.48 tint on top keeps text legible without hiding the texture.
                self.d2d.FillRectangle(&to_d2d(info), tint);
            } else {
                self.d2d.FillRectangle(&to_d2d(info), &bg);
            }

            // The loupe is a single fixed **physical**-pixel box (340×180 image +
            // info strip, DPI-invariant — see MagnifierConfig::PANEL_WIDTH_PHYSICAL)
            // clipped to one rounded rect, so the info strip shares that fixed
            // width. Its fonts and spacing therefore stay in physical pixels:
            // scaling them by DPI while the box stays put would overflow it on
            // high-DPI displays. On the single colour/coordinate line the panel is
            // wide enough that both fit with room to spare (asserted by
            // `widest_colour_value_fits_the_info_row`).
            let scale = 1.0f32;
            let pad_v = INFO_PADDING_V_DIP * scale;
            let pad_h = INFO_PADDING_H_DIP * scale;
            let swatch_sz = INFO_SWATCH_SIZE_DIP * scale;
            let swatch_radius = INFO_SWATCH_RADIUS_DIP * scale;
            let swatch_gap = INFO_SWATCH_GAP_DIP * scale;
            let coord_block_gap = INFO_COORD_BLOCK_GAP_DIP * scale;
            let kbd_pad = INFO_KBD_PADDING_DIP * scale;
            let kbd_radius = INFO_KBD_RADIUS_DIP * scale;
            let kbd_text_gap = INFO_KBD_TEXT_GAP_DIP * scale;
            let kbd_line_h = INFO_KBD_LINE_HEIGHT_DIP * scale;
            let hint_item_gap = INFO_HINT_ITEM_GAP_DIP * scale;
            let kbd_row_gap = INFO_KBD_ROW_GAP_DIP * scale;
            let kbd_block_gap = INFO_KBD_BLOCK_GAP_DIP * scale;

            // ── Row 1 (colour value + right-aligned coordinates) + 2×2 grid ──
            // Row 1: [swatch] colour value  ............  (x, y)   (coords right-aligned)
            // Row 2: [S 色值格式]  [C 复制色值]
            // Row 3: [P 坐标模式]  [Z 滚轮缩放]
            let content_l = info.left as f32 + pad_h;
            let content_r = info.right as f32 - pad_h;
            let row1_h = swatch_sz; // colour/coordinate line height (swatch-sized)
            let content_h = (info.bottom - info.top) as f32 - pad_v * 2.0;
            let block_h = row1_h + kbd_block_gap + kbd_line_h * 2.0 + kbd_row_gap;
            let row1_y = info.top as f32 + pad_v + (content_h - block_h).max(0.0) / 2.0;
            let kbd_row_a_y = row1_y + row1_h + kbd_block_gap;
            let kbd_row_b_y = kbd_row_a_y + kbd_line_h + kbd_row_gap;

            // ---- Row 1: swatch + colour value ----
            let swatch_rect = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: content_l,
                    top: row1_y + (row1_h - swatch_sz) / 2.0,
                    right: content_l + swatch_sz,
                    bottom: row1_y + (row1_h + swatch_sz) / 2.0,
                },
                radiusX: swatch_radius,
                radiusY: swatch_radius,
            };
            {
                let (r, g, b) = view.magnifier_rgb.unwrap_or((0, 0, 0));
                let _ = swatch_fill.SetColor(&color(
                    r as f32 / 255.0,
                    g as f32 / 255.0,
                    b as f32 / 255.0,
                    1.0,
                ));
            }
            self.d2d.FillRoundedRectangle(&swatch_rect, &swatch_fill);
            self.d2d.DrawRoundedRectangle(&swatch_rect, &swatch_border, 1.0, None);
            let inset_rect = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: swatch_rect.rect.left + 1.0,
                    top: swatch_rect.rect.top + 1.0,
                    right: swatch_rect.rect.right - 1.0,
                    bottom: swatch_rect.rect.bottom - 1.0,
                },
                radiusX: (swatch_radius - 1.0).max(0.0),
                radiusY: (swatch_radius - 1.0).max(0.0),
            };
            self.d2d.DrawRoundedRectangle(&inset_rect, &swatch_inset, 1.0, None);

            // Position: a single "(x, y)" string, right-aligned on row 1, so the
            // colour value keeps the remaining (left) width.
            let (cx, cy) = if view.magnifier_relative && !view.selection.is_empty() {
                (view.cursor.x - view.selection.left, view.cursor.y - view.selection.top)
            } else {
                (view.cursor.x + view.screen_origin.x, view.cursor.y + view.screen_origin.y)
            };
            let pos_text = format!("({}, {})", cx, cy);
            let value_format = self.info_text_format_mut(true, INFO_COORD_VALUE_FONT_DIP, DWRITE_FONT_WEIGHT_NORMAL, false)?;
            let pos_w = self.measure_text_width_in(&pos_text, &value_format)?;
            let coord_left = content_r - pos_w;

            // Colour value fills the band between the swatch and the position.
            // The 340 px panel leaves ample room, so the widest value fits without
            // ever reaching the position text (asserted by the info-row width test).
            let colour_text = view.magnifier_color_text.as_deref().unwrap_or("--");
            let primary_format = self.info_text_format_mut(true, INFO_PRIMARY_COLOR_FONT_DIP, DWRITE_FONT_WEIGHT_SEMI_BOLD, false)?;
            let colour_x = content_l + swatch_sz + swatch_gap;
            let colour_right = (coord_left - coord_block_gap).max(colour_x);
            {
                let wide = colour_text.encode_utf16().collect::<Vec<u16>>();
                // Draw across the full row-1 band and let the format's paragraph
                // centring put the text on the swatch's exact vertical centre; the
                // earlier manual (row1_h - font)/2 offset compounded with paragraph
                // centring pushed the glyphs ~3px below the swatch.
                self.d2d.DrawText(
                    &wide, &primary_format,
                    &D2D_RECT_F {
                        left: colour_x, top: row1_y,
                        right: colour_right, bottom: row1_y + row1_h,
                    },
                    &primary_brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            // Right-aligned position text, sharing row 1's vertical centre.
            {
                let wide_pos = pos_text.encode_utf16().collect::<Vec<u16>>();
                self.d2d.DrawText(
                    &wide_pos, &value_format,
                    &D2D_RECT_F {
                        left: coord_left, top: row1_y,
                        right: content_r, bottom: row1_y + row1_h,
                    },
                    &coord_val_brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            // ---- Row 2: four <kbd> hint items, laid out horizontally & centred ----
            // `hint_format` measures the CJK description (left-flushed); `hint_centered`
            // centres the single key letter inside its box. Both share the
            // vertical-centre paragraph alignment baked into `info_text_format_mut`.
            let hint_format = self.info_text_format_mut(false, INFO_HINT_FONT_DIP, DWRITE_FONT_WEIGHT_NORMAL, false)?;
            let hint_centered = self.info_text_format_mut(false, INFO_HINT_FONT_DIP, DWRITE_FONT_WEIGHT_NORMAL, true)?;
            let hints = INFO_HINTS;
            // Measure every item (box + gap + description); the two columns
            // share the widest item width so the grid lines up, and the whole
            // 2×2 block is centred inside the strip.
            let mut box_ws = [0.0f32; 4];
            let mut desc_ws = [0.0f32; 4];
            let mut item_ws = [0.0f32; 4];
            for i in 0..4 {
                let key_w = self.measure_text_width_in(hints[i].0, &hint_format)?;
                box_ws[i] = (key_w + kbd_pad * 2.0).max(kbd_line_h);
                desc_ws[i] = self.measure_text_width_in(hints[i].1, &hint_format)?;
                item_ws[i] = box_ws[i] + kbd_text_gap + desc_ws[i];
            }
            let col_w = item_ws.iter().cloned().fold(0.0f32, f32::max);
            let grid_w = col_w * 2.0 + hint_item_gap;
            let col0_x = (content_l + content_r - grid_w) / 2.0;
            let col1_x = col0_x + col_w + hint_item_gap;
            let slots = [
                (col0_x, kbd_row_a_y),
                (col1_x, kbd_row_a_y),
                (col0_x, kbd_row_b_y),
                (col1_x, kbd_row_b_y),
            ];
            for i in 0..4 {
                let (key, desc) = hints[i];
                let (item_x, row_y) = slots[i];
                let box_w = box_ws[i];
                let desc_w = desc_ws[i];
                let box_rect = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: item_x, top: row_y,
                        right: item_x + box_w, bottom: row_y + kbd_line_h,
                    },
                    radiusX: kbd_radius, radiusY: kbd_radius,
                };
                self.d2d.FillRoundedRectangle(&box_rect, &kbd_fill_brush);
                let bot_rect = D2D_RECT_F {
                    left: item_x + 1.0, top: row_y + kbd_line_h - 1.0,
                    right: item_x + box_w - 1.0, bottom: row_y + kbd_line_h,
                };
                self.d2d.FillRectangle(&bot_rect, &kbd_bottom_brush);
                {
                    let wide = key.encode_utf16().collect::<Vec<u16>>();
                    self.d2d.DrawText(
                        &wide, &hint_centered,
                        &D2D_RECT_F {
                            left: item_x, top: row_y,
                            right: item_x + box_w, bottom: row_y + kbd_line_h,
                        },
                        &kbd_text_brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
                let desc_x = item_x + box_w + kbd_text_gap;
                let wide_desc = desc.encode_utf16().collect::<Vec<u16>>();
                self.d2d.DrawText(
                    &wide_desc, &hint_format,
                    &D2D_RECT_F {
                        left: desc_x, top: row_y,
                        right: desc_x + desc_w + 2.0, bottom: row_y + kbd_line_h,
                    },
                    &hint_brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            // Panel outline: a thin dark rounded frame around the magnifier.
            let outline_rr = D2D1_ROUNDED_RECT {
                rect: to_d2d(geometry.bounds),
                radiusX: corner_radius,
                radiusY: corner_radius,
            };
            self.d2d.DrawRoundedRectangle(&outline_rr, dark, 1.0, None);
            self.d2d.PopLayer();
            core::mem::drop(lp);

            // The pointer reticle is a *local* crosshair centred on the cursor.
            // The magnifier itself deliberately has no detached pixel marker: the
            // sampled pixel is emphasised by the cell ring above, which moves with
            // the panel and never paints outside it.
            let reticle = crate::capture::geometry::crosshair_geometry(
                view.cursor,
                crate::capture::geometry::crosshair_radius(metrics.dpi),
                view.frame,
            );
            self.d2d.DrawLine(
                vector2(reticle.horizontal.left as f32 + 0.5, view.cursor.y as f32 + 0.5),
                vector2(reticle.horizontal.right as f32 - 0.5, view.cursor.y as f32 + 0.5),
                crosshair,
                1.0,
                None,
            );
            self.d2d.DrawLine(
                vector2(view.cursor.x as f32 + 0.5, reticle.vertical.top as f32 + 0.5),
                vector2(view.cursor.x as f32 + 0.5, reticle.vertical.bottom as f32 - 0.5),
                crosshair,
                1.0,
                None,
            );
        }
        Ok(())
    }

    /// Advance width of `text` rendered in a specific DirectWrite format.
    fn measure_text_width_in(&self, text: &str, format: &IDWriteTextFormat) -> Result<f32, String> {
        let wide = text.encode_utf16().collect::<Vec<u16>>();
        let layout = unsafe {
            self.dwrite
                .CreateTextLayout(&wide, format, f32::INFINITY, f32::INFINITY)
        }
        .map_err(|e| super::hresult("IDWriteFactory::CreateTextLayout", &e))?;
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe { layout.GetMetrics(&mut m) }
            .map_err(|e| super::hresult("IDWriteTextLayout::GetMetrics", &e))?;
        Ok(m.width)
    }

    /// Resolve (and cache) a DirectWrite format for the info panel.
    ///
    /// `mono`: use Consolas/Lucida Console; otherwise Segoe UI Variable Display/Segoe UI.
    /// `weight`: a `DWRITE_FONT_WEIGHT` constant (NORMAL/SEMI_BOLD/BOLD).
    fn info_text_format_mut(
        &mut self,
        mono: bool,
        size_dip: f32,
        weight: DWRITE_FONT_WEIGHT,
        center: bool,
    ) -> Result<IDWriteTextFormat, String> {
        let scale = self.metrics.dpi.max(96) as f32 / 96.0;
        let font_size = size_dip * scale;
        let family_slot = if mono { 1u8 } else { 0u8 };
        let size_key = (font_size * 100.0).round() as u32;
        let weight_val = weight.0 as u32;
        let align_val = if center { 1u32 } else { 0u32 };
        let key = (family_slot, size_key, weight_val, align_val);
        if let Some((_, f)) = self.info_formats.iter().find(|(k, _)| *k == key) {
            return Ok(f.clone());
        }
        let locale = to_wide("en-us");
        // The embedded HarmonyOS subset is the primary family for every info-panel
        // string; the slot's original fonts survive only as fallbacks in case the
        // private registration was refused (e.g. a locked-down font host).
        let fallbacks: &[&str] = if mono {
            &[MONO_FONT_FAMILY, MONO_FONT_FALLBACK]
        } else {
            &[LABEL_FONT_FAMILY, LABEL_FONT_FALLBACK]
        };
        let families = std::iter::once(INFO_FONT_FAMILY).chain(fallbacks.iter().copied());
        let dw_weight = weight;
        let mut format = None;
        let mut last_err = String::new();
        for family in families {
            let wide = to_wide(family);
            match unsafe {
                self.dwrite.CreateTextFormat(
                    PCWSTR(wide.as_ptr()),
                    None,
                    dw_weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    font_size,
                    PCWSTR(locale.as_ptr()),
                )
            } {
                Ok(created) => {
                    unsafe {
                        if center {
                            let _ = created.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
                        }
                        let _ = created.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                        // Info strings are always single-line; never wrap them.
                        let _ = created.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                    }
                    format = Some(created);
                    break;
                }
                Err(e) => {
                    last_err = super::hresult(family, &e);
                    continue;
                }
            }
        }
        let format = format.ok_or_else(|| format!("no usable info font family ({last_err})"))?;
        self.info_formats.push((key, format.clone()));
        Ok(format)
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
        let mask = color(
            MASK_RGB.0 * MASK_ALPHA,
            MASK_RGB.1 * MASK_ALPHA,
            MASK_RGB.2 * MASK_ALPHA,
            MASK_ALPHA,
        );
        // Selection border and handle outline: #1f75db.
        let accent = color(31.0 / 255.0, 117.0 / 255.0, 219.0 / 255.0, 1.0);
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
        let hover_fill = color(1.0, 1.0, 1.0, 0.10);
        self.hover_fill_brush = Some(self.create_brush(&hover_fill)?);
        // The preview is the *capture* (docs/21 §5.22): green wash + green outline, so the box that
        // would be taken is one colour and the blue chain around it is another.
        let preview_fill = color(
            CAPTURE_RGB.0,
            CAPTURE_RGB.1,
            CAPTURE_RGB.2,
            0.18,
        );
        self.preview_fill_brush = Some(self.create_brush(&preview_fill)?);
        self.capture_brush = Some(self.create_brush(&color(
            CAPTURE_RGB.0,
            CAPTURE_RGB.1,
            CAPTURE_RGB.2,
            1.0,
        ))?);
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
    }
}

/// Register the embedded info font privately for this process, once.
///
/// `AddFontMemResourceEx` loads the subset straight from the bytes baked into the
/// binary — no temp file, no system install, no admin rights. `OverlayRenderer::new`
/// follows with `IDWriteFactory::ReloadSystemFonts` so the shared DWrite factory
/// re-enumerates and can resolve `"HarmonyOS Sans SC"` by name. The embedded slice
/// is `'static`, so the registration stays valid for the whole process.
fn register_info_font_once() {
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| unsafe {
        let mut installed = 0u32;
        // A failure here is non-fatal: every text format falls back to a stock
        // family when the embedded one cannot be resolved.
        let _ = AddFontMemResourceEx(
            INFO_EMBEDDED_FONT.as_ptr().cast(),
            INFO_EMBEDDED_FONT.len() as u32,
            None,
            &mut installed,
        );
    });
}

fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

/// The chain's ring colour: the brand hue, lifted until it carries *luminance*, not just hue.
///
/// Two real-screen corrections went into this number. `#4a9bff` at 60% was invisible: the product's
/// mask is 45% black (the prototype's was 32%), a masked light page lands at ~140 grey, and a
/// mid-tone blue has almost exactly that luminance — the ring differed in hue and not in brightness.
/// Lifting it to `#8fc2ff` reached only 1.33:1 on that grey, because *any* mid-lightness colour does:
/// pale blue, pale grey and pale green all sit in the same luminance band. `#b9d9ff` keeps the hue
/// while clearing the grey (~2.4:1), and the underlay below supplies the local edge.
const CHAIN_RING_RGB: (f32, f32, f32) = (185.0 / 255.0, 217.0 / 255.0, 255.0 / 255.0);

/// Underlay drawn wider than every ring — the part that actually makes it legible.
///
/// A thin line over arbitrary screenshots cannot be made readable by colour alone: whatever
/// luminance it has, some screenshot matches it. The measured numbers on a masked *light* page
/// (~140 grey): ring core 1.33:1, ring + this underlay 4.9:1. It reads as a light line with a dark
/// edge — carved on light content, glowing on dark content.
const CHAIN_RING_SHADOW: f32 = 0.80;
/// How much wider than the ring the underlay is drawn, in logical pixels.
const CHAIN_RING_SHADOW_WIDTH: f32 = 2.0;

/// The capture green (`#1bb15f`): the box that would be taken, and only that.
///
/// Measured on the masked content: 3.2:1 on dark, 7.5:1 on light, and 3.2:1 even over a
/// blue-tinted mask — which is why the decisive element can carry the colour while the chain stays
/// blue (docs/21 §5.22).
const CAPTURE_RGB: (f32, f32, f32) = (27.0 / 255.0, 177.0 / 255.0, 95.0 / 255.0);

/// The ring brush is mutable (`SetColor` per ring) because each level carries its own opacity.
fn chain_ring_color(alpha: f32) -> D2D1_COLOR_F {
    color(
        CHAIN_RING_RGB.0,
        CHAIN_RING_RGB.1,
        CHAIN_RING_RGB.2,
        alpha.clamp(0.0, 1.0),
    )
}

/// Build the point type Direct2D expects.
fn vector2(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

fn to_d2d(rect: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: rect.left as f32,
        top: rect.top as f32,
        right: rect.right as f32,
        bottom: rect.bottom as f32,
    }
}

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::{
        ChainRingView, MASK_ALPHA, MASK_TINT_ALPHA, OverlayRenderer, RenderMetrics, RenderView,
        to_d2d,
    };
    use crate::capture::annotation::{
        AnnotationGeometry, AnnotationItem, AnnotationKind, AnnotationStyle,
    };
    use ::windows::Win32::Graphics::Direct2D::Common::D2D1_ALPHA_MODE_PREMULTIPLIED;
    use ::windows::Win32::Graphics::Direct2D::D2D1_BITMAP_OPTIONS_TARGET;
    use crate::capture::geometry::{
        Handle, Point, Rect, SizeLabelPlacement, size_label_placement,
    };

    /// A flat-coloured BGRA frame.
    fn solid_bgra(width: u32, height: u32, pixel: [u8; 4]) -> Vec<u8> {
        pixel
            .iter()
            .copied()
            .cycle()
            .take((width * height * 4) as usize)
            .collect()
    }

    fn pixel_at(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let offset = ((y * width + x) * 4) as usize;
        [
            pixels[offset],
            pixels[offset + 1],
            pixels[offset + 2],
            pixels[offset + 3],
        ]
    }

    /// The exact pixel the L1 mask produces for an opaque source pixel.
    ///
    /// Derived from the palette constants rather than hard-coded, so adding a layer to the mask (the
    /// theme tint, docs/21 §5.22) moves every assertion with it instead of breaking a dozen tests.
    /// Straight (non-premultiplied) blends; the readback is BGRA, so channel 0 is blue.
    fn masked(pixel: [u8; 4]) -> [u8; 4] {
        // The neutral mask is black at MASK_ALPHA…
        let masked = [0, 1, 2].map(|channel| pixel[channel] as f32 * (1.0 - MASK_ALPHA));
        // …and the theme tint is `#1f75db` at MASK_TINT_ALPHA, written in BGRA like a readback.
        const ACCENT_BGRA: [f32; 3] = [219.0, 117.0, 31.0];
        let out = [0, 1, 2].map(|channel| {
            masked[channel] * (1.0 - MASK_TINT_ALPHA) + ACCENT_BGRA[channel] * MASK_TINT_ALPHA
        });
        [
            out[0].round() as u8,
            out[1].round() as u8,
            out[2].round() as u8,
            255,
        ]
    }

    /// Compare a readback pixel with the palette's expected value, tolerating one unit per channel.
    ///
    /// The mask's layers are composited by Direct2D in premultiplied space, rounding at each stage,
    /// while `masked()` is a straight-alpha calculation — the two can disagree by one. Anything
    /// beyond that is a real difference and still fails.
    fn assert_pixel_close(actual: [u8; 4], expected: [u8; 4], what: &str) {
        for channel in 0..3 {
            assert!(
                (actual[channel] as i32 - expected[channel] as i32).abs() <= 1,
                "{what}: got {actual:?}, expected {expected:?}",
            );
        }
    }

    /// Visual regression for the L0/L1/L2 composition.
    ///
    /// Renders the exact composition the overlay presents into an offscreen target and
    /// reads it back, so the mask, the selection cut-out and the chrome are all
    /// verified against real GPU output rather than a mock.
    #[test]
    fn composed_frame_masks_outside_the_selection_and_keeps_it_clear() {
        let Ok(device) = super::GraphicsDevice::create() else {
            eprintln!("no D3D11 device in this session; skipping the composition check");
            return;
        };
        let width = 64u32;
        let height = 48u32;
        let background = [64u8, 96, 128, 255];
        let Ok(frame) = device.create_bgra_texture(width, height, &solid_bgra(width, height, background))
        else {
            return;
        };
        let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
            return;
        };
        renderer
            .update_frame(width, height, &solid_bgra(width, height, background))
            .unwrap();
        // The overlay creates its swap chain before the first draw; this also builds
        // the L1/L2 brushes the composition needs.
        renderer.ensure_back_buffer(width, height).unwrap();

        // A render target the test can read back.
        let target = renderer
            .device()
            .create_render_target_texture(width, height)
            .unwrap();
        let d2d_context = renderer.device().create_d2d_context().unwrap();
        let target_bitmap = super::super::d3d11::create_bitmap_from_texture(
            &d2d_context,
            &target.texture,
            D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
        .unwrap();

        let selection = Rect::new(16, 12, 48, 36);
        let mut view = RenderView::new(Rect::from_origin_size(
            Point::new(0, 0),
            width as i32,
            height as i32,
        ));
        view.show_chrome = false;
        view.cursor_visible = false;
        renderer.draw_to(&target_bitmap, &view).unwrap();
        let unselected = renderer.device().read_back_bgra(&target.texture).unwrap();
        // Before the first drag every pixel is masked; the frame shows through at the
        // mask's complement.
        assert_pixel_close(
            pixel_at(&unselected, width, 32, 24),
            masked(background),
            "an unselected frame must be uniformly masked",
        );

        view.selection = selection;
        view.cursor = Point::new(24, 18);
        // The magnifier draws cursor crosshairs across the whole frame, so it stays off
        // while the L0/L1 layers are checked.
        view.cursor_visible = false;
        view.show_chrome = true;

        renderer.draw_to(&target_bitmap, &view).unwrap();
        let composed = renderer.device().read_back_bgra(&target.texture).unwrap();

        // Inside the selection the L0 pixels survive untouched.
        let inside = pixel_at(&composed, width, 32, 24);
        assert_eq!(
            inside, background,
            "selected pixels must keep the raw back-buffer brightness"
        );

        // Outside the selection the mask darkens the frame by exactly MASK_ALPHA.
        let outside = pixel_at(&composed, width, 4, 4);
        assert!(
            outside[0] < background[0] && outside[1] < background[1] && outside[2] < background[2],
            "masked pixel {outside:?} must be darker than the frame {background:?}"
        );
        assert_pixel_close(
            outside,
            masked(background),
            "mask alpha must match the configured value",
        );

        let _ = frame;
    }

    /// The annotated export must be byte-identical to the crop of the exact
    /// composition the preview presents, with annotations baked in and the mask,
    /// selection chrome and control handles excluded (docs/11 §8.2/§8.3).
    ///
    /// `render_export` runs the same [`OverlayRenderer::draw_to`] layer path into a
    /// frame-sized offscreen target, then reads `selection` back. This renders the
    /// identical view through the swap-chain-style path (`draw_to` into a fresh
    /// render-target bitmap) and asserts the returned crop equals that full frame's
    /// selection region pixel-for-pixel, so a readback offset/pitch/format bug cannot
    /// pass. It then spot-checks that a red fill landed inside the artifact while a
    /// selected-but-unannotated pixel stayed at the raw frame brightness (no mask).
    #[test]
    fn export_crop_matches_preview_and_bakes_annotations() {
        let Ok(device) = super::GraphicsDevice::create() else {
            eprintln!("no D3D11 device in this session; skipping the export check");
            return;
        };
        let width = 64u32;
        let height = 48u32;
        let background = [64u8, 96, 128, 255];
        let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
            return;
        };
        renderer
            .update_frame(width, height, &solid_bgra(width, height, background))
            .unwrap();
        renderer.ensure_back_buffer(width, height).unwrap();

        // A preview target we can read the whole frame back from.
        let target = renderer
            .device()
            .create_render_target_texture(width, height)
            .unwrap();
        let d2d_context = renderer.device().create_d2d_context().unwrap();
        let target_bitmap = super::super::d3d11::create_bitmap_from_texture(
            &d2d_context,
            &target.texture,
            D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
        .unwrap();

        let frame = Rect::from_origin_size(Point::new(0, 0), width as i32, height as i32);
        let selection = Rect::new(16, 12, 48, 36);
        // A solid red filled rectangle sitting wholly inside the selection.
        let item = AnnotationItem {
            id: 1,
            kind: AnnotationKind::Rectangle,
            geometry: AnnotationGeometry::Rect {
                bounds: Rect::new(20, 16, 40, 30),
            },
            style: AnnotationStyle {
                stroke_color: [1.0, 0.0, 0.0, 1.0],
                fill_color: Some([1.0, 0.0, 0.0, 1.0]),
                ..AnnotationStyle::default()
            },
        };

        // The view the export path uses: chrome/cursor/selection-box/draft all off.
        let mut view = RenderView::new(frame);
        view.selection = selection;
        view.show_chrome = false;
        view.cursor_visible = false;
        view.annotation_items = vec![item];
        view.annotation_selected_id = None;
        view.annotation_draft = None;

        // Reference: present the same view and read the whole frame back.
        renderer.draw_to(&target_bitmap, &view).unwrap();
        let full = renderer.device().read_back_bgra(&target.texture).unwrap();

        // The artifact crop.
        let crop_w = selection.width() as u32;
        let crop_h = selection.height() as u32;
        let export = renderer.render_export(&view).unwrap();
        assert_eq!(
            export.len(),
            (crop_w * crop_h * 4) as usize,
            "export must be exactly the selection region, tightly packed"
        );

        // Every exported pixel equals the preview's same location: same code path,
        // correct row pitch and offset.
        for y in 0..crop_h {
            for x in 0..crop_w {
                let preview = pixel_at(&full, width, selection.left as u32 + x, selection.top as u32 + y);
                let artifact = pixel_at(&export, crop_w, x, y);
                assert_eq!(
                    artifact, preview,
                    "export pixel ({x},{y}) must match the presented preview crop"
                );
            }
        }

        // The annotation is baked into the artifact (not just the preview):
        // a fill-interior pixel is red, not the blue frame.
        let red = pixel_at(&export, crop_w, (30 - selection.left) as u32, (23 - selection.top) as u32);
        assert!(
            red[2] > 200 && red[0] < 60 && red[1] < 60,
            "annotation fill must appear in the export, got {red:?}"
        );

        // Selected-but-unannotated pixels keep the raw frame brightness: the L1 mask
        // and the L3 selection chrome never enter the artifact.
        let clear = pixel_at(&export, crop_w, 1, 1);
        assert_eq!(
            clear, background,
            "selected, unannotated pixels must stay raw (no mask/chrome)"
        );
    }

    /// The chrome is preview-only and must not touch the L0 pixels inside the
    /// selection; the border itself is drawn on the selection edge.
    #[test]
    fn chrome_stays_out_of_the_exported_pixels() {
        let Ok(device) = super::GraphicsDevice::create() else {
            return;
        };
        let width = 64u32;
        let height = 48u32;
        let background = [200u8, 180, 160, 255];
        let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
            return;
        };
        renderer
            .update_frame(width, height, &solid_bgra(width, height, background))
            .unwrap();
        renderer.ensure_back_buffer(width, height).unwrap();
        let target = renderer
            .device()
            .create_render_target_texture(width, height)
            .unwrap();
        let d2d_context = renderer.device().create_d2d_context().unwrap();
        let bitmap = super::super::d3d11::create_bitmap_from_texture(
            &d2d_context,
            &target.texture,
            D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
        .unwrap();

        let mut view = RenderView::new(Rect::from_origin_size(
            Point::new(0, 0),
            width as i32,
            height as i32,
        ));
        view.selection = Rect::new(16, 12, 48, 36);
        // The label prefers to sit below the selection, so chrome draws over some
        // selected pixels; the check below samples one it cannot cover.
        view.show_chrome = true;
        view.cursor_visible = false;

        renderer.draw_to(&bitmap, &view).unwrap();
        let composed = renderer.device().read_back_bgra(&target.texture).unwrap();

        // Inside the selection, at least one row stays untouched by the border and the
        // grips. The label cannot fit inside a frame this small, so it must be dropped
        // rather than painted over the selection.
        let preserved = (13..35).any(|y| pixel_at(&composed, width, 32, y) == background);
        assert!(
            preserved,
            "selected pixels must reach the output unmodified somewhere in the selection"
        );
    }

    /// Hover and preview are painted as their own layers, and neither is baked into the
    /// exported pixels (docs/14 §8).
    #[test]
    fn window_snap_hints_are_painted_but_never_exported() {
        let Ok(device) = super::GraphicsDevice::create() else {
            return;
        };
        let width = 64u32;
        let height = 48u32;
        let background = [200u8, 180, 160, 255];
        let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
            return;
        };
        renderer
            .update_frame(width, height, &solid_bgra(width, height, background))
            .unwrap();
        renderer.ensure_back_buffer(width, height).unwrap();
        let target = renderer
            .device()
            .create_render_target_texture(width, height)
            .unwrap();
        let d2d_context = renderer.device().create_d2d_context().unwrap();
        let bitmap = super::super::d3d11::create_bitmap_from_texture(
            &d2d_context,
            &target.texture,
            D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
        .unwrap();

        let mut view = RenderView::new(Rect::from_origin_size(
            Point::new(0, 0),
            width as i32,
            height as i32,
        ));
        view.cursor_visible = false;
        let window = Rect::new(8, 8, 40, 32);
        let sample = |pixels: &[u8], x: u32, y: u32| pixel_at(pixels, width, x, y);

        // Baseline: no hint at all, so the whole frame is masked down.
        renderer.draw_to(&bitmap, &view).unwrap();
        let bare = renderer.device().read_back_bgra(&target.texture).unwrap();
        let masked = sample(&bare, 20, 20);

        // Hover: the window is lifted out of the mask, everything else is untouched.
        view.hover_bounds = Some(window);
        renderer.draw_to(&bitmap, &view).unwrap();
        let hovered = renderer.device().read_back_bgra(&target.texture).unwrap();
        let hover_pixel = sample(&hovered, 20, 20);
        assert_ne!(hover_pixel, masked, "the hover wash must lighten the window");
        assert_eq!(
            sample(&hovered, 60, 44),
            masked,
            "the mask outside the hovered window must not change"
        );

        // Preview: its own accent wash, distinct from both the mask and the hover wash.
        view.hover_bounds = None;
        view.preview_bounds = Some(window);
        renderer.draw_to(&bitmap, &view).unwrap();
        let previewed = renderer.device().read_back_bgra(&target.texture).unwrap();
        let preview_pixel = sample(&previewed, 20, 20);
        assert_ne!(preview_pixel, masked, "the preview must be visible");
        assert_ne!(
            preview_pixel, hover_pixel,
            "the preview must be distinguishable from the hover hint"
        );

        // Deep-selection ancestors are outlined so the nesting is visible; the level that is
        // also the emphasised rectangle is skipped instead of being stroked twice.
        let ancestor = Rect::new(4, 4, 44, 36);
        view.hover_bounds = None;
        view.preview_bounds = Some(window);
        view.chain_rings = vec![
            ChainRingView { rect: ancestor, inner: false, alpha: 0.6 },
            ChainRingView { rect: window, inner: false, alpha: 0.6 },
        ];
        renderer.draw_to(&bitmap, &view).unwrap();
        let with_path = renderer.device().read_back_bgra(&target.texture).unwrap();
        assert_ne!(
            sample(&with_path, 24, 4),
            masked,
            "an ancestor level must be outlined"
        );

        // The ring is painted here; *how legible* it is belongs to
        // `chain_rings_carry_luminance_on_light_and_dark_content`, which scans the whole stroke —
        // a ring is a bright core plus a dark underlay, so one arbitrary pixel can legitimately sit
        // on either side of the background.
        let stroke: Vec<[u8; 4]> = (0..8).map(|y| sample(&with_path, 24, y)).collect();
        assert!(
            stroke.iter().any(|pixel| *pixel != masked),
            "an ancestor level must actually be painted: {stroke:?}",
        );

        // Inner rings are painted *above* the preview wash — the whole point of the two-pass order,
        // and what the prototype got wrong first (an accent line under an accent wash is invisible).
        //
        // The measurable consequence of the order: with the wash on, the ring's own pixels stay
        // (nearly) unchanged, because nothing is painted over them. Painted underneath, the wash
        // would tint every ring pixel. So draw the same inner ring twice — bare, then under the wash
        // — and compare the ring's own brightest pixel.
        let inner = Rect::new(12, 12, 36, 28);
        view.hover_bounds = None;
        view.chain_rings = vec![ChainRingView {
            rect: inner,
            inner: true,
            alpha: 0.9,
        }];
        let brightest = |pixels: &[u8]| {
            (6..22)
                .map(|y| sample(pixels, 24, y))
                .max_by_key(|pixel| pixel[0] as i32 + pixel[1] as i32 + pixel[2] as i32)
                .unwrap()
        };
        view.preview_bounds = None;
        renderer.draw_to(&bitmap, &view).unwrap();
        let bare = renderer.device().read_back_bgra(&target.texture).unwrap();
        let bare_ring = brightest(&bare);
        view.preview_bounds = Some(window);
        renderer.draw_to(&bitmap, &view).unwrap();
        let with_wash = renderer.device().read_back_bgra(&target.texture).unwrap();
        let washed_ring = brightest(&with_wash);
        assert_ne!(
            sample(&with_wash, 24, 22),
            sample(&bare, 24, 22),
            "the preview wash has to be painted for this comparison to mean anything",
        );
        let drift: i32 = (0..3)
            .map(|channel| (bare_ring[channel] as i32 - washed_ring[channel] as i32).abs())
            .sum();
        assert!(
            drift <= 30,
            "the inner ring moved by {drift} under the wash ({bare_ring:?} vs {washed_ring:?}): \
             it is being painted underneath it",
        );

        // Exporting the previewed rectangle must produce raw frozen pixels: a hint is a
        // hint, and it must never reach the artifact.
        view.hover_bounds = Some(window);
        view.selection = window;
        let exported = renderer.render_export(&view).unwrap();
        assert_eq!(exported.len(), (window.width() * window.height() * 4) as usize);
        assert!(
            exported
                .chunks_exact(4)
                .all(|pixel| pixel == background.as_slice()),
            "hover/preview hints must never be exported"
        );
    }

    /// The size label is painted with an opaque panel above the selection when there is room.
    /// The regression the user found on a real screen: a mid-tone blue ring on a **masked light**
    /// page. The mask (45% black) lands a white page at ~140 grey, whose luminance is nearly that of
    /// a mid blue — so the first ring colour differed in hue and not in brightness and could not be
    /// seen. This asserts the ring's *luminance* against the masked background, not just that it
    /// changed, and does it on light and dark content alike.
    #[test]
    fn chain_rings_carry_luminance_on_light_and_dark_content() {
        use crate::capture::geometry::Point as GPoint;

        let relative_luminance = |pixel: [u8; 4]| -> f32 {
            // Readback is BGRA; the eye weighs green most.
            let channel = |value: u8| {
                let value = value as f32 / 255.0;
                if value <= 0.03928 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(pixel[2]) + 0.7152 * channel(pixel[1]) + 0.0722 * channel(pixel[0])
        };
        let ratio = |a: [u8; 4], b: [u8; 4]| {
            let (high, low) = {
                let (a, b) = (relative_luminance(a), relative_luminance(b));
                if a > b { (a, b) } else { (b, a) }
            };
            (high + 0.05) / (low + 0.05)
        };

        for (name, background, inner) in [
            ("light", [250u8, 250, 250, 255], false),
            ("mid", [128, 128, 128, 255], false),
            ("dark", [24, 24, 28, 255], false),
            // The inner rings' actual situation: inside the capture wash, on light content — the
            // combination the user is looking at when the wheel has walked up a level.
            ("light + capture wash", [250, 250, 250, 255], true),
        ] {
            let Ok(device) = super::GraphicsDevice::create() else {
                return;
            };
            let width = 96u32;
            let height = 64u32;
            // The user's monitor: at 144 DPI the ring is 1.5 physical px, which is the case that has
            // to read. (The first version drew a raw physical pixel — thinner here than in any
            // prototype, which ran at 96 DPI.)
            let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 144) else {
                return;
            };
            renderer
                .update_frame(width, height, &solid_bgra(width, height, background))
                .unwrap();
            renderer.ensure_back_buffer(width, height).unwrap();
            let target = renderer
                .device()
                .create_render_target_texture(width, height)
                .unwrap();
            let context = renderer.device().create_d2d_context().unwrap();
            let bitmap = super::super::d3d11::create_bitmap_from_texture(
                &context,
                &target.texture,
                D2D1_BITMAP_OPTIONS_TARGET,
                D2D1_ALPHA_MODE_PREMULTIPLIED,
            )
            .unwrap();

            let mut view = RenderView::new(Rect::from_origin_size(
                GPoint::new(0, 0),
                width as i32,
                height as i32,
            ));
            view.cursor_visible = false;
            view.show_chrome = false;
            // The ring sits well inside the frame; the sample below is 6 px below it, i.e. masked
            // content with nothing else painted on it.
            let ring = Rect::new(20, 20, 76, 48);
            if inner {
                view.preview_bounds = Some(Rect::new(12, 12, 84, 56));
            }
            view.chain_rings = vec![ChainRingView {
                rect: ring,
                inner,
                alpha: 1.0, // the floor of the ramp, i.e. the faintest ring the plan can produce
            }];
            renderer.draw_to(&bitmap, &view).unwrap();
            let pixels = renderer.device().read_back_bgra(&target.texture).unwrap();
            // Scan the column through the ring's top edge: a 1.5 px line plus its 2.5 px underlay
            // are anti-aliased across a few rows, and what the eye uses is the *pair* — a bright
            // core with a dark edge. Sampling one arbitrary row measures neither.
            let column: Vec<[u8; 4]> = (16..26)
                .map(|y| pixel_at(&pixels, width, 48, y))
                .collect();
            let plain = pixel_at(&pixels, width, 48, 40);
            let brightest = *column
                .iter()
                .max_by(|a, b| relative_luminance(**a).total_cmp(&relative_luminance(**b)))
                .unwrap();
            let darkest = *column
                .iter()
                .min_by(|a, b| relative_luminance(**a).total_cmp(&relative_luminance(**b)))
                .unwrap();
            eprintln!(
                "[ring] {name}: brightest {brightest:?} {:.2}:1 · darkest {darkest:?} {:.2}:1 · \
                 background {plain:?}",
                ratio(brightest, plain),
                ratio(darkest, plain),
            );
            // The pair is what makes it legible, so the assertion is on the *better* side: on light
            // content the dark edge carries it (3.4:1 there, 1.5:1 for the core), on mid and dark
            // content the light core does (3.6:1 and 7.0:1). Requiring both sides would demand a
            // colour that no screenshot can ever match.
            let best = ratio(brightest, plain).max(ratio(darkest, plain));
            assert!(
                best >= 2.5,
                "on {name} content the ring peaks at {best:.2}:1 ({brightest:?} core / {darkest:?} \
                 edge against {plain:?})",
            );
        }
    }

    /// The size label is painted with an opaque panel above the selection when there is room.
    #[test]
    fn size_label_panel_is_painted_at_the_selection_top_left() {
        let Ok(device) = super::GraphicsDevice::create() else {
            return;
        };
        let width = 640u32;
        let height = 480u32;
        let background = [200u8, 180, 160, 255];
        let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
            return;
        };
        renderer
            .update_frame(width, height, &solid_bgra(width, height, background))
            .unwrap();
        renderer.ensure_back_buffer(width, height).unwrap();
        let target = renderer
            .device()
            .create_render_target_texture(width, height)
            .unwrap();
        let d2d_context = renderer.device().create_d2d_context().unwrap();
        let bitmap = super::super::d3d11::create_bitmap_from_texture(
            &d2d_context,
            &target.texture,
            D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
        .unwrap();

        let mut view = RenderView::new(Rect::from_origin_size(
            Point::new(0, 0),
            width as i32,
            height as i32,
        ));
        let selection = Rect::new(200, 200, 400, 280);
        view.selection = selection;
        view.show_chrome = true;
        view.cursor_visible = false;

        renderer.draw_to(&bitmap, &view).unwrap();
        let composed = renderer.device().read_back_bgra(&target.texture).unwrap();

        // Above the selection, aligned with its left edge, the label panel replaces
        // the masked frame with an opaque, high-contrast surface.
        let label_row = selection.top - 12;
        let panel = pixel_at(
            &composed,
            width,
            (selection.left + 12) as u32,
            label_row as u32,
        );
        assert_ne!(
            panel,
            masked(background),
            "the label panel must be painted at the selection top-left"
        );
        assert_eq!(panel[3], 255, "the label panel must be fully opaque");
    }

    /// Regression for the drag ghost: every present repaints the whole surface, so a
    /// moving selection cannot leave the previous frame's chrome behind. The size label
    /// and grips are painted partly *outside* the selection rect, so a partial repaint
    /// that only covered the raw selection would strand the old label — the exact ghost
    /// this guards against.
    #[test]
    fn moving_the_selection_erases_the_previous_chrome() {
        let Ok(device) = super::GraphicsDevice::create() else {
            return;
        };
        let width = 640u32;
        let height = 480u32;
        let background = [200u8, 180, 160, 255];
        let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
            return;
        };
        let frame = Rect::from_origin_size(Point::new(0, 0), width as i32, height as i32);
        renderer
            .update_frame(width, height, &solid_bgra(width, height, background))
            .unwrap();
        renderer.ensure_back_buffer(width, height).unwrap();
        let target = renderer
            .device()
            .create_render_target_texture(width, height)
            .unwrap();
        let d2d_context = renderer.device().create_d2d_context().unwrap();
        let bitmap = super::super::d3d11::create_bitmap_from_texture(
            &d2d_context,
            &target.texture,
            D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
        .unwrap();

        let selection_a = Rect::new(120, 120, 240, 200);
        let selection_b = Rect::new(360, 360, 480, 440);

        // Frame A: selection at A, chrome drawn.
        let mut view_a = RenderView::new(frame);
        view_a.selection = selection_a;
        view_a.show_chrome = true;
        view_a.cursor_visible = false;
        renderer.draw_to(&bitmap, &view_a).unwrap();
        let after_a = renderer.device().read_back_bgra(&target.texture).unwrap();

        // A probe on A's size label, above the top-left — outside the raw selection.
        let probe = Point::new(selection_a.left + 12, selection_a.top - 12);
        assert_ne!(
            pixel_at(&after_a, width, probe.x as u32, probe.y as u32),
            masked(background),
            "frame A must paint the size label above the selection"
        );

        // Frame B: move the selection far away. Because the whole surface is repainted,
        // the previous chrome is gone — a stale back buffer (the flip-model ghost) would
        // leave the label behind.
        let mut view_b = RenderView::new(frame);
        view_b.selection = selection_b;
        view_b.show_chrome = true;
        view_b.cursor_visible = false;
        renderer.draw_to(&bitmap, &view_b).unwrap();
        let after_b = renderer.device().read_back_bgra(&target.texture).unwrap();

        assert_pixel_close(
            pixel_at(&after_b, width, probe.x as u32, probe.y as u32),
            masked(background),
            "moving the selection must erase the previous chrome (ghost regression)",
        );
    }

    #[test]
    fn metrics_scale_with_dpi() {
        let at_100 = RenderMetrics::for_dpi(96);
        let at_200 = RenderMetrics::for_dpi(192);
        assert!((at_200.border_width - at_100.border_width * 2.0).abs() < 0.01);
        assert!((at_200.handle_size - at_100.handle_size * 2.0).abs() < 0.01);
        assert!((at_200.label_font_size - at_100.label_font_size * 2.0).abs() < 0.01);
        // Border stays visible even at 100%: never below one physical pixel.
        assert!(at_100.border_width >= 1.0);
    }

    #[test]
    fn label_text_reports_physical_pixels() {
        assert_eq!(
            RenderMetrics::label_text_at(Rect::new(0, 0, 1920, 1080), Point::new(0, 0)),
            "0,0 1920×1080 px"
        );
        assert_eq!(
            RenderMetrics::label_text_at(Rect::new(10, 10, 11, 11), Point::new(-1920, 200)),
            "-1910,210 1×1 px"
        );
    }

    #[test]
    fn label_size_grows_with_digits_and_dpi() {
        let metrics = RenderMetrics::for_dpi(96);
        let origin = Point::new(0, 0);
        let small = metrics.label_size_at(Rect::new(0, 0, 9, 9), origin);
        let large = metrics.label_size_at(Rect::new(0, 0, 1920, 1080), origin);
        assert!(large.0 > small.0);
        assert!(small.0 > 0 && small.1 > 0);
        let scaled = RenderMetrics::for_dpi(192);
        assert!(scaled.label_size_at(Rect::new(0, 0, 9, 9), origin).0 > small.0);
    }

    #[test]
    fn mask_alpha_is_translucent_but_dark() {
        assert!(MASK_ALPHA > 0.0 && MASK_ALPHA < 1.0);
    }

    #[test]
    fn geometry_helpers_match_the_shared_selection_maths() {
        // The renderer must not re-derive label placement: it delegates to the
        // tested geometry module.
        let placement = size_label_placement(
            Rect::new(0, 0, 100, 50),
            (80, 24),
            Rect::new(0, 0, 400, 400),
            8,
        );
        assert_eq!(
            placement,
            Some(SizeLabelPlacement {
                rect: Rect::new(0, 58, 80, 82),
                above: false
            })
        );
        for handle in Handle::ALL {
            let anchor = handle.anchor(Rect::new(0, 0, 100, 50));
            assert!(anchor.x >= 0 && anchor.x <= 100);
        }
        let _ = Point::new(0, 0);
        let _ = to_d2d(Rect::new(1, 2, 3, 4));
    }

    /// The info row must fit the **widest** colour string the sampler can emit
    /// (`hsl(360,100%,100%)`) alongside a representative position, so no colour
    /// value can ever wrap or clip. Widths are measured with the real embedded
    /// font, not an estimate — this is the correct way to prove the layout.
    #[test]
    fn widest_colour_value_fits_the_info_row() {
        let Ok(device) = super::GraphicsDevice::create() else {
            eprintln!("no D3D11 device; skipping the info-row width check");
            return;
        };
        let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
            return;
        };

        let colour_fmt = renderer
            .info_text_format_mut(
                true,
                super::INFO_PRIMARY_COLOR_FONT_DIP,
                super::DWRITE_FONT_WEIGHT_SEMI_BOLD,
                false,
            )
            .unwrap();
        let widest_colour = renderer
            .measure_text_width_in("hsl(360,100%,100%)", &colour_fmt)
            .unwrap();

        let pos_fmt = renderer
            .info_text_format_mut(
                true,
                super::INFO_COORD_VALUE_FONT_DIP,
                super::DWRITE_FONT_WEIGHT_NORMAL,
                false,
            )
            .unwrap();
        let widest_pos = renderer
            .measure_text_width_in("(2560, 1440)", &pos_fmt)
            .unwrap();

        // Horizontal budget at 96 DPI (scale = 1): the loupe is a fixed 340-
        // physical-px box. The colour value (left) and the coordinates (right)
        // share row 1, so both must fit in the space left after the swatch and
        // the gap between them.
        let content = crate::capture::geometry::MagnifierConfig::PANEL_WIDTH_PHYSICAL as f32
            - 2.0 * super::INFO_PADDING_H_DIP;
        let swatch_block = super::INFO_SWATCH_SIZE_DIP + super::INFO_SWATCH_GAP_DIP;
        let available = content - swatch_block - super::INFO_COORD_BLOCK_GAP_DIP;

        assert!(
            widest_colour + widest_pos <= available,
            "widest colour ({widest_colour}px) + coordinates ({widest_pos}px) do \
             not share row 1 (available {available}px = content {content} - swatch \
             {swatch_block} - gap {})",
            super::INFO_COORD_BLOCK_GAP_DIP
        );
    }

    /// Codepoints a TrueType font answers with a real glyph (`cmap` subtable formats 0/4/6/12).
    ///
    /// Deliberately small: this exists to check one subsetted font from a test, not to become a
    /// font library. Unknown subtable formats are skipped rather than guessed at.
    fn cmap_codepoints(bytes: &[u8]) -> std::collections::HashSet<u32> {
        use std::collections::HashSet;

        let u16_at = |at: usize| u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
        let u32_at = |at: usize| {
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize
        };
        let mut covered = HashSet::new();
        let mut cmap = None;
        for record in 0..u16_at(4) {
            let at = 12 + record * 16;
            if &bytes[at..at + 4] == b"cmap" {
                cmap = Some(u32_at(at + 8));
            }
        }
        let Some(cmap) = cmap else {
            return covered;
        };
        for subtable in 0..u16_at(cmap + 2) {
            let record = cmap + 4 + subtable * 8;
            let offset = cmap + u32_at(record + 4);
            match u16_at(offset) {
                // Byte encoding: a 256-entry glyph index array.
                0 => {
                    for code in 0..256 {
                        if bytes[offset + 6 + code] != 0 {
                            covered.insert(code as u32);
                        }
                    }
                }
                // Segment mapping: the one Windows uses for BMP text.
                4 => {
                    let segments = u16_at(offset + 6) / 2;
                    let ends = offset + 14;
                    let starts = ends + segments * 2 + 2;
                    let deltas = starts + segments * 2;
                    let ranges = deltas + segments * 2;
                    for segment in 0..segments {
                        let end = u16_at(ends + segment * 2);
                        let start = u16_at(starts + segment * 2);
                        let delta = u16_at(deltas + segment * 2);
                        let range = u16_at(ranges + segment * 2);
                        for code in start..=end {
                            if code == 0xFFFF {
                                continue;
                            }
                            let glyph = if range == 0 {
                                (code + delta) & 0xFFFF
                            } else {
                                // idRangeOffset is relative to its own slot.
                                let at = ranges + segment * 2 + range + (code - start) * 2;
                                if at + 2 > bytes.len() {
                                    continue;
                                }
                                let glyph = u16_at(at);
                                if glyph == 0 { 0 } else { (glyph + delta) & 0xFFFF }
                            };
                            if glyph != 0 {
                                covered.insert(code as u32);
                            }
                        }
                    }
                }
                // Trimmed mapping: first code + glyph index array.
                6 => {
                    let first = u16_at(offset + 6);
                    for i in 0..u16_at(offset + 8) {
                        if u16_at(offset + 10 + i * 2) != 0 {
                            covered.insert((first + i) as u32);
                        }
                    }
                }
                // Segmented coverage: (start, end, start glyph) triples, full Unicode range.
                12 => {
                    for group in 0..u32_at(offset + 12) {
                        let at = offset + 16 + group * 12;
                        for code in u32_at(at)..=u32_at(at + 4) {
                            covered.insert(code as u32);
                        }
                    }
                }
                _ => {}
            }
        }
        covered
    }

    /// Every string the overlay draws with the embedded family (docs/21 §5.21/§5.23).
    ///
    /// This is the one list, and it lives in Rust next to the drawing code. Wherever the text has
    /// a producer, the producer is *called* rather than copied, so the list cannot drift from what
    /// is painted. It feeds two things:
    ///
    /// * the coverage gate below — the embedded subset has to render every character of these;
    /// * `subfont/drawn-text.txt`, which `subfont/build_subset.py` turns into the subset's
    ///   codepoint list — so the font carries exactly the characters the overlay uses and nothing
    ///   that merely happens to appear in some other literal in the crate.
    ///
    /// Adding UI text means extending this list (or the producer it comes from). A string drawn
    /// inline in this file without being listed here is caught by the builder's guard, which scans
    /// the text-drawing files and fails on any character this list does not account for.
    fn overlay_drawn_strings() -> Vec<String> {
        use crate::capture::window_detection::LevelChain;
        use crate::platform::windows::capture::overlay::{LEVEL_HINT, level_hint, preview_label};

        let mut drawn = vec![
            // Size label, zoom badge, hex colour, and the two other colour formats.
            "-1910,210 1×1 px".to_owned(),
            "1.5×".to_owned(),
            "#ABCDEF".to_owned(),
            "rgb(255,0,0)".to_owned(),
            "hsl(359,100%,100%)".to_owned(),
        ];
        for (key, description) in super::INFO_HINTS {
            drawn.push(key.to_owned());
            drawn.push(description.to_owned());
        }
        let walked = {
            let mut chain = LevelChain::new(9);
            chain.shallower();
            chain
        };
        // Every state the preview label has: element, walked-to container, whole window, degraded.
        drawn.push(preview_label(Rect::new(0, 0, 341, 55), false, None, false));
        drawn.push(preview_label(Rect::new(0, 0, 689, 55), false, Some(walked), true));
        drawn.push(preview_label(Rect::new(0, 0, 3840, 2088), true, None, false));
        // …and the two one-shot hints.
        drawn.push(LEVEL_HINT.to_owned());
        drawn.push(level_hint(8, 9));
        drawn
    }

    /// Generate `subfont/drawn-text.txt` for the font builder; `subfont/subset.ps1` runs this.
    ///
    /// Ignored so the ordinary suite never writes files. The builder refuses to run on a file older
    /// than the sources, so a stale list cannot silently produce a font that is missing glyphs.
    #[test]
    #[ignore = "codegen for subfont/build_subset.py; run through subfont/subset.ps1"]
    fn write_drawn_text_for_the_font_subset() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../subfont/drawn-text.txt");
        let mut body = String::from(
            "// Generated by `cargo test --lib write_drawn_text_for_the_font_subset -- --ignored`\n\
             // from `overlay_drawn_strings()` in win/d2d.rs. Every line is one string the overlay\n\
             // draws; subfont/build_subset.py turns their characters into the font subset.\n\
             // Do not edit: extend `overlay_drawn_strings()` instead.\n",
        );
        for text in overlay_drawn_strings() {
            body.push_str(&text);
            body.push('\n');
        }
        std::fs::write(&path, body).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
        eprintln!("wrote {}", path.display());
    }

    /// The gate the hand-written `--unicodes` list kept failing to be (docs/21 §5.21).
    ///
    /// The embedded subset is a hard allowlist: a drawn string with a character that is not in it
    /// does not fail, log, or throw — DirectWrite falls back per glyph, so the text quietly renders
    /// half in another font (or as tofu where no fallback exists).
    #[test]
    fn the_embedded_subset_covers_the_strings_the_overlay_draws() {
        let covered = cmap_codepoints(super::INFO_EMBEDDED_FONT);
        let drawn = overlay_drawn_strings();

        for text in &drawn {
            for character in text.chars() {
                assert!(
                    covered.contains(&(character as u32)),
                    "the embedded subset has no glyph for {character:?} (U+{:04X}), drawn by \
                     {text:?}: rebuild it with subfont/subset.ps1, which regenerates the\n\
                     codepoint list from `overlay_drawn_strings()`",
                    character as u32,
                );
            }
        }
    }

    /// Glyph count from the font's `maxp` table — the ceiling a `cmap` reader can be held to.
    fn font_glyph_count(bytes: &[u8]) -> usize {
        let u16_at = |at: usize| u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
        let u32_at = |at: usize| {
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize
        };
        for record in 0..u16_at(4) {
            let at = 12 + record * 16;
            if &bytes[at..at + 4] == b"maxp" {
                return u16_at(u32_at(at + 8) + 4);
            }
        }
        0
    }

    /// The gate is only as good as its instrument, so the reader above is checked too: it has to
    /// Measurement, not a gate: what the embedded subset costs at runtime (docs/21 §5.23).
    ///
    /// The file is 14 KB, which is a *binary size* item. The runtime items are the one-time
    /// registration and the per-draw text layout, and neither depends on how many glyphs the file
    /// carries — only on the glyphs actually shaped. This prints both, next to two stock families
    /// (one of them a ~10 MB installed CJK font) so the comparison is visible rather than argued.
    #[test]
    #[ignore = "measurement probe; run with --ignored --nocapture"]
    fn font_cost_probe() {
        use ::windows::Win32::Graphics::DirectWrite::{
            DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_TEXT_METRICS,
        };
        use ::windows::Win32::Graphics::Gdi::RemoveFontMemResourceEx;
        use ::windows::core::PCWSTR;
        use std::time::Instant;

        let bytes = super::INFO_EMBEDDED_FONT;
        let samples = 200;
        let strings = ["3840×2088 px  容器 8/9", "#A1B2C3", "hsl(359,100%,100%)"];

        println!(
            "embedded subset: {} B, {} codepoints, {} glyphs",
            bytes.len(),
            cmap_codepoints(bytes).len(),
            font_glyph_count(bytes)
        );

        // 1) The one-time cost, measured on the same bytes the process registers at start-up.
        let mut installed = 0u32;
        let started = Instant::now();
        let handle = unsafe {
            ::windows::Win32::Graphics::Gdi::AddFontMemResourceEx(
                bytes.as_ptr().cast(),
                bytes.len() as u32,
                None,
                &mut installed,
            )
        };
        println!(
            "AddFontMemResourceEx      : {:.0} µs (once per process; {installed} face(s))",
            started.elapsed().as_secs_f64() * 1e6
        );
        if !handle.0.is_null() {
            let _ = unsafe { RemoveFontMemResourceEx(handle) };
        }

        // 2) Per-draw layout: the overlay builds a text layout for the size label and the preview
        //    label on every present (`measure_label`), so this is the number that shows up in a
        //    frame — and it is about shaping, not about the size of the font file.
        let Ok(device) = super::GraphicsDevice::create() else {
            eprintln!("no D3D11 device; skipping the layout half of the probe");
            return;
        };
        let Ok(renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
            eprintln!("no renderer; skipping the layout half of the probe");
            return;
        };
        for family in ["HarmonyOS Sans SC", "Microsoft YaHei UI", "Segoe UI"] {
            let wide = super::to_wide(family);
            let locale = super::to_wide("en-us");
            let format = unsafe {
                renderer.dwrite.CreateTextFormat(
                    PCWSTR(wide.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    12.0,
                    PCWSTR(locale.as_ptr()),
                )
            };
            let Ok(format) = format else {
                println!("{family:26}: unavailable");
                continue;
            };
            let mut worst = 0.0_f64;
            let mut total = 0.0_f64;
            for _ in 0..samples {
                for text in strings {
                    let wide = super::to_wide(text);
                    let started = Instant::now();
                    let Ok(layout) = (unsafe {
                        renderer.dwrite.CreateTextLayout(
                            &wide,
                            &format,
                            f32::INFINITY,
                            f32::INFINITY,
                        )
                    }) else {
                        continue;
                    };
                    let mut metrics = DWRITE_TEXT_METRICS::default();
                    let _ = unsafe { layout.GetMetrics(&mut metrics) };
                    let elapsed = started.elapsed().as_secs_f64() * 1e6;
                    worst = worst.max(elapsed);
                    total += elapsed;
                }
            }
            println!(
                "{family:26}: layout+metrics avg {:.1} µs, worst {:.1} µs  ({} samples)",
                total / (samples * strings.len()) as f64,
                worst,
                samples * strings.len()
            );
        }
    }

    /// The gate is only as good as its instrument, so the reader above is checked too: it has to
    /// find the glyphs the subset does carry and not invent ranges it never read.
    #[test]
    fn the_cmap_reader_finds_glyphs_and_does_not_invent_them() {
        let covered = cmap_codepoints(super::INFO_EMBEDDED_FONT);
        for character in ['×', '0', 'p', 'x', '色'] {
            assert!(
                covered.contains(&(character as u32)),
                "{character:?} is in the subset and the reader missed it"
            );
        }
        // A `cmap` answers codepoints with *glyphs*, and the font only has so many: a reader that
        // mis-reads an offset and walks a whole plane reports far more codepoints than that, which
        // is the failure mode that would make the gate above pass on a deficient font.
        let glyphs = font_glyph_count(super::INFO_EMBEDDED_FONT);
        assert!(glyphs > 50, "the subset should carry the chrome's glyphs ({glyphs})");
        assert!(
            covered.len() <= glyphs,
            "the reader reports {} codepoints for a font with {glyphs} glyphs",
            covered.len()
        );
        // The noncharacters are the one range a font is guaranteed never to map.
        assert!(
            !covered.contains(&0xFDD0),
            "U+FDD0 is a noncharacter and cannot be in the font"
        );
    }
}
