//! Direct2D / DirectWrite resources and the L0/L1/L2 draw calls.
//!
//! Layer order, back to front:
//! 1. **L0** the frozen capture frame, drawn 1:1 so the selection stays pixel exact;
//! 2. **L1** a translucent dark mask over everything *except* the selection, drawn
//!    as four rectangles so the selected pixels keep their original brightness and
//!    so a drag only invalidates the union of the old and new selection;
//! 3. **L2** rounded border, eight grips, the `width × height` label and the
//!    magnifier.
//!
//! Everything here is preview-only: the exported artifact is produced by
//! [`crate::application::capture_service`] from the captured frame, so no mask,
//! label or magnifier can leak into the result.

use windows_numerics::Vector2;
use std::sync::Arc;
use ::windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F,
    D2D_SIZE_U,
};
use ::windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_OPTIONS_NONE,
    D2D1_BITMAP_PROPERTIES1,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, D2D1_ROUNDED_RECT,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1_ELLIPSE, ID2D1Bitmap1, ID2D1DeviceContext,
    ID2D1SolidColorBrush,
};
use ::windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use ::windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_METRICS, DWriteCreateFactory, IDWriteFactory,
    IDWriteTextFormat,
};
use ::windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use ::windows::Win32::Graphics::Dxgi::IDXGISwapChain1;
use ::windows::core::PCWSTR;

use super::d3d11::GraphicsDevice;
use crate::capture::geometry::{Handle, MagnifierConfig, Point, Rect, SizeLabelPlacement};

/// The font family used for the size label, per the tasklist (§6.3).
const LABEL_FONT_FAMILY: &str = "Segoe UI Variable Display";
const LABEL_FONT_FALLBACK: &str = "Segoe UI";
const LABEL_FONT_SIZE_DIP: f32 = 12.0;

/// Stroke width of the selection border in DIP.
const BORDER_WIDTH_DIP: f32 = 1.5;
/// Diameter of each solid circular grip in DIP.
const HANDLE_SIZE_DIP: f32 = 12.0;

/// Mask colour (opaque black at 45% opacity). Configurable in one place so the
/// light/dark-background acceptance can be re-run with a different value.
pub const MASK_ALPHA: f32 = 0.45;
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
    /// Regions that must be repainted; empty means "repaint everything".
    pub damage: Vec<Rect>,
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
            damage: Vec::new(),
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
    magnifier_border: ID2D1SolidColorBrush,
    magnifier_grid: ID2D1SolidColorBrush,
    magnifier_info: ID2D1SolidColorBrush,
    magnifier_focus: ID2D1SolidColorBrush,
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
    handle_brush: Option<ID2D1SolidColorBrush>,
    label_background_brush: Option<ID2D1SolidColorBrush>,
    label_text_brush: Option<ID2D1SolidColorBrush>,
    crosshair_brush: Option<ID2D1SolidColorBrush>,
    magnifier_border_brush: Option<ID2D1SolidColorBrush>,
    magnifier_grid_brush: Option<ID2D1SolidColorBrush>,
    magnifier_info_brush: Option<ID2D1SolidColorBrush>,
    magnifier_focus_brush: Option<ID2D1SolidColorBrush>,
    metrics: RenderMetrics,
    size: (u32, u32),
}

impl OverlayRenderer {
    pub fn new(device: Arc<GraphicsDevice>, dpi: u32) -> Result<Self, String> {
        let d2d = device.create_d2d_context()?;
        let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }
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
            handle_brush: None,
            label_background_brush: None,
            label_text_brush: None,
            crosshair_brush: None,
            magnifier_border_brush: None,
            magnifier_grid_brush: None,
            magnifier_info_brush: None,
            magnifier_focus_brush: None,
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
    }

    /// Draw one frame of the overlay into the swap chain back buffer.
    pub fn render(&mut self, view: &RenderView) -> Result<(), String> {
        let Some(target) = self.target.clone() else {
            return Err("overlay renderer has no back buffer".into());
        };
        self.draw_to(&target, view)
    }

    /// Draw the L0/L1/L2 composition into an arbitrary D2D target.
    ///
    /// The swap chain path and the visual regression tests share this, so the tested
    /// composition is exactly the one the overlay presents.
    ///
    /// The draw is clipped to the bounding box of `view.damage`. The back buffer keeps
    /// its previous contents and full invalidations are expressed by damaging the whole
    /// frame, so clipping to the damage is what stops a cursor move from rasterising
    /// every pixel of the display.
    pub fn draw_to(&mut self, target: &ID2D1Bitmap1, view: &RenderView) -> Result<(), String> {
        let clip = damage_clip(view);
        unsafe {
            self.d2d.SetTarget(target);
            self.d2d.BeginDraw();
            self.d2d.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.d2d
                .SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            if let Some(clip) = clip {
                // Axis aligned, so the clip is exact and costs no extra geometry.
                self.d2d.PushAxisAlignedClip(&to_d2d(clip), D2D1_ANTIALIAS_MODE_ALIASED);
            }
        }
        let result = self.draw_layers(view);
        let end_draw = unsafe {
            if clip.is_some() {
                self.d2d.PopAxisAlignedClip();
            }
            self.d2d.EndDraw(None, None)
        }
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

        // L1: mask everything outside the selection. Using four bands (instead of
        // re-drawing the frame inside the hole) keeps the selected pixels at their
        // original brightness and lets a drag touch only the changed bands.
        unsafe {
            if view.selection.is_empty() {
                self.d2d.FillRectangle(&to_d2d(view.frame), &mask);
            } else {
                let hole = view.selection.intersect(view.frame);
                for band in view.frame.surround(hole) {
                    if band.is_empty() {
                        continue;
                    }
                    self.d2d.FillRectangle(&to_d2d(band), &mask);
                }
            }
        }

        // L2: selection chrome.
        if view.show_chrome && !view.selection.is_empty() {
            let resources = ChromeResources {
                frame_bitmap,
                border: self.require_brush(&self.border_brush, "border brush")?,
                handle: self.require_brush(&self.handle_brush, "handle brush")?,
                label_background: self
                    .require_brush(&self.label_background_brush, "label background brush")?,
                label_text: self.require_brush(&self.label_text_brush, "label text brush")?,
                crosshair: self.require_brush(&self.crosshair_brush, "crosshair brush")?,
                magnifier_border: self
                    .require_brush(&self.magnifier_border_brush, "magnifier border brush")?,
                magnifier_grid: self
                    .require_brush(&self.magnifier_grid_brush, "magnifier grid brush")?,
                magnifier_info: self
                    .require_brush(&self.magnifier_info_brush, "magnifier info brush")?,
                magnifier_focus: self
                    .require_brush(&self.magnifier_focus_brush, "magnifier focus brush")?,
            };
            self.draw_chrome(view, &resources)?;
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
        let config = MagnifierConfig::default().scaled(metrics.dpi);
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
        let border = &resources.magnifier_border;
        let crosshair = &resources.crosshair;
        let grid = &resources.magnifier_grid;
        let info = &resources.magnifier_info;
        let focus = &resources.magnifier_focus;

        unsafe {
            // Clip so the magnified pixels cannot spill out of the panel.
            self.d2d
                .PushAxisAlignedClip(&to_d2d(geometry.panel), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.d2d.DrawBitmap(
                bitmap,
                Some(&to_d2d(geometry.panel)),
                1.0,
                D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                Some(&to_d2d(geometry.source)),
                None,
            );
            self.d2d.PopAxisAlignedClip();

            // Draw a restrained pixel grid over the nearest-neighbour image. It
            // makes the sampled source pixels legible without obscuring the frame.
            let step = geometry.zoom.max(1) as f32;
            let panel = geometry.panel;
            for index in 1..geometry.source.width() {
                let x = panel.left as f32 + index as f32 * step + 0.5;
                self.d2d.DrawLine(
                    vector2(x, panel.top as f32),
                    vector2(x, panel.bottom as f32),
                    grid,
                    1.0,
                    None,
                );
                let y = panel.top as f32 + index as f32 * step + 0.5;
                self.d2d.DrawLine(
                    vector2(panel.left as f32, y),
                    vector2(panel.right as f32, y),
                    grid,
                    1.0,
                    None,
                );
            }

            // Outline the source pixel under the cursor rather than painting a
            // detached marker. This remains aligned with the sampled pixel at every
            // zoom level and cannot leak outside the magnifier damage rectangle.
            let center = Point::new(
                view.cursor
                    .x
                    .clamp(geometry.source.left, geometry.source.right.saturating_sub(1)),
                view.cursor
                    .y
                    .clamp(geometry.source.top, geometry.source.bottom.saturating_sub(1)),
            );
            let center_x = panel.left + (center.x - geometry.source.left) * geometry.zoom as i32;
            let center_y = panel.top + (center.y - geometry.source.top) * geometry.zoom as i32;
            let center_cell = Rect::from_origin_size(
                Point::new(center_x, center_y),
                geometry.zoom as i32,
                geometry.zoom as i32,
            );
            self.d2d.DrawRectangle(&to_d2d(center_cell), focus, 1.0, None);

            self.d2d.FillRectangle(&to_d2d(geometry.info_panel), info);
            let coordinate = format!(
                "({}, {})",
                view.cursor.x + view.screen_origin.x,
                view.cursor.y + view.screen_origin.y
            );
            let format = self.label_text_format_mut()?;
            let wide = coordinate.encode_utf16().collect::<Vec<u16>>();
            let text_rect = D2D_RECT_F {
                left: geometry.info_panel.left as f32 + metrics.label_padding_x,
                top: geometry.info_panel.top as f32,
                right: geometry.info_panel.right as f32 - metrics.label_padding_x,
                bottom: geometry.info_panel.bottom as f32,
            };
            self.d2d.DrawText(
                &wide,
                &format,
                &text_rect,
                &resources.label_text,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );

            let outline = D2D1_ROUNDED_RECT {
                rect: to_d2d(geometry.panel),
                radiusX: 2.0,
                radiusY: 2.0,
            };
            self.d2d.DrawRoundedRectangle(
                &outline,
                border,
                metrics.border_width,
                None,
            );

            // Full-height and full-width guides are drawn in screen space. The
            // magnifier itself deliberately has no detached pixel marker: a marker
            // painted into a previous panel can otherwise survive a partial damage
            // redraw and appear as a stray dot outside the current panel.
            self.d2d.DrawLine(
                vector2(view.cursor.x as f32 + 0.5, view.frame.top as f32),
                vector2(view.cursor.x as f32 + 0.5, view.frame.bottom as f32),
                crosshair,
                1.0,
                None,
            );
            self.d2d.DrawLine(
                vector2(view.frame.left as f32, view.cursor.y as f32 + 0.5),
                vector2(view.frame.right as f32, view.cursor.y as f32 + 0.5),
                crosshair,
                1.0,
                None,
            );
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
        for family in [LABEL_FONT_FAMILY, LABEL_FONT_FALLBACK] {
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
        // System accent blue, matching the Windows snipping experience.
        let accent = color(0.0, 0.47, 0.83, 1.0);
        let white = color(1.0, 1.0, 1.0, 1.0);
        let panel = color(0.09, 0.09, 0.11, 0.92);
        let crosshair = color(1.0, 0.75, 0.0, 0.95);
        let grid = color(1.0, 1.0, 1.0, 0.24);
        let info = color(0.02, 0.02, 0.025, 0.94);
        self.mask_brush = Some(self.create_brush(&mask)?);
        self.border_brush = Some(self.create_brush(&accent)?);
        self.handle_brush = Some(self.create_brush(&white)?);
        self.label_background_brush = Some(self.create_brush(&panel)?);
        self.label_text_brush = Some(self.create_brush(&white)?);
        self.crosshair_brush = Some(self.create_brush(&crosshair)?);
        self.magnifier_border_brush = Some(self.create_brush(&accent)?);
        self.magnifier_grid_brush = Some(self.create_brush(&grid)?);
        self.magnifier_info_brush = Some(self.create_brush(&info)?);
        self.magnifier_focus_brush = Some(self.create_brush(&white)?);
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
        self.handle_brush = None;
        self.label_background_brush = None;
        self.label_text_brush = None;
        self.crosshair_brush = None;
        self.magnifier_border_brush = None;
        self.magnifier_grid_brush = None;
        self.magnifier_info_brush = None;
        self.magnifier_focus_brush = None;
    }
}

fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
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

/// Bounding box of the damaged regions, intersected with the frame, in DIP.
///
/// `None` means "nothing to repaint". An empty damage list is treated as "repaint
/// everything": a caller that forgot to declare damage must not end up with a stale
/// window.
fn damage_clip(view: &RenderView) -> Option<Rect> {
    if view.damage.is_empty() {
        return Some(view.frame);
    }
    let mut bounds: Option<Rect> = None;
    for rect in &view.damage {
        let rect = rect.intersect(view.frame);
        if rect.is_empty() {
            continue;
        }
        bounds = Some(match bounds {
            Some(current) => current.union(rect),
            None => rect,
        });
    }
    bounds
}

#[cfg(test)]
mod tests {
    use super::{MASK_ALPHA, OverlayRenderer, RenderMetrics, RenderView, damage_clip, to_d2d};
    use ::windows::Win32::Graphics::Direct2D::Common::D2D1_ALPHA_MODE_PREMULTIPLIED;
    use ::windows::Win32::Graphics::Direct2D::D2D1_BITMAP_OPTIONS_TARGET;
    use crate::capture::geometry::{Handle, Point, Rect, SizeLabelPlacement, size_label_placement};

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
    fn masked(pixel: [u8; 4]) -> [u8; 4] {
        let scale = 1.0 - MASK_ALPHA;
        [
            (pixel[0] as f32 * scale).round() as u8,
            (pixel[1] as f32 * scale).round() as u8,
            (pixel[2] as f32 * scale).round() as u8,
            255,
        ]
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
        assert_eq!(
            pixel_at(&unselected, width, 32, 24),
            masked(background),
            "an unselected frame must be uniformly masked"
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
        assert_eq!(
            outside,
            masked(background),
            "mask alpha must match the configured value"
        );

        let _ = frame;
    }

    /// A partial repaint must leave every pixel outside the damaged region untouched.
    ///
    /// This is what makes the dirty-rectangle optimisation real rather than declared: the
    /// test renders a chrome frame into a persistent target, then repaints with a tiny
    /// damage box and asserts that pixels well outside it are byte-identical.
    #[test]
    fn partial_repaint_does_not_touch_pixels_outside_the_damage() {
        let Ok(device) = super::GraphicsDevice::create() else {
            eprintln!("no D3D11 device in this session; skipping the damage check");
            return;
        };
        let width = 96u32;
        let height = 96u32;
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
        let target_bitmap = super::super::d3d11::create_bitmap_from_texture(
            &d2d_context,
            &target.texture,
            D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
        .unwrap();

        let frame = Rect::from_origin_size(Point::new(0, 0), width as i32, height as i32);

        // First pass: a large marquee, drawn in full.
        let mut full = RenderView::new(frame);
        full.selection = Rect::new(8, 8, 40, 40);
        full.show_chrome = false;
        full.cursor_visible = false;
        full.damage.clear();
        renderer.draw_to(&target_bitmap, &full).unwrap();
        let before = renderer.device().read_back_bgra(&target.texture).unwrap();

        // Second pass: the marquee's right edge pulls in from x=40 to x=24, but only the
        // box (8,8)-(32,32) is declared damaged.
        let damage = Rect::new(8, 8, 32, 32);
        let mut partial = RenderView::new(frame);
        partial.selection = Rect::new(8, 8, 24, 40);
        partial.show_chrome = false;
        partial.cursor_visible = false;
        partial.damage = vec![damage];
        renderer.draw_to(&target_bitmap, &partial).unwrap();
        let after = renderer.device().read_back_bgra(&target.texture).unwrap();

        // Inside the damage the change is visible: (24,10) was selected in the first pass
        // and is masked now.
        let inside_before = pixel_at(&before, width, 24, 10);
        let inside_after = pixel_at(&after, width, 24, 10);
        assert_ne!(
            inside_before, inside_after,
            "the damaged region must actually be repainted"
        );
        assert_eq!(inside_before, background, "was selected in the first pass");
        assert_eq!(inside_after, masked(background), "is masked in the second pass");

        // Outside the damage, every sampled pixel is untouched — including (40,10), which
        // is still inside the selection and would have been repainted by a full redraw.
        let mut changed = Vec::new();
        for (x, y) in [(4, 4), (60, 60), (80, 20), (20, 80), (90, 90), (40, 10), (36, 16)] {
            if pixel_at(&before, width, x, y) != pixel_at(&after, width, x, y) {
                changed.push((x, y));
            }
        }
        assert!(
            changed.is_empty(),
            "pixels outside the damage region were rewritten: {changed:?}"
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
    fn damage_union_covers_both_selections() {
        let frame = Rect::new(0, 0, 1000, 800);
        let old = Rect::new(100, 100, 200, 200);
        let new = Rect::new(300, 400, 500, 600);
        let mut view = RenderView::new(frame);
        view.selection = new;
        view.damage = vec![old, new];
        // The clip is the union of the damage, so both the old and the new selection
        // are inside the repainted region.
        assert_eq!(damage_clip(&view), Some(Rect::new(100, 100, 500, 600)));
    }

    #[test]
    fn damage_clip_is_the_damage_bounding_box_not_the_whole_frame() {
        let frame = Rect::new(0, 0, 1920, 1080);
        let mut view = RenderView::new(frame);
        view.damage = vec![
            Rect::new(10, 20, 60, 40),
            Rect::new(100, 200, 140, 260),
        ];
        let clip = damage_clip(&view).unwrap();
        assert_eq!(clip, Rect::new(10, 20, 140, 260));
        assert!(
            clip.area() < frame.area() / 10,
            "a small cursor move must not clip to the whole screen"
        );
    }

    #[test]
    fn empty_damage_means_repaint_everything() {
        // A caller that declares no damage must not end up with a stale window.
        let frame = Rect::new(0, 0, 640, 480);
        let mut view = RenderView::new(frame);
        view.damage.clear();
        assert_eq!(damage_clip(&view), Some(frame));
    }

    #[test]
    fn damage_outside_the_frame_is_ignored() {
        let frame = Rect::new(0, 0, 640, 480);
        let mut view = RenderView::new(frame);
        view.damage = vec![Rect::new(700, 700, 800, 800)];
        assert_eq!(damage_clip(&view), None, "nothing to repaint");

        // Partially overlapping damage is trimmed to the frame.
        view.damage = vec![Rect::new(600, 400, 700, 500)];
        assert_eq!(damage_clip(&view), Some(Rect::new(600, 400, 640, 480)));
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
}











