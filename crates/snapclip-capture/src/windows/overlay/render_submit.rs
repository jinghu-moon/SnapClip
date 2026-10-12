//! Painting, invalidation and the render tick (docs/23 D3 / T1.6.4).
//!
//! Split out of `overlay.rs`: the controller's presentation half — when the surface is
//! dirty, when a repaint is coalesced, and what a frame does when the device is lost.
//! These are `pub(super)` because the parent module owns the message loop that drives them.

use super::*;

/// The selection the overlay draws. Starting a scroll session ends the capture session, so its
/// selection is empty by design; the scroll session's selection is the one on screen thereafter.
pub(super) fn selection_for_render(
    session_selection: crate::geometry::Rect,
    scroll_selection: Option<crate::geometry::Rect>,
) -> crate::geometry::Rect {
    scroll_selection.unwrap_or(session_selection)
}

impl OverlayController {
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
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.render();
        if self.preview_transition.is_running(Instant::now())
            || self.chain_fade_running()
            || self.walk_activity_running()
            || self.guide.running()
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
        let guide = (self.guide.alpha() > 0.0).then(|| GuideView {
            groups: self.guide_groups.clone(),
            alpha: self.guide.alpha(),
        });
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
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
        let scroll = self.scroll.as_ref().map(scroll::ScrollRun::view);
        let state = OverlayFrameState {
            selection,
            cursor: self.cursor,
            cursor_visible,
            show_chrome,
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
