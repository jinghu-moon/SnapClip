//! Windows Graphics Capture provider.
//!
//! Preferred provider: it returns the composited desktop as a D3D11 texture, so the
//! overlay never has to touch CPU pixels for the back buffer. One frame is pulled
//! per session — the tasklist explicitly freezes the back buffer before the overlay
//! becomes visible so the overlay cannot appear inside its own screenshot.

use std::time::{Duration, Instant};

use ::windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use ::windows::Graphics::DirectX::DirectXPixelFormat;
use ::windows::Graphics::SizeInt32;
use ::windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use ::windows::Win32::Graphics::Dxgi::IDXGIDevice;
use ::windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use ::windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use ::windows::core::{Interface, factory};

use super::super::monitor;
use super::d3d11::{GraphicsDevice, GpuFrame};

/// How long to wait for the first frame before falling back to another provider.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_millis(1500);
const FRAME_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Whether this Windows build exposes Windows Graphics Capture at all.
///
/// Probes the API instead of trusting a version number: the type can exist while
/// the runtime refuses to hand out a frame pool (some virtual machines and remote
/// sessions).
pub fn is_supported() -> bool {
    GraphicsCaptureSession::IsSupported().unwrap_or(false)
}

/// Capture one frame of `layout` through WGC.
///
/// The caller keeps the returned texture alive for the whole session; the WGC frame
/// pool and session are dropped as soon as the pixels have been copied, and the
/// session's own reference keeps the texture valid afterwards.
pub fn capture_monitor(
    device: &GraphicsDevice,
    layout: &monitor::CapturedMonitor,
) -> Result<GpuFrame, String> {
    if !is_supported() {
        return Err("Windows Graphics Capture is not supported on this system".into());
    }

    let item = create_item_for_monitor(layout.handle)?;
    let size = SizeInt32 {
        Width: layout.width() as i32,
        Height: layout.height() as i32,
    };

    let dxgi_device: IDXGIDevice = device
        .device()
        .cast()
        .map_err(|error| super::hresult("IDXGIDevice::cast", &error))?;
    let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device) }
        .map_err(|error| super::hresult("CreateDirect3D11DeviceFromDXGIDevice", &error))?;
    let direct3d_device: ::windows::Graphics::DirectX::Direct3D11::IDirect3DDevice = inspectable
        .cast()
        .map_err(|error| super::hresult("IDirect3DDevice::cast", &error))?;

    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &direct3d_device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        2,
        size,
    )
    .map_err(|error| super::hresult("Direct3D11CaptureFramePool::CreateFreeThreaded", &error))?;
    let session = pool
        .CreateCaptureSession(&item)
        .map_err(|error| super::hresult("CreateCaptureSession", &error))?;
    session
        .SetIsCursorCaptureEnabled(false)
        .map_err(|error| super::hresult("SetIsCursorCaptureEnabled", &error))?;
    // The yellow capture border is cosmetic: report but never fail on it.
    if let Err(error) = session.SetIsBorderRequired(false) {
        eprintln!("[snapclip][capture] WGC border suppression unavailable: {error}");
    }
    session
        .StartCapture()
        .map_err(|error| super::hresult("GraphicsCaptureSession::StartCapture", &error))?;

    let frame = next_frame(&pool)?;
    let surface = frame
        .Surface()
        .map_err(|error| super::hresult("Direct3D11CaptureFrame::Surface", &error))?;
    let access: IDirect3DDxgiInterfaceAccess = surface
        .cast()
        .map_err(|error| super::hresult("IDirect3DDxgiInterfaceAccess::cast", &error))?;
    let texture: ID3D11Texture2D = unsafe { access.GetInterface() }
        .map_err(|error| super::hresult("IDirect3DDxgiInterfaceAccess::GetInterface", &error))?;

    // Keep our own reference, then tear the capture session down immediately.
    session
        .Close()
        .map_err(|error| super::hresult("GraphicsCaptureSession::Close", &error))?;
    pool.Close()
        .map_err(|error| super::hresult("Direct3D11CaptureFramePool::Close", &error))?;

    Ok(GpuFrame {
        texture,
        width: layout.width(),
        height: layout.height(),
    })
}

fn next_frame(
    pool: &Direct3D11CaptureFramePool,
) -> Result<windows::Graphics::Capture::Direct3D11CaptureFrame, String> {
    let deadline = Instant::now() + FIRST_FRAME_TIMEOUT;
    let mut last_error = String::new();
    while Instant::now() < deadline {
        match pool.TryGetNextFrame() {
            Ok(frame) => return Ok(frame),
            Err(error) => last_error = super::hresult("TryGetNextFrame", &error),
        }
        std::thread::sleep(FRAME_POLL_INTERVAL);
    }
    Err(format!(
        "Windows Graphics Capture produced no frame within {}ms ({last_error})",
        FIRST_FRAME_TIMEOUT.as_millis()
    ))
}

/// Build a capture item for a monitor.
///
/// The monitor handle arrives as an `isize` because `monitor::CapturedMonitor`
/// stores the `windows-sys` handle while the WinRT interop needs the `windows`
/// handle; both are the same `*mut c_void` at the ABI level.
fn create_item_for_monitor(handle: isize) -> Result<GraphicsCaptureItem, String> {
    let interop: IGraphicsCaptureItemInterop =
        factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
            .map_err(|error| super::hresult("GraphicsCaptureItem interop factory", &error))?;
    let monitor = ::windows::Win32::Graphics::Gdi::HMONITOR(handle as *mut core::ffi::c_void);
    unsafe { interop.CreateForMonitor(monitor) }
        .map_err(|error| super::hresult("IGraphicsCaptureItemInterop::CreateForMonitor", &error))
}

#[cfg(test)]
mod tests {
    use super::is_supported;

    #[test]
    fn support_probe_never_panics() {
        // The answer is machine dependent; the probe itself must always return.
        let _ = is_supported();
    }
}



