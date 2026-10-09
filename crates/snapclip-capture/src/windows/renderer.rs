//! Overlay renderer: owns the D3D11 device, the DirectComposition swap chain and
//! the Direct2D draw calls.
//!
//! One device and one swap chain live for as long as the overlay window does. When
//! the overlay moves to a monitor with different dimensions only the swap chain and
//! its render target are recreated.
//!
//! The frozen frame is uploaded as an L0 bitmap for one session. The renderer is
//! destroyed when that session ends, so no selection or captured pixels survive into
//! the next F5 invocation.

use std::sync::Arc;

use crate::annotation::{AnnotationDocument, AnnotationId, AnnotationItem};
use crate::geometry::{LevelReach, MonitorLayout, Point, Rect};

use super::providers::FrozenFrame;
use super::win::d2d::{
    ChainRingView, OverlayRenderer as D2dRenderer, RenderMetrics, RenderView,
};
use super::win::d3d11::{AsyncSampleBuffer, CompositionTarget, GraphicsDevice};
use ::windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

/// Everything the overlay needs to draw one frame.
#[derive(Debug, Clone)]
pub struct OverlayFrameState {
    pub selection: Rect,
    pub cursor: Point,
    pub cursor_visible: bool,
    pub show_chrome: bool,
    /// Current color sample RGB from the magnifier sampler.
    pub magnifier_rgb: Option<(u8, u8, u8)>,
    /// The sampled colour rendered in the current `ColorFormat` (`#RRGGBB` /
    /// `rgb(...)` / `hsl(...)`), shown in the info panel's primary slot.
    /// `S` cycles the format; there is no secondary slot any more.
    pub magnifier_color_text: Option<String>,
    /// Info-panel coordinate is relative to the selection origin (P toggle).
    pub magnifier_relative: bool,
    /// Current loupe zoom (`Z` + wheel). `0.1..=40.0`.
    pub magnifier_zoom: f32,
    // ── Annotations ───────────────────────────────────────────────────────────
    /// Committed annotation items to render at L2 (after mask, before chrome).
    pub annotation_items: Vec<AnnotationItem>,
    /// Id of the currently selected annotation (shows selection box).
    pub annotation_selected_id: Option<AnnotationId>,
    /// Item currently being drawn (not yet committed).
    pub annotation_draft: Option<AnnotationItem>,
    /// Window under the cursor, in monitor-local coordinates (paint-only hint).
    pub hover_bounds: Option<Rect>,
    /// Automatic-snap preview, in monitor-local coordinates.
    pub preview_bounds: Option<Rect>,
    /// Deep-selection ancestor levels (frame → deepest), monitor-local.
    /// Which rings those are is decided by `chain_rings` (docs/21 §5.22); this is the paint-ready
    /// form, monitor-local, with the selected level already removed (the preview paints it).
    pub chain_rings: Vec<ChainRingView>,
    /// Text drawn beside the automatic-snap preview (docs/21 §5.21); `None` = no label.
    ///
    /// Built by the overlay, which is the side that knows the level chain, the target kind and
    /// whether the last query was answered at all; the renderer only places and draws it.
    pub preview_label: Option<String>,
    /// Whether the previewed box is the whole window rather than an element (docs/21 §5.21) — the
    /// renderer then uses a neutral wash and a thin outline instead of the accent preview.
    pub preview_is_window: bool,
    /// How "walking" the level chain is right now, `0.0..=1.0` (docs/21 §5.24, A3).
    ///
    /// Drives the capture box's colour and wash: brand blue at rest, the capture green while the
    /// walk is live. The box itself is identified by the mask hole, which the renderer derives from
    /// `preview_bounds`/`selection`.
    pub capture_green: f32,
    /// How visible the preview box is, `0.0..=1.0` (docs/21 §5.24, ②): the box eases in when it
    /// appears, and is 1 from then on. The mask hole is not part of it — the content is already at
    /// its own brightness while the outline is still arriving.
    pub preview_alpha: f32,
    /// How many wheel stops the level walk still has in each direction, while it is above the answer
    /// (docs/21 §5.24, A1).
    pub level_badge: Option<LevelReach>,
    /// A one-shot hint in monitor-local coordinates (docs/21 §5.21): the level walk has to be
    /// explained once, or nobody finds it.
    pub hint: Option<(Point, String)>,
    /// The scroll session's panel, when one is running (docs/30 §19.7).
    ///
    /// Carried as the model rather than as pre-computed rectangles so the overlay can hand over a
    /// panel it just updated without translating it, and so the drawing layer keeps having no opinion.
    pub(crate) scroll_panel: Option<crate::scroll::panel::ScrollPanel>,
}

impl OverlayFrameState {
    pub fn new() -> Self {
        Self {
            selection: Rect::default(),
            cursor: Point::default(),
            cursor_visible: false,
            show_chrome: false,
            magnifier_rgb: None,
            magnifier_color_text: None,
            magnifier_relative: false,
            magnifier_zoom: 20.0,
            annotation_items: Vec::new(),
            annotation_selected_id: None,
            annotation_draft: None,
            hover_bounds: None,
            preview_bounds: None,
            chain_rings: Vec::new(),
            preview_label: None,
            preview_is_window: false,
            capture_green: 0.0,
            preview_alpha: 1.0,
            level_badge: None,
            hint: None,
            scroll_panel: None,
        }
    }
}

impl Default for OverlayFrameState {
    fn default() -> Self {
        Self::new()
    }
}

/// Owns the graphics stack behind one overlay window.
pub struct Win32Renderer {
    d2d: D2dRenderer,
    composition: CompositionTarget,
    layout: MonitorLayout,
    frame: Rect,
    sample_buffer: AsyncSampleBuffer,
}

impl Win32Renderer {
    /// Create the device stack and bind a composition target to `window`.
    ///
    /// The window handle comes from the typed `windows` bindings because that is what
    /// DirectComposition and Direct2D expect.
    pub fn new(
        window: ::windows::Win32::Foundation::HWND,
        layout: &MonitorLayout,
        device: Arc<GraphicsDevice>,
    ) -> Result<Self, String> {
        let sample_buffer =
            AsyncSampleBuffer::new(device.device(), device.context())?;
        let d2d = D2dRenderer::new(device, layout.dpi)?;
        let composition = d2d.composition_target(window)?;
        let mut renderer = Self {
            d2d,
            composition,
            layout: layout.clone(),
            frame: layout.local_bounds(),
            sample_buffer,
        };
        renderer.resize(layout)?;
        Ok(renderer)
    }

    /// DPI-dependent chrome metrics.
    /// DPI-dependent chrome metrics.
    #[allow(dead_code)]
    pub fn metrics(&self) -> RenderMetrics {
        self.d2d.metrics()
    }

    /// Current monitor layout.
    pub fn layout(&self) -> &MonitorLayout {
        &self.layout
    }

    /// Local frame rectangle (origin at the monitor's top-left).
    pub fn frame(&self) -> Rect {
        self.frame
    }

    /// Recreate the back buffer for a monitor geometry change.
    pub fn resize(&mut self, layout: &MonitorLayout) -> Result<(), String> {
        let width = layout.bounds.width().max(1) as u32;
        let height = layout.bounds.height().max(1) as u32;
        self.d2d.set_dpi(layout.dpi);
        self.d2d.ensure_back_buffer(width, height)?;
        if let Some(swap_chain) = self.d2d.swap_chain() {
            self.composition.attach(swap_chain)?;
        }
        self.layout = layout.clone();
        self.frame = layout.local_bounds();
        Ok(())
    }

    /// Bind the frozen frame as the L0 layer.
    ///
    /// When the provider produced a GPU texture the bitmap is created directly over it,
    /// so no pixels cross the PCIe bus; GPU-less frames fall back to a one-off upload.
    pub fn set_frame(&mut self, frozen: &FrozenFrame) -> Result<(), String> {
        if let Some(texture) = frozen.render_source() {
            return self.d2d.set_frame_texture(
                frozen.frame.width,
                frozen.frame.height,
                texture,
            );
        }
        let pixels = frozen
            .pixels()
            .map_err(|error| format!("frozen frame pixels unavailable: {error}"))?;
        self.d2d
            .update_frame(frozen.frame.width, frozen.frame.height, pixels)
    }

    /// Draw and present one frame.
    ///
    /// `annotations` is borrowed from the overlay controller so we can avoid
    /// cloning the item list every tick; `OverlayFrameState.annotation_items`
    /// carries the data when the controller wants to decouple.
    pub fn render(
        &mut self,
        state: &OverlayFrameState,
        annotations: Option<&AnnotationDocument>,
    ) -> Result<(), String> {
        // Build the RenderView. Prefer live borrow when available.
        let live_items: Vec<AnnotationItem> = annotations
            .map(|d| d.items().to_vec())
            .unwrap_or_else(|| state.annotation_items.clone());
        let live_selected = annotations.and_then(|d| d.selected_id()).or(state.annotation_selected_id);
        let live_draft = annotations
            .and_then(|d| d.draft.clone())
            .or(state.annotation_draft.clone());

        let view = RenderView {
            frame: self.frame,
            selection: state.selection,
            cursor: state.cursor,
            screen_origin: Point::new(self.layout.bounds.left, self.layout.bounds.top),
            cursor_visible: state.cursor_visible,
            show_chrome: state.show_chrome,
            work_area: self.layout.local_work_area(),
            magnifier_rgb: state.magnifier_rgb,
            magnifier_color_text: state.magnifier_color_text.clone(),
            magnifier_relative: state.magnifier_relative,
            magnifier_zoom: state.magnifier_zoom,
            annotation_items: live_items,
            annotation_selected_id: live_selected,
            annotation_draft: live_draft,
            hover_bounds: state.hover_bounds,
            preview_bounds: state.preview_bounds,
            chain_rings: state.chain_rings.clone(),
            preview_label: state.preview_label.clone(),
            preview_is_window: state.preview_is_window,
            capture_green: state.capture_green,
            preview_alpha: state.preview_alpha,
            level_badge: state.level_badge,
            hint: state.hint.clone(),
            scroll_panel: state.scroll_panel.clone(),
        };
        self.d2d.render(&view)?;
        self.d2d.present()?;
        self.composition.commit()
    }

    /// Produce the annotated export for `state.selection`.
    ///
    /// Replays the same [`AnnotationDocument`] the preview drew, but with chrome,
    /// cursor, the selection box and the draft suppressed, and reads the selected
    /// region back to CPU BGRA. This is what keeps the PNG pixel-identical to the
    /// screen crop (docs/11 §8.2 "导出和预览重放同一份文档").
    pub fn render_export(
        &mut self,
        state: &OverlayFrameState,
        annotations: Option<&AnnotationDocument>,
    ) -> Result<Vec<u8>, String> {
        let live_items: Vec<AnnotationItem> = annotations
            .map(|d| d.items().to_vec())
            .unwrap_or_else(|| state.annotation_items.clone());
        let view = RenderView {
            frame: self.frame,
            selection: state.selection,
            cursor: state.cursor,
            screen_origin: Point::new(self.layout.bounds.left, self.layout.bounds.top),
            cursor_visible: false,
            show_chrome: false,
            work_area: self.layout.local_work_area(),
            magnifier_rgb: None,
            magnifier_color_text: None,
            magnifier_relative: false,
            magnifier_zoom: 20.0,
            annotation_items: live_items,
            annotation_selected_id: None,
            annotation_draft: None,
            // Hover/preview hints are cleared by `OverlayRenderer::render_export`, which
            // owns the "no hints in the artifact" invariant.
            hover_bounds: state.hover_bounds,
            preview_bounds: state.preview_bounds,
            // Hover/preview/path hints are stripped by `OverlayRenderer::render_export`.
            chain_rings: state.chain_rings.clone(),
            // …and so are the labels and the one-shot hint: an artifact never carries UI.
            preview_label: None,
            preview_is_window: false,
            // An artifact carries no UI, so nothing may fade in either; the view built here has no
            // preview at all.
            preview_alpha: 1.0,
            // An artifact carries no UI, so nothing may walk its colour either; the view built here
            // has no preview anyway, and the mask hole is the exported selection.
            capture_green: 0.0,
            level_badge: None,
            hint: None,
            // The panel is UI, so the artifact gets none of it. `render_export` enforces the same
            // thing one layer down; this line is the one that keeps the panel out of the pixels even
            // if that invariant is ever loosened.
            scroll_panel: None,
        };
        self.d2d.render_export(&view)
    }

    /// Whether a failure message indicates a lost graphics device.
    pub fn is_device_lost(message: &str) -> bool {
        GraphicsDevice::is_device_lost(message)
    }

    /// Submit a non-blocking tile copy for async color sampling.
    /// Returns the slot index on success.
    pub fn request_sample(
        &mut self,
        texture: &ID3D11Texture2D,
        x: u32,
        y: u32,
        tile_size: u32,
    ) -> Result<usize, String> {
        self.sample_buffer.submit(texture, x, y, tile_size)
    }

    /// Poll a previously-submitted slot. Returns Some(Ok(tile_data)) when the GPU
    /// copy is complete, None if still in-flight, Some(Err) on failure.
    pub fn poll_sample(&mut self, slot: usize) -> Option<Result<Vec<u8>, String>> {
        self.sample_buffer.poll(slot)
    }

    /// Reset all pending sample slots.
    #[allow(dead_code)]
    pub fn reset_samples(&mut self) {
        self.sample_buffer.reset();
    }
}






