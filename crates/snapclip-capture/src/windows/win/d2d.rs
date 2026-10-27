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
    CLSID_D2D1GaussianBlur, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_CPU_READ, D2D1_BITMAP_OPTIONS_NONE,
    D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1, D2D1_DRAW_TEXT_OPTIONS_CLIP,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP, D2D1_GAMMA_2_2,
    D2D1_GAUSSIANBLUR_OPTIMIZATION_BALANCED, D2D1_GAUSSIANBLUR_PROP_BORDER_MODE,
    D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION, D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
    D2D1_LAYER_PARAMETERS1, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_MAP_OPTIONS_READ,
    D2D1_PROPERTY_TYPE_UNKNOWN,
    D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, ID2D1Bitmap1, ID2D1DeviceContext,
    ID2D1Brush, ID2D1Effect, ID2D1Geometry, ID2D1Image, ID2D1LinearGradientBrush, ID2D1RenderTarget,
    ID2D1SolidColorBrush,
};
use ::windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use ::windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_ISOLATED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_METRICS,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
};
use ::windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use ::windows::Win32::Graphics::Dxgi::IDXGISwapChain1;
use ::windows::core::{Interface, PCWSTR};
use std::sync::Arc;
use windows_numerics::Vector2;

use super::d3d11::GraphicsDevice;
use crate::annotation::{AnnotationGeometry, AnnotationItem, AnnotationKind};
// The palette and the light-math live in one place: the paint layer and the A4 ring decision read
// the same constants, so a re-measured mask or wash cannot leave the table describing a world that
// is no longer painted (docs/21 §5.24.11).
use crate::geometry::{
    Handle, LabelWhere, LevelReach, MagnifierConfig, Point, Rect, SizeLabelPlacement,
    magnifier_geometry,
};
use crate::keymap::LoupeHint;
use crate::overlay_font;
use crate::ring_contrast::{
    ACCENT_RGB, CAPTURE_RGB, CHAIN_RING_RGB, CHAIN_RING_SHADOW, HOVER_WASH_ALPHA, MASK_ALPHA,
    MASK_RGB, MASK_TINT_ALPHA, PREVIEW_WASH_ALPHA, PREVIEW_WASH_WALK_SCALE, Rgb, mix,
};
use crate::scroll::layout::Layout;
use crate::geometry::{
    REGION_SEGMENT_PAD_DIP, RegionPanelLayout, RegionPanelTarget, SegmentWidths, pill_radius,
};
use crate::region::{
    Delay, Destination, PanelSlot, Ratio, RegionField, RATIOS, RATIO_FREE_LABEL,
};
use crate::units::{LabelDisplay, LabelToggle, LengthUnit};
use snapclip_ui::panel::{Glyph, PanelTheme, Segment, SegmentState, Tag, TagState};
use snapclip_ui::panel::TagColors;
use snapclip_ui::panel::d2d::{PanelPainter, SegmentColors};
pub(crate) use guide_pass::{GUIDE_AVOID_DIP, GuideView};

/// The font family used for the size label, per the tasklist (§6.3) — the same two families the
/// overlay's typeface falls back to.
const LABEL_FONT_FAMILY: &str = overlay_font::FALLBACKS[0];
const LABEL_FONT_FALLBACK: &str = overlay_font::FALLBACKS[1];
const LABEL_FONT_SIZE_DIP: f32 = 12.0;

/// Stroke width of the selection border in DIP.
const BORDER_WIDTH_DIP: f32 = 1.5;
/// Diameter of each solid circular grip in DIP.
const HANDLE_SIZE_DIP: f32 = 12.0;

/// Monospace family used for every numeric value in the magnifier info panel.
const MONO_FONT_FAMILY: &str = "Consolas";
const MONO_FONT_FALLBACK: &str = "Lucida Console";

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

/// 自定义截图面板要画的东西（docs/23 §14.54）。
///
/// 与徽标本身同一个路子：绘制需要的每一项都由调用方算好放进来，画的地方只负责画 ——
/// 布局、命中目标、数值文本和状态都不在这里重算。
#[derive(Debug, Clone, PartialEq)]
pub struct RegionPanelView {
    /// 每个控件的矩形，也是命中测试读的那一份。
    pub layout: RegionPanelLayout,
    /// 四个数字格此刻写的字（正在输入的那一格写的是输入缓冲）。
    pub values: [(RegionField, String); 4],
    /// 四个数字格此刻是否非法（描红）。
    pub invalid: [bool; 4],
    /// 键盘焦点；面板没拿到焦点就是 `None`。
    pub focus: Option<PanelSlot>,
    /// 正在输入的那一格（数字缓冲的归属）。
    pub editing: Option<RegionField>,
    /// 插入点在缓冲里的位置（字节，0..=len）；光标画在它对应的 x 上。
    pub caret: usize,
    /// 光标这一刻是否可见：控制器用低频计时器开关它，于是光标会闪（docs/23 §14.55）。
    pub caret_visible: bool,
    /// 指针悬停的控件。
    pub hover: Option<RegionPanelTarget>,
    /// 单位组与延时组各自选中的项。
    pub unit: LengthUnit,
    pub delay: Delay,
    /// 分段器选中块的平移：`(起点段号, 进度 0..1)`；静止时 `None`（直接画在选中段上）。
    /// 段矩形始终由布局给出，插值只影响"画在哪"，命中判定读的还是布局（docs/23 §14.55.5）。
    pub unit_slide: Option<(usize, f32)>,
    pub delay_slide: Option<(usize, f32)>,
    /// 宽高比是否锁定。
    pub locked: bool,
    /// 三个可选标签的选中状态。
    pub toggles: [(LabelToggle, bool); 3],
    /// 页脚那句话（校验失败时）。
    pub status: Option<String>,
    /// 比例按钮上的字（`无比例` 或比例名）。
    pub ratio_label: String,
    /// 下拉里给哪一项打勾（`Ratio::menu_index`，0 是「无比例」）。
    pub ratio_selected: usize,
    /// 这张截图的落点（决定「确定」的文案与页脚读数）。
    pub destination: Destination,
    /// 页脚读数：没有校验错误时显示的那句。
    pub readout: String,
    /// 开始按钮是否可用。
    pub can_start: bool,
    /// 图片卡是否展开（收起时画摘要、不画圆角行）。
    pub image_open: bool,
    /// 图片卡卡头右侧的现状摘要（收起时显示）。
    pub image_summary: String,
    /// 圆角值块里那个数字。
    pub radius_text: String,
    /// 圆角（px）：值块的角标弧度与"0 时用 muted"都看它。
    pub radius_px: u32,
}

/// 倒计时遮罩层的内容：一帧"模糊 + 压暗"的像素，已读回 CPU（premultiplied BGRA、自上而下）。
///
/// 为什么需要它：遮罩层是 layered 窗口，内容由 `UpdateLayeredWindow` 一次性提交；而正常
/// 截图态是「冻帧 + 模糊 + 压暗」（`mask_blur` 那条）。只给纯色会让倒计时期间的观感比正常态
/// 少一层模糊（docs/23 §14.55.9.11）。
pub(crate) struct DimSource {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
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
    /// Whether the selection is committed enough to show edit grips.
    pub show_handles: bool,
    /// Rounding of the frame in physical pixels; the grips ride its arcs (docs/23 §14.55.21).
    pub corner_radius: i32,
    /// Whether the live size/coordinate label is useful in this state.
    pub show_size_label: bool,
    /// How the label reads — its unit, whether it carries the origin, whether it names the unit.
    ///
    /// How the readout reads, from the session's preferences — the value the custom-region panel
    /// edits while the session runs (docs/23 §14.54).
    pub label: LabelDisplay,
    /// Whether the pointer is on the badge, so it is painted as the control it is (docs/23 §14.53).
    ///
    /// Set from the pointer and the badge's own rectangle: it lights up the moment the pointer is on
    /// it, which is what tells the user that the badge is a control — and that a click there opens the
    /// custom-region panel (docs/23 §14.54).
    pub label_hovered: bool,
    /// 自定义截图面板（docs/23 §14.54）；`None` 表示这次会话没有打开它。
    pub region_panel: Option<RegionPanelView>,
    /// 延时倒计时的剩余秒数（docs/23 §14.55.9）。
    pub countdown: Option<u32>,
    /// Whether the screenshot loupe is active for this interaction state.
    pub show_magnifier: bool,
    /// A short-lived cue that arrow keys are moving the sampling cursor.
    pub precision_feedback: bool,
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
            show_handles: true,
            corner_radius: 0,
            show_size_label: true,
            label: LabelDisplay::default(),
            label_hovered: false,
            region_panel: None,
            countdown: None,
            show_magnifier: true,
            precision_feedback: false,
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
    /// 延时倒计时的数字：主题色 80% 透明度（docs/23 §14.55.9）。
    countdown_brush: Option<ID2D1SolidColorBrush>,
    mask_brush: Option<ID2D1SolidColorBrush>,
    /// 圆角洞的"框减去圆角洞"几何（帧、洞、半径变化时重建）。见 `rounded_hole_geometry`。
    mask_ring: Option<(Rect, Rect, i32, ID2D1Geometry)>,
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
    /// The custom-region panel's own parts (docs/23 §14.54): the fill of its cards and of whatever
    /// the pointer is on, its hairline, the accent that marks the selected segment and the primary
    /// button, and a muted foreground for everything that is not the value being edited.
    panel_fill_brush: Option<ID2D1SolidColorBrush>,
    panel_line_brush: Option<ID2D1SolidColorBrush>,
    panel_accent_brush: Option<ID2D1SolidColorBrush>,
    panel_muted_brush: Option<ID2D1SolidColorBrush>,
    /// 图片卡专用的三支：底（主色 14%）、边（主色 60%）、标题与箭头（浅主色）。
    /// 输入区一律中性灰、图片卡一律主色淡染，不靠分隔线区分「截什么」和「出什么」
    /// （docs/23 §14.55.19）。
    panel_out_fill_brush: Option<ID2D1SolidColorBrush>,
    panel_out_line_brush: Option<ID2D1SolidColorBrush>,
    panel_out_title_brush: Option<ID2D1SolidColorBrush>,
    /// 自定义截图面板的"这一格不能用"：校验失败时描红下划线与标签（docs/23 §14.54）。
    panel_danger_brush: Option<ID2D1SolidColorBrush>,
    /// 面板图形：`assets/icon` 的 PNG（icon-bake 用 resvg 从 SVG 烘出来的），WIC 解码后按状态色
    /// 预着色（docs/23 §14.54）。
    panel_icons: PanelIcons,
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
    /// The size badge floating on the thumbnail.
    scroll_badge_brush: Option<ID2D1SolidColorBrush>,
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
    /// The footer's status line.
    pub status: String,
    /// The buttons' texts, in [`Layout::buttons`] order: key and label.
    pub buttons: [String; 2],
}

/// The scroll panel's colour, the demo's `rgba(18,20,24,.92)`.
const SCROLL_PANEL: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 18.0 / 255.0,
    g: 20.0 / 255.0,
    b: 24.0 / 255.0,
    a: 0.92,
};
const SCROLL_BUTTON_PAD_DIP: f32 = 3.0;
/// The size badge floating on the thumbnail: the loupe's `20×` badge in everything but the number.
const SCROLL_BADGE_OPACITY: f32 = 0.55;
const SCROLL_BADGE_PAD_DIP: f32 = 7.0;
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

/// The loupe's rectangle when it is on screen, for the size label to step aside (docs/23 §14.53).
///
/// `cursor_visible` and `show_magnifier` are two different questions, and this asks both: the first
/// is "is the pointer over the overlay", the second is "is the loupe painted". A label that stepped
/// aside for an invisible loupe would place itself around nothing.
fn loupe_obstacle(view: &RenderView, metrics: RenderMetrics) -> Option<Rect> {
    if !view.cursor_visible || !view.show_magnifier {
        return None;
    }
    let work_area = if view.work_area.is_empty() {
        view.frame
    } else {
        view.work_area
    };
    Some(
        magnifier_geometry(
            view.cursor,
            MagnifierConfig::with_zoom(view.magnifier_zoom).scaled(metrics.dpi),
            view.frame,
            work_area,
        )
        .bounds,
    )
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
        overlay_font::register();
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
            countdown_brush: None,
            mask_brush: None,
            mask_ring: None,
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
            panel_fill_brush: None,
            panel_line_brush: None,
            panel_accent_brush: None,
            panel_muted_brush: None,
            panel_out_fill_brush: None,
            panel_out_line_brush: None,
            panel_out_title_brush: None,
            panel_danger_brush: None,
            panel_icons: PanelIcons::new(),
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
            scroll_badge_brush: None,
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
            label_hovered: false,
            region_panel: None,
            countdown: None,
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

    /// 倒计时遮罩层要显示的那一帧：冻帧先模糊、再压上遮罩的两层，然后读回 CPU。
    ///
    /// `divisor` > 1 表示按 1/divisor 分辨率渲染：模糊本身就是低频，放大回来观感几乎无损，
    /// 而读回的数据量按平方下降 —— 4K 屏上这是把这次一次性开销压到几十毫秒以内的关键。
    ///
    /// 必须在帧之外调用（内部自己 `BeginDraw`/`EndDraw`）；耗时由调用方测量。
    pub(crate) fn countdown_dim_source(&mut self, divisor: u32) -> Result<DimSource, String> {
        self.ensure_blurred_frame()?;
        let (Some(frame), Some(blurred)) = (self.frame_bitmap.clone(), self.blurred_frame.clone())
        else {
            return Err("倒计时遮罩：没有冻结帧或模糊帧".to_owned());
        };
        let full = unsafe { frame.GetPixelSize() };
        if full.width == 0 || full.height == 0 {
            return Err("倒计时遮罩：冻结帧尺寸为 0".to_owned());
        }
        let divisor = divisor.max(1);
        let width = (full.width / divisor).max(1);
        let height = (full.height / divisor).max(1);
        let size = D2D_SIZE_U { width, height };
        let target = unsafe {
            self.d2d
                .CreateBitmap(size, None, 0, &Self::dim_bitmap_properties(false))
        }
        .map_err(|error| super::hresult("ID2D1DeviceContext::CreateBitmap(dim target)", &error))?;
        let readback = unsafe {
            self.d2d
                .CreateBitmap(size, None, 0, &Self::dim_bitmap_properties(true))
        }
        .map_err(|error| super::hresult("ID2D1DeviceContext::CreateBitmap(dim readback)", &error))?;
        let (Some(mask), tint) = (self.mask_brush.clone(), self.mask_tint_brush.clone()) else {
            return Err("倒计时遮罩：遮罩画刷未创建".to_owned());
        };
        let previous = unsafe { self.d2d.GetTarget() }.ok();
        let area = to_d2d(Rect::from_origin_size(
            Point::new(0, 0),
            width as i32,
            height as i32,
        ));
        unsafe {
            self.d2d.SetTarget(&target);
            self.d2d.BeginDraw();
            let clear = color(0.0, 0.0, 0.0, 0.0);
            self.d2d.Clear(Some(&clear));
            // 缩小是 4× 以上、源已经模糊过，双线性就够（没有高频可混叠）。
            self.d2d.DrawBitmap(
                &blurred,
                Some(&area),
                1.0,
                D2D1_INTERPOLATION_MODE_LINEAR,
                None,
                None,
            );
            // 与正常截图态同一组画刷：先黑色遮罩，再那一点点主题色。
            self.d2d.FillRectangle(&area, &mask);
            if let Some(tint) = tint.as_ref() {
                self.d2d.FillRectangle(&area, tint);
            }
            self.d2d.EndDraw(None, None)
        }
        .map_err(|error| super::hresult("ID2D1DeviceContext::EndDraw(dim)", &error))?;
        unsafe { readback.CopyFromBitmap(None, &target, None) }
            .map_err(|error| super::hresult("ID2D1Bitmap1::CopyFromBitmap(dim)", &error))?;
        let mapped = unsafe { readback.Map(D2D1_MAP_OPTIONS_READ) }
            .map_err(|error| super::hresult("ID2D1Bitmap1::Map(dim)", &error))?;
        let row_bytes = (width as usize) * 4;
        let mut pixels = vec![0u8; row_bytes * height as usize];
        for row in 0..height as usize {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    mapped.bits.add(row * mapped.pitch as usize),
                    pixels.as_mut_ptr().add(row * row_bytes),
                    row_bytes,
                )
            };
        }
        let _ = unsafe { readback.Unmap() };
        if let Some(previous) = previous {
            unsafe { self.d2d.SetTarget(&previous) };
        }
        Ok(DimSource {
            width,
            height,
            pixels,
        })
    }

    /// 遮罩层那两张位图的属性：一张可画（目标），一张只能读（读回）。
    fn dim_bitmap_properties(cpu_read: bool) -> D2D1_BITMAP_PROPERTIES1 {
        D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: if cpu_read {
                D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW
            } else {
                D2D1_BITMAP_OPTIONS_TARGET
            },
            ..Default::default()
        }
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

        // A delay countdown: the window is cut down to the frame's band, the badge and the countdown
        // box (`overlay/session.rs`), so this surface must be **transparent apart from exactly those
        // pieces**. Anything else drawn here — the frozen frame, the mask, the snap hints, the guide,
        // the loupe, the panel (already closed) — lands inside that region as a grey block over the
        // live screen (docs/23 §14.55.9.2). Hence an early return instead of a flag on every layer:
        // what is painted and what the region covers are the same set by construction.
        //
        // `FrameMarks::EDITING` is the whole of the first piece: the same routine that draws a
        // plain capture's frame draws its handles and its size label (`marks.size_label`).
        if let Some(seconds) = view.countdown {
            unsafe { self.d2d.Clear(Some(&color(0.0, 0.0, 0.0, 0.0))) };
            self.draw_selection_frame(view, FrameMarks::EDITING)?;
            let text = self.require_brush(&self.countdown_brush, "countdown brush")?;
            let format =
                self.info_text_format_mut(false, COUNTDOWN_DIP, DWRITE_FONT_WEIGHT_SEMI_BOLD, true)?;
            self.draw_text_in(&seconds.to_string(), &format, to_d2d(view.selection), &text)?;
            return Ok(());
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
        // A rounded selection needs a rounded hole: four straight bands leave the wedge between
        // each arc and the corner of its rectangle undimmed (docs/23 §14.55.23). Only when the hole
        // *is* the selection — a hover preview's hole is that window's own rectangle.
        let rounded_hole = (view.corner_radius > 0 && hole == view.selection)
            .then(|| crate::geometry::radius_cap(hole.width(), hole.height()).min(view.corner_radius))
            .filter(|radius| *radius > 0);
        // The blurred mask (a setting): the frame blurred outside the hole, under the same mask
        // and tint, so the selection is the one sharp thing on screen.
        if let Some(radius) = rounded_hole {
            let ring = self.rounded_hole_geometry(view.frame, hole, radius)?;
            if view.mask_blur
                && let Some(blurred) = self.blurred_frame.clone()
            {
                self.draw_bitmap_through(view.frame, &ring, &blurred)?;
            }
            unsafe { self.d2d.FillGeometry(&ring, &mask, None::<&ID2D1Brush>) };
        } else {
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
        }
        // …and a trace of the theme colour on top of it (docs/21 §5.22). Black does the darkening;
        // the tint is only brand presence. The prototype measured why it stays small: at 30% the
        // screen shifts towards the rings' hue and a blue ring drops from 3.1:1 to 1.2–1.8:1,
        // while 10% costs almost nothing.
        if let Some(tint) = self.mask_tint_brush.clone() {
            match rounded_hole {
                Some(radius) => {
                    let ring = self.rounded_hole_geometry(view.frame, hole, radius)?;
                    unsafe { self.d2d.FillGeometry(&ring, &tint, None::<&ID2D1Brush>) };
                }
                None => fill_outside(&self.d2d, view.frame, hole, &tint),
            }
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

        // L3.7: the custom-region panel, above the badge: it is the layer the user is typing into,
        // and its own dropdown hangs over everything below it (docs/23 §14.54).
        if let Some(panel) = view.region_panel.as_ref() {
            self.draw_region_panel(panel)?;
        }

        Ok(())
    }

    /// "Whole frame minus a rounded hole", cached by its three inputs.
    ///
    /// The third consumer of the selection's radius, after the frame stroke and the image mask:
    /// the dim layer outside the hole has to follow the same arc or the corner wedges stay bright
    /// (docs/23 §14.55.23).
    fn rounded_hole_geometry(
        &mut self,
        frame: Rect,
        hole: Rect,
        radius: i32,
    ) -> Result<ID2D1Geometry, String> {
        if let Some((cached_frame, cached_hole, cached_radius, geometry)) = self.mask_ring.as_ref()
            && *cached_frame == frame
            && *cached_hole == hole
            && *cached_radius == radius
        {
            return Ok(geometry.clone());
        }
        let factory = unsafe { self.d2d.GetFactory() }
            .map_err(|error| super::hresult("ID2D1DeviceContext::GetFactory", &error))?;
        let outer: ID2D1Geometry = unsafe { factory.CreateRectangleGeometry(&to_d2d(frame)) }
            .map_err(|error| super::hresult("CreateRectangleGeometry", &error))?
            .cast()
            .map_err(|error| super::hresult("cast to ID2D1Geometry", &error))?;
        let rounded = D2D1_ROUNDED_RECT {
            rect: to_d2d(hole),
            radiusX: radius as f32,
            radiusY: radius as f32,
        };
        let inner: ID2D1Geometry = unsafe { factory.CreateRoundedRectangleGeometry(&rounded) }
            .map_err(|error| super::hresult("CreateRoundedRectangleGeometry", &error))?
            .cast()
            .map_err(|error| super::hresult("cast to ID2D1Geometry", &error))?;
        // Alternate fill: the rounded hole inside the frame rect cancels out, leaving the band.
        // (`ID2D1Factory` in this windows crate exposes `CreateGeometryGroup`, not
        // `CreateCombinedGeometry`.)
        let pieces = [Some(outer), Some(inner)];
        let geometry: ID2D1Geometry = unsafe {
            factory.CreateGeometryGroup(
                windows::Win32::Graphics::Direct2D::Common::D2D1_FILL_MODE_ALTERNATE,
                &pieces,
            )
        }
        .map_err(|error| super::hresult("CreateGeometryGroup", &error))?
        .cast()
        .map_err(|error| super::hresult("cast to ID2D1Geometry", &error))?;
        self.mask_ring = Some((frame, hole, radius, geometry.clone()));
        Ok(geometry)
    }

    /// Draw `bitmap` over `bounds`, clipped to `clip` — the blurred mask under a rounded hole.
    fn draw_bitmap_through(
        &self,
        bounds: Rect,
        clip: &ID2D1Geometry,
        bitmap: &ID2D1Bitmap1,
    ) -> Result<(), String> {
        let layer = unsafe { self.d2d.CreateLayer(None) }
            .map_err(|error| super::hresult("CreateLayer", &error))?;
        let parameters = D2D1_LAYER_PARAMETERS1 {
            contentBounds: to_d2d(bounds),
            geometricMask: std::mem::ManuallyDrop::new(Some(clip.clone())),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: windows_numerics::Matrix3x2::identity(),
            opacity: 1.0,
            opacityBrush: std::mem::ManuallyDrop::new(None),
            layerOptions: Default::default(),
        };
        let rect = to_d2d(bounds);
        unsafe {
            self.d2d.PushLayer(&parameters, &layer);
            self.d2d.DrawBitmap(
                bitmap,
                Some(&rect),
                1.0,
                D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                Some(&rect),
                None,
            );
            self.d2d.PopLayer();
        }
        let _ = std::mem::ManuallyDrop::into_inner(parameters.geometricMask);
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
        {
            // 方向芯片（↑/↓）同样是胶囊：以前写死 3.0，所以三处芯片里只有一处是真胶囊。
            let radius = pill_radius(placement.rect);
            let painter = PanelPainter {
                context: &self.d2d,
                text_factory: &self.dwrite,
            };
            painter.fill_round(placement.rect, radius.round() as i32, &background);
            let text_rect = D2D_RECT_F {
                left: placement.rect.left as f32 + metrics.label_padding_x,
                top: placement.rect.top as f32 + metrics.label_padding_y,
                right: placement.rect.right as f32 - metrics.label_padding_x,
                bottom: placement.rect.bottom as f32 - metrics.label_padding_y,
            };
            painter.draw_text_rect(text, &format, text_rect, &foreground);
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
        if view.cursor_visible && view.show_magnifier {
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
        // 圆角跟着选区走（docs/23 §14.55.21）：半径 0 就是原来的直角框，半径越大弧越明显。
        // 半径上限与手柄布局、图像遮罩共用 `geometry::radius_cap`。
        let frame_radius = view
            .corner_radius
            .min(crate::geometry::radius_cap(selection.width(), selection.height()));
        if frame_radius > 0 {
            let inset = (metrics.border_width / 2.0).round() as i32;
            self.stroke_round(
                Rect::new(
                    selection.left + inset,
                    selection.top + inset,
                    selection.right - inset,
                    selection.bottom - inset,
                ),
                (frame_radius - inset).max(0),
                &border,
                metrics.border_width,
            );
        } else {
            unsafe {
                self.d2d.DrawRectangle(
                    &frame_stroke_rect(selection, metrics.border_width),
                    &border,
                    metrics.border_width,
                    None,
                );
            }
        }
        if marks.handles && view.show_handles {
            let handle = self.require_brush(&self.handle_brush, "handle brush")?;
            let radius = metrics.handle_size / 2.0;
            for grip in Handle::ALL {
                let anchor = if matches!(
                    grip,
                    Handle::TopLeft | Handle::TopRight | Handle::BottomRight | Handle::BottomLeft
                ) {
                    crate::geometry::corner_handle_anchor(selection, frame_radius, grip)
                } else {
                    grip.anchor(selection)
                };
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
        if marks.size_label && view.show_size_label {
            self.draw_size_label(selection, view, metrics)?;
        }
        Ok(())
    }

    /// 填一块圆角矩形（面板、卡片、药丸都从这里走）。
    fn fill_round(&self, rect: Rect, radius: i32, brush: &ID2D1SolidColorBrush) {
        PanelPainter {
            context: &self.d2d,
            text_factory: &self.dwrite,
        }
        .fill_round(rect, radius, brush);
    }

    /// 描一块圆角矩形的边（发丝线、聚焦环、按钮轮廓）。
    fn stroke_round(&self, rect: Rect, radius: i32, brush: &ID2D1SolidColorBrush, width: f32) {
        PanelPainter {
            context: &self.d2d,
            text_factory: &self.dwrite,
        }
        .stroke_round(rect, radius, brush, width);
    }

    /// 一条线（两点之间）：图标与分隔线都用它。
    fn line(&self, from: Point, to: Point, brush: &ID2D1SolidColorBrush, width: f32) {
        PanelPainter {
            context: &self.d2d,
            text_factory: &self.dwrite,
        }
        .line((from.x, from.y), (to.x, to.y), brush, width);
    }

    /// L3.7: the custom-region panel (docs/23 §14.54).
    ///
    /// One translucent layer over the frozen screen: the screen's own mask blur is what shows through
    /// it, so the panel does not blur anything itself. Every rectangle comes from
    /// [`RegionPanelLayout`] — the same one the pointer hit-tests against — so what is drawn is what
    /// clicks.
    /// 面板底下的毛玻璃：把**未调暗**的帧模糊结果按面板的圆角矩形裁一块画上去（docs/23 §14.55）。
    ///
    /// 模糊源是 [`Self::blurred_frame`]：mask 那一路、info strip、guide、magnifier 都从它裁，它是纯粹
    /// 的帧模糊、**没有叠加遮罩暗度**，所以这里不会把暗度叠第二次。圆角裁剪用 D2D 的正规做法
    /// （`PushLayer` + `geometricMask` 的圆角矩形几何），不用直角 clip 糊过去。
    fn draw_panel_backdrop(&self, bounds: Rect, radius: i32) -> Result<(), String> {
        let Some(blurred) = self.blurred_frame.clone() else {
            // 没有模糊源（导出路径根本不建它）就只留那层半透明底：可读性不受影响，只是不毛玻璃。
            return Ok(());
        };
        let rounded = D2D1_ROUNDED_RECT {
            rect: to_d2d(bounds),
            radiusX: radius as f32,
            radiusY: radius as f32,
        };
        let factory = unsafe { self.d2d.GetFactory() }
            .map_err(|error| super::hresult("ID2D1DeviceContext::GetFactory", &error))?;
        let mask: ID2D1Geometry = unsafe { factory.CreateRoundedRectangleGeometry(&rounded) }
            .map_err(|error| super::hresult("CreateRoundedRectangleGeometry", &error))?
            .cast()
            .map_err(|error| super::hresult("cast to ID2D1Geometry", &error))?;
        let layer = unsafe { self.d2d.CreateLayer(None) }
            .map_err(|error| super::hresult("CreateLayer", &error))?;
        let parameters = D2D1_LAYER_PARAMETERS1 {
            contentBounds: to_d2d(bounds),
            geometricMask: std::mem::ManuallyDrop::new(Some(mask)),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: windows_numerics::Matrix3x2::identity(),
            opacity: 1.0,
            opacityBrush: std::mem::ManuallyDrop::new(None),
            layerOptions: Default::default(),
        };
        // 源与目标同一块矩形：模糊帧与后缓冲同一坐标系，1:1，不需要插值。
        let rect = to_d2d(bounds);
        unsafe {
            self.d2d.PushLayer(&parameters, &layer);
            self.d2d.DrawBitmap(
                &blurred,
                Some(&rect),
                1.0,
                D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                Some(&rect),
                None,
            );
            self.d2d.PopLayer();
        }
        let _ = std::mem::ManuallyDrop::into_inner(parameters.geometricMask);
        Ok(())
    }

    /// 图片卡（docs/23 §14.55.19）：底部常驻、整行卡头是开关；主色淡染把「出什么」与
    /// 输入区（中性灰）分开。收起时只画卡头与摘要，展开时多一行圆角滑块。
    fn draw_image_card(&mut self, panel: &RegionPanelView) -> Result<(), String> {
        let layout = &panel.layout;
        let card = layout.image_card;
        if card.height() <= 0 {
            return Ok(());
        }
        let k = self.metrics.dpi.max(96) as f32 / 96.0;
        let dip = |value: f32| (value * k).round() as i32;
        let out_fill = self.require_brush(&self.panel_out_fill_brush, "out fill brush")?;
        let out_line = self.require_brush(&self.panel_out_line_brush, "out line brush")?;
        let out_title = self.require_brush(&self.panel_out_title_brush, "out title brush")?;
        let accent = self.require_brush(&self.panel_accent_brush, "panel accent brush")?;
        let fill = self.require_brush(&self.panel_fill_brush, "panel fill brush")?;
        let muted = self.require_brush(&self.panel_muted_brush, "panel muted brush")?;
        let text = self.require_brush(&self.label_text_brush, "label text brush")?;

        let hovered = matches!(panel.hover, Some(RegionPanelTarget::Slot(PanelSlot::Image)));
        let focused = panel.focus == Some(PanelSlot::Image);
        let radius = dip(10.0);
        self.fill_round(card, radius, &out_fill);
        if hovered {
            self.fill_round(card, radius, &fill);
        }
        self.stroke_round(card, radius, &out_line, 1.0);
        if focused {
            let ring = Rect::new(
                card.left - dip(2.0),
                card.top - dip(2.0),
                card.right + dip(2.0),
                card.bottom + dip(2.0),
            );
            self.stroke_round(ring, radius + dip(2.0), &accent, 2.0);
        }

        let title = self.info_text_format_mut(false, 10.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, false)?;
        self.draw_text_in("图片", &title, to_d2d(layout.image_head), &out_title)?;

        // 收起时右侧是现状摘要；展开时摘要隐去（下面的行本来就在显示它）。
        if !panel.image_open {
            let format = self.info_text_format_mut(false, 11.0, DWRITE_FONT_WEIGHT_NORMAL, false)?;
            let width = self.measure_text_width(&panel.image_summary, &format)?.round() as i32;
            let summary = Rect::new(
                (layout.image_summary.right - width).max(layout.image_summary.left),
                layout.image_summary.top,
                layout.image_summary.right,
                layout.image_summary.bottom,
            );
            self.draw_text_in(&panel.image_summary, &format, to_d2d(summary), &muted)?;
        }

        // 折叠箭头：收起指向下、展开指向上。
        let chevron = layout.image_chevron;
        let (head_dy, tip_dy) = if panel.image_open { (2.0, -2.0) } else { (-2.0, 2.0) };
        let head = Point::new(chevron.left, chevron.center().y + dip(head_dy));
        let tip = Point::new(chevron.center().x, chevron.center().y + dip(tip_dy));
        let tail = Point::new(chevron.right, head.y);
        self.line(head, tip, &out_title, dip(2.0) as f32);
        self.line(tip, tail, &out_title, dip(2.0) as f32);

        if !panel.image_open {
            return Ok(());
        }

        let label = self.info_text_format_mut(false, 11.0, DWRITE_FONT_WEIGHT_NORMAL, false)?;
        self.draw_text_in("圆角", &label, to_d2d(layout.radius_label), &muted)?;
        // 轨道：已选部分主色、其余中性；滑块 14 的白圆。
        let groove = layout.radius_groove;
        let groove_radius = (groove.height().max(1) / 2).max(1);
        self.fill_round(groove, groove_radius, &fill);
        let filled = Rect::new(
            groove.left,
            groove.top,
            layout
                .radius_thumb
                .center()
                .x
                .max(groove.left + groove_radius),
            groove.bottom,
        );
        if filled.width() > 0 {
            self.fill_round(filled, groove_radius, &accent);
        }
        let thumb = layout.radius_thumb;
        let thumb_radius = thumb.width() as f32 / 2.0;
        let circle = D2D1_ELLIPSE {
            point: vector2(thumb.center().x as f32, thumb.center().y as f32),
            radiusX: thumb_radius,
            radiusY: thumb_radius,
        };
        unsafe { self.d2d.FillEllipse(&circle, &text) };
        let slider_focused = panel.focus == Some(PanelSlot::Radius);
        let slider_hovered = matches!(panel.hover, Some(RegionPanelTarget::Radius(_)));
        if slider_focused || slider_hovered {
            let out = dip(if slider_focused { 2.0 } else { 4.0 }) as f32;
            let halo = D2D1_ELLIPSE {
                point: circle.point,
                radiusX: thumb_radius + out,
                radiusY: thumb_radius + out,
            };
            let ring_brush = if slider_focused { &accent } else { &fill };
            unsafe { self.d2d.DrawEllipse(&halo, ring_brush, 2.0, None) };
        }

        // 值块：底 + 数字（0 时 muted）。角标那个小方框已经移除，只留数值（docs/23 §14.55.21）。
        let value = layout.radius_value;
        self.fill_round(value, dip(4.0), &fill);
        let number = value;
        let format = self.info_text_format_mut(true, 11.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, true)?;
        let number_brush = if panel.radius_px > 0 { &text } else { &muted };
        self.draw_text_in(&panel.radius_text, &format, to_d2d(number), number_brush)?;
        Ok(())
    }

    fn draw_region_panel(&mut self, panel: &RegionPanelView) -> Result<(), String> {
        let layout = &panel.layout;
        let k = self.metrics.dpi.max(96) as f32 / 96.0;
        let dip = |value: f32| (value * k).round() as i32;
        let surface = self.require_brush(&self.label_background_brush, "label background brush")?;
        let text = self.require_brush(&self.label_text_brush, "label text brush")?;
        let muted = self.require_brush(&self.panel_muted_brush, "panel muted brush")?;
        let line = self.require_brush(&self.panel_line_brush, "panel line brush")?;
        let fill = self.require_brush(&self.panel_fill_brush, "panel fill brush")?;
        let accent = self.require_brush(&self.border_brush, "border brush")?;
        let accent_fill = self.require_brush(&self.panel_accent_brush, "panel accent brush")?;
        let danger = self.require_brush(&self.panel_danger_brush, "panel danger brush")?;

        let big_radius = dip(12.0);
        // 先铺面板范围里的毛玻璃，再压那层半透明底色：文字对比度由底色保证（docs/23 §14.55）。
        self.draw_panel_backdrop(layout.panel, big_radius)?;
        self.fill_round(layout.panel, big_radius, &surface);
        self.stroke_round(layout.panel, big_radius, &line, 1.0);

        // ── 页头：标题 / 单位 / 倍率 / 关闭 ─────────────────────────────────
        let title_format = self.info_text_format_mut(false, 12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, false)?;
        self.draw_text_in("自定义截图", &title_format, to_d2d(layout.title), &text)?;

        // 容器由布局给出：两块的并集曾让它与选中块等大，选中背景因此贴着外框（docs/23 §14.55）。
        let unit_segments: Vec<(Rect, &str, bool)> = layout
            .unit
            .iter()
            .map(|(unit, rect)| (*rect, unit.segment_label(), *unit == panel.unit))
            .collect();
        let unit_hovered = layout
            .unit
            .iter()
            .position(|(unit, _)| panel.hover == Some(RegionPanelTarget::Unit(*unit)));
        self.draw_segmented(
            layout.unit_track,
            &unit_segments,
            unit_hovered,
            panel.focus == Some(PanelSlot::Unit),
            panel.unit_slide,
            &SegmentStyle {
                track: &fill,
                selected: &accent_fill,
                hover: &line,
                focus: &accent,
                text: &text,
                muted: &muted,
                mono: true,
                size_dip: 10.0,
                weight: DWRITE_FONT_WEIGHT_SEMI_BOLD,
            },
        )?;

        self.fill_round(layout.scale, dip(4.0), &fill);
        let scale_format = self.info_text_format_mut(true, 10.0, DWRITE_FONT_WEIGHT_NORMAL, true)?;
        self.draw_text_in(
            &format!("{}%", (k * 100.0).round() as i32),
            &scale_format,
            to_d2d(layout.scale),
            &muted,
        )?;

        // 关闭：两条线画一个叉。它和页脚的取消是同一件事（布局里也指向同一个槽）。
        let close_hover = panel.hover == Some(RegionPanelTarget::Slot(PanelSlot::Cancel));
        if close_hover {
            self.fill_round(layout.close, dip(6.0), &fill);
        }
        let close_brush = if close_hover { &text } else { &muted };
        self.draw_glyph(
            Glyph::Close,
            centered_square(layout.close, dip(PANEL_ICON_DIP)),
            close_brush,
            false,
        )?;

        // ── 两张卡 ────────────────────────────────────────────────────────
        let card_title_format =
            self.info_text_format_mut(false, 10.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, false)?;
        for (index, card) in layout.cards.into_iter().enumerate() {
            self.fill_round(card, dip(10.0), &fill);
            self.stroke_round(card, dip(10.0), &line, 1.0);
            let title = layout.card_titles[index];
            let title_box = Rect::new(title.left, title.top, title.left + dip(60.0), title.bottom);
            self.draw_text_in(
                if index == 0 { "尺寸" } else { "位置" },
                &card_title_format,
                to_d2d(title_box),
                &muted,
            )?;
        }
        // 比例按钮：住在尺寸卡头里，右对齐在交换图标左侧。常态无底，悬停/展开铺 5 圆角底；
        // 选了比例（非「无比例」）时值用主色，提示宽高被锁在一起（docs/23 §14.55.15）。
        let ratio_hover = panel.hover == Some(RegionPanelTarget::Slot(PanelSlot::Ratio));
        let menu_expanded = layout.menu.is_some();
        if ratio_hover || menu_expanded {
            self.fill_round(layout.ratio, dip(5.0), &fill);
        }
        if panel.focus == Some(PanelSlot::Ratio) {
            self.stroke_round(layout.ratio, dip(5.0), &accent, 1.5);
        }
        let ratio_format = self.info_text_format_mut(false, 12.0, DWRITE_FONT_WEIGHT_NORMAL, false)?;
        let ratio_box = Rect::new(
            layout.ratio.left + dip(7.0),
            layout.ratio.top,
            layout.ratio.right - dip(18.0),
            layout.ratio.bottom,
        );
        let ratio_brush = if panel.locked { &accent } else { &muted };
        self.draw_text_in(&panel.ratio_label, &ratio_format, to_d2d(ratio_box), ratio_brush)?;
        self.draw_glyph(
            Glyph::Chevron,
            centered_square_at(
                layout.ratio.right - dip(12.0),
                layout.ratio.top + layout.ratio.height() / 2,
                dip(PANEL_ICON_DIP),
            ),
            &muted,
            menu_expanded,
        )?;

        for (slot, rect) in layout.actions {
            let hovered = panel.hover == Some(RegionPanelTarget::Slot(slot));
            if hovered {
                self.fill_round(rect, dip(5.0), &fill);
            }
            if panel.focus == Some(slot) {
                self.stroke_round(rect, dip(5.0), &accent, 2.0);
            }
            if let Some(icon) = action_icon(slot) {
                self.draw_glyph(icon, centered_square(rect, dip(PANEL_ICON_DIP)), &muted, false)?;
            }
        }

        // ── 四个数字：下划线是它的边，聚焦转主色、非法转红 ────────────────
        let number_format = self.info_text_format_mut(true, 20.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, true)?;
        let label_format = self.info_text_format_mut(false, 10.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, true)?;
        for (index, (field, rect)) in layout.fields.iter().enumerate() {
            let invalid = panel.invalid[index];
            let focused = panel.focus == Some(field.slot());
            let underline = if invalid {
                &danger
            } else if focused {
                &accent
            } else {
                &line
            };
            let underline_y = rect.bottom;
            self.line(
                Point::new(rect.left, underline_y),
                Point::new(rect.right, underline_y),
                underline,
                if focused || invalid { 2.0 } else { 1.0 },
            );
            let value = panel
                .values
                .iter()
                .find(|(item, _)| item == field)
                .map(|(_, value)| value.as_str())
                .unwrap_or("");
            self.draw_text_in(value, &number_format, to_d2d(*rect), &text)?;
            // 插入光标：数字是**居中**画的，所以文字左端 = 格子中心 - 总宽/2，插入点 = 文字左端 +
            // **光标左侧前缀**的推进宽度（不是"总宽/2"——那样退格到中间就会画错位置）。
            // 可见性由 `caret_visible` 决定，于是光标能闪（docs/23 §14.55）。
            if panel.editing == Some(*field) && panel.caret_visible {
                let full = self.measure_text_width(value, &number_format)?;
                let prefix: String = value.chars().take(panel.caret).collect();
                let before = self.measure_text_width(&prefix, &number_format)?;
                let text_left = rect.left + rect.width() / 2 - (full / 2.0).ceil() as i32;
                let caret_x = text_left + before.ceil() as i32;
                let center_y = rect.top + rect.height() / 2;
                // 加长到与数字视觉高度相当（20 dip 字号 ⇒ 约 23 dip），加粗到 3 px 并随 DPI 缩放。
                let half = dip(CARET_HALF_DIP);
                self.line(
                    Point::new(caret_x, center_y - half),
                    Point::new(caret_x, center_y + half),
                    &accent,
                    (3.0 * k).max(1.0),
                );
            }
            let label = layout.field_labels[index];
            let label_brush = if invalid {
                &danger
            } else if focused {
                &accent
            } else {
                &muted
            };
            self.draw_text_in(field.label(), &label_format, to_d2d(label), label_brush)?;
        }
        // 分隔：尺寸卡是 `×`（宽 × 高），位置卡是一条竖线。
        let sep_format = self.info_text_format_mut(
            true,
            18.0,
            ::windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_BOLD,
            true,
        )?;
        // 锁了比例时 `×` 转主色：这两个数不再独立（docs/23 §14.55.15）。
        let times_brush = if panel.locked { &accent } else { &muted };
        self.draw_text_in("×", &sep_format, to_d2d(layout.separators[0]), times_brush)?;
        let mid = (layout.separators[1].left + layout.separators[1].right) / 2;
        self.line(
            Point::new(mid, layout.separators[1].top + dip(4.0)),
            Point::new(mid, layout.separators[1].bottom - dip(4.0)),
            &line,
            1.0,
        );

        // ── 去向行：标签 + 复制/贴图（比例按钮已搬进尺寸卡头）+ 延时四段 ─────
        let row_label_format =
            self.info_text_format_mut(false, 10.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, false)?;
        self.draw_text_in("去向", &row_label_format, to_d2d(layout.destination_label), &muted)?;
        self.draw_text_in("角标", &row_label_format, to_d2d(layout.toggle_label), &muted)?;
        self.draw_text_in("延时", &row_label_format, to_d2d(layout.delay_label), &muted)?;

        // 去向与单位、延时共用同一份分段器绘制（docs/23 §14.55.4）。
        let destination_segments: Vec<(Rect, &str, bool)> = layout
            .destination
            .iter()
            .map(|(destination, rect)| (*rect, destination.label(), *destination == panel.destination))
            .collect();
        let destination_hovered = layout
            .destination
            .iter()
            .position(|(destination, _)| {
                panel.hover == Some(RegionPanelTarget::Destination(*destination))
            });
        self.draw_segmented(
            layout.destination_track,
            &destination_segments,
            destination_hovered,
            panel.focus == Some(PanelSlot::Destination),
            None,
            &SegmentStyle {
                track: &fill,
                selected: &accent_fill,
                hover: &line,
                focus: &accent,
                text: &text,
                muted: &muted,
                mono: false,
                size_dip: 11.0,
                weight: DWRITE_FONT_WEIGHT_NORMAL,
            },
        )?;

        // 与单位分段器共用一份绘制（docs/23 §14.55.4）。
        let delay_segments: Vec<(Rect, &str, bool)> = layout
            .delay
            .iter()
            .map(|(delay, rect)| (*rect, delay.label(), *delay == panel.delay))
            .collect();
        let delay_hovered = layout
            .delay
            .iter()
            .position(|(delay, _)| panel.hover == Some(RegionPanelTarget::Delay(*delay)));
        self.draw_segmented(
            layout.delay_track,
            &delay_segments,
            delay_hovered,
            panel.focus == Some(PanelSlot::Delay),
            panel.delay_slide,
            &SegmentStyle {
                track: &fill,
                selected: &accent_fill,
                hover: &line,
                focus: &accent,
                text: &text,
                muted: &muted,
                mono: false,
                size_dip: 11.0,
                weight: DWRITE_FONT_WEIGHT_NORMAL,
            },
        )?;

        // ── 角标行：三个可选标签 + 角标预览 ───────────────────────────────
        let toggle_format = self.info_text_format_mut(false, 11.0, DWRITE_FONT_WEIGHT_NORMAL, true)?;
        for (toggle, rect) in layout.toggles {
            let on = panel
                .toggles
                .iter()
                .find(|(item, _)| *item == toggle)
                .map(|(_, on)| *on)
                .unwrap_or(false);
            let hovered = panel.hover == Some(RegionPanelTarget::Slot(toggle.slot()));
            Tag::new(rect).paint(
                &PanelPainter {
                    context: &self.d2d,
                    text_factory: &self.dwrite,
                },
                TagState {
                    selected: on,
                    interaction: snapclip_ui::panel::InteractionState {
                        hovered,
                        focused: panel.focus == Some(toggle.slot()),
                        ..Default::default()
                    },
                },
                TagColors {
                    off_fill: &fill,
                    off_hover: &line,
                    on_fill: &accent_fill,
                    line: &line,
                    on_line: &accent,
                    focus: &accent,
                    disabled: &muted,
                },
            );
            let brush = if on { &text } else { &muted };
            self.draw_text_in(toggle.label(), &toggle_format, to_d2d(rect), brush)?;
        }
        // 预览标签也是胶囊：与角标、方向芯片共用同一套半径算法。

        // ── 页脚：一句话 + 取消 / 确定 ─────────────────────────────────
        let status_format = self.info_text_format_mut(false, 11.0, DWRITE_FONT_WEIGHT_NORMAL, false)?;
        match panel.status.as_deref() {
            // 有校验错误：红字占这个槽（读数让位）。
            Some(status) => self.draw_text_in(status, &status_format, to_d2d(layout.status), &danger)?,
            // 没有错误：这一张会去哪、出多大。
            None => self.draw_text_in(
                &panel.readout,
                &status_format,
                to_d2d(layout.status),
                &muted,
            )?,
        }
        let cancel_hover = panel.hover == Some(RegionPanelTarget::Slot(PanelSlot::Cancel));
        if cancel_hover {
            self.fill_round(layout.cancel, dip(6.0), &fill);
        }
        self.stroke_round(
            layout.cancel,
            dip(6.0),
            if cancel_hover { &accent } else { &line },
            1.0,
        );
        if panel.focus == Some(PanelSlot::Cancel) {
            self.stroke_round(layout.cancel, dip(6.0), &accent, 2.0);
        }
        let button_format = self.info_text_format_mut(false, 12.0, DWRITE_FONT_WEIGHT_NORMAL, true)?;
        self.draw_image_card(panel)?;
        self.draw_text_in("取消", &button_format, to_d2d(layout.cancel), &text)?;

        let start_hover = panel.hover == Some(RegionPanelTarget::Slot(PanelSlot::Start));
        let start_brush = if panel.can_start { &accent_fill } else { &fill };
        self.fill_round(layout.start, dip(6.0), start_brush);
        if start_hover && panel.can_start {
            // 主按钮已经是主题色：悬停时加一圈亮边，而不是再造一个更亮的蓝。
            self.stroke_round(layout.start, dip(6.0), &line, 1.5);
        }
        if panel.focus == Some(PanelSlot::Start) {
            self.stroke_round(layout.start, dip(6.0), &accent, 2.0);
        }
        let start_format = self.info_text_format_mut(false, 12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, true)?;
        let start_text_brush = if panel.can_start { &text } else { &muted };
        // 按钮文案随去向走（复制 / 贴图），仍是同一个 `PanelSlot::Start`。
        self.draw_text_in(
            panel.destination.label(),
            &start_format,
            to_d2d(layout.start),
            start_text_brush,
        )?;

        // ── 下拉：挂在触发器下面，压着下面的行 ─────────────────────────────
        if let Some(menu) = layout.menu {
            self.fill_round(menu, dip(10.0), &surface);
            self.stroke_round(menu, dip(10.0), &line, 1.0);
            let name_format = self.info_text_format_mut(false, 12.0, DWRITE_FONT_WEIGHT_NORMAL, false)?;
            for (index, item) in layout.menu_items.iter().enumerate() {
                let selected = panel.ratio_selected == index;
                let hovered = panel.hover == Some(RegionPanelTarget::MenuItem(index));
                if selected || hovered {
                    let inset = dip(3.0);
                    self.fill_round(
                        Rect::new(item.left + inset, item.top, item.right - inset, item.bottom),
                        dip(6.0),
                        if selected { &accent_fill } else { &fill },
                    );
                }
                let label = Ratio::label(Ratio::at_menu(index));
                let check_x = item.left + dip(14.0);
                if selected {
                    let mid_y = item.top + item.height() / 2;
                    self.draw_glyph(
                        Glyph::Check,
                        centered_square_at(check_x, mid_y, dip(PANEL_ICON_DIP)),
                        &text,
                        false,
                    )?;
                }
                let name_box = Rect::new(
                    item.left + dip(26.0),
                    item.top,
                    item.right - dip(10.0),
                    item.bottom,
                );
                self.draw_text_in(label, &name_format, to_d2d(name_box), &text)?;
            }
        }
        Ok(())
    }

    /// 画一个分段器：容器底 + 四边内缩的选中块 + 每段文字 + 聚焦环。
    ///
    /// 圆角走**嵌套规则**（外层 [`segment_outer_radius`]，内层减掉实际内缩），所以是圆角矩形、
    /// 不是胶囊；指到哪一段就亮哪一段（两处分段器同一个语义）。单位与延时都调用这里
    /// （docs/23 §14.55.4）。
    fn draw_segmented(
        &mut self,
        track: Rect,
        segments: &[(Rect, &str, bool)],
        hovered: Option<usize>,
        focused: bool,
        slide: Option<(usize, f32)>,
        style: &SegmentStyle<'_>,
    ) -> Result<(), String> {
        let scale = self.metrics.dpi.max(96) as f32 / 96.0;
        let format = self.info_text_format_mut(style.mono, style.size_dip, style.weight, true)?;
        let state = SegmentState {
            track,
            segments: segments
                .iter()
                .map(|(rect, _, selected)| Segment {
                    rect: *rect,
                    selected: *selected,
                    disabled: false,
                })
                .collect(),
            hovered,
            pressed: None,
            focused,
            slide,
        };
        let labels: Vec<&str> = segments.iter().map(|(_, label, _)| *label).collect();
        PanelPainter {
            context: &self.d2d,
            text_factory: &self.dwrite,
        }
        .segmented(
            &state,
            &labels,
            &format,
            SegmentColors {
                track: style.track,
                selected: style.selected,
                hover: style.hover,
                focus: style.focus,
                text: style.text,
                muted: style.muted,
            },
            scale,
        );
        Ok(())
    }

    /// 面板的图形：`assets/icon` 的 PNG，按当前状态色画到 `rect` 里。
    ///
    /// 图是 `icon-bake` 用 resvg 从设计给的 SVG 烘出来的（docs/23 §14.54），所以它就是设计稿那张图；
    /// 着色与缩放分别由颜色矩阵与目标矩形负责，不需要变换。
    fn draw_glyph(
        &mut self,
        glyph: Glyph,
        rect: Rect,
        brush: &ID2D1SolidColorBrush,
        flip_v: bool,
    ) -> Result<(), String> {
        let colour = unsafe { brush.GetColor() };
        self.panel_icons
            .draw(&self.d2d, glyph, rect, colour, flip_v)
    }

    /// 一段文字在某个格式下的推进宽度（px）——插入光标要落在文字右端（docs/23 §14.55）。
    fn measure_text_width(
        &mut self,
        text: &str,
        format: &IDWriteTextFormat,
    ) -> Result<f32, String> {
        PanelPainter {
            context: &self.d2d,
            text_factory: &self.dwrite,
        }
        .measure_text_width(text, format)
        .map_err(|error| super::hresult("IDWriteTextLayout::GetMetrics", &error))
    }

    fn draw_size_label(
        &mut self,
        selection: Rect,
        view: &RenderView,
        metrics: RenderMetrics,
    ) -> Result<(), String> {
        // The badge is the one control in the overlay that answers to the pointer rather than to the
        // keymap, so it says so: the pointer on it fills it with the interface accent (the same brush
        // the frame's outline uses) while the white text stays as it is — the pairing every "on" state
        // in the overlay already uses (docs/23 §14.53).
        let background = if view.label_hovered {
            self.require_brush(&self.border_brush, "border brush")?
        } else {
            self.require_brush(&self.label_background_brush, "label background brush")?
        };
        let foreground = self.require_brush(&self.label_text_brush, "label text brush")?;
        let text = view.label.text(selection, view.screen_origin, metrics.dpi);
        if text.is_empty() {
            // 三项都关掉：标签没有说话可说，不画比画一个空盒子诚实（docs/23 §14.54）。
            return Ok(());
        }
        // Measure the exact glyph run before choosing a placement. A character-count
        // estimate is wrong for CJK, signs and proportional digits and can select a
        // different ladder step, causing the label to jump while the rectangle moves.
        let (measured, format) = self.measure_label(&text)?;
        let Some(placement) = self.place_label(selection, view, measured, metrics) else {
            // No room beside the selection: dropping the label beats covering the
            // pixels the user is choosing.
            return Ok(());
        };

        {
            // 胶囊：圆角取高度的一半（`geometry::pill_radius`）——三处芯片共用同一个算法。
            let radius = pill_radius(placement.rect);
            let painter = PanelPainter {
                context: &self.d2d,
                text_factory: &self.dwrite,
            };
            painter.fill_round(placement.rect, radius.round() as i32, &background);
            let text_rect = D2D_RECT_F {
                left: placement.rect.left as f32 + metrics.label_padding_x,
                top: placement.rect.top as f32 + metrics.label_padding_y,
                right: placement.rect.right as f32 - metrics.label_padding_x,
                bottom: placement.rect.bottom as f32 - metrics.label_padding_y,
            };
            painter.draw_text_rect(&text, &format, text_rect, &foreground);
        }
        Ok(())
    }

    /// Where the size label will be painted, in back-buffer coordinates, or `None` when this state
    /// has no label to place.
    ///
    /// The paint path and the pointer path both go through this, so the badge the user aims at is
    /// the badge the user sees — the pointer's hover test runs against this rectangle, and a click on
    /// it opens the custom-region panel (docs/23 §14.54). Measuring with the same DirectWrite layout
    /// the paint uses is what keeps a change of unit or of the origin from moving the badge away from
    /// the rectangle the hit test already answered for.
    pub fn size_label_bounds(&mut self, view: &RenderView) -> Result<Option<Rect>, String> {
        if view.selection.is_empty() {
            return Ok(None);
        }
        let text = view
            .label
            .text(view.selection, view.screen_origin, self.metrics.dpi);
        if text.is_empty() {
            // 三项都关掉：没有徽标可画，也就没有徽标菜单（docs/23 §14.54）。
            return Ok(None);
        }
        let (measured, _format) = self.measure_label(&text)?;
        Ok(self
            .place_label(view.selection, view, measured, self.metrics)
            .map(|placement| placement.rect))
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
        let primary = crate::geometry::size_label_placement(
            selection,
            size,
            work_area,
            metrics.label_gap.round() as i32,
            Some(view.cursor),
        )?;

        // The label and loupe are both secondary chrome. When their default placements
        // collide, keep the loupe attached to the sampling cursor and move the label to
        // the next valid ladder rung instead of letting one cover the other.
        let avoid = loupe_obstacle(view, metrics);
        let overlaps = |rect: Rect| {
            avoid.is_some_and(|obstacle| !rect.intersect(obstacle).is_empty())
        };
        if !overlaps(primary.rect) {
            return Some(primary);
        }

        let (width, height) = (size.0.max(1), size.1.max(1));
        let gap = metrics.label_gap.round() as i32;
        let clamp = |left: i32, top: i32, where_: LabelWhere| {
            let max_left = (work_area.right - width).max(work_area.left);
            let max_top = (work_area.bottom - height).max(work_area.top);
            let rect = Rect::from_origin_size(
                Point::new(left.clamp(work_area.left, max_left), top.clamp(work_area.top, max_top)),
                width,
                height,
            );
            (!overlaps(rect)).then_some(SizeLabelPlacement { rect, where_ })
        };
        let above = selection.top - gap - height;
        if above >= work_area.top {
            if let Some(candidate) = clamp(selection.left, above, LabelWhere::Above) {
                return Some(candidate);
            }
        }
        let below = selection.bottom + gap;
        if below + height <= work_area.bottom {
            if let Some(candidate) = clamp(selection.left, below, LabelWhere::Below) {
                return Some(candidate);
            }
        }
        if width + 2 * gap <= selection.width() && height + 2 * gap <= selection.height() {
            if let Some(candidate) = clamp(
                selection.left + gap,
                selection.top + gap,
                LabelWhere::Inside,
            ) {
                return Some(candidate);
            }
        }
        None
    }

    // ── Scroll capture panel (docs/33) ──────────────────────────────────────────────────────────

    /// The scroll session's panel (the demo's `drawScrolling`): a shadowed square card holding the
    /// thumbnail's tail in a full-bleed well with the viewport outlined and the image's size on a
    /// floating badge, then the status line and the buttons. Every rectangle comes from [`Layout`],
    /// which the hit test and the window region use too.
    fn draw_scroll_panel(&mut self, scroll: &ScrollPanelView) -> Result<(), String> {
        let k = self.metrics.dpi.max(96) as f32 / 96.0;
        let backdrop = self.require_brush(&self.scroll_panel_brush, "scroll panel brush")?;
        let accent = self.require_brush(&self.scroll_box_brush, "scroll box brush")?;
        let shadow = self.require_brush(&self.scroll_shadow_brush, "scroll shadow brush")?;
        let title = self.require_brush(&self.scroll_title_brush, "scroll title brush")?;
        let muted = self.require_brush(&self.scroll_muted_brush, "scroll muted brush")?;
        let button = self.require_brush(&self.scroll_button_brush, "scroll button brush")?;
        let badge_background = self.require_brush(&self.scroll_badge_brush, "scroll badge brush")?;
        let fade = self
            .scroll_fade_brush
            .clone()
            .ok_or("scroll fade brush is unavailable")?;
        let layout = &scroll.layout;

        // The card: a soft shadow (three stacked layers, inside the layout's footprint), then the
        // card itself. Both are crisp rectangles — the panel has square corners.
        unsafe {
            for grow in [1.5, 3.0, 4.0] {
                let rect = layout.panel.inflate((grow * k) as i32);
                self.d2d.FillRectangle(
                    &D2D_RECT_F {
                        top: rect.top as f32 + 2.0 * k,
                        ..to_d2d(rect)
                    },
                    &shadow,
                );
            }
            self.d2d.FillRectangle(&to_d2d(layout.panel), &backdrop);
        }

        // The well: the thumbnail's newest rows, clipped to the well's rectangle. It fills the panel
        // down to the footer, so the image starts at the panel's own top edge.
        self.draw_scroll_well(scroll, k, &accent, &fade)?;

        // The size badge floats on the thumbnail's top-right corner, the way the loupe's `20×`
        // badge floats on the magnified pixels.
        let size_format =
            self.info_text_format_mut(false, 11.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, false)?;
        let size_width = self.measure_text_width_in(&scroll.size, &size_format)?;
        let badge = Rect::new(
            layout.badge.right - size_width.ceil() as i32 - (SCROLL_BADGE_PAD_DIP * k) as i32,
            layout.badge.top,
            layout.badge.right,
            layout.badge.bottom,
        );
        unsafe {
            // Square corners like everything else on the panel.
            self.d2d.FillRectangle(&to_d2d(layout.badge), &badge_background);
        }
        self.draw_text_in(&scroll.size, &size_format, to_d2d(badge), &title)?;

        // The status line and the buttons.
        let small = self.info_text_format_mut(false, 11.0, DWRITE_FONT_WEIGHT_NORMAL, false)?;
        let status = to_d2d(layout.status);
        self.draw_text_in(
            &scroll.status,
            &small,
            D2D_RECT_F {
                left: status.left,
                ..status
            },
            &muted,
        )?;
        for (rect, label) in layout.buttons.iter().zip(&scroll.buttons) {
            unsafe {
                self.d2d.FillRectangle(&to_d2d(*rect), &button);
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
        // A square clip: the well has the panel's own corners.
        let mask: ID2D1Geometry = unsafe { factory.CreateRectangleGeometry(&to_d2d(well)) }
            .map_err(|e| super::hresult("CreateRectangleGeometry", &e))?
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
        PanelPainter {
            context: &self.d2d,
            text_factory: &self.dwrite,
        }
        .draw_text_rect(text, format, rect, brush);
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
        let text_metrics = PanelPainter {
            context: &self.d2d,
            text_factory: &self.dwrite,
        }
        .measure_text(text, &format)
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
        let fs = item.style.font_size;
        let approx_w = (content.chars().count() as f32 * fs * 0.55).max(10.0);
        let approx_h = fs * 2.0;
        let text_rect = D2D_RECT_F {
            left: position.x as f32,
            top: position.y as f32,
            right: position.x as f32 + approx_w,
            bottom: position.y as f32 + approx_h,
        };
        PanelPainter {
            context: &self.d2d,
            text_factory: &self.dwrite,
        }
        .draw_text_rect(content, &format, text_rect, stroke);
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
        for family in overlay_font::FAMILIES {
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

    /// Decode the baked glyph PNGs and pre-tint them for every colour the panel draws them in.
    ///
    /// **Must be called outside a frame**: each tint render needs its own `BeginDraw`/`EndDraw`, and
    /// Direct2D forbids nesting those inside the frame's draw pass (see `d2d::panel_icon`). The
    /// controller calls it before the frame that shows the panel, so the 1.6 ms is paid when the user
    /// opens the panel instead of on the session-start path.
    ///
    /// The colour list comes from the panel's own brushes, so the palette cannot drift from what
    /// `draw_glyph` later asks for.
    /// 量出两处分段器**最宽选项**的宽度（物理像素），供布局给每段同宽用。
    ///
    /// 量宽用的字体/字号/字重必须与绘制**完全一致**（这里刻意与 `draw_region_panel` 里那两处
    /// `info_text_format_mut` 写成同一份参数），否则量出来的宽度和画出来的文字对不上。
    /// 每段各留 `REGION_SEGMENT_PAD_DIP` 的左右内边距，文字在各段内居中（docs/23 §14.55.6）。
    pub(crate) fn segment_widths(&mut self) -> Result<SegmentWidths, String> {
        let scale = self.metrics.dpi.max(96) as f32 / 96.0;
        let pad = REGION_SEGMENT_PAD_DIP * scale;
        let mut unit = 0.0f32;
        let unit_format =
            self.info_text_format_mut(true, 10.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, true)?;
        for label in LengthUnit::ALL.map(|unit| unit.segment_label()) {
            unit = unit.max(self.measure_text_width(label, &unit_format)?);
        }
        let mut delay = 0.0f32;
        let delay_format =
            self.info_text_format_mut(false, 11.0, DWRITE_FONT_WEIGHT_NORMAL, true)?;
        for label in Delay::ALL.map(|delay| delay.label()) {
            delay = delay.max(self.measure_text_width(label, &delay_format)?);
        }
        let mut destination = 0.0f32;
        for label in Destination::ALL.map(|destination| destination.label()) {
            destination = destination.max(self.measure_text_width(label, &delay_format)?);
        }
        // 比例按钮：文字 12 NORMAL + 箭头位 18 + 两侧内边距 7/7 —— 与 `draw_region_panel` 里
        // 那处 `info_text_format_mut(false, 12.0, NORMAL, false)` 是同一份参数。
        let ratio_format =
            self.info_text_format_mut(false, 12.0, DWRITE_FONT_WEIGHT_NORMAL, false)?;
        let mut ratio_label = 0.0f32;
        for label in std::iter::once(RATIO_FREE_LABEL).chain(RATIOS.iter().map(|ratio| ratio.name)) {
            ratio_label = ratio_label.max(self.measure_text_width(label, &ratio_format)?);
        }
        // 菜单：最宽项 + 26 的检查位 + 10 的右内边距 + 5×2 的菜单内边距（与绘制同一个算式）。
        let ratio_menu = ratio_label + 26.0 * scale + 10.0 * scale + 10.0 * scale;
        Ok(SegmentWidths {
            unit: (unit + 2.0 * pad).ceil() as i32,
            delay: (delay + 2.0 * pad).ceil() as i32,
            destination: (destination + 2.0 * pad).ceil() as i32,
            // 按钮 = 文字 + 箭头位 18 + 左右内边距 7/7。
            ratio_label: (ratio_label + 32.0 * scale).ceil() as i32,
            ratio_menu: ratio_menu.ceil() as i32,
        })
    }

    pub(crate) fn prepare_panel_icons(&mut self) -> Result<(), String> {
        let colours: Vec<D2D1_COLOR_F> = [&self.label_text_brush, &self.panel_muted_brush]
            .into_iter()
            .filter_map(|brush| brush.as_ref().map(|brush| unsafe { brush.GetColor() }))
            .collect();
        self.panel_icons.prepare(&self.d2d, &colours)
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
        let panel_theme = PanelTheme::DARK_OVERLAY;
        let panel_color = |rgba: [f32; 4]| color(rgba[0], rgba[1], rgba[2], rgba[3]);
        let crosshair = color(1.0, 0.75, 0.0, 0.95);
        // Dark charcoal grid lines: keep them legible when the magnified
        // content is light (white-on-white was invisible on pale screens).
        let grid = color(0.12, 0.12, 0.16, 0.55);
        let info = color(0.02, 0.02, 0.025, 0.94);
        self.mask_brush = Some(self.create_brush(&mask)?);
        self.border_brush = Some(self.create_brush(&accent)?);
        // 倒计时数字：主题色、80% 透明度。用 ACCENT_RGB 的同一个来源，不另立颜色。
        self.countdown_brush = Some(self.create_brush(&color_of(ACCENT_RGB, 0.8))?);
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
        // The custom-region panel: the accent marks whatever is currently selected or on, and the
        // muted white carries the labels the user is not editing (docs/23 §14.54).
        self.panel_fill_brush = Some(self.create_brush(&panel_color(panel_theme.fill))?);
        self.panel_line_brush = Some(self.create_brush(&panel_color(panel_theme.line))?);
        self.panel_accent_brush = Some(self.create_brush(&panel_color(panel_theme.accent))?);
        self.panel_muted_brush = Some(self.create_brush(&panel_color(panel_theme.muted))?);
        self.panel_out_fill_brush = Some(self.create_brush(&color_of(ACCENT_RGB, 0.14))?);
        self.panel_out_line_brush = Some(self.create_brush(&color_of(ACCENT_RGB, 0.60))?);
        self.panel_out_title_brush = Some(self.create_brush(&rgba(124, 180, 243, 1.0))?);
        self.panel_danger_brush = Some(self.create_brush(&panel_color(panel_theme.danger))?);
        // The panel's glyphs are *not* prepared here. Decoding and pre-tinting them costs a
        // measurable 1.6 ms, and this runs on the session-start path (F5 → overlay visible), so the
        // work moved to `prepare_panel_icons`, which the controller calls just before the frame that
        // shows the panel. A device rebuild still drops the palette here; the next ask re-does it.
        self.panel_icons.reset();
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
        self.scroll_badge_brush = Some(self.create_brush(&color(0.0, 0.0, 0.0, SCROLL_BADGE_OPACITY))?);
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
        self.countdown_brush = None;
        self.mask_brush = None;
        self.mask_ring = None;
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
        self.panel_fill_brush = None;
        self.panel_line_brush = None;
        self.panel_accent_brush = None;
        self.panel_muted_brush = None;
        self.panel_out_fill_brush = None;
        self.panel_out_line_brush = None;
        self.panel_out_title_brush = None;
        self.panel_danger_brush = None;
        self.panel_icons.reset();
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
        self.scroll_badge_brush = None;
        self.scroll_fade_brush = None;
        self.scroll_preview_bitmap = None;
    }
}

/// Whether a draw of `view` crops from the blurred frame: the blurred mask, the guide, or the
/// magnifier's info strip.
fn needs_backdrop(view: &RenderView) -> bool {
    view.scroll.is_none()
        && (view.mask_blur
            || view.guide.is_some()
            // 面板自己要有毛玻璃（docs/23 §14.55），所以它开着就得有模糊源。
            || view.region_panel.is_some()
            || (view.show_chrome && view.cursor_visible))
}

use helpers::*;
use panel_icon::PanelIcons;

/// 面板图形画多大：方形边长（dip）。
const PANEL_ICON_DIP: f32 = 14.0;

/// 延时倒计时数字的字号（dip）。选 96：在 1080p 上约一屏高的十分之一，隔着房间也看得见，
/// 又不至于在小选区里被裁掉（放不下时 D2D 只是居中裁切，不会崩）。
const COUNTDOWN_DIP: f32 = 96.0;

/// 插入光标的高度一半（dip）：数字是 20 dip，视觉高度约 15 dip，所以上下各 7.5。
const CARET_HALF_DIP: f32 = 11.5;

/// 分段器共用的一份画刷 + 标签字体。
///
/// 单位（PX/DP，等宽小号）与延时（正文）只有字体与段数不同，其余完全一样；抽成一份，
/// 两处就不会各自漂移（docs/23 §14.55.4）。
struct SegmentStyle<'a> {
    track: &'a ID2D1SolidColorBrush,
    selected: &'a ID2D1SolidColorBrush,
    hover: &'a ID2D1SolidColorBrush,
    focus: &'a ID2D1SolidColorBrush,
    text: &'a ID2D1SolidColorBrush,
    muted: &'a ID2D1SolidColorBrush,
    mono: bool,
    size_dip: f32,
    weight: DWRITE_FONT_WEIGHT,
}

/// 组内动作 → 图标；锁定那一格按状态换成开/关两把锁。
fn action_icon(slot: PanelSlot) -> Option<Glyph> {
    match slot {
        PanelSlot::Swap => Some(Glyph::Swap),
        PanelSlot::Center => Some(Glyph::Center),
        PanelSlot::Origin => Some(Glyph::Origin),
        _ => None,
    }
}

/// `rect` 里居中的正方形：图标的外接框（不随槽的宽高比变形）。
fn centered_square(rect: Rect, size: i32) -> Rect {
    centered_square_at(
        rect.left + rect.width() / 2,
        rect.top + rect.height() / 2,
        size,
    )
}

fn centered_square_at(cx: i32, cy: i32, size: i32) -> Rect {
    let half = size / 2;
    Rect::new(cx - half, cy - half, cx - half + size, cy - half + size)
}

mod guide_pass;
mod helpers;
mod magnifier_pass;
mod panel_icon;

#[cfg(test)]
mod tests;
