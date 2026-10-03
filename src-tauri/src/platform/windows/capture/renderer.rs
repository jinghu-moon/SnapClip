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

use crate::capture::geometry::{MonitorLayout, Point, Rect};

use super::providers::FrozenFrame;
use super::win::d2d::{OverlayRenderer as D2dRenderer, RenderMetrics, RenderView};
use super::win::d3d11::{CompositionTarget, GraphicsDevice};

/// Everything the overlay needs to draw one frame.
#[derive(Debug, Clone)]
pub struct OverlayFrameState {
    pub selection: Rect,
    pub cursor: Point,
    pub cursor_visible: bool,
    pub show_chrome: bool,
    /// Regions invalidated since the previous draw.
    pub damage: Vec<Rect>,
}

impl OverlayFrameState {
    pub fn new() -> Self {
        Self {
            selection: Rect::default(),
            cursor: Point::default(),
            cursor_visible: false,
            show_chrome: false,
            damage: Vec::new(),
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
        let d2d = D2dRenderer::new(device, layout.dpi)?;
        let composition = d2d.composition_target(window)?;
        let mut renderer = Self {
            d2d,
            composition,
            layout: layout.clone(),
            frame: layout.local_bounds(),
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
    /// Monitor-local frame rectangle currently rendered.
    #[allow(dead_code)]
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

    /// Damage caused by the cursor overlay at `cursor`.
    ///
    /// The magnifier panel and the crosshair guides are the only things that move with
    /// the cursor, so this is what a mouse move has to repaint. Handing this to the
    /// renderer keeps one implementation of the magnifier geometry: the drawing code
    /// and the invalidation code call the same `magnifier_geometry`.
    pub fn cursor_damage(&self, cursor: Point) -> Vec<Rect> {
        let metrics = self.d2d.metrics();
        let config = crate::capture::geometry::MagnifierConfig::default().scaled(metrics.dpi);
        let geometry = crate::capture::geometry::magnifier_geometry(
            cursor,
            config,
            self.frame,
            self.layout.local_work_area(),
        );
        let mut damage = vec![geometry.bounds];
        // The crosshair guides span the frame, so a move invalidates both lines.
        damage.push(Rect::new(self.frame.left, cursor.y, self.frame.right, cursor.y + 1));
        damage.push(Rect::new(cursor.x, self.frame.top, cursor.x + 1, self.frame.bottom));
        damage
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
    pub fn render(&mut self, state: &OverlayFrameState) -> Result<(), String> {
        let view = RenderView {
            frame: self.frame,
            selection: state.selection,
            cursor: state.cursor,
            screen_origin: Point::new(self.layout.bounds.left, self.layout.bounds.top),
            cursor_visible: state.cursor_visible,
            show_chrome: state.show_chrome,
            work_area: self.layout.local_work_area(),
            damage: state.damage.clone(),
        };
        self.d2d.render(&view)?;
        self.d2d.present()?;
        self.composition.commit()
    }

    /// Whether a failure message indicates a lost graphics device.
    pub fn is_device_lost(message: &str) -> bool {
        GraphicsDevice::is_device_lost(message)
    }
}






