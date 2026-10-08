//! The two injection transports, and why they are not a fallback
//! (`docs/30` §14.3, §24.6, C6; task `P3.01`).
//!
//! # Two problems, two transports
//!
//! `SendInput` and `PostMessageW` are **not** a primary path and a retry. They
//! solve different problems, and the choice between them is made *before* the
//! first notch is fired, never after a failure (`docs/30` §24.6 rule 1):
//!
//! * `SendInput` puts a wheel event into the **system input queue**. Where that
//!   event lands is decided by the system's mouse-wheel routing rule, not by us
//!   — under `MOUSE_POS` routing it goes to whatever window is **under the
//!   cursor**. That is why this transport must place the cursor first, and why
//!   it cannot be "tried" against a target we have not aimed at.
//! * `PostMessageW(WM_MOUSEWHEEL)` addresses a **specific window**, so we decide
//!   the injection point. It also has to be posted to the *deepest child* under
//!   the point: Chromium's renderer is a child window
//!   (`Chrome_RenderWidgetHostHWND`), so a message posted to the frame is a
//!   message posted to the wrong window (`docs/30` §24.6 rule 2, F-14).
//!
//! The `E-INJECT-1` measurements (`docs/30` §24.6.1) confirm both paths reach
//! Chromium (800 px each, correlation 0.93/0.94), so neither is a degraded
//! fallback. The one thing that separates them in practice is **who chooses the
//! injection point**.
//!
//! # `Posted` does not mean "it happened"
//!
//! `PostMessageW` returning `TRUE` only means the message was queued, and
//! `SendInput` returning `1` only means the event was inserted. Neither says the
//! target acted on it. Every outcome of this module is therefore
//! `InjectStatus::Posted` and nothing stronger; the evidence that a step worked
//! is a **content displacement**, which is `P3.03`'s job (`docs/30` §24.7).
//!
//! # Shape
//!
//! The decisions (which window, which coordinates, which status) are separated
//! from the Win32 calls behind [`InjectionTarget`], so they are testable without
//! a desktop (`docs/30` §29.2, G9) and so that the `E-INJECT-1` probe can drive
//! **this** code rather than a copy of it.

#![allow(dead_code)] // wired by `P3.02`/`P3.03` and driven by the `E-INJECT-1` probe.

use crate::geometry::Point;
use crate::scroll::observation::Axis;
use ::windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use ::windows::Win32::Graphics::Gdi::ClientToScreen;
use ::windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEINPUT, MOUSEEVENTF_WHEEL, SendInput,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    CWP_SKIPINVISIBLE, ChildWindowFromPointEx, IsWindow, PostMessageW, SetCursorPos,
};

/// `WHEEL_DELTA`, the notches-to-units constant of the wheel API.
pub(crate) const WHEEL_DELTA: i32 = 120;

/// Which way the wheel is turned, in Win32's own units.
///
/// `notches > 0` means **advance the document**: for the vertical axis the
/// content moves up (the design's `d > 0`, `docs/30` §15.1), for the horizontal
/// axis the content moves left. The sign in Win32's wire format is *not* the
/// same for the two axes — see [`wheel_delta`].
pub(crate) fn wheel_delta(axis: Axis, notches: i32) -> i32 {
    match axis {
        // Positive `mouseData` is "rotated forward, away from the user", which
        // scrolls the document *up* — the opposite of advancing it.
        Axis::Vertical => -WHEEL_DELTA * notches,
        // Positive `mouseData` on `MOUSEEVENTF_HWHEEL` is "rotated to the
        // right", which advances a horizontal document.
        Axis::Horizontal => WHEEL_DELTA * notches,
    }
}

/// The `lParam` of a wheel message: screen coordinates, packed low-word x.
///
/// `docs/30` §24.6 rule 2 says the point is converted with `ScreenToClient` to
/// walk down the child chain — but the message itself carries **screen**
/// coordinates, per the `WM_MOUSEWHEEL` documentation. The `E-INJECT-1` probe
/// found both work on this machine (800 px either way, §24.6.1), which is a
/// geometric coincidence of a single-DPI desktop; the documentation is the
/// contract.
pub(crate) fn wheel_lparam(screen: Point) -> isize {
    (((screen.y as u32 & 0xFFFF) << 16) | (screen.x as u32 & 0xFFFF)) as isize
}

/// The `wParam` of a wheel message: the signed delta in the high word.
pub(crate) fn wheel_wparam(delta: i32) -> usize {
    ((delta as u32 & 0xFFFF) << 16) as usize
}

/// The largest notch count the wire format can carry.
///
/// The wheel delta travels in the **high word** of `wParam` (and in
/// `mouseData`), both of which are signed 16-bit fields. One notch is
/// [`WHEEL_DELTA`], so anything past `i16::MAX / WHEEL_DELTA` would have to be
/// truncated — and a truncated delta is a silently different scroll amount,
/// which is worse than a refusal.
pub(crate) const MAX_NOTCHES: i32 = i16::MAX as i32 / WHEEL_DELTA;

/// Which transport fires the wheel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InjectPath {
    /// The system input queue. The routing rule decides where it lands.
    SendInput,
    /// A message addressed to a window we pick.
    PostMessageW,
}

/// Who places the cursor before a `SendInput` notch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Aim {
    /// This process places it. The normal case: under `MOUSE_POS` routing the
    /// wheel follows the cursor, so the aim is part of the injection.
    PlaceCursor,
    /// The caller has already placed it.
    ///
    /// This exists because a sender at a **lower** integrity level than the
    /// foreground window cannot call `SetCursorPos` — it fails and leaves the
    /// last error at 0. The low-integrity arm of `E-INJECT-1` therefore has the
    /// operator aim the cursor and sets this, which leaves only the delivery
    /// step under test instead of reporting "the cursor could not be placed".
    AssumePlaced,
}

/// The outcome of one injection, in the shared vocabulary of `docs/30` §24.6
/// rule 3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InjectStatus {
    /// The transport accepted it. **Not** evidence that the target acted —
    /// `PostMessageW` returning `TRUE` only means the message was queued, and
    /// `SendInput` returning `1` only means the event was inserted. The evidence
    /// is a content displacement, which is `P3.03`'s job (§24.7).
    Posted,
    /// The request cannot be expressed on the wire.
    InvalidRequest,
    /// The handle is not a window any more.
    TargetNotFound,
    /// The point could not be resolved, or the cursor could not be placed.
    /// `code == 0` means no Win32 call failed — the window tree could not be
    /// walked, which is what a failed `ClientToScreen` looks like.
    CoordinateFailure { code: i32 },
    /// The transport refused. For a post that is access or validity, because a
    /// post has no queue slot to fail on.
    PostFailed { code: i32 },
}

/// One injection, expressed so that the platform is the only thing missing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct InjectRequest {
    /// The scroll target we were handed, not necessarily the window that has to
    /// receive the message — see [`InjectionTarget::child_at`].
    pub target: isize,
    /// Where the wheel is aimed, in screen coordinates.
    pub screen: Point,
    /// Positive advances the document (see [`wheel_delta`]).
    pub notches: i32,
    pub axis: Axis,
    pub path: InjectPath,
    pub aim: Aim,
}

/// What one injection produced.
#[derive(Clone, Copy, Debug)]
pub(crate) struct InjectOutcome {
    pub status: InjectStatus,
    /// Events accepted by the transport (1 for a post, `1` per notch for
    /// `SendInput`). Meaningful only when `status == Posted`.
    pub delivered: u32,
    /// The window that actually received the message, when there was one. The
    /// descended child is not the window we were handed, and saying which one it
    /// was is the difference between "the target ignored it" and "we posted to
    /// the wrong window".
    pub target_window: Option<isize>,
}

impl InjectOutcome {
    fn posted(delivered: u32, target_window: Option<isize>) -> Self {
        Self {
            status: InjectStatus::Posted,
            delivered,
            target_window,
        }
    }

    fn failed(status: InjectStatus) -> Self {
        Self {
            status,
            delivered: 0,
            target_window: None,
        }
    }
}

/// The platform calls this module needs, separated so the decisions can be
/// tested without a desktop (`docs/30` §29.2, G9).
pub(crate) trait InjectionTarget {
    /// Whether `hwnd` still names a window.
    fn is_window(&self, hwnd: isize) -> bool;
    /// `hwnd`'s client origin in screen coordinates.
    fn client_origin(&self, hwnd: isize) -> Option<Point>;
    /// The child of `hwnd` containing `local` — which is in `hwnd`'s **client**
    /// coordinates, because that is what the child lookup takes.
    fn child_at(&self, hwnd: isize, local: Point) -> Option<isize>;
    /// Move the cursor. Needed before `SendInput` under `MOUSE_POS` routing.
    fn place_cursor(&self, screen: Point) -> Result<(), i32>;
    /// Insert one wheel event into the system input queue.
    fn send_wheel(&self, delta: i32) -> Result<u32, i32>;
    /// Post one wheel message; the message id is `WM_MOUSEWHEEL` or
    /// `WM_MOUSEHWHEEL` depending on the axis.
    fn post_wheel(
        &self,
        message: u32,
        hwnd: isize,
        wparam: usize,
        lparam: isize,
    ) -> Result<(), i32>;
}

/// `WM_MOUSEWHEEL` / `WM_MOUSEHWHEEL`.
pub(crate) fn wheel_message(axis: Axis) -> u32 {
    match axis {
        Axis::Vertical => 0x020A,
        Axis::Horizontal => 0x020E,
    }
}

/// Walk from `root` down to the deepest child containing `screen`.
///
/// `ScreenToClient` is what makes this walk correct: `ChildWindowFromPointEx`
/// takes the point in the *child's* client coordinates, so every level has to be
/// converted. The message's own `lParam` still carries screen coordinates
/// ([`wheel_lparam`]) — the conversion is for the walk, not for the wire.
///
/// Returns `None` when a client origin cannot be resolved, which is a
/// coordinate failure rather than a reason to post to the frame: posting to the
/// frame is exactly the mistake that makes browsers ignore the message.
fn descend(target: &dyn InjectionTarget, root: isize, screen: Point) -> Option<isize> {
    /// A window tree deeper than this is not a real one; the cap is what stops a
    /// parent/child cycle from looping forever.
    const MAX_DEPTH: usize = 16;

    let mut current = root;
    for _ in 0..MAX_DEPTH {
        let origin = target.client_origin(current)?;
        let local = Point::new(screen.x - origin.x, screen.y - origin.y);
        match target.child_at(current, local) {
            Some(child) if child != current => current = child,
            _ => break,
        }
    }
    Some(current)
}

/// Fire one wheel step at `request.target`.
///
/// The path is **given**, not chosen: choosing is `P3.02`'s job, and keeping the
/// choice out of here is what makes "not a fallback" enforceable — this function
/// has no second attempt to fall back to.
pub(crate) fn inject(target: &dyn InjectionTarget, request: &InjectRequest) -> InjectOutcome {
    if request.notches == 0 {
        return InjectOutcome::failed(InjectStatus::InvalidRequest);
    }
    if request.notches.abs() > MAX_NOTCHES {
        return InjectOutcome::failed(InjectStatus::InvalidRequest);
    }
    if request.target == 0 || !target.is_window(request.target) {
        return InjectOutcome::failed(InjectStatus::TargetNotFound);
    }

    let delta = wheel_delta(request.axis, request.notches);
    match request.path {
        InjectPath::SendInput => {
            if request.aim == Aim::PlaceCursor {
                if let Err(code) = target.place_cursor(request.screen) {
                    return InjectOutcome::failed(InjectStatus::CoordinateFailure { code });
                }
            }
            match target.send_wheel(delta) {
                Ok(delivered) => InjectOutcome::posted(delivered, Some(request.target)),
                Err(code) => InjectOutcome::failed(InjectStatus::PostFailed { code }),
            }
        }
        InjectPath::PostMessageW => {
            let Some(deepest) = descend(target, request.target, request.screen) else {
                return InjectOutcome::failed(InjectStatus::CoordinateFailure { code: 0 });
            };
            let message = wheel_message(request.axis);
            let wparam = wheel_wparam(delta);
            let lparam = wheel_lparam(request.screen);
            match target.post_wheel(message, deepest, wparam, lparam) {
                Ok(()) => InjectOutcome::posted(1, Some(deepest)),
                Err(code) => InjectOutcome::failed(InjectStatus::PostFailed { code }),
            }
        }
    }
}

/// The real platform, in one place.
pub(crate) struct Win32Injection;

fn hwnd(handle: isize) -> HWND {
    HWND(handle as *mut core::ffi::c_void)
}

impl InjectionTarget for Win32Injection {
    fn is_window(&self, handle: isize) -> bool {
        handle != 0 && unsafe { IsWindow(Some(hwnd(handle))) }.as_bool()
    }

    fn client_origin(&self, handle: isize) -> Option<Point> {
        let mut origin = POINT { x: 0, y: 0 };
        if unsafe { ClientToScreen(hwnd(handle), &mut origin) }.as_bool() {
            Some(Point::new(origin.x, origin.y))
        } else {
            None
        }
    }

    fn child_at(&self, handle: isize, local: Point) -> Option<isize> {
        let child = unsafe {
            ChildWindowFromPointEx(
                hwnd(handle),
                POINT {
                    x: local.x,
                    y: local.y,
                },
                CWP_SKIPINVISIBLE,
            )
        };
        if child.is_invalid() {
            None
        } else {
            Some(child.0 as isize)
        }
    }

    fn place_cursor(&self, screen: Point) -> Result<(), i32> {
        unsafe { SetCursorPos(screen.x, screen.y) }
            .map_err(|error| error.code().0)
    }

    fn send_wheel(&self, delta: i32) -> Result<u32, i32> {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: delta as u32,
                    dwFlags: MOUSEEVENTF_WHEEL,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        // `SendInput` returns how many events it inserted into the system queue;
        // one means "queued", never "delivered" (§24.6 rule 3).
        let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
        if sent != 1 {
            Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0))
        } else {
            Ok(sent)
        }
    }

    fn post_wheel(
        &self,
        message: u32,
        handle: isize,
        wparam: usize,
        lparam: isize,
    ) -> Result<(), i32> {
        unsafe { PostMessageW(Some(hwnd(handle)), message, WPARAM(wparam), LPARAM(lparam)) }
            .map_err(|error| error.code().0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;
    use std::cell::RefCell;

    const ROOT: isize = 0x10;
    const FRAME: isize = 0x20;
    const RENDERER: isize = 0x30;
    const SCREEN: Point = Point::new(1060, 740);

    fn origin() -> Point {
        Point::new(1000, 700)
    }

    /// Deliberately small: a descent that passed **screen** coordinates to
    /// `ChildWindowFromPointEx` would look for `(1060, 740)` and find nothing,
    /// while the correct client-relative point is `(60, 40)`.
    fn child_rect() -> Rect {
        Rect::new(0, 0, 100, 100)
    }

    fn pack(x: i32, y: i32) -> isize {
        (((y as u32 & 0xFFFF) << 16) | (x as u32 & 0xFFFF)) as isize
    }

    struct Window {
        hwnd: isize,
        origin: Point,
        children: Vec<(isize, Rect)>,
    }

    #[derive(Default)]
    struct Recording {
        posts: Vec<(u32, isize, usize, isize)>,
        cursor: Vec<Point>,
        wheels: Vec<i32>,
    }

    struct Scripted {
        windows: Vec<Window>,
        record: RefCell<Recording>,
        post_error: Option<i32>,
        cursor_error: Option<i32>,
        wheel_error: Option<i32>,
        /// A window whose client origin cannot be resolved.
        blind: Option<isize>,
    }

    impl Scripted {
        fn tree() -> Self {
            Self {
                windows: vec![
                    Window {
                        hwnd: ROOT,
                        origin: origin(),
                        children: vec![(FRAME, child_rect())],
                    },
                    Window {
                        hwnd: FRAME,
                        origin: origin(),
                        children: vec![(RENDERER, child_rect())],
                    },
                    Window {
                        hwnd: RENDERER,
                        origin: origin(),
                        children: Vec::new(),
                    },
                ],
                record: RefCell::new(Recording::default()),
                post_error: None,
                cursor_error: None,
                wheel_error: None,
                blind: None,
            }
        }

        fn empty() -> Self {
            Self {
                windows: Vec::new(),
                ..Self::tree()
            }
        }

        fn with_post_error(code: i32) -> Self {
            Self {
                post_error: Some(code),
                ..Self::tree()
            }
        }

        fn with_blind(hwnd: isize) -> Self {
            Self {
                blind: Some(hwnd),
                ..Self::tree()
            }
        }

        fn window(&self, hwnd: isize) -> Option<&Window> {
            self.windows.iter().find(|window| window.hwnd == hwnd)
        }
    }

    impl InjectionTarget for Scripted {
        fn is_window(&self, hwnd: isize) -> bool {
            self.window(hwnd).is_some()
        }

        fn client_origin(&self, hwnd: isize) -> Option<Point> {
            if self.blind == Some(hwnd) {
                return None;
            }
            self.window(hwnd).map(|window| window.origin)
        }

        fn child_at(&self, hwnd: isize, local: Point) -> Option<isize> {
            let window = self.window(hwnd)?;
            window
                .children
                .iter()
                .find(|(_, rect)| {
                    local.x >= rect.left
                        && local.x < rect.right
                        && local.y >= rect.top
                        && local.y < rect.bottom
                })
                .map(|(child, _)| *child)
        }

        fn place_cursor(&self, screen: Point) -> Result<(), i32> {
            match self.cursor_error {
                Some(code) => Err(code),
                None => {
                    self.record.borrow_mut().cursor.push(screen);
                    Ok(())
                }
            }
        }

        fn send_wheel(&self, delta: i32) -> Result<u32, i32> {
            match self.wheel_error {
                Some(code) => Err(code),
                None => {
                    self.record.borrow_mut().wheels.push(delta);
                    Ok(1)
                }
            }
        }

        fn post_wheel(
            &self,
            message: u32,
            hwnd: isize,
            wparam: usize,
            lparam: isize,
        ) -> Result<(), i32> {
            match self.post_error {
                Some(code) => Err(code),
                None => {
                    self.record
                        .borrow_mut()
                        .posts
                        .push((message, hwnd, wparam, lparam));
                    Ok(())
                }
            }
        }
    }

    fn request(path: InjectPath, notches: i32) -> InjectRequest {
        InjectRequest {
            target: ROOT,
            screen: SCREEN,
            notches,
            axis: Axis::Vertical,
            path,
            aim: Aim::PlaceCursor,
        }
    }

    #[test]
    fn post_message_sinks_to_the_deepest_child_window() {
        let scripted = Scripted::tree();
        let outcome = inject(&scripted, &request(InjectPath::PostMessageW, 1));

        assert_eq!(outcome.status, InjectStatus::Posted);
        assert_eq!(
            outcome.target_window,
            Some(RENDERER),
            "the message must land on the deepest child under the point, not on the \
             frame we were handed: Chromium's renderer is a child window and a message \
             posted to the frame is a message posted to the wrong window (§24.6 rule 2)"
        );
        let record = scripted.record.borrow();
        assert_eq!(record.posts.len(), 1);
        assert_eq!(record.posts[0].1, RENDERER);
    }

    #[test]
    fn the_lparam_uses_screen_coordinates() {
        let scripted = Scripted::tree();
        let outcome = inject(&scripted, &request(InjectPath::PostMessageW, 1));
        assert_eq!(outcome.status, InjectStatus::Posted);

        let lparam = scripted.record.borrow().posts[0].3;
        assert_eq!(
            lparam,
            wheel_lparam(SCREEN),
            "the lParam must be the packed screen point"
        );
        assert_eq!(lparam, pack(1060, 740));
        assert_ne!(
            lparam,
            pack(60, 40),
            "the lParam must not be client-relative: ScreenToClient is how we walk the \
             child chain, but WM_MOUSEWHEEL carries screen coordinates (MSDN). The two \
             agree on a single-DPI desktop, which is exactly why this has to be asserted \
             rather than observed (§24.6.1)"
        );
    }

    #[test]
    fn post_failed_and_target_not_found_are_distinct_statuses() {
        let gone = Scripted::empty();
        assert_eq!(
            inject(&gone, &request(InjectPath::PostMessageW, 1)).status,
            InjectStatus::TargetNotFound,
            "a handle that is not a window is TargetNotFound, not a post failure"
        );

        let refused = Scripted::with_post_error(5);
        assert_eq!(
            inject(&refused, &request(InjectPath::PostMessageW, 1)).status,
            InjectStatus::PostFailed { code: 5 },
            "a refused post is its own status: a post has no queue slot to fail on, so \
             the error is about access or validity, and P3.03 switches paths on it"
        );
        assert!(refused.record.borrow().posts.is_empty());
    }

    #[test]
    fn a_zero_notch_request_is_invalid() {
        let scripted = Scripted::tree();
        assert_eq!(
            inject(&scripted, &request(InjectPath::SendInput, 0)).status,
            InjectStatus::InvalidRequest
        );
        assert_eq!(
            inject(&scripted, &request(InjectPath::PostMessageW, 0)).status,
            InjectStatus::InvalidRequest
        );
        let record = scripted.record.borrow();
        assert!(record.wheels.is_empty() && record.posts.is_empty());
    }

    #[test]
    fn the_two_axes_turn_the_wheel_in_opposite_directions() {
        assert_eq!(wheel_delta(Axis::Vertical, 1), -WHEEL_DELTA);
        assert_eq!(wheel_delta(Axis::Vertical, -1), WHEEL_DELTA);
        assert_eq!(wheel_delta(Axis::Horizontal, 1), WHEEL_DELTA);
        assert_eq!(wheel_delta(Axis::Horizontal, -1), -WHEEL_DELTA);
        assert_ne!(
            wheel_wparam(wheel_delta(Axis::Vertical, 1)),
            wheel_wparam(wheel_delta(Axis::Horizontal, 1)),
            "advancing the document is a negative delta on one axis and a positive \
             delta on the other; one shared sign would scroll one axis backwards"
        );
    }

    #[test]
    fn send_input_places_the_cursor_before_it_fires() {
        let scripted = Scripted::tree();
        let outcome = inject(&scripted, &request(InjectPath::SendInput, 1));
        assert_eq!(outcome.status, InjectStatus::Posted);
        assert_eq!(outcome.delivered, 1);

        let record = scripted.record.borrow();
        assert_eq!(
            record.cursor,
            vec![SCREEN],
            "under MOUSE_POS routing the wheel goes wherever the cursor is, so the \
             cursor is placed before the first notch — not after a failure (§24.6 rule 1)"
        );
        assert_eq!(record.wheels, vec![-WHEEL_DELTA]);
        assert!(
            record.posts.is_empty(),
            "SendInput must not post anything: the transports are alternatives, not a \
             sequence"
        );
    }

    #[test]
    fn an_unplaceable_cursor_is_a_coordinate_failure_not_a_post_failure() {
        let scripted = Scripted {
            cursor_error: Some(0),
            ..Scripted::tree()
        };
        let outcome = inject(&scripted, &request(InjectPath::SendInput, 1));
        assert_eq!(outcome.status, InjectStatus::CoordinateFailure { code: 0 });
        assert!(
            scripted.record.borrow().wheels.is_empty(),
            "nothing may be fired once the aim is known to be wrong"
        );
    }

    #[test]
    fn the_caller_may_own_the_aim() {
        let scripted = Scripted::tree();
        let mut request = request(InjectPath::SendInput, 1);
        request.aim = Aim::AssumePlaced;
        let outcome = inject(&scripted, &request);

        assert_eq!(outcome.status, InjectStatus::Posted);
        assert!(
            scripted.record.borrow().cursor.is_empty(),
            "Aim::AssumePlaced exists for the low-integrity arm of E-INJECT-1, where a \
             sender below the foreground window cannot call SetCursorPos and the \
             operator places the cursor instead (§24.6.2 conclusion 6)"
        );
    }

    #[test]
    fn a_path_the_wire_format_cannot_express_is_reported() {
        let scripted = Scripted::tree();
        assert_eq!(MAX_NOTCHES, 273);

        let outcome = inject(&scripted, &request(InjectPath::PostMessageW, MAX_NOTCHES + 1));
        assert_eq!(
            outcome.status,
            InjectStatus::InvalidRequest,
            "one notch is WHEEL_DELTA = 120 units and the delta field is 16 bits, so \
             |notches| <= {MAX_NOTCHES}; a larger request cannot be expressed and must not \
             be silently truncated into a different amount"
        );
        assert!(scripted.record.borrow().posts.is_empty());

        // The boundary itself is expressible, and it is expressible *exactly*:
        // a wrong off-by-one here would truncate the largest legal request.
        let boundary = inject(&scripted, &request(InjectPath::PostMessageW, MAX_NOTCHES));
        assert_eq!(boundary.status, InjectStatus::Posted);
        let wparam = scripted.record.borrow().posts[0].2;
        assert_eq!(wparam, wheel_wparam(-WHEEL_DELTA * MAX_NOTCHES));
        assert_eq!(
            (wparam >> 16) as u16 as i16,
            -WHEEL_DELTA as i16 * MAX_NOTCHES as i16,
            "the delta must survive the 16-bit round trip without wrapping"
        );
    }

    #[test]
    fn the_two_axes_post_different_messages() {
        let scripted = Scripted::tree();
        let mut horizontal = request(InjectPath::PostMessageW, 1);
        horizontal.axis = Axis::Horizontal;
        assert_eq!(inject(&scripted, &horizontal).status, InjectStatus::Posted);

        let mut vertical = request(InjectPath::PostMessageW, 1);
        vertical.axis = Axis::Vertical;
        assert_eq!(inject(&scripted, &vertical).status, InjectStatus::Posted);

        let record = scripted.record.borrow();
        assert_eq!(record.posts[0].0, wheel_message(Axis::Horizontal));
        assert_eq!(record.posts[1].0, wheel_message(Axis::Vertical));
        assert_ne!(
            record.posts[0].0, record.posts[1].0,
            "a horizontal scroll posted as WM_MOUSEWHEEL is a vertical scroll"
        );
    }

    #[test]
    fn an_unresolvable_point_is_a_coordinate_failure_not_a_post() {
        let scripted = Scripted::with_blind(ROOT);
        let outcome = inject(&scripted, &request(InjectPath::PostMessageW, 1));

        assert_eq!(outcome.status, InjectStatus::CoordinateFailure { code: 0 });
        assert!(
            scripted.record.borrow().posts.is_empty(),
            "posting to the frame when the child chain cannot be walked is the exact \
             mistake that makes browsers ignore the message (§24.6 rule 2); refusing is \
             the honest answer"
        );
    }
}
