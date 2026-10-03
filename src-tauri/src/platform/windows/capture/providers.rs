//! Frame providers: Windows Graphics Capture first, BitBlt as the compatibility
//! fallback. Both hand back one frozen frame per session.

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use ::windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

use crate::capture::application::PixelSliceSource;
use crate::capture::geometry::Rect;
use crate::capture::session::CapturedFrame;
use crate::capture::{CaptureError, CaptureResult};
use crate::domain::PixelFormat;

use super::monitor::CapturedMonitor;
use super::win::bitblt;
use super::win::d3d11::{GraphicsDevice, GpuFrame};
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
    pixels: OnceLock<Vec<u8>>,
}

impl FrozenFrame {
    /// The GPU texture of the frozen frame, if the provider produced one.
    #[allow(dead_code)]
    pub fn texture(&self) -> Option<&GpuFrame> {
        self.texture.as_ref()
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
    /// This is the single GPU → CPU transfer on the capture path; it is deferred to
    /// the moment the artifact is produced rather than paid while arming the session.
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
        let pixels = device
            .read_back_bgra(&gpu.texture)
            .map_err(|message| classify_device_error("GPU readback", message))?;
        // A racing reader may have won; either buffer is equivalent.
        Ok(self.pixels.get_or_init(|| pixels))
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
        let frame_rect = self.frame.frame.rect();
        let clipped = region.intersect(frame_rect);
        if clipped.is_empty() {
            return Err(CaptureError::InvalidState(
                "artifact region does not overlap the frozen frame".into(),
            ));
        }
        // Triggers the single deferred readback on the first crop.
        let pixels = self.frame.pixels()?;
        let stride = self.frame.frame.width as usize * 4;
        let mut output = Vec::with_capacity(clipped.area() as usize * 4);
        for row in clipped.top..clipped.bottom {
            let start = row as usize * stride + clipped.left as usize * 4;
            let end = start + clipped.width() as usize * 4;
            if end > pixels.len() {
                return Err(CaptureError::CaptureFailed(
                    "frozen frame pixel buffer is shorter than its geometry".into(),
                ));
            }
            output.extend_from_slice(&pixels[start..end]);
        }
        Ok(output)
    }
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

    /// Shared device used by WGC textures and the overlay renderer.
    pub fn shared_device(&self) -> Arc<GraphicsDevice> {
        self.device.clone()
    }

    /// Capture one frozen frame of `monitor`.
    ///
    /// Tries the preferred provider first and falls back to BitBlt when WGC is
    /// unavailable, so a machine without WGC still gets a working screenshot.
    pub fn capture(&mut self, monitor: &CapturedMonitor) -> CaptureResult<FrozenFrame> {
        let started_at = Instant::now();
        let mut attempts = Vec::new();
        if self.preferred == Some(ProviderKind::Wgc) {
            attempts.push(ProviderKind::Wgc);
        }
        attempts.push(ProviderKind::BitBlt);

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
                ))
            }
            ProviderKind::BitBlt => {
                let captured = bitblt::capture_monitor(monitor)
                    .map_err(CaptureError::CaptureFailed)?;
                // BitBlt hands back CPU pixels, so upload them once to get the same
                // GPU-resident L0 representation the WGC path has.
                let gpu = self
                    .device
                    .create_bgra_texture(captured.width, captured.height, &captured.pixels)
                    .map_err(|message| classify_device_error("texture upload", message))?;
                Ok(frozen_frame(
                    monitor,
                    ProviderKind::BitBlt,
                    Some(gpu),
                    self.device.clone(),
                ))
            }
        }
    }
}

fn frozen_frame(
    monitor: &CapturedMonitor,
    provider: ProviderKind,
    texture: Option<GpuFrame>,
    device: Arc<GraphicsDevice>,
) -> FrozenFrame {
    FrozenFrame {
        frame: CapturedFrame {
            width: monitor.width(),
            height: monitor.height(),
            pixel_format: PixelFormat::Bgra8Unorm,
            captured_at_unix_ms: crate::application::clipboard_ingest::unix_time_ms(),
            provider: provider.name(),
        },
        monitor: monitor.clone(),
        provider,
        texture,
        device: Some(device),
        pixels: OnceLock::new(),
    }
}

fn classify_device_error(context: &str, message: String) -> CaptureError {
    if GraphicsDevice::is_device_lost(&message) {
        CaptureError::DeviceRemoved(format!("{context}: {message}"))
    } else {
        CaptureError::CaptureFailed(format!("{context}: {message}"))
    }
}

#[cfg(test)]
mod tests {
    use super::{CaptureProviders, FrozenFrame, FrozenFramePixels, ProviderKind};
    use crate::capture::application::PixelSliceSource;
    use crate::capture::geometry::{MonitorLayout, Point, Rect};
    use crate::capture::session::CapturedFrame;
    use crate::domain::PixelFormat;
    use std::sync::OnceLock;

    /// A frame whose pixels are already materialised, so the pure cropping logic can be
    /// exercised without a GPU.
    fn frozen(width: u32, height: u32) -> FrozenFrame {
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
            monitor: super::CapturedMonitor {
                handle: 0,
                layout: MonitorLayout {
                    bounds: Rect::from_origin_size(Point::new(0, 0), width as i32, height as i32),
                    work_area: Rect::from_origin_size(Point::new(0, 0), width as i32, height as i32),
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
    fn provider_names_are_stable_for_events() {
        assert_eq!(ProviderKind::Wgc.name(), "wgc");
        assert_eq!(ProviderKind::BitBlt.name(), "bitblt");
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
}






