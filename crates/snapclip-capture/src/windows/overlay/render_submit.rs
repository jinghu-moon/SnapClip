//! Painting, invalidation and the render tick (docs/23 D3 / T1.6.4).
//!
//! Split out of `overlay.rs`: the controller's presentation half — when the surface is
//! dirty, when a repaint is coalesced, and what a frame does when the device is lost.
//! These are `pub(super)` because the parent module owns the message loop that drives them.

use super::*;

impl OverlayController {
    /// Mark the surface dirty for a discrete edit (undo / redo / delete / z-order /
    /// toolbar command) and coalesce a full repaint into the next tick.
    pub(super) fn invalidate_all(&mut self) {
        self.invalidate();
    }

    // ---- rendering -------------------------------------------------------

    /// Take over the preview port of a scroll session and start painting its panel (docs/30 §19.3).
    ///
    /// The overlay only holds the consumer side: the driver publishes into the same [`PreviewStream`]
    /// and posts [`SCROLL_READY_MESSAGE`] when it has, which is what makes this a poll rather than a
    /// callback (`docs/30` §21.4: the message is the only wake-up, and the port deliberately does not
    /// carry a second one).
    ///
    /// Separate from the panel itself so the geometry of the session — the viewport extent, which
    /// decides how tall the viewport box is — is stated once, when the session is handed over, rather
    /// than inferred from updates that never carry it.
    ///
    /// The caller is the assembly root, `P7.05`'s `begin_scroll_session`; the `#[allow(dead_code)]`
    /// this carried until then said so, and it is gone now that the hand-over exists.
    pub(crate) fn watch_scroll_preview(
        &mut self,
        preview: std::sync::Arc<crate::scroll::preview::PreviewStream>,
        cross_len: u64,
        extent: u64,
    ) {
        self.scroll_preview = Some(preview);
        self.scroll_panel = Some(crate::scroll::panel::ScrollPanel::new(cross_len, extent));
        self.invalidate();
    }

    /// Fold everything the driver has published since the last call into the panel (docs/30 §19.3).
    ///
    /// `take` is oldest-first and returns one update at a time — §19.3.2 chose that so a consumer that
    /// is woken *once* still sees everything, which is exactly this loop. Returns whether anything
    /// changed, so the caller can decide to repaint instead of repainting blind.
    pub(crate) fn on_scroll_ready(&mut self) -> bool {
        let Some(preview) = self.scroll_preview.clone() else {
            return false;
        };
        // The drain is scoped so the borrow of the panel ends before `invalidate`, which needs the
        // whole controller.
        let changed = {
            let Some(panel) = self.scroll_panel.as_mut() else {
                return false;
            };
            let mut changed = false;
            while let Some(update) = preview.take() {
                panel.on_update(update);
                changed = true;
            }
            changed
        };
        if changed {
            self.invalidate();
        }
        changed
    }

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
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.render();
        if self.preview_transition.is_running(Instant::now())
            || self.chain_fade_running()
            || self.walk_activity_running()
            // …and the box's own appear ramp, which is a plain ramp on the same clock.
            || self.preview_appear() < 1.0
        {
            // Schedule the next frame: the animation or the fade is still moving.
            self.invalidate();
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
        let state = OverlayFrameState {
            selection: self.session.selection(),
            cursor: self.cursor,
            cursor_visible,
            show_chrome: self.session.shows_chrome(),
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
            scroll_panel: self.scroll_panel.clone(),
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
