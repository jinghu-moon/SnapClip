//! Window detection, the hover preview and refinement scheduling (docs/23 D3, slice 5).
//!
//! Split out of `overlay.rs`: the largest remaining cluster, and the one the product is about —
//! what the cursor is over, which level of the box chain is current, how the hint/chain/walk
//! animations advance, and how a refinement job is submitted and applied.
//! `pub(super)` because the input handlers and the window procedure drive all of it.

use super::*;

impl OverlayController {
    /// Start a fresh window-detection cycle for a new session (docs/14 §5.3).
    ///
    /// Everything that could leak from the previous session — hover, preview, snapshot,
    /// outstanding requests — is dropped here, and the refresh is handed to the detection
    /// worker so the overlay thread never runs `EnumWindows`/DWM itself.
    pub(super) fn begin_window_detection(&mut self, layout: &MonitorLayout) {
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
    pub(super) fn request_snapshot_refresh(&mut self) {
        self.snapshot_request = Some(self.detector.request_refresh(&self.exclusions));
    }

    /// Monitor-local cursor position expressed in virtual-desktop coordinates.
    pub(super) fn cursor_screen(&self) -> Option<Point> {
        self.layout().map(|layout| layout.to_screen(self.cursor))
    }

    /// The hovered window in monitor-local coordinates, ready for the paint layer.
    ///
    /// Returns `None` once the session is settled, which is what turns window hover off
    /// after a confirmation (docs/14 §4.1) without a second state flag: `update_hover`
    /// already refuses to resolve a target outside `Selecting`.
    pub(super) fn hover_bounds_local(&self) -> Option<Rect> {
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
    pub(super) fn selected_deep_level(&self) -> Option<PathLevel> {
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
    pub(super) fn preview_label_text(&self) -> Option<String> {
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
    pub(super) fn preview_kind(&self) -> Option<LevelKind> {
        let deep = self.deep_target.as_ref()?;
        let level = self
            .selected_deep_level()
            .or_else(|| deep.path.last().copied())?;
        Some(level.kind)
    }

    /// Whether the user has walked off the deepest level — which is what makes the label say `容器`
    /// and what makes the level badge appear at all (docs/21 §5.22).
    pub(super) fn walked_up(&self) -> bool {
        self.deep_levels.is_some_and(|chain| !chain.is_deepest())
    }

    /// What the level badge shows: how many stops the wheel still has each way (docs/21 §5.24, A1).
    pub(super) fn level_badge(&self) -> Option<LevelReach> {
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
    pub(super) fn preview_is_window(&self) -> bool {
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
    pub(super) fn hint_text(&self) -> Option<(Point, String)> {
        let hint = self.hint.as_ref()?;
        if Instant::now() >= hint.until || self.session.state() != CaptureState::Selecting {
            return None;
        }
        Some((self.cursor, hint.text.clone()))
    }

    /// Arm a one-shot hint, replacing whatever was showing (docs/21 §5.21).
    pub(super) fn arm_hint(&mut self, text: String, millis: u64) {
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
    pub(super) fn arm_level_hint(&mut self) {
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
    pub(super) fn hint_showing(&self) -> bool {
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
    pub(super) fn touch_chain(&mut self) {
        self.chain.touch(Instant::now());
    }

    /// Advance the ③b fade and the ② rise; returns whether anything changed and therefore needs a
    /// repaint.
    ///
    /// Called from the coalescing render tick, which also keeps itself alive while the fade is still
    /// moving (`chain_fade_running`), so the fade is driven by the same 15 ms clock as everything
    /// else rather than by a timer of its own.
    pub(super) fn advance_chain_fade(&mut self) -> bool {
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
    pub(super) fn chain_fade_pending(&self) -> bool {
        self.chain.pending()
    }

    /// Whether the fade is *moving* right now: only then does the render tick re-arm itself, so the
    /// 1.2 s of waiting costs nothing (docs/21 §5.22).
    pub(super) fn chain_fade_running(&self) -> bool {
        self.chain.running(Instant::now())
    }

    /// A walk just happened: the capture box turns green (docs/21 §5.24, A3).
    ///
    /// Called for every walk attempt, including one that ends up pinned against the end of the
    /// chain: the colour says "the wheel was used", and a notch that does nothing is exactly when
    /// that needs saying.
    pub(super) fn touch_walk(&mut self) {
        self.walk.touch(Instant::now());
        self.invalidate();
    }

    /// Step the walk envelope; returns whether the painted value changed.
    ///
    /// Driven by the same coalescing render tick as the chain fade, for the same reason: the value
    /// only moves at ~60 Hz at most, so a timer of its own would only add ways to be late.
    pub(super) fn advance_walk_activity(&mut self) -> bool {
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
    pub(super) fn walk_activity_running(&self) -> bool {
        self.walk.running(Instant::now())
    }

    pub(super) fn walk_activity_pending(&self) -> bool {
        self.walk.pending()
    }

    /// How green to paint the capture box.
    ///
    /// The whole-window fallback gets none: it is the neutral answer, it has no capture colour to
    /// lift, and the prototype keeps it grey. Everything else follows the walk (docs/21 §5.24, A3).
    pub(super) fn capture_box_green(&self) -> f32 {
        if self.preview_is_window() {
            0.0
        } else {
            self.walk.activity()
        }
    }

    /// Step the deep-selection level: `-1` toward the window frame, `+1` toward the published box.
    ///
    /// Returns whether anything moved, so the wheel can decide whether to consume the event.
    pub(super) fn step_deep_level(&mut self, delta: i32) -> bool {
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
    pub(super) fn chain_rings_local(&mut self) -> Vec<ChainRingView> {
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
    pub(super) fn update_hover(&mut self) {
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
    pub(super) fn drive_refinement(&mut self, screen: Point, target: Option<WindowTarget>) {
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

    pub(super) fn clear_hover(&mut self) {
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
    pub(super) fn sync_preview_rect(&mut self) {
        self.set_preview_target(self.gesture.snap_preview().map(|preview| preview.selection));
    }

    pub(super) fn set_preview_target(&mut self, target: Option<Rect>) {
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
    pub(super) fn preview_appear(&self) -> f32 {
        self.preview_appeared_at
            .map(|since| appear_share(since.elapsed(), PREVIEW_APPEAR_MS))
            .unwrap_or(1.0)
    }

    /// Advance the preview animation and report whether the painted rectangle changed.
    pub(super) fn advance_preview_animation(&mut self) -> bool {
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
    pub(super) fn arm_dwell(&mut self) {
        let generation = self.gesture.dwell_generation();
        if self.dwell_armed == Some(generation) {
            return;
        }
        self.dwell_armed = Some(generation);
        unsafe { SetTimer(self.window, DWELL_TIMER_ID, DEFAULT_DWELL_MS, None) };
    }

    pub(super) fn disarm_dwell(&mut self) {
        if self.dwell_armed.take().is_some() {
            unsafe { KillTimer(self.window, DWELL_TIMER_ID) };
        }
    }

    /// The dwell timer expired: decide whether an automatic-snap preview is shown.
    ///
    /// Everything here is a cached lookup plus one rectangle conversion — no Win32 and no
    /// DWM call runs on this path (docs/14 §10.1).
    pub(super) fn on_dwell(&mut self) {
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
    pub(super) fn preview_for_cursor(&mut self) -> Option<(WindowTarget, Rect, Rect)> {
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
    pub(super) fn on_detection_ready(&mut self) {
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
    pub(super) fn apply_hover_validity(&mut self, target: WindowTarget, validity: HoverValidity) {
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
    pub(super) fn refresh_preview_for_cursor(&mut self) {
        let generation = self.gesture.dwell_generation();
        let preview = self.preview_for_cursor();
        if self.gesture.apply_dwell(generation, preview) {
            self.sync_preview_rect();
            self.invalidate();
        }
    }

    /// Periodic hover re-validation tick. Only enqueues; never reads DWM here.
    pub(super) fn on_hover_tick(&mut self) {
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
    pub(super) fn poll_refinement_timeout(&mut self) {
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
    pub(super) fn arm_refinement(&mut self) {
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
    pub(super) fn disarm_refinement(&mut self) {
        self.refinement_pending = None;
        unsafe { KillTimer(self.window, REFINEMENT_TIMER_ID) };
    }

    /// The refinement dwell expired: submit a deep query if the scheduler allows one.
    pub(super) fn on_refinement_tick(&mut self) {
        // The timer is periodic by nature; killing it here is what makes the dwell a
        // one-shot. A new hover re-arms it.
        unsafe { KillTimer(self.window, REFINEMENT_TIMER_ID) };
        if let Some(job) = self.refine.on_dwell_due() {
            self.submit_refinement(job);
        }
    }

    /// The single-flight slot just freed: issue the position a dwell was deferred for, if any.
    pub(super) fn submit_follow_up(&mut self) {
        if let Some(job) = self.refine.take_follow_up() {
            self.metrics.record_refinement_follow_up();
            // An answer for the current position is on its way again: hold the verified
            // rectangle instead of letting the preview fall back to the whole window.
            self.refinement_pending = Some(Instant::now());
            self.submit_refinement(job);
        }
    }

    /// Hand one scheduled query to the worker.
    pub(super) fn submit_refinement(&mut self, job: RefinementJob) {
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
    pub(super) fn hit_test_passes_through(&self) -> bool {
        self.hit_test_pass_through.is_active() && !any_mouse_button_down()
    }

    /// A deep-selection result arrived.
    ///
    /// A published path only ever *refines* the v1 whole-window target: the overlay keeps
    /// painting the v1 frame when nothing came back, which is what makes a missing or
    /// failing accessibility provider a degradation rather than a regression.
    pub(super) fn on_refinement_ready(&mut self) {
        let Some(result) = self.refinement.take_result() else {
            return;
        };
        self.apply_refinement_result(result);
        // Whatever happened to this answer, a position whose dwell expired while it ran has not
        // been asked about yet.
        self.submit_follow_up();
    }

    pub(super) fn apply_refinement_result(&mut self, result: RefinementResult) {
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

    pub(super) fn arm_hover_timer(&mut self) {
        unsafe {
            SetTimer(
                self.window,
                HOVER_TIMER_ID,
                DEFAULT_HOVER_REVALIDATE_MS,
                None,
            )
        };
    }

    pub(super) fn disarm_hover_timer(&mut self) {
        self.hover_request = None;
        unsafe { KillTimer(self.window, HOVER_TIMER_ID) };
    }

    /// `Enter` while a preview is shown: hand the target to the worker for validation.
    ///
    /// Returns whether a confirmation was started, so the caller can fall through to the
    /// normal confirm path when there is no preview.
    pub(super) fn confirm_snap_preview(&mut self) -> bool {
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
    pub(super) fn apply_confirmation(&mut self, target: WindowTarget, valid: bool) {
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
