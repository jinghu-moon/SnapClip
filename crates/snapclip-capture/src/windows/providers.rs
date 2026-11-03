//! Frame providers: Windows Graphics Capture first, BitBlt as the compatibility
//! fallback. Both hand back one frozen frame per session.

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use ::windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

use crate::geometry::Rect;
use crate::ports::PixelSliceSource;
use crate::session::CapturedFrame;
use crate::{CaptureError, CaptureResult};
use snapclip_model::pixels::{PixelBuffer, PixelDescriptor, PixelSource};
use std::sync::atomic::{AtomicU64, Ordering};

use super::color_output::ColorSnapshot;
use super::monitor::CapturedMonitor;
use super::win::bitblt;
use super::win::d3d11::{GpuFrame, GraphicsDevice};
use super::win::wgc;

/// Which provider produced a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Wgc,
    BitBlt,
}

impl ProviderKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Wgc => "wgc",
            Self::BitBlt => "bitblt",
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
    pixels: OnceLock<PixelBuffer>,
    color_snapshot: ColorSnapshot,
}

impl FrozenFrame {
    pub fn color_snapshot(&self) -> &ColorSnapshot {
        &self.color_snapshot
    }
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
    pub fn pixels(&self) -> CaptureResult<&PixelBuffer> {
        if let Some(pixels) = self.pixels.get() {
            return Ok(pixels);
        }
        let pixels = self.read_region(self.frame.rect())?;
        Ok(self.pixels.get_or_init(|| pixels))
    }

    /// Read only the requested rectangle, preserving the frozen source definition.
    pub fn read_region(&self, region: Rect) -> CaptureResult<PixelBuffer> {
        let clipped = region.intersect(self.frame.rect());
        if clipped.is_empty() {
            return Err(CaptureError::InvalidState(
                "artifact region does not overlap the frozen frame".into(),
            ));
        }
        let descriptor = self.frame.source.descriptor();
        let width = clipped.width() as u32;
        let height = clipped.height() as u32;
        if let Some(pixels) = self.pixels.get() {
            if pixels.descriptor() != descriptor
                || pixels.size() != [self.frame.width, self.frame.height]
            {
                return Err(CaptureError::CaptureFailed(
                    "frozen CPU pixels do not match their source".into(),
                ));
            }
            let bytes = crop_pixel_rows(
                pixels.as_bytes(),
                pixels.row_stride(),
                descriptor.bytes_per_pixel(),
                clipped,
            )
            .ok_or_else(|| {
                CaptureError::CaptureFailed("frozen pixel crop is out of bounds".into())
            })?;
            return PixelBuffer::from_tight(width, height, descriptor, bytes)
                .map_err(|message| CaptureError::CaptureFailed(message.into()));
        }
        let gpu = self.texture.as_ref().ok_or_else(|| {
            CaptureError::CaptureFailed("frozen frame has neither a texture nor pixels".into())
        })?;
        let device = self.device.as_ref().ok_or_else(|| {
            CaptureError::CaptureFailed("frozen frame has a texture but no device".into())
        })?;
        let started_at = Instant::now();
        let pixels = device
            .read_back_pixels(
                &gpu.texture,
                [clipped.left as u32, clipped.top as u32, width, height],
                descriptor,
            )
            .map_err(|message| classify_device_error("GPU region readback", message))?;
        eprintln!(
            "[snapclip][capture] region readback provider={} size={}x{} bytes={} elapsed_ms={}",
            self.frame.provider,
            width,
            height,
            pixels.as_bytes().len(),
            started_at.elapsed().as_millis()
        );
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
    fn source(&self) -> PixelSource {
        self.frame.frame.source
    }

    fn read_pixels(&self, region: Rect) -> CaptureResult<PixelBuffer> {
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
fn crop_pixel_rows(
    pixels: &[u8],
    stride: usize,
    bytes_per_pixel: usize,
    region: Rect,
) -> Option<Vec<u8>> {
    let width = usize::try_from(region.width()).ok()?;
    let height = usize::try_from(region.height()).ok()?;
    let left = usize::try_from(region.left).ok()?;
    let top = usize::try_from(region.top).ok()?;
    let row_bytes = width.checked_mul(bytes_per_pixel)?;
    let left_bytes = left.checked_mul(bytes_per_pixel)?;
    if width == 0 || height == 0 || left_bytes.checked_add(row_bytes)? > stride {
        return None;
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(row_bytes.checked_mul(height)?)
        .ok()?;
    for row in 0..height {
        let source_row = top.checked_add(row)?;
        let start = source_row.checked_mul(stride)?.checked_add(left_bytes)?;
        let end = start.checked_add(row_bytes)?;
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
        // Existing UI explicitly requests compatibility SDR until every consumer
        // has a high-precision path. This is not an HDR fallback.
        self.capture_impl(monitor, false)
    }

    /// Acquire without discarding Advanced Color. The UI must not call this
    /// until its export, scroll and display consumers accept the typed source.
    pub fn capture_preserving_color(
        &mut self,
        monitor: &CapturedMonitor,
    ) -> CaptureResult<FrozenFrame> {
        self.capture_impl(monitor, true)
    }

    fn capture_impl(
        &mut self,
        monitor: &CapturedMonitor,
        preserve_color: bool,
    ) -> CaptureResult<FrozenFrame> {
        let started_at = Instant::now();
        let snapshot = ColorSnapshot::query(monitor.handle).unwrap_or_else(|error| {
            self.diagnostics
                .push(format!("display color unavailable: {error}"));
            ColorSnapshot::unknown(monitor.handle)
        });
        let descriptor = if preserve_color {
            snapshot.capture_descriptor()
        } else {
            PixelDescriptor::bgra8()
        };
        let attempts = attempt_order(self.preferred, descriptor);

        let mut last_error: Option<CaptureError> = None;
        for provider in attempts {
            eprintln!(
                "[snapclip][capture] provider attempt={} monitor={}x{}",
                provider.name(),
                monitor.width(),
                monitor.height()
            );
            match self.capture_with(provider, monitor, descriptor, &snapshot) {
                Ok(frame) => {
                    let after = ColorSnapshot::query(monitor.handle)
                        .unwrap_or_else(|_| ColorSnapshot::unknown(monitor.handle));
                    if snapshot != after {
                        return Err(CaptureError::CaptureFailed(
                            "display color changed during acquisition; retry the capture".into(),
                        ));
                    }
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
                    self.diagnostics
                        .push(format!("{} provider failed: {error}", provider.name()));
                    if descriptor == PixelDescriptor::bgra8() {
                        self.preferred = Some(ProviderKind::BitBlt);
                    }
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
        descriptor: PixelDescriptor,
        color_snapshot: &ColorSnapshot,
    ) -> CaptureResult<FrozenFrame> {
        match provider {
            ProviderKind::Wgc => {
                // No readback here: L0 renders from this texture and the CPU copy is
                // produced lazily only if an artifact is actually exported.
                let gpu =
                    wgc::capture_monitor(&self.device, monitor, descriptor).map_err(|message| {
                        classify_device_error("Windows Graphics Capture", message)
                    })?;
                frozen_frame(
                    monitor,
                    ProviderKind::Wgc,
                    Some(gpu),
                    self.device.clone(),
                    None,
                    descriptor,
                    color_snapshot.clone(),
                )
            }
            ProviderKind::BitBlt => {
                let captured =
                    bitblt::capture_monitor(monitor).map_err(CaptureError::CaptureFailed)?;
                // BitBlt hands back CPU pixels, so upload them once to get the same
                // GPU-resident L0 representation the WGC path has. Phase 3: retain the
                // original CPU pixels so the export path crops them directly instead of
                // reading back from the GPU again.
                let cpu_pixels = captured.pixels.clone();
                let gpu = self
                    .device
                    .create_bgra_texture(captured.width, captured.height, &captured.pixels)
                    .map_err(|message| classify_device_error("texture upload", message))?;
                frozen_frame(
                    monitor,
                    ProviderKind::BitBlt,
                    Some(gpu),
                    self.device.clone(),
                    Some(cpu_pixels),
                    PixelDescriptor::bgra8(),
                    color_snapshot.clone(),
                )
            }
        }
    }
}

fn frozen_frame(
    monitor: &CapturedMonitor,
    provider: ProviderKind,
    texture: Option<GpuFrame>,
    device: Arc<GraphicsDevice>,
    pre_materialized_pixels: Option<Vec<u8>>,
    descriptor: PixelDescriptor,
    color_snapshot: ColorSnapshot,
) -> CaptureResult<FrozenFrame> {
    let pixels = OnceLock::new();
    if let Some(buf) = pre_materialized_pixels {
        let buffer = PixelBuffer::from_tight(monitor.width(), monitor.height(), descriptor, buf)
            .map_err(|message| CaptureError::CaptureFailed(message.into()))?;
        let _ = pixels.set(buffer);
    }
    static SOURCE_GENERATION: AtomicU64 = AtomicU64::new(1);
    Ok(FrozenFrame {
        frame: CapturedFrame {
            width: monitor.width(),
            height: monitor.height(),
            source: PixelSource::new(
                descriptor,
                SOURCE_GENERATION.fetch_add(1, Ordering::Relaxed),
            ),
            captured_at_unix_ms: snapclip_model::unix_time_ms(),
            provider: provider.name(),
            origin: Some(monitor.origin()),
        },
        monitor: monitor.clone(),
        provider,
        texture,
        device: Some(device),
        pixels,
        color_snapshot,
    })
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
            source: snapclip_model::pixels::PixelSource::new(
                snapclip_model::pixels::PixelDescriptor::bgra8(),
                0,
            ),
            captured_at_unix_ms: 1,
            provider: "test",
            origin: Some(crate::geometry::Point::new(0, 0)),
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
        pixels: OnceLock::from(
            PixelBuffer::from_tight(width, height, PixelDescriptor::bgra8(), pixels).unwrap(),
        ),
        color_snapshot: ColorSnapshot::unknown(0),
    }
}

/// Providers tried for one capture, in order.
///
/// WGC is attempted first only when it is the preferred provider; BitBlt is always
/// the final entry so a machine without WGC — or a WGC run whose first frame times
/// out — still falls back to a frame instead of failing the session outright
/// (docs/11 §3.2). Keeping this decision pure makes the fallback contract unit
/// testable without a GPU.
fn attempt_order(
    preferred: Option<ProviderKind>,
    descriptor: PixelDescriptor,
) -> Vec<ProviderKind> {
    let mut attempts = Vec::new();
    if preferred == Some(ProviderKind::Wgc) {
        attempts.push(ProviderKind::Wgc);
    }
    if descriptor == PixelDescriptor::bgra8() {
        attempts.push(ProviderKind::BitBlt);
    }
    attempts
}

#[cfg(test)]
mod tests {
    use super::{
        CaptureProviders, FrozenFrame, FrozenFramePixels, GraphicsDevice, OnceLock, PixelBuffer,
        PixelDescriptor, PixelSource, ProviderKind, attempt_order, classify_device_error,
        crop_pixel_rows,
    };
    use crate::CaptureError;
    use crate::geometry::Rect;
    use crate::ports::PixelSliceSource;

    /// Shorthand for the shared test frame, which lives outside `tests` so the export
    /// worker can build an `Arc<FrozenFrame>` with no GPU either.
    fn frozen(width: u32, height: u32) -> FrozenFrame {
        super::test_frozen_frame(width, height)
    }

    #[test]
    fn pixel_slice_crops_rows_without_padding() {
        let frame = frozen(4, 3);
        let source = FrozenFramePixels::new(&frame);
        let pixels = source.read_pixels(Rect::new(1, 1, 3, 3)).unwrap();
        let pixels = pixels.as_bytes();
        assert_eq!(
            pixels,
            vec![1, 1, 0, 255, 2, 1, 0, 255, 1, 2, 0, 255, 2, 2, 0, 255]
        );
    }

    #[test]
    fn pixel_slice_clips_out_of_range_regions() {
        let frame = frozen(4, 3);
        let source = FrozenFramePixels::new(&frame);
        let pixels = source.read_pixels(Rect::new(2, 2, 100, 100)).unwrap();
        let pixels = pixels.as_bytes();
        assert_eq!(pixels, vec![2, 2, 0, 255, 3, 2, 0, 255]);
        assert!(source.read_pixels(Rect::new(50, 50, 60, 60)).is_err());
    }

    #[test]
    fn read_region_only_transfers_the_selection() {
        // The Phase 3 acceptance criterion in CPU form: a 2x1 read of a 4x3 frame
        // hands back exactly 8 bytes and never materialises the frame around it.
        let frame = frozen(4, 3);
        let pixels = frame.read_region(Rect::new(1, 2, 3, 3)).unwrap();
        let pixels = pixels.as_bytes();
        assert_eq!(pixels, vec![1, 2, 0, 255, 2, 2, 0, 255]);
        assert_eq!(
            pixels.len(),
            2 * 4,
            "a 2x1 selection is exactly two BGRA pixels"
        );
    }

    #[test]
    fn read_region_clips_a_selection_hanging_off_the_right_and_bottom() {
        let frame = frozen(4, 3);
        let pixels = frame.read_region(Rect::new(3, 2, 40, 40)).unwrap();
        let pixels = pixels.as_bytes();
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
        let pixels = pixels.as_bytes();
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
        let cropped = crop_pixel_rows(&padded, 12, 4, Rect::new(1, 1, 2, 2)).unwrap();
        assert_eq!(cropped, vec![13, 10, 0, 255]);
        let whole = crop_pixel_rows(&padded, 12, 4, Rect::new(0, 0, 2, 2)).unwrap();
        assert_eq!(
            whole,
            vec![
                10, 10, 0, 255, 11, 10, 0, 255, 12, 10, 0, 255, 13, 10, 0, 255
            ]
        );
    }

    #[test]
    fn frozen_fp16_crop_retains_encoding_generation_and_negative_bits() {
        use snapclip_model::pixels::{AlphaMode, PixelEncoding};
        let descriptor = PixelDescriptor::new(
            snapclip_model::PixelFormat::Rgba16Float,
            PixelEncoding::ScRgb,
            AlphaMode::Straight,
        )
        .unwrap();
        let pixel = [0, 0xbc, 0, 0x49, 0, 0, 0, 0x3c];
        let mut frame = frozen(4, 3);
        frame.frame.source = PixelSource::new(descriptor, 41);
        frame.pixels =
            OnceLock::from(PixelBuffer::from_tight(4, 3, descriptor, pixel.repeat(12)).unwrap());
        let source = FrozenFramePixels::new(&frame);
        assert_eq!(source.source().generation(), 41);
        let cropped = source.read_pixels(Rect::new(1, 1, 3, 3)).unwrap();
        assert_eq!(cropped.descriptor(), descriptor);
        assert_eq!(cropped.size(), [2, 2]);
        assert_eq!(cropped.as_bytes(), pixel.repeat(4));
        let mut padded = pixel.repeat(2);
        padded.extend_from_slice(&[99; 8]);
        padded.extend_from_slice(&pixel.repeat(2));
        padded.extend_from_slice(&[99; 8]);
        assert_eq!(
            crop_pixel_rows(&padded, 24, 8, Rect::new(1, 1, 2, 2)).unwrap(),
            pixel
        );
        assert!(
            crop_pixel_rows(&padded, 24, 8, Rect::new(2, 0, 4, 1)).is_none(),
            "a crop cannot spill into the following row"
        );
    }

    #[test]
    fn crop_rejects_a_buffer_shorter_than_the_selection() {
        let padded = vec![10, 10, 0, 255, 11, 10, 0, 255, 0, 0, 0, 0];
        // Two rows demanded, only one present.
        assert!(crop_pixel_rows(&padded, 12, 4, Rect::new(0, 0, 2, 2)).is_none());
        // Stride narrower than the region can never be cropped.
        assert!(crop_pixel_rows(&padded, 4, 4, Rect::new(0, 0, 2, 1)).is_none());
        // Degenerate regions are rejected rather than producing an empty success.
        assert!(crop_pixel_rows(&padded, 12, 4, Rect::new(0, 0, 0, 4)).is_none());
        assert!(crop_pixel_rows(&padded, 12, 4, Rect::new(-1, 0, 4, 4)).is_none());
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
            attempt_order(Some(ProviderKind::Wgc), PixelDescriptor::bgra8()),
            vec![ProviderKind::Wgc, ProviderKind::BitBlt]
        );
    }

    #[test]
    fn bitblt_is_the_only_provider_once_wgc_is_dropped() {
        // After a WGC failure the preferred provider is demoted to BitBlt; the
        // next session must not re-attempt the broken WGC path, and a machine
        // that never had WGC goes straight to BitBlt.
        assert_eq!(
            attempt_order(Some(ProviderKind::BitBlt), PixelDescriptor::bgra8()),
            vec![ProviderKind::BitBlt]
        );
        assert_eq!(
            attempt_order(None, PixelDescriptor::bgra8()),
            vec![ProviderKind::BitBlt]
        );
    }

    #[test]
    fn high_precision_never_falls_back_to_bitblt() {
        use snapclip_model::pixels::{AlphaMode, PixelEncoding};
        let descriptor = PixelDescriptor::new(
            snapclip_model::PixelFormat::Rgba16Float,
            PixelEncoding::ScRgb,
            AlphaMode::Straight,
        )
        .unwrap();
        assert_eq!(
            attempt_order(Some(ProviderKind::Wgc), descriptor),
            vec![ProviderKind::Wgc]
        );
        assert!(attempt_order(Some(ProviderKind::BitBlt), descriptor).is_empty());
        assert!(attempt_order(None, descriptor).is_empty());
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

    // The name says `when_available` because three ledgers quote it verbatim (`docs/09` line 121,
    // `docs/30` section 21.5 and `docs/31` R-21); it is `#[ignore]`d rather than skipped, so the
    // suffix is now only a name.
    #[test]
    #[ignore = "L3 (docs/31 D-14): probes and captures the monitor at the cursor; run with --ignored --test-threads=1 on a live desktop"]
    fn providers_probe_and_capture_a_real_monitor_when_available() {
        let _ = super::super::monitor::set_per_monitor_v2_awareness();
        let mut providers =
            CaptureProviders::new().expect("this test needs a D3D11 device (docs/31 D-14)");
        let monitor = super::super::monitor::captured_monitor_at_cursor()
            .expect("this test needs a monitor at the cursor (docs/31 D-14)");
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
                    pixels.as_bytes().len(),
                    (frame.frame.width * frame.frame.height * 4) as usize
                );
                assert!(frame.pixels_read(), "readback must be cached");
            }
            // `--ignored` is a request for a live desktop; a capture that refuses there is a
            // failure of the environment the run claimed, not a reason to pass (docs/31 D-14).
            Err(error) => panic!("capture refused on the desktop this test asked for: {error}"),
        }
    }

    /// The hard Phase 3 gate: on real hardware a small selection must not pull the
    /// whole monitor across the bus.
    #[test]
    #[ignore = "L3 (docs/31 D-14): reads a region back from the monitor at the cursor; run with --ignored --test-threads=1 on a live desktop"]
    fn a_300x200_selection_reads_back_only_its_own_bytes() {
        let _ = super::super::monitor::set_per_monitor_v2_awareness();
        let mut providers =
            CaptureProviders::new().expect("this test needs a D3D11 device (docs/31 D-14)");
        let monitor = super::super::monitor::captured_monitor_at_cursor()
            .expect("this test needs a monitor at the cursor (docs/31 D-14)");
        let frozen = providers.capture(&monitor).unwrap_or_else(|error| {
            panic!("capture refused on the desktop this test asked for: {error}")
        });
        // WGC frames have no CPU pixels, so this exercises `CopySubresourceRegion`.
        // A BitBlt frame is the CPU crop; both must return the selection's bytes.
        let selection = Rect::new(
            0,
            0,
            300.min(monitor.width() as i32),
            200.min(monitor.height() as i32),
        );
        let pixels = frozen.read_region(selection).unwrap();
        let pixels = pixels.as_bytes();
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
}
