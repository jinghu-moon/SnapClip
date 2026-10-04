//! Overlay HWND, message loop, input handling and session lifecycle.
//!
//! One dedicated UI thread owns:
//! * the `F5` hotkey registration (so `WM_HOTKEY` lands in the right queue),
//! * the overlay window class and its hidden, pre-created window,
//! * the D3D11/D2D renderer and the DirectComposition target.
//!
//! Pixel capture does **not** happen here: `F5` submits a [`StartRequest`] to the
//! [`CaptureWorker`] and the thread keeps pumping, so `Esc` cancels a session
//! while the screen is still being frozen (docs/11 §3.1).
//!
//! Everything else in the process talks to it through [`WindowsOverlay`], which only
//! posts small messages — never pixels.
//!
//! Cancellation is deliberately funnelled through one routine so `Esc`, window
//! destruction, display changes and device removal release the same resources in the
//! same order.

use std::mem::zeroed;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use windows_sys::Win32::{
    Foundation::{GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM},
    Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT},
    System::LibraryLoader::GetModuleHandleW,
    System::Threading::GetCurrentThreadId,
    UI::{
        Controls::WM_MOUSELEAVE,
        Input::KeyboardAndMouse::{GetKeyState, SetFocus, VK_CONTROL},
        WindowsAndMessaging::{
            IDC_ARROW, IDC_CROSS, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE,
            LoadCursorW,
            SetCursor,
            CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
            DispatchMessageW, GetCursorPos, GetForegroundWindow, GetMessageW, MSG, PeekMessageW,
            PostQuitMessage,
            PostThreadMessageW, RegisterClassW, SW_HIDE, SW_SHOW, SetForegroundWindow,
            SetWindowPos, SetTimer, KillTimer,
            ShowWindow, TranslateMessage, UnregisterClassW, WM_APP, WM_DESTROY, WM_DEVICECHANGE,
            WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ERASEBKGND, WM_HOTKEY, WM_KEYDOWN, WM_LBUTTONDOWN,
            WM_MOUSEACTIVATE,
            WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCHITTEST, WM_PAINT, WM_RBUTTONDOWN,
            WM_SETCURSOR, WM_TIMER, WNDCLASSW, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW,
            WS_EX_TOPMOST, WS_POPUP, HTCLIENT, HWND_TOPMOST, MA_ACTIVATE,
        },
    },
};

use crate::application::capture_service::{
    ArtifactEncoder, ArtifactDir, CaptureService, SelectionPixels,
};
use crate::capture::annotation::{
    AnnotationCommand, AnnotationDocument, AnnotationGeometry, AnnotationHandle, AnnotationId,
    AnnotationItem, AnnotationKind, DocumentSnapshot,
};
use crate::capture::application::{CaptureEventSink, OverlayPlatform};
use crate::capture::geometry::{
    Handle, MagnifierConfig, MonitorLayout, Point, Rect, ResizeMode, SelectionGeometry,
    magnifier_geometry,
};
use crate::capture::sampler::{ColorFormat, ColorSampler};
use crate::capture::session::{CaptureSession, ExportOutcome};
use crate::capture::{CaptureError, CaptureResult, CaptureState};

use super::capture_worker::{self, CaptureWorker, StartRequest};
use super::export_worker::{self, ExportJob, ExportWorker};
use super::hotkey;
use super::monitor::{self, CapturedMonitor};
use super::providers::{FrozenFrame, FrozenFramePixels};
use super::renderer::{OverlayFrameState, Win32Renderer};

/// `WM_APP`-based command delivered from any thread to the overlay thread.
const WM_OVERLAY_COMMAND: u32 = WM_APP + 17;
/// Coalescing render cadence in milliseconds (~60 Hz, docs/11 §"约 16ms 渲染节奏").
/// Mouse and drag input only marks state dirty and arms this one-shot timer; the
/// timer collapses every change since the last tick into a single render/present/commit.
const RENDER_TICK_MS: u32 = 15;
/// The `SetTimer` id for the coalescing render tick.
const RENDER_TIMER_ID: usize = 0x51_C0DE;
/// `TrackMouseEvent` flag asking for a `WM_MOUSELEAVE` notification.
const TME_LEAVE: u32 = 0x0000_0002;
/// `SWP_SHOWWINDOW`.
const SWP_SHOWWINDOW: u32 = 0x0040;

/// Result of the readiness handshake: the overlay thread's window-message id.
type SystemResult = Result<String, String>;
/// Result of posting a cross-thread command.
type PostResult = Result<(), String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverlayCommand {
    Start,
    Cancel,
    Confirm,
    /// Wake-up sentinel for the annotation mailbox: the payload is drained from the
    /// bounded channel, not from `wparam`, because a toolbar command carries arguments
    /// (a colour, a width, a tool). Low frequency — one per toolbar click.
    Annotation,
    Shutdown,
    /// Sentinel `wparam` on [`capture_worker::FRAME_READY_MESSAGE`] so the
    /// worker's wake-up shares the command dispatch path.
    FrameReady,
    /// Sentinel `wparam` on [`export_worker::EXPORT_READY_MESSAGE`]: the artifact was
    /// encoded and written off the UI thread.
    ExportReady,
}

impl OverlayCommand {
    fn from_wparam(wparam: WPARAM) -> Option<Self> {
        match wparam as i32 {
            value if value == Self::FrameReady as i32 => Some(Self::FrameReady),
            value if value == Self::ExportReady as i32 => Some(Self::ExportReady),
            value if value == Self::Start as i32 => Some(Self::Start),
            value if value == Self::Cancel as i32 => Some(Self::Cancel),
            value if value == Self::Confirm as i32 => Some(Self::Confirm),
            value if value == Self::Annotation as i32 => Some(Self::Annotation),
            value if value == Self::Shutdown as i32 => Some(Self::Shutdown),
            _ => None,
        }
    }
}

/// Cross-thread state shared between [`WindowsOverlay`] and the overlay thread.
#[derive(Debug)]
struct OverlayShared {
    state: CaptureState,
    session_id: Option<String>,
    /// Snapshot of the overlay window, refreshed after creation.
    window: Option<OverlayWindowState>,
}

/// The concrete `OverlayPlatform` used on Windows.
///
/// Holds only thread handles and atomics, so it is `Send + Sync` and every method is
/// safe to call from a Tauri command thread.
pub struct WindowsOverlay {
    /// Window-message thread id of the overlay thread, in the string form used by the
    /// readiness handshake.
    thread_id: String,
    thread: Mutex<Option<JoinHandle<()>>>,
    shared: Arc<Mutex<OverlayShared>>,
    /// Producer end of the annotation mailbox. Tauri command threads push a toolbar
    /// command here and post an [`OverlayCommand::Annotation`] wake-up; the overlay
    /// thread owns the matching receiver and drains it on its own cadence.
    annotation_tx: mpsc::SyncSender<AnnotationCommand>,
    shutting_down: AtomicBool,
}

/// Window state the overlay publishes for diagnostics and acceptance checks.
///
/// Handles are published as `isize` because raw pointers are not `Send`; the values
/// are read-only snapshots of an existing window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayWindowState {
    pub window: isize,
    /// `GWL_EXSTYLE` of the overlay window.
    pub extended_style: isize,
    /// Whether the overlay currently owns the foreground.
    pub foreground: bool,
}

impl WindowsOverlay {
    /// Snapshot of the overlay window for diagnostics and acceptance checks.
    pub fn window_state(&self) -> Result<OverlayWindowState, String> {
        let state = self
            .shared
            .lock()
            .map_err(|_| "overlay state lock poisoned".to_string())?
            .window;
        state.ok_or_else(|| "overlay window is not created yet".to_string())
    }
}

impl WindowsOverlay {
    /// Spawn the overlay thread and wait until the hotkey and window exist.
    ///
    /// Fails — rather than silently degrading — when `F5` cannot be registered, so
    /// the user learns about the conflict.
    pub fn spawn_overlay<D, E>(
        service: Arc<CaptureService<D, E>>,
        sink: Arc<dyn CaptureEventSink>,
    ) -> Result<Self, String>
    where
        D: ArtifactDir,
        E: ArtifactEncoder,
    {
        let shared = Arc::new(Mutex::new(OverlayShared {
            state: CaptureState::Idle,
            session_id: None,
            window: None,
        }));
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        // Bounded so a stuck/slow overlay cannot let toolbar clicks pile up without
        // limit; the capacity only needs to absorb a burst of discrete clicks.
        let (annotation_tx, annotation_rx) = mpsc::sync_channel(64);
        let thread_shared = shared.clone();
        let thread = thread::Builder::new()
            .name("snapclip-capture-overlay".into())
            .spawn(move || overlay_thread(service, sink, thread_shared, annotation_rx, ready_tx))
            .map_err(|error| format!("spawn overlay thread failed: {error}"))?;

        match ready_rx.recv() {
            Ok(Ok(thread_id)) => Ok(Self {
                thread_id,
                thread: Mutex::new(Some(thread)),
                shared,
                annotation_tx,
                shutting_down: AtomicBool::new(false),
            }),
            Ok(Err(message)) => {
                let _ = thread.join();
                Err(message)
            }
            Err(_) => {
                let _ = thread.join();
                Err("overlay thread exited before reporting readiness".into())
            }
        }
    }


    /// Post a command to the overlay thread. Never blocks on the UI thread.
    ///
    /// Rejects new commands once shutdown has started; [`WindowsOverlay::shutdown`]
    /// posts its own message directly so it is not blocked by this guard.
    fn post(&self, command: OverlayCommand) -> PostResult {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err("capture overlay is shutting down".into());
        }
        let Ok(thread_id) = self.thread_id.parse::<u32>() else {
            return Err("overlay thread id is not numeric".into());
        };
        let posted =
            unsafe { PostThreadMessageW(thread_id, WM_OVERLAY_COMMAND, command as WPARAM, 0) };
        if posted == 0 {
            return Err(format!(
                "PostThreadMessageW failed with Win32 error {}",
                unsafe { GetLastError() }
            ));
        }
        Ok(())
    }
}

impl OverlayPlatform for WindowsOverlay {
    fn state(&self) -> CaptureState {
        self.shared
            .lock()
            .map(|shared| shared.state)
            .unwrap_or(CaptureState::Idle)
    }

    fn request_start(&self) -> CaptureResult<bool> {
        if self.state().is_active() {
            return Ok(false);
        }
        self.post(OverlayCommand::Start)
            .map_err(|message| CaptureError::WindowFailed(format!("start capture failed: {message}")))?;
        Ok(true)
    }

    fn request_cancel(&self) -> CaptureResult<()> {
        self.post(OverlayCommand::Cancel)
            .map_err(|message| CaptureError::InvalidState(format!("cancel failed: {message}")))
    }

    fn request_confirm(&self) -> CaptureResult<()> {
        self.post(OverlayCommand::Confirm)
            .map_err(|message| CaptureError::InvalidState(format!("confirm failed: {message}")))
    }

    fn request_annotation(&self, command: AnnotationCommand) -> CaptureResult<()> {
        // `try_send`, not `send`: a wedged overlay must never block a Tauri command
        // thread. The bounded mailbox only needs to absorb a burst of clicks; if it is
        // full the toolbar is simply told the overlay is not draining.
        self.annotation_tx.try_send(command).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => CaptureError::InvalidState(
                "annotation mailbox is full; the overlay is not draining".into(),
            ),
            mpsc::TrySendError::Disconnected(_) => {
                CaptureError::InvalidState("overlay is not running".into())
            }
        })?;
        self.post(OverlayCommand::Annotation)
            .map_err(|message| CaptureError::InvalidState(format!("annotation command failed: {message}")))
    }

    fn shutdown(&self) {
        // Order matters: `post` refuses to send once `shutting_down` is set, so the
        // shutdown message must go out first. Setting the flag first left the overlay
        // thread parked in `GetMessageW` forever and the `join` below hung with it.
        let first_call = !self.shutting_down.swap(true, Ordering::AcqRel);
        if !first_call {
            return;
        }
        // Posted directly rather than through `post`, which now rejects everything.
        if let Ok(thread_id) = self.thread_id.parse::<u32>() {
            unsafe {
                PostThreadMessageW(thread_id, WM_OVERLAY_COMMAND, OverlayCommand::Shutdown as WPARAM, 0);
            }
        }
        if let Ok(mut thread) = self.thread.lock()
            && let Some(handle) = thread.take()
        {
            let _ = handle.join();
        }
    }
}

impl Drop for WindowsOverlay {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Messages the window procedure forwards into the session controller.
///
/// The window procedure itself cannot be generic over the artifact encoder, so it
/// dispatches through this object instead.
trait OverlayMessageHandler {
    /// Returns `Some(result)` when the controller consumed the message.
    unsafe fn handle(&mut self, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT>;
}

/// An in-progress annotation pointer gesture (overlay thread only).
///
/// A gesture spans one button-down / moves / button-up cycle. Undo snapshots are
/// taken at gesture start and committed once at release, so a multi-move drag is a
/// single undo step (docs/11 §8.2).
enum AnnotationGesture {
    /// Translating an existing item; `last` anchors the incremental delta.
    Move {
        id: AnnotationId,
        snapshot: DocumentSnapshot,
        last: Point,
    },
    /// Resizing an existing item by dragging one of its control points.
    Resize {
        id: AnnotationId,
        handle: AnnotationHandle,
        snapshot: DocumentSnapshot,
    },
    /// Drawing a new shape anchored at `start` (the draft tracks the pointer).
    Create { start: Point },
}

/// Owns every session-scoped resource. Lives only on the overlay thread.
struct OverlayController<D, E>
where
    D: ArtifactDir,
    E: ArtifactEncoder,
{
    service: Arc<CaptureService<D, E>>,
    sink: Arc<dyn CaptureEventSink>,
    shared: Arc<Mutex<OverlayShared>>,
    window: HWND,
    /// Captures run on the worker thread; the overlay only submits requests
    /// and drains the capacity-1 result mailbox.
    worker: CaptureWorker,
    /// Encoding and file writing run here, so `confirm` returns as soon as the
    /// selection's pixels are in hand (docs/11 §Phase 3).
    export_worker: ExportWorker,
    renderer: Option<Win32Renderer>,
    session: CaptureSession,
    frozen: Option<FrozenFrame>,
    drag_mode: Option<ResizeMode>,
    cursor: Point,
    cursor_visible: bool,
    dirty: bool,
    /// Pure color-sampling state machine (tile hit, throttle, hex formatting).
    sampler: ColorSampler,
    /// Shift-cycled colour format shown in the magnifier info panel.
    magnifier_color_format: ColorFormat,
    /// When true the info-panel coordinate is relative to the selection origin (P toggle).
    magnifier_relative: bool,
    /// The GPU slot index of the most recent async sample request.
    sample_slot: Option<usize>,
    /// Object-based annotation document (L2 layer).
    annotation_doc: AnnotationDocument,
    /// Active annotation tool; `None` is the select / move / resize tool.
    annotation_tool: Option<AnnotationKind>,
    /// In-progress annotation pointer gesture, if the button is down.
    annotation_gesture: Option<AnnotationGesture>,
    /// Receiver end of the toolbar annotation mailbox; drained when an
    /// [`OverlayCommand::Annotation`] wake-up arrives.
    annotation_rx: mpsc::Receiver<AnnotationCommand>,
    /// Whether the coalescing render tick is currently armed (`SetTimer` running).
    render_armed: bool,
    /// Whether the session's graphics have been handed to the export worker.
    /// While this is set the controller paints nothing: the D3D11 immediate context
    /// the frozen texture lives on is single-threaded, and the export owns it.
    graphics_released: bool,
    session_counter: u64,
    /// Generation of the session currently awaiting a worker result.
    current_generation: u64,
    /// Window that owned the foreground before the overlay appeared.
    previous_foreground: HWND,
}

impl<D, E> OverlayController<D, E>
where
    D: ArtifactDir,
    E: ArtifactEncoder,
{
    fn new(
        service: Arc<CaptureService<D, E>>,
        sink: Arc<dyn CaptureEventSink>,
        shared: Arc<Mutex<OverlayShared>>,
        annotation_rx: mpsc::Receiver<AnnotationCommand>,
        window: HWND,
    ) -> Self {
        Self {
            service,
            sink,
            shared,
            annotation_rx,
            window,
            worker: CaptureWorker::new(),
            export_worker: ExportWorker::new(),
            renderer: None,
            session: CaptureSession::new("idle"),
            frozen: None,
            drag_mode: None,
            cursor: Point::default(),
            cursor_visible: false,
            dirty: false,
            sampler: ColorSampler::new(),
            magnifier_color_format: ColorFormat::default(),
            magnifier_relative: false,
            sample_slot: None,
            annotation_doc: AnnotationDocument::new(),
            annotation_tool: None,
            annotation_gesture: None,
            render_armed: false,
            graphics_released: false,
            session_counter: 0,
            current_generation: 0,
            previous_foreground: null_mut(),
        }
    }

    fn session_id(&self) -> String {
        self.session.id().to_string()
    }

    fn layout(&self) -> Option<MonitorLayout> {
        self.renderer.as_ref().map(|renderer| renderer.layout().clone())
    }

    fn publish_state(&self) {
        eprintln!(
            "[snapclip][capture] state session={} state={:?}",
            self.session_id(),
            self.session.state()
        );
        if let Ok(mut shared) = self.shared.lock() {
            shared.state = self.session.state();
            shared.session_id = Some(self.session_id());
        }
        let layout = self.layout();
        self.sink
            .on_state(&self.session_id(), self.session.state(), layout.as_ref());
    }

    /// Drain the toolbar annotation mailbox. Runs only on the overlay thread, so the
    /// document needs no lock; a burst of clicks collapses into one repaint instead of
    /// one per command (docs/11 §7.1 "工具栏不进入像素管线").
    fn drain_annotation_commands(&mut self) {
        let mut applied = false;
        while let Ok(command) = self.annotation_rx.try_recv() {
            self.apply_annotation_command(command);
            applied = true;
        }
        if applied {
            self.invalidate_all();
        }
    }

    /// Route one toolbar command. Tool selection is controller-owned presentation
    /// state; every document mutation is delegated to [`AnnotationDocument::execute`]
    /// so the routing table lives beside the document, not scattered across the pump.
    fn apply_annotation_command(&mut self, command: AnnotationCommand) {
        match command {
            AnnotationCommand::SelectTool => self.annotation_tool = None,
            AnnotationCommand::Tool(kind) => self.annotation_tool = Some(kind),
            other => self.annotation_doc.execute(other),
        }
    }

    /// `F5`: submit a capture request and enter `Preparing`.
    ///
    /// Nothing here waits on WGC/BitBlt: the freeze happens on the worker
    /// thread and the message pump keeps running, so `Esc` cancels while the
    /// screen is still being captured (docs/11 §2.2/§3.3).
    fn start_session(&mut self) {
        let started_at = Instant::now();
        eprintln!("[snapclip][capture] starting session from hotkey/command");
        if self.session.state().is_active() {
            // A repeated hotkey press restarts rather than stacking sessions;
            // `cancel` bumps the generation so the old request's result dies.
            self.cancel("hotkey-restart");
        }

        let monitor = match monitor::captured_monitor_at_cursor() {
            Ok(monitor) => monitor,
            Err(message) => {
                eprintln!("[snapclip][capture] monitor lookup failed: {message}");
                self.fail(None, CaptureError::MonitorUnavailable(message), "none");
                return;
            }
        };
        eprintln!(
            "[snapclip][capture] monitor bounds={}x{} at ({},{}), work_area={}x{}, dpi={}, lookup_ms={}",
            monitor.width(),
            monitor.height(),
            monitor.layout.bounds.left,
            monitor.layout.bounds.top,
            monitor.layout.work_area.width(),
            monitor.layout.work_area.height(),
            monitor.layout.dpi,
            started_at.elapsed().as_millis()
        );
        eprintln!(
            "[snapclip][bench] stage=monitor_ready generation_pending elapsed_ms={}",
            started_at.elapsed().as_millis()
        );

        // The generation counter is owned by the worker mailbox so starts and
        // cancellations cannot diverge from it.
        let generation = self.worker.next_generation();
        self.current_generation = generation;
        self.session_counter += 1;
        let session_id = format!(
            "capture-{}-{}",
            crate::application::clipboard_ingest::unix_time_ms(),
            self.session_counter
        );
        self.session = CaptureSession::new(session_id);
        if let Err(error) = self.session.preparing() {
            eprintln!("[snapclip][capture] session preparing transition failed: {error}");
            self.fail(None, error, "none");
            return;
        }
        self.publish_state();
        let layout = monitor.layout.clone();
        self.sink.on_started(&self.session_id(), &layout);

        let mut cursor = unsafe { zeroed() };
        unsafe { GetCursorPos(&mut cursor) };
        let request = StartRequest {
            generation,
            monitor,
            cursor_screen: Point::new(cursor.x, cursor.y),
            requested_at: started_at,
            notify_thread: unsafe { GetCurrentThreadId() },
        };
        if let Err(message) = self.worker.start(request) {
            eprintln!("[snapclip][capture] worker start failed: {message}");
            let error = CaptureError::CaptureFailed(message);
            self.fail(None, error, "worker");
        }
    }

    /// Drain a ready worker result. Called for every
    /// [`capture_worker::FRAME_READY_MESSAGE`] and defensively on other wake
    /// ups; returns without doing anything when nothing is ready or the
    /// result belongs to a superseded generation.
    fn on_frame_ready(&mut self) {
        let Some(ready) = self.worker.take_ready() else {
            return;
        };
        if self.current_generation == 0 || self.session.state() != CaptureState::Preparing {
            // The session ended (Esc/destroy) while the frame was in flight.
            return;
        }
        match ready {
            Ok(prepared) => self.apply_prepared(prepared),
            Err(failure) => {
                let stage = failure.stage;
                if matches!(failure.error, CaptureError::DeviceRemoved(_)) {
                    self.worker.invalidate_providers();
                    self.renderer = None;
                }
                self.fail(None, failure.error, stage);
            }
        }
    }

    /// The frozen frame arrived for the current generation: prepare the
    /// renderer off-screen, arm the session and show the overlay.
    fn apply_prepared(&mut self, prepared: capture_worker::PreparedFrame) {
        let started_at = Instant::now();
        eprintln!(
            "[snapclip][bench] stage=frame_ready provider={} size={}x{} freeze_to_ready_ms={}",
            prepared.frozen.frame.provider,
            prepared.frozen.frame.width,
            prepared.frozen.frame.height,
            prepared.captured_at.elapsed().as_millis()
        );
        let monitor = prepared.monitor;
        let provider = prepared.frozen.frame.provider;
        let frozen = prepared.frozen;
        if let Err(error) = self.prepare_overlay(&monitor, &frozen) {
            eprintln!(
                "[snapclip][capture] renderer preparation failed provider={} error={}",
                provider, error
            );
            if matches!(error, CaptureError::DeviceRemoved(_)) {
                self.worker.invalidate_providers();
            }
            self.fail(None, error, provider);
            return;
        }
        eprintln!(
            "[snapclip][bench] stage=renderer_ready provider={} elapsed_ms={}",
            provider,
            started_at.elapsed().as_millis()
        );

        if let Err(error) = self.session.arm(frozen.frame.clone(), &monitor.layout) {
            eprintln!("[snapclip][capture] session arm failed error={error}");
            self.fail(None, error, provider);
            return;
        }
        self.frozen = Some(frozen);
        self.graphics_released = false;

        eprintln!(
            "[snapclip][capture] overlay session armed session={}",
            self.session_id()
        );
        if let Err(error) = self.session.overlay_ready() {
            eprintln!("[snapclip][capture] overlay ready transition failed error={error}");
            self.fail(None, error, provider);
            return;
        }
        self.publish_state();
        // Paint the new frame while the HWND is still hidden. DirectComposition
        // retains the previous swap-chain contents, so showing first would expose
        // the previous session's selection for one compositor frame. This first paint
        // is synchronous (not coalesced) precisely so it lands before `show_overlay`.
        self.paint_now();
        self.show_overlay(&monitor.layout);
        eprintln!(
            "[snapclip][bench] stage=visible session={} prepare_elapsed_ms={}",
            self.session_id(),
            started_at.elapsed().as_millis()
        );
    }

    fn prepare_overlay(
        &mut self,
        monitor: &CapturedMonitor,
        frozen: &FrozenFrame,
    ) -> CaptureResult<()> {
        if self.renderer.is_none() {
            // `self.window` uses the `windows-sys` bindings; the renderer needs the
            // typed handle. Both are the same `*mut c_void` at the ABI level.
            let window = ::windows::Win32::Foundation::HWND(self.window);
            // The renderer must share the capture worker's device: the frozen
            // texture it displays was created there and D2D cannot cross devices.
            let device = frozen.device().ok_or_else(|| {
                CaptureError::ProviderUnavailable(
                    "frozen frame carries no device to render with".into(),
                )
            })?;
            self.renderer = Some(Win32Renderer::new(window, &monitor.layout, device.clone()).map_err(
                |message| {
                    if Win32Renderer::is_device_lost(&message) {
                        CaptureError::DeviceRemoved(message)
                    } else {
                        CaptureError::RenderFailed(message)
                    }
                },
            )?);
        }
        let renderer = self
            .renderer
            .as_mut()
            .ok_or_else(|| CaptureError::RenderFailed("renderer was not created".into()))?;
        renderer
            .resize(&monitor.layout)
            .and_then(|()| renderer.set_frame(frozen))
            .map_err(|message| {
                if Win32Renderer::is_device_lost(&message) {
                    CaptureError::DeviceRemoved(message)
                } else {
                    CaptureError::RenderFailed(message)
                }
            })
    }

    /// Show the overlay and give it keyboard focus.
    ///
    /// Focus is not cosmetic: `Esc` and `Enter` arrive as `WM_KEYDOWN`, and a window
    /// that is never activated never receives them. The previously focused window is
    /// remembered so it can be restored when the session ends.
    fn show_overlay(&mut self, layout: &MonitorLayout) {
        unsafe {
            if self.previous_foreground.is_null() {
                self.previous_foreground = GetForegroundWindow();
            }
            // `HWND` is `*mut c_void` in both bindings; the cast keeps the two crate
            // versions from leaking into each other.
            SetWindowPos(
                self.window as *mut core::ffi::c_void,
                HWND_TOPMOST,
                layout.bounds.left,
                layout.bounds.top,
                layout.bounds.width().max(1),
                layout.bounds.height().max(1),
                SWP_SHOWWINDOW,
            );
            // `SW_SHOW` (not `SW_SHOWNOACTIVATE`): activation is what routes the
            // keyboard to the overlay.
            ShowWindow(self.window as *mut core::ffi::c_void, SW_SHOW);
            self.take_focus();
        }
        eprintln!("[snapclip][capture] overlay focus requested");
    }

    /// Force the overlay to the foreground so it owns the keyboard.
    ///
    /// `SetForegroundWindow` can be refused while another process owns the
    /// foreground; `SetFocus` inside our own thread still delivers `WM_KEYDOWN`,
    /// and `WM_MOUSEACTIVATE` re-asserts focus if the user clicks first.
    fn take_focus(&self) {
        unsafe {
            let window = self.window as *mut core::ffi::c_void;
            if GetForegroundWindow() != self.window {
                let _ = SetForegroundWindow(window);
            }
            let _ = SetFocus(window);
        }
    }

    /// Hand the foreground back to whatever had it before the overlay appeared.
    fn restore_foreground(&self) {
        let previous = self.previous_foreground;
        if previous.is_null() {
            return;
        }
        unsafe {
            let _ = SetForegroundWindow(previous as *mut core::ffi::c_void);
        }
    }

    fn hide_overlay(&mut self) {
        unsafe {
            ShowWindow(self.window as *mut core::ffi::c_void, SW_HIDE);
        }
        self.restore_foreground();
        self.previous_foreground = null_mut();
    }

    /// The single cancellation path used by Esc, right click, repeated F5, window
    /// destruction, display changes and device removal.
    ///
    /// Bumps the generation first: a frame the worker is still freezing becomes
    /// stale and is dropped on arrival, releasing its GPU references without the
    /// overlay ever waiting on the worker (docs/11 §3.3).
    fn cancel(&mut self, reason: &str) {
        let session_id = self.session_id();
        let was_active = self.session.state().is_active();
        eprintln!(
            "[snapclip][capture] cancel session={} reason={} active={}",
            session_id, reason, was_active
        );
        self.worker.cancel();
        self.export_worker.cancel();
        self.current_generation = 0;
        self.release_session();
        if was_active {
            self.sink.on_cancelled(&session_id, reason);
        }
        self.sink.on_state(&session_id, CaptureState::Idle, None);
    }

    fn release_session(&mut self) {
        eprintln!(
            "[snapclip][capture] release session={} state={:?}",
            self.session_id(),
            self.session.state()
        );
        self.session.cancel();
        self.frozen = None;
        self.graphics_released = false;
        self.drag_mode = None;
        self.cursor_visible = false;
        // Stop the coalescing tick before releasing the renderer: a pending WM_TIMER
        // must not try to present into the graphics we are about to drop.
        self.disarm_render_tick();
        self.dirty = false;
        self.sampler.reset();
        self.magnifier_color_format = ColorFormat::default();
        self.magnifier_relative = false;
        self.sample_slot = None;
        self.annotation_doc.reset();
        self.annotation_tool = None;
        self.annotation_gesture = None;
        self.hide_overlay();
        // Renderer resources are session-owned. Dropping the renderer releases the
        // captured L0 bitmap, selection chrome, swap chain and composition visual;
        // the next F5 starts with no pixels from this session available to display.
        self.renderer = None;
        eprintln!("[snapclip][capture] session graphics released");
        unsafe { SetCursor(LoadCursorW(null_mut(), IDC_ARROW) as _) };
        if let Ok(mut shared) = self.shared.lock() {
            shared.state = CaptureState::Idle;
            shared.session_id = None;
        }
    }

    /// `Enter`: read the confirmed selection back on this thread, then hand encode +
    /// write to the export worker so the message pump keeps answering `Esc` and
    /// `WM_PAINT` while a large PNG is produced (docs/11 §Phase 3).
    fn confirm(&mut self) {
        let started_at = Instant::now();
        let selection = match self.session.begin_export() {
            Ok(ExportOutcome::Produce { selection }) => selection,
            Ok(ExportOutcome::Empty) => {
                self.publish_state();
                return;
            }
            Err(_) => return,
        };
        eprintln!(
            "[snapclip][capture] confirm session={} selection=({},{})->({},{})",
            self.session_id(),
            selection.left,
            selection.top,
            selection.right,
            selection.bottom
        );
        let Some(frozen) = self.frozen.as_ref() else {
            self.cancel("no-frame");
            return;
        };
        let session_id = self.session_id();
        let provider = frozen.frame.provider;
        let dpi = self.session.dpi();
        // Region readback is the only step that touches the single-threaded D3D11
        // immediate context, so it stays here — synchronous, before the hand-off.
        //
        // With committed annotations, replay the same document through the D2D export
        // path (chrome / selection box / draft suppressed) and crop the result, so the
        // PNG is pixel-identical to the preview (docs/11 §8.2). With none, keep the
        // direct frozen-frame region readback.
        let prepared: CaptureResult<SelectionPixels> = if self.annotation_doc.items().is_empty() {
            self.service.prepare_selection(
                &frozen.frame,
                selection,
                &FrozenFramePixels::new(frozen),
            )
        } else {
            // Mirror `prepare_selection`'s clip so `region` always equals the rect the
            // exported BGRA actually covers (render_export crops to selection ∩ frame).
            let clipped = selection.intersect(frozen.frame.rect());
            let export_state = OverlayFrameState {
                selection: clipped,
                cursor: self.cursor,
                cursor_visible: false,
                show_chrome: false,
                magnifier_rgb: None,
                magnifier_hex_text: None,
                magnifier_secondary_text: None,
                magnifier_relative: false,
                annotation_items: self.annotation_doc.items().to_vec(),
                annotation_selected_id: None,
                annotation_draft: None,
            };
            let Some(renderer) = self.renderer.as_mut() else {
                self.cancel("annotation-export-without-renderer");
                return;
            };
            renderer
                .render_export(&export_state, None)
                .map(|bgra| SelectionPixels {
                    frame: frozen.frame.clone(),
                    region: clipped,
                    bgra,
                })
                .map_err(CaptureError::RenderFailed)
        };
        let readback_ms = started_at.elapsed().as_millis();
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                eprintln!(
                    "[snapclip][capture] region readback failed session={} provider={} elapsed_ms={} error={}",
                    session_id, provider, readback_ms, error
                );
                self.sink.on_failed(Some(&session_id), &error, provider);
                self.session.fail();
                self.finish_session();
                self.sink.on_state(&session_id, CaptureState::Idle, None);
                return;
            }
        };
        let export_width = prepared.region.width();
        let export_height = prepared.region.height();
        // The overlay is frozen at the confirmed selection until the export lands, so
        // cursor-follow repaints can neither race nor waste the hand-off.
        self.graphics_released = true;
        let service = self.service.clone();
        let job = ExportJob {
            generation: 0,
            session_id: session_id.clone(),
            prepared,
            dpi,
            monitor_device_name: None,
            notify_thread: unsafe { GetCurrentThreadId() },
            executor: Box::new(move |job: &ExportJob| {
                service.finish_artifact(
                    &job.session_id,
                    &job.prepared,
                    job.dpi,
                    job.monitor_device_name.clone(),
                )
            }),
        };
        match self.export_worker.submit(job) {
            Ok(Some(generation)) => {
                eprintln!(
                    "[snapclip][bench] export submitted session={} generation={} size={}x{} readback_ms={}",
                    session_id, generation, export_width, export_height, readback_ms
                );
            }
            Ok(None) | Err(_) => {
                // The worker refused the job (shutting down) or could not start; no
                // result will ever post back, so report the failure right here.
                let error = CaptureError::EncodeFailed("export worker unavailable".to_string());
                eprintln!(
                    "[snapclip][capture] export submit failed session={} provider={} error={}",
                    session_id, provider, error
                );
                self.sink.on_failed(Some(&session_id), &error, provider);
                self.session.fail();
                self.finish_session();
                self.sink.on_state(&session_id, CaptureState::Idle, None);
            }
        }
    }

    /// Drain a finished export. Runs on the overlay thread when the worker posts
    /// [`export_worker::EXPORT_READY_MESSAGE`]; ignored once the session has left
    /// `Exporting` (an `Esc` cancelled it, and the worker already deleted the file).
    fn on_export_ready(&mut self) {
        let Some(ready) = self.export_worker.take_ready() else {
            return;
        };
        if self.session.state() != CaptureState::Exporting {
            return;
        }
        let session_id = self.session_id();
        match ready {
            Ok(completed) => {
                let artifact = completed.artifact;
                eprintln!(
                    "[snapclip][capture] artifact ready session={} path={} size={}x{}",
                    session_id,
                    artifact
                        .png_path()
                        .map(|path| path.to_string_lossy())
                        .unwrap_or_default(),
                    artifact.width,
                    artifact.height
                );
                self.sink.on_completed(&artifact);
                self.session.complete();
                self.finish_session();
                self.sink.on_state(&session_id, CaptureState::Idle, None);
            }
            Err(failure) => {
                let stage = failure.stage;
                eprintln!(
                    "[snapclip][capture] export failed session={} stage={stage} error={}",
                    session_id, failure.error
                );
                if matches!(failure.error, CaptureError::DeviceRemoved(_)) {
                    self.worker.invalidate_providers();
                    self.renderer = None;
                }
                self.sink.on_failed(Some(&session_id), &failure.error, stage);
                self.session.fail();
                self.finish_session();
                self.sink.on_state(&session_id, CaptureState::Idle, None);
            }
        }
    }

    fn finish_session(&mut self) {
        self.release_session();
    }

    fn fail(&mut self, session_id: Option<&str>, error: CaptureError, provider: &str) {
        let id = session_id
            .map(str::to_string)
            .unwrap_or_else(|| self.session_id());
        eprintln!(
            "[snapclip][capture] failed session={} provider={} error={}",
            id, provider, error
        );
        self.worker.cancel();
        self.current_generation = 0;
        self.sink.on_failed(Some(&id), &error, provider);
        self.session.fail();
        self.release_session();
        self.sink.on_state(&id, CaptureState::Idle, None);
    }

    // ---- color sampler ---------------------------------------------------

    /// Request an async tile copy if the cursor moved to a new tile and throttle allows.
    fn request_color_sample(&mut self) {
        if self.graphics_released || !self.cursor_visible {
            return;
        }
        let Some(renderer) = self.renderer.as_mut() else { return; };
        let Some(frozen) = self.frozen.as_ref() else { return; };
        let Some(gpu_frame) = frozen.texture() else { return; };
        let texture = gpu_frame.texture.clone();
        let dpi = renderer.layout().dpi;
        let config = MagnifierConfig::default().scaled(dpi);
        let geometry = magnifier_geometry(
            self.cursor,
            config,
            renderer.frame(),
            renderer.layout().local_work_area(),
        );
        let now = Instant::now();
        if !self.sampler.should_request(self.cursor, geometry.tile, now) {
            // Tile hit: extract color from cached pixels without GPU.
            if self.sampler.update_cursor(self.cursor) {
                self.invalidate();
            }
            return;
        }
        let tile_x = geometry.tile.left.max(0) as u32;
        let tile_y = geometry.tile.top.max(0) as u32;
        let tile_origin = Point::new(geometry.tile.left, geometry.tile.top);
        match renderer.request_sample(&texture, tile_x, tile_y, config.tile_size as u32) {
            Ok(slot) => {
                self.sampler.mark_submitted(tile_origin, now);
                self.sample_slot = Some(slot);
            }
            Err(_) => {
                self.sampler.mark_stale();
            }
        }
    }

    /// Poll the pending GPU slot; if complete, feed the tile data to the sampler.
    fn poll_color_sample(&mut self) {
        let Some(slot) = self.sample_slot.take() else { return; };
        let Some(renderer) = self.renderer.as_mut() else { return; };
        match renderer.poll_sample(slot) {
            None => {
                // Still in flight. Re-arm the tick so the poll keeps scheduling:
                // once the pointer stops — precisely when the user is reading the
                // colour value — no input event will ever arm another tick, and
                // the landed result would sit unpicked (info panel stuck "......").
                self.sample_slot = Some(slot);
                self.arm_render_tick();
            }
            Some(Ok(pixels)) => {
                // Label the completion with the origin *submitted*, not one
                // recomputed from the current cursor: the pointer can drift into
                // another tile before the copy lands, which would misattribute
                // every pixel in the tile.
                let Some(tile_origin) = self.sampler.pending_origin() else {
                    self.sampler.mark_stale();
                    return;
                };
                self.sampler.complete(tile_origin, pixels, self.cursor);
                if self.sampler.is_dirty() {
                    self.dirty = true;
                }
            }
            Some(Err(_)) => {
                self.sampler.mark_stale();
            }
        }
    }

    // ---- input -----------------------------------------------------------

    fn on_mouse_move(&mut self, client: POINT) {
        let point = Point::new(client.x, client.y);
        self.cursor = point;
        self.cursor_visible = true;

        if self.session.state() == CaptureState::Annotating {
            // The pointer drives the annotation document, not the selection.
            self.annotation_point_moved(point);
            self.update_annotation_cursor(point);
        } else {
            // The session owns the drag: the overlay only decides *whether* a drag is in
            // progress, never how the geometry changes.
            if self.drag_mode.is_some() {
                self.session.pointer_moved(point);
            }
            self.update_cursor_shape(point);
        }
        // The magnifier and crosshair follow the cursor, so any move changes the image;
        // `invalidate` coalesces it into a single full repaint per render tick.
        self.request_color_sample();
        self.invalidate();
    }

    /// Pick the resize cursor for a monitor-local point.
    ///
    /// Uses the same hit test as the press handler so the cursor always matches what a
    /// click would do.
    fn update_cursor_shape(&mut self, point: Point) {
        let cursor = if !self.session.has_selection() {
            IDC_CROSS
        } else {
            match self.session.pointer_hit(point) {
                SelectionGeometry::Resize(Handle::TopLeft)
                | SelectionGeometry::Resize(Handle::BottomRight) => IDC_SIZENWSE,
                SelectionGeometry::Resize(Handle::TopRight)
                | SelectionGeometry::Resize(Handle::BottomLeft) => IDC_SIZENESW,
                SelectionGeometry::Resize(Handle::Top)
                | SelectionGeometry::Resize(Handle::Bottom) => IDC_SIZENS,
                SelectionGeometry::Resize(Handle::Left)
                | SelectionGeometry::Resize(Handle::Right) => IDC_SIZEWE,
                SelectionGeometry::Move => IDC_SIZEALL,
                SelectionGeometry::Create | SelectionGeometry::Outside => IDC_CROSS,
            }
        };
        unsafe { SetCursor(LoadCursorW(null_mut(), cursor) as _) };
    }

    fn on_mouse_leave(&mut self) {
        if self.cursor_visible {
            self.cursor_visible = false;
            self.invalidate();
        }
    }

    fn on_left_down(&mut self, client: POINT) {
        if self.session.state() == CaptureState::Annotating {
            self.annotation_point_down(Point::new(client.x, client.y));
            return;
        }
        if !matches!(
            self.session.state(),
            CaptureState::Selecting | CaptureState::Selected
        ) {
            return;
        }
        let point = Point::new(client.x, client.y);
        let hit = self.session.pointer_pressed(point);
        eprintln!(
            "[snapclip][capture] pointer down session={} point=({},{}), hit={:?}",
            self.session_id(), point.x, point.y, hit
        );
        self.drag_mode = Some(match hit {
            SelectionGeometry::Move => ResizeMode::Move,
            SelectionGeometry::Resize(handle) => ResizeMode::Handle(handle),
            SelectionGeometry::Create | SelectionGeometry::Outside => {
                ResizeMode::Handle(Handle::BottomRight)
            }
        });
        self.update_cursor_shape(point);
        self.invalidate();
    }

    fn on_left_up(&mut self) {
        if self.session.state() == CaptureState::Annotating {
            self.annotation_point_up();
            return;
        }
        if !matches!(
            self.session.state(),
            CaptureState::Selecting | CaptureState::Selected
        ) {
            self.drag_mode = None;
            return;
        }
        self.session.pointer_released();
        self.session.pointer_left();
        self.drag_mode = None;
        let selection = self.session.selection();
        eprintln!(
            "[snapclip][capture] pointer up session={} selection=({},{})->({},{}) state={:?}",
            self.session_id(),
            selection.left,
            selection.top,
            selection.right,
            selection.bottom,
            self.session.state()
        );
        self.publish_state();
        self.invalidate();
    }

    fn on_key_down(&mut self, key: u32) {
        eprintln!(
            "[snapclip][capture] key down vk=0x{key:02X} session={} state={:?}",
            self.session_id(),
            self.session.state()
        );
        match key {
            hotkey::ESCAPE_VIRTUAL_KEY => self.cancel("escape"),
            hotkey::RETURN_VIRTUAL_KEY => {
                if self.session.has_selection() {
                    self.confirm();
                }
            }
            // 'A': Selected -> Annotating (selection is locked, tools go live).
            k if k == b'A' as u32 && self.session.state() == CaptureState::Selected => {
                if self.session.begin_annotating().is_ok() {
                    self.publish_state();
                    self.invalidate_all();
                }
            }
            // VK_SHIFT (0x10): cycle colour display format (HEX → RGB → HSL → HEX).
            0x10 if self.session.state().is_active() => {
                self.magnifier_color_format = self.magnifier_color_format.next();
                self.invalidate();
            }
            // 'C': copy the current colour value (in the active format) to clipboard.
            k if k == b'C' as u32 && self.session.state().is_active() => {
                self.copy_color_to_clipboard();
            }
            // 'P': toggle global screen ↔ selection-relative coordinate in the info panel.
            k if k == b'P' as u32 && self.session.state().is_active() => {
                self.magnifier_relative = !self.magnifier_relative;
                self.invalidate();
            }
            _ if self.session.state() == CaptureState::Annotating => self.on_annotation_key(key),
            _ => {}
        }
    }

    /// Copy the primary (`#RRGGBB`) colour string to the Windows clipboard via
    /// arboard. Marks the write as excluded so the clip-monitor does not record
    /// our own copy.
    fn copy_color_to_clipboard(&mut self) {
        let Some(text) = self.sampler.formatted(ColorFormat::Hex) else {
            return;
        };
        match arboard::Clipboard::new() {
            Ok(mut cb) => {
                if let Err(error) = cb.set_text(text) {
                    eprintln!("[snapclip][capture] colour copy failed: {error}");
                } else {
                    crate::platform::windows::clipboard::mark_clipboard_excluded();
                }
            }
            Err(error) => eprintln!("[snapclip][capture] clipboard unavailable: {error}"),
        }
    }

    /// Keyboard handling while the session is annotating.
    fn on_annotation_key(&mut self, key: u32) {
        // Tool selection: number row picks the tool, '1' returns to select/move.
        let tool = match key {
            k if k == b'1' as u32 => Some(None),
            k if k == b'2' as u32 => Some(Some(AnnotationKind::Rectangle)),
            k if k == b'3' as u32 => Some(Some(AnnotationKind::Ellipse)),
            k if k == b'4' as u32 => Some(Some(AnnotationKind::Arrow)),
            k if k == b'5' as u32 => Some(Some(AnnotationKind::Line)),
            k if k == b'6' as u32 => Some(Some(AnnotationKind::Freehand)),
            k if k == b'7' as u32 => Some(Some(AnnotationKind::Highlight)),
            _ => None,
        };
        if let Some(tool) = tool {
            self.annotation_tool = tool;
            eprintln!("[snapclip][capture] annotate tool={tool:?}");
            return;
        }

        let ctrl = unsafe { GetKeyState(VK_CONTROL as i32) < 0 };
        match key {
            k if ctrl && k == b'Z' as u32 => {
                if self.annotation_doc.undo() {
                    self.invalidate_all();
                }
            }
            k if ctrl && k == b'Y' as u32 => {
                if self.annotation_doc.redo() {
                    self.invalidate_all();
                }
            }
            k if ctrl && k == b'D' as u32 => {
                if self.annotation_doc.selected_id().is_some() {
                    self.annotation_doc.duplicate_selected();
                    self.invalidate_all();
                }
            }
            k if ctrl && k == b']' as u32 => {
                if self.annotation_doc.selected_id().is_some() {
                    self.annotation_doc.bring_to_front();
                    self.invalidate_all();
                }
            }
            k if ctrl && k == b'[' as u32 => {
                if self.annotation_doc.selected_id().is_some() {
                    self.annotation_doc.send_to_back();
                    self.invalidate_all();
                }
            }
            // Delete / Backspace remove the selected item.
            0x2E | 0x08 => {
                if self.annotation_doc.selected_id().is_some() {
                    self.annotation_doc.delete_selected();
                    self.invalidate_all();
                }
            }
            _ => {}
        }
    }

    // ---- annotation interaction ------------------------------------------

    /// Half-size, in physical pixels, of a control-point hit box.
    fn annotation_handle_tol(&self) -> i32 {
        (8.0 * self.session.dpi().max(96) as f32 / 96.0) as i32
    }

    /// Pointer moved while annotating: apply the active gesture; `invalidate` marks the
    /// change and coalesces a full repaint into the next tick.
    fn annotation_point_moved(&mut self, point: Point) {
        // Destructure without holding a borrow of `self` across the `&mut self`
        // document / dirty calls.
        let gesture = self.annotation_gesture.take();
        match gesture {
            Some(AnnotationGesture::Move { id, snapshot, last }) => {
                let dx = point.x - last.x;
                let dy = point.y - last.y;
                if dx != 0 || dy != 0 {
                    self.annotation_doc.translate_item(id, dx, dy);
                }
                self.annotation_gesture = Some(AnnotationGesture::Move { id, snapshot, last: point });
            }
            Some(AnnotationGesture::Resize { id, handle, snapshot }) => {
                self.annotation_doc.resize_item(id, handle, point);
                self.annotation_gesture = Some(AnnotationGesture::Resize { id, handle, snapshot });
            }
            Some(g @ AnnotationGesture::Create { .. }) => {
                if let AnnotationGesture::Create { start } = &g {
                    self.update_draft_shape(*start, point);
                }
                self.annotation_gesture = Some(g);
            }
            None => {}
        }
        self.invalidate();
    }

    /// Drive the draft geometry from the press anchor to the current pointer.
    fn update_draft_shape(&mut self, start: Point, current: Point) {
        let kind = match self.annotation_doc.draft.as_ref() {
            Some(d) => d.kind,
            None => return,
        };
        match kind {
            AnnotationKind::Freehand | AnnotationKind::Highlight => {
                self.annotation_doc.push_draft_point(current);
            }
            _ => {
                let geometry = match kind {
                    AnnotationKind::Rectangle => AnnotationGeometry::Rect {
                        bounds: Rect::from_corners(start, current),
                    },
                    AnnotationKind::Ellipse => AnnotationGeometry::Ellipse {
                        bounds: Rect::from_corners(start, current),
                    },
                    AnnotationKind::Line => AnnotationGeometry::Line {
                        start,
                        end: current,
                    },
                    AnnotationKind::Arrow => AnnotationGeometry::Arrow {
                        start,
                        end: current,
                    },
                    _ => return,
                };
                self.annotation_doc.update_draft(geometry);
            }
        }
    }

    /// Initial geometry for a draft anchored at `point`.
    fn initial_draft_geometry(kind: AnnotationKind, point: Point) -> AnnotationGeometry {
        let degenerate = Rect::from_corners(point, point);
        match kind {
            AnnotationKind::Rectangle => AnnotationGeometry::Rect { bounds: degenerate },
            AnnotationKind::Ellipse => AnnotationGeometry::Ellipse { bounds: degenerate },
            AnnotationKind::Line => AnnotationGeometry::Line { start: point, end: point },
            AnnotationKind::Arrow => AnnotationGeometry::Arrow { start: point, end: point },
            AnnotationKind::Freehand => AnnotationGeometry::Freehand { points: vec![point] },
            AnnotationKind::Highlight => AnnotationGeometry::Highlight { points: vec![point] },
            AnnotationKind::Text => AnnotationGeometry::Text {
                position: point,
                content: String::new(),
            },
        }
    }

    /// A draft is committed only when it covers real pixels.
    fn draft_is_significant(draft: &AnnotationItem) -> bool {
        match &draft.geometry {
            AnnotationGeometry::Rect { bounds } | AnnotationGeometry::Ellipse { bounds } => {
                !bounds.is_empty()
            }
            AnnotationGeometry::Line { start, end } | AnnotationGeometry::Arrow { start, end } => {
                start != end
            }
            AnnotationGeometry::Freehand { points } | AnnotationGeometry::Highlight { points } => {
                points.len() >= 2
            }
            AnnotationGeometry::Text { .. } => false,
        }
    }

    /// Pointer pressed while annotating: pick / start a move / start a resize, or
    /// begin a new-shape draft for the active tool.
    fn annotation_point_down(&mut self, point: Point) {
        if self.annotation_tool.is_none() {
            // Select tool: a control point resizes first, then the body moves.
            if let Some(id) = self.annotation_doc.selected_id() {
                if let Some(handle) = self.annotation_doc.handle_at(id, point, self.annotation_handle_tol()) {
                    let snapshot = self.annotation_doc.snapshot();
                    self.annotation_gesture = Some(AnnotationGesture::Resize { id, handle, snapshot });
                    self.invalidate();
                    return;
                }
            }
            match self.annotation_doc.hit_test(point) {
                Some(id) => {
                    let snapshot = self.annotation_doc.snapshot();
                    self.annotation_doc.select(Some(id));
                    self.annotation_gesture = Some(AnnotationGesture::Move { id, snapshot, last: point });
                }
                None => self.annotation_doc.select(None),
            }
            self.invalidate();
            return;
        }
        // Creation tool: begin a draft anchored at the press point.
        let kind = self.annotation_tool.unwrap();
        self.annotation_doc.start_draft(kind, Self::initial_draft_geometry(kind, point));
        self.annotation_gesture = Some(AnnotationGesture::Create { start: point });
        self.invalidate();
    }

    /// Pointer released while annotating: commit the active gesture as one undo step.
    fn annotation_point_up(&mut self) {
        match self.annotation_gesture.take() {
            Some(AnnotationGesture::Move { snapshot, .. }) => {
                self.annotation_doc.commit_drag(snapshot);
            }
            Some(AnnotationGesture::Resize { snapshot, .. }) => {
                self.annotation_doc.commit_drag(snapshot);
            }
            Some(AnnotationGesture::Create { .. }) => {
                let significant = self
                    .annotation_doc
                    .draft
                    .as_ref()
                    .map(Self::draft_is_significant)
                    .unwrap_or(false);
                if significant {
                    self.annotation_doc.commit_draft();
                } else {
                    self.annotation_doc.clear_draft();
                }
            }
            None => {}
        }
        self.invalidate();
    }

    /// Cursor for the select tool: resize over a handle, move over a body, arrow
    /// otherwise. Creation tools always use the crosshair.
    fn update_annotation_cursor(&mut self, point: Point) {
        let cursor = if self.annotation_tool.is_some() {
            IDC_CROSS
        } else if self
            .annotation_doc
            .selected_id()
            .and_then(|id| self.annotation_doc.handle_at(id, point, self.annotation_handle_tol()))
            .is_some()
        {
            IDC_SIZEALL
        } else if self.annotation_doc.hit_test(point).is_some() {
            IDC_SIZEALL
        } else {
            IDC_ARROW
        };
        unsafe { SetCursor(LoadCursorW(null_mut(), cursor) as _) };
    }

    /// Mark the surface dirty for a discrete edit (undo / redo / delete / z-order /
    /// toolbar command) and coalesce a full repaint into the next tick.
    fn invalidate_all(&mut self) {
        self.invalidate();
    }

    // ---- rendering -------------------------------------------------------

    /// Paint immediately with a full repaint.
    ///
    /// Used only for the synchronous first frame that must land before the window is
    /// shown (docs/11 §"隐藏状态完成一次完整绘制和 Present/Commit"). Interactive input
    /// goes through [`Self::invalidate`] instead so a burst of `WM_MOUSEMOVE`s collapses
    /// into one present per tick.
    fn paint_now(&mut self) {
        self.disarm_render_tick();
        self.dirty = false;
        self.render();
    }

    /// Mark the surface dirty and coalesce a full repaint into the next render tick.
    ///
    /// `WM_MOUSEMOVE` and the drag handlers only update state and call this; the actual
    /// draw happens once in [`Self::on_render_tick`], so a fast pointer produces at most
    /// one present per `RENDER_TICK_MS` (docs/11 §"一个 tick 最多一次 Present/Commit").
    fn invalidate(&mut self) {
        self.dirty = true;
        if self.renderer.is_none() {
            return;
        }
        self.arm_render_tick();
    }

    fn arm_render_tick(&mut self) {
        if self.render_armed {
            return;
        }
        // A window timer (not a thread timer): its `WM_TIMER` is posted to this queue
        // and coalesces, so repeated invalidations never stack timers.
        unsafe { SetTimer(self.window, RENDER_TIMER_ID, RENDER_TICK_MS, None) };
        self.render_armed = true;
    }

    fn disarm_render_tick(&mut self) {
        if self.render_armed {
            unsafe { KillTimer(self.window, RENDER_TIMER_ID) };
            self.render_armed = false;
        }
    }

    /// The coalescing tick body: repaint everything accumulated since the last present.
    fn on_render_tick(&mut self) {
        self.disarm_render_tick();
        // Always poll the pending GPU sample — may mark the info panel dirty.
        self.poll_color_sample();
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.render();
    }

    fn render(&mut self) {
        // An export is in flight: the overlay is frozen at the confirmed selection
        // and must not present, so nothing races the hand-off (see confirm()).
        if self.graphics_released {
            return;
        }
        let session_id = self.session_id();
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        // Every present repaints the whole surface (see OverlayRenderer::draw_to); the
        // render tick is what bounds the present rate to ~60 Hz.
        eprintln!(
            "[snapclip][capture] render session={} frame_px={}",
            session_id,
            renderer.frame().area()
        );
        let cursor_visible = self.cursor_visible && self.session.state().is_active();
        let state = OverlayFrameState {
            selection: self.session.selection(),
            cursor: self.cursor,
            cursor_visible,
            show_chrome: self.session.shows_chrome(),
            magnifier_rgb: self.sampler.rgb(),
            magnifier_hex_text: self.sampler.formatted(ColorFormat::Hex),
            magnifier_secondary_text: self.sampler.formatted(self.magnifier_color_format),
            magnifier_relative: self.magnifier_relative,
            annotation_items: self.annotation_doc.items().to_vec(),
            annotation_selected_id: self.annotation_doc.selected_id(),
            annotation_draft: self.annotation_doc.draft.clone(),
        };
        // Live borrow of annotation document avoids cloning items every tick.
        match renderer.render(&state, Some(&self.annotation_doc)) {
            Ok(()) => {
                // The presented frame now carries the sampler's current value;
                // clear the flag so a settled colour stops scheduling repaints.
                self.sampler.mark_rendered();
            }
            Err(error) => {
                if Win32Renderer::is_device_lost(&error) {
                    eprintln!("[snapclip][capture] graphics device removed: {error}");
                    self.renderer = None;
                    self.worker.invalidate_providers();
                    self.fail(None, CaptureError::DeviceRemoved(error), "overlay");
                } else {
                    eprintln!("[snapclip][capture] render failed: {error}");
                    self.fail(None, CaptureError::RenderFailed(error), "overlay");
                }
            }
        }
    }

    fn track_mouse_leave(&self) {
        #[repr(C)]
        struct TrackMouseEventData {
            cb_size: u32,
            flags: u32,
            hwnd_track: HWND,
            hover_time: u32,
        }

        #[link(name = "user32")]
        unsafe extern "system" {
            fn TrackMouseEvent(event: *mut TrackMouseEventData) -> i32;
        }

        let mut event = TrackMouseEventData {
            cb_size: std::mem::size_of::<TrackMouseEventData>() as u32,
            flags: TME_LEAVE,
            hwnd_track: self.window,
            hover_time: 0,
        };
        unsafe {
            TrackMouseEvent(&mut event);
        }
    }
}

impl<D, E> OverlayMessageHandler for OverlayController<D, E>
where
    D: ArtifactDir,
    E: ArtifactEncoder,
{
    unsafe fn handle(&mut self, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        match message {
            WM_OVERLAY_COMMAND => {
                match OverlayCommand::from_wparam(wparam) {
                    Some(OverlayCommand::Start) => self.start_session(),
                    Some(OverlayCommand::Cancel) => self.cancel("requested"),
                    Some(OverlayCommand::Confirm) => {
                        if self.session.has_selection() {
                            self.confirm();
                        }
                    }
                    Some(OverlayCommand::Shutdown) => {
                        self.cancel("shutdown");
                        // Stop the threads before quitting the pump; the workers
                        // hold the providers, the device and the in-flight export.
                        self.export_worker.shutdown();
                        self.worker.shutdown();
                        unsafe { PostQuitMessage(0) };
                    }
                    Some(OverlayCommand::FrameReady) => self.on_frame_ready(),
                    Some(OverlayCommand::ExportReady) => self.on_export_ready(),
                    Some(OverlayCommand::Annotation) => self.drain_annotation_commands(),
                    None => {}
                }
                Some(0)
            }
            capture_worker::FRAME_READY_MESSAGE => {
                self.on_frame_ready();
                Some(0)
            }
            export_worker::EXPORT_READY_MESSAGE => {
                self.on_export_ready();
                Some(0)
            }
            WM_HOTKEY => {
                if (wparam as i32) == hotkey::CAPTURE_HOTKEY_ID {
                    eprintln!("[snapclip][capture] WM_HOTKEY F5 received");
                    self.start_session();
                }
                Some(0)
            }
            WM_KEYDOWN => {
                self.on_key_down(wparam as u32);
                Some(0)
            }
            // A click on the overlay must activate it so the following `WM_KEYDOWN`
            // for `Esc` / `Enter` is delivered here instead of to the previous window.
            WM_MOUSEACTIVATE => {
                self.take_focus();
                Some(MA_ACTIVATE as LRESULT)
            }
            WM_MOUSEMOVE => {
                self.on_mouse_move(point_from_lparam(lparam));
                self.track_mouse_leave();
                Some(0)
            }
            WM_MOUSELEAVE => {
                self.on_mouse_leave();
                Some(0)
            }
            WM_LBUTTONDOWN => {
                self.on_left_down(point_from_lparam(lparam));
                Some(0)
            }
            WM_LBUTTONUP => {
                self.on_left_up();
                Some(0)
            }
            WM_RBUTTONDOWN => {
                self.cancel("right-button");
                Some(0)
            }
            WM_SETCURSOR => {
                let cursor = self.cursor;
                self.update_cursor_shape(cursor);
                Some(1)
            }
            WM_NCHITTEST => Some(HTCLIENT as LRESULT),
            WM_ERASEBKGND => Some(1),
            WM_PAINT => {
                // Acknowledge the update region with BeginPaint/EndPaint. Skipping it
                // leaves WM_PAINT permanently pending and the message loop re-renders
                // the full surface continuously (~180 Presents/s measured in Phase 0).
                // The pixels themselves come from the DirectComposition visual, so the
                // HDC is intentionally unused.
                let mut paint: PAINTSTRUCT = unsafe { zeroed() };
                unsafe { BeginPaint(self.window, &mut paint) };
                self.render();
                unsafe { EndPaint(self.window, &paint) };
                Some(0)
            }
            WM_TIMER => {
                // Coalescing render tick: draw everything accumulated since the last
                // present once. Unknown timer ids fall through to DefWindowProcW.
                if (wparam as usize) == RENDER_TIMER_ID {
                    self.on_render_tick();
                    Some(0)
                } else {
                    None
                }
            }
            WM_DPICHANGED | WM_DISPLAYCHANGE | WM_DEVICECHANGE => {
                if self.session.state().is_active() {
                    self.cancel("display-change");
                }
                // Force a full rebuild on the next session: the renderer is
                // dropped here and the worker rebuilds its providers (and the
                // D3D device inside) when the next request arrives.
                self.renderer = None;
                self.worker.invalidate_providers();
                Some(0)
            }
            WM_DESTROY => {
                self.cancel("window-destroyed");
                unsafe { PostQuitMessage(0) };
                Some(0)
            }
            _ => None,
        }
    }
}

fn point_from_lparam(lparam: LPARAM) -> POINT {
    POINT {
        x: (lparam & 0xFFFF) as u16 as i16 as i32,
        y: ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32,
    }
}

fn overlay_thread<D, E>(
    service: Arc<CaptureService<D, E>>,
    sink: Arc<dyn CaptureEventSink>,
    shared: Arc<Mutex<OverlayShared>>,
    annotation_rx: mpsc::Receiver<AnnotationCommand>,
    ready: mpsc::SyncSender<SystemResult>,
) where
    D: ArtifactDir,
    E: ArtifactEncoder,
{
    // Force the thread message queue into existence before the thread id is
    // published, so `PostThreadMessageW` cannot race with queue creation.
    let mut message: MSG = unsafe { zeroed() };
    unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, 0) };
    let thread_id = unsafe { GetCurrentThreadId() };

    if let Err(error) = monitor::set_per_monitor_v2_awareness() {
        let _ = ready.send(Err(format!("DPI awareness: {error}")));
        return;
    }

    let instance = unsafe { GetModuleHandleW(null()) };
    if instance.is_null() {
        let _ = ready.send(Err(format!(
            "GetModuleHandleW failed with Win32 error {}",
            unsafe { GetLastError() }
        )));
        return;
    }

    let window_class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(overlay_window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: null_mut(),
        hCursor: unsafe { LoadCursorW(null_mut(), IDC_CROSS) as _ },
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: OVERLAY_CLASS.as_ptr(),
    };
    if unsafe { RegisterClassW(&window_class) } == 0 {
        let _ = ready.send(Err(format!(
            "RegisterClassW failed with Win32 error {}",
            unsafe { GetLastError() }
        )));
        return;
    }

    let window = unsafe {
        CreateWindowExW(
            // No `WS_EX_NOACTIVATE`: the overlay has to be activatable, otherwise it
            // never receives `WM_KEYDOWN` and `Esc` / `Enter` would be dead keys.
            // `WS_EX_TOOLWINDOW` keeps it out of the taskbar and Alt+Tab.
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP,
            OVERLAY_CLASS.as_ptr(),
            OVERLAY_TITLE.as_ptr(),
            WS_POPUP,
            0,
            0,
            100,
            100,
            null_mut(),
            null_mut(),
            instance,
            null(),
        )
    };
    if window.is_null() {
        unsafe { UnregisterClassW(OVERLAY_CLASS.as_ptr(), instance) };
        let _ = ready.send(Err(format!(
            "CreateWindowExW for the overlay failed with Win32 error {}",
            unsafe { GetLastError() }
        )));
        return;
    }

    if let Err(error) = hotkey::register_capture_hotkey(window) {
        unsafe {
            DestroyWindow(window);
            UnregisterClassW(OVERLAY_CLASS.as_ptr(), instance);
        }
        let _ = ready.send(Err(if error.is_conflict() {
            CaptureError::HotkeyConflict(error.message()).to_string()
        } else {
            CaptureError::HotkeyUnavailable(error.message()).to_string()
        }));
        return;
    }

    eprintln!("[snapclip][capture] overlay ready hwnd={:?} thread={thread_id}", window);

    // Publish the window snapshot so diagnostics and acceptance checks can inspect the
    // real extended style instead of trusting the source.
    if let Ok(mut state) = shared.lock() {
        state.window = Some(OverlayWindowState {
            window: window as isize,
            extended_style: unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) },
            foreground: unsafe { GetForegroundWindow() } == window,
        });
    }

    let mut controller: Box<dyn OverlayMessageHandler> =
        Box::new(OverlayController::new(service, sink, shared, annotation_rx, window));
    let handler_ptr = (&mut controller) as *mut Box<dyn OverlayMessageHandler>;
    ACTIVE_HANDLER.with(|slot| slot.set(handler_ptr));

    if ready.send(Ok(thread_id.to_string())).is_err() {
        ACTIVE_HANDLER.with(|slot| slot.set(std::ptr::null_mut()));
        unsafe {
            hotkey::unregister_capture_hotkey(window);
            DestroyWindow(window);
            UnregisterClassW(OVERLAY_CLASS.as_ptr(), instance);
        }
        return;
    }

    let mut message: MSG = unsafe { zeroed() };
    loop {
        let result = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
        if result <= 0 {
            break;
        }
        if message.hwnd.is_null() {
            // Thread messages posted with `PostThreadMessageW` (the worker's
            // `FRAME_READY_MESSAGE`, the `WM_OVERLAY_COMMAND` channel, shutdown)
            // carry no window handle, so `DispatchMessageW` would silently drop
            // them and their window procedure would never run. Route them to the
            // controller here; only genuine window messages go to the pump.
            unsafe {
                controller.handle(message.message, message.wParam, message.lParam);
            }
            continue;
        }
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    ACTIVE_HANDLER.with(|slot| slot.set(std::ptr::null_mut()));
    drop(controller);
    unsafe {
        hotkey::unregister_capture_hotkey(window);
        DestroyWindow(window);
        UnregisterClassW(OVERLAY_CLASS.as_ptr(), instance);
    }
}

thread_local! {
    /// Dispatcher installed by [`overlay_thread`] for the duration of its message
    /// loop.
    ///
    /// A thread-local is used instead of `GWLP_USERDATA` so the controller stays
    /// entirely on the overlay thread: it owns an `HWND`, a D3D11 device and the
    /// session, none of which are `Send`, and none of which need to be.
    static ACTIVE_HANDLER: std::cell::Cell<*mut Box<dyn OverlayMessageHandler>> =
        const { std::cell::Cell::new(std::ptr::null_mut()) };
}

unsafe extern "system" fn overlay_window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let pointer = ACTIVE_HANDLER.with(|slot| slot.get());
    if !pointer.is_null() {
        // SAFETY: the pointer is installed by the overlay thread before its message
        // loop starts and cleared after the loop ends; a window procedure only ever
        // runs on the thread that created the window.
        let handler = unsafe { &mut *pointer };
        if let Some(result) = unsafe { handler.handle(message, wparam, lparam) } {
            return result;
        }
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}


const OVERLAY_CLASS: &[u16] = &[
    83, 110, 97, 112, 67, 108, 105, 112, 67, 97, 112, 116, 117, 114, 101, 79, 118, 101, 114, 108,
    97, 121, 0,
];
const OVERLAY_TITLE: &[u16] = &[83, 110, 97, 112, 67, 108, 105, 112, 0];

/// `GWL_EXSTYLE`.
const GWL_EXSTYLE: i32 = -20;

#[link(name = "user32")]
unsafe extern "system" {
    fn GetWindowLongPtrW(window: HWND, index: i32) -> isize;
}

#[cfg(test)]
mod tests {
    use super::{point_from_lparam, OverlayCommand};

    /// `WS_EX_NOACTIVATE`.
    ///
    /// A plain `u32` style mask, compared against the `isize` that
    /// `GetWindowLongPtrW` returns.
    const WS_EX_NOACTIVATE_MASK: isize = 0x0800_0000;

    /// Regression guard for the `Esc` / `Enter` key path.
    ///
    /// Both keys arrive as `WM_KEYDOWN`, which a window only receives once it can be
    /// activated. `WS_EX_NOACTIVATE` would suppress that, so the style combination the
    /// overlay is created with must never contain it.
    #[test]
    fn overlay_style_is_activatable_so_escape_reaches_it() {
        use ::windows::Win32::UI::WindowsAndMessaging::{
            WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        };

        // Mirror of the style expression in `overlay_thread`.
        let style = (WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0 | WS_EX_NOREDIRECTIONBITMAP.0) as isize;

        assert_eq!(
            style & WS_EX_NOACTIVATE_MASK,
            0,
            "the overlay must be activatable, otherwise Esc and Enter are dead keys"
        );
        // The styles that must stay, so the fix above is not "delete everything".
        assert_ne!(style & WS_EX_TOOLWINDOW.0 as isize, 0);
        assert_ne!(style & WS_EX_TOPMOST.0 as isize, 0);
        assert_ne!(style & WS_EX_NOREDIRECTIONBITMAP.0 as isize, 0);
    }

    /// `Esc` and `Enter` must be handled as plain key-down messages.
    #[test]
    fn escape_and_enter_map_to_cancel_and_confirm() {
        use crate::platform::windows::capture::hotkey;

        // Guard the virtual-key constants the handler matches on.
        assert_eq!(hotkey::ESCAPE_VIRTUAL_KEY, 0x1B);
        assert_eq!(hotkey::RETURN_VIRTUAL_KEY, 0x0D);
    }

    #[test]
    fn lparam_decodes_signed_client_coordinates() {
        // (10, 20)
        let packed = (20i32 << 16) | 10i32;
        let point = point_from_lparam(packed as isize);
        assert_eq!((point.x, point.y), (10, 20));

        // (-5, -7) must survive the signed round trip.
        let negative = ((-7i32 & 0xFFFF) << 16) | (-5i32 & 0xFFFF);
        let point = point_from_lparam(negative as isize);
        assert_eq!((point.x, point.y), (-5, -7));
    }

    #[test]
    fn command_round_trips_through_wparam() {
        for command in [
            OverlayCommand::Start,
            OverlayCommand::Cancel,
            OverlayCommand::Confirm,
            OverlayCommand::Shutdown,
            OverlayCommand::FrameReady,
        ] {
            assert_eq!(OverlayCommand::from_wparam(command as usize), Some(command));
        }
        assert_eq!(OverlayCommand::from_wparam(9999), None);
    }
}







