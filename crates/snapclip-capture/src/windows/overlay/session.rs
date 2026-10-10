//! Session lifecycle: start, frame, confirm, export, finish, cancel (docs/23 D3, slice 4).
//!
//! Split out of `overlay.rs`: everything that gives a capture session its shape — the state
//! it publishes, the worker hand-offs, the single cancellation path and the colour sampler.
//! `pub(super)` because the parent module's window procedure and the input handlers call in.

use super::*;

/// The line height the plan's starting gain assumes, in physical pixels.
///
/// `docs/30` §16.2's `starting_px_per_notch(lines, height)` was calibrated with `(3, 20) ⇒ 60 px`,
/// and `E-INJECT-1` measured **100 px per notch** on this machine (`docs/30` §24.6.1). The closed
/// loop is what turns the difference into a correction (`§16.6`), so this is a starting point
/// rather than a measurement — but it has to exist, because a plan that assumed zero would never
/// move the target and the loop would have nothing to correct from.
pub(super) const SCROLL_LINE_HEIGHT_PX: u32 = 20;

impl OverlayController {
    pub(super) fn session_id(&self) -> String {
        self.session.id().to_string()
    }

    pub(super) fn layout(&self) -> Option<MonitorLayout> {
        self.renderer.as_ref().map(|renderer| renderer.layout().clone())
    }

    pub(super) fn publish_state(&self) {
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
    pub(super) fn drain_annotation_commands(&mut self) {
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
    pub(super) fn apply_annotation_command(&mut self, command: AnnotationCommand) {
        match command {
            AnnotationCommand::SelectTool => self.annotation_tool = None,
            AnnotationCommand::Tool(kind) => self.annotation_tool = Some(kind),
            other => self.annotation_doc.execute(other),
        }
    }

    /// `F7`: arm the scroll intent and open the ordinary selection UI (`docs/32` §4.4, `OQ-27`).
    ///
    /// The shape is **"select, then confirm means scroll"**: the region a session scrolls is the
    /// region the user picks, and the window it reads from is resolved from that selection
    /// (`windows/scroll_target.rs`). Starting the capture session is what gives the user something
    /// to select on, and it is also where the DPI, the frozen frame under the panel and the monitor
    /// layout come from.
    pub(super) fn start_scroll_entry(&mut self) {
        self.scroll_entry_armed = true;
        eprintln!("[snapclip][capture] scroll entry armed (F7): select a region and confirm");
        self.start_session();
    }

    /// Hand a confirmed selection to a scroll session and start its driver (`docs/32` `P7.05`).
    ///
    /// The order is the contract: resolve the window, hand over — which **ends** the capture
    /// session (ADR-19) — then build the plan from what the hand-over carries, then start the
    /// driver and give the overlay the preview port. Everything after the hand-over speaks about a
    /// session whose capture frame is already gone, so nothing here may read `self.session.frame()`.
    ///
    /// Every refusal happens **before** the hand-over: the user keeps a selected session they can
    /// still export, which is the same promise `begin_scroll` itself makes for a window that does
    /// not overlap the selection.
    pub(super) fn begin_scroll_session(&mut self) {
        if self.scroll_runtime.is_some() {
            eprintln!("[snapclip][capture] a scroll session is already running");
            return;
        }
        let Some(layout) = self.monitor_layout.clone() else {
            eprintln!("[snapclip][capture] scroll entry refused: this session has no monitor layout");
            return;
        };
        let axis = crate::scroll::observation::Axis::Vertical;
        let choice = crate::windows::scroll_target::choose_in_snapshot(
            self.session.selection(),
            self.snapshot.candidates(),
            &layout,
            axis,
        );
        let target = match choice {
            crate::scroll::target::TargetChoice::Accepted(target) => target,
            other => {
                eprintln!("[snapclip][capture] scroll entry refused: {other:?}");
                return;
            }
        };
        let window = target.window();
        let bounds = target.bounds();
        let handle = window as isize;
        // The aim point is part of a `SendInput` injection under `MOUSE_POS` routing (§24.6 rule 1),
        // so it is resolved before the hand-over: a window whose client area cannot be resolved has
        // no injectable point, and refusing after the capture session ended would strand the user.
        let Some(screen) = window::client_center(handle) else {
            eprintln!("[snapclip][capture] scroll entry refused: 0x{window:x} has no client area");
            return;
        };

        let handoff = match self.session.begin_scroll(window, bounds) {
            Ok(handoff) => handoff,
            Err(error) => {
                eprintln!("[snapclip][capture] scroll hand-over failed: {error}");
                return;
            }
        };
        self.scroll_entry_armed = false;

        let target = handoff.target();
        let cross_len = target.bounds().width().max(0) as u64;
        let extent = target.bounds().height().max(0) as u64;
        // The plan states the viewport as a `u32` (a viewport taller than 4 Gi px is not a case the
        // canvas has an answer for) while the memory budget is computed in pixels; the clamp keeps
        // the two statements about the same rectangle rather than panicking on an impossible one.
        let viewport_extent = u32::try_from(extent).unwrap_or(u32::MAX);
        let budget = crate::scroll::canvas::MemoryBudget::for_viewport(cross_len, extent);
        let plan =
            crate::scroll::session::ScrollPlan::new(axis, cross_len, viewport_extent, budget)
                .with_wheel(
                    crate::windows::scroll_actuator::wheel_lines_per_notch(),
                    SCROLL_LINE_HEIGHT_PX,
                );
        let probe = crate::windows::scroll_actuator::TargetProbe::at_session_start(handle);
        // The device is created **inside** the factory, on the driver thread: that is what makes
        // the driver its owner (`docs/30` §21.3.1), and it is why `scroll_source.rs` never names
        // `GraphicsDevice::create` — its own gate forbids it (§11.2).
        let runtime = crate::scroll::session::ScrollRuntime::start(
            plan,
            move || {
                let device = crate::windows::win::d3d11::GraphicsDevice::create()
                    .map(std::sync::Arc::new);
                crate::windows::scroll_source::OpeningFrameSource::open(device, handle, axis, bounds)
            },
            crate::windows::scroll_actuator::WindowWheelActuator::new(handle, screen, axis, probe),
        );
        self.watch_scroll_preview(runtime.preview().clone(), cross_len, extent);
        self.scroll_runtime = Some(runtime);
        eprintln!(
            "[snapclip][capture] scroll session started window=0x{window:x} cross={cross_len} \
             extent={extent} capture_dpi={}",
            handoff.dpi()
        );
    }

    /// Perform a press on the scroll panel's controls (`docs/32` `P7.06`).
    ///
    /// One `match` with **no wildcard arm**: a new `PanelAction` is a compile error here rather than
    /// a button that silently does nothing, which is how "every action maps to exactly one
    /// controller call" stays true without a test that could be forgotten.
    ///
    /// The controller calls are the sticky-bit / capacity-1 commands of §27.2, so none of them
    /// blocks the overlay thread: `stop` and `cancel` latch a promise the driver reads at its next
    /// cancellation point, `undo` adds one request, and `set_follow(true)` writes the one-slot
    /// follow value.
    pub(super) fn apply_panel_action(&mut self, action: crate::scroll::panel::PanelAction) {
        let Some(runtime) = self.scroll_runtime.as_ref() else {
            return;
        };
        match action {
            crate::scroll::panel::PanelAction::ReturnToLatest => {
                runtime.controller().set_follow(true);
            }
            crate::scroll::panel::PanelAction::Undo => runtime.controller().undo(),
            crate::scroll::panel::PanelAction::Stop => runtime.controller().stop(),
            crate::scroll::panel::PanelAction::Cancel => runtime.controller().cancel(),
        }
    }

    /// Stop and join the scroll driver, if one is running (`docs/32` `P7.05`).
    ///
    /// Separate from the capture session's teardown because the promises differ: a cancelled scroll
    /// session produces **no artifact** (`docs/32` §3.4), and its driver has to be joined before
    /// session produces **no artifact** (`docs/32` §3.4), and its driver has to be joined before
    /// the controller goes away — a thread that outlived its owner is a thread nobody can stop.
    ///
    /// The artifact path (stop → row-band export → clipboard/history) is `P7.09`/`P7.10`; until
    /// then this logs the session's own tallies and hands the panel back to nothing, which is the
    /// honest intermediate state rather than a stop button that quietly does half the job.
    pub(super) fn stop_scroll_session(&mut self, reason: &str) {
        let Some(mut runtime) = self.scroll_runtime.take() else {
            return;
        };
        runtime.controller().cancel();
        match runtime.teardown() {
            Some(session) => eprintln!(
                "[snapclip][capture] scroll session ended reason={reason} steps={} committed={} \
                 discarded={}",
                session.step(),
                session.committed(),
                session.discarded()
            ),
            None => eprintln!(
                "[snapclip][capture] scroll session ended reason={reason} (the driver returned \
                 nothing: it was already torn down)"
            ),
        }
        self.scroll_preview = None;
        self.scroll_panel = None;
    }

    /// `F5`: submit a capture request and enter `Preparing`.
    ///
    /// Nothing here waits on WGC/BitBlt: the freeze happens on the worker
    /// thread and the message pump keeps running, so `Esc` cancels while the
    /// screen is still being captured (docs/11 §2.2/§3.3).
    pub(super) fn start_session(&mut self) {
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
            snapclip_model::unix_time_ms(),
            self.session_counter
        );
        self.session = CaptureSession::new(session_id);
        // Per-session paint state starts clean: the previous session's walk colour, chain visibility
        // and appear clocks must not be inherited by the next F5 (docs/21 §5.24).
        self.walk = WalkColour::default();
        self.chain = ChainVisibility::default();
        self.ring_appear.clear();
        self.preview_appeared_at = None;
        if let Err(error) = self.session.preparing() {
            eprintln!("[snapclip][capture] session preparing transition failed: {error}");
            self.fail(None, error, "none");
            return;
        }
        self.publish_state();
        let layout = monitor.layout.clone();
        self.sink.on_started(&self.session_id(), &layout);

        // Window detection is per-session (docs/14 §5.3): no hover, no preview, no
        // snapshot and a fresh epoch. The refresh itself runs on the detection worker.
        self.begin_window_detection(&monitor.layout);

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
    pub(super) fn on_frame_ready(&mut self) {
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
    pub(super) fn apply_prepared(&mut self, prepared: capture_worker::PreparedFrame) {
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
        // Kept for the scroll entry: the window snapshot speaks virtual-desktop coordinates and the
        // overlay's selection is monitor-local, so the conversion needs this layout (`P7.05`).
        self.monitor_layout = Some(monitor.layout.clone());

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
        // Start the periodic hover re-validation now that there is something to hover
        // over; it is disarmed with the session.
        self.arm_hover_timer();
        eprintln!(
            "[snapclip][bench] stage=visible session={} prepare_elapsed_ms={}",
            self.session_id(),
            started_at.elapsed().as_millis()
        );
    }

    /// The single cancellation path used by Esc, right click, repeated F5, window
    /// destruction, display changes and device removal.
    ///
    /// Bumps the generation first: a frame the worker is still freezing becomes
    /// stale and is dropped on arrival, releasing its GPU references without the
    /// overlay ever waiting on the worker (docs/11 §3.3).
    pub(super) fn cancel(&mut self, reason: &str) {
        let session_id = self.session_id();
        let was_active = self.session.state().is_active();
        eprintln!(
            "[snapclip][capture] cancel session={} reason={} active={}",
            session_id, reason, was_active
        );
        // The scroll driver is joined here rather than left to `Drop`: this is the one path every
        // cancellation goes through (`docs/32` `P7.05`), and `Esc` on a running scroll session has
        // to stop the thread that is injecting into the user's desktop.
        self.scroll_entry_armed = false;
        self.stop_scroll_session(reason);
        self.worker.cancel();
        self.export_worker.cancel();
        self.current_generation = 0;
        self.release_session();
        if was_active {
            self.sink.on_cancelled(&session_id, reason);
        }
        self.sink.on_state(&session_id, CaptureState::Idle, None);
    }

    pub(super) fn release_session(&mut self) {
        eprintln!(
            "[snapclip][capture] release session={} state={:?}",
            self.session_id(),
            self.session.state()
        );
        // Report the window-detection metrics for this session before clearing them, so
        // every session leaves one line of evidence behind.
        let summary = self.metrics.summary_line();
        self.metrics.log_line(&summary, true);
        self.metrics.log_line(
            &format!("last deep target {}", describe_deep(self.deep_target.as_ref())),
            true,
        );
        // Forced, like the line above: "the precision top-up ran and decided nothing" is the
        // state a user reporting "elements inside this box are not recognized" is looking at,
        // and it used to be invisible unless the per-operation log was switched on.
        if let Some(precision) = self.metrics.last_precision() {
            self.metrics.log_line(&format!("last precision {precision}"), true);
        }
        self.metrics.reset();
        self.session.cancel();
        self.frozen = None;
        self.graphics_released = false;
        self.gesture.reset();
        self.set_preview_target(None);
        self.hover_target = None;
        self.dwell_armed = None;
        self.snapshot_request = None;
        self.confirm_request = None;
        self.snapshot.release();
        self.disarm_dwell();
        self.disarm_hover_timer();
        self.refine.reset();
        self.deep_target = None;
        self.deep_levels = None;
        self.hint = None;
        self.hint_taught = false;
        self.pending_downgrade = None;
        self.refinement_pending = None;
        self.disarm_refinement();
        // The session is over: nothing is waiting for an answer, and whatever the last query
        // left behind must not survive into the next one.
        self.refinement.retire();
        self.cursor_visible = false;
        // Stop the coalescing tick before releasing the renderer: a pending WM_TIMER
        // must not try to present into the graphics we are about to drop.
        self.disarm_render_tick();
        self.dirty = false;
        self.sampler.reset();
        self.magnifier_color_format = ColorFormat::default();
        self.magnifier_relative = false;
        self.magnifier_zoom = MagnifierConfig::ZOOM_DEFAULT;
        self.z_held = false;
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
    pub(super) fn confirm(&mut self) {
        // `F7`'s session: the same gesture that produces a screenshot produces a scroll session
        // when the session was started for scrolling (`docs/32` §4.4, `OQ-27`). One interception
        // point, because `Enter` and the synchronous confirm path both end up here.
        if self.scroll_entry_armed {
            self.begin_scroll_session();
            return;
        }
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
                magnifier_color_text: None,
                magnifier_relative: false,
                magnifier_zoom: self.magnifier_zoom,
                annotation_items: self.annotation_doc.items().to_vec(),
                annotation_selected_id: None,
                annotation_draft: None,
                // The exported pixels must never contain a hover or preview hint.
                hover_bounds: None,
                preview_bounds: None,
                chain_rings: Vec::new(),
                // …and neither a preview label nor the one-shot hint.
                preview_label: None,
                preview_is_window: false,
                capture_green: 0.0,
                preview_alpha: 1.0,
                level_badge: None,
                hint: None,
                // The panel is UI, so the export preview path gets none of it either.
                scroll_panel: None,
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
        let writer = self.writer.clone();
        let job = ExportJob {
            generation: 0,
            session_id: session_id.clone(),
            prepared,
            dpi,
            monitor_device_name: None,
            notify_thread: unsafe { GetCurrentThreadId() },
            executor: Box::new(move |job: &ExportJob| {
                writer.write(
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
    pub(super) fn on_export_ready(&mut self) {
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

    pub(super) fn finish_session(&mut self) {
        self.release_session();
    }

    pub(super) fn fail(&mut self, session_id: Option<&str>, error: CaptureError, provider: &str) {
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
    pub(super) fn request_color_sample(&mut self) {
        if self.graphics_released || !self.cursor_visible {
            return;
        }
        let Some(renderer) = self.renderer.as_mut() else { return; };
        let Some(frozen) = self.frozen.as_ref() else { return; };
        let Some(gpu_frame) = frozen.texture() else { return; };
        let texture = gpu_frame.texture.clone();
        let dpi = renderer.layout().dpi;
        let config = MagnifierConfig::with_zoom(self.magnifier_zoom).scaled(dpi);
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
    pub(super) fn poll_color_sample(&mut self) {
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
}
