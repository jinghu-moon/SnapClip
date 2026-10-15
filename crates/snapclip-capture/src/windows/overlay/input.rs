//! User input: pointer, keyboard and annotation gestures (docs/23 D3 / T1.6.2).
//!
//! Split out of `overlay.rs`. These are the handlers that turn Windows messages into
//! controller state — they decide nothing about *how* the overlay looks (that is
//! `render_submit`) or *what* a session is (that is the parent module); they only translate.
//! `pub(super)` because the parent module's window procedure dispatches to them.

use super::*;

fn nudge_point(point: POINT, dx: i32, dy: i32) -> POINT {
    POINT {
        x: point.x.saturating_add(dx),
        y: point.y.saturating_add(dy),
    }
}

impl OverlayController {
    pub(super) fn on_mouse_move(&mut self, client: POINT) {
        let point = Point::new(client.x, client.y);
        let shift = self.shift_down || modifiers().1;
        self.cursor = point;
        self.cursor_visible = true;
        // A cursor move deliberately does **not** touch the chain (docs/21 §5.24, ④): it used to, so
        // that "the hand is on its way to inspect it" would not lose the rings — but the effect was
        // that a user who kept moving the mouse never saw them fade, which is the disappearance this
        // exists for. Walks and new answers are the triggers now, so "scrolling stopped" really does
        // mean the chain goes away.

        // A scroll session has no selection to edit: the pointer only ever reaches the panel,
        // whose buttons it presses.
        if self.scroll.is_some() {
            self.update_cursor_shape(point);
            return;
        }

        if self.session.state() == CaptureState::Annotating {
            // The pointer drives the annotation document, not the selection.
            self.annotation_point_moved(point);
            self.update_annotation_cursor(point);
        } else {
            // The gesture machine decides whether this move is hover, a pending press or a
            // drag (docs/14 §4.2). The session still owns *how* the geometry changes.
            match self.gesture.move_cursor(point) {
                MoveOutcome::Hover => {
                    // Superseded before it could be painted: counted rather than queued.
                    self.metrics
                        .record_mouse_move_coalesced(u64::from(self.render_armed));
                    // Hover resolves from the cached snapshot only — no Win32, no DWM.
                    self.update_hover();
                    self.arm_dwell();
                }
                MoveOutcome::Pending => {}
                MoveOutcome::ManualDragStarted { press_point } => {
                    // Crossing the drag threshold turns the pending press into a free
                    // drag anchored at the press point. The snap preview is already gone:
                    // the press replaced it.
                    self.session.begin_drag(
                        press_point,
                        ResizeMode::Handle(Handle::BottomRight),
                        true,
                    );
                    self.session
                        .pointer_moved_with_constraints(point, shift);
                    self.disarm_dwell();
                }
                MoveOutcome::Dragging => {
                    self.session
                        .pointer_moved_with_constraints(point, shift);
                }
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
    pub(super) fn update_cursor_shape(&mut self, point: Point) {
        let cursor = if self.scroll.is_some() {
            IDC_ARROW
        } else if !self.session.has_selection() {
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

    pub(super) fn on_mouse_leave(&mut self) {
        if self.cursor_visible {
            self.cursor_visible = false;
            self.invalidate();
        }
    }

    pub(super) fn on_left_down(&mut self, client: POINT) {
        let point = Point::new(client.x, client.y);
        if let Some(run) = self.scroll.as_ref() {
            if let Some(button) = run.layout.button_at(point.x, point.y) {
                self.on_scroll_button(button);
            }
            return;
        }
        if self.session.state() == CaptureState::Annotating {
            self.annotation_point_down(point);
            return;
        }
        if !matches!(
            self.session.state(),
            CaptureState::Selecting | CaptureState::Selected
        ) {
            return;
        }
        // A press is the user taking over; the hint must not sit next to the result (docs/21 §5.21).
        self.hint = None;
        // Ask what the press *would* do, then record the gesture. Neither step changes the
        // selection (docs/14 §4.2).
        let hit = self.session.press(point);
        let outcome = self.gesture.press(point, hit, self.session.selection());
        if let PressOutcome::BeginEdit(mode) = outcome {
            // Grabbing a handle or the interior of a selection is deliberate: the drag
            // starts immediately, with no threshold.
            self.session.begin_drag(point, mode, false);
            self.disarm_dwell();
        }
        eprintln!(
            "[snapclip][capture] pointer down session={} point=({},{}), hit={:?}, outcome={:?}",
            self.session_id(),
            point.x,
            point.y,
            hit,
            outcome
        );
        self.update_cursor_shape(point);
        self.invalidate();
    }

    pub(super) fn on_left_up(&mut self) {
        if self.scroll.is_some() {
            return;
        }
        if self.session.state() == CaptureState::Annotating {
            self.annotation_point_up();
            return;
        }
        if !matches!(
            self.session.state(),
            CaptureState::Selecting | CaptureState::Selected
        ) {
            self.gesture.release();
            return;
        }
        // Releasing the button only ever commits an explicit drag or edit. A pending press
        // is a click, and an automatic snap is **never** confirmed by releasing the button
        // (docs/14 §4.2).
        match self.gesture.release() {
            ReleaseOutcome::CommitDrag => {
                self.session.pointer_released();
                self.session.pointer_left();
                // The selection is final: read it back now, while the frozen frame is still
                // alive, so `F6` can pin it without the user having to save first.
                self.publish_selection("drag");
            }
            ReleaseOutcome::Click => {
                self.session.pointer_left();
                // Primary confirm gesture (docs/14 §4.3): a click on a previewed window
                // commits it. A click with no preview leaves every state untouched.
                self.confirm_snap_preview();
            }
        }
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

    /// A key went down; `true` when it meant something here.
    ///
    /// The keymap decides what a key does ([`crate::keymap`]); this only dispatches. A key the map
    /// does not know goes to the annotation tools while annotating, and is otherwise left alone —
    /// which is what lets `Alt+F4` and the other system keys through `WM_SYSKEYDOWN`.
    pub(super) fn on_key_down(&mut self, vk: u32) -> bool {
        let state = self.session.state();
        self.metrics
            .log_line(&format!("key down vk=0x{vk:02X} state={state:?}"), false);
        if is_shift_vk(vk) {
            self.shift_down = true;
            self.on_snap_modifier_changed();
            return state.is_active();
        }
        let context = if self.scroll.is_some() {
            Context::Scroll
        } else {
            Context::Pick
        };
        let (ctrl, shift, alt) = modifiers();
        if let Some(key) = Key::from_virtual_key(vk) {
            let keymap = self.preferences.keymap.clone();
            // A held key only arms its wheel gesture; it does nothing by itself.
            if !(ctrl || shift || alt) && keymap.holds(context, key) {
                if state.is_active() {
                    self.held = Some(key);
                }
                return true;
            }
            if let Some(action) = keymap.action_for(context, &Chord::key(key, ctrl, shift, alt)) {
                self.run_action(action);
                return true;
            }
        }
        if self.scroll.is_none() && state == CaptureState::Annotating {
            return self.on_annotation_key(vk);
        }
        false
    }

    /// A key went up: it no longer holds a wheel gesture.
    pub(super) fn on_key_up(&mut self, vk: u32) {
        if is_shift_vk(vk) {
            self.shift_down = false;
            self.on_snap_modifier_changed();
        }
        if self.held.is_some() && self.held == Key::from_virtual_key(vk) {
            self.held = None;
        }
    }

    /// The wheel turned outside a scroll session; `true` when it meant something here.
    pub(super) fn on_wheel(&mut self, delta: i16) -> bool {
        let (ctrl, shift, alt) = modifiers();
        let chord = Chord::wheel(self.held, ctrl, shift, alt);
        match self.preferences.keymap.action_for(Context::Pick, &chord) {
            Some(Action::LoupeZoom) if self.session.state().is_active() => {
                let direction = if delta > 0 { 1 } else { -1 };
                self.magnifier_zoom = MagnifierConfig::zoom_step(self.magnifier_zoom, direction);
                self.invalidate();
                true
            }
            Some(Action::LevelWheel) => {
                // The wheel walks the deep-selection levels (docs/21 §5.17): up toward the window
                // frame, down toward the element under the cursor. Consumed, so the page underneath
                // never sees a scroll it did not ask for.
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
                walked
            }
            _ => false,
        }
    }

    fn run_action(&mut self, action: Action) {
        let state = self.session.state();
        match action {
            Action::Cancel | Action::ScrollCancel => self.cancel("escape"),
            // Up walks toward the window frame, down back toward the box the refinement published
            // (docs/21 §5.17).
            Action::LevelUp => {
                self.step_deep_level(-1);
            }
            Action::LevelDown => {
                self.step_deep_level(1);
            }
            Action::CursorUp => self.nudge_system_cursor(0, -1),
            Action::CursorDown => self.nudge_system_cursor(0, 1),
            Action::CursorLeft => self.nudge_system_cursor(-1, 0),
            Action::CursorRight => self.nudge_system_cursor(1, 0),
            Action::Confirm => {
                let has_selection = self.session.has_selection();
                // F7 arms a distinct flow: Enter confirms the user's rectangle and starts the
                // scroll session immediately. A snap preview may still be under the cursor, but
                // routing Enter through the asynchronous snap confirmation would consume the key.
                if self.scroll_entry_armed && has_selection {
                    self.confirm();
                    return;
                }
                // A snap preview is confirmed through the detection worker — never validated
                // synchronously on the overlay thread. A settled selection goes straight to the
                // export path.
                if !self.confirm_snap_preview() && has_selection {
                    self.confirm();
                }
            }
            // Selected -> Annotating (selection is locked, tools go live).
            Action::Annotate => {
                if state == CaptureState::Selected && self.session.begin_annotating().is_ok() {
                    self.publish_state();
                    self.invalidate_all();
                }
            }
            Action::ColorFormat => {
                if state.is_active() {
                    self.magnifier_color_format = self.magnifier_color_format.next();
                    self.invalidate();
                }
            }
            Action::ColorCopy => {
                if state.is_active() {
                    self.copy_color_to_clipboard();
                }
            }
            // Screen ↔ selection-relative coordinates in the info strip.
            Action::CoordMode => {
                if state.is_active() {
                    self.magnifier_relative = !self.magnifier_relative;
                    self.invalidate();
                }
            }
            Action::ScrollAuto => self.toggle_scroll_auto(),
            Action::ScrollFinish => self.finish_scroll_session("key"),
            // Pointer gestures: the map lists them for the guide, and the pointer handlers own them.
            Action::LevelWheel | Action::LoupeZoom | Action::PickClick | Action::PickDrag => {}
        }
    }

    /// Move the real system cursor by one physical screen pixel. The resulting `WM_MOUSEMOVE`
    /// follows the same path as a physical mouse move, so hover detection, snapping and the
    /// magnifier all observe the same point without a second coordinate model.
    fn nudge_system_cursor(&mut self, dx: i32, dy: i32) {
        if !matches!(
            self.session.state(),
            CaptureState::Preparing
                | CaptureState::Armed
                | CaptureState::Selecting
                | CaptureState::Selected
        ) {
            return;
        }
        let mut cursor = POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut cursor) } == 0 {
            return;
        }
        unsafe {
            let target = nudge_point(cursor, dx, dy);
            SetCursorPos(target.x, target.y);
        }
        // Keep the cue short and restart it for a burst of arrow-key nudges. The
        // cursor still follows the normal WM_MOUSEMOVE path, so hover, sampling and
        // snapping use one coordinate source.
        self.precision_feedback_until = Some(Instant::now() + Duration::from_millis(650));
        self.invalidate();
    }

    /// Copy the currently displayed colour string (HEX / RGB / HSL, whichever the `S`
    /// cycle is on) to the clipboard.
    ///
    /// The write belongs to the shell (`ClipboardWriter`), because owning the clipboard
    /// also means marking the entry so SnapClip's own clip monitor ignores it.
    pub(super) fn copy_color_to_clipboard(&mut self) {
        let format = self.magnifier_color_format;
        let Some(text) = self.sampler.formatted(format) else {
            return;
        };
        self.clipboard.copy_text(&text);
    }

    /// Keyboard handling while the session is annotating; `true` when the key meant something.
    pub(super) fn on_annotation_key(&mut self, key: u32) -> bool {
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
            return true;
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
            _ => return false,
        }
        true
    }

    // ---- annotation interaction ------------------------------------------

    /// Half-size, in physical pixels, of a control-point hit box.
    pub(super) fn annotation_handle_tol(&self) -> i32 {
        (8.0 * self.session.dpi().max(96) as f32 / 96.0) as i32
    }

    /// Pointer moved while annotating: apply the active gesture; `invalidate` marks the
    /// change and coalesces a full repaint into the next tick.
    pub(super) fn annotation_point_moved(&mut self, point: Point) {
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
                self.annotation_gesture = Some(AnnotationGesture::Move {
                    id,
                    snapshot,
                    last: point,
                });
            }
            Some(AnnotationGesture::Resize {
                id,
                handle,
                snapshot,
            }) => {
                self.annotation_doc.resize_item(id, handle, point);
                self.annotation_gesture = Some(AnnotationGesture::Resize {
                    id,
                    handle,
                    snapshot,
                });
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
    pub(super) fn update_draft_shape(&mut self, start: Point, current: Point) {
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
    pub(super) fn initial_draft_geometry(kind: AnnotationKind, point: Point) -> AnnotationGeometry {
        let degenerate = Rect::from_corners(point, point);
        match kind {
            AnnotationKind::Rectangle => AnnotationGeometry::Rect { bounds: degenerate },
            AnnotationKind::Ellipse => AnnotationGeometry::Ellipse { bounds: degenerate },
            AnnotationKind::Line => AnnotationGeometry::Line {
                start: point,
                end: point,
            },
            AnnotationKind::Arrow => AnnotationGeometry::Arrow {
                start: point,
                end: point,
            },
            AnnotationKind::Freehand => AnnotationGeometry::Freehand {
                points: vec![point],
            },
            AnnotationKind::Highlight => AnnotationGeometry::Highlight {
                points: vec![point],
            },
            AnnotationKind::Text => AnnotationGeometry::Text {
                position: point,
                content: String::new(),
            },
        }
    }

    /// A draft is committed only when it covers real pixels.
    pub(super) fn draft_is_significant(draft: &AnnotationItem) -> bool {
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
    pub(super) fn annotation_point_down(&mut self, point: Point) {
        if self.annotation_tool.is_none() {
            // Select tool: a control point resizes first, then the body moves.
            if let Some(id) = self.annotation_doc.selected_id() {
                if let Some(handle) =
                    self.annotation_doc
                        .handle_at(id, point, self.annotation_handle_tol())
                {
                    let snapshot = self.annotation_doc.snapshot();
                    self.annotation_gesture = Some(AnnotationGesture::Resize {
                        id,
                        handle,
                        snapshot,
                    });
                    self.invalidate();
                    return;
                }
            }
            match self.annotation_doc.hit_test(point) {
                Some(id) => {
                    let snapshot = self.annotation_doc.snapshot();
                    self.annotation_doc.select(Some(id));
                    self.annotation_gesture = Some(AnnotationGesture::Move {
                        id,
                        snapshot,
                        last: point,
                    });
                }
                None => self.annotation_doc.select(None),
            }
            self.invalidate();
            return;
        }
        // Creation tool: begin a draft anchored at the press point.
        let kind = self.annotation_tool.unwrap();
        self.annotation_doc
            .start_draft(kind, Self::initial_draft_geometry(kind, point));
        self.annotation_gesture = Some(AnnotationGesture::Create { start: point });
        self.invalidate();
    }

    /// Pointer released while annotating: commit the active gesture as one undo step.
    pub(super) fn annotation_point_up(&mut self) {
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
    pub(super) fn update_annotation_cursor(&mut self, point: Point) {
        let cursor = if self.annotation_tool.is_some() {
            IDC_CROSS
        } else if self
            .annotation_doc
            .selected_id()
            .and_then(|id| {
                self.annotation_doc
                    .handle_at(id, point, self.annotation_handle_tol())
            })
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

    pub(super) fn track_mouse_leave(&self) {
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

#[cfg(test)]
mod tests {
    use super::nudge_point;
    use windows_sys::Win32::Foundation::POINT;

    #[test]
    fn cursor_nudges_move_exactly_one_screen_pixel() {
        let origin = POINT { x: 100, y: 200 };
        let same = |actual: POINT, x: i32, y: i32| assert_eq!((actual.x, actual.y), (x, y));
        same(nudge_point(origin, 0, -1), 100, 199);
        same(nudge_point(origin, 0, 1), 100, 201);
        same(nudge_point(origin, -1, 0), 99, 200);
        same(nudge_point(origin, 1, 0), 101, 200);
    }
}

/// Ctrl, Shift and Alt as they were when the message being handled was posted.
fn modifiers() -> (bool, bool, bool) {
    let down = |vk: u16| unsafe { GetKeyState(vk as i32) < 0 };
    (down(VK_CONTROL), down(VK_SHIFT), down(VK_MENU))
}

fn is_shift_vk(vk: u32) -> bool {
    vk == VK_SHIFT as u32 || vk == VK_LSHIFT as u32 || vk == VK_RSHIFT as u32
}
