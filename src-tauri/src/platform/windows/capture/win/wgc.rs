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
use ::windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use ::windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use ::windows::Win32::Graphics::Dxgi::IDXGIDevice;
use ::windows::Win32::System::Threading::{
    CreateEventW, ResetEvent, SetEvent, WaitForSingleObjectEx,
};
use ::windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use ::windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use ::windows::core::{Interface, factory};

use super::super::monitor;
use super::d3d11::{GraphicsDevice, GpuFrame};

/// How long to wait for the first frame before falling back to another provider.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_millis(1500);
/// Poll interval for the degenerate case where the arrival handler cannot be
/// installed; the normal path waits on the event instead.
const FRAME_POLL_INTERVAL: Duration = Duration::from_millis(5);

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

    // Event-driven first-frame wait (docs/11 §3.2): `FrameArrived` signals a
    // manual-reset event and the worker parks on it, replacing the old 20 ms
    // `TryGetNextFrame` sleep-poll so arrival latency is no longer quantised.
    // If the event or the registration is unavailable the wait degrades to a
    // short poll; the capture itself never fails for this reason.
    let arrival = FrameArrivalEvent::new();
    let token = if arrival.is_valid() {
        match pool.FrameArrived(&arrival.handler()) {
            Ok(token) => Some(token),
            Err(error) => {
                eprintln!(
                    "[snapclip][capture] WGC FrameArrived unavailable ({}); polling the frame pool",
                    super::hresult("Direct3D11CaptureFramePool::FrameArrived", &error)
                );
                None
            }
        }
    } else {
        None
    };

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

    let frame = next_frame(&pool, &arrival, token.is_some());
    let frame = match frame {
        Ok(frame) => frame,
        Err(error) => {
            if let Some(token) = token {
                let _ = pool.RemoveFrameArrived(token);
            }
            let _ = session.Close();
            let _ = pool.Close();
            return Err(error);
        }
    };
    if let Some(token) = token {
        let _ = pool.RemoveFrameArrived(token);
    }
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

/// A manual-reset event signalled from the pool's `FrameArrived` handler.
///
/// The Windows 0.61 bindings do not expose `CreateWaitable`/`WaitHandle` yet, so
/// the event-driven arrival required by docs/11 §3.2 is built on the free-threaded
/// pool plus a delegate: the callback runs on a system thread-pool thread and only
/// touches the event handle, while the capture worker parks on it instead of
/// sleep-polling `TryGetNextFrame`.
struct FrameArrivalEvent(HANDLE);

/// The delegate type the pool expects, spelled once for the handler factory.
type ArrivalHandler = ::windows::Foundation::TypedEventHandler<
    Direct3D11CaptureFramePool,
    ::windows::core::IInspectable,
>;

impl FrameArrivalEvent {
    fn new() -> Self {
        let handle = unsafe {
            CreateEventW(None, true, false, ::windows::core::PCWSTR::null())
        }
        .unwrap_or(HANDLE(core::ptr::null_mut()));
        Self(handle)
    }

    fn is_valid(&self) -> bool {
        !self.0 .0.is_null() && self.0 .0 != (-1isize as *mut core::ffi::c_void)
    }

    fn handler(&self) -> ArrivalHandler {
        // The callback holds only the raw HANDLE value (not a reference to
        // this struct), so it stays valid for as long as the event exists.
        let handle = self.0 .0 as isize;
        ArrivalHandler::new(move |_sender, _args| {
            let handle = HANDLE(handle as *mut core::ffi::c_void);
            unsafe {
                let _ = SetEvent(handle);
            }
            Ok(())
        })
    }
}

impl Drop for FrameArrivalEvent {
    fn drop(&mut self) {
        if self.is_valid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// Block until the pool delivers the first frame or the timeout expires.
///
/// With a registered arrival handler the worker sleeps on the event and wakes the
/// moment a frame is queued; `TryGetNextFrame` is still the source of truth (the
/// event merely says "look again"), and the handler-less fallback polls.
fn next_frame(
    pool: &Direct3D11CaptureFramePool,
    arrival: &FrameArrivalEvent,
    event_driven: bool,
) -> Result<::windows::Graphics::Capture::Direct3D11CaptureFrame, String> {
    let deadline = Instant::now() + FIRST_FRAME_TIMEOUT;
    loop {
        match pool.TryGetNextFrame() {
            Ok(frame) => return Ok(frame),
            Err(error) => {
                // The most recent failure is reported if the budget expires
                // right here; otherwise it is discarded and re-probed next
                // iteration, so no stale value is carried across the wait.
                let message = super::hresult("TryGetNextFrame", &error);
                if Instant::now() >= deadline {
                    return Err(format!(
                        "Windows Graphics Capture produced no frame within {}ms ({message})",
                        FIRST_FRAME_TIMEOUT.as_millis()
                    ));
                }
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if event_driven && arrival.is_valid() {
            let millis = remaining.as_millis().min(u32::MAX as u128 - 1) as u32;
            let status = unsafe { WaitForSingleObjectEx(arrival.0, millis, false) };
            if status != WAIT_OBJECT_0 && status != WAIT_TIMEOUT {
                return Err(format!(
                    "WaitForSingleObjectEx on the WGC arrival event failed: {}",
                    status.0
                ));
            }
            // Manual reset: clear it so the next arrival signals again; a frame
            // that arrived between the failed TryGetNextFrame and the Reset is
            // picked up by the next loop's TryGetNextFrame before this Reset
            // or immediately after it — either way no frame is lost.
            unsafe {
                let _ = ResetEvent(arrival.0);
            }
        } else {
            std::thread::sleep(FRAME_POLL_INTERVAL);
        }
    }
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



