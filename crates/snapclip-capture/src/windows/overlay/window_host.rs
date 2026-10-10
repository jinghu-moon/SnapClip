//! Window class, HWND, message loop, focus lifecycle and the `WM_*` dispatch skeleton
//! (docs/23 T1.6.1).
//!
//! Everything here is about the *window and the thread it lives on*: creation, focus, the
//! hotkey, exclusion from capture, the message pump and the free helpers the message
//! handlers share with the controller.

use super::*;

impl OverlayMessageHandler for OverlayController {
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
            detection_worker::DETECTION_READY_MESSAGE => {
                self.on_detection_ready();
                Some(0)
            }
            refinement_worker::REFINEMENT_READY_MESSAGE => {
                self.on_refinement_ready();
                Some(0)
            }
            SCROLL_READY_MESSAGE => {
                // The driver published into the preview port; fold it into the panel and let the
                // render tick paint. The wake-up is the only one this port gets (docs/30 §19.3: the
                // port deliberately carries no second one), so everything published since the last
                // wake-up has to be drained here — which is what `on_scroll_ready` does.
                self.on_scroll_ready();
                Some(0)
            }
            WM_HOTKEY => {
                // The id is the only thing the message carries about which key fired, so the
                // table is the dispatch (`docs/32` §4.1). An id this crate did not register
                // reaches no entry point — and is still swallowed, so hotkey traffic never
                // falls through to `DefWindowProcW`.
                if let Some(hotkey) = hotkey::from_id(wparam as i32) {
                    eprintln!("[snapclip][capture] WM_HOTKEY {} received", hotkey.label);
                    match hotkey.kind {
                        hotkey::HotkeyKind::Capture => self.start_session(),
                        hotkey::HotkeyKind::Scroll => self.start_scroll_entry(),
                    }
                }
                Some(0)
            }
            WM_KEYDOWN => {
                // When the user's IME is Chinese/Japanese/Korean the letter keys
                // are handed to the IME as `WM_KEYDOWN vk=VK_PROCESSKEY (0xE5)`
                // before we can match them. We already close the overlay's IMC
                // on focus-in, but a stale IME (or a mid-session Win+Space
                // layout swap) can still deliver 0xE5. Translate the physical
                // scan code out of `lParam` bits 16-23 in that case so `S/C/P`
                // keep firing.
                let raw_vk = wparam as u32;
                let vk = if raw_vk == 0xE5 {
                    let scan = ((lparam as u32) >> 16) & 0xFF;
                    let mapped = unsafe { MapVirtualKeyW(scan, MAPVK_VSC_TO_VK) };
                    if mapped != 0 { mapped } else { raw_vk }
                } else {
                    raw_vk
                };
                self.on_key_down(vk);
                Some(0)
            }
            WM_KEYUP => {
                let raw_vk = wparam as u32;
                let vk = if raw_vk == 0xE5 {
                    let scan = ((lparam as u32) >> 16) & 0xFF;
                    let mapped = unsafe { MapVirtualKeyW(scan, MAPVK_VSC_TO_VK) };
                    if mapped != 0 { mapped } else { raw_vk }
                } else {
                    raw_vk
                };
                if vk == b'Z' as u32 {
                    self.z_held = false;
                }
                None
            }
            WM_MOUSEWHEEL => {
                let delta = ((wparam >> 16) & 0xFFFF) as u16 as i16;
                if self.z_held && self.session.state().is_active() {
                    let direction = if delta > 0 { 1 } else { -1 };
                    self.magnifier_zoom = MagnifierConfig::zoom_step(self.magnifier_zoom, direction);
                    self.invalidate();
                    Some(0)
                } else {
                    // Plain wheel walks the deep-selection levels (docs/21 §5.17): up toward the
                    // window frame, down toward the element under the cursor. Consumed, so the page
                    // underneath never sees a scroll it did not ask for.
                    //
                    // The message is translated into level steps first, so a touchpad's small deltas
                    // add up to one walk instead of five (v3 B2, docs/21 §5.24).
                    let steps = self.wheel.steps(delta as i32, Instant::now());
                    let mut walked = false;
                    for _ in 0..steps.abs() {
                        // A stop that does not move — the end of the chain — must not end the count:
                        // the walk is also where the level hint gets taught.
                        walked |= self.step_deep_level(-steps.signum());
                    }
                    walked.then_some(0)
                }
            }
            WM_SETFOCUS => {
                self.disable_ime_for_overlay();
                None
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
            // While the refinement worker has an accessibility point hit test in flight, this
            // window lets that hit test fall through to the application underneath: UIA has no
            // notion of Z order and would otherwise answer the overlay for every query, which is
            // exactly what the precision top-up needs (docs/21 §5.7).
            WM_NCHITTEST => Some(if self.hit_test_passes_through() {
                HTTRANSPARENT as LRESULT
            } else {
                HTCLIENT as LRESULT
            }),
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
                } else if (wparam as usize) == DWELL_TIMER_ID {
                    // Cursor rested long enough: query the cached snapshot for a preview.
                    self.on_dwell();
                    Some(0)
                } else if (wparam as usize) == HOVER_TIMER_ID {
                    // Re-validate the hovered window on the detection worker; this thread
                    // only enqueues (docs/14 §5.5).
                    self.on_hover_tick();
                    Some(0)
                } else if (wparam as usize) == REFINEMENT_TIMER_ID {
                    // The target held still long enough: hand a deep query to the
                    // refinement worker (docs/18 §2).
                    self.on_refinement_tick();
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

/// Keep the overlay out of capture output (docs/14 §7, layer 1 of three).
///
/// `WDA_EXCLUDEFROMCAPTURE` requires Windows 10 2004+. SnapClip deliberately does not
/// probe the OS version first: the capture path freezes the frame **before** the overlay
/// is shown, so the fallback is unconditional and always in effect. A failure here only
/// means the extra hardening is unavailable on this build — it can never mean the
/// overlay could reach a screenshot.
pub(super) fn exclude_overlay_from_capture(window: HWND) -> Result<(), u32> {
    if unsafe { SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE) } != 0 {
        return Ok(());
    }
    Err(unsafe { GetLastError() })
}

pub(super) fn describe_rect(rect: Rect) -> String {
    format!("({},{})->({},{})", rect.left, rect.top, rect.right, rect.bottom)
}

/// How long the chain stays fully visible after the last touch (docs/21 §5.22).
///
/// Long enough to read the chain after a wheel notch, short enough that it does not sit over the
/// page while the user is doing something else with the overlay open.
pub(super) const CHAIN_FADE_AFTER_MS: u64 = 1200;
/// How long the fade itself takes. Eight steps over 240 ms is one repaint every 30 ms — bounded,
/// and fast enough to read as a fade rather than as a sequence of pictures.
pub(super) const CHAIN_FADE_MS: u64 = 240;
/// Steps an **alpha** fade is quantised into. Each step is a full-surface present, so this *is* the
/// cost — and a step of `1/8` of an alpha is below the eye's threshold, which is why the rings keep
/// this coarse ramp while the capture box's *colour* does not (docs/21 §5.24, ①).
pub(super) const CHAIN_FADE_STEPS: u32 = 8;

/// How long the chain takes to come back when it is used (docs/21 §5.24, ②).
///
/// The counterpart to the fade: a chain that appeared out of nowhere reads as a glitch next to a
/// preview box that eases onto its target, so the rings ease in over the same kind of window.
pub(super) const CHAIN_RISE_MS: u64 = 120;
/// How long one ring takes to appear when a walk adds it to the chain (docs/21 §5.24, ②).
///
/// Per ring, not per chain: walking up a level drops the outermost ring and adds one inside, and
/// only the one that appeared should animate — the rings that survived are already on screen.
pub(super) const RING_APPEAR_MS: u64 = 140;
/// How long the capture box takes to fade in when a preview appears (docs/21 §5.24, ②).
///
/// Only on appearance, never on a re-target: a walk changes the box on every notch, and re-fading
/// there would make the box blink while the user is stepping through levels.
pub(super) const PREVIEW_APPEAR_MS: u64 = 120;

/// How long the capture box takes to turn green after a walk (docs/21 §5.24, A3).
///
/// The prototype's 100 ms, which is about how long the preview box takes to ease onto a new level:
/// a colour that jumped on the same frame as the notch would read as a glitch, and it has to have
/// arrived by the time the box has.
pub(super) const WALK_RISE_MS: u64 = 100;

/// How much of a fade has run, `idle` after the event: 0 while it is recent, 1 when it is gone.
///
/// Continuous and **eased** (`out_quad` — the same curve the preview rectangle uses, docs/21 §5.24
/// ③), because this is the share a *colour* interpolates: quantising it is invisible on an alpha and
/// visible banding on a hue (①). The rings still quantise, on top of this, through [`fade_at_steps`].
pub(super) fn fade_at(idle: Duration, hold_ms: u64, fade_ms: u64) -> f32 {
    let idle_ms = idle.as_millis() as u64;
    if idle_ms <= hold_ms {
        return 0.0;
    }
    out_quad((idle_ms - hold_ms) as f32 / fade_ms as f32)
}

/// The same fade, quantised into `steps` values — for an alpha, where the steps are invisible.
pub(super) fn fade_at_steps(idle: Duration, hold_ms: u64, fade_ms: u64, steps: u32) -> f32 {
    (fade_at(idle, hold_ms, fade_ms) * steps as f32).round() / steps as f32
}

/// How far an element that appeared `elapsed` ago has come in, over `over_ms` (docs/21 §5.24, ②).
///
/// Pure and shared by the rings and the capture box, so "appear" means one thing in this file.
pub(super) fn appear_share(elapsed: Duration, over_ms: u64) -> f32 {
    out_quad((elapsed.as_secs_f32() * 1000.0 / over_ms as f32).clamp(0.0, 1.0))
}

/// How visible the chain is, `idle` after the last touch (docs/21 §5.22).
pub(crate) fn chain_visibility_at(idle: Duration) -> f32 {
    1.0 - fade_at_steps(idle, CHAIN_FADE_AFTER_MS, CHAIN_FADE_MS, CHAIN_FADE_STEPS)
}

/// The one-shot hint that explains the level walk (docs/21 §5.21).
///
/// Without it the feature is invisible: a wheel that silently changes what will be captured is
/// indistinguishable from a wheel that does nothing.
///
/// `pub(crate)` so the embedded-font coverage gate can require its characters
/// (`win::d2d::tests::the_embedded_subset_covers_the_strings_the_overlay_draws`).
pub(crate) const LEVEL_HINT: &str = "滚轮 / ↑↓ 换吸附层级";

/// How long a one-shot hint stays on screen.
pub(crate) const LEVEL_HINT_MS: u64 = 2600;

/// The sentence that explains the level badge, shown the first time the walk shows one (§5.24 A1).
///
/// It has to answer two things the chip cannot: what the two numbers count, and which end is which.
/// `↑ = 窗口` is the part nobody can guess, and it is what makes the chip legible the next time the
/// wheel is used. The numbers are the badge's own, so the sentence and the chip never disagree.
pub(crate) fn level_hint(reach: LevelReach) -> String {
    format!(
        "↑{} ↓{} · ↑ = 窗口 · 滚轮 / ↑↓ 切换",
        reach.up, reach.down
    )
}

/// Whether a successful level walk should (re)arm the teaching sentence (docs/21 §5.21).
///
/// Two rules about two different things:
/// * the lesson is **once per session** — the sentence belongs to the moment the number appears,
///   and a user who already knows does not need it over every later rectangle;
/// * but while that same sentence is still on screen, another walk **rewrites its numbers**, so
///   the sentence and the label never disagree about which level the user is on.
pub(super) fn should_teach(taught: bool, showing: bool) -> bool {
    !taught || showing
}

/// The text the automatic-snap preview's label shows (docs/21 §5.21).
///
/// Pure so the format is testable. Beyond the size it carries the three things a user cannot infer
/// from the rectangle: **what the box is** (窗口 / 容器 / 元素) and that nothing answered for this
/// position — the last one as `?`, because a fallback must not look like a confident answer.
///
/// The kind word comes first because it is the question the user is actually asking ("did it snap
/// to the thing, or to the shell around it?"), and it is the only one of the two facts that needs no
/// explanation. `容器` is what makes a walked-up-to box self-explanatory without reading a number.
///
/// **The level counter is not here** (docs/21 §5.22): it moved to its own badge — now the `↑n ↓n`
/// chip of §5.24 A1 — because the two say different things (the label says "what this box is", the
/// badge says "how much chain is left") and because leaving it in made the label change width on
/// every notch of the wheel.
///
/// `pub(crate)` so the embedded-font coverage gate can require its characters
/// (`win::d2d::tests::the_embedded_subset_covers_the_strings_the_overlay_draws`).
pub(crate) fn preview_label(
    rect: Rect,
    is_window: bool,
    kind: Option<LevelKind>,
    walked: bool,
    degraded: bool,
) -> String {
    // The noun comes from the transport when it has one (docs/21 §5.24, B6), and from the label's own
    // two words when it does not: a box nobody could name is still "the element" or "the container
    // around it", and saying `面板` for every anonymous `Pane` would be worse than saying nothing.
    let kind = if is_window {
        "窗口"
    } else if let Some(noun) = kind.and_then(LevelKind::noun_zh) {
        noun
    } else if walked {
        "容器"
    } else {
        "元素"
    };
    let mut text = format!("{}×{} px  {kind}", rect.width(), rect.height());
    if degraded {
        text.push('?');
    }
    text
}

/// What the level badge shows: how many stops the walk still has each way (docs/21 §5.24, A1).
///
/// `None` while the answer itself is selected: the deepest level *is* the answer and is where every
/// preview starts, so a badge there would only report the state "nothing has been walked". A
/// single-level chain has nothing to count either.
///
/// The counts come from the same `next_visible_stop` the wheel uses, so the chip cannot promise
/// notches the walk would not take — the failure mode that made the old `6/9` misleading once B1
/// started skipping levels that look identical.
pub(crate) fn level_badge_reach(chain: Option<LevelChain>, path: &[PathLevel]) -> Option<LevelReach> {
    let chain = chain.filter(|chain| chain.len() > 1 && !chain.is_deepest())?;
    let threshold = RingOptions::default().collapse_gap_px;
    Some(LevelReach {
        up: stops_from(path, chain.index(), -1, threshold),
        down: stops_from(path, chain.index(), 1, threshold),
    })
}

/// `2/6` for the confirm line: which level of the chain the ancestor walk selected (docs/21 §5.17).
pub(super) fn describe_level(chain: Option<LevelChain>) -> String {
    match chain {
        Some(chain) if !chain.is_empty() => format!("{}/{}", chain.index() + 1, chain.len()),
        _ => "deepest".to_owned(),
    }
}

/// Whether any mouse button is physically held down.
///
/// Used to veto the hit-test pass-through: the flag exists so that one accessibility point hit test
/// can see through the overlay (docs/21 §5.7), and a genuine pass-through must never take a click or
/// a release away from the overlay while the user is dragging.
pub(super) fn any_mouse_button_down() -> bool {
    const DOWN: i16 = 0x8000_u16 as i16;
    [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON]
        .into_iter()
        .any(|key| unsafe { GetAsyncKeyState(key as i32) } & DOWN != 0)
}

/// One-line account of a deep target for the default (non-verbose) session log.
pub(super) fn describe_deep(deep: Option<&DeepTarget>) -> String {
    match deep {
        None => "none".to_owned(),
        Some(deep) => {
            // The deepest level's *kind* is what the size label prints as a noun (docs/21 §5.24 B6),
            // so it belongs in the line that says what the session answered with: "the label says
            // 容器" and "the walk called it Pane" are the same fact from two sides.
            let deepest = deep
                .path
                .last()
                .map(|level| level.kind.debug_name())
                .unwrap_or("none");
            format!(
                "hwnd={} kind={:?} bounds={} depth={} reason={:?} deepest={}",
                deep.window.hwnd,
                deep.kind,
                describe_rect(deep.screen_bounds),
                deep.path.len(),
                deep.stop_reason,
                deepest
            )
        }
    }
}

/// Whether two cursor positions count as "the cursor has not moved" for the downgrade
/// confirmation. The dwell already guarantees stillness; a couple of pixels of jitter must not
/// cancel a legitimate confirmation.
pub(super) fn points_close(left: Point, right: Point) -> bool {
    (left.x - right.x).abs() <= 3 && (left.y - right.y).abs() <= 3
}

/// System drag threshold (`SM_CXDRAG`) in physical pixels for a monitor DPI.
///
/// The value is a logical distance, so it is scaled the same way the reference selector
/// scales `QApplication::startDragDistance()`. Below the threshold a press stays a click;
/// above it the gesture becomes a free drag (docs/14 §4.2).
pub(super) fn system_drag_threshold(dpi: u32) -> i32 {
    let base = unsafe { GetSystemMetrics(SM_CXDRAG) }.max(1);
    let scaled = (base as f32) * (dpi.max(96) as f32 / 96.0);
    scaled.round().max(1.0) as i32
}

/// Automatic-snap radius in physical pixels (docs/14 §5.4).
pub(super) fn snap_radius_px() -> u32 {
    DEFAULT_SNAP_RADIUS_PX
}

pub(super) fn point_from_lparam(lparam: LPARAM) -> POINT {
    POINT {
        x: (lparam & 0xFFFF) as u16 as i16 as i32,
        y: ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32,
    }
}

pub(super) fn overlay_thread(
    service: Arc<CaptureService>,
    sink: Arc<dyn CaptureEventSink>,
    clipboard: Arc<dyn ClipboardWriter>,
    writer: Arc<dyn ArtifactWriter>,
    options: crate::window_detection::DetectionOptions,
    shared: Arc<Mutex<OverlayShared>>,
    annotation_rx: mpsc::Receiver<AnnotationCommand>,
    ready: mpsc::SyncSender<SystemResult>,
) {
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

    // Layer 1 of the three-layer self-exclusion (docs/14 §7): keep the overlay out of any
    // capture, even if a future path captures while it is visible. The hit-filter layer is
    // registered by the controller, and the fallback is the capture-before-show ordering.
    match exclude_overlay_from_capture(window) {
        Ok(()) => eprintln!("[snapclip][capture] overlay excluded from capture"),
        Err(code) => eprintln!(
            "[snapclip][capture] overlay affinity unavailable (Win32 error {code}); \
             relying on capture-before-show"
        ),
    }

    if let Err(error) = hotkey::register(window) {
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

    let mut controller: Box<dyn OverlayMessageHandler> =
        Box::new(OverlayController::new(
            service,
            sink,
            clipboard,
            writer,
            options,
            shared,
            annotation_rx,
            window,
            thread_id,
        ));
    let handler_ptr = (&mut controller) as *mut Box<dyn OverlayMessageHandler>;
    ACTIVE_HANDLER.with(|slot| slot.set(handler_ptr));

    if ready.send(Ok(thread_id.to_string())).is_err() {
        ACTIVE_HANDLER.with(|slot| slot.set(std::ptr::null_mut()));
        unsafe {
            hotkey::unregister(window);
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
        hotkey::unregister(window);
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


pub(super) const OVERLAY_CLASS: &[u16] = &[
    83, 110, 97, 112, 67, 108, 105, 112, 67, 97, 112, 116, 117, 114, 101, 79, 118, 101, 114, 108,
    97, 121, 0,
];
pub(super) const OVERLAY_TITLE: &[u16] = &[83, 110, 97, 112, 67, 108, 105, 112, 0];
