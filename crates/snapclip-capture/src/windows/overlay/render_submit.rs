//! Painting, invalidation and the render tick (docs/23 D3 / T1.6.4).
//!
//! Split out of `overlay.rs`: the controller's presentation half — when the surface is
//! dirty, when a repaint is coalesced, and what a frame does when the device is lost.
//! These are `pub(super)` because the parent module owns the message loop that drives them.

use super::*;


use crate::geometry::region_panel_layout;
use crate::region::{preset_for, Region, RegionField};
use crate::units::LabelToggle;
use crate::windows::win::d2d::RegionPanelView;

/// The selection the overlay draws. Starting a scroll session ends the capture session, so its
/// selection is empty by design; the scroll session's selection is the one on screen thereafter.
pub(super) fn selection_for_render(
    session_selection: crate::geometry::Rect,
    scroll_selection: Option<crate::geometry::Rect>,
) -> crate::geometry::Rect {
    scroll_selection.unwrap_or(session_selection)
}

impl OverlayController {
    pub(super) fn precision_feedback_active(&self) -> bool {
        self.precision_feedback_until
            .is_some_and(|until| Instant::now() < until)
    }

    /// Mark the surface dirty for a discrete edit (undo / redo / delete / z-order /
    /// toolbar command) and coalesce a full repaint into the next tick.
    pub(super) fn invalidate_all(&mut self) {
        self.invalidate();
    }

    // ---- rendering -------------------------------------------------------

    /// Paint immediately with a full repaint.
    ///
    /// Used only for the synchronous first frame that must land before the window is
    /// shown (docs/11 §"隐藏状态完成一次完整绘制和 Present/Commit"). Interactive input
    /// goes through [`Self::invalidate`] instead so a burst of `WM_MOUSEMOVE`s collapses
    /// into one present per tick.
    pub(super) fn paint_now(&mut self) {
        self.disarm_render_tick();
        self.dirty = false;
        self.render();
    }

    /// Mark the surface dirty and coalesce a full repaint into the next render tick.
    ///
    /// `WM_MOUSEMOVE` and the drag handlers only update state and call this; the actual
    /// draw happens once in [`Self::on_render_tick`], so a fast pointer produces at most
    /// one present per `RENDER_TICK_MS` (docs/11 §"一个 tick 最多一次 Present/Commit").
    pub(super) fn invalidate(&mut self) {
        self.dirty = true;
        if self.renderer.is_none() {
            return;
        }
        self.arm_render_tick();
    }

    /// 光标闪烁：面板开着且有数字格聚焦时才需要这个低频计时器。
    ///
    /// 每帧问一次是安全的，因为**只在需要与否发生变化时**才真的 `SetTimer`/`KillTimer`——重复
    /// `SetTimer` 会把计数重置，那样计时器永远不会到点（与 `arm_render_tick` 同一个坑）。
    fn sync_caret_blink(&mut self) {
        let wanted = self
            .region_panel
            .as_ref()
            .is_some_and(|panel| panel.editing().is_some());
        if wanted == self.caret_timer_armed {
            return;
        }
        self.caret_timer_armed = wanted;
        if wanted {
            unsafe { SetTimer(self.window, CARET_TIMER_ID, CARET_BLINK_MS, None) };
        } else {
            unsafe { KillTimer(self.window, CARET_TIMER_ID) };
            // 停掉之后复位成可见：下次聚焦不该从"正好不可见"那半个周期开始。
            self.caret_visible = true;
        }
    }

    /// 闪烁计时器到点：翻一下光标的可见性。500 ms 一次，所以不必让渲染 tick 一直开着。
    pub(super) fn on_caret_blink(&mut self) {
        self.caret_visible = !self.caret_visible;
        self.invalidate();
    }

    pub(super) fn arm_render_tick(&mut self) {
        if self.render_armed {
            return;
        }
        // A window timer (not a thread timer): its `WM_TIMER` is posted to this queue
        // and coalesces, so repeated invalidations never stack timers.
        unsafe { SetTimer(self.window, RENDER_TIMER_ID, RENDER_TICK_MS, None) };
        self.render_armed = true;
    }

    pub(super) fn disarm_render_tick(&mut self) {
        if self.render_armed {
            unsafe { KillTimer(self.window, RENDER_TIMER_ID) };
            self.render_armed = false;
        }
    }

    /// The coalescing tick body: repaint everything accumulated since the last present.
    pub(super) fn on_render_tick(&mut self) {
        self.disarm_render_tick();
        // Always poll the pending GPU sample — may mark the info panel dirty.
        self.poll_color_sample();
        // Advance the preview transition on this same coalescing clock. While it runs it is
        // what keeps the tick alive, so the highlight eases into place instead of jumping
        // between control sizes (docs/18 §10).
        if self.advance_preview_animation() {
            self.dirty = true;
        }
        // …and the ③b chain fade, on the same clock: it is a series of quantised steps, so the tick
        // only marks dirty when the step actually changes (docs/21 §5.22).
        if self.advance_chain_fade() {
            self.dirty = true;
        }
        // …and the walk colour, which is the same kind of bounded ramp (docs/21 §5.24, A3).
        if self.advance_walk_activity() {
            self.dirty = true;
        }
        // …and the shortcut guide's fade, which steps aside when the pointer comes near.
        if self.advance_guide_fade() {
            self.dirty = true;
        }
        // …and the segmented controls' sliding blocks, which are the same kind of clock work.
        if self.advance_segment_slides() {
            self.dirty = true;
        }
        if self.precision_feedback_until.is_some_and(|until| Instant::now() >= until) {
            self.precision_feedback_until = None;
            self.dirty = true;
        }
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.render();
        if self.preview_transition.is_running(Instant::now())
            || self.chain_fade_running()
            || self.walk_activity_running()
            || self.guide.running()
            || self.unit_slide.running(Instant::now())
            || self.delay_slide.running(Instant::now())
            || self.precision_feedback_active()
            // …and the box's own appear ramp, which is a plain ramp on the same clock.
            || self.preview_appear() < 1.0
        {
            // Schedule the next frame: the animation or the fade is still moving.
            self.invalidate();
        }
    }

    /// Step the shortcut guide's fade toward where it should be; `true` when it changed.
    ///
    /// It shows while picking and steps aside when the pointer comes within
    /// [`GUIDE_AVOID_DIP`] of it, so it never covers what the user is about to select.
    /// The panel as it should be painted right now (docs/23 §14.54).
    ///
    /// The four numbers are read off the session's own selection — the panel keeps no second copy —
    /// while the unit, the lock and the delay are the panel's own state and the three switches come
    /// from the preferences this session started with.
    fn region_panel_view(&self) -> Option<RegionPanelView> {
        let panel = self.region_panel.as_ref()?;
        let layout = self.renderer.as_ref()?.layout();
        let origin = Point::new(layout.bounds.left, layout.bounds.top);
        let dpi = self.session.dpi();
        let selection = self.session.selection();
        let region = Region::from_rect(selection, origin);
        let problem = panel.problem(region);
        let rects = region_panel_layout(
            dpi as f32 / 96.0,
            layout.local_work_area(),
            selection,
            panel.menu().is_some(),
            self.segment_widths,
        );
        let hit = preset_for(region);
        Some(RegionPanelView {
            values: RegionField::ALL.map(|field| (field, panel.field_text(field, region, dpi))),
            invalid: RegionField::ALL.map(|field| {
                problem
                    .is_some_and(|problem| problem.fields().contains(&field))
            }),
            hover: self
                .cursor_visible
                .then(|| rects.target_at(self.cursor))
                .flatten(),
            focus: Some(panel.focus()),
            editing: panel.editing(),
            caret: panel.caret(),
            caret_visible: self.caret_visible,
            unit: panel.unit(),
            delay: panel.delay(),
            unit_slide: self.unit_slide.view(Instant::now()),
            delay_slide: self.delay_slide.view(Instant::now()),
            locked: panel.locked(),
            toggles: LabelToggle::ALL.map(|toggle| (toggle, toggle.on(self.preferences.label))),
            status: panel.status(region, dpi),
            preset: hit.map_or_else(|| "自定义".to_string(), |preset| preset.name.to_string()),
            preset_hit: hit.and_then(|preset| {
                crate::region::PRESETS
                    .iter()
                    .position(|item| item.name == preset.name)
            }),
            can_start: problem.is_none(),
            layout: rects,
        })
    }

    /// 推进两处分段器的平移；`true` 表示还在动（渲染 tick 要接着排帧）。
    ///
    /// 与 [`Self::advance_guide_fade`] 同一套约定：控制器持时钟、渲染 tick 推进、视图带进度，
    /// 模型不知道动画存在。静止时顺手把"当前段"跟上选中项。
    fn advance_segment_slides(&mut self) -> bool {
        let now = Instant::now();
        let selected = self
            .region_panel
            .as_ref()
            .map(|panel| (panel.unit().index(), panel.delay().index()));
        if let Some((unit, delay)) = selected {
            self.unit_slide.settle(unit);
            self.delay_slide.settle(delay);
        }
        self.unit_slide.settle_if_done(now);
        self.delay_slide.settle_if_done(now);
        self.unit_slide.running(now) || self.delay_slide.running(now)
    }

    fn advance_guide_fade(&mut self) -> bool {
        let wanted = !self.guide_groups.is_empty()
            && self.session.state() == CaptureState::Selecting
            && self.scroll.is_none();
        let near = wanted && self.cursor_visible && self.pointer_near_guide();
        self.guide.advance(wanted && !near, Instant::now())
    }

    fn pointer_near_guide(&mut self) -> bool {
        let reach = (GUIDE_AVOID_DIP * self.session.dpi().max(96) as f32 / 96.0).round() as i32;
        let groups = self.guide_groups.clone();
        let Some(renderer) = self.renderer.as_mut() else {
            return false;
        };
        match renderer.guide_bounds(&groups) {
            Ok(Some(bounds)) => bounds.inflate(reach).contains(self.cursor),
            Ok(None) => false,
            Err(error) => {
                eprintln!("[snapclip][capture] guide layout failed: {error}");
                false
            }
        }
    }

    pub(super) fn render(&mut self) {
        // An export is in flight: the overlay is frozen at the confirmed selection
        // and must not present, so nothing races the hand-off (see confirm()).
        if self.graphics_released {
            return;
        }
        let session_id = self.session_id();
        // Resolve the paint-only window hints before borrowing the renderer, so the
        // snapshot lookup and the preview read do not overlap a mutable borrow.
        let hover_bounds = self.hover_bounds_local();
        // The painted preview is the *eased* rectangle; the gesture keeps the true target
        // for confirmation, so the animation can never change what gets committed.
        let preview_bounds = self.preview_rect;
        let chain_rings = self.chain_rings_local();
        // …and the two label texts, for the same reason: they read the level chain and the
        // last precision decision, which are `self` reads.
        let preview_label = self.preview_label_text();
        let preview_is_window = self.preview_is_window();
        let capture_green = self.capture_box_green();
        let preview_alpha = self.preview_appear();
        let level_badge = self.level_badge();
        let hint = self.hint_text();
        let precision_feedback = self.precision_feedback_active();
        let guide = (self.guide.alpha() > 0.0).then(|| GuideView {
            groups: self.guide_groups.clone(),
            alpha: self.guide.alpha(),
        });
        // …and how the badge reads, for the same reason: it is a `self` read, and the renderer borrow
        // below outlives it (docs/23 §14.53).
        let label = self.preferences.label;
        // …and the custom-region panel, for the same reason: it is a `self` read too, and the
        // renderer borrow below outlives it (docs/23 §14.54).
        let region_panel = self.region_panel_view();
        // …以及延时倒计时的剩余秒数，同样必须在渲染器借用之前读出来（docs/23 §14.55.9）。
        let countdown = self.countdown_seconds_left();
        match frame_kind(self.countdown_hud, countdown) {
            // 结构闸：HUD 还生效、倒计时却已经结束 —— 窗口还没隐藏，这一帧坚决不画普通画面。
            // "先画一帧再隐藏"就是用户看到的那下灰底（docs/23 §14.55.9.6）。
            FrameKind::Suppressed => {
                self.frames_suppressed += 1;
                return;
            }
            FrameKind::Hud => self.frames_hud += 1,
            FrameKind::Normal => {
                self.frames_normal += 1;
                // 可见性读数：如果普通帧全在窗口隐藏之后，那"灰底"就不是普通帧，得换方向查。
                // `self.window` 是 windows-sys 的 `HWND`（裸指针），所以这里用同一族的 IsWindowVisible。
                if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(self.window) } != 0 {
                    self.frames_normal_visible += 1;
                }
            }
        }
        // 光标闪烁的计时器：面板开着且有数字格聚焦时才需要（内部只在状态变化时真正 Set/Kill）。
        self.sync_caret_blink();
        let panel_open = self.region_panel.is_some();
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        if panel_open {
            // The panel's glyphs are decoded and pre-tinted here, before the frame: the tint renders
            // have their own BeginDraw/EndDraw and cannot nest inside this one. Idempotent, so asking
            // every frame is free, and the session-start path no longer pays the 1.6 ms it used to
            // (docs/23 §14.54).
            if let Err(error) = renderer.prepare_panel_icons() {
                eprintln!("[snapclip][capture] panel icons unavailable: {error}");
            }
        }
        // Every present repaints the whole surface (see OverlayRenderer::draw_to); the
        // render tick is what bounds the present rate to ~60 Hz.
        // Per-present logging is diagnostics, not telemetry: it used to print once per frame
        // and bury everything else (a 10 s session produced thousands of lines). It is now
        // behind the verbose switch, and the per-session summary carries the counters.
        self.metrics.log_line(
            &format!(
                "render session={} frame_px={}",
                session_id,
                renderer.frame().area()
            ),
            false,
        );
        let cursor_visible = self.cursor_visible && self.session.state().is_active();
        // `CaptureSession::begin_scroll` hands the capture session back to Idle; the scroll
        // session keeps the selection on screen.
        let scroll_selection = self.scroll.as_ref().map(|run| run.selection);
        let selection = selection_for_render(self.session.selection(), scroll_selection);
        // A scroll session draws its own view of the selection (frame and panel only); the
        // capture session behind it is Idle and has no chrome.
        let show_chrome = self.session.shows_chrome();
        let show_handles = self.session.shows_handles();
        let show_size_label = self.session.shows_size_label();
        // The badge lights up while the pointer is on it: it is the overlay's one pointer-driven
        // control, and the highlight is what says that a click here opens something (docs/23 §14.54).
        let label_hovered = cursor_visible
            && self
                .label_badge
                .is_some_and(|badge| badge.contains(self.cursor));
        // The loupe samples the picture under the pointer; while the panel is open the pointer is on a
        // form, and the loupe — which hangs below the pointer — would sit on top of the rows being
        // typed into. Hiding it also keeps the badge in place: the label steps aside for the loupe's
        // rectangle, and a badge that moved would move the click target the panel opens from
        // (docs/23 §14.54).
        let show_magnifier = self.session.shows_magnifier() && self.region_panel.is_none();
        let scroll = self.scroll.as_ref().map(scroll::ScrollRun::view);
        let state = OverlayFrameState {
            selection,
            cursor: self.cursor,
            cursor_visible,
            show_chrome,
            show_handles,
            show_size_label,
            label,
            label_hovered,
            region_panel,
            countdown,
            show_magnifier,
            precision_feedback,
            magnifier_rgb: self.sampler.rgb(),
            magnifier_color_text: self.sampler.formatted(self.magnifier_color_format),
            magnifier_relative: self.magnifier_relative,
            magnifier_zoom: self.magnifier_zoom,
            annotation_items: self.annotation_doc.items().to_vec(),
            annotation_selected_id: self.annotation_doc.selected_id(),
            annotation_draft: self.annotation_doc.draft.clone(),
            hover_bounds,
            preview_bounds,
            chain_rings,
            preview_label,
            preview_is_window,
            capture_green,
            preview_alpha,
            level_badge,
            hint,
            scroll,
            mask_blur: self.preferences.mask_blur,
            loupe_hints: self.loupe_hints.clone(),
            guide,
        };
        // Where the badge lands, taken from the code that draws it: the menu hangs off this
        // rectangle and the badge is the pointer's way in, so a second opinion about where the badge
        // went would be a hit test that can disagree with the paint pass (docs/23 §14.53).
        self.label_badge = match show_size_label {
            true => match renderer.size_label_bounds(&state) {
                Ok(bounds) => bounds,
                Err(error) => {
                    eprintln!("[snapclip][capture] size label bounds failed: {error}");
                    None
                }
            },
            false => None,
        };
        // Live borrow of annotation document avoids cloning items every tick.
        //
        // The present is timed here because this is the only place that knows a full repaint
        // happened: the overlay's cost model is frames, and `present=`/`present_us` in the session
        // summary are what let "four more rings cost nothing" be checked on a real machine
        // (docs/21 §5.22).
        let presented_at = Instant::now();
        let outcome = renderer.render(&state, Some(&self.annotation_doc));
        self.metrics.record_present(presented_at.elapsed());
        match outcome {
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
}

#[cfg(test)]
mod tests {
    use super::selection_for_render;
    use crate::geometry::Rect;

    #[test]
    fn the_scroll_selection_replaces_the_idle_capture_selection_for_rendering() {
        let selection = Rect::new(100, 200, 700, 900);
        assert_eq!(
            selection_for_render(Rect::default(), Some(selection)),
            selection
        );
        assert_eq!(selection_for_render(selection, None), selection);
    }
}

/// 这一帧该画什么（docs/23 §14.55.9.6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FrameKind {
    /// 倒计时 HUD：只画选框、角标、数字。
    Hud,
    /// 普通帧。
    Normal,
    /// HUD 还生效，但倒计时已经结束（窗口尚未隐藏）：压掉，绝不允许画出普通画面。
    Suppressed,
}

/// 用**状态**决定帧的类型，而不是靠"谁别 invalidate"。
pub(super) fn frame_kind(countdown_hud: bool, countdown_seconds: Option<u32>) -> FrameKind {
    match (countdown_hud, countdown_seconds) {
        // 还在数：HUD（闸没置位也画 HUD —— 那说明 arm 与状态不同步，HUD 是更安全的答案）。
        (_, Some(_)) => FrameKind::Hud,
        (true, None) => FrameKind::Suppressed,
        (false, None) => FrameKind::Normal,
    }
}
