//! Frame providers: Windows Graphics Capture first, BitBlt as the compatibility
//! fallback. Both hand back one frozen frame per session.

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use ::windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

use crate::ports::PixelSliceSource;
use crate::geometry::Rect;
use crate::session::CapturedFrame;
use crate::{CaptureError, CaptureResult};
use snapclip_model::PixelFormat;

use super::monitor::CapturedMonitor;
use super::win::bitblt;
use super::win::d3d11::{GraphicsDevice, GpuFrame};
use super::win::wgc;

/// Which provider produced a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Wgc,
    BitBlt,
    /// The window-level WGC path (`CreateForWindow` + one session-long pool).
    ///
    /// It is not an alternative monitor provider: the scroll path is its only caller
    /// (`docs/30` §24.2), and it delivers many frames per session instead of one.
    WgcWindow,
}

impl ProviderKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Wgc => "wgc",
            Self::BitBlt => "bitblt",
            Self::WgcWindow => "wgc-window",
        }
    }
}

/// The frozen back buffer of one session.
///
/// The GPU texture is the primary representation: the overlay renders L0 straight from
/// it, so arming a session performs **no** GPU → CPU transfer. CPU pixels are produced
/// lazily, only when something actually needs them (the artifact export on confirm).
pub struct FrozenFrame {
    pub frame: CapturedFrame,
    /// The monitor this frame was taken from.
    ///
    /// Retained for diagnostics and for the multi-monitor acceptance checks; the
    /// overlay only needs the frame rectangle, which `frame` already carries.
    #[allow(dead_code)]
    pub monitor: CapturedMonitor,
    pub provider: ProviderKind,
    texture: Option<GpuFrame>,
    /// Kept so the pixels can still be read back after capture.
    ///
    /// `None` is only possible for a frame that already carries materialised pixels
    /// (BitBlt at capture time, and the pure cropping tests).
    device: Option<Arc<GraphicsDevice>>,
    /// Filled on first use. `OnceLock` because the readback must happen at most once.
    pixels: OnceLock<Vec<u8>>,
}

impl FrozenFrame {
    /// The GPU texture of the frozen frame, if the provider produced one.
    #[allow(dead_code)]
    pub fn texture(&self) -> Option<&GpuFrame> {
        self.texture.as_ref()
    }

    /// The device the texture lives on.
    ///
    /// The overlay builds its renderer from exactly this device: D2D surfaces can
    /// only be created from a texture belonging to the same D3D11 device, and on
    /// the async path the providers (and their device) now live on the capture
    /// worker, so the frame has to carry the shared handle across.
    pub fn device(&self) -> Option<&Arc<GraphicsDevice>> {
        self.device.as_ref()
    }

    /// GPU texture when available, otherwise CPU pixels uploaded into one.
    ///
    /// The overlay always has a texture to draw from; only a provider that failed on
    /// both paths ends up without one.
    pub fn render_source(&self) -> Option<&ID3D11Texture2D> {
        self.texture.as_ref().map(|gpu| &gpu.texture)
    }

    /// CPU pixels of the frozen frame, read back on first use.
    ///
    /// This is the *fallback* full-frame transfer: an artifact export goes through
    /// [`Self::read_region`] instead, so a 300x200 selection never pays for a 4K
    /// readback (docs/11 §Phase 3). Only a frame with no selection — the renderer's
    /// CPU upload path and the tests — uses this.
    pub fn pixels(&self) -> CaptureResult<&[u8]> {
        if let Some(pixels) = self.pixels.get() {
            return Ok(pixels);
        }
        let Some(gpu) = self.texture.as_ref() else {
            return Err(CaptureError::CaptureFailed(
                "frozen frame has neither a texture nor pixels".into(),
            ));
        };
        let Some(device) = self.device.as_ref() else {
            return Err(CaptureError::CaptureFailed(
                "frozen frame has a texture but no device to read it back with".into(),
            ));
        };
        // Phase 0 observability: this transfer is always frame-sized today, whatever
        // the eventual selection is. Phase 3 replaces it with a region readback and
        // will be judged against these byte counts.
        let started_at = std::time::Instant::now();
        let pixels = device
            .read_back_bgra(&gpu.texture)
            .map_err(|message| classify_device_error("GPU readback", message))?;
        eprintln!(
            "[snapclip][capture] readback provider={} frame={}x{} bytes={} elapsed_ms={}",
            self.frame.provider,
            self.frame.width,
            self.frame.height,
            pixels.len(),
            started_at.elapsed().as_millis()
        );
        // A racing reader may have won; either buffer is equivalent.
        Ok(self.pixels.get_or_init(|| pixels))
    }

    /// Tightly packed BGRA pixels of one region, **without** a full-frame transfer.
    ///
    /// The two provider paths converge here:
    /// * BitBlt already holds CPU pixels, so the region is a pure memory crop.
    /// * WGC copies only the requested box (`CopySubresourceRegion`) into a
    ///   region-sized staging texture, so `readback_bytes == region area × 4` whatever
    ///   the monitor resolution is.
    ///
    /// `region` is monitor-local and is clipped to the frame; a region that cannot
    /// supply a single pixel — fully off-frame, or degenerate after clipping — is an
    /// error rather than an empty buffer, so no encoder ever sees a zero-size image.
    pub fn read_region(&self, region: Rect) -> CaptureResult<Vec<u8>> {
        let frame_rect = self.frame.rect();
        let clipped = region.intersect(frame_rect);
        if clipped.is_empty() {
            return Err(CaptureError::InvalidState(if region.is_empty() {
                "artifact region is empty".into()
            } else {
                "artifact region does not overlap the frozen frame".into()
            }));
        }
        let width = clipped.width() as usize;
        let height = clipped.height() as usize;

        // BitBlt kept its CPU pixels, so cropping them is cheaper than any GPU round
        // trip — and it keeps the GPU out of the export path entirely.
        if let Some(pixels) = self.pixels.get() {
            return crop_pixel_rows(pixels, self.frame.width as usize * 4, clipped).ok_or_else(
                || {
                    CaptureError::CaptureFailed(
                        "frozen frame pixel buffer is shorter than its geometry".into(),
                    )
                },
            );
        }

        let gpu = self.texture.as_ref().ok_or_else(|| {
            CaptureError::CaptureFailed("frozen frame has neither a texture nor pixels".into())
        })?;
        let device = self.device.as_ref().ok_or_else(|| {
            CaptureError::CaptureFailed(
                "frozen frame has a texture but no device to read it back with".into(),
            )
        })?;
        // Phase 3 observability: the byte count is now the selection's, not the
        // monitor's, which is exactly the acceptance criterion for this phase.
        let started_at = std::time::Instant::now();
        let pixels = device
            .read_back_region_bgra(
                &gpu.texture,
                clipped.left.max(0) as u32,
                clipped.top.max(0) as u32,
                width as u32,
                height as u32,
            )
            .map_err(|message| classify_device_error("GPU region readback", message))?;
        eprintln!(
            "[snapclip][capture] region readback provider={} region=({},{})-{}x{} frame={}x{} bytes={} elapsed_ms={}",
            self.frame.provider,
            clipped.left,
            clipped.top,
            width,
            height,
            self.frame.width,
            self.frame.height,
            pixels.len(),
            started_at.elapsed().as_millis()
        );
        if pixels.len() != width * height * 4 {
            return Err(CaptureError::CaptureFailed(format!(
                "region readback returned {} bytes, expected {}",
                pixels.len(),
                width * height * 4
            )));
        }
        Ok(pixels)
    }

    /// Whether the CPU pixels have been read back yet.
    ///
    /// The whole point of the deferred readback is that arming a session leaves this
    /// `false`, so it is asserted directly.
    #[allow(dead_code)]
    pub fn pixels_read(&self) -> bool {
        self.pixels.get().is_some()
    }

    /// Failures attributable to a lost graphics device.
    ///
    /// The interactive path classifies through `Win32Renderer::is_device_lost`; this
    /// variant exists for provider-level checks.
    #[allow(dead_code)]
    pub fn is_device_lost(error: &CaptureError) -> bool {
        match error {
            CaptureError::DeviceRemoved(message)
            | CaptureError::RenderFailed(message)
            | CaptureError::CaptureFailed(message) => GraphicsDevice::is_device_lost(message),
            _ => false,
        }
    }
}

impl std::fmt::Debug for FrozenFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FrozenFrame")
            .field("size", &(self.frame.width, self.frame.height))
            .field("provider", &self.provider)
            .field("has_texture", &self.texture.is_some())
            .field("pixels_read", &self.pixels.get().is_some())
            .finish()
    }
}

/// Pixel source that crops the frozen frame for the artifact encoder.
pub struct FrozenFramePixels<'a> {
    frame: &'a FrozenFrame,
}

impl<'a> FrozenFramePixels<'a> {
    pub fn new(frame: &'a FrozenFrame) -> Self {
        Self { frame }
    }
}

impl PixelSliceSource for FrozenFramePixels<'_> {
    fn read_bgra(&self, region: Rect) -> CaptureResult<Vec<u8>> {
        // One code path for both providers: the frame decides whether the region is a
        // CPU crop (BitBlt) or a `CopySubresourceRegion` (WGC).
        self.frame.read_region(region)
    }
}

/// Crop `region` out of a top-down BGRA buffer whose rows are `stride` bytes wide.
///
/// Kept separate from the pixel source because the two failure modes that matter — a
/// padded (`RowPitch > width × 4`) source and a selection flush against the right/bottom
/// edge — are properties of this arithmetic, not of the GPU or of Win32, so they can be
/// tested exhaustively without either.
///
/// Returns `None` when `pixels` cannot supply every row of `region`.
fn crop_pixel_rows(pixels: &[u8], stride: usize, region: Rect) -> Option<Vec<u8>> {
    let width = usize::try_from(region.width()).ok()?;
    let height = usize::try_from(region.height()).ok()?;
    let left = usize::try_from(region.left).ok()?;
    let top = usize::try_from(region.top).ok()?;
    if width == 0 || height == 0 || stride < width * 4 || left > stride / 4 {
        return None;
    }
    let mut output = Vec::with_capacity(width * height * 4);
    for row in 0..height {
        let source_row = top.checked_add(row)?;
        let start = source_row.checked_mul(stride)?.checked_add(left.checked_mul(4)?)?;
        let end = start.checked_add(width * 4)?;
        output.extend_from_slice(pixels.get(start..end)?);
    }
    Some(output)
}

/// A device that has to be created before any provider can run.
pub struct CaptureProviders {
    device: Arc<GraphicsDevice>,
    preferred: Option<ProviderKind>,
    diagnostics: Vec<String>,
}

impl CaptureProviders {
    /// Probe once at startup. The result is reused for every session.
    pub fn new() -> Result<Self, CaptureError> {
        let started_at = Instant::now();
        let device = Arc::new(
            GraphicsDevice::create()
                .map_err(|message| classify_device_error("D3D11CreateDevice", message))?,
        );
        let wgc_supported = wgc::is_supported();
        eprintln!(
            "[snapclip][capture] providers initialized wgc_supported={} elapsed_ms={}",
            wgc_supported,
            started_at.elapsed().as_millis()
        );
        let mut diagnostics = Vec::new();
        if !wgc_supported {
            diagnostics.push(
                "Windows Graphics Capture is unavailable; using the BitBlt fallback".to_string(),
            );
        }
        Ok(Self {
            device,
            preferred: Some(if wgc_supported {
                ProviderKind::Wgc
            } else {
                ProviderKind::BitBlt
            }),
            diagnostics,
        })
    }

    /// The graphics device shared by every provider.
    #[allow(dead_code)]
    #[cfg(test)]
    pub fn device(&self) -> &GraphicsDevice {
        &self.device
    }

    /// Provider wiring messages collected while probing and capturing.
    #[allow(dead_code)]
    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    /// Capture one frozen frame of `monitor`.
    ///
    /// Tries the preferred provider first and falls back to BitBlt when WGC is
    /// unavailable, so a machine without WGC still gets a working screenshot.
    pub fn capture(&mut self, monitor: &CapturedMonitor) -> CaptureResult<FrozenFrame> {
        let started_at = Instant::now();
        let attempts = attempt_order(self.preferred);

        let mut last_error: Option<CaptureError> = None;
        for provider in attempts {
            eprintln!(
                "[snapclip][capture] provider attempt={} monitor={}x{}",
                provider.name(),
                monitor.width(),
                monitor.height()
            );
            match self.capture_with(provider, monitor) {
                Ok(frame) => {
                    eprintln!(
                        "[snapclip][capture] provider success={} elapsed_ms={}",
                        provider.name(),
                        started_at.elapsed().as_millis()
                    );
                    return Ok(frame);
                }
                Err(error) => {
                    eprintln!(
                        "[snapclip][capture] provider failure={} elapsed_ms={} error={}",
                        provider.name(),
                        started_at.elapsed().as_millis(),
                        error
                    );
                    self.diagnostics.push(format!(
                        "{} provider failed: {error}",
                        provider.name()
                    ));
                    self.preferred = Some(ProviderKind::BitBlt);
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            CaptureError::ProviderUnavailable("no frame provider is available".into())
        }))
    }

    fn capture_with(
        &self,
        provider: ProviderKind,
        monitor: &CapturedMonitor,
    ) -> CaptureResult<FrozenFrame> {
        match provider {
            ProviderKind::Wgc => {
                // No readback here: L0 renders from this texture and the CPU copy is
                // produced lazily only if an artifact is actually exported.
                let gpu = wgc::capture_monitor(&self.device, monitor)
                    .map_err(|message| classify_device_error("Windows Graphics Capture", message))?;
                Ok(frozen_frame(
                    monitor,
                    ProviderKind::Wgc,
                    Some(gpu),
                    self.device.clone(),
                    None,
                ))
            }
            ProviderKind::BitBlt => {
                let captured = bitblt::capture_monitor(monitor)
                    .map_err(CaptureError::CaptureFailed)?;
                // BitBlt hands back CPU pixels, so upload them once to get the same
                // GPU-resident L0 representation the WGC path has. Phase 3: retain the
                // original CPU pixels so the export path crops them directly instead of
                // reading back from the GPU again.
                let cpu_pixels = captured.pixels.clone();
                let gpu = self
                    .device
                    .create_bgra_texture(captured.width, captured.height, &captured.pixels)
                    .map_err(|message| classify_device_error("texture upload", message))?;
                Ok(frozen_frame(
                    monitor,
                    ProviderKind::BitBlt,
                    Some(gpu),
                    self.device.clone(),
                    Some(cpu_pixels),
                ))
            }
            ProviderKind::WgcWindow => Err(CaptureError::ProviderUnavailable(
                "the window-level provider serves the scroll path, not a monitor capture"
                    .into(),
            )),
        }
    }
}

fn frozen_frame(
    monitor: &CapturedMonitor,
    provider: ProviderKind,
    texture: Option<GpuFrame>,
    device: Arc<GraphicsDevice>,
    pre_materialized_pixels: Option<Vec<u8>>,
) -> FrozenFrame {
    let pixels = OnceLock::new();
    if let Some(buf) = pre_materialized_pixels {
        let _ = pixels.set(buf);
    }
    FrozenFrame {
        frame: CapturedFrame {
            width: monitor.width(),
            height: monitor.height(),
            pixel_format: PixelFormat::Bgra8Unorm,
            captured_at_unix_ms: snapclip_model::unix_time_ms(),
            provider: provider.name(),
        },
        monitor: monitor.clone(),
        provider,
        texture,
        device: Some(device),
        pixels,
    }
}

fn classify_device_error(context: &str, message: String) -> CaptureError {
    if GraphicsDevice::is_device_lost(&message) {
        CaptureError::DeviceRemoved(format!("{context}: {message}"))
    } else {
        CaptureError::CaptureFailed(format!("{context}: {message}"))
    }
}

/// A frame whose pixels are already materialised, so the cropping and export paths
/// can be exercised without a GPU, a monitor or a device.
///
/// The pixel grid encodes its own coordinates (`[x, y, 0, 255]`), which is what makes
/// a crop verifiable byte for byte.
#[cfg(test)]
pub(crate) fn test_frozen_frame(width: u32, height: u32) -> FrozenFrame {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            pixels.extend_from_slice(&[x as u8, y as u8, 0, 255]);
        }
    }
    FrozenFrame {
        frame: CapturedFrame {
            width,
            height,
            pixel_format: PixelFormat::Bgra8Unorm,
            captured_at_unix_ms: 1,
            provider: "test",
        },
        monitor: CapturedMonitor {
            handle: 0,
            layout: crate::geometry::MonitorLayout {
                bounds: Rect::from_origin_size(
                    crate::geometry::Point::new(0, 0),
                    width as i32,
                    height as i32,
                ),
                work_area: Rect::from_origin_size(
                    crate::geometry::Point::new(0, 0),
                    width as i32,
                    height as i32,
                ),
                dpi: 96,
                primary: true,
            },
        },
        provider: ProviderKind::BitBlt,
        texture: None,
        device: None,
        pixels: OnceLock::from(pixels),
    }
}

/// Providers tried for one capture, in order.
///
/// WGC is attempted first only when it is the preferred provider; BitBlt is always
/// the final entry so a machine without WGC — or a WGC run whose first frame times
/// out — still falls back to a frame instead of failing the session outright
/// (docs/11 §3.2). Keeping this decision pure makes the fallback contract unit
/// testable without a GPU.
fn attempt_order(preferred: Option<ProviderKind>) -> Vec<ProviderKind> {
    let mut attempts = Vec::new();
    // The window backend belongs to the scroll path, which orders its own backends
    // (`docs/30` §24.7, task `P2.06`). It is returned as a *single* attempt here so a
    // wiring mistake fails loudly instead of silently reading the desktop through the
    // BitBlt fallback and reporting a frame that is not the target window's.
    if preferred == Some(ProviderKind::WgcWindow) {
        return vec![ProviderKind::WgcWindow];
    }
    if preferred == Some(ProviderKind::Wgc) {
        attempts.push(ProviderKind::Wgc);
    }
    attempts.push(ProviderKind::BitBlt);
    attempts
}

/// Where one region readback of a delivered frame actually goes.
///
/// The policy above the transfer — clip the request to the frame, refuse a second read
/// of the same frame, account the bytes — is worth testing without a GPU, so the
/// transfer is a seam: `GpuTransfer` is the production implementation and the tests
/// substitute a recorder. Both go through [`ScrollFrame::read_region`].
trait RegionTransfer {
    /// The frame's pixel size, which is what a request is clipped against.
    fn size(&self) -> (u32, u32);

    /// Copy `region` (already clipped to the frame) out of the frame.
    fn transfer(&mut self, region: Rect) -> Result<Vec<u8>, String>;
}

/// The production transfer: `CopySubresourceRegion` into a region-sized staging
/// texture, so the bytes that cross the bus are the region's, not the monitor's.
struct GpuTransfer {
    device: Arc<GraphicsDevice>,
    texture: ID3D11Texture2D,
    size: (u32, u32),
}

impl RegionTransfer for GpuTransfer {
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn transfer(&mut self, region: Rect) -> Result<Vec<u8>, String> {
        self.device.read_back_region_bgra(
            &self.texture,
            region.left.max(0) as u32,
            region.top.max(0) as u32,
            region.width() as u32,
            region.height() as u32,
        )
    }
}

/// One delivered frame of the window-level capture path, read back **once**.
///
/// This is the scroll path's replacement for [`FrozenFrame`]: a session delivers many
/// frames, each of which is alive only until the next one, and each of which may be read
/// back exactly once (§11.3 — "每一步只允许产生一次 GPU→CPU 拷贝"). The read is lazy
/// because a delivered frame is not a wanted frame: the step decides, and the stability
/// window's second look at the *same* frame is not a second transfer.
pub struct ScrollFrame {
    /// The delivered frame, kept alive for as long as its texture is read from.
    ///
    /// The frame pool owns the buffer the texture points at and reclaims it when the
    /// frame is dropped, so this field is a lifetime anchor rather than data.
    #[allow(dead_code)]
    delivered: Option<wgc::WgcFrame>,
    transfer: Box<dyn RegionTransfer>,
    reads: u32,
    read_bytes: u64,
}

impl ScrollFrame {
    /// Wrap a frame delivered by a [`wgc::WgcSession`].
    pub fn from_wgc(device: Arc<GraphicsDevice>, frame: wgc::WgcFrame) -> Self {
        let (width, height) = frame.size();
        let size = (width.max(0) as u32, height.max(0) as u32);
        let texture = frame.texture().clone();
        Self {
            delivered: Some(frame),
            transfer: Box::new(GpuTransfer {
                device,
                texture,
                size,
            }),
            reads: 0,
            read_bytes: 0,
        }
    }

    #[cfg(test)]
    fn from_transfer(transfer: Box<dyn RegionTransfer>) -> Self {
        Self {
            delivered: None,
            transfer,
            reads: 0,
            read_bytes: 0,
        }
    }

    /// The frame's pixel size — the viewport of the step that will read it.
    pub fn size(&self) -> (u32, u32) {
        self.transfer.size()
    }

    pub fn provider(&self) -> ProviderKind {
        ProviderKind::WgcWindow
    }

    /// How many GPU→CPU transfers this frame has paid for. At most one, ever.
    pub fn reads(&self) -> u32 {
        self.reads
    }

    /// Bytes that actually crossed the bus for this frame.
    pub fn read_bytes(&self) -> u64 {
        self.read_bytes
    }

    /// Tightly packed BGRA pixels of one region of this frame.
    ///
    /// `region` is frame-local and is clipped to the frame; a region that cannot supply
    /// a single pixel is an error rather than an empty buffer, so no estimator ever sees
    /// a zero-size frame. Calling this twice is an error too: the second call would be a
    /// second transfer of the same pixels, which §11.3 forbids, and returning a cached
    /// copy would hide that the caller is stepping twice over one frame.
    pub fn read_region(&mut self, region: Rect) -> CaptureResult<Vec<u8>> {
        if self.reads > 0 {
            return Err(CaptureError::InvalidState(
                "this frame has already been read back once; a frame is read once and \
                 then replaced (§11.3) — take the next frame instead of reading this one twice"
                    .into(),
            ));
        }
        let (width, height) = self.transfer.size();
        let frame_rect = Rect::new(0, 0, width as i32, height as i32);
        let clipped = region.intersect(frame_rect);
        if clipped.is_empty() {
            return Err(CaptureError::InvalidState(if region.is_empty() {
                "the region to read is empty".into()
            } else {
                "the region to read does not overlap the captured frame".into()
            }));
        }
        let expected = clipped.width() as usize * clipped.height() as usize * 4;
        let started_at = Instant::now();
        // Counted before the call: a transfer that comes back short still happened, and
        // the caller must not be able to retry it on this frame.
        self.reads += 1;
        let pixels = self
            .transfer
            .transfer(clipped)
            .map_err(|message| classify_device_error("GPU region readback", message))?;
        self.read_bytes += pixels.len() as u64;
        if pixels.len() != expected {
            return Err(CaptureError::CaptureFailed(format!(
                "region readback returned {} bytes, expected {expected}",
                pixels.len()
            )));
        }
        eprintln!(
            "[snapclip][capture] scroll region read provider={} frame={}x{} region=({},{})-{}x{} bytes={} elapsed_ms={}",
            self.provider().name(),
            width,
            height,
            clipped.left,
            clipped.top,
            clipped.width(),
            clipped.height(),
            pixels.len(),
            started_at.elapsed().as_millis()
        );
        Ok(pixels)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        attempt_order, classify_device_error, crop_pixel_rows, CaptureProviders, FrozenFrame,
        FrozenFramePixels, GraphicsDevice, ProviderKind, RegionTransfer, ScrollFrame,
    };
    use crate::CaptureError;
    use crate::ports::PixelSliceSource;
    use crate::geometry::Rect;

    /// Shorthand for the shared test frame, which lives outside `tests` so the export
    /// worker can build an `Arc<FrozenFrame>` with no GPU either.
    fn frozen(width: u32, height: u32) -> FrozenFrame {
        super::test_frozen_frame(width, height)
    }

    #[test]
    fn pixel_slice_crops_rows_without_padding() {
        let frame = frozen(4, 3);
        let source = FrozenFramePixels::new(&frame);
        let pixels = source.read_bgra(Rect::new(1, 1, 3, 3)).unwrap();
        assert_eq!(
            pixels,
            vec![1, 1, 0, 255, 2, 1, 0, 255, 1, 2, 0, 255, 2, 2, 0, 255]
        );
    }

    #[test]
    fn pixel_slice_clips_out_of_range_regions() {
        let frame = frozen(4, 3);
        let source = FrozenFramePixels::new(&frame);
        let pixels = source.read_bgra(Rect::new(2, 2, 100, 100)).unwrap();
        assert_eq!(pixels, vec![2, 2, 0, 255, 3, 2, 0, 255]);
        assert!(source.read_bgra(Rect::new(50, 50, 60, 60)).is_err());
    }

    #[test]
    fn read_region_only_transfers_the_selection() {
        // The Phase 3 acceptance criterion in CPU form: a 2x1 read of a 4x3 frame
        // hands back exactly 8 bytes and never materialises the frame around it.
        let frame = frozen(4, 3);
        let pixels = frame.read_region(Rect::new(1, 2, 3, 3)).unwrap();
        assert_eq!(pixels, vec![1, 2, 0, 255, 2, 2, 0, 255]);
        assert_eq!(pixels.len(), 2 * 4, "a 2x1 selection is exactly two BGRA pixels");
    }

    #[test]
    fn read_region_clips_a_selection_hanging_off_the_right_and_bottom() {
        let frame = frozen(4, 3);
        let pixels = frame.read_region(Rect::new(3, 2, 40, 40)).unwrap();
        assert_eq!(pixels, vec![3, 2, 0, 255]);
    }

    #[test]
    fn read_region_clips_a_negative_origin_to_the_frame() {
        // Monitor-local regions start at (0, 0), but a drag begun off-screen must not
        // index the pixel buffer with a negative row/column: the region is clipped
        // first, then cropped.
        let frame = frozen(4, 3);
        // `right`/`bottom` are exclusive, so a (-5,-5)-(1,1) request overlaps the
        // frame's half-open [0,1)x[0,1) on exactly one pixel: clipping must produce
        // that pixel, never index the buffer with a negative row or column.
        let pixels = frame.read_region(Rect::new(-5, -5, 1, 1)).unwrap();
        assert_eq!(pixels, vec![0, 0, 0, 255]);
        // A region clipped down to nothing is refused, not silently empty.
        assert!(matches!(
            frame.read_region(Rect::new(-5, -5, -1, -1)),
            Err(CaptureError::InvalidState(_))
        ));
        // Fully off-frame in the positive direction as well.
        assert!(matches!(
            frame.read_region(Rect::new(-8, -8, -4, -4)),
            Err(CaptureError::InvalidState(_))
        ));
    }

    #[test]
    fn crop_handles_a_padded_row_pitch() {
        // A staging texture whose `RowPitch` exceeds `width × 4` is the normal case on
        // real hardware; cropping must skip the padding instead of shifting rows.
        // Frame: 2x2, stride 12 (2 pixels + 4 bytes of padding).
        let padded = vec![
            10, 10, 0, 255, 11, 10, 0, 255, 0, 0, 0, 0, // row 0 + padding
            12, 10, 0, 255, 13, 10, 0, 255, 0, 0, 0, 0, // row 1 + padding
        ];
        let cropped = crop_pixel_rows(&padded, 12, Rect::new(1, 1, 2, 2)).unwrap();
        assert_eq!(cropped, vec![13, 10, 0, 255]);
        let whole = crop_pixel_rows(&padded, 12, Rect::new(0, 0, 2, 2)).unwrap();
        assert_eq!(
            whole,
            vec![10, 10, 0, 255, 11, 10, 0, 255, 12, 10, 0, 255, 13, 10, 0, 255]
        );
    }

    #[test]
    fn crop_rejects_a_buffer_shorter_than_the_selection() {
        let padded = vec![10, 10, 0, 255, 11, 10, 0, 255, 0, 0, 0, 0];
        // Two rows demanded, only one present.
        assert!(crop_pixel_rows(&padded, 12, Rect::new(0, 0, 2, 2)).is_none());
        // Stride narrower than the region can never be cropped.
        assert!(crop_pixel_rows(&padded, 4, Rect::new(0, 0, 2, 1)).is_none());
        // Degenerate regions are rejected rather than producing an empty success.
        assert!(crop_pixel_rows(&padded, 12, Rect::new(0, 0, 0, 4)).is_none());
        assert!(crop_pixel_rows(&padded, 12, Rect::new(-1, 0, 4, 4)).is_none());
    }

    #[test]
    fn provider_names_are_stable_for_events() {
        assert_eq!(ProviderKind::Wgc.name(), "wgc");
        assert_eq!(ProviderKind::BitBlt.name(), "bitblt");
    }

    #[test]
    fn wgc_is_tried_before_the_bitblt_fallback() {
        // A WGC-preferred session must attempt BitBlt as well, so a first-frame
        // timeout cannot fail the session outright (docs/11 §3.2).
        assert_eq!(
            attempt_order(Some(ProviderKind::Wgc)),
            vec![ProviderKind::Wgc, ProviderKind::BitBlt]
        );
    }

    #[test]
    fn bitblt_is_the_only_provider_once_wgc_is_dropped() {
        // After a WGC failure the preferred provider is demoted to BitBlt; the
        // next session must not re-attempt the broken WGC path, and a machine
        // that never had WGC goes straight to BitBlt.
        assert_eq!(attempt_order(Some(ProviderKind::BitBlt)), vec![ProviderKind::BitBlt]);
        assert_eq!(attempt_order(None), vec![ProviderKind::BitBlt]);
    }

    #[test]
    fn a_first_frame_timeout_is_a_fallback_eligible_failure() {
        // `wgc::next_frame` yields this message when the first frame never lands.
        // The trailing code is a wait timeout (DXGI_ERROR_WAIT_TIMEOUT), not a
        // device-lost code, so it must be reported as a plain capture failure that
        // lets `capture` fall back to BitBlt rather than tearing down the device.
        let wait_timeout = 0x887A_0027u32 as i32;
        let message = format!(
            "Windows Graphics Capture produced no frame within 1500ms (TryGetNextFrame failed (timed out) #code={wait_timeout})"
        );
        assert!(
            !GraphicsDevice::is_device_lost(&message),
            "a wait timeout must not be mistaken for device removal"
        );
        assert!(matches!(
            classify_device_error("Windows Graphics Capture", message),
            CaptureError::CaptureFailed(_)
        ));
    }

    #[test]
    fn a_device_lost_hresult_is_classified_as_device_removal() {
        // DXGI_ERROR_DEVICE_REMOVED must surface as `DeviceRemoved` so the overlay
        // invalidates providers and rebuilds the device on the next session
        // (docs/11 §2.2 device-removal cleanup path).
        let removed = 0x887A_0005u32 as i32;
        let message = format!("Windows Graphics Capture failed (device removed) #code={removed}");
        assert!(GraphicsDevice::is_device_lost(&message));
        let classified = classify_device_error("Windows Graphics Capture", message);
        assert!(matches!(classified, CaptureError::DeviceRemoved(_)));
        assert!(FrozenFrame::is_device_lost(&classified));
    }

    #[test]
    fn providers_probe_and_capture_a_real_monitor_when_available() {
        let _ = super::super::monitor::set_per_monitor_v2_awareness();
        let Ok(mut providers) = CaptureProviders::new() else {
            return;
        };
        let Ok(monitor) = super::super::monitor::captured_monitor_at_cursor() else {
            return;
        };
        match providers.capture(&monitor) {
            Ok(frame) => {
                assert_eq!(frame.frame.width, monitor.width());
                assert_eq!(frame.frame.height, monitor.height());
                assert!(
                    frame.texture().is_some(),
                    "capture must produce a GPU texture"
                );
                // Arming a session must not transfer the frame back to the CPU; the
                // pixels are produced lazily, only for an actual artifact export.
                assert!(
                    !frame.pixels_read(),
                    "capturing must not read the frame back to the CPU"
                );
                let pixels = frame.pixels().expect("readback must succeed on demand");
                assert_eq!(
                    pixels.len(),
                    (frame.frame.width * frame.frame.height * 4) as usize
                );
                assert!(frame.pixels_read(), "readback must be cached");
            }
            Err(error) => eprintln!("capture unavailable in this session: {error}"),
        }
    }

    /// The hard Phase 3 gate: on real hardware a small selection must not pull the
    /// whole monitor across the bus.
    #[test]
    fn a_300x200_selection_reads_back_only_its_own_bytes() {
        let _ = super::super::monitor::set_per_monitor_v2_awareness();
        let Ok(mut providers) = CaptureProviders::new() else {
            eprintln!("no D3D11 device in this session; skipping the region readback check");
            return;
        };
        let Ok(monitor) = super::super::monitor::captured_monitor_at_cursor() else {
            eprintln!("no monitor at the cursor; skipping the region readback check");
            return;
        };
        let Ok(frozen) = providers.capture(&monitor) else {
            eprintln!("capture unavailable in this session; skipping the region readback check");
            return;
        };
        // WGC frames have no CPU pixels, so this exercises `CopySubresourceRegion`.
        // A BitBlt frame is the CPU crop; both must return the selection's bytes.
        let selection = Rect::new(0, 0, 300.min(monitor.width() as i32), 200.min(monitor.height() as i32));
        let pixels = frozen.read_region(selection).unwrap();
        assert_eq!(
            pixels.len(),
            300 * 200 * 4,
            "readback bytes must be the selection's, not the monitor's"
        );
        assert!(
            !frozen.pixels_read(),
            "a region readback must never materialise the full frame"
        );
    }

    // --- the scroll path reads regions (§11.3, §24.2; task P2.02) ---------------

    /// Records what the device was asked for, so the *request* can be asserted and
    /// not only its result.
    ///
    /// A real transfer would need a GPU; the policy under test here is which rect is
    /// asked for, whether a frame is read at all before a step wants it, and how many
    /// bytes are accounted — all three are visible from outside the transfer.
    struct RegionRecorder {
        size: (u32, u32),
        calls: std::sync::Arc<std::sync::Mutex<Vec<Rect>>>,
    }

    impl RegionTransfer for RegionRecorder {
        fn size(&self) -> (u32, u32) {
            self.size
        }

        fn transfer(&mut self, region: Rect) -> Result<Vec<u8>, String> {
            self.calls.lock().unwrap().push(region);
            let bytes = region.width() as usize * region.height() as usize * 4;
            Ok(vec![0u8; bytes])
        }
    }

    fn recording_frame(
        size: (u32, u32),
    ) -> (ScrollFrame, std::sync::Arc<std::sync::Mutex<Vec<Rect>>>) {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let frame = ScrollFrame::from_transfer(Box::new(RegionRecorder {
            size,
            calls: calls.clone(),
        }));
        (frame, calls)
    }

    #[test]
    fn a_scroll_source_reads_a_region_instead_of_the_whole_frame() {
        let (mut frame, calls) = recording_frame((1280, 960));
        assert_eq!(frame.size(), (1280, 960));
        assert_eq!(frame.provider(), ProviderKind::WgcWindow);
        // Receiving a frame is not reading it: the transfer happens when a step asks
        // for the region it needs (F-01 — a step only needs the newly revealed band).
        assert_eq!(frame.reads(), 0, "delivering a frame must not read it back");

        let viewport = Rect::new(0, 0, 1280, 960);
        let pixels = frame
            .read_region(viewport)
            .expect("the viewport of a delivered frame must read back");
        assert_eq!(pixels.len(), 1280 * 960 * 4);
        assert_eq!(frame.reads(), 1);
        assert_eq!(frame.read_bytes(), (1280 * 960 * 4) as u64);
        assert_eq!(calls.lock().unwrap().as_slice(), &[viewport]);

        // A region smaller than the frame costs the region's bytes, and the request
        // the device receives is that region — never the frame around it.
        let (mut frame, calls) = recording_frame((1280, 960));
        let band = Rect::new(0, 700, 1280, 960);
        let pixels = frame.read_region(band).unwrap();
        assert_eq!(pixels.len(), 1280 * 260 * 4);
        assert!(pixels.len() < (1280 * 960 * 4) / 2);
        assert_eq!(calls.lock().unwrap().as_slice(), &[band]);

        // A region hanging off the frame is clipped to it, and the clipped rect is
        // what the device is asked for.
        let (mut frame, calls) = recording_frame((1280, 960));
        let pixels = frame.read_region(Rect::new(1200, 900, 1400, 1100)).unwrap();
        assert_eq!(pixels.len(), 80 * 60 * 4);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[Rect::new(1200, 900, 1280, 960)]
        );

        // A region with no overlap is refused *before* the transfer: an empty buffer
        // must never reach the estimator as if it were a frame.
        let (mut frame, calls) = recording_frame((1280, 960));
        assert!(frame.read_region(Rect::new(2000, 2000, 10, 10)).is_err());
        assert!(frame.read_region(Rect::new(0, 0, 0, 10)).is_err());
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(frame.reads(), 0, "a refused region must not reach the device");
        assert_eq!(frame.read_bytes(), 0);
    }

    #[test]
    fn reading_the_same_region_twice_is_an_error_or_a_cache_hit_but_never_a_second_gpu_transfer() {
        let (mut frame, calls) = recording_frame((1280, 960));
        let viewport = Rect::new(0, 0, 1280, 960);
        frame.read_region(viewport).expect("the first read is the step's");

        let second = frame.read_region(viewport);
        assert!(
            matches!(second, Err(CaptureError::InvalidState(_))),
            "a second read of the same frame must be refused, not silently repeated"
        );
        assert_eq!(
            calls.lock().unwrap().len(),
            1,
            "the refused read must never reach the device"
        );
        assert_eq!(frame.reads(), 1);
        assert_eq!(
            frame.read_bytes(),
            (1280 * 960 * 4) as u64,
            "the byte account must not count a transfer that did not happen"
        );
    }

    #[test]
    fn one_hundred_delivered_frames_cost_one_hundred_region_reads() {
        // §30.1's "每步仅一次回读" row, in the form the capture layer can check on its
        // own: 100 frames in, 100 transfers, no matter how many steps asked.
        let mut transfers = 0u32;
        for _ in 0..100 {
            let (mut frame, calls) = recording_frame((1280, 960));
            frame.read_region(Rect::new(0, 0, 1280, 960)).unwrap();
            transfers += calls.lock().unwrap().len() as u32;
        }
        assert_eq!(transfers, 100);
    }

    #[test]
    fn a_short_transfer_is_an_error_rather_than_a_truncated_frame() {
        // A transfer that returns fewer bytes than the claim asked for must not reach
        // the estimator: a short buffer would be read as if its rows were the frame's.
        let (mut frame, _calls) = recording_frame((1280, 960));
        let short = frame.read_region(Rect::new(0, 0, 4, 4)).unwrap();
        assert_eq!(short.len(), 4 * 4 * 4);
        assert!(frame.read_region(Rect::new(0, 0, 4, 4)).is_err());
    }

    #[test]
    fn the_window_backend_is_not_a_monitor_backend() {
        // `ProviderKind::WgcWindow` exists so the scroll path has a name for its own
        // backend; it is not an alternative monitor provider, and asking the monitor
        // path for it must fail loudly instead of silently reading the desktop.
        assert_eq!(ProviderKind::WgcWindow.name(), "wgc-window");
        let device = match GraphicsDevice::create() {
            Ok(device) => std::sync::Arc::new(device),
            Err(message) => {
                panic!("P2.02 needs a D3D11 device to check the monitor/window split: {message}")
            }
        };
        let mut providers = CaptureProviders {
            device,
            preferred: Some(ProviderKind::WgcWindow),
            diagnostics: Vec::new(),
        };
        let monitor = super::super::monitor::captured_monitor_at_cursor();
        if let Ok(monitor) = monitor {
            let error = providers
                .capture(&monitor)
                .expect_err("the window backend must not serve a monitor capture");
            assert!(
                matches!(error, CaptureError::ProviderUnavailable(_)),
                "expected ProviderUnavailable, got {error:?}"
            );
        }
    }
}





