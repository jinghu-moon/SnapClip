//! User input: pointer, keyboard and annotation gestures (docs/23 D3 / T1.6.2).
//!
//! Split out of `overlay.rs`. These are the handlers that turn Windows messages into
//! controller state — they decide nothing about *how* the overlay looks (that is
//! `render_submit`) or *what* a session is (that is the parent module); they only translate.
//! `pub(super)` because the parent module's window procedure dispatches to them.

use super::*;

use crate::geometry::{region_panel_layout, RegionPanelLayout, RegionPanelTarget};
use crate::region::{Delay, Destination, PanelChange, PanelKey, Region};
use crate::units::LengthUnit;

/// The keys the custom-region panel knows (docs/23 §14.54).
///
/// Anything else is not "unknown" but "not the panel's": while it is open the keyboard belongs to
/// it entirely, because its fields are text and the session's chords would fire on letters the user
/// meant as numbers or as nothing at all.
fn panel_key(vk: u32, shift: bool) -> Option<PanelKey> {
    const VK_BACK: u32 = 0x08;
    const VK_END: u32 = 0x23;
    const VK_HOME: u32 = 0x24;
    const VK_DELETE: u32 = 0x2E;
    const VK_TAB: u32 = 0x09;
    const VK_LEFT: u32 = 0x25;
    const VK_UP: u32 = 0x26;
    const VK_RIGHT: u32 = 0x27;
    const VK_DOWN: u32 = 0x28;
    match vk {
        VK_TAB if shift => Some(PanelKey::BackTab),
        VK_TAB => Some(PanelKey::Tab),
        VK_LEFT => Some(PanelKey::Left),
        VK_UP => Some(PanelKey::Up),
        VK_RIGHT => Some(PanelKey::Right),
        VK_DOWN => Some(PanelKey::Down),
        VK_BACK => Some(PanelKey::Backspace),
        VK_DELETE => Some(PanelKey::Delete),
        VK_HOME => Some(PanelKey::Home),
        VK_END => Some(PanelKey::End),
        vk if vk == crate::windows::hotkey::ESCAPE_VIRTUAL_KEY => Some(PanelKey::Escape),
        vk if vk == crate::windows::hotkey::RETURN_VIRTUAL_KEY => Some(PanelKey::Enter),
        // 数字**不在这里**：它走 `WM_CHAR`（见 `on_char`）。两条路都收的话，一个字符会落两次。
        _ => None,
    }
}

/// 命中结果 → 光标。普通截图与倒计时 HUD 共用这一套映射，免得两处各写一遍之后走偏。
pub(super) fn selection_cursor(geometry: SelectionGeometry) -> *const u16 {
    match geometry {
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
}

/// 倒计时 HUD 里的光标：**只有边框与手柄上是改大小光标**，其余一律普通箭头 —— 那几秒浮层只在
/// "选框环"上存在，指针在别处时它已经不属于这个窗口（docs/23 §14.55.9.3）。
pub(super) fn countdown_cursor(geometry: SelectionGeometry) -> *const u16 {
    match geometry {
        SelectionGeometry::Resize(_) => selection_cursor(geometry),
        _ => IDC_ARROW,
    }
}

/// HUD 光标：拖拽中**锁定那个方向**的改大小光标（指针此时可能已经远离边框，按命中结果算会
/// 跳回箭头），不拖时按命中结果。
pub(super) fn countdown_cursor_while(
    dragging: Option<Handle>,
    geometry: SelectionGeometry,
) -> *const u16 {
    match dragging {
        Some(handle) => selection_cursor(SelectionGeometry::Resize(handle)),
        None => countdown_cursor(geometry),
    }
}

fn nudge_point(point: POINT, dx: i32, dy: i32) -> POINT {
    POINT {
        x: point.x.saturating_add(dx),
        y: point.y.saturating_add(dy),
    }
}

impl OverlayController {
    /// 把指针位置换算成半径并应用 —— 点击与拖动**共用**这一处（`radius_at` 是唯一映射）。
    fn drag_radius_to(&mut self, point: Point) {
        if let (Some(layout), Some(mut panel)) =
            (self.region_panel_layout(), self.region_panel.take())
        {
            let change = panel.set_radius(layout.radius_at(point.x), self.region_now());
            self.region_panel = Some(panel);
            self.apply_region_panel_change(change);
        }
        self.invalidate();
    }

    pub(super) fn on_mouse_move(&mut self, client: POINT) {
        // 半径滑块：按下后**一直跟着移动**改值（点击与拖动共用 `radius_at` 这一个映射）。
        // 之前这里没有分支，所以只能点、不能拖（docs/23 §14.55.20 的 Bug 1）。
        if self.radius_drag {
            self.drag_radius_to(Point::new(client.x, client.y));
            return;
        }
        let point = Point::new(client.x, client.y);
        let shift = self.shift_down || modifiers().1;
        self.cursor = point;
        self.cursor_visible = true;
        // 倒计时 HUD：拖动中把移动喂给既有的改大小手势（窗口只在环上，只有捕获才收得到）；
        // 不拖的时候只更新光标（docs/23 §14.55.9.3）。
        if self.countdown.is_some() {
            if self.countdown_dragging {
                self.session.pointer_moved_with_constraints(point, shift);
                // 区域每帧跟随：只在松手时更新的话，往外拖时框线会跑出窗口被裁掉。
                self.sync_countdown_region();
            }
            self.update_cursor_shape(point);
            self.invalidate();
            return;
        }
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
    /// 面板盖住的地方由面板决定光标样式：控件上是指手，面板内其余地方是箭头。
    ///
    /// 返回 `true` 表示这次已经设好光标，调用方不要再套截图那份样式。数字输入格按用户口径
    /// 也算"控件"⇒手指（docs/23 §14.55.6）。
    fn panel_cursor(&self, point: Point) -> bool {
        let Some(layout) = self.region_panel_layout() else {
            return false;
        };
        if !layout.panel.contains(point) {
            return false;
        }
        let on_control = matches!(
            layout.target_at(point),
            Some(RegionPanelTarget::Slot(_))
                | Some(RegionPanelTarget::Unit(_))
                | Some(RegionPanelTarget::Delay(_))
                | Some(RegionPanelTarget::MenuItem(_))
        );
        let cursor = if on_control {
            IDC_HAND
        } else {
            IDC_ARROW
        };
        unsafe { SetCursor(LoadCursorW(null_mut(), cursor) as _) };
        true
    }

    pub(super) fn update_cursor_shape(&mut self, point: Point) {
        // 倒计时 HUD：边框/手柄上是改大小光标（拖得动），其余照旧是普通箭头（docs/23 §14.55.9.3）。
        if self.countdown.is_some() {
            let cursor =
                countdown_cursor_while(self.countdown_handle, self.session.pointer_hit(point));
            unsafe { SetCursor(LoadCursorW(null_mut(), cursor) as _) };
            return;
        }
        if self.panel_cursor(point) {
            return;
        }
        let cursor = if self.scroll.is_some() {
            IDC_ARROW
        } else if !self.session.has_selection() {
            IDC_CROSS
        } else {
            selection_cursor(self.session.pointer_hit(point))
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
        // 倒计时是等待中的 HUD：**只允许拖边框/手柄改大小** —— 选区已经定了，这时候再起框选
        // 或移动选区都不是用户的意思（docs/23 §14.55.9.3）。
        if self.countdown.is_some() {
            self.begin_countdown_resize(point);
            return;
        }
        // The custom-region panel is the topmost layer: a press inside it belongs to it, and its
        // blank space swallows the press instead of starting a drag behind it. A press *outside* it
        // is the ordinary gesture — dragging the region with the panel open is how the two halves
        // work together (docs/23 §14.54).
        if self.radius_drag {
            self.drag_radius_to(point);
            self.update_cursor_shape(point);
            self.invalidate();
            return;
        }
        if self.region_panel.is_some() {
            if let Some(panel) = self.region_panel_rect() {
                if panel.contains(point) {
                    self.chrome_swallowed_press = true;
                    self.on_region_panel_press(point);
                    return;
                }
            }
        }
        // 尺寸栏上的按下：开关精确面板（docs/23 §14.54）。悬停只把它点亮，点才开面板。
        if self.label_badge.is_some_and(|badge| badge.contains(point)) {
            self.chrome_swallowed_press = true;
            self.toggle_region_panel();
            return;
        }
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
        if self.radius_drag {
            // 滑块松手：结束拖动，别让这次释放走成一次确认。
            self.radius_drag = false;
            return;
        }
        if self.countdown.is_some() {
            self.end_countdown_resize();
            return;
        }
        // A press overlay chrome took is not the gesture's: swallowing the release with it is what
        // keeps a click on the badge or in the panel from committing the selection a moment later.
        if self.chrome_swallowed_press {
            self.chrome_swallowed_press = false;
            return;
        }
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
        // 倒计时期间只认 Esc（取消）：等待的那几秒里本应用可能仍是前台窗口，Enter/S 之类
        // 会把这次截图提前做掉或改掉设置（docs/23 §14.55.9）。
        if self.countdown.is_some() && Key::from_virtual_key(vk) != Some(Key::Escape) {
            return false;
        }
        if is_shift_vk(vk) {
            self.shift_down = true;
            self.on_snap_modifier_changed();
            return state.is_active();
        }
        // The panel is a form that is being typed into: while it is open every key belongs to it, or
        // the session's chords (`S`, `B`, `H`…) would fire on the letters the user meant as text
        // (docs/23 §14.54).
        if self.region_panel.is_some() {
            let shift = self.shift_down || modifiers().1;
            if let Some(key) = panel_key(vk, shift) {
                self.on_region_panel_key(key);
            }
            return true;
        }
        // 倒计时期间 Esc = 取消这次延时：停表、把鼠标还回来、不抓帧；选区还在，用户想截可以照常截
        // （docs/23 §14.55.9）。
        if self.countdown.is_some() && vk == crate::windows::hotkey::ESCAPE_VIRTUAL_KEY {
            // 取消是"回到普通截图状态"：窗口继续可见，所以顺序里没有 Hide 那一步，
            // 最后重画一帧普通画面（docs/23 §14.55.9.5）。
            self.leave_countdown(super::session::CountdownExit::Cancel);
            self.pending_region = None;
            self.delay_destination = None;
            return true;
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
            // The panel's entry must not depend on the size label's layout (docs/46): with all
            // three readouts off there is no badge to click, and the panel is where those
            // readouts are switched back on. Availability is the context plus a live selection.
            Action::OpenRegionPanel => {
                if Self::panel_entry_available(state, self.session.has_selection()) {
                    self.open_region_panel();
                    self.invalidate();
                }
            }
            // Ctrl+S arms the save destination. The decision is the same predicate the region
            // panel uses, so it depends on the context and a live selection and never on the
            // label badge (docs/46, docs/42 D11). The handoff itself (hiding the frame, moving
            // the selected pixels into the panel) lands with the delivery batch.
            Action::OpenSavePanel => {
                if Self::panel_entry_available(state, self.session.has_selection()) {
                    // Arms the save panel for the stage that owns it. Hiding the overlay here
                    // would be wrong: the frame must leave through the session exit sequence
                    // (`ExitStep::Hide`), which also restores the window region and the dim
                    // layer. A direct `hide_overlay` from an action handler skips those steps.
                    self.pending_save = true;
                    self.invalidate();
                    // The handoff belongs to the session, not to the action handler:
                    // `request_save` reads the region back, tells the sink, and leaves through
                    // `finish_session`, so the dim layer, the countdown and the window region
                    // all unwind in the documented order.
                    self.request_save();
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

    /// The panel's own rectangle, from the same layout the paint pass and the hit test use.
    fn region_panel_rect(&self) -> Option<Rect> {
        self.region_panel_layout().map(|layout| layout.panel)
    }

    /// 点尺寸栏：打开或收起精确面板。
    ///
    /// 面板的校验范围就是本屏（`monitor_bounds`），最小选区与会话用同一条规则（`minimum_selection`）
    /// —— 面板的描红和会话的 `snap_to` 因此不会各说一套（docs/23 §14.54）。
    /// Whether the panel can be opened at all. Deliberately independent of the size label: it
    /// used to be reachable only through the badge, and with all three readouts off there is no
    /// badge, which locked the panel — and the switches that turn the readouts back on — away
    /// (docs/45, docs/46). Context plus a live selection is the whole condition.
    fn panel_entry_available(state: CaptureState, has_selection: bool) -> bool {
        matches!(state, CaptureState::Selecting | CaptureState::Selected) && has_selection
    }

    /// Opens the panel if it is closed. Idempotent: an already open panel is left alone, because
    /// Escape is the close gesture and one key must not mean two opposite things (docs/46).
    fn open_region_panel(&mut self) {
        if self.region_panel.is_some() {
            return;
        }
        self.toggle_region_panel();
    }

    fn toggle_region_panel(&mut self) {
        if self.region_panel.is_none() {
            // 打开面板时量一次分段器选项宽度 —— 在帧之外（量宽/建文本布局不需要 BeginDraw），
            // 且只量一次：绘制与命中此后都读 `self.segment_widths` 这一份（docs/23 §14.55.6）。
            if let Some(renderer) = self.renderer.as_mut() {
                match renderer.segment_widths() {
                    Ok(widths) => self.segment_widths = widths,
                    Err(error) => {
                        eprintln!("[snapclip][capture] segment widths unavailable: {error}")
                    }
                }
            }
        }
        self.region_panel = if self.region_panel.is_some() {
            None
        } else {
            Some(crate::region::RegionPanel::new(
                self.preferences.label.length_unit,
                crate::region::Delay::ALL[0],
                self.monitor_bounds(),
                self.minimum_selection(),
            ))
        };
        // IME 跟着面板的寿命走：开着就摘掉（数字才不会被输入法吃掉），关了就还回去（docs/23 §14.55）。
        if self.region_panel.is_some() {
            self.detach_ime_for_panel();
        } else {
            self.restore_ime_for_panel();
        }
        self.invalidate();
    }

    /// 本屏在桌面上的矩形：面板的校验与"居中"都在这个范围里。
    fn monitor_bounds(&self) -> Rect {
        self.renderer
            .as_ref()
            .map(|renderer| renderer.layout().bounds)
            .unwrap_or_default()
    }

    /// 单位只有**一份真相**：面板切了 dp/px 就写回偏好，角标读的正是偏好。
    ///
    /// 以前面板的 `unit` 是自己的状态、偏好里另有一份，于是面板切单位只改了面板里的数字，
    /// 选框左上角的角标仍按旧单位显示——用户报的"角标不跟随面板"（docs/23 §14.55.6）。
    fn sync_panel_unit(&mut self) {
        let Some(unit) = self.region_panel.as_ref().map(|panel| panel.unit()) else {
            return;
        };
        if self.preferences.label.length_unit == unit {
            return;
        }
        self.preferences.label.length_unit = unit;
        self.publish_preferences();
        self.invalidate();
    }

    /// 会话能接受的最小选区：面板描红用的是同一条规则，不另说一套。
    fn minimum_selection(&self) -> i32 {
        crate::geometry::SelectionSnapshot::new(Rect::default(), self.session.dpi())
            .minimum_size()
    }

    /// The panel's layout as of this moment (docs/23 §14.54).
    ///
    /// Recomputed rather than cached: it is a pure function of the display scale, the work area and
    /// whether the dropdown is open, and a cache would be one more thing that can lag the drawing.
    fn region_panel_layout(&self) -> Option<RegionPanelLayout> {
        let panel = self.region_panel.as_ref()?;
        let renderer = self.renderer.as_ref()?;
        Some(region_panel_layout(
            self.session.dpi() as f32 / 96.0,
            renderer.layout().local_work_area(),
            self.session.selection(),
            panel.menu().is_some(),
            crate::geometry::PanelCard {
                open: panel.image_open(),
                radius_px: panel.radius_px() as i32,
            },
            self.segment_widths,
        ))
    }

    /// Where this monitor's origin sits on the desktop: the one place the panel's numbers and the
    /// session's selection are converted between desktop pixels and this monitor's pixels.
    pub(super) fn monitor_origin(&self) -> Point {
        self.renderer
            .as_ref()
            .map(|renderer| {
                let bounds = renderer.layout().bounds;
                Point::new(bounds.left, bounds.top)
            })
            .unwrap_or_default()
    }

    /// The region the panel is showing right now: the session's selection in desktop pixels.
    fn region_now(&self) -> Region {
        Region::from_rect(self.session.selection(), self.monitor_origin())
    }

    /// A press inside the panel: whatever it landed on acts, blank space does nothing.
    fn on_region_panel_press(&mut self, point: Point) {
        let Some(layout) = self.region_panel_layout() else {
            return;
        };
        let dpi = self.session.dpi();
        let region = self.region_now();
        // 分段器换了选项就起一次平移（快照前后值，谁变了给谁起，点选与键盘同一条路）。
        let before = self.region_panel.as_ref().map(|panel| (panel.unit(), panel.delay()));
        let change = match layout.target_at(point) {
            None | Some(RegionPanelTarget::MenuPadding) => return,
            Some(RegionPanelTarget::Unit(unit)) => {
                if let Some(panel) = self.region_panel.as_mut() {
                    panel.set_unit(unit);
                }
                PanelChange::Repaint
            }
            Some(RegionPanelTarget::Delay(delay)) => {
                // 点哪段选哪段（与 hover 的逐段语义一致）。
                if let Some(panel) = self.region_panel.as_mut() {
                    panel.choose_delay(delay);
                }
                PanelChange::Repaint
            }
            Some(RegionPanelTarget::Radius(value)) => {
                // 按下即生效，并进入拖动：继续移动（哪怕移出轨道）都按同一套映射改值。
                self.radius_drag = true;
                self.region_panel
                    .as_mut()
                    .map(|panel| panel.set_radius(value, region))
                    .unwrap_or(PanelChange::None)
            }
            Some(RegionPanelTarget::Destination(destination)) => {
                // 点哪段选哪段，与延时同一条语义。
                if let Some(panel) = self.region_panel.as_mut() {
                    panel.choose_destination(destination);
                }
                PanelChange::Repaint
            }
            Some(RegionPanelTarget::MenuItem(index)) => {
                let Some(mut panel) = self.region_panel.take() else {
                    return;
                };
                let change = panel.pick(index, region, dpi);
                self.region_panel = Some(panel);
                change
            }
            Some(RegionPanelTarget::Slot(slot)) => {
                let Some(mut panel) = self.region_panel.take() else {
                    return;
                };
                let change = panel.click(slot, region, dpi);
                self.region_panel = Some(panel);
                change
            }
        };
        if let Some((unit, delay)) = before {
            self.start_segment_slides(unit, delay);
        }
        self.apply_region_panel_change(change);
        self.sync_panel_unit();
    }

    /// 分段器换了选项就起一次平移：`from` 由段号给出，插值在绘制端做（docs/23 §14.55.5）。
    fn start_segment_slides(&mut self, before_unit: LengthUnit, before_delay: Delay) {
        let Some(panel) = self.region_panel.as_ref() else {
            return;
        };
        let now = Instant::now();
        if before_unit != panel.unit() {
            self.unit_slide.start(panel.unit().index(), now);
        }
        if before_delay != panel.delay() {
            self.delay_slide.start(panel.delay().index(), now);
        }
    }

    /// A key press while the panel has the keyboard.
    /// 字符入口：`WM_CHAR` 送来的字，中文输入法组合结束后送来的也是字符。
    ///
    /// 数字只从这里入账（`panel_key` 不再收 VK 数字，否则同一个字符会落两次）。这里只做字符→按键的
    /// 归一化（中文输入法常送全角 `１`）；数字格的规则——只数字、7 位以内——仍在 `RegionPanel` 里。
    /// docs/23 §14.55。
    pub(super) fn on_char(&mut self, character: char) -> bool {
        if self.region_panel.is_none() {
            return false;
        }
        let Some(key) = crate::region::panel_key_from_char(character) else {
            return false;
        };
        self.on_region_panel_key(key);
        true
    }

    fn on_region_panel_key(&mut self, key: PanelKey) {
        let dpi = self.session.dpi();
        let region = self.region_now();
        let Some(mut panel) = self.region_panel.take() else {
            return;
        };
        let before = (panel.unit(), panel.delay());
        let change = panel.key(key, region, dpi);
        self.region_panel = Some(panel);
        self.start_segment_slides(before.0, before.1);
        self.apply_region_panel_change(change);
        self.sync_panel_unit();
    }

    /// Hand this session's display preferences to the shared slot: "use these from now on".
    ///
    /// The custom-region panel is the one editor of those settings inside a session, and this is its
    /// only door out: the slot is what the next session reads and what the shell writes to disk when
    /// the capture ends (docs/23 §14.54).
    pub(super) fn publish_preferences(&mut self) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.preferences = self.preferences.clone();
        }
    }

    /// Turn one decision of the panel into session state: the selection, a display switch, the
    /// capture itself, or the end of the session.
    fn apply_region_panel_change(&mut self, change: PanelChange) {
        // 聚焦、敲键、点选之后立刻让光标可见（像所有文本框那样），别在"暗"的半个周期里打字。
        self.caret_visible = true;
        match change {
            PanelChange::None => {}
            PanelChange::Repaint => self.invalidate(),
            PanelChange::Toggle(toggle) => {
                let label = self.preferences.label;
                self.preferences.label = label.set(toggle, !toggle.on(label));
                self.publish_preferences();
                self.invalidate();
            }
            PanelChange::Radius(radius) => {
                // 圆角同时作用于成图（遮罩）与截图框本身（弧 + 手柄位置），所以会话也要拿到它，
                // 否则命中判定还按直角算、与看到的圆角对不上（docs/23 §14.55.21）。
                self.corner_radius_px = radius;
                self.session.set_corner_radius(radius as i32);
                self.invalidate();
            }
            PanelChange::Region(region) => {
                self.apply_region_from_panel(region);
                self.invalidate();
            }
            PanelChange::Confirm(region) => {
                self.apply_region_from_panel(region);
                let delay = self
                    .region_panel
                    .as_ref()
                    .map(|panel| panel.delay())
                    .unwrap_or(Delay::ALL[0]);
                // 面板马上要被收掉，先把去向取出来（延时要按它执行，docs/23 §14.55.15）。
                let destination = self
                    .region_panel
                    .as_ref()
                    .map(|panel| panel.destination())
                    .unwrap_or(Destination::Copy);
                if delay.seconds() > 0 {
                    // 延时：浮层先收起来，等到点再重新抓一帧 —— 冻结帧必须是等待之后的那一帧，
                    // 否则"延时"只是把同一个画面晚点截出来。区域跟着那次会话走。
                    self.region_panel = None;
                    // 面板没了：把 IME 还给这个窗口（docs/23 §14.55）。
                    self.restore_ime_for_panel();
                    self.pending_region = Some(region);
                    // 延时到点后按**面板里选的那个去向**执行（复制 / 贴图），不再写死贴图。
                    self.delay_destination = Some(destination);
                    // 浮层留着，在选区正中倒数；到点再收浮层、抓"等待之后那一帧"并贴图
                    // （docs/23 §14.55.9）。等待期间鼠标穿过浮层，用户可以去操作别的窗口。
                    self.arm_countdown(delay.seconds());
                    self.invalidate();
                    return;
                }
                if self.session.has_selection() {
                    self.confirm();
                } else {
                    // 区域本身不合法：面板已经把那句话写在页脚上，这里不另说一套。
                    self.invalidate();
                }
            }
            PanelChange::Cancel => {
                self.region_panel = None;
                self.restore_ime_for_panel();
                self.cancel("region-panel");
            }
        }
    }

    /// Put a region the panel produced onto the session's selection.
    ///
    /// The one conversion between the two coordinate systems: the panel speaks desktop pixels
    /// (what the badge and the artifact speak), the session stores this monitor's pixels.
    fn apply_region_from_panel(&mut self, region: Region) {
        if !self.session.snap_to(region.to_rect(self.monitor_origin())) {
            // The session refused it (too small, or not in a state that takes a selection): the
            // panel marks the same cells red from the same rule, so nothing more is said here.
            return;
        }
        self.publish_state();
    }

    /// 倒计时 HUD 里按下：只接受"边框/手柄改大小"，并抓住鼠标。
    ///
    /// 命中判定与几何都复用普通状态那一套（`hit_test` 对边框与手柄都答 `Resize`，
    /// `CaptureSession::begin_drag` 是同一个手势机），这里只决定"允许哪一种"。
    fn begin_countdown_resize(&mut self, point: Point) {
        let SelectionGeometry::Resize(handle) = self.session.pointer_hit(point) else {
            return;
        };
        if !self.session.begin_drag(point, ResizeMode::Handle(handle), false) {
            return;
        }
        // 唯一的 `SetCapture`（docs/23 §14.55.9.3）：HUD 的窗口只在选框环上存在，往外拖时指针
        // 立刻离开窗口，不捕获就收不到 `WM_MOUSEMOVE`，拖动会中途断掉。
        unsafe { SetCapture(self.window) };
        self.countdown_dragging = true;
        self.countdown_handle = Some(handle);
        self.update_cursor_shape(point);
        self.invalidate();
    }

    /// 松开：结束手势，并把改过的大小同时写回"到点贴图的那份区域"与 HUD 的窗口区域。
    fn end_countdown_resize(&mut self) {
        if !self.countdown_dragging {
            return;
        }
        self.release_countdown_capture();
        self.session.pointer_released();
        self.sync_countdown_targets();
        self.publish_state();
        self.invalidate();
    }

    /// 释放捕获并清掉手势（幂等）。松手、Esc、会话被取消/失败都经 `release_session` 走到这里。
    pub(super) fn release_countdown_capture(&mut self) {
        if !self.countdown_dragging {
            return;
        }
        self.countdown_dragging = false;
        self.countdown_handle = None;
        unsafe { ReleaseCapture() };
        self.session.pointer_left();
    }

    /// 松手时：贴图区域与窗口区域一起跟随当前选区。
    ///
    /// **贴图区域不跟着更新，用户改了框也没用** —— 到点贴的会是点「确定」那一刻的区域
    /// （docs/23 §14.55.9.3）。
    fn sync_countdown_targets(&mut self) {
        if self.countdown.is_none() {
            return;
        }
        let (pin, pieces) = Self::countdown_after_resize(
            self.session.selection(),
            self.monitor_origin(),
            self.label_badge,
            self.session.dpi(),
        );
        if pin.is_some() {
            self.pending_region = pin;
        }
        self.set_window_region(&pieces);
    }

    /// 拖动过程中：窗口区域跟着新选区走，否则框线会跑到窗口外面被裁掉。
    fn sync_countdown_region(&mut self) {
        if self.countdown.is_none() {
            return;
        }
        self.update_countdown_dim();
        let pieces = super::session::countdown_window_pieces(
            self.session.selection(),
            self.label_badge,
            self.session.dpi(),
            i32::try_from(self.corner_radius_px).unwrap_or(0),
        );
        self.set_window_region(&pieces);
    }
}

#[cfg(test)]
mod tests {
    use super::{nudge_point, OverlayController};
    use crate::keymap::{Chord, Context, Key, Keymap};
    use crate::CaptureState;
    use crate::units::LabelDisplay;
    use windows_sys::Win32::Foundation::POINT;

    // ── Controller fixture ────────────────────────────────────────────────────────────────
    //
    // `docs/41`: the `Ctrl+S` branch sets `pending_save`, and nothing tested that line, because
    // reaching it needed a controller and the controller needed nine collaborators. The
    // collaborators below are the smallest ones that let an action actually run; two of them
    // are trait objects whose real implementations live in the shell.

    /// A clipboard writer that copies nothing. A fixture test never copies, and a stand-in that
    /// silently succeeded would hide a test that had wandered into the wrong path.
    // Not used yet on purpose: the fixture exists so the next stage can write the assertions
    // that reach the `Ctrl+S` branch, and a stand-in with no caller is a warning the compiler
    // is right to raise.
    struct TestClipboard;

    impl crate::ports::ClipboardWriter for TestClipboard {
        fn copy_text(&self, _text: &str) {}

        fn copy_image(
            &self,
            _width: u32,
            _height: u32,
            _bgra: &[u8],
            _png: Option<&std::path::Path>,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    /// An artifact writer that refuses to write. Reaching it from a fixture test means the test
    /// is exercising the export path, which is not what the fixture is for, so it panics rather
    /// than pretending to have produced a file.
    struct TestWriter;

    impl crate::ports::ArtifactWriter for TestWriter {
        fn write(
            &self,
            _session_id: &str,
            _prepared: &crate::artifact::SelectionPixels,
            _dpi: u32,
            _monitor_device_name: Option<String>,
        ) -> crate::CaptureResult<snapclip_model::CaptureArtifact> {
            unimplemented!("the controller fixture does not write artifacts")
        }

        fn write_bands(
            &self,
            _session_id: &str,
            _spec: &crate::ports::SelectionSpec,
            _bands: &mut dyn crate::ports::BandSource,
            _dpi: u32,
            _monitor_device_name: Option<String>,
        ) -> crate::CaptureResult<snapclip_model::CaptureArtifact> {
            unimplemented!("the controller fixture does not write artifacts")
        }
    }

    /// The smallest controller that can run an action. It owns no window and no frame; the
    /// window handle is null because no action taken here touches the window, and the session
    /// starts idle with no selection.
    fn controller_for_tests() -> OverlayController {
        controller_for_tests_with_sink(std::sync::Arc::new(crate::ports::NullEventSink))
    }

    /// The same fixture with a sink the test can read back. `Ctrl+S` ends the session, so the
    /// only way to observe what it handed over is to record it as it arrives.
    fn controller_for_tests_with_sink(
        sink: std::sync::Arc<dyn crate::ports::CaptureEventSink>,
    ) -> OverlayController {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(super::OverlayShared {
            state: CaptureState::Idle,
            session_id: None,
            preferences: Default::default(),
            selection: None,
        }));
        // The sender is dropped at the end of this function, which is what a controller with no
        // annotation surface would see.
        let (_annotation_tx, annotation_rx) =
            std::sync::mpsc::sync_channel::<crate::annotation::AnnotationCommand>(1);
        OverlayController::new(
            std::sync::Arc::new(crate::artifact::CaptureService::new()),
            sink,
            std::sync::Arc::new(TestClipboard),
            std::sync::Arc::new(TestWriter),
            Default::default(),
            shared,
            annotation_rx,
            // `windows-sys` types `HWND` as a pointer alias, so a null pointer is the value.
            std::ptr::null_mut(),
            0,
        )
    }

    /// A monitor layout and a frame, copied in shape from the session's own tests, because
    /// driving a session to `Selecting` needs both.
    fn layout() -> crate::geometry::MonitorLayout {
        crate::geometry::MonitorLayout {
            bounds: crate::geometry::Rect::new(0, 0, 1920, 1080),
            work_area: crate::geometry::Rect::new(0, 0, 1920, 1040),
            dpi: 96,
            primary: true,
        }
    }

    fn frame() -> crate::session::CapturedFrame {
        crate::session::CapturedFrame {
            width: 1920,
            height: 1080,
            pixel_format: snapclip_model::PixelFormat::Bgra8Unorm,
            captured_at_unix_ms: 1_700_000_000_000,
            provider: "test",
            origin: Some(crate::geometry::Point::new(0, 0)),
        }
    }

    /// `Idle -> Armed -> Selecting`, the state `press` and `snap_to` require before they do
    /// anything at all. Without this the fixture sits idle and the action under test never runs.
    fn armed_session() -> crate::session::CaptureSession {
        let mut session = crate::session::CaptureSession::new("fixture");
        session
            .arm(frame(), &layout())
            .expect("a fresh session arms");
        session
            .overlay_ready()
            .expect("an armed session can be shown");
        session
    }

    /// Gives the fixture a real selection by driving the session the way its own tests do, so
    /// the branch that arms the save panel is reached through the product's own state.
    fn controller_with_a_selection() -> OverlayController {
        let mut controller = controller_for_tests();
        // Selecting first: both calls below are no-ops while the session is idle, which is
        // exactly why the earlier version of this fixture could not reach the branch it was
        // written for, and why the test above it had to be ignored.
        controller.session = armed_session();
        let point = crate::geometry::Point { x: 10, y: 10 };
        controller.session.press(point);
        controller.session.snap_to(crate::geometry::Rect {
            left: 10,
            top: 10,
            right: 110,
            bottom: 110,
        });
        controller
    }

    /// Records what the capture side hands to the shell, because `Ctrl+S` ends the session and
    /// there is nothing left to inspect afterwards. Only the size is kept: the pixels are not
    /// `Clone` and the assertions here are about the handoff happening at all, with the right
    /// region, and once.
    #[derive(Default)]
    struct RecordingSink {
        saves: std::sync::Mutex<Vec<(String, i32, i32)>>,
    }

    impl crate::ports::CaptureEventSink for RecordingSink {
        fn on_started(&self, _: &str, _: &crate::geometry::MonitorLayout) {}
        fn on_state(&self, _: &str, _: CaptureState, _: Option<&crate::geometry::MonitorLayout>) {}
        fn on_completed(&self, _: &snapclip_model::CaptureArtifact) {}
        fn on_cancelled(&self, _: &str, _: &str) {}
        fn on_failed(&self, _: Option<&str>, _: &crate::CaptureError, _: &str) {}

        fn on_save_requested(&self, session_id: &str, pixels: &crate::artifact::SelectionPixels) {
            if let Ok(mut saves) = self.saves.lock() {
                saves.push((
                    session_id.to_string(),
                    pixels.region.width(),
                    pixels.region.height(),
                ));
            }
        }
    }

    /// A controller with a live selection and a sink the test can read.
    fn controller_with_a_selection_and_sink(
        sink: std::sync::Arc<RecordingSink>,
    ) -> OverlayController {
        let mut controller = controller_for_tests_with_sink(sink);
        controller.session = armed_session();
        let point = crate::geometry::Point { x: 10, y: 10 };
        controller.session.press(point);
        controller.session.snap_to(crate::geometry::Rect {
            left: 10,
            top: 10,
            right: 110,
            bottom: 110,
        });
        controller
    }

    /// `Ctrl+S` hands the selected pixels to the shell exactly once, with the region the user
    /// dragged. Deleting the `request_save` call, or the sink call inside it, has to turn this
    /// red.
    #[test]
    fn the_save_request_carries_the_selected_pixels() {
        use crate::keymap::Action;

        let sink = std::sync::Arc::new(RecordingSink::default());
        let mut controller = controller_with_a_selection_and_sink(sink.clone());
        // The fixture has no frozen frame, so `publish_selection` cannot read one back; it
        // returns early and leaves the slot as it found it. Seeding the slot is therefore the
        // honest way to ask the question this test is about: when the pixels are there, is the
        // shell told once, with the region the user dragged?
        let region = controller.session.selection();
        let bgra = vec![0u8; (region.width() * region.height() * 4) as usize];
        if let Ok(mut shared) = controller.shared.lock() {
            shared.selection = Some(std::sync::Arc::new(crate::artifact::SelectionPixels {
                frame: frame(),
                region,
                bgra,
                origin: Some(crate::geometry::Point::new(0, 0)),
                corner_radius: 0,
            }));
        }
        controller.run_action(Action::OpenSavePanel);

        let saves = sink.saves.lock().expect("the recording sink is not poisoned");
        assert_eq!(
            saves.len(),
            1,
            "ctrl+s with a selection must ask the shell to save exactly once"
        );
        assert_eq!(
            (saves[0].1, saves[0].2),
            (100, 100),
            "the handoff must carry the region the user dragged, not an empty or default one"
        );
    }

    /// Without a selection there is nothing to hand over, so the sink stays silent. This is the
    /// half that a branch entered by accident would fail.
    #[test]
    fn no_selection_means_no_save_request() {
        use crate::keymap::Action;

        let sink = std::sync::Arc::new(RecordingSink::default());
        let mut controller = controller_for_tests_with_sink(sink.clone());
        // Idle: no selection, and `panel_entry_available` refuses, so the action is a no-op.
        controller.run_action(Action::OpenSavePanel);

        let saves = sink.saves.lock().expect("the recording sink is not poisoned");
        assert!(
            saves.is_empty(),
            "an idle session has no pixels to save and must not ask the shell"
        );
    }

    /// The guard inside `request_save` is invisible through the action path, because the entry
    /// predicate already requires a selection, so it is called directly here. The case it
    /// exists for is real: the shared slot keeps the last selection for consumers outside
    /// capture and is not cleared when a session ends, so a session with no selection of its
    /// own could otherwise hand over the previous session's picture.
    #[test]
    fn a_stale_read_back_is_not_handed_over_without_a_selection() {
        let sink = std::sync::Arc::new(RecordingSink::default());
        let mut controller = controller_for_tests_with_sink(sink.clone());
        if let Ok(mut shared) = controller.shared.lock() {
            shared.selection = Some(std::sync::Arc::new(crate::artifact::SelectionPixels {
                frame: frame(),
                region: crate::geometry::Rect {
                    left: 0,
                    top: 0,
                    right: 4,
                    bottom: 4,
                },
                bgra: vec![0u8; 4 * 4 * 4],
                origin: Some(crate::geometry::Point::new(0, 0)),
                corner_radius: 0,
            }));
        }
        // Idle, so there is no selection of this session's own, but the slot is not empty.
        controller.request_save();

        let saves = sink.saves.lock().expect("the recording sink is not poisoned");
        assert!(
            saves.is_empty(),
            "a session with no selection must not hand over the previous session's pixels"
        );
    }

    /// The line that arms the save panel. `Ctrl+S` with a selection has to set `pending_save`,
    /// and deleting that assignment has to turn this red, which is what makes the line covered
    /// rather than merely present.
    #[test]
    fn arming_the_save_panel_sets_the_flag() {
        use crate::keymap::Action;

        let mut controller = controller_with_a_selection();
        assert!(
            controller.session.has_selection(),
            "the fixture must reach the branch with a selection"
        );
        assert!(!controller.pending_save, "nothing is armed before the action runs");

        controller.run_action(Action::OpenSavePanel);

        assert!(
            controller.pending_save,
            "ctrl+s with a selection must arm the save panel"
        );
    }

    /// The other half of the same predicate: no selection means nothing happens, which is what
    /// keeps the shortcut from doing anything before a drag has happened.
    ///
    /// Honest limit: this passes because the session is `Idle` and the branch never runs, not
    /// because the branch decided to do nothing. It pins the default and nothing else.
    #[test]
    fn arming_the_save_panel_needs_a_selection() {
        use crate::keymap::Action;

        let mut controller = controller_for_tests();
        assert!(!controller.session.has_selection(), "the bare fixture has no selection");

        controller.run_action(Action::OpenSavePanel);

        assert!(
            !controller.pending_save,
            "without a selection the save panel must not be armed"
        );
    }

    /// `docs/41`: `Ctrl+S` arms the save panel. The binding, its context and its label are
    /// asserted by field name, because `Spec` is a struct and destructuring it as a tuple is
    /// what silently dropped these assertions in an earlier attempt.
    ///
    /// The label reuses the panel title's four characters, which are already in the embedded
    /// subset, so adding this action must not disturb the glyph gate.
    #[test]
    fn the_save_panel_action_is_bound_to_ctrl_s_in_the_pick_context() {
        use crate::keymap::{Action, Group};

        assert_eq!(
            Action::ALL.len(),
            21,
            "adding an action must grow ALL, which is a fixed length array"
        );

        let spec = Action::OpenSavePanel.spec();
        assert_eq!(spec.id, "panel.save");
        assert_eq!(spec.group, Group::Output);
        assert_eq!(spec.row, "panel", "the save entry shares the panel row");
        assert_eq!(spec.label, "保存图片");
        assert_eq!(spec.context, Context::Pick);
        assert_eq!(
            spec.defaults,
            &["Ctrl+S"],
            "ctrl+s is free in the pick context and means save in this product"
        );
    }

    /// `docs/46`: the entry is the context plus a live selection, and the three readout
    /// preferences must not be able to remove it. All eight combinations are exercised, because
    /// the lock-out only appeared in the one where every readout is off.
    #[test]
    fn the_panel_entry_ignores_the_size_label_preferences() {
        for size in [false, true] {
            for position in [false, true] {
                for coords in [false, true] {
                    let label = LabelDisplay {
                        show_size: size,
                        show_coordinates: position,
                        show_unit: coords,
                        ..LabelDisplay::default()
                    };
                    // The condition does not take the label at all; build it anyway so a future
                    // signature that starts taking it would have to come through here.
                    let _ = label;
                    for state in [CaptureState::Selecting, CaptureState::Selected] {
                        assert!(
                            OverlayController::panel_entry_available(state, true),
                            "state={state:?} size={size} position={position} coords={coords}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_panel_entry_needs_a_live_selection() {
        for state in [CaptureState::Idle, CaptureState::Preparing, CaptureState::Annotating] {
            assert!(!OverlayController::panel_entry_available(state, true), "{state:?}");
        }
        assert!(!OverlayController::panel_entry_available(CaptureState::Selecting, false));
    }

    #[test]
    fn ctrl_enter_opens_the_panel_in_the_pick_context() {
        let map = Keymap::default();
        let chord = Chord::key(Key::Enter, true, false, false);
        assert_eq!(
            map.action_for(Context::Pick, &chord),
            Some(crate::keymap::Action::OpenRegionPanel)
        );
    }

    #[test]
    fn enter_still_confirms_and_ctrl_enter_is_not_bound_while_scrolling() {
        let map = Keymap::default();
        let ctrl_enter = Chord::key(Key::Enter, true, false, false);
        assert_eq!(
            map.action_for(Context::Pick, &Chord::key(Key::Enter, false, false, false)),
            Some(crate::keymap::Action::Confirm)
        );
        assert_eq!(map.action_for(Context::Scroll, &ctrl_enter), None);
    }

    /// `docs/41` S2: `Ctrl+S` arms the save panel from the capture frame, and only there.
    #[test]
    fn ctrl_s_arms_the_save_panel_in_the_pick_context() {
        let map = Keymap::default();
        let ctrl_s = Chord::key(Key::Char('S'), true, false, false);
        assert_eq!(
            map.action_for(Context::Pick, &ctrl_s),
            Some(crate::keymap::Action::OpenSavePanel)
        );
        assert_eq!(map.action_for(Context::Scroll, &ctrl_s), None);
        assert_eq!(
            map.action_for(Context::Pick, &Chord::key(Key::Enter, false, false, false)),
            Some(crate::keymap::Action::Confirm)
        );
    }

    /// `ALL` is a fixed-size array, so adding a variant without widening it is a compile error
    /// today; this keeps the count honest if it ever becomes a slice. The chord itself is
    /// asserted through the keymap above, which is the behaviour that matters.
    #[test]
    fn the_save_panel_action_is_declared_once() {
        assert_eq!(crate::keymap::Action::ALL.len(), 21);
    }

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
