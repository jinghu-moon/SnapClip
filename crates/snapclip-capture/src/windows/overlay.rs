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
use std::time::{Duration, Instant};

use windows_sys::Win32::{
    Foundation::{GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM},
    Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT},
    System::LibraryLoader::GetModuleHandleW,
    System::Threading::GetCurrentThreadId,
    UI::{
        Controls::WM_MOUSELEAVE,
        Input::Ime::{ImmGetContext, ImmReleaseContext, ImmSetOpenStatus},
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, GetKeyState, MapVirtualKeyW, SetFocus, VK_CONTROL, VK_LBUTTON,
            VK_MBUTTON, VK_RBUTTON, MAPVK_VSC_TO_VK,
        },
        WindowsAndMessaging::{
            IDC_ARROW, IDC_CROSS, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE,
            LoadCursorW,
            SetCursor,
            CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
            DispatchMessageW, GetCursorPos, GetForegroundWindow, GetMessageW, MSG, PeekMessageW,
            PostQuitMessage,
            PostThreadMessageW, RegisterClassW, SW_HIDE, SW_SHOW, SetForegroundWindow,
            SetWindowPos, SetTimer, KillTimer, GetSystemMetrics, SM_CXDRAG,
            SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
            ShowWindow, TranslateMessage, UnregisterClassW, WM_APP, WM_DESTROY, WM_DEVICECHANGE,
            WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ERASEBKGND, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP,
            WM_MOUSEWHEEL, WM_SETFOCUS, WM_LBUTTONDOWN,
            WM_MOUSEACTIVATE,
            WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCHITTEST, WM_PAINT, WM_RBUTTONDOWN,
            WM_SETCURSOR, WM_TIMER, WNDCLASSW, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW,
            WS_EX_TOPMOST, WS_POPUP, HTCLIENT, HTTRANSPARENT, HWND_TOPMOST, MA_ACTIVATE,
        },
    },
};

use crate::artifact::{CaptureService, SelectionPixels};
use crate::annotation::{
    AnnotationCommand, AnnotationDocument, AnnotationGeometry, AnnotationHandle, AnnotationId,
    AnnotationItem, AnnotationKind, DocumentSnapshot,
};
use crate::ports::{ArtifactWriter, CaptureEventSink, ClipboardWriter, OverlayPlatform};
use crate::diagnostics::WindowDetectionMetrics;
use crate::geometry::{
    Handle, LevelReach, MagnifierConfig, MonitorLayout, Point, Rect, ResizeMode, SelectionGeometry,
    magnifier_geometry, window_rect_to_local,
};
use crate::sampler::{ColorFormat, ColorSampler};
use crate::session::{CaptureSession, ExportOutcome};
use crate::window_detection::model::RequestId;
use crate::window_detection::{
    DEFAULT_DWELL_MS, DEFAULT_HOVER_REVALIDATE_MS, DEFAULT_SNAP_RADIUS_PX,
    DeepTarget, Exclusions, LevelChain, LevelKind, PathLevel, RingOptions, RingRole, chain_rings,
    next_visible_stop, out_quad, stops_from,
    GestureState, HoverValidity, MoveOutcome, PressOutcome, RefinementJob, RefinementOutcome,
    RefinementScheduler,
    ReleaseOutcome, Replacement, WindowSnapshot, WindowTarget, classify_replacement,
    preview_bounds,
};
use crate::{CaptureError, CaptureResult, CaptureState};

use super::capture_worker::{self, CaptureWorker, StartRequest};
use super::detection_worker::{self, DetectionResult, DetectionWorker};
use super::export_worker::{self, ExportJob, ExportWorker};
use super::win::window;
use super::refinement_worker::{self, RefinementResult, RefinementWorker};
use super::hotkey;
use super::monitor::{self, CapturedMonitor};
use super::providers::{FrozenFrame, FrozenFramePixels};
use super::renderer::{OverlayFrameState, Win32Renderer};
use super::win::d2d::ChainRingView;

/// `WM_APP`-based command delivered from any thread to the overlay thread.
const WM_OVERLAY_COMMAND: u32 = WM_APP + 17;
/// Coalescing render cadence in milliseconds (~60 Hz, docs/11 §"约 16ms 渲染节奏").
/// Mouse and drag input only marks state dirty and arms this one-shot timer; the
/// timer collapses every change since the last tick into a single render/present/commit.
const RENDER_TICK_MS: u32 = 15;
/// The `SetTimer` id for the coalescing render tick.
const RENDER_TIMER_ID: usize = 0x51_C0DE;

/// One-shot timer that fires once the cursor has been still for
/// [`DEFAULT_DWELL_MS`]; the expiry handler only queries the cached snapshot.
const DWELL_TIMER_ID: usize = 0x51_C0DF;

/// Periodic timer that re-validates the hovered window on the detection worker
/// (docs/14 §5.5). It only *enqueues*; the DWM read happens off this thread.
const HOVER_TIMER_ID: usize = 0x51_C0E0;

/// One-shot timer that fires once the hovered target has been still for
/// [`crate::window_detection::REFINEMENT_DWELL_MS`] (docs/18 §2). Expiry only
/// *enqueues* a refinement query; the accessibility traversal runs on its own thread.
const REFINEMENT_TIMER_ID: usize = 0x51_C0E1;

/// How long the preview waits for the deep query that answers the **current** cursor position.
///
/// Long enough to cover the refinement dwell plus a normal query (measured 2–76 ms) and the
/// confirming dwell a staged downgrade needs (docs/18 §13.3), short enough that a stuck or
/// unsupported provider still degrades to the v1 window frame promptly. While it lasts the
/// preview keeps the last verified rectangle instead of falling back to the whole frame, which
/// is what removed the "expand to the window, then shrink to the box" flicker (§13.5).
const REFINEMENT_PREVIEW_WAIT_MS: u32 = 320;
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

pub(crate) use state::*;


/// Cross-thread state shared between [`WindowsOverlay`] and the overlay thread.
#[derive(Debug)]
struct OverlayShared {
    state: CaptureState,
    session_id: Option<String>,
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

impl WindowsOverlay {
    /// Spawn the overlay thread and wait until the hotkey and window exist.
    ///
    /// Fails — rather than silently degrading — when `F5` cannot be registered, so
    /// the user learns about the conflict.
    pub fn spawn_overlay(
        service: Arc<CaptureService>,
        sink: Arc<dyn CaptureEventSink>,
        clipboard: Arc<dyn ClipboardWriter>,
        writer: Arc<dyn ArtifactWriter>,
        options: crate::window_detection::DetectionOptions,
    ) -> Result<Self, String>
    {
        let shared = Arc::new(Mutex::new(OverlayShared {
            state: CaptureState::Idle,
            session_id: None,
        }));
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        // Bounded so a stuck/slow overlay cannot let toolbar clicks pile up without
        // limit; the capacity only needs to absorb a burst of discrete clicks.
        let (annotation_tx, annotation_rx) = mpsc::sync_channel(64);
        let thread_shared = shared.clone();
        let thread = thread::Builder::new()
            .name("snapclip-capture-overlay".into())
            .spawn(move || {
                overlay_thread(
                    service,
                    sink,
                    clipboard,
                    writer,
                    options,
                    thread_shared,
                    annotation_rx,
                    ready_tx,
                )
            })
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
struct OverlayController {
    service: Arc<CaptureService>,
    /// Encoding and writing, owned by the composition root (docs/23 T2.4).
    writer: Arc<dyn ArtifactWriter>,
    sink: Arc<dyn CaptureEventSink>,
    /// Clipboard writes the overlay itself performs (the `C` colour copy).
    clipboard: Arc<dyn ClipboardWriter>,
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
    /// Explicit pointer gesture (docs/14 §4.2). Button-down no longer mutates the
    /// selection; the gesture decides when a drag actually starts.
    gesture: GestureState,
    /// Window the cursor is over, in virtual-desktop screen coordinates.
    hover_target: Option<WindowTarget>,
    /// Latest window snapshot. Replaced wholesale when a refresh lands, never mutated
    /// from the mouse path (except through `apply_candidate_update`).
    snapshot: WindowSnapshot,
    /// Refresh request currently outstanding; results tagged otherwise are dropped.
    snapshot_request: Option<RequestId>,
    /// Confirmation request currently outstanding.
    confirm_request: Option<RequestId>,
    /// Single-flight hover re-validation request (docs/14 §5.5).
    hover_request: Option<RequestId>,
    /// Preview rectangle currently painted (eased, monitor-local pixels).
    preview_rect: Option<Rect>,
    /// Preview rectangle the animation is heading for; `None` means no preview.
    preview_target: Option<Rect>,
    preview_transition: crate::window_detection::RectTransition,
    /// HWND/process exclusions the snapshot is built with (docs/14 §7).
    exclusions: Exclusions,
    detector: DetectionWorker,
    /// v2 refinement: schedules and owns accessibility deep-selection queries
    /// (docs/18 §2, §5). v1 never reads from it unless a path was published.
    refine: RefinementScheduler,
    refinement: RefinementWorker,
    /// The published deep path, if the refinement worker produced one this session.
    deep_target: Option<DeepTarget>,
    /// Which level of `deep_target`'s chain the user walked to (docs/21 §5.17). `None` means the
    /// published box itself, which is what every session starts with.
    deep_levels: Option<LevelChain>,
    /// The one-shot hint currently armed, if any (docs/21 §5.21).
    ///
    /// Two different sentences use this slot: the affordance ("the wheel walks the levels"),
    /// armed when the session starts, and the explanation of the counter, armed the first time
    /// the user actually walks a level. Either way it is dropped on a press.
    hint: Option<ArmedHint>,
    /// Whether this session has already explained the level counter (docs/21 §5.21).
    ///
    /// Once per session: the sentence belongs to the moment the number appears, and a user who
    /// keeps walking does not need it again — the label carries the numbers from then on.
    hint_taught: bool,
    /// How visible the chain of rings is (docs/21 §5.22): eased in when it is used, quantised away
    /// when the walk goes quiet.
    chain: ChainVisibility,
    /// Which rings are on screen and since when, so a level a walk just added fades in while the
    /// rings that survived it do not re-animate (docs/21 §5.24, ②).
    ring_appear: RingAppear,
    /// When the current preview box appeared, so it fades in — and only when it *appears*: a walk
    /// re-targets the box on every notch, and re-fading there would make it blink (docs/21 §5.24, ②).
    preview_appeared_at: Option<Instant>,
    /// The capture box's walk colour (docs/21 §5.24, A3): the green that says "you just moved this".
    walk: WalkColour,
    /// Wheel deltas accumulating into level steps (v3 B2, docs/21 §5.24).
    wheel: WheelAccumulator,
    /// A shallower target waiting for its confirming dwell (docs/18 §13.3).
    ///
    /// A cursor merely passing through a parent container produces one such result; displaying
    /// it immediately is what made the frame grow and shrink again on the way from A to B.
    pending_downgrade: Option<(Rect, Point)>,
    /// When the deep answer for the **current** cursor position was last expected.
    ///
    /// Armed every time the refinement dwell is armed and cleared when that answer lands, so it
    /// tracks the question rather than the hover. While it is fresh the preview keeps the last
    /// verified rectangle of this window instead of falling back to the whole window frame: the
    /// preview dwell (120 ms) can fire before the refinement dwell (80 ms) *plus* its query has
    /// finished, and the frame appearing as the whole window first and shrinking to the real box
    /// a moment later is what the product rejected (docs/18 §13.4/§13.5). Bounded by
    /// [`REFINEMENT_PREVIEW_WAIT_MS`], so a provider that never answers still degrades to v1.
    refinement_pending: Option<Instant>,
    /// Read by this window's `WM_NCHITTEST` while the refinement worker has an accessibility point
    /// hit test in flight (docs/21 §5.7). Without it the hit test answers the overlay — it covers
    /// the desktop — instead of the application underneath, which is what the precision top-up
    /// needs; the flag only ever lives for one accessibility call.
    hit_test_pass_through: window::HitTestPassThrough,
    metrics: WindowDetectionMetrics,
    /// Dwell generation the pending timer was armed for.
    dwell_armed: Option<u64>,
    /// Snap radius in physical pixels for the current session's DPI.
    snap_radius: u32,
    cursor: Point,
    cursor_visible: bool,
    dirty: bool,
    /// Pure color-sampling state machine (tile hit, throttle, hex formatting).
    sampler: ColorSampler,
    /// `S`-cycled colour format shown in the magnifier info panel's primary slot
    /// (Hex → Rgb → Hsl → Hex).
    magnifier_color_format: ColorFormat,
    /// When true the info-panel coordinate is relative to the selection origin (P toggle).
    magnifier_relative: bool,
    /// Current loupe zoom (`Z` + wheel). `0.1..=40.0`.
    magnifier_zoom: f32,
    /// Whether the `Z` key is currently held (gates wheel-to-zoom).
    z_held: bool,
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

impl OverlayController {
    fn new(
        service: Arc<CaptureService>,
        sink: Arc<dyn CaptureEventSink>,
        clipboard: Arc<dyn ClipboardWriter>,
        writer: Arc<dyn ArtifactWriter>,
        options: crate::window_detection::DetectionOptions,
        shared: Arc<Mutex<OverlayShared>>,
        annotation_rx: mpsc::Receiver<AnnotationCommand>,
        window: HWND,
        thread_id: u32,
    ) -> Self {
        let metrics = WindowDetectionMetrics::new();
        let detector = DetectionWorker::new(thread_id, metrics.clone());
        // The refinement worker gets the pass-through flag this window's `WM_NCHITTEST` reads
        // (docs/21 §5.7): the overlay covers the desktop, so without it the accessibility point hit
        // test answers the overlay itself and the precision top-up can never fire.
        let hit_test_pass_through = window::HitTestPassThrough::default();
        let refinement = RefinementWorker::new(
            thread_id,
            hit_test_pass_through.clone(),
            // The caller's detection preference (docs/23 T4.4.1); `Default` is exactly what
            // this argument used to be.
            options.adopt_text_runs,
            metrics.clone(),
        );
        // The overlay must never be offered as its own snap target: it is full-screen and
        // frontmost, so a snapshot that included it would return the overlay for every
        // point (docs/14 §7, layer 2). The process exclusion is the fallback for windows
        // SnapClip creates later (toolbar, colour panel, main window).
        let mut exclusions = Exclusions::new();
        exclusions.exclude_hwnd(window as isize);
        exclusions.exclude_process(std::process::id());
        Self {
            service,
            writer,
            sink,
            clipboard,
            shared,
            annotation_rx,
            window,
            worker: CaptureWorker::new(),
            export_worker: ExportWorker::new(),
            renderer: None,
            session: CaptureSession::new("idle"),
            frozen: None,
            gesture: GestureState::new(system_drag_threshold(96)),
            hover_target: None,
            snapshot: WindowSnapshot::empty(),
            snapshot_request: None,
            confirm_request: None,
            hover_request: None,
            preview_rect: None,
            preview_target: None,
            preview_transition: crate::window_detection::RectTransition::settled(
                Rect::default(),
                Instant::now(),
            ),
            exclusions,
            detector,
            refine: RefinementScheduler::new(),
            refinement,
            deep_target: None,
            deep_levels: None,
            hint: None,
            hint_taught: false,
            chain: ChainVisibility::default(),
            ring_appear: RingAppear::default(),
            preview_appeared_at: None,
            walk: WalkColour::default(),
            wheel: WheelAccumulator::default(),
            pending_downgrade: None,
            refinement_pending: None,
            hit_test_pass_through,
            metrics,
            dwell_armed: None,
            snap_radius: DEFAULT_SNAP_RADIUS_PX,
            cursor: Point::default(),
            cursor_visible: false,
            dirty: false,
            sampler: ColorSampler::new(),
            magnifier_color_format: ColorFormat::default(),
            magnifier_relative: false,
            magnifier_zoom: MagnifierConfig::ZOOM_DEFAULT,
            z_held: false,
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


    // ---- input -----------------------------------------------------------

    /// Start a fresh window-detection cycle for a new session (docs/14 §5.3).
    ///
    /// Everything that could leak from the previous session — hover, preview, snapshot,
    /// outstanding requests — is dropped here, and the refresh is handed to the detection
    /// worker so the overlay thread never runs `EnumWindows`/DWM itself.
    fn begin_window_detection(&mut self, layout: &MonitorLayout) {
        self.gesture = GestureState::new(system_drag_threshold(layout.dpi));
        self.set_preview_target(None);
        self.snap_radius = snap_radius_px();
        self.hover_target = None;
        self.dwell_armed = None;
        self.confirm_request = None;
        self.hover_request = None;
        // v2 state is per-session too: no cached deep path, no in-flight query.
        self.refine.reset();
        self.deep_target = None;
        self.deep_levels = None;
        self.pending_downgrade = None;
        self.refinement_pending = None;
        self.disarm_refinement();
        self.refinement.retire();
        self.snapshot.release();
        self.request_snapshot_refresh();
        // Arm the *affordance* hint for this session (docs/21 §5.21). The explanation of the
        // counter is a different sentence, and it is armed later, by the first level walk.
        self.hint_taught = false;
        self.arm_hint(LEVEL_HINT.to_owned(), LEVEL_HINT_MS);
        // ③b starts from "just touched": a session opens with the chain fully visible.
        self.touch_chain();
    }

    /// Ask the detection worker for a new snapshot. Only the newest request is kept.
    fn request_snapshot_refresh(&mut self) {
        self.snapshot_request = Some(self.detector.request_refresh(&self.exclusions));
    }

    /// Monitor-local cursor position expressed in virtual-desktop coordinates.
    fn cursor_screen(&self) -> Option<Point> {
        self.layout().map(|layout| layout.to_screen(self.cursor))
    }

    /// The hovered window in monitor-local coordinates, ready for the paint layer.
    ///
    /// Returns `None` once the session is settled, which is what turns window hover off
    /// after a confirmation (docs/14 §4.1) without a second state flag: `update_hover`
    /// already refuses to resolve a target outside `Selecting`.
    fn hover_bounds_local(&self) -> Option<Rect> {
        let layout = self.layout()?;
        let target = self.hover_target?;
        let rect = window_rect_to_local(target.screen_bounds(), &layout);
        (!rect.is_empty()).then_some(rect)
    }

    /// The ancestor level the user walked to, if any (docs/21 §5.17).
    ///
    /// `None` means "the published box", which is what the refinement produced and what the preview
    /// shows until the wheel or an arrow key asks for another level. The level carries its own kind,
    /// which is what the size label names it by (docs/21 §5.24, B6).
    fn selected_deep_level(&self) -> Option<PathLevel> {
        let deep = self.deep_target.as_ref()?;
        let chain = self.deep_levels?;
        (!chain.is_deepest())
            .then(|| chain.current(&deep.path))
            .flatten()
    }

    /// The label the automatic-snap preview carries (docs/21 §5.21).
    ///
    /// `None` when there is no preview to describe. The text is built here rather than in the
    /// renderer because this is the side that knows the three things the label needs beyond the
    /// size: the level the user walked to, whether the box is the whole window, and whether anything
    /// answered for this position at all.
    ///
    /// It describes the *target* rather than the rectangle currently being eased into place: a size
    /// that counted through intermediate values for the 101 ms of a transition would be noise, and
    /// the numbers are what the user is making a decision with (docs/21 §5.21).
    fn preview_label_text(&self) -> Option<String> {
        let rect = self.preview_target?;
        if rect.is_empty() || self.session.state() != CaptureState::Selecting {
            return None;
        }
        Some(preview_label(
            rect,
            self.preview_is_window(),
            self.preview_kind(),
            self.walked_up(),
            matches!(
                self.metrics.last_precision_outcome(),
                Some(crate::diagnostics::PrecisionOutcome::Unavailable)
            ),
        ))
    }

    /// What kind of thing the previewed box is, when a transport said (docs/21 §5.24, B6).
    ///
    /// The walked-to level if there is one, otherwise the published box — the same pair the preview
    /// rectangle comes from, so the noun and the box cannot describe different levels.
    fn preview_kind(&self) -> Option<LevelKind> {
        let deep = self.deep_target.as_ref()?;
        let level = self
            .selected_deep_level()
            .or_else(|| deep.path.last().copied())?;
        Some(level.kind)
    }

    /// Whether the user has walked off the deepest level — which is what makes the label say `容器`
    /// and what makes the level badge appear at all (docs/21 §5.22).
    fn walked_up(&self) -> bool {
        self.deep_levels.is_some_and(|chain| !chain.is_deepest())
    }

    /// What the level badge shows: how many stops the wheel still has each way (docs/21 §5.24, A1).
    fn level_badge(&self) -> Option<LevelReach> {
        let path = self
            .deep_target
            .as_ref()
            .map(|deep| deep.path.as_slice())
            .unwrap_or(&[]);
        level_badge_reach(self.deep_levels, path)
    }

    /// Whether the previewed box is the whole window rather than an element (docs/21 §5.21).
    ///
    /// A whole-window answer is the v1 fallback, so the paint layer draws it with the neutral wash
    /// and a thin outline instead of the accent preview: "we could not get below the window" should
    /// not look like a confident element pick.
    fn preview_is_window(&self) -> bool {
        if self.deep_target.as_ref().is_some_and(|deep| {
            deep.kind == crate::window_detection::model::TargetKind::TopLevelWindowFrame
        }) {
            return true;
        }
        // The wheel can walk the preview all the way up to the frame itself (docs/21 §5.17), and
        // `path[0]` is always that frame: the box *is* the window then, even though the answer
        // underneath it was an element.
        self.deep_levels
            .is_some_and(|chain| chain.len() > 1 && chain.index() == 0)
    }

    /// The one-shot hint, while it is still worth drawing (docs/21 §5.21).
    fn hint_text(&self) -> Option<(Point, String)> {
        let hint = self.hint.as_ref()?;
        if Instant::now() >= hint.until || self.session.state() != CaptureState::Selecting {
            return None;
        }
        Some((self.cursor, hint.text.clone()))
    }

    /// Arm a one-shot hint, replacing whatever was showing (docs/21 §5.21).
    fn arm_hint(&mut self, text: String, millis: u64) {
        self.hint = Some(ArmedHint {
            until: Instant::now() + Duration::from_millis(millis),
            text,
        });
    }

    /// Explain the level counter the first time it appears (docs/21 §5.21).
    ///
    /// The number is meaningless on its own — `8/9` could be a zoom, a page, a colour channel —
    /// and the one fact nobody can guess is which end is the window. So the sentence arrives with
    /// the counter, at the moment the user's own wheel made it appear, instead of three seconds
    /// earlier when it would have been about something that had not happened yet.
    ///
    /// While that one sentence is still on screen, another walk rewrites its numbers
    /// ([`should_teach`]): a sentence frozen at `8/9` next to a label already reading `3/7` is worse
    /// than not explaining at all. After it has gone it does not come back — the lesson is once per
    /// session, and the label carries the numbers from then on.
    fn arm_level_hint(&mut self) {
        if self.deep_levels.is_none_or(|chain| chain.is_empty()) {
            return;
        }
        if !should_teach(self.hint_taught, self.hint_showing()) {
            return;
        }
        self.hint_taught = true;
        // The sentence explains what is on screen, so it quotes the **badge's** numbers (v3 A1):
        // stays in each direction, not the level ordinal the log prints. Walking back down to the
        // answer leaves nothing to count, and the sentence falls back to the plain affordance.
        let text = match self.level_badge() {
            Some(reach) => level_hint(reach),
            None => LEVEL_HINT.to_owned(),
        };
        self.arm_hint(text, LEVEL_HINT_MS);
    }

    /// Whether the one-shot hint is on screen right now (docs/21 §5.21).
    ///
    /// An expired hint stays armed until something replaces it, so the deadline — not the presence
    /// of the value — is what decides.
    fn hint_showing(&self) -> bool {
        self.hint
            .as_ref()
            .is_some_and(|hint| Instant::now() < hint.until)
    }

    /// The chain was just used: a walk or a new answer (docs/21 §5.22, §5.24 ④).
    ///
    /// The rise starts from whatever is on screen, so a chain re-used mid-fade comes back smoothly
    /// rather than jumping — and a *cursor move* deliberately no longer counts. It used to, so that
    /// "the hand is on its way to inspect it" would not lose the rings; the effect was that a user
    /// who kept moving the mouse never saw them fade at all, which is the disappearance the user
    /// asked for (docs/21 §5.24).
    fn touch_chain(&mut self) {
        self.chain.touch(Instant::now());
    }

    /// Advance the ③b fade and the ② rise; returns whether anything changed and therefore needs a
    /// repaint.
    ///
    /// Called from the coalescing render tick, which also keeps itself alive while the fade is still
    /// moving (`chain_fade_running`), so the fade is driven by the same 15 ms clock as everything
    /// else rather than by a timer of its own.
    fn advance_chain_fade(&mut self) -> bool {
        if !self.chain.advance(Instant::now()) {
            return false;
        }
        // One repaint per step, and that is the whole cost model — so it gets its own counter
        // instead of hiding inside `present=` (docs/21 §5.22).
        self.metrics.record_chain_fade_frame();
        true
    }

    /// Whether there is still a fade to come or to finish (docs/21 §5.22).
    ///
    /// True from the moment the chain is touched until it has faded out, so the hover tick can keep
    /// *one* repaint alive to notice the 1.2 s deadline. It is deliberately not what keeps the
    /// render tick armed — see `chain_fade_running`.
    fn chain_fade_pending(&self) -> bool {
        self.chain.pending()
    }

    /// Whether the fade is *moving* right now: only then does the render tick re-arm itself, so the
    /// 1.2 s of waiting costs nothing (docs/21 §5.22).
    fn chain_fade_running(&self) -> bool {
        self.chain.running(Instant::now())
    }

    /// A walk just happened: the capture box turns green (docs/21 §5.24, A3).
    ///
    /// Called for every walk attempt, including one that ends up pinned against the end of the
    /// chain: the colour says "the wheel was used", and a notch that does nothing is exactly when
    /// that needs saying.
    fn touch_walk(&mut self) {
        self.walk.touch(Instant::now());
        self.invalidate();
    }

    /// Step the walk envelope; returns whether the painted value changed.
    ///
    /// Driven by the same coalescing render tick as the chain fade, for the same reason: the value
    /// only moves at ~60 Hz at most, so a timer of its own would only add ways to be late.
    fn advance_walk_activity(&mut self) -> bool {
        if !self.walk.advance(Instant::now()) {
            return false;
        }
        // Counted like the chain fade: one bounded ramp, and this is the number that proves the
        // bound on a real machine (docs/21 §5.24).
        self.metrics.record_walk_frame();
        true
    }

    /// Whether the walk envelope has anything left to do (docs/21 §5.24).
    ///
    /// The two-part rule the chain fade uses (docs/21 §5.22): `running` is what keeps the render
    /// tick alive while the value moves, and `pending` keeps *one* repaint alive so the 1.2 s hold
    /// can be noticed at all — a resting cursor produces no other repaint.
    fn walk_activity_running(&self) -> bool {
        self.walk.running(Instant::now())
    }

    fn walk_activity_pending(&self) -> bool {
        self.walk.pending()
    }

    /// How green to paint the capture box.
    ///
    /// The whole-window fallback gets none: it is the neutral answer, it has no capture colour to
    /// lift, and the prototype keeps it grey. Everything else follows the walk (docs/21 §5.24, A3).
    fn capture_box_green(&self) -> f32 {
        if self.preview_is_window() {
            0.0
        } else {
            self.walk.activity()
        }
    }

    /// Step the deep-selection level: `-1` toward the window frame, `+1` toward the published box.
    ///
    /// Returns whether anything moved, so the wheel can decide whether to consume the event.
    fn step_deep_level(&mut self, delta: i32) -> bool {
        if self.session.state() != CaptureState::Selecting {
            return false;
        }
        // No answer, no box: nothing would show the colour, and arming the walk envelope for it
        // would keep the render tick alive for a second of empty repaints.
        if self.deep_target.is_none() {
            return false;
        }
        // The walk is what the capture box's colour reports, so it is touched before the step is
        // resolved: a notch against either end of the chain still says "the wheel was used"
        // (docs/21 §5.24, A3).
        self.touch_walk();
        let Some(deep) = self.deep_target.as_ref() else {
            return false;
        };
        // A level walk starts from what is on screen right now, so the chain is built on first use
        // for whatever answer is currently published.
        let chain = self
            .deep_levels
            .get_or_insert_with(|| LevelChain::new(deep.path.len()));
        // Skip the levels that would look identical (v3 B1, docs/21 §5.24): with collapse and the cap
        // of seven, some levels have no ring, and a notch that changes nothing on screen reads as a
        // dead wheel.
        let target = next_visible_stop(
            &deep.path,
            chain.index(),
            delta,
            RingOptions::default().collapse_gap_px,
        );
        let moved = chain.jump_to(target);
        if moved {
            // The first successful walk is where the counter appears, so that is where it gets
            // explained (docs/21 §5.21).
            self.arm_level_hint();
            // A walk is the clearest "I am looking at the chain" there is (docs/21 §5.22).
            self.touch_chain();
            self.refresh_preview_for_cursor();
            self.invalidate();
        }
        moved
    }

    /// The level chain as rings to paint, in monitor-local coordinates (docs/21 §5.22).
    ///
    /// Which levels are drawn is `chain_rings`'s decision — anchors, collapse, merge, the cap of
    /// seven — and this function only converts the result into the paint layer's coordinates and
    /// drops the selected level, which the preview paints with the capture colour.
    ///
    /// Empty unless the published deep target belongs to the window currently under the cursor: a
    /// path for another window would draw outlines over unrelated pixels.
    fn chain_rings_local(&mut self) -> Vec<ChainRingView> {
        let Some(layout) = self.layout() else {
            return Vec::new();
        };
        let Some(deep) = self.deep_target.as_ref() else {
            return Vec::new();
        };
        if self.hover_target.map(|target| target.identity()) != Some(deep.window) {
            return Vec::new();
        }
        let selected = self
            .deep_levels
            .map(|chain| chain.index())
            .unwrap_or(deep.path.len().saturating_sub(1));
        // In monitor-local pixels, with everything the paint layer needs, so the borrow of
        // `self.deep_target` ends before the appear clocks below need `&mut self`.
        let planned: Vec<(Rect, bool, f32)> =
            chain_rings(&deep.path, selected, RingOptions::default())
                .rings
                .iter()
                .filter(|ring| ring.role != RingRole::Selected)
                .map(|ring| {
                    (
                        window_rect_to_local(deep.path[ring.index].rect, &layout),
                        ring.role == RingRole::Inner,
                        ring.alpha,
                    )
                })
                .collect();
        let now = Instant::now();
        // ③b: the whole chain fades as one after the walk goes quiet (docs/21 §5.22). The badge and
        // the capture box do not — they are the answer, not the context.
        let visibility = self.chain.value();
        let views: Vec<ChainRingView> = planned
            .into_iter()
            .filter(|(rect, _, _)| !rect.is_empty())
            .map(|(rect, inner, alpha)| {
                // ②: a ring that is already on screen keeps its clock; one a walk just added starts
                // at zero and eases in, so stepping up a level shows *which* ring arrived.
                let appear = self.ring_appear.share(rect, now);
                ChainRingView {
                    rect,
                    inner,
                    alpha: alpha * visibility * appear,
                }
            })
            .collect();
        // Forget the rings that are gone, so the next time one of them appears it fades in again.
        let live: Vec<Rect> = views.iter().map(|view| view.rect).collect();
        self.ring_appear.retain(&live);
        views
    }

    /// Re-resolve the hovered window from the cached snapshot. **Pure cache read.**
    fn update_hover(&mut self) {
        if self.session.state() != CaptureState::Selecting {
            self.clear_hover();
            return;
        }
        let Some(screen) = self.cursor_screen() else {
            self.clear_hover();
            return;
        };
        let started = Instant::now();
        let target = self.snapshot.hit_test(screen);
        self.metrics.record_hit_test(started.elapsed());

        // v2 refinement tracks the **cursor**, not the window. Inside one window the user
        // changes the control under the pointer without changing the window, so feeding the
        // scheduler only on window change made deep selection appear "hard to trigger": a
        // query ran once per window and never followed the controls. The scheduler itself
        // decides whether the new point needs the worker at all (moving inside the already
        // published path still answers from cache), so this is cheap.
        self.drive_refinement(screen, target);

        let unchanged = match (&target, &self.hover_target) {
            (None, None) => true,
            (Some(new), Some(old)) => {
                new.identity() == old.identity() && new.screen_bounds() == old.screen_bounds()
            }
            _ => false,
        };
        if unchanged {
            return;
        }
        if target.is_some() {
            self.metrics.record_hover_target_switch();
        }
        if let Some(target) = &target {
            self.metrics.log_line(
                &format!(
                    "hover hwnd={} z={} bounds=({},{})->({},{})",
                    target.identity().hwnd,
                    target.candidate.z_order,
                    target.screen_bounds().left,
                    target.screen_bounds().top,
                    target.screen_bounds().right,
                    target.screen_bounds().bottom
                ),
                false,
            );
        }
        self.hover_target = target;
        self.invalidate();
    }

    /// Feed one cursor position to the v2 refinement scheduler.
    ///
    /// Called for **every** cursor update, not just when the hovered window changes: the
    /// refinement target is the control under the pointer, and controls change far more
    /// often than windows do (docs/18 §2).
    fn drive_refinement(&mut self, screen: Point, target: Option<WindowTarget>) {
        // A staged downgrade only survives while the cursor stays where it was staged.
        if let Some((_, staged)) = self.pending_downgrade
            && !points_close(staged, screen)
        {
            self.pending_downgrade = None;
        }
        let epoch = self.snapshot.epoch();
        let actions = self
            .refine
            .on_cursor_moved(epoch, target.map(|target| target.identity()), screen);
        if actions.invalidate_in_flight {
            // The cursor moved away from the question that query was asked: the answer coming
            // back is stale and will be dropped, so this is a retirement like any other.
            self.refinement.retire();
        }
        if actions.arm_dwell {
            // A deep answer for *this* position is on its way: arming the dwell is what marks
            // the wait (and refreshes its deadline), so the preview holds its last verified
            // rectangle until the answer lands instead of showing the whole window first.
            self.arm_refinement();
        } else {
            self.disarm_refinement();
        }
        // Mirror the scheduler's published path: it is the single source of truth for
        // "which deep target is live", and the paint layer reads it from here.
        let published = self.refine.cached();
        let changed = match (published, self.deep_target.as_ref()) {
            (None, None) => false,
            (Some(new), Some(old)) => {
                new.window != old.window || new.screen_bounds != old.screen_bounds
            }
            _ => true,
        };
        if changed {
            self.deep_target = published.cloned();
            // A new answer is a new chain to look at, so it counts as activity for ③b (docs/21
            // §5.22) — otherwise the chain could be born already faded.
            self.touch_chain();
            // A new answer for the pointer resets the level walk to the published box (docs/21
            // §5.17): a chain belongs to the answer it was walked on, and carrying an index over to a
            // different element is how a walk ends up publishing a box nobody asked for.
            self.deep_levels = self
                .deep_target
                .as_ref()
                .map(|deep| LevelChain::new(deep.path.len()));
        }
        // …and the walk only lasts while the cursor stays on what it selected: moving off the chosen
        // level hands the choice back to the pointer.
        if let (Some(chain), Some(deep)) = (self.deep_levels, self.deep_target.as_ref())
            && chain
                .current(&deep.path)
                .is_some_and(|level| !level.rect.contains(screen))
        {
            self.deep_levels = None;
        }
    }

    fn clear_hover(&mut self) {
        if self.hover_target.take().is_some() {
            self.invalidate();
        }
    }

    /// Make the painted preview rectangle follow the gesture's preview target.
    ///
    /// Three rules, taken from the reference transition (docs/18 §10.2):
    /// * the **first** preview of a session is presented directly — nothing animates out of
    ///   an empty frame;
    /// * a new target eases from the rectangle currently on screen, so re-targeting
    ///   mid-flight continues smoothly instead of snapping;
    /// * the preview **disappears** directly; it never shrinks towards nothing.
    fn sync_preview_rect(&mut self) {
        self.set_preview_target(self.gesture.snap_preview().map(|preview| preview.selection));
    }

    fn set_preview_target(&mut self, target: Option<Rect>) {
        if self.preview_target == target {
            // Same target: no state change and no repaint, exactly like the reference.
            return;
        }
        self.preview_target = target;
        let now = Instant::now();
        match (target, self.preview_rect) {
            (Some(to), None) => {
                self.preview_transition.present(to, now);
                self.preview_rect = Some(to);
                // ②: the box *appeared* — ease it in. A re-target only moves it (the transition),
                // because re-fading on every notch would make it blink.
                self.preview_appeared_at = Some(now);
            }
            (Some(to), Some(from)) => self.preview_transition.start(from, to, now),
            (None, _) => {
                self.preview_transition.present(Rect::default(), now);
                self.preview_rect = None;
                self.preview_appeared_at = None;
            }
        }
    }

    /// How visible the preview box is: eased in when it appears, 1 later (docs/21 §5.24, ②).
    ///
    /// The *hole* in the mask is not part of this: it is punched by the mask, which is painted in one
    /// pass, so the content is already at its own brightness when the outline is still arriving.
    fn preview_appear(&self) -> f32 {
        self.preview_appeared_at
            .map(|since| appear_share(since.elapsed(), PREVIEW_APPEAR_MS))
            .unwrap_or(1.0)
    }

    /// Advance the preview animation and report whether the painted rectangle changed.
    fn advance_preview_animation(&mut self) -> bool {
        let now = Instant::now();
        if self.preview_transition.is_running(now) {
            let value = self.preview_transition.value_at(now);
            if self.preview_rect != Some(value) {
                self.preview_rect = Some(value);
                self.metrics.log_line(
                    &format!(
                        "preview anim=({},{})->({},{}) target=({},{})->({},{})",
                        value.left,
                        value.top,
                        value.right,
                        value.bottom,
                        self.preview_target.map(|target| target.left).unwrap_or_default(),
                        self.preview_target.map(|target| target.top).unwrap_or_default(),
                        self.preview_target.map(|target| target.right).unwrap_or_default(),
                        self.preview_target.map(|target| target.bottom).unwrap_or_default(),
                    ),
                    false,
                );
                return true;
            }
            return false;
        }
        // Settled: make the final frame exactly the target so rounding never leaves the
        // highlight a pixel away from the control it stands for.
        if let Some(target) = self.preview_target
            && self.preview_rect != Some(target)
        {
            self.preview_rect = Some(target);
            return true;
        }
        false
    }

    /// Arm the one-shot dwell timer for the current gesture generation.
    fn arm_dwell(&mut self) {
        let generation = self.gesture.dwell_generation();
        if self.dwell_armed == Some(generation) {
            return;
        }
        self.dwell_armed = Some(generation);
        unsafe { SetTimer(self.window, DWELL_TIMER_ID, DEFAULT_DWELL_MS, None) };
    }

    fn disarm_dwell(&mut self) {
        if self.dwell_armed.take().is_some() {
            unsafe { KillTimer(self.window, DWELL_TIMER_ID) };
        }
    }

    /// The dwell timer expired: decide whether an automatic-snap preview is shown.
    ///
    /// Everything here is a cached lookup plus one rectangle conversion — no Win32 and no
    /// DWM call runs on this path (docs/14 §10.1).
    fn on_dwell(&mut self) {
        let Some(armed) = self.dwell_armed.take() else {
            return;
        };
        // `SetTimer` repeats; killing it here makes the dwell a one-shot, so a resting
        // cursor does not keep waking the message loop every 120 ms.
        unsafe { KillTimer(self.window, DWELL_TIMER_ID) };
        if armed != self.gesture.dwell_generation() {
            // A move slipped in between the timer firing and this handler: the position
            // the timer was armed for is gone, so nothing is previewed.
            return;
        }
        let preview = self.preview_for_cursor();
        if self.gesture.apply_dwell(armed, preview) {
            match self.gesture.snap_preview() {
                Some(preview) => self.metrics.log_line(
                    &format!(
                        "auto-snap preview hwnd={} epoch={} local=({},{})->({},{})",
                        preview.target.identity().hwnd,
                        preview.target.candidate.snapshot_epoch,
                        preview.selection.left,
                        preview.selection.top,
                        preview.selection.right,
                        preview.selection.bottom
                    ),
                    false,
                ),
                None => self
                    .metrics
                    .log_line("auto-snap preview cleared", false),
            }
            self.sync_preview_rect();
            self.invalidate();
        }
    }

    /// Nearest window for the current cursor position, converted to monitor-local space.
    fn preview_for_cursor(&mut self) -> Option<(WindowTarget, Rect, Rect)> {
        if self.session.state() != CaptureState::Selecting {
            return None;
        }
        let layout = self.layout()?;
        let screen = layout.to_screen(self.cursor);
        let started = Instant::now();
        let target = self.snapshot.nearest_target(screen, self.snap_radius);
        self.metrics.record_nearest_target(started.elapsed());
        let target = target?;
        // Which rectangle the preview may show is a pure decision (docs/18 §13.5): an answer for
        // this position that is still on its way keeps the last verified rectangle — or withholds
        // the preview entirely when this window has none yet — and only a finished wait falls
        // back to the v1 whole-window frame.
        let waiting = self.refinement_pending.is_some_and(|at| {
            at.elapsed() < Duration::from_millis(u64::from(REFINEMENT_PREVIEW_WAIT_MS))
        });
        let bounds = preview_bounds(
            self.deep_target.as_ref(),
            self.selected_deep_level().map(|level| level.rect),
            target.identity(),
            screen,
            target.screen_bounds(),
            waiting,
        )?;
        let local = window_rect_to_local(bounds, &layout);
        if local.is_empty() {
            return None;
        }
        Some((target, local, self.session.selection()))
    }

    /// Drain one detection-worker result.
    fn on_detection_ready(&mut self) {
        let Some(result) = self.detector.take_result() else {
            return;
        };
        match result {
            DetectionResult::Refreshed { request, snapshot } => {
                if self.snapshot_request != Some(request) {
                    self.metrics.record_worker_stale_result_dropped();
                    return;
                }
                self.snapshot_request = None;
                match snapshot {
                    Ok(snapshot) => {
                        self.metrics.log_line(
                            &format!(
                                "snapshot epoch={} candidates={}",
                                snapshot.epoch(),
                                snapshot.len()
                            ),
                            false,
                        );
                        self.snapshot = snapshot;
                        // A rebuilt snapshot invalidates every deep path and query
                        // (docs/18 §2): they describe the previous generation's geometry.
                        self.refine.on_snapshot_changed();
                        self.deep_target = None;
                        self.disarm_refinement();
                        self.update_hover();
    }
                    Err(error) => {
                        eprintln!("[snapclip][capture] window snapshot failed: {error}");
                    }
                }
            }
            DetectionResult::Confirmed {
                request,
                target,
                valid,
            } => {
                if self.confirm_request != Some(request) {
                    self.metrics.record_worker_stale_result_dropped();
                    return;
                }
                self.confirm_request = None;
                self.apply_confirmation(target, valid);
            }
            DetectionResult::Revalidated {
                request,
                target,
                validity,
            } => {
                if self.hover_request != Some(request) {
                    self.metrics.record_hover_revalidate_stale_dropped();
                    return;
                }
                self.hover_request = None;
                self.apply_hover_validity(target, validity);
            }
        }
    }

    /// Persist a hover re-validation result (docs/14 §5.5).
    ///
    /// The ordering matters and is the whole point of this routine: `BoundsChanged` is
    /// written **back into the snapshot** first, and only then is hover/preview recomputed
    /// from the same data source. Updating the highlight without the snapshot would leave
    /// the next `hit_test` reading the old rectangle and the highlight jumping back.
    fn apply_hover_validity(&mut self, target: WindowTarget, validity: HoverValidity) {
        let Some(current) = self.hover_target else {
            return;
        };
        if current.identity() != target.identity() {
            // A newer hover replaced this one while the worker was reading.
            self.metrics.record_hover_revalidate_stale_dropped();
            return;
        }
        match validity {
            HoverValidity::Valid => {}
            HoverValidity::BoundsChanged { .. } => {
                if !validity.applies_to(self.snapshot.epoch(), target.identity()) {
                    self.metrics.record_hover_revalidate_stale_dropped();
                    return;
                }
                if !self.snapshot.apply_candidate_update(&validity) {
                    return;
                }
                let new_bounds = validity.changed_bounds().unwrap_or(target.screen_bounds());
                self.metrics.log_line(
                    &format!(
                        "hover bounds changed hwnd={} new=({},{})->({},{})",
                        target.identity().hwnd,
                        new_bounds.left,
                        new_bounds.top,
                        new_bounds.right,
                        new_bounds.bottom
                    ),
                    false,
                );
                // Re-read from the snapshot that now holds the new rectangle.
                self.hover_target = None;
                self.update_hover();
                self.refresh_preview_for_cursor();
                self.invalidate_all();
            }
            HoverValidity::Invalid => {
                self.metrics.record_stale_target();
                self.metrics
                    .log_line(&format!("hover invalid hwnd={}", target.identity().hwnd), false);
                // Drop the highlight and the preview, then rebuild the snapshot. The
                // re-hit happens when the fresh snapshot lands, so a re-entry point is
                // never resolved against the snapshot that still lists the dead window.
                self.hover_target = None;
                self.gesture.clear_preview();
                self.sync_preview_rect();
                self.request_snapshot_refresh();
                self.invalidate_all();
            }
        }
    }

    /// Re-evaluate the dwell preview for the current cursor position and generation.
    fn refresh_preview_for_cursor(&mut self) {
        let generation = self.gesture.dwell_generation();
        let preview = self.preview_for_cursor();
        if self.gesture.apply_dwell(generation, preview) {
            self.sync_preview_rect();
            self.invalidate();
        }
    }

    /// Periodic hover re-validation tick. Only enqueues; never reads DWM here.
    fn on_hover_tick(&mut self) {
        if self.session.state() != CaptureState::Selecting {
            return;
        }
        // The one-shot hint expires on the wall clock, and a resting cursor produces no other
        // repaint: while it is still showing, keep one tick of life so it can go away on time
        // (docs/21 §5.21).
        if self.hint_text().is_some() {
            self.invalidate();
        }
        // Same reason, for ③b: the fade's 1.2 s deadline is on the wall clock too, and a resting
        // cursor produces no other repaint. One tick is enough to notice the deadline; from there
        // the render tick drives the fade itself (docs/21 §5.22).
        if self.chain_fade_pending() {
            self.invalidate();
        }
        // …and the same for the capture box's walk colour. A walk also touches the chain, so this is
        // normally the *same* repaint the line above already asked for; it is here because the walk
        // envelope's deadline is its own, and a line is cheaper than a coupling nobody can see.
        if self.walk_activity_pending() {
            self.invalidate();
        }
        self.poll_refinement_timeout();
        let Some(hover) = self.hover_target else {
            return;
        };
        if self.hover_request.is_some() {
            // Single-flight: one outstanding re-validation at a time.
            return;
        }
        self.hover_request = Some(self.detector.request_revalidate(hover));
    }

    /// Release an in-flight refinement query that blew its budget (docs/18 §3).
    ///
    /// A provider wedged inside a COM call cannot be interrupted, so the overlay stops waiting on
    /// it: the slot is freed, the gate retired, and the preview falls back to the v1 frame for
    /// this position instead of holding a rectangle no answer will ever confirm. Without this the
    /// session would silently lose deep selection — the same user-visible failure as a lost
    /// result, from a different cause.
    fn poll_refinement_timeout(&mut self) {
        let Some(expired) = self.refine.on_in_flight_timeout(Instant::now()) else {
            return;
        };
        self.refinement.retire();
        self.metrics.record_refinement_inflight_timeout();
        self.metrics.log_line(
            &format!("refinement timeout request={}", expired.get()),
            false,
        );
        self.refinement_pending = None;
        self.refresh_preview_for_cursor();
        self.submit_follow_up();
    }

    /// Arm the one-shot refinement dwell timer for the current target.
    ///
    /// Arming is the overlay's only statement of "an answer for this position is on its way", so
    /// the wait deadline starts (and restarts) here — including the confirming dwell a staged
    /// downgrade re-arms (docs/18 §13.3).
    fn arm_refinement(&mut self) {
        self.refinement_pending = Some(Instant::now());
        unsafe {
            SetTimer(
                self.window,
                REFINEMENT_TIMER_ID,
                crate::window_detection::REFINEMENT_DWELL_MS,
                None,
            )
        };
    }

    /// Nothing is waiting for an answer any more: the preview may fall back to the v1 frame.
    fn disarm_refinement(&mut self) {
        self.refinement_pending = None;
        unsafe { KillTimer(self.window, REFINEMENT_TIMER_ID) };
    }

    /// The refinement dwell expired: submit a deep query if the scheduler allows one.
    fn on_refinement_tick(&mut self) {
        // The timer is periodic by nature; killing it here is what makes the dwell a
        // one-shot. A new hover re-arms it.
        unsafe { KillTimer(self.window, REFINEMENT_TIMER_ID) };
        if let Some(job) = self.refine.on_dwell_due() {
            self.submit_refinement(job);
        }
    }

    /// The single-flight slot just freed: issue the position a dwell was deferred for, if any.
    fn submit_follow_up(&mut self) {
        if let Some(job) = self.refine.take_follow_up() {
            self.metrics.record_refinement_follow_up();
            // An answer for the current position is on its way again: hold the verified
            // rectangle instead of letting the preview fall back to the whole window.
            self.refinement_pending = Some(Instant::now());
            self.submit_refinement(job);
        }
    }

    /// Hand one scheduled query to the worker.
    fn submit_refinement(&mut self, job: RefinementJob) {
        // The query needs the window frame; it comes from the same snapshot the hover came
        // from, so a window that vanished simply skips its query.
        let Some(bounds) = self.snapshot.find(job.window).map(|candidate| candidate.screen_bounds)
        else {
            // No query was issued for this position, so nothing is coming: end the wait here
            // instead of holding a rectangle for a deadline that has no answer behind it.
            self.refine.on_failure(job.request);
            self.refinement_pending = None;
            return;
        };
        self.metrics.log_line(
            &format!(
                "refinement submit hwnd={} point=({},{}) epoch={}",
                job.window.hwnd, job.point.x, job.point.y, job.epoch
            ),
            false,
        );
        self.metrics.record_refinement_submitted();
        // The scheduler's request id is the one that comes back with the result, so the
        // worker is handed the whole job instead of issuing an id of its own.
        self.refinement.request(job, bounds);
    }

    /// Whether this window should currently let hit tests fall through to what is below it.
    ///
    /// The flag is set by the refinement worker around one accessibility point hit test
    /// (docs/21 §5.7). A held mouse button vetoes it: `HTTRANSPARENT` is a genuine pass-through, so a
    /// press or a release landing in that instant would go to the application underneath instead of
    /// the overlay — and a query can legitimately run while a marquee drag sits still. The button
    /// state is read with `GetAsyncKeyState` rather than `GetKeyState` so the veto follows the
    /// physical buttons and not whichever messages this thread happens to have processed.
    fn hit_test_passes_through(&self) -> bool {
        self.hit_test_pass_through.is_active() && !any_mouse_button_down()
    }

    /// A deep-selection result arrived.
    ///
    /// A published path only ever *refines* the v1 whole-window target: the overlay keeps
    /// painting the v1 frame when nothing came back, which is what makes a missing or
    /// failing accessibility provider a degradation rather than a regression.
    fn on_refinement_ready(&mut self) {
        let Some(result) = self.refinement.take_result() else {
            return;
        };
        self.apply_refinement_result(result);
        // Whatever happened to this answer, a position whose dwell expired while it ran has not
        // been asked about yet.
        self.submit_follow_up();
    }

    fn apply_refinement_result(&mut self, result: RefinementResult) {
        match result.outcome {
            RefinementOutcome::Target(target) => {
                if !self
                    .refine
                    .on_result(result.request, result.epoch, (*target).clone())
                {
                    // A superseded result (another window's query, or one whose point the cursor
                    // has left) must not end the wait for the question we are still asking.
                    self.metrics.record_refinement_superseded();
                    return;
                }
                // The current question has been answered. The staging branch below re-arms the
                // dwell, which starts a new wait; anything else ends the wait here.
                self.refinement_pending = None;
                // A downgrade (a shallower target that still contains what is on screen) is by
                // far the most likely reading of "the cursor passed through a parent
                // container". Show it only once the next dwell reproduces it at the same
                // point; anything else would expand the frame during a transit (docs/18 §13.3).
                if classify_replacement(self.deep_target.as_ref(), &target) == Replacement::NeedsConfirmation
                {
                    let cursor = self.cursor_screen();
                    let confirmed = match (&self.pending_downgrade, cursor) {
                        (Some((rect, staged)), Some(current)) => {
                            *rect == target.screen_bounds && points_close(*staged, current)
                        }
                        _ => false,
                    };
                    if !confirmed {
                        self.metrics.record_refinement_downgrade_staged();
                        self.pending_downgrade = cursor.map(|point| (target.screen_bounds, point));
                        // Ask again after the dwell: a resting cursor reproduces the target and
                        // the downgrade is applied then; moving clears it.
                        self.arm_refinement();
                        return;
                    }
                    self.pending_downgrade = None;
                } else {
                    self.pending_downgrade = None;
                }
                self.metrics.log_line(
                    &format!(
                        "refinement published hwnd={} bounds=({},{})->({},{}) depth={} reason={:?}",
                        target.window.hwnd,
                        target.screen_bounds.left,
                        target.screen_bounds.top,
                        target.screen_bounds.right,
                        target.screen_bounds.bottom,
                        target.path.len(),
                        target.stop_reason
                    ),
                    false,
                );
                self.deep_target = Some(*target);
                self.metrics.record_refinement_published(result.elapsed);
                self.refresh_preview_for_cursor();
                self.invalidate_all();
            }
            RefinementOutcome::Empty(reason) => {
                // Free the single-flight slot so the next dwell can try again.
                if self.refine.on_failure(result.request) {
                    // Nothing is coming for this position any more: the preview falls back to
                    // the v1 frame rather than holding a rectangle the provider never confirmed.
                    self.refinement_pending = None;
                    self.metrics.record_refinement_empty();
                    self.metrics
                        .log_line(&format!("refinement empty reason={reason:?}"), false);
                }
            }
        }
    }

    fn arm_hover_timer(&mut self) {
        unsafe {
            SetTimer(
                self.window,
                HOVER_TIMER_ID,
                DEFAULT_HOVER_REVALIDATE_MS,
                None,
            )
        };
    }

    fn disarm_hover_timer(&mut self) {
        self.hover_request = None;
        unsafe { KillTimer(self.window, HOVER_TIMER_ID) };
    }

    /// `Enter` while a preview is shown: hand the target to the worker for validation.
    ///
    /// Returns whether a confirmation was started, so the caller can fall through to the
    /// normal confirm path when there is no preview.
    fn confirm_snap_preview(&mut self) -> bool {
        let Some(preview) = self.gesture.snap_preview() else {
            return false;
        };
        // Repeated Enter keeps only the newest confirmation.
        let request = self.detector.request_confirm(preview.target);
        self.confirm_request = Some(request);
        // What the user is about to commit next to what deep selection last published, both in
        // monitor-local pixels: the two disagreeing is exactly the "probe resolves the element,
        // the app confirms the window" gap (docs/21 §8), and this line tells which side of that
        // gap a session fell on.
        let deep_local = self.layout().zip(self.deep_target.as_ref()).map(|(layout, deep)| {
            // What the walk selected, not necessarily what the refinement published (docs/21 §5.17).
            window_rect_to_local(
                self.selected_deep_level()
                    .map(|level| level.rect)
                    .unwrap_or(deep.screen_bounds),
                &layout,
            )
        });
        self.metrics.log_line(
            &format!(
                "confirm requested hwnd={} epoch={} confirmation={} preview_local={} \
                 deep_local={} pending={} deep={} level={}",
                preview.target.identity().hwnd,
                preview.target.candidate.snapshot_epoch,
                request.get(),
                describe_rect(preview.selection),
                deep_local.map_or_else(|| "none".to_owned(), describe_rect),
                self.refinement_pending.is_some(),
                describe_deep(self.deep_target.as_ref()),
                describe_level(self.deep_levels),
            ),
            true,
        );
        true
    }

    /// Apply a validated confirmation (docs/14 §5.4).
    fn apply_confirmation(&mut self, target: WindowTarget, valid: bool) {
        let Some(preview) = self.gesture.snap_preview() else {
            return;
        };
        if preview.target.identity() != target.identity() {
            self.metrics.record_worker_stale_result_dropped();
            return;
        }
        if valid && self.session.snap_to(preview.selection) {
            self.metrics.log_line(
                &format!(
                    "snap confirmed hwnd={} selection=({},{})->({},{})",
                    target.identity().hwnd,
                    preview.selection.left,
                    preview.selection.top,
                    preview.selection.right,
                    preview.selection.bottom
                ),
                true,
            );
            self.gesture.clear_preview();
            self.sync_preview_rect();
            self.hover_target = None;
            self.disarm_dwell();
            self.publish_state();
            self.invalidate_all();
            return;
        }
        // The target turned out to be gone: keep the selection the user had, drop the
        // preview, refresh the snapshot and let the next dwell try again.
        self.metrics.record_stale_target();
        eprintln!(
            "[snapclip][capture] snap confirmation failed hwnd={} valid={}",
            target.identity().hwnd,
            valid
        );
        self.gesture.clear_preview();
        self.sync_preview_rect();
        self.request_snapshot_refresh();
        self.update_hover();
        self.invalidate_all();
    }

}

// Crate-internal re-export: the window/thread half moved to `window_host` (T1.6.1) and
// `crate::windows::overlay::{LEVEL_HINT, level_hint, preview_label}` is how the render
// font gate reaches it. `pub(crate)`, not `pub`: nothing here is external API.
pub(crate) use window_host::*;

#[cfg(test)]
mod tests;
mod window_host;
mod state;
mod render_submit;
mod window_restore;
mod input;
mod session;
