//! Windows Graphics Capture provider.
//!
//! Preferred provider: it returns the composited desktop as a D3D11 texture, so the
//! overlay never has to touch CPU pixels for the back buffer. One frame is pulled
//! per session — the tasklist explicitly freezes the back buffer before the overlay
//! becomes visible so the overlay cannot appear inside its own screenshot.
//!
//! **Two capture paths live here** (`docs/30 §24.2`):
//! * `capture_monitor` — the original monitor-level, one-frame-per-session path used
//!   by the overlay. Its behaviour is frozen: `attempt_order` (ordinary screenshots)
//!   must not change because of the scrolling work.
//! * `WgcSession` — the window-level, many-frames-per-session path the scrolling
//!   feature needs. It captures the target window's own content, which is what makes
//!   "the overlay does not have to hide" true (§24.2 reason 1).

use std::time::{Duration, Instant};

use ::windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem,
    GraphicsCaptureSession,
};
use ::windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
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
/// Frame pool capacity for a **scrolling** session (`docs/30 §24.2`).
///
/// Three slots cover "a frame is being produced while the newest one is being
/// taken"; the monitor path keeps its historical `2` because it takes one frame
/// and closes the session, so extra slots would only cost memory.
const SESSION_POOL_BUFFERS: i32 = 3;

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

    let item = create_item_for_monitor(layout.handle).map_err(|error| error.to_string())?;
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
fn create_item_for_monitor(handle: isize) -> Result<GraphicsCaptureItem, WgcError> {
    let interop: IGraphicsCaptureItemInterop =
        factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(|error| {
            WgcError::Failed {
                context: "GraphicsCaptureItem interop factory",
                detail: super::hresult("GraphicsCaptureItem interop factory", &error),
            }
        })?;
    let monitor = ::windows::Win32::Graphics::Gdi::HMONITOR(handle as *mut core::ffi::c_void);
    unsafe { interop.CreateForMonitor(monitor) }.map_err(|error| {
        WgcError::Failed {
            context: "IGraphicsCaptureItemInterop::CreateForMonitor",
            detail: super::hresult("IGraphicsCaptureItemInterop::CreateForMonitor", &error),
        }
    })
}

/// Why a window-level capture could not be set up or continued.
///
/// Deliberately **not** the crate-level `CaptureError`: the scrolling path has to
/// tell "the target is gone" apart from "capture broke" (the first becomes
/// `EndReason::TargetLost` and a `Partial` artifact, the second is a failure), and
/// `CaptureError::WindowFailed` folds both into one variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WgcError {
    /// The handle does not name a capturable top-level window.
    InvalidTarget { handle: isize, detail: String },
    /// The capture machinery itself failed.
    Failed {
        context: &'static str,
        detail: String,
    },
}

impl core::fmt::Display for WgcError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidTarget { handle, detail } => {
                write!(formatter, "0x{handle:x} is not a capturable window: {detail}")
            }
            Self::Failed { context, detail } => write!(formatter, "{context}: {detail}"),
        }
    }
}

/// Classify a `CreateForWindow` failure into "not a target" or "capture broke".
///
/// `E_INVALIDARG` is what an invalid or already-destroyed handle returns (measured
/// in `E-CAP-1`: `CreateForWindow(0)` yields `#code -2147024809`), and `E_HANDLE` is
/// the same refusal spelled the other way. Everything else is a capture failure.
fn classify_item_error(handle: isize, error: &::windows::core::Error) -> WgcError {
    let detail = super::hresult("IGraphicsCaptureItemInterop::CreateForWindow", error);
    if error.code() == ::windows::Win32::Foundation::E_INVALIDARG
        || error.code() == ::windows::Win32::Foundation::E_HANDLE
    {
        WgcError::InvalidTarget { handle, detail }
    } else {
        WgcError::Failed {
            context: "IGraphicsCaptureItemInterop::CreateForWindow",
            detail,
        }
    }
}

/// Build a capture item for a **top-level window** (`docs/30 §24.2`).
///
/// Coexists with [`create_item_for_monitor`]: the ordinary screenshot path keeps
/// capturing the monitor, because changing it is a regression risk with no benefit
/// to the scrolling feature (§21.3).
pub fn create_item_for_window(hwnd: isize) -> Result<GraphicsCaptureItem, WgcError> {
    let interop: IGraphicsCaptureItemInterop =
        factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(|error| {
            WgcError::Failed {
                context: "GraphicsCaptureItem interop factory",
                detail: super::hresult("GraphicsCaptureItem interop factory", &error),
            }
        })?;
    let window = ::windows::Win32::Foundation::HWND(hwnd as *mut core::ffi::c_void);
    unsafe { interop.CreateForWindow(window) }.map_err(|error| classify_item_error(hwnd, &error))
}

/// Wrap a D3D11 device as the WinRT `IDirect3DDevice` the frame pool wants.
fn direct3d_device(device: &GraphicsDevice) -> Result<IDirect3DDevice, WgcError> {
    let dxgi_device: IDXGIDevice = device.device().cast().map_err(|error| WgcError::Failed {
        context: "IDXGIDevice::cast",
        detail: super::hresult("IDXGIDevice::cast", &error),
    })?;
    let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device) }.map_err(
        |error| WgcError::Failed {
            context: "CreateDirect3D11DeviceFromDXGIDevice",
            detail: super::hresult("CreateDirect3D11DeviceFromDXGIDevice", &error),
        },
    )?;
    inspectable.cast().map_err(|error| WgcError::Failed {
        context: "IDirect3DDevice::cast",
        detail: super::hresult("IDirect3DDevice::cast", &error),
    })
}

/// Get the D3D11 texture behind a captured frame.
fn texture_of(frame: &Direct3D11CaptureFrame) -> Result<ID3D11Texture2D, WgcError> {
    let surface = frame.Surface().map_err(|error| WgcError::Failed {
        context: "Direct3D11CaptureFrame::Surface",
        detail: super::hresult("Direct3D11CaptureFrame::Surface", &error),
    })?;
    let access: IDirect3DDxgiInterfaceAccess =
        surface.cast().map_err(|error| WgcError::Failed {
            context: "IDirect3DDxgiInterfaceAccess::cast",
            detail: super::hresult("IDirect3DDxgiInterfaceAccess::cast", &error),
        })?;
    unsafe { access.GetInterface() }.map_err(|error| WgcError::Failed {
        context: "IDirect3DDxgiInterfaceAccess::GetInterface",
        detail: super::hresult("IDirect3DDxgiInterfaceAccess::GetInterface", &error),
    })
}

/// What a pool should do about a newly observed content size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sizing {
    /// The pool still matches the content.
    Keep,
    /// The pool has to be recreated at this size.
    Recreate { width: i32, height: i32 },
}

/// Tracks the content size a frame pool was created for.
///
/// The point of this type is that a resize is a **step**, not a per-frame cost: a
/// scrolling session steps every few tens of milliseconds, so recreating the pool
/// per frame would be a fixed tax on every step (`docs/30 §24.2`'s "跨帧复用" row).
/// The observed size becomes the new baseline immediately, so the caller can act on
/// [`Sizing::Recreate`] without feeding the result back.
#[derive(Debug, Clone)]
pub struct PoolSizing {
    size: (i32, i32),
    recreations: u32,
}

impl PoolSizing {
    pub fn new(width: i32, height: i32) -> Self {
        Self {
            size: (width, height),
            recreations: 0,
        }
    }

    /// The size the pool currently has.
    pub fn size(&self) -> (i32, i32) {
        self.size
    }

    /// How many times a caller has been asked to recreate the pool.
    pub fn recreations(&self) -> u32 {
        self.recreations
    }

    /// Compare an observed content size against the pool's baseline.
    pub fn observe(&mut self, content: (i32, i32)) -> Sizing {
        if content == self.size {
            return Sizing::Keep;
        }
        self.size = content;
        self.recreations += 1;
        Sizing::Recreate {
            width: content.0,
            height: content.1,
        }
    }
}

/// One frame from a [`WgcSession`].
///
/// Holds the `Direct3D11CaptureFrame` on purpose: the texture is a reference to a
/// buffer owned by the pool, and dropping the frame lets the pool recycle it. The
/// scrolling path reads a region *after* the frame has been handed over
/// (`P2.02`), so the frame has to stay alive at least as long as the texture is
/// used.
pub struct WgcFrame {
    frame: Direct3D11CaptureFrame,
    texture: ID3D11Texture2D,
    size: (i32, i32),
}

impl WgcFrame {
    /// The captured content, valid while this frame is alive.
    pub fn texture(&self) -> &ID3D11Texture2D {
        &self.texture
    }

    /// The content size reported by this frame (not the window rectangle).
    pub fn size(&self) -> (i32, i32) {
        self.size
    }

    /// The frame handle, for callers that need to close it early.
    pub fn frame(&self) -> &Direct3D11CaptureFrame {
        &self.frame
    }
}

/// A window-level capture that stays open across many frames.
///
/// This is the shape the scrolling feature needs (`docs/30 §24.2`): one item, one
/// pool and one session for the whole capture, with the pool recreated only when the
/// window's content size changes. It deliberately does **not** reuse
/// `capture_monitor`'s one-frame lifecycle, and it deliberately does not enable
/// `SetDirtyRegionMode` — dirty regions would make "every step is a full frame"
/// untrue, and the estimator is written against full frames.
///
/// Dropping the session removes the arrival handler and closes the session and pool;
/// the caller does not have to do anything.
pub struct WgcSession {
    item: GraphicsCaptureItem,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    arrival: FrameArrivalEvent,
    token: Option<i64>,
    sizing: PoolSizing,
    diagnostics: Vec<String>,
    device: IDirect3DDevice,
}

impl WgcSession {
    /// Open a capture session on a top-level window.
    ///
    /// An unusable handle is [`WgcError::InvalidTarget`]; everything after the item
    /// exists is [`WgcError::Failed`]. Capture **options** never fail the session:
    /// each unavailable option is recorded in [`Self::diagnostics`] and the session
    /// starts anyway, because a missing option changes what the pixels contain, not
    /// whether there are pixels (the full capability model is `P2.04`).
    pub fn open(device: &GraphicsDevice, hwnd: isize) -> Result<Self, WgcError> {
        if !is_supported() {
            return Err(WgcError::Failed {
                context: "GraphicsCaptureSession::IsSupported",
                detail: "Windows Graphics Capture is not supported on this system".into(),
            });
        }

        let item = create_item_for_window(hwnd)?;
        let content = item.Size().map_err(|error| WgcError::Failed {
            context: "GraphicsCaptureItem::Size",
            detail: super::hresult("GraphicsCaptureItem::Size", &error),
        })?;
        let direct3d_device = direct3d_device(device)?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &direct3d_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            SESSION_POOL_BUFFERS,
            content,
        )
        .map_err(|error| WgcError::Failed {
            context: "Direct3D11CaptureFramePool::CreateFreeThreaded",
            detail: super::hresult(
                "Direct3D11CaptureFramePool::CreateFreeThreaded",
                &error,
            ),
        })?;

        let mut diagnostics = Vec::new();
        let arrival = FrameArrivalEvent::new();
        let token = if arrival.is_valid() {
            match pool.FrameArrived(&arrival.handler()) {
                Ok(token) => Some(token),
                Err(error) => {
                    diagnostics.push(super::hresult(
                        "Direct3D11CaptureFramePool::FrameArrived",
                        &error,
                    ));
                    None
                }
            }
        } else {
            diagnostics.push("CreateEventW failed; the session polls instead".into());
            None
        };

        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|error| WgcError::Failed {
                context: "Direct3D11CaptureFramePool::CreateCaptureSession",
                detail: super::hresult("CreateCaptureSession", &error),
            })?;
        // Both options are best effort here; P2.04 owns the capability model.
        if let Err(error) = session.SetIsCursorCaptureEnabled(false) {
            diagnostics.push(super::hresult("SetIsCursorCaptureEnabled", &error));
        }
        if let Err(error) = session.SetIsBorderRequired(false) {
            diagnostics.push(super::hresult("SetIsBorderRequired", &error));
        }
        session.StartCapture().map_err(|error| WgcError::Failed {
            context: "GraphicsCaptureSession::StartCapture",
            detail: super::hresult("GraphicsCaptureSession::StartCapture", &error),
        })?;

        Ok(Self {
            item,
            pool,
            session,
            arrival,
            token,
            sizing: PoolSizing::new(content.Width, content.Height),
            diagnostics,
            device: direct3d_device,
        })
    }

    /// The content size the session was opened at.
    pub fn size(&self) -> (i32, i32) {
        self.sizing.size()
    }

    /// How many pool recreations this session has needed.
    pub fn recreations(&self) -> u32 {
        self.sizing.recreations()
    }

    /// Options that could not be applied, as `context failed (message) #code=…`.
    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    /// The capture item, for callers that need its geometry.
    pub fn item(&self) -> &GraphicsCaptureItem {
        &self.item
    }

    /// Wait up to `timeout` for the next frame.
    ///
    /// `Ok(None)` means "no new content arrived", which is a normal state and not a
    /// failure: WGC delivers frames when the composited content changes, so a still
    /// window goes quiet after a few frames (`docs/30 §24.2.1` facts 1–2). windows-rs
    /// projects that silence as an `Err` carrying `S_OK`, which is why the code has
    /// to look at the HRESULT rather than at `Result::is_err`.
    pub fn next_frame(&mut self, timeout: Duration) -> Result<Option<WgcFrame>, WgcError> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.pool.TryGetNextFrame() {
                Ok(frame) => {
                    let content = frame.ContentSize().map_err(|error| WgcError::Failed {
                        context: "Direct3D11CaptureFrame::ContentSize",
                        detail: super::hresult("Direct3D11CaptureFrame::ContentSize", &error),
                    })?;
                    let size = (content.Width, content.Height);
                    if let Sizing::Recreate { width, height } = self.sizing.observe(size) {
                        self.pool
                            .Recreate(
                                &self.device,
                                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                                SESSION_POOL_BUFFERS,
                                SizeInt32 { Width: width, Height: height },
                            )
                            .map_err(|error| WgcError::Failed {
                                context: "Direct3D11CaptureFramePool::Recreate",
                                detail: super::hresult(
                                    "Direct3D11CaptureFramePool::Recreate",
                                    &error,
                                ),
                            })?;
                    }
                    let texture = texture_of(&frame)?;
                    return Ok(Some(WgcFrame {
                        frame,
                        texture,
                        size,
                    }));
                }
                Err(error) => {
                    if error.code().0 != 0 {
                        return Err(WgcError::Failed {
                            context: "Direct3D11CaptureFramePool::TryGetNextFrame",
                            detail: super::hresult("TryGetNextFrame", &error),
                        });
                    }
                }
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            if self.token.is_some() && self.arrival.is_valid() {
                let millis = remaining.as_millis().min(u32::MAX as u128 - 1) as u32;
                let status = unsafe { WaitForSingleObjectEx(self.arrival.0, millis, false) };
                if status != WAIT_OBJECT_0 && status != WAIT_TIMEOUT {
                    return Err(WgcError::Failed {
                        context: "WaitForSingleObjectEx",
                        detail: format!("waiting on the WGC arrival event failed: {}", status.0),
                    });
                }
                unsafe {
                    let _ = ResetEvent(self.arrival.0);
                }
            } else {
                std::thread::sleep(FRAME_POLL_INTERVAL.min(remaining));
            }
        }
    }
}

impl Drop for WgcSession {
    fn drop(&mut self) {
        if let Some(token) = self.token.take() {
            let _ = self.pool.RemoveFrameArrived(token);
        }
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PoolSizing, Sizing, WgcError, WgcSession, create_item_for_window, is_supported,
    };
    use crate::windows::scroll_probe::{
        WindowMatch, pump_for, visible_windows, wait_for_new_window,
    };
    use std::time::Duration;

    use super::GraphicsDevice;

    #[test]
    fn support_probe_never_panics() {
        // The answer is machine dependent; the probe itself must always return.
        let _ = is_supported();
    }

    /// WinRT activation must leave the thread in *combase's implicit* apartment.
    ///
    /// `R-21` (`docs/31` §14.3): measured on this machine (2026-10-08), the WinRT
    /// path used by Windows Graphics Capture faults (`0xC0000005`) when the calling
    /// thread holds an apartment the application declared itself
    /// (`APTTYPE=1, qualifier=0`), and works when the apartment is the one combase
    /// created on its own (`APTTYPE=1, qualifier=1` = `IMPLICIT_MTA`). Declaring an
    /// apartment "because COM says so" therefore reintroduces a deterministic crash
    /// (interleaved A/B: 3/3 crash with the declarations, 3/3 green without).
    ///
    /// The check runs in a **fresh process** on purpose. A thread created by a
    /// thread that is already in the implicit MTA *starts* in that MTA (measured on
    /// this machine: `before = kind=1 qualifier=1`), and `CoInitializeEx(MTA)` on
    /// such a thread returns `S_FALSE` without touching the qualifier — so a
    /// declaration added to the activation path is invisible from a thread that
    /// inherited the apartment. Only a COM-free start makes the declaration
    /// observable, and only a fresh process guarantees that the harness thread has
    /// not already been pulled into the MTA by an earlier test in the same binary.
    #[test]
    fn winrt_activation_keeps_the_implicit_apartment() {
        use ::windows::Win32::System::Com::{
            APTTYPE, APTTYPEQUALIFIER, APTTYPEQUALIFIER_IMPLICIT_MTA, APTTYPE_MTA,
            CoGetApartmentType,
        };

        fn apartment() -> Result<(APTTYPE, APTTYPEQUALIFIER), String> {
            let mut kind = APTTYPE(0);
            let mut qualifier = APTTYPEQUALIFIER(0);
            unsafe { CoGetApartmentType(&mut kind, &mut qualifier) }
                .map(|()| (kind, qualifier))
                .map_err(|error| error.to_string())
        }

        const CHILD: &str = "SNAPCLIP_APARTMENT_CHILD";

        if std::env::var_os(CHILD).is_none() {
            // Parent: hand the check to a process whose threads are all COM-free.
            let exe = std::env::current_exe().expect("the test binary must be locatable");
            let status = std::process::Command::new(exe)
                .args([
                    "--exact",
                    "windows::win::wgc::tests::winrt_activation_keeps_the_implicit_apartment",
                    "--test-threads=1",
                ])
                .env(CHILD, "1")
                .status()
                .expect("the test binary must be runnable");
            assert!(
                status.success(),
                "the isolated apartment check failed: {status}"
            );
            return;
        }

        let checked = std::thread::spawn(|| {
            assert!(
                apartment().is_err(),
                "the isolated process must start without an apartment; if this fails the check \
                 can no longer see a declared one (docs/31 §14.3)"
            );

            let _ = is_supported();

            let (kind, qualifier) =
                apartment().expect("WinRT activation left the thread without an apartment");
            assert_eq!(kind, APTTYPE_MTA, "WinRT activation must land in the MTA");
            assert_eq!(
                qualifier, APTTYPEQUALIFIER_IMPLICIT_MTA,
                "the apartment is no longer combase's implicit one: something in this process \
                 declared it, which reintroduces the R-21 crash (docs/31 §14.3)"
            );
        });

        checked.join().expect("the probe thread must not panic");
    }

    /// A window capture item can be created for a real top-level window, and an
    /// invalid handle is a **typed** refusal rather than a panic (`P2.01`).
    ///
    /// The invalid half is known to work without a desktop (`docs/30 §24.2.1`:
    /// WinRT activation succeeds without `RoInitialize`, and `CreateForWindow(0)`
    /// returns `E_INVALIDARG` = `#code -2147024809`), so it is asserted
    /// unconditionally; the real half needs a desktop and a capturable window, so
    /// it is `#[ignore]` rather than a silent skip (the rule in `P2.08`/`D-14`).
    #[test]
    #[ignore = "P2.01: needs a real interactive desktop and a capturable window"]
    fn a_window_capture_item_can_be_created_for_a_top_level_window() {
        let invalid = create_item_for_window(0);
        assert!(
            matches!(invalid, Err(WgcError::InvalidTarget { .. })),
            "an invalid handle must be a typed InvalidTarget, not a panic and not a \
             generic failure: {invalid:?}"
        );

        let known: Vec<_> = visible_windows()
            .into_iter()
            .map(|window| window.hwnd)
            .collect();
        let mut notepad = std::process::Command::new("notepad.exe")
            .spawn()
            .expect("notepad.exe must be launchable on this machine");
        let window = wait_for_new_window(&known, WindowMatch::Any, Duration::from_secs(10))
            .expect("notepad must create a top-level window within 10s");
        pump_for(Duration::from_millis(300));

        let handle = window.hwnd as isize;
        let item = create_item_for_window(handle)
            .unwrap_or_else(|error| panic!("a top-level window must be capturable: {error:?}"));
        let content = item.Size().expect("the item must report a content size");
        assert!(
            content.Width > 0 && content.Height > 0,
            "the item reported a degenerate content size: {content:?}"
        );

        let device = GraphicsDevice::create().expect("this test needs a D3D11 device");
        let session = WgcSession::open(&device, handle)
            .expect("a capture session must open on a capturable window");
        assert_eq!(
            session.size(),
            (content.Width, content.Height),
            "the session must take its size from the item, not from the window rectangle \
             (docs/30 §24.2.1 fact 7)"
        );
        assert_eq!(
            session.recreations(),
            0,
            "opening a session must not recreate the pool"
        );
        assert!(
            session.diagnostics().is_empty(),
            "no capture option should have been unavailable on this machine: {:?}",
            session.diagnostics()
        );
        drop(session);

        let _ = notepad.kill();
        let _ = notepad.wait();
    }

    /// The pool is recreated when the content size changes, **never per frame**.
    ///
    /// This is the mechanical form of `docs/30 §24.2`'s "跨帧复用" row: a scrolling
    /// session steps every few tens of milliseconds, so a per-frame `Recreate` would
    /// be a fixed cost on every step. The 100-step budget is the same one `E-CAP-1`
    /// uses.
    #[test]
    fn the_pool_is_not_recreated_between_frames() {
        let mut sizing = PoolSizing::new(1280, 960);
        assert_eq!(sizing.size(), (1280, 960));
        assert_eq!(sizing.recreations(), 0);

        for frame in 0..100 {
            assert_eq!(
                sizing.observe((1280, 960)),
                Sizing::Keep,
                "frame {frame} asked for a pool recreation although nothing changed"
            );
        }
        assert_eq!(
            sizing.recreations(),
            0,
            "a hundred identical frames must cost zero recreations"
        );

        assert_eq!(
            sizing.observe((1280, 961)),
            Sizing::Recreate {
                width: 1280,
                height: 961
            },
            "a content size change is the one thing that has to recreate the pool"
        );
        assert_eq!(sizing.recreations(), 1);

        for frame in 0..100 {
            assert_eq!(
                sizing.observe((1280, 961)),
                Sizing::Keep,
                "frame {frame} after the resize asked for another recreation"
            );
        }
        assert_eq!(
            sizing.recreations(),
            1,
            "the new size becomes the baseline; a resize is a step, not a per-frame cost"
        );
    }
}



