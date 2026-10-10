//! User input: pointer, keyboard and annotation gestures (docs/23 D3 / T1.6.2).
//!
//! Split out of `overlay.rs`. These are the handlers that turn Windows messages into
//! controller state — they decide nothing about *how* the overlay looks (that is
//! `render_submit`) or *what* a session is (that is the parent module); they only translate.
//! `pub(super)` because the parent module's window procedure dispatches to them.

use super::*;

/// What a press does with the scroll panel (`docs/32` `P7.06`).
///
/// Three answers rather than two: "the panel handled it" and "the panel ignored it" are different,
/// and a press on the panel's padding is still a press on the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PanelPress {
    /// A live control: the action it names.
    Action(crate::scroll::panel::PanelAction),
    /// The viewport box itself: a press here starts a drag (`P7.07`), which is not one of the four
    /// button actions and moves the view rather than promising anything about the pixels.
    Viewport,
    /// On the panel, but on nothing that acts (padding, the strip outside the box, a greyed button).
    OnPanel,
    /// Not the panel: the press belongs to whatever is underneath.
    Outside,
}

impl OverlayController {
    pub(super) fn on_mouse_move(&mut self, client: POINT) {
        let point = Point::new(client.x, client.y);
        self.cursor = point;
        self.cursor_visible = true;
        // A cursor move deliberately does **not** touch the chain (docs/21 §5.24, ④): it used to, so
        // that "the hand is on its way to inspect it" would not lose the rings — but the effect was
        // that a user who kept moving the mouse never saw them fade, which is the disappearance this
        // exists for. Walks and new answers are the triggers now, so "scrolling stopped" really does
        // mean the chain goes away.

        // A drag of the viewport box owns the pointer until the button comes up (`P7.07`): moving the
        // view is not a selection gesture, and letting the hover path run underneath would drag the
        // capture selection around while the user is only moving the scroll view.
        if self.viewport_grab.is_some() && self.drag_viewport_to(point) {
            self.invalidate();
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
                    self.session.pointer_moved(point);
                    self.disarm_dwell();
                }
                MoveOutcome::Dragging => {
                    self.session.pointer_moved(point);
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
        let cursor = if !self.session.has_selection() {
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
        // A running scroll session owns the panel's controls (`docs/32` `P7.06`). The press is
        // resolved *before* the selection logic, because the overlay still holds the capture
        // session's selection underneath the panel and a press on `停止` must not also start a drag.
        if self.scroll_runtime.is_some() {
            match self.scroll_panel_press(point) {
                PanelPress::Action(action) => {
                    eprintln!(
                        "[snapclip][capture] scroll panel press point=({},{}) action={action:?}",
                        point.x, point.y
                    );
                    self.apply_panel_action(action);
                    self.invalidate();
                    return;
                }
                // On the panel but not on a live control (the padding, the strip outside the box, a
                // greyed button): still not the canvas. Falling through would let a press beside a
                // button start a selection drag underneath the panel.
                PanelPress::Viewport => {
                    self.begin_viewport_drag(point);
                    self.invalidate();
                    return;
                }
                PanelPress::OnPanel => return,
                PanelPress::Outside => {}
            }
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
            self.session_id(), point.x, point.y, hit, outcome
        );
        self.update_cursor_shape(point);
        self.invalidate();
    }

    /// Whether a scroll-session hit at `point` (client pixels) belongs to the panel rather than to
    /// the content underneath it (`docs/32` `P7.07b`).
    ///
    /// One hit test with two consumers — the press path and `WM_NCHITTEST` — so that "where the panel
    /// is" cannot drift between "what a click does" and "what receives the wheel".
    pub(super) fn scroll_panel_owns(&self, point: Point) -> bool {
        !matches!(self.scroll_panel_press(point), PanelPress::Outside)
    }

    /// The action a press at `point` (client pixels) performs on the scroll panel.
    ///
    /// Four answers, because "the panel handled it", "the box was grabbed" and "the panel ignored it"
    /// are three different things: a press on the panel's padding is still a press on the panel, and a
    /// press on the viewport box is neither a button nor nothing.
    fn scroll_panel_press(&self, point: Point) -> PanelPress {
        let Some(panel) = self.scroll_panel.as_ref() else {
            return PanelPress::Outside;
        };
        let Some((layout, local)) = self.panel_local(point) else {
            return PanelPress::Outside;
        };
        match layout.hit_test(local) {
            crate::scroll::panel::PanelHit::Button(index) => {
                match crate::scroll::panel::actions(&panel.answers())[index] {
                    Some(action) => PanelPress::Action(action),
                    // Drawn greyed: the press lands on the panel and does nothing.
                    None => PanelPress::OnPanel,
                }
            }
            // The strip's **viewport box** is the drag (`P7.07`); the layout can hand out the strip but
            // not the box, because where the box sits depends on where the session has got to.
            crate::scroll::panel::PanelHit::Strip => {
                if panel.viewport_box(layout.strip).contains(local) {
                    PanelPress::Viewport
                } else {
                    PanelPress::OnPanel
                }
            }
            crate::scroll::panel::PanelHit::Outside => PanelPress::Outside,
        }
    }

    /// The panel's placement and the point in the layout's own DIP space (`P7.06`/`P7.07`).
    ///
    /// One conversion for the press, the drag and the hit test: the panel's rectangles are a DIP
    /// layout, the pointer arrives in pixels, and `panel::to_dip_point` is the renderer's own scale
    /// applied backwards.
    fn panel_local(&self, point: Point) -> Option<(crate::scroll::panel::PanelLayout, Point)> {
        let geometry = self.session.geometry()?;
        let (frame, layout) = crate::windows::win::d2d::scroll_panel_rect(
            geometry.work_area,
            geometry.frame,
            geometry.dpi,
        );
        let local = crate::scroll::panel::to_dip_point(
            point,
            Point::new(frame.left, frame.top),
            geometry.dpi,
        );
        Some((layout, local))
    }

    /// Grab the viewport box: remember where, and stop following (`P7.07`).
    ///
    /// `set_follow(false)` is the promise §19.5 makes about a drag — touching the box is the user saying
    /// "I am looking somewhere else", and a session that kept following would yank the view back to the
    /// newest band on the very next step.
    fn begin_viewport_drag(&mut self, point: Point) {
        let Some((_, local)) = self.panel_local(point) else {
            return;
        };
        let Some(panel) = self.scroll_panel.as_ref() else {
            return;
        };
        let snapshot = panel.clone();
        let band = panel.answers().capture_at;
        // The panel as it is **now** travels with the drag: `band_for_drag` is a change relative to the
        // position it is asked about, and the live panel only catches up when the driver publishes.
        self.viewport_grab = Some((local.y, snapshot));
        if let Some(runtime) = self.scroll_runtime.as_ref() {
            runtime.controller().set_follow(false);
        }
        self.request_strip_window();
        eprintln!(
            "[snapclip][capture] viewport drag begin band={band} grab_y={}",
            local.y
        );
    }

    /// Move the view to wherever the pointer is. `false` when no drag is in flight.
    fn drag_viewport_to(&mut self, point: Point) -> bool {
        let Some((grab_y, snapshot)) = self.viewport_grab.as_ref() else {
            return false;
        };
        let (grab_y, snapshot) = (*grab_y, snapshot.clone());
        let Some((layout, local)) = self.panel_local(point) else {
            return false;
        };
        let band = snapshot.band_for_drag(layout.strip, grab_y, local.y);
        if let Some(runtime) = self.scroll_runtime.as_ref() {
            runtime.controller().set_band(band);
        }
        self.request_strip_window();
        true
    }

    /// Ask the driver to derive the window the strip is showing (`docs/32` ADR-20).
    ///
    /// The panel is the half that knows which part of the canvas it is a picture of, and the driver is
    /// the half that owns the canvas — so the strip's pixels exist only if somebody asks.
    fn request_strip_window(&mut self) {
        let Some(panel) = self.scroll_panel.as_ref() else {
            return;
        };
        let (first_row, rows) = panel.strip_window();
        if let Some(runtime) = self.scroll_runtime.as_ref() {
            runtime
                .controller()
                .request_window(crate::scroll::session::WindowRequest { first_row, rows });
        }
    }

    pub(super) fn on_left_up(&mut self) {
        // The end of a viewport drag is the end of that gesture and nothing else: no selection was
        // being made, so nothing has to be committed or discarded (`P7.07`).
        if self.viewport_grab.take().is_some() {
            eprintln!("[snapclip][capture] viewport drag end");
            self.invalidate();
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

    pub(super) fn on_key_down(&mut self, key: u32) {
        let state = self.session.state();
        let is_active = state.is_active();
        let has_selection = self.session.has_selection();
        self.metrics
            .log_line(&format!("key down vk=0x{key:02X} state={state:?}"), false);
        match key {
            hotkey::ESCAPE_VIRTUAL_KEY => self.cancel("escape"),
            // Up/Down: step the deep-selection level (docs/21 §5.17). Up walks toward the window
            // frame, down back toward the box the refinement published.
            k if k == 0x26 => {
                self.step_deep_level(-1);
            }
            k if k == 0x28 => {
                self.step_deep_level(1);
            }
            hotkey::RETURN_VIRTUAL_KEY => {
                // A snap preview is confirmed through the detection worker — never
                // validated synchronously on the overlay thread. A settled selection goes
                // straight to the export path.
                if !self.confirm_snap_preview() && has_selection {
                    self.confirm();
                }
            }
            // 'A': Selected -> Annotating (selection is locked, tools go live).
            k if k == b'A' as u32 => {
                if state == CaptureState::Selected && self.session.begin_annotating().is_ok() {
                    self.publish_state();
                    self.invalidate_all();
                }
            }
            // 'S' (0x53): cycle colour display format (HEX → RGB → HSL → HEX).
            k if k == b'S' as u32 => {
                if is_active {
                    self.magnifier_color_format = self.magnifier_color_format.next();
                    self.invalidate();
                }
            }
            // 'C': copy the current colour value (in the active format) to clipboard.
            k if k == b'C' as u32 => {
                if is_active {
                    self.copy_color_to_clipboard();
                }
            }
            // 'P': toggle global screen ↔ selection-relative coordinate in the info panel.
            k if k == b'P' as u32 => {
                if is_active {
                    self.magnifier_relative = !self.magnifier_relative;
                    self.invalidate();
                }
            }
            // 'Z': held-key gate for wheel-to-zoom. Consumed only as a mode
            // flag; the actual zoom change happens in WM_MOUSEWHEEL.
            k if k == b'Z' as u32 => {
                if is_active {
                    self.z_held = true;
                }
            }
            _ if state == CaptureState::Annotating => self.on_annotation_key(key),
            _ => {}
        }
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

    /// Keyboard handling while the session is annotating.
    pub(super) fn on_annotation_key(&mut self, key: u32) {
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
            return;
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
            _ => {}
        }
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
                self.annotation_gesture = Some(AnnotationGesture::Move { id, snapshot, last: point });
            }
            Some(AnnotationGesture::Resize { id, handle, snapshot }) => {
                self.annotation_doc.resize_item(id, handle, point);
                self.annotation_gesture = Some(AnnotationGesture::Resize { id, handle, snapshot });
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
            AnnotationKind::Line => AnnotationGeometry::Line { start: point, end: point },
            AnnotationKind::Arrow => AnnotationGeometry::Arrow { start: point, end: point },
            AnnotationKind::Freehand => AnnotationGeometry::Freehand { points: vec![point] },
            AnnotationKind::Highlight => AnnotationGeometry::Highlight { points: vec![point] },
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
                if let Some(handle) = self.annotation_doc.handle_at(id, point, self.annotation_handle_tol()) {
                    let snapshot = self.annotation_doc.snapshot();
                    self.annotation_gesture = Some(AnnotationGesture::Resize { id, handle, snapshot });
                    self.invalidate();
                    return;
                }
            }
            match self.annotation_doc.hit_test(point) {
                Some(id) => {
                    let snapshot = self.annotation_doc.snapshot();
                    self.annotation_doc.select(Some(id));
                    self.annotation_gesture = Some(AnnotationGesture::Move { id, snapshot, last: point });
                }
                None => self.annotation_doc.select(None),
            }
            self.invalidate();
            return;
        }
        // Creation tool: begin a draft anchored at the press point.
        let kind = self.annotation_tool.unwrap();
        self.annotation_doc.start_draft(kind, Self::initial_draft_geometry(kind, point));
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
            .and_then(|id| self.annotation_doc.handle_at(id, point, self.annotation_handle_tol()))
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
