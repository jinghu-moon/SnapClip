//! Session lifecycle: start, frame, confirm, export, finish, cancel (docs/23 D3, slice 4).
//!
//! Split out of `overlay.rs`: everything that gives a capture session its shape — the state
//! it publishes, the worker hand-offs, the single cancellation path and the colour sampler.
//! `pub(super)` because the parent module's window procedure and the input handlers call in.

use crate::windows::win::window::WindowRegion;
use super::*;

impl OverlayController {
    pub(super) fn session_id(&self) -> String {
        self.session.id().to_string()
    }

    pub(super) fn layout(&self) -> Option<MonitorLayout> {
        self.renderer
            .as_ref()
            .map(|renderer| renderer.layout().clone())
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

    /// `F7`: arm the scroll intent and open the ordinary selection UI (`docs/33`). Confirming the
    /// selection then starts a scroll session instead of producing a screenshot.
    pub(super) fn start_scroll_entry(&mut self) {
        self.start_session();
        self.scroll_entry_armed = true;
        // Enter now means "start scrolling", and the guide says so.
        self.guide_groups = self.guide_groups_for(true);
        eprintln!("[snapclip][capture] scroll entry armed (F7): select a region and confirm");
    }

    /// The shortcut guide for this session's keymap; empty when the guide is turned off.
    fn guide_groups_for(&self, scroll_entry: bool) -> Arc<[GuideGroup]> {
        if !self.preferences.shortcut_guide {
            return Arc::from(Vec::new());
        }
        self.preferences
            .keymap
            .guide(Context::Pick, scroll_entry)
            .into()
    }

    /// `F5`: submit a capture request and enter `Preparing`.
    ///
    /// Nothing here waits on WGC/BitBlt: the freeze happens on the worker
    /// thread and the message pump keeps running, so `Esc` cancels while the
    /// screen is still being captured (docs/11 §2.2/§3.3).
    /// 面板选了延时：收起浮层，等画面变完再抓（`REGION_DELAY_TIMER_ID`）。
    ///
    /// 抓的是等待**之后**那一帧，所以这里结束当前会话并叫一次新的；区域存在 `pending_region` 里，
    /// 由那次会话在能接受选区时放上去（docs/23 §14.54）。
    /// 面板确认了延时：**浮层留在屏幕上**，在选区正中倒数，到点再抓（docs/23 §14.55.9）。
    ///
    /// 与旧行为的差别：以前是"收起浮层干等"，用户看不到还剩几秒；现在是"看得见地等"。
    pub(super) fn arm_countdown(&mut self, seconds: u32) {
        self.countdown = Some(Countdown::start(Instant::now(), seconds));
        // 基线与差值是这条日志的意义所在：它回答"从开始等到退出，有没有画过普通帧"
        // （docs/23 §14.55.9.5）。
        self.frames_normal_at_countdown = self.frames_normal;
        // 结构闸生效，并记下两个新读数（可见普通帧 / 被压掉的普通帧）的基线。
        self.countdown_hud = true;
        self.frames_normal_visible_at_countdown = self.frames_normal_visible;
        self.frames_suppressed_at_countdown = self.frames_suppressed;
        // 新的一次等待：不带着上一次可能残留的拖拽手势。
        self.countdown_dragging = false;
        self.countdown_handle = None;
        // 进入倒计时的顺序是数据（`countdown_enter_steps`）。第一条最关键：**先把屏幕上的
        // 普通画面同步换成 HUD 帧**，再收窄窗口区域、再叠遮罩层 —— 反过来会在收窄区域的那一刻
        // 把屏幕上旧遮罩切掉一半，而新遮罩还没出现，用户看到的就是"面板消失时选区外闪一下"
        // （docs/23 §14.55.9.9）。
        self.enter_countdown();
        unsafe { SetTimer(self.window, COUNTDOWN_TIMER_ID, COUNTDOWN_TICK_MS, None) };
        self.invalidate();
    }

    /// 按 [`countdown_enter_steps`] 进入倒计时。
    pub(super) fn enter_countdown(&mut self) {
        for step in countdown_enter_steps() {
            match *step {
                EnterStep::PaintHud => self.paint_now(),
                EnterStep::NarrowRegion => self.set_window_region(&countdown_window_pieces(
                    self.session.selection(),
                    self.label_badge,
                    self.session.dpi(),
                    i32::try_from(self.corner_radius_px).unwrap_or(0),
                )),
                EnterStep::ShowDim => self.show_countdown_dim(),
                EnterStep::RaiseOverlay => self.raise_overlay_above_dim(),
            }
        }
    }

    /// 把主浮层提回遮罩层之上：两者都是 TOPMOST，不提的话框环的外半边会被遮罩压暗。
    pub(super) fn raise_overlay_above_dim(&mut self) {
        unsafe {
            SetWindowPos(
                self.window,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE
                    | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                    | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOSIZE,
            )
        };
    }

    /// 倒计时遮罩层的内容分辨率除数：**1**（全分辨率）。
    ///
    /// 实测（3840×2160、真 GPU）：8.8 ms 渲染 + 读回，1/2 是 2.9 ms、1/4 是 1.2 ms ——
    /// 但低分辨率内容要靠 GDI 放大，而 `StretchBlt` 会把 32bpp DIB 的 alpha 清零、
    /// `AlphaBlend` 会静默失败，两者都让整块窗口变透明。既然全分辨率远低于 50 ms 门限，
    /// 就不为省那几毫秒去换一个会丢 alpha 的放大步骤（docs/23 §14.55.9.11）。
    const DIM_DIVISOR: u32 = 1;

    /// 显示（或重设）倒计时遮罩层：铺满显示器、选区开洞。
    ///
    /// **不改变 Z 序** —— 提层是 [`EnterStep::RaiseOverlay`]，顺序由数据钉住（docs/23 §14.55.9.9）。
    pub(super) fn show_countdown_dim(&mut self) {
        let Some(monitor) = self.renderer.as_ref().map(|renderer| renderer.layout().bounds) else {
            return;
        };
        let selection = self.session.selection();
        // 内容优先用「冻帧 + 模糊 + 压暗」那一帧：只给纯色会比正常截图态少一层模糊
        // （docs/23 §14.55.9.11）。渲染与读回都在帧之外做，按 1/DIM_DIVISOR 分辨率降级；
        // 拿不到就退回纯色 —— 少一层模糊也比阻断倒计时强。
        let started = Instant::now();
        let content = self
            .renderer
            .as_mut()
            .and_then(|renderer| match renderer.countdown_dim_source(Self::DIM_DIVISOR) {
                Ok(source) => Some(source),
                Err(error) => {
                    eprintln!("[snapclip][capture] countdown dim source: {error}");
                    None
                }
            });
        let source_ms = started.elapsed();
        let started = Instant::now();
        match self.dim_layer.show(monitor, selection, self.corner_radius_px as i32, content.as_ref()) {
            Ok(()) => eprintln!(
                "[snapclip][capture] countdown dim shown: source_ms={} paint_ms={} {}",
                source_ms.as_millis(),
                started.elapsed().as_millis(),
                self.dim_layer.diagnose()
            ),
            Err(error) => eprintln!("[snapclip][capture] countdown dim layer: {error}"),
        }
    }

    /// 拖动改框时挪洞（不重填、不重新提交层内容）。
    pub(super) fn update_countdown_dim(&mut self) {
        if self.countdown.is_none() {
            return;
        }
        let Some(monitor) = self.renderer.as_ref().map(|renderer| renderer.layout().bounds) else {
            return;
        };
        let selection = self.session.selection();
        if let Err(error) = self.dim_layer.update(monitor, selection, self.corner_radius_px as i32) {
            eprintln!("[snapclip][capture] countdown dim update: {error}");
        }
    }



    /// 倒计时里拖边框改大小之后，两样东西必须一起跟随新选区：
    ///
    /// * 到点贴图用的区域 —— 不跟随的话，用户改了框也白改，贴的还是点「确定」那一刻的区域；
    /// * HUD 的窗口区域 —— 不跟随的话，框线会跑到窗口外面被裁掉。
    pub(super) fn countdown_after_resize(
        selection: Rect,
        origin: Point,
        badge: Option<Rect>,
        dpi: u32,
    ) -> (Option<crate::region::Region>, Vec<WindowRegion>) {
        let pin = (!selection.is_empty()).then(|| crate::region::Region::from_rect(selection, origin));
        (pin, countdown_window_pieces(selection, badge, dpi, 0))
    }

    /// 倒计时每秒一跳：还在数就重绘；到 0 收浮层、抓"等待之后那一帧"并交给贴图（docs/23 §14.55.9）。
    pub(super) fn on_countdown_tick(&mut self) {
        let Some(countdown) = self.countdown else {
            return;
        };
        match countdown.step(Instant::now()) {
            CountdownStep::Show(_) => self.invalidate(),
            CountdownStep::Fire => {
                // 顺序由 `countdown_exit_steps` 钉住（docs/23 §14.55.9.5）：**先让窗口不可见**，
                // 再退出 HUD 渲染模式、还原区域，并且**不请求重绘**——那一帧普通画面（冻结画面 +
                // 遮罩）就是用户看到的"边框外侧一圈暗灰"。
                self.leave_countdown(CountdownExit::Fire);
                self.on_region_delay();
            }
        }
    }

    /// 还剩几秒（给视图画数字用）；没有倒计时就是 `None`。
    pub(super) fn countdown_seconds_left(&self) -> Option<u32> {
        let now = Instant::now();
        match self.countdown?.step(now) {
            CountdownStep::Show(seconds) => Some(seconds),
            CountdownStep::Fire => None,
        }
    }

    /// 倒计时收尾的**状态部分**：停表、把鼠标还回来、清掉倒计时；返回此前是否在跑（幂等）。
    ///
    /// **不碰窗口区域** —— 区域由调用方按"窗口此刻还可见吗"决定先后：到点那条路必须**先隐藏**
    /// 再还原（否则可见的窗口会按普通帧画一次，就是那一下灰底，docs/23 §14.55.9.4）；而取消那条路
    /// 窗口本来就该继续显示普通画面，直接还原再重绘即可。
    pub(super) fn finish_countdown(&mut self) -> bool {
        if self.countdown.take().is_none() {
            return false;
        }
        unsafe { KillTimer(self.window, COUNTDOWN_TIMER_ID) };
        // 遮罩暗色层与倒计时同生共死：三个出口都经这里（docs/23 §14.55.9.7）。
        // 正在拖边框时按 Esc：先把鼠标还回来（docs/23 §14.55.9.3）。
        self.release_countdown_capture();
        // 解闸。到点/失败这两个出口走 `ExitStep::Hide` 在前，所以此时窗口已不可见；
        // Esc 取消这条窗口要继续可见，先解闸再重画才是对的（docs/23 §14.55.9.6）。
        self.countdown_hud = false;
        true
    }

    /// 走一遍"退出倒计时"的顺序，顺序本身在 [`countdown_exit_steps`] 里。
    ///
    /// 顺带把这一帧的统计打进日志：倒计时过渡时是否画过**普通帧**，从日志就能判断，不必靠人眼
    /// 反复确认（docs/23 §14.55.9.5）。
    pub(super) fn leave_countdown(&mut self, exit: CountdownExit) {
        for step in countdown_exit_steps(exit) {
            match step {
                ExitStep::Hide => self.hide_overlay(),
                ExitStep::HideDim => self.dim_layer.hide(),
                ExitStep::ClearCountdown => {
                    self.finish_countdown();
                }
                ExitStep::RestoreRegion => self.set_window_region(&[]),
                ExitStep::Repaint => self.invalidate(),
            }
        }
        eprintln!(
            "[snapclip][capture] countdown exit={exit:?} normal_frames_since_start={} normal_frames_visible_since_start={} suppressed_frames_since_start={} hud_frames_since_start={}",
            self.frames_normal - self.frames_normal_at_countdown,
            self.frames_normal_visible - self.frames_normal_visible_at_countdown,
            self.frames_suppressed - self.frames_suppressed_at_countdown,
            self.frames_hud
        );
    }

    /// 延时到点：结束等待，开一次带着那块区域的会话。
    pub(super) fn on_region_delay(&mut self) {
        if self.pending_region.is_none() {
            // 等待期间用户按了 Esc：没有要截的区域，也不该凭空开一次会话。
            self.delay_destination = None;
            return;
        }
        // 这一次启动是延时自己发起的，`start_session` 因此保留待截区域与"到点贴图"。
        self.delay_in_flight = true;
        self.start_session();
        self.delay_in_flight = false;
    }

    pub(super) fn start_session(&mut self) {
        let started_at = Instant::now();
        eprintln!("[snapclip][capture] starting session from hotkey/command");
        // 延时留下的"待截区域/到点贴图"只归延时自己发起的会话。用户手动起的会话（F5/F7、
        // 托盘）把它们清掉：等待期间用户改了主意再来一次，不该被上一次的意图劫持。
        if !self.delay_in_flight {
            self.pending_region = None;
            self.delay_destination = None;
        }
        if self.session.state().is_active() || self.scroll.is_some() {
            // A repeated hotkey press restarts rather than stacking sessions;
            // `cancel` bumps the generation so the old request's result dies.
            self.cancel("hotkey-restart");
        }
        // `F5` means a screenshot; `start_scroll_entry` re-arms the scroll intent after this.
        self.scroll_entry_armed = false;
        // The preferences are read once per session, so a change in the settings never rearranges
        // the session the user is in.
        if let Ok(shared) = self.shared.lock() {
            self.preferences = shared.preferences.clone();
        }
        self.loupe_hints = self.preferences.keymap.loupe_hints().into();
        self.guide_groups = self.guide_groups_for(false);
        self.guide = GuideFade::default();
        self.unit_slide = SegmentSlide::default();
        self.delay_slide = SegmentSlide::default();

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
        // Kept for the scroll entry, which outlives the capture session's geometry.
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
        // 延时截图：到点之后开的这次会话把当时那块区域直接放上去（docs/23 §14.54）。
        // 放在 `overlay_ready` 之后：`snap_to` 只在会话能接受选区时才生效。
        if let Some(region) = self.pending_region.take() {
            let destination = std::mem::take(&mut self.delay_destination);
            let accepted = self.session.snap_to(region.to_rect(self.monitor_origin()));
            if !accepted {
                eprintln!("[snapclip][capture] delayed region refused by the session");
            }
            self.publish_state();
            // 到点之后按面板里那个去向执行：
            //   * 贴图：把这一帧的区域像素放进 `F6` 取图的那个槽位，再请外壳去贴（外壳投的是
            //     同一条 `WM_HOTKEY`，所以贴图代码一行都没新写，docs/23 §14.55.8）。
            //   * 复制：走既有 `confirm()` 那条路（复制 + 入历史），不另造一份。
            // 两条都直接 return：**不 paint、不显示浮层**——用户正在别的窗口里操作，闪一下就是打扰。
            match delay_outcome(destination, accepted) {
                DelayOutcome::Pin => {
                    self.publish_selection("panel-delay-pin");
                    self.sink.on_pin_requested(&self.session_id());
                    self.finish_session();
                    return;
                }
                DelayOutcome::Copy => {
                    self.confirm();
                    return;
                }
                DelayOutcome::Show => {}
                DelayOutcome::Refused => {
                    // 失败要说出来，不能静默：没有合法区域就没有可复制/可贴的东西。
                    self.fail(
                        None,
                        CaptureError::InvalidState("delayed region refused by the session".into()),
                        provider,
                    );
                    return;
                }
            }
        }
        // Paint the new frame while the HWND is still hidden. DirectComposition
        // retains the previous swap-chain contents, so showing first would expose
        // the previous session's selection for one compositor frame. This first paint
        // is synchronous (not coalesced) precisely so it lands before `show_overlay`.
        self.paint_now();
        self.show_overlay(&monitor.layout);
        // Start the periodic hover re-validation now that there is something to hover
        // over; it is disarmed with the session.
        self.arm_hover_timer();
        // The guide fades in on the render clock.
        if !self.guide_groups.is_empty() {
            self.invalidate();
        }
        eprintln!(
            "[snapclip][bench] stage=visible session={} prepare_elapsed_ms={}",
            self.session_id(),
            started_at.elapsed().as_millis()
        );
    }

    /// Read back the committed selection and leave it where a consumer outside capture
    /// can take it: `F6` pins "the picture I just selected", and this is the moment that
    /// picture exists.
    ///
    /// **It cannot move to `confirm`.** The frozen frame — the only copy of these pixels —
    /// dies with the session (`release_session` drops it, taking the GPU texture with it),
    /// and the user's gesture is select-then-pin, not select-then-save. Leaving the readback
    /// at `confirm` is what made `F6` answer "还没有可贴的图" on a selection the user had
    /// just dragged: nothing had been exported, so nothing had ever been read back.
    ///
    /// The cost is a region-sized readback per committed gesture (~1 ms for a 1441x900
    /// selection on the measured path). It is paid once per drag, not per mouse move, and
    /// `F9` re-reads the same region anyway — `FrozenFrame` caches its own pixels, so the
    /// second read is a copy, not another GPU transfer.
    ///
    /// `reason` only names the gesture in the log, so a slow readback can be attributed.
    pub(super) fn publish_selection(&mut self, reason: &str) {
        let selection = self.session.selection();
        if selection.is_empty() {
            return;
        }
        let Some(frozen) = self.frozen.as_ref() else {
            return;
        };
        let started_at = Instant::now();
        match self
            .service
            .prepare_selection(&frozen.frame, selection, &FrozenFramePixels::new(frozen))
        {
            Ok(prepared) => {
                let readback_ms = started_at.elapsed().as_millis();
                eprintln!(
                    "[snapclip][bench] stage=selection_kept reason={} session={} size={}x{} readback_ms={}",
                    reason,
                    self.session_id(),
                    prepared.region.width(),
                    prepared.region.height(),
                    readback_ms
                );
                // 半径在**唯一的发布收口**里写入：每条发布路径（面板确认走 Pin、延时走 Pin、
                // 拖拽后发布）都自动带上，不再由调用点各填一次（docs/23 §14.55.21）。
                let mut prepared = prepared;
                prepared.corner_radius = self.corner_radius_px as i32;
                if let Ok(mut shared) = self.shared.lock() {
                    shared.selection = Some(Arc::new(prepared));
                }
            }
            // Not fatal: the pinned copy is a convenience, and the gesture itself
            // (export, annotate, cancel) has not failed. `F6` reports its own miss.
            Err(error) => eprintln!(
                "[snapclip][capture] selection readback failed session={} reason={} error={}",
                self.session_id(),
                reason,
                error
            ),
        }
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
        self.scroll_entry_armed = false;
        // A running scroll session is discarded: dropping its worker stops and joins the thread,
        // and nothing is written (`docs/33`).
        if let Some(run) = self.end_scroll() {
            eprintln!(
                "[snapclip][scroll] cancelled session={} reason={reason}",
                run.session_id
            );
            self.sink.on_cancelled(&run.session_id, reason);
            drop(run);
        }
        self.worker.cancel();
        // The export worker is cancelled only when **this** capture session has an export in flight.
        // A scroll export outlives the capture session (the hand-over ended it), so an `Esc` after
        // the scroll session finished must not delete the file it is writing.
        if self.session.state() == CaptureState::Exporting {
            self.export_worker.cancel();
            self.current_generation = 0;
        }
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
            &format!(
                "last deep target {}",
                describe_deep(self.deep_target.as_ref())
            ),
            true,
        );
        // Forced, like the line above: "the precision top-up ran and decided nothing" is the
        // state a user reporting "elements inside this box are not recognized" is looking at,
        // and it used to be invisible unless the per-operation log was switched on.
        if let Some(precision) = self.metrics.last_precision() {
            self.metrics
                .log_line(&format!("last precision {precision}"), true);
        }
        self.metrics.reset();
        self.session.cancel();
        self.frozen = None;
        self.graphics_released = false;
        self.gesture.reset();
        // 会话结束（取消/失败/完成）时若还在拖倒计时边框，捕获必须还回去。
        self.release_countdown_capture();
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
        self.precision_feedback_until = None;
        // Stop the coalescing tick before releasing the renderer: a pending WM_TIMER
        // must not try to present into the graphics we are about to drop.
        self.disarm_render_tick();
        self.dirty = false;
        self.sampler.reset();
        self.magnifier_color_format = ColorFormat::default();
        self.magnifier_relative = false;
        self.magnifier_zoom = MagnifierConfig::ZOOM_DEFAULT;
        self.held = None;
        self.guide = GuideFade::default();
        self.unit_slide = SegmentSlide::default();
        self.delay_slide = SegmentSlide::default();
        self.label_badge = None;
        self.chrome_swallowed_press = false;
        // 面板属于它所在的那次会话：下一个会话要么自己打开它，要么没有。
        self.region_panel = None;
        // 会话结束：面板没了，IME 必须还给这个窗口（docs/23 §14.55）。
        self.restore_ime_for_panel();
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

    /// The export executor: one `SelectionPixels` written in one call, for a screenshot and for
    /// a scroll session's long image alike.
    pub(super) fn export_executor(writer: Arc<dyn ArtifactWriter>) -> ExportExecutor {
        Box::new(move |job: &ExportJob| {
            writer.write(
                &job.session_id,
                &job.prepared,
                job.dpi,
                job.monitor_device_name.clone(),
            )
        })
    }

    /// `Enter`: read the confirmed selection back on this thread, then hand encode +
    /// write to the export worker so the message pump keeps answering `Esc` and
    /// `WM_PAINT` while a large PNG is produced (docs/11 §Phase 3).
    pub(super) fn confirm(&mut self) {
        // `F7`'s session: the same gesture that produces a screenshot starts a scroll session
        // (`docs/33`). One interception point, because every confirm path ends up here.
        if self.scroll_entry_armed {
            self.begin_scroll_session();
            return;
        }
        // `确定` 必须按**当前去向**分派：去向 = 贴图时走贴图链路、不提交导出；
        // 去向 = 复制时才走导出/复制。以前 `confirm()` 无条件 begin_export()，
        // 于是"贴图"永远变成"复制"（docs/23 §14.55.20 的 Bug 3，真机日志里没有一行 [pin]）。
        let destination = self
            .region_panel
            .as_ref()
            .map(|panel| panel.destination())
            .unwrap_or(Destination::Copy);
        eprintln!(
            "[snapclip][capture] confirm session={} destination={:?}",
            self.session_id(),
            destination
        );
        if confirm_action(destination) == ConfirmAction::Pin {
            self.publish_selection("panel-confirm-pin");
            self.sink.on_pin_requested(&self.session_id());
            self.finish_session();
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
                show_handles: false,
                // 导出像素与框的圆角无关：遮罩在 `round_corners` 里统一做一次。
                corner_radius: 0,
                show_size_label: false,
                // The label is not drawn on the export path at all; the unit is carried through
                // unchanged so the export state stays a faithful copy of the session's.
                label: self.preferences.label,
                label_hovered: false,
                region_panel: None,
                show_magnifier: false,
                precision_feedback: false,
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
                scroll: None,
                // …nor the guide, nor the blur outside the selection.
                mask_blur: false,
                loupe_hints: self.loupe_hints.clone(),
                guide: None,
                // 倒计时是给用户的提示，不进导出像素（docs/23 §14.55.9）。
                countdown: None,
            };
            let Some(renderer) = self.renderer.as_mut() else {
                self.cancel("annotation-export-without-renderer");
                return;
            };
            renderer
                .render_export(&export_state, None)
                .map(|bgra| SelectionPixels {
                    corner_radius: self.corner_radius_px as i32,
                    // The annotated export is still this monitor's frame, so it keeps a real
                    // screen position; no layout means the overlay is gone and the honest
                    // answer is "no position", not "the primary monitor".
                    origin: self
                        .layout()
                        .map(|layout| Point::new(layout.bounds.left, layout.bounds.top)),
                    frame: frozen.frame.clone(),
                    region: clipped,
                    bgra,
                })
                .map_err(CaptureError::RenderFailed)
        };
        let readback_ms = started_at.elapsed().as_millis();
        let mut prepared = match prepared {
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
        // Two producers feed this point (the direct frozen-frame readback and the replayed
        // annotated export), and every consumer forks off the same `prepared` below — the pin
        // slot and the export worker alike. Rounding here therefore lands on both destinations
        // once, instead of once per path (docs/23 §14.55.17).
        crate::artifact::round_corners(
            &mut prepared.bgra,
            prepared.region.width() as u32,
            prepared.region.height() as u32,
            self.corner_radius_px,
        );
        let export_width = prepared.region.width();
        let export_height = prepared.region.height();
        // Hand the pixels to whoever pins "the thing I just captured" (`F6`). The gesture that
        // committed this selection already published them (`publish_selection`); this refreshed
        // copy is the one case that differs — annotations were rendered into it above — and it
        // is the same buffer by reference that the export worker is about to write, so the
        // hand-off costs nothing. The slot outlives the session on purpose: the frozen texture
        // dies with it, and the PNG the export writes is a different representation.
        if let Ok(mut shared) = self.shared.lock() {
            shared.selection = Some(Arc::new(prepared.clone()));
        }
        // The overlay is frozen at the confirmed selection until the export lands, so
        // cursor-follow repaints can neither race nor waste the hand-off.
        self.graphics_released = true;
        let job = ExportJob {
            generation: 0,
            session_id: session_id.clone(),
            prepared,
            dpi,
            monitor_device_name: None,
            notify_thread: unsafe { GetCurrentThreadId() },
            executor: Self::export_executor(self.writer.clone()),
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
    /// [`export_worker::EXPORT_READY_MESSAGE`].
    ///
    /// Two owners: the capture session in `Exporting`, or a scroll session's image
    /// (`scroll_export`), which outlives the capture session it came from. Anything else is a stale
    /// result (an `Esc` cancelled it, and the worker already deleted the file).
    pub(super) fn on_export_ready(&mut self) {
        let Some(ready) = self.export_worker.take_ready() else {
            return;
        };
        if let Some(scroll_id) = self.scroll_export.take() {
            self.on_scroll_export_ready(&scroll_id, ready);
            return;
        }
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
                self.sink
                    .on_failed(Some(&session_id), &failure.error, stage);
                self.session.fail();
                self.finish_session();
                self.sink.on_state(&session_id, CaptureState::Idle, None);
            }
        }
    }

    /// A scroll session's export finished: publish it (`docs/33`). The desktop and clipboard copies
    /// already ran on the export thread ([`Self::scroll_executor`]).
    fn on_scroll_export_ready(
        &mut self,
        session_id: &str,
        ready: Result<export_worker::CompletedExport, export_worker::ExportFailure>,
    ) {
        match ready {
            Ok(completed) => {
                let artifact = completed.artifact;
                eprintln!(
                    "[snapclip][scroll] artifact ready session={session_id} size={}x{}",
                    artifact.width, artifact.height
                );
                self.sink.on_completed(&artifact);
            }
            Err(failure) => {
                eprintln!(
                    "[snapclip][scroll] export failed session={session_id} stage={} error={}",
                    failure.stage, failure.error
                );
                self.sink
                    .on_failed(Some(session_id), &failure.error, failure.stage);
            }
        }
        self.sink.on_state(session_id, CaptureState::Idle, None);
    }

    pub(super) fn finish_session(&mut self) {
        self.release_session();
    }

    pub(super) fn fail(&mut self, session_id: Option<&str>, error: CaptureError, provider: &str) {
        // 失败之后不留延时留下的意图：下一次会话从干净状态开始。
        self.pending_region = None;
        self.delay_destination = None;
        // 倒计时也要一起收：同样先退出可见状态，再退渲染模式（docs/23 §14.55.9.5）。
        if self.countdown.is_some() {
            self.leave_countdown(CountdownExit::Fail);
        }
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
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let Some(frozen) = self.frozen.as_ref() else {
            return;
        };
        let Some(gpu_frame) = frozen.texture() else {
            return;
        };
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
        let Some(slot) = self.sample_slot.take() else {
            return;
        };
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
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

/// 延时到点后，这一次会话该怎么收场。
///
/// 抽成纯函数是为了能被测试盯住：这条路径要真跑起来得有一帧真实画面与一次会话，
/// 而"到底该贴图、该显示、还是该报错"本身只是一个判断（docs/23 §14.55.8）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DelayOutcome {
    /// 不是延时发起的会话：照常显示浮层等用户确认。
    Show,
    /// 延时到点、区域被接受、去向是**复制**：走既有 `confirm()`。
    Copy,
    /// 延时到点、区域被接受、去向是**贴图**：把图放进贴图槽位、请外壳贴出去。
    Pin,
    /// 延时到点但区域被会话拒绝：必须报错，不能静默什么都不做。
    Refused,
}

/// `destination` = 这次会话是延时发起的，以及面板里选的那个去向；`accepted` = 会话接受了区域。
///
/// 去向是这张截图的规格之一：延时不再写死贴图，`复制` 与 `贴图` 各走各自现成的那条路
/// （docs/23 §14.55.15）。
pub(super) fn delay_outcome(
    destination: Option<Destination>,
    accepted: bool,
) -> DelayOutcome {
    match (destination, accepted) {
        (None, _) => DelayOutcome::Show,
        (Some(_), false) => DelayOutcome::Refused,
        (Some(Destination::Copy), true) => DelayOutcome::Copy,
        (Some(Destination::Pin), true) => DelayOutcome::Pin,
    }
}

/// `确定` 按去向分派到哪里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConfirmAction {
    Export,
    Pin,
}

pub(super) fn confirm_action(destination: Destination) -> ConfirmAction {
    match destination {
        Destination::Copy => ConfirmAction::Export,
        Destination::Pin => ConfirmAction::Pin,
    }
}

#[cfg(test)]
mod confirm_action_tests {
    use super::{ConfirmAction, Destination};

    #[test]
    fn pin_does_not_export_and_copy_does_not_pin() {
        assert_eq!(
            super::confirm_action(Destination::Pin),
            ConfirmAction::Pin,
            "去向=贴图不得走导出"
        );
        assert_eq!(
            super::confirm_action(Destination::Copy),
            ConfirmAction::Export,
            "去向=复制才走导出"
        );
    }
}

#[cfg(test)]
mod delay_outcome_tests {
    use super::{DelayOutcome, delay_outcome};
    use crate::region::Destination;

    #[test]
    fn a_normal_session_just_shows_the_overlay() {
        assert_eq!(delay_outcome(None, true), DelayOutcome::Show);
        assert_eq!(delay_outcome(None, false), DelayOutcome::Show);
    }

    #[test]
    fn a_delayed_session_follows_the_destination_it_was_armed_with() {
        assert_eq!(
            delay_outcome(Some(Destination::Pin), true),
            DelayOutcome::Pin
        );
        assert_eq!(
            delay_outcome(Some(Destination::Copy), true),
            DelayOutcome::Copy
        );
    }

    #[test]
    fn a_delayed_session_whose_region_was_refused_reports_instead_of_silence() {
        assert_eq!(
            delay_outcome(Some(Destination::Pin), false),
            DelayOutcome::Refused
        );
        assert_eq!(
            delay_outcome(Some(Destination::Copy), false),
            DelayOutcome::Refused
        );
    }
}

/// 倒计时期间窗口应收窄到的矩形：就是选区本身。
///
/// 空选区返回 `None`（还原整块），否则把一个退化矩形交给 `SetWindowRgn` 会让窗口消失。
pub(super) fn countdown_window_pieces(
    selection: Rect,
    badge: Option<Rect>,
    dpi: u32,
    radius: i32,
) -> Vec<WindowRegion> {
    if selection.is_empty() {
        return Vec::new();
    }
    let scale = dpi.max(96) as f32 / 96.0;
    // 选框环：外框外扩、内框内缩；环宽要够放框线与手柄圆点。
    let reach = (COUNTDOWN_RING_REACH_DIP * scale).round().max(1.0) as i32;
    let outer = Rect::new(
        selection.left - reach,
        selection.top - reach,
        selection.right + reach,
        selection.bottom + reach,
    );
    let inner = Rect::new(
        selection.left + reach,
        selection.top + reach,
        selection.right - reach,
        selection.bottom - reach,
    );
    // 圆角框下矩形环会与框线不贴合：半径 > 0 时改用圆角环，外弧与框同半径、向内加一圈环宽。
    // 半径 0 走原来的矩形环（与改动前逐字节一致）。
    let hole = if inner.width() > 0 && inner.height() > 0 {
        inner
    } else {
        Rect::new(0, 0, 0, 0)
    };
    let mut pieces = vec![if radius > 0 {
        WindowRegion::RoundRing {
            outer,
            inner: hole,
            radius: radius + reach,
        }
    } else {
        WindowRegion::Ring {
            outer,
            inner: hole,
        }
    }];
    // 角标在选区之外，单独留一块；否则倒计时期间它会消失。
    if let Some(badge) = badge.filter(|badge| !badge.is_empty()) {
        pieces.push(WindowRegion::Rect(badge));
    }
    // 倒计时数字所在的小方框。
    let side = (COUNTDOWN_BOX_DIP * scale).round() as i32;
    let centre = selection.center();
    pieces.push(WindowRegion::Rect(Rect::from_origin_size(
        Point::new(centre.x - side / 2, centre.y - side / 2),
        side,
        side,
    )));
    pieces
}

/// 倒计时那块小方框的边长（dip）：够放下 96 dip 的数字并留出余量。
const COUNTDOWN_BOX_DIP: f32 = 168.0;
/// 选框环的环宽（dip，单侧）：要盖住框线并让手柄圆点完整。
const COUNTDOWN_RING_REACH_DIP: f32 = 8.0;


/// 倒计时怎么退出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CountdownExit {
    /// 到点：窗口先不可见，再退出 HUD、还原区域；**不请求重绘**。
    Fire,
    /// Esc 取消：窗口要继续可见，所以不清可见性，最后重画一帧普通画面。
    Cancel,
    /// 失败：和到点一样，先退出可见状态（失败后浮层不该留在屏幕上）。
    Fail,
}

/// 进入倒计时的一步。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EnterStep {
    /// 先**同步**画一帧 HUD（透明背景 + 框线/角标/数字 + 倒计时），把屏幕上那张普通画面换掉。
    PaintHud,
    /// 再把窗口区域收窄到框环/角标/倒计时框：此时屏幕上已经没有旧画面可被切掉了。
    NarrowRegion,
    /// **先**叠上遮罩层（选区外压暗；layered + transparent，点击可穿）。
    ShowDim,
    /// 紧接着把主浮层提回遮罩层之上：旧画面是不透明的，于是它立刻盖住刚出现的遮罩，
    /// 屏幕上回到"只有旧遮罩"。这一提必须排在换画面之前，否则两层暗色会叠着露出来。
    RaiseOverlay,
}

/// 进入倒计时的**顺序**：做成数据，测试才能钉住"交接时暗色不断档、也不叠层"。
///
/// 两种曾经的写法都被实测否掉：
/// * "收窄区域 → 显示遮罩 → 等合并 tick 重绘"：收窄时屏幕上还是旧画面，旧遮罩被切掉、新遮罩
///   还没出现 ⇒ 暗色闪一下（§14.55.9.9）；
/// * "先换 HUD 帧 → 再显示遮罩"：`PaintHud` 让主浮层在选区外变透明（旧遮罩立刻消失），而新遮罩
///   要到下一拍才出现 ⇒ 暗色**先消失再恢复**（§14.55.9.10，用户描述为"突然消失，再恢复"）。
///
/// 现在的顺序把"交接"压在同一段同步调用里，中间不放任何重绘：
/// 1. `ShowDim` —— 新遮罩就位（它显示时会被提到最顶，于是短暂压在旧画面之上）；
/// 2. `RaiseOverlay` —— 立刻把主浮层提回遮罩之上，不透明的旧画面重新盖住它 ⇒ 看不出变化；
/// 3. `PaintHud` —— 换成透明背景的 HUD 帧，**露出**早已就位的遮罩 ⇒ 暗色自始至终都在；
/// 4. `NarrowRegion` —— HUD 帧在选区外本来就透明，此刻收窄不会切掉任何可见内容。
pub(super) fn countdown_enter_steps() -> &'static [EnterStep] {
    &[
        EnterStep::ShowDim,
        EnterStep::RaiseOverlay,
        EnterStep::PaintHud,
        EnterStep::NarrowRegion,
    ]
}

/// 退出倒计时的一步。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExitStep {
    Hide,
    /// 收掉遮罩层。它必须在"退出可见状态"这一步里（和 `Hide` 一起），不能留到最后。
    HideDim,
    ClearCountdown,
    RestoreRegion,
    Repaint,
}

/// 退出倒计时的**顺序**：把它做成数据，才能被测试钉住"先隐藏、再退渲染模式"这个交错的先后。
///
/// 曾经的实现是"先清状态、后隐藏"：中间留出一瞬"窗口仍可见、倒计时已结束"，渲染器于是按普通帧
/// 画了一次（冻结画面 + 遮罩），用户看到的是框线外侧突然出现一圈暗灰（docs/23 §14.55.9.5）。
pub(super) fn countdown_exit_steps(exit: CountdownExit) -> &'static [ExitStep] {
    match exit {
        CountdownExit::Fire | CountdownExit::Fail => &[
            ExitStep::Hide,
            ExitStep::HideDim,
            ExitStep::ClearCountdown,
            ExitStep::RestoreRegion,
        ],
        CountdownExit::Cancel => &[
            ExitStep::HideDim,
            ExitStep::ClearCountdown,
            ExitStep::RestoreRegion,
            ExitStep::Repaint,
        ],
    }
}

/// 延时倒计时：面板收起后，在选区正中显示剩余秒数（docs/23 §14.55.9）。
///
/// 只吃一个时钟读数，所以能用假时刻测；`Option` 在控制器里承担"只触发一次"的语义
/// （`on_countdown_tick` 用 `take` 取走它，取走之后不可能再 Fire）。
#[derive(Debug, Clone, Copy)]
pub(super) struct Countdown {
    deadline: Instant,
}

impl Countdown {
    pub(super) fn start(now: Instant, seconds: u32) -> Self {
        Self {
            deadline: now + std::time::Duration::from_secs(seconds.max(1) as u64),
        }
    }

    pub(super) fn step(self, now: Instant) -> CountdownStep {
        countdown_step(self.deadline.saturating_duration_since(now))
    }
}

/// 一次倒计时推进的结论：还在数（显示这个数字），还是该抓帧了。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CountdownStep {
    Show(u32),
    Fire,
}

/// 剩余时长 → 显示的数字（**向上取整**：还剩 0.4 秒也要显示 1，不能出现"显示 0 却还在等"）。
pub(super) fn countdown_seconds(left: std::time::Duration) -> u32 {
    ((left.as_millis() as u64).saturating_add(999) / 1000) as u32
}

pub(super) fn countdown_step(left: std::time::Duration) -> CountdownStep {
    match countdown_seconds(left) {
        0 => CountdownStep::Fire,
        seconds => CountdownStep::Show(seconds),
    }
}


#[cfg(test)]
mod countdown_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_number_rounds_up_and_never_shows_zero_while_waiting() {
        assert_eq!(countdown_seconds(Duration::from_millis(3000)), 3);
        assert_eq!(countdown_seconds(Duration::from_millis(2999)), 3);
        assert_eq!(countdown_seconds(Duration::from_millis(1001)), 2);
        assert_eq!(countdown_seconds(Duration::from_millis(1)), 1);
        assert_eq!(countdown_step(Duration::ZERO), CountdownStep::Fire);
    }

    #[test]
    fn the_countdown_walks_down_to_fire_exactly_at_the_deadline() {
        let start = Instant::now();
        let countdown = Countdown::start(start, 3);
        assert_eq!(countdown.step(start), CountdownStep::Show(3));
        assert_eq!(countdown.step(start + Duration::from_millis(900)), CountdownStep::Show(3));
        assert_eq!(countdown.step(start + Duration::from_millis(1100)), CountdownStep::Show(2));
        assert_eq!(countdown.step(start + Duration::from_millis(2999)), CountdownStep::Show(1));
        assert_eq!(countdown.step(start + Duration::from_secs(3)), CountdownStep::Fire);
        assert_eq!(countdown.step(start + Duration::from_secs(9)), CountdownStep::Fire);
    }

    #[test]
    fn a_zero_second_countdown_still_shows_one_second() {
        // 面板允许的延时就那几个值，都不是 0；真给 0 也不该"立刻抓帧"而没有任何提示。
        let start = Instant::now();
        let countdown = Countdown::start(start, 0);
        assert_eq!(countdown.step(start), CountdownStep::Show(1));
    }
}
