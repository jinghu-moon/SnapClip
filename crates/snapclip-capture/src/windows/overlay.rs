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
/// `WM_APP`-based notification that the scroll session's state changed, sent from the scroll driver
/// to the overlay thread (`docs/30` §21.4).
///
/// `pub(crate)`: both ends live in this crate — the driver posts it, this overlay thread's message
/// loop consumes it — so there is nothing to expose to the application. The offset is checked
/// against every id that is already taken by
/// `tests::the_scroll_message_id_collides_with_nothing_that_is_already_posted`.
pub(crate) const SCROLL_READY_MESSAGE: u32 = WM_APP + 45;
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
/// safe to call from any thread: each one posts to the overlay thread and returns.
pub struct WindowsOverlay {
    /// Window-message thread id of the overlay thread, in the string form used by the
    /// readiness handshake.
    thread_id: String,
    thread: Mutex<Option<JoinHandle<()>>>,
    shared: Arc<Mutex<OverlayShared>>,
    /// Producer end of the annotation mailbox. The toolbar path pushes a command
    /// here and posts an [`OverlayCommand::Annotation`] wake-up; the overlay
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
        // `try_send`, not `send`: a wedged overlay must never block the caller.
        // The bounded mailbox only needs to absorb a burst of clicks; if it is
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
    /// The scroll session's preview port, once a scroll capture has been handed to the overlay
    /// (docs/30 §19.3).
    ///
    /// `Arc` because the driver thread publishes into it while this thread drains it, and the
    /// controller only ever holds the consumer side. `None` until [`Self::watch_scroll_preview`].
    scroll_preview: Option<std::sync::Arc<crate::scroll::preview::PreviewStream>>,
    /// The panel model the overlay paints (docs/30 §19.1/§19.4/§19.7), folded forward from
    /// `scroll_preview` on every [`SCROLL_READY_MESSAGE`].
    scroll_panel: Option<crate::scroll::panel::ScrollPanel>,
    /// The layout of the monitor this session's frame came from.
    ///
    /// Kept so the scroll entry can turn the window snapshot's **virtual-desktop** rectangles into
    /// this overlay's **monitor-local** ones (`docs/32` `P7.05`); the conversion itself lives in
    /// `windows/scroll_target.rs`, which is the only place allowed to do it.
    monitor_layout: Option<MonitorLayout>,
    /// The running scroll session, once a selection has been handed over (`docs/32` ADR-19).
    ///
    /// The controller owns it because it owns the thread's lifetime: `teardown` joins the driver,
    /// and a session that outlived its controller would be a thread nobody can stop.
    scroll_runtime: Option<crate::scroll::session::ScrollRuntime>,
    /// Whether `F7` started this capture session — i.e. whether confirming it means "start
    /// scrolling" rather than "produce a screenshot" (`docs/32` §4.4, `OQ-27`).
    scroll_entry_armed: bool,
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
            scroll_preview: None,
            scroll_panel: None,
            monitor_layout: None,
            scroll_runtime: None,
            scroll_entry_armed: false,
        }
    }


    // ---- input -----------------------------------------------------------


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
mod hover;
