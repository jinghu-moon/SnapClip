//! The scroll panel: the eight answers the overlay draws while a session runs
//! (`docs/30` §19.1, §19.4, §19.7; `P5.03`).
//!
//! # Why this is a model and not a drawing routine
//!
//! §28.4 forbids anything under `scroll/` from naming the platform, so the panel cannot live where
//! the pixels are. What it owns instead is everything about the panel that does **not** need a
//! device: the eight answers of §19.1, the appearance of the viewport box (§19.4), and the geometry
//! that places it. `windows/win/d2d.rs` is a consumer of this module rather than a second copy of
//! it — the drawing code reads these constants instead of restating them.
//!
//! That split is also what makes §30.6's first two rows checkable at L2. "The box is dashed, carries
//! a counter, and is not red" needs no `ID2D1DeviceContext`, and a test that cannot silently skip a
//! missing GPU is a test that actually ran. Reading the pixels back is `P5.05`'s job.
//!
//! # The three places this differs from PixPin
//!
//! PixPin's scroll UI is the reference for the shape — a strip with a viewport box, a live `W × H`,
//! and stop/cancel buttons. §19.1 changes exactly three things about it, and each one is a rule this
//! module has to keep rather than a preference it may drift from:
//!
//! 1. **Stop and cancel are separate buttons with separate promises** (§20.5). PixPin is two-stage:
//!    "stop" moves to an export state and a second decision saves. Here stopping *is* exporting, and
//!    cancelling produces no file at all — [`Answers::can_stop`] and [`Answers::can_cancel`] are
//!    different questions even when they happen to share an answer.
//! 2. **A failed step is not drawn as a failure.** PixPin paints a red box, which tells the user to
//!    roll back to the last good position. §16.10 says one frame that cannot be used costs the
//!    session nothing, so the honest message is "this step was not adopted, the session continues" —
//!    and the colour that means "you must act" would invite exactly the pointless manual
//!    intervention §19.4 forbids. Hence [`ViewState::Unadopted`]'s neutral grey.
//! 3. **There is a visible count of unadopted steps** (G12). A run that recovered 40 of 42 steps and
//!    one that recovered 12 of 42 must not look the same, and the counter is also what distinguishes
//!    "this step was not adopted" from "nothing has gone wrong yet".

use crate::geometry::Rect;
use crate::scroll::displacement::Status;
use crate::scroll::preview::PreviewUpdate;
use crate::scroll::session::{ScrollDiagnosticCode, StopReason};

// ── Layout, in DIP ───────────────────────────────────────────────────────────────
//
// DIP and not back-buffer pixels, because the drawing layer is the only place that knows a DPI:
// `windows/win/d2d.rs` scales these once, the way it already scales `INFO_PADDING_V_DIP`. Keeping the
// numbers here rather than in the drawing code is what lets the geometry be tested without a device.
//
// §19.7 fixes the panel to the right-hand side and puts its width on the `gpui-kit` design token
// scale — the same *number*, without taking the dependency (§27.1: the capture crate must not depend
// on the shell).

/// The panel's width. §19.7's token value.
pub(crate) const PANEL_WIDTH_DIP: i32 = 248;

/// Padding inside the panel's frame.
pub(crate) const PANEL_PADDING_DIP: i32 = 12;

/// The height of the strip the thumbnail and the viewport box are drawn in. This is the denominator
/// of every question-2 answer, so it is a model constant rather than a drawing one.
pub(crate) const PANEL_STRIP_HEIGHT_DIP: i32 = 220;

/// One line of the panel's text column.
pub(crate) const PANEL_LINE_HEIGHT_DIP: i32 = 22;

/// The floor §19.4 puts under the viewport box. A pixel-level requirement, not a taste: at 100,000
/// rows a strictly proportional box is 2 DIP tall, and question 2 ("where has it got to") has no
/// answer if the box cannot be seen.
pub(crate) const VIEWPORT_MIN_HEIGHT_DIP: i32 = 4;

/// Margin between the panel and the edge of the work area.
pub(crate) const PANEL_MARGIN_DIP: i32 = 16;

/// Gap between two of the panel's blocks.
pub(crate) const PANEL_GAP_DIP: i32 = 8;

/// Corner radius of the panel's backdrop.
pub(crate) const PANEL_RADIUS_DIP: i32 = 8;

/// Height of a button.
pub(crate) const BUTTON_HEIGHT_DIP: i32 = 26;

/// The gap between two buttons — horizontally within a row, and vertically between the two rows.
///
/// One constant for both, because it is one spacing decision: the buttons are a 2×2 block and the
/// block reads as a block only while the two gaps agree.
pub(crate) const BUTTON_GAP_DIP: i32 = 8;

/// Where the panel's pieces go, in DIP, relative to the panel's own top-left corner (docs/30 §19.7).
///
/// The panel is a fixed-size column, and `new` is the only place its height is derived, so the piece
/// rectangles cannot drift away from the frame they are drawn in. It stays in DIP and knows nothing
/// about DPI or the window: the caller scales and anchors, and [`ScrollPanel::viewport_box`] is
/// proportional, so the same layout is correct at any scale.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PanelLayout {
    /// The panel's own frame, relative to its top-left corner (so always starting at the origin).
    pub(crate) panel: Rect,
    /// The status line — question 1, and question 5 when it is the final line.
    pub(crate) status: Rect,
    /// The amount line — question 3.
    pub(crate) amount: Rect,
    /// The thumbnail strip the viewport box is placed in — questions 2 and 4.
    pub(crate) strip: Rect,
    /// The buttons, in reading order: [`RETURN_TEXT`], [`UNDO_TEXT`], [`STOP_TEXT`],
    /// [`CANCEL_TEXT`].
    ///
    /// Four buttons on **two rows of two** rather than §19.7's single row of four. The two rows are
    /// the two kinds of thing a press can do — the first row acts on the view (which part of the
    /// canvas the user is looking at), the second on the session's promise about the pixels (§20.5) —
    /// and 248 DIP of width puts four labels in one row on top of each other: "回到最新" alone is four
    /// characters, and the drawing layer would clip it rather than shrink it (`P5.04`'s `DEV` entry
    /// records the deviation from the ASCII sketch; the order is the sketch's).
    ///
    /// Two buttons rather than one with a mode, because §20.5 makes two different promises and a
    /// control that changes meaning is how a user loses work.
    pub(crate) buttons: [Rect; 4],
    /// Question 8's line, below the buttons as §19.7 draws it.
    ///
    /// Reserved whether or not there is anything to say: a panel that grows when a diagnostic
    /// arrives moves everything under the user's cursor, and an empty line is cheaper than that.
    pub(crate) trouble: Rect,
}

impl PanelLayout {
    pub(crate) fn new() -> Self {
        let left = PANEL_PADDING_DIP;
        let right = PANEL_WIDTH_DIP - PANEL_PADDING_DIP;
        let status = Rect::new(left, PANEL_PADDING_DIP, right, PANEL_PADDING_DIP + PANEL_LINE_HEIGHT_DIP);
        let amount_top = status.bottom + PANEL_GAP_DIP;
        let amount = Rect::new(left, amount_top, right, amount_top + PANEL_LINE_HEIGHT_DIP);
        let strip_top = amount.bottom + PANEL_GAP_DIP;
        let strip = Rect::new(left, strip_top, right, strip_top + PANEL_STRIP_HEIGHT_DIP);
        let buttons_top = strip.bottom + PANEL_GAP_DIP;
        let button_width = (right - left - BUTTON_GAP_DIP) / 2;
        let second_row_top = buttons_top + BUTTON_HEIGHT_DIP + BUTTON_GAP_DIP;
        let column = |row_top: i32, column: i32| {
            let column_left = left + column * (button_width + BUTTON_GAP_DIP);
            Rect::new(
                column_left,
                row_top,
                column_left + button_width,
                row_top + BUTTON_HEIGHT_DIP,
            )
        };
        let buttons = [
            column(buttons_top, 0),
            column(buttons_top, 1),
            column(second_row_top, 0),
            column(second_row_top, 1),
        ];
        let trouble_top = second_row_top + BUTTON_HEIGHT_DIP + PANEL_GAP_DIP;
        let trouble = Rect::new(left, trouble_top, right, trouble_top + PANEL_LINE_HEIGHT_DIP);
        Self {
            panel: Rect::new(0, 0, PANEL_WIDTH_DIP, trouble.bottom + PANEL_PADDING_DIP),
            status,
            amount,
            strip,
            buttons,
            trouble,
        }
    }
}

// ── The panel's words ───────────────────────────────────────────────────────────
//
// They live here rather than in the drawing code for the same reason the geometry does: the font
// subset has to carry every character that reaches the screen, and the way it learns which those are
// is by asking a producer (`drawn_strings`, below) rather than by reading the drawing code
// (`subfont/build_subset.py` guards the other direction: a non-ASCII literal in a text-drawing file
// that the list does not cover fails the build).
//
// §19.4 is the reason the trouble line and the button labels are shaped the way they are: the three
// states exist to answer "is there anything for me to do", and the buttons answer "how do I stop"
// and "how do I cancel" *separately*.

/// Question 1 while the loop is running and adopting steps.
pub(crate) const RUNNING_TEXT: &str = "正在滚动截取";

/// Question 1 when the last step was not adopted. Deliberately not "失败": §16.10 continues past a
/// single bad frame, so there is nothing for the user to do about it — which is exactly why §19.4
/// draws it dashed and grey rather than red.
pub(crate) const UNADOPTED_TEXT: &str = "本步未被采用，会话继续";

/// Question 6's button: keep the rows, then export.
pub(crate) const STOP_TEXT: &str = "停止";

/// Question 7's button: throw the rows away. A different promise from [`STOP_TEXT`] (§20.5), so it
/// gets a different word and its own hit region rather than one button with two meanings.
pub(crate) const CANCEL_TEXT: &str = "取消";

/// §19.5's gesture, as a button: hand the session back to the automatic loop.
///
/// It is the **inverse** of a drag, which is why it needs a word: a drag is the user taking over
/// (§19.5: "自动跟随停止"), and this is the only way back — pressing it clears the manual mode and
/// nothing else does. Both it and [`UNDO_TEXT`] act on the view rather than on the session's promise,
/// which is why they are a row of their own (`PanelLayout::buttons`).
pub(crate) const RETURN_TEXT: &str = "回到最新";

/// §19.6's "undo one step", pressed once per step. §19.6 constraint 4 keeps the UI at "the last step"
/// rather than a step picker, so one press is one undo and pressing it again is the next one.
pub(crate) const UNDO_TEXT: &str = "撤销";

// ── The panel's palette ──────────────────────────────────────────────────────────

/// A colour the panel paints with, as 8-bit channels.
///
/// Defined here rather than borrowed from `windows/win/ring_contrast.rs` because §28.4's gate forbids
/// this module from naming that one, and the panel's colours are part of the panel's §19.4 contract
/// (`P5.03`'s `DEV` entry records how the accent stays a single value: an L2 test in the drawing
/// layer, which can see both, asserts they agree).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PanelColor {
    pub(crate) r: u8,
    pub(crate) g: u8,
    pub(crate) b: u8,
}

impl PanelColor {
    pub(crate) const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// The overlay's brand accent, which is also what the selection chrome uses. An adopted step and a
/// finished session both wear it: nothing distinguishes them but the text.
pub(crate) const ADOPTED_RGB: PanelColor = PanelColor::new(31, 117, 219);

/// Neutral grey for a step that was not adopted. Deliberately **not** red: red would say "roll back
/// to the last good position", and §16.10 says one unusable frame never asks the user for anything.
/// It is not the capture green either — that means "this is the one".
pub(crate) const UNADOPTED_RGB: PanelColor = PanelColor::new(0x9a, 0xa3, 0xad);

// ── Question 4's answer, and what it looks like ──────────────────────────────────

/// The three appearances of §19.4. `Uncertain` and `None` and the scene-cut streak are one state
/// here because they get one appearance: what the user has to do about them is identical — nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewState {
    /// The last step was adopted.
    Confirmed,
    /// The last step was not adopted. The session is still running (§16.10).
    Unadopted,
    /// The session is over; the box stops moving.
    Ended,
}

/// How the viewport box is drawn for the current [`ViewState`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ViewportAppearance {
    pub(crate) state: ViewState,
    /// A dashed outline rather than a solid one. The primary signal, because it survives being
    /// rendered small and in any colour.
    pub(crate) dashed: bool,
    pub(crate) color: PanelColor,
    /// The count of unadopted steps so far, shown beside the box (G12). `Some` exactly when the box
    /// is dashed: the number is what turns "this step failed" into "3 of 12 steps failed".
    pub(crate) badge: Option<u32>,
}

// ── §19.5's two levels ───────────────────────────────────────────────────────────

/// How much of the canvas the strip shows.
///
/// §19.5 allows two levels and no more, and §19.2 is why: fitting the canvas and showing half of it at
/// twice the scale are two mappings, while anything in between needs general tiled rendering of an
/// arbitrarily long image — which is exactly what §19.2 refused.
///
/// The level is therefore **not** a scale factor applied to the drawing: it changes which canvas rows
/// the strip is a picture of, and the viewport box's height follows from that (§19.4's box is the
/// viewport against the rows on screen, so halving the rows doubles the box).
///
/// `P5.04` landed the level, its effect on the drawing and its two-value vocabulary. What it did **not**
/// land is the press: which rectangle the cursor was in when the button was released is the overlay's
/// (§19.7's hit regions, `P6`), the same way `ScrollPanel::note_diagnostic`'s caller is. The `allow`s
/// below say exactly that and come off when the routing arrives.
#[allow(dead_code)] // The zoom button's press routing is the overlay's (`P6`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ZoomLevel {
    /// The whole canvas in the strip.
    Fit,
    /// Half of it, so twice the pixels per row.
    Double,
}

#[allow(dead_code)] // The zoom button's press routing is the overlay's (`P6`).
impl ZoomLevel {
    /// Every level, in the order the button cycles them. A third level would have to appear here and
    /// in `next` together — which is what makes "no more than two" checkable rather than a promise.
    pub(crate) const ALL: [Self; 2] = [Self::Fit, Self::Double];

    pub(crate) fn next(self) -> Self {
        match self {
            Self::Fit => Self::Double,
            Self::Double => Self::Fit,
        }
    }
}

// ── The answers the panel draws ──────────────────────────────────────────────────

/// §19.1's eight questions, answered — plus the one §19.5's "back to the newest" needs.
///
/// Named fields rather than a list, because a list would let a caller answer six of them and pass for
/// complete. The questions are not interchangeable: 6 and 7 differ in what they promise about the
/// pixels (§20.5), and 4 and 8 are a judgement about the last step versus a report about the session.
///
/// The ninth field is not a ninth question: §19.5's return button is an inverse of a gesture, not
/// something the session is asked, and it is here because the panel's controls and the panel's text
/// have to come from one place to stay in step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Answers {
    /// 1 — what the session is doing, in words the user can act on.
    pub(crate) doing: String,
    /// 2 — the first canvas row the viewport box is over.
    pub(crate) capture_at: u64,
    /// 3 — how much has been captured: `W × H`, the step count and the unadopted count.
    pub(crate) amount: String,
    /// 4 — whether the last step's result is valid.
    pub(crate) valid: ViewState,
    /// 5 — whether the session is still running.
    pub(crate) continuing: bool,
    /// 6 — whether "stop" would do anything. Stopping keeps the pixels (§20.5).
    pub(crate) can_stop: bool,
    /// 7 — whether "cancel" would do anything. Cancelling throws the pixels away (§20.5).
    pub(crate) can_cancel: bool,
    /// 8 — the most recent anomaly, as a sentence. `None` while nothing has been reported.
    pub(crate) trouble: Option<String>,
    /// Whether "back to the newest" would do anything (§19.5): true exactly while the user has taken
    /// the session over. A control with nothing to do is one the user has to press to read, and this
    /// one is the only way back from manual mode — so it has to be visibly available *and* visibly
    /// spent.
    pub(crate) can_return_to_latest: bool,
}

/// What the panel shows while a session runs.
///
/// It is told everything it needs through [`Self::on_update`] — the session's own types stay behind
/// the port, which is the reason `PreviewUpdate` exists at all (§19.3: the panel cannot read the
/// session).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScrollPanel {
    cross_len: u64,
    extent: u64,
    primary_len: u64,
    steps: u32,
    discarded: u32,
    band: u64,
    /// The last step's verdict, or `None` before the first step — which is question 4's answer
    /// before there is anything to judge, and reads as "valid" because nothing has gone wrong.
    status: Option<Status>,
    ended: Option<StopReason>,
    trouble: Option<ScrollDiagnosticCode>,
    /// Which window of the canvas the strip shows (§19.5's two levels).
    zoom: ZoomLevel,
    /// Whether the session is still driving the scroll. The panel's own copy of §19.5's mode: it
    /// arrives through the port like everything else, because the panel cannot read the session and a
    /// second reader would be a second thing to keep in step.
    follow: bool,
}

impl ScrollPanel {
    /// The corner counter §19.4 puts beside a dashed viewport box: how many steps this session has
    /// not adopted.
    ///
    /// A producer rather than a `format!` in the drawing code, for the same reason the amount line is
    /// asked for: the characters have to reach the font subset, and the way they get there is by being
    /// produced here and enumerated by [`drawn_strings`].
    pub(crate) fn badge_text(discarded: u32) -> String {
        format!("弃 {discarded}")
    }

    pub(crate) fn new(cross_len: u64, extent: u64) -> Self {
        Self {
            cross_len,
            extent,
            primary_len: 0,
            steps: 0,
            discarded: 0,
            band: 0,
            status: None,
            ended: None,
            trouble: None,
            zoom: ZoomLevel::Fit,
            follow: true,
        }
    }

    /// Which window of the canvas the strip shows (§19.5).
    #[allow(dead_code)] // The zoom button's press routing is the overlay's (`P6`); see `ZoomLevel`.
    pub(crate) fn zoom(&self) -> ZoomLevel {
        self.zoom
    }

    #[allow(dead_code)] // The zoom button's press routing is the overlay's (`P6`); see `ZoomLevel`.
    pub(crate) fn set_zoom(&mut self, zoom: ZoomLevel) {
        self.zoom = zoom;
    }

    /// §19.5's zoom control: one button, two levels, pressed again to go back.
    #[allow(dead_code)] // The zoom button's press routing is the overlay's (`P6`); see `ZoomLevel`.
    pub(crate) fn toggle_zoom(&mut self) {
        self.zoom = self.zoom.next();
    }

    /// Whether the session is still following the scroll, as last told by the port (§19.5's mode).
    pub(crate) fn follow(&self) -> bool {
        self.follow
    }

    /// How many canvas rows the strip is a picture of at the current level.
    ///
    /// `double` is `div_ceil`, not a division: the last row of an odd-length canvas has to be
    /// reachable, and a window that stops short of it would leave the newest rows undrawable.
    fn visible_rows(&self) -> u64 {
        match self.zoom {
            ZoomLevel::Fit => self.primary_len,
            ZoomLevel::Double => self.primary_len.div_ceil(2),
        }
    }

    /// The first canvas row the strip starts at.
    ///
    /// Fitted, that is row 0 — the whole canvas is on screen. At 2× the window follows the viewport
    /// and is centred on it, clamped so the window is always full: a window hanging off either end
    /// would draw blank rows that are not blank in the canvas, which is a worse lie than a box that
    /// stops dead centre.
    fn visible_origin(&self, visible: u64) -> u64 {
        match self.zoom {
            ZoomLevel::Fit => 0,
            ZoomLevel::Double => {
                let last = self.primary_len.saturating_sub(visible);
                self.band.saturating_sub(visible / 2).min(last)
            }
        }
    }

    /// Fold one update into the panel's state. Every kind but one is a state and replaces the last.
    pub(crate) fn on_update(&mut self, update: PreviewUpdate) {
        match update {
            PreviewUpdate::Span {
                primary_len,
                steps,
                discarded,
            } => {
                self.primary_len = primary_len;
                self.steps = steps;
                self.discarded = discarded;
            }
            PreviewUpdate::Viewport { band, status } => {
                self.band = band;
                self.status = Some(status);
            }
            PreviewUpdate::Ended { reason } => self.ended = Some(reason),
            // §19.5's mode is not one of the eight questions either, but it *is* a state — the newest
            // one wins, the way `Span` and `Viewport` do — and the panel cannot derive it: the flag
            // lives in the session, which the panel is not allowed to read.
            PreviewUpdate::Follow { follow } => self.follow = follow,
            // Where the pixels are readable is not one of the eight questions — §19.1's list is what
            // the *user* is being told. Who does care is the overlay wiring, which uses this update
            // to decide when to refresh the windowed thumbnail (`P5.02`).
            PreviewUpdate::Bands { .. } => {}
        }
    }

    /// Record an anomaly for question 8's hint bar.
    ///
    /// Fed from the diagnostics that already exist (§26.4 — a code is the `code` of an event, not a
    /// second channel), so this is a method rather than a fifth `PreviewUpdate`: a diagnostic is not
    /// a state of the session, and the port's one-slot-per-kind rule would silently make the newest
    /// anomaly the only one a late consumer ever sees.
    #[allow(dead_code)] // The wiring from the diagnostics channel is the composition root's (`P6`).
    pub(crate) fn note_diagnostic(&mut self, code: ScrollDiagnosticCode) {
        self.trouble = Some(code);
    }

    /// §19.1's eight answers. See [`Answers`].
    pub(crate) fn answers(&self) -> Answers {
        let continuing = self.ended.is_none();
        Answers {
            doing: self.doing(),
            capture_at: self.band,
            amount: format!(
                "{} × {}   步 {} (弃 {})",
                self.cross_len, self.primary_len, self.steps, self.discarded
            ),
            valid: self.state(),
            continuing,
            can_stop: continuing,
            can_cancel: continuing,
            trouble: self.trouble.map(trouble_text).map(str::to_owned),
            can_return_to_latest: !self.follow,
        }
    }

    fn state(&self) -> ViewState {
        if self.ended.is_some() {
            return ViewState::Ended;
        }
        match self.status {
            Some(Status::Uncertain { .. } | Status::None) => ViewState::Unadopted,
            _ => ViewState::Confirmed,
        }
    }

    fn doing(&self) -> String {
        if let Some(reason) = self.ended {
            return ended_text(reason).to_owned();
        }
        match self.state() {
            ViewState::Unadopted => UNADOPTED_TEXT.to_owned(),
            _ => RUNNING_TEXT.to_owned(),
        }
    }

    /// §19.4's appearance for the current state.
    pub(crate) fn appearance(&self) -> ViewportAppearance {
        match self.state() {
            ViewState::Confirmed => ViewportAppearance {
                state: ViewState::Confirmed,
                dashed: false,
                color: ADOPTED_RGB,
                badge: None,
            },
            ViewState::Unadopted => ViewportAppearance {
                state: ViewState::Unadopted,
                dashed: true,
                color: UNADOPTED_RGB,
                badge: Some(self.discarded),
            },
            ViewState::Ended => ViewportAppearance {
                state: ViewState::Ended,
                dashed: false,
                color: ADOPTED_RGB,
                badge: None,
            },
        }
    }

    /// Where the viewport box goes inside `strip`, in the same DIP space as `strip`.
    ///
    /// The box is the viewport placed inside **the window the strip is showing** (§19.5's level), so
    /// both its height and its travel are relative to that window rather than to the whole canvas.
    /// Fitted, the window is the whole canvas and this is the ratio it always was; at 2× the window is
    /// half as long, so the same viewport is twice as tall on screen and travels twice as far through
    /// a strip that covers half the canvas.
    pub(crate) fn viewport_box(&self, strip: Rect) -> Rect {
        let visible = self.visible_rows();
        let origin = self.visible_origin(visible);
        let height = viewport_box_height(visible, self.extent, strip.height());
        let travel = (strip.height() - height).max(0);
        // The denominators are the *last* band rather than the current extent, so the box reaches the
        // bottom exactly when the session has reached the end; `.max(1)` keeps a canvas that fits
        // from dividing by zero.
        let last = visible.saturating_sub(self.extent).max(1);
        let at = self.band.saturating_sub(origin).min(last);
        let top = strip.top + (i64::from(travel) * at as i64 / last as i64) as i32;
        Rect::new(strip.left, top, strip.right, top + height)
    }
}

/// The viewport box's height in DIP: the strip scaled by the ratio of viewport to content, floored
/// at [`VIEWPORT_MIN_HEIGHT_DIP`] while the content is taller than the viewport.
pub(crate) fn viewport_box_height(primary_len: u64, extent: u64, strip_height: i32) -> i32 {
    if strip_height <= 0 {
        return 0;
    }
    if primary_len <= extent || primary_len == 0 {
        return strip_height;
    }
    let proportional = (i64::from(strip_height) * extent as i64 / primary_len as i64) as i32;
    proportional.clamp(VIEWPORT_MIN_HEIGHT_DIP.min(strip_height), strip_height)
}

/// Question 1's terminal answers, and question 5's reason text.
///
/// No wildcard arm: a new [`StopReason`] has to be given words here, because a stop the user cannot
/// read is a stop they will report as a crash.
fn ended_text(reason: StopReason) -> &'static str {
    match reason {
        StopReason::UserStopped => "已停止，正在导出",
        StopReason::UserCancelled => "已取消，没有产物",
        StopReason::EndReached => "已结束：到达页面底部",
        StopReason::TargetLost => "已结束：目标窗口不见了",
        StopReason::CaptureFailed => "已结束：捕获失败",
        StopReason::DeviceLost => "已结束：图形设备丢失",
        StopReason::ActuatorFailed => "已结束：注入失败",
        StopReason::MemoryLimit => "已结束：内存不足",
        StopReason::ExportBudget => "已结束：导出预算不足",
        StopReason::Timeout => "已结束：超时",
        StopReason::InternalError => "已结束：内部错误，结果已丢弃",
    }
}

/// Question 8's sentence for a diagnostic code.
///
/// No wildcard arm, for the same reason as [`ended_text`]: §26.3's vocabulary is closed at thirteen
/// and extending it has to be a deliberate act in two places rather than a silent fallback here.
fn trouble_text(code: ScrollDiagnosticCode) -> &'static str {
    match code {
        ScrollDiagnosticCode::CaptureOptionUnavailable => "捕获选项不可用",
        ScrollDiagnosticCode::CaptureBackendFallback => "已切换捕获后端",
        ScrollDiagnosticCode::InjectPathSwitched => "已切换注入方式",
        ScrollDiagnosticCode::FrameDiscarded => "有一帧没有用上",
        ScrollDiagnosticCode::StepUncertain => "本步未被采用",
        ScrollDiagnosticCode::SceneCutStreak => "页面内容正在变化",
        ScrollDiagnosticCode::ModelDecayed => "估计模型置信度下降",
        ScrollDiagnosticCode::BandSpilled => "内存不足，条带已换出到磁盘",
        ScrollDiagnosticCode::MemoryBudgetConstrained => "内存预算紧张",
        ScrollDiagnosticCode::UndoPerformed => "已撤销上一步",
        ScrollDiagnosticCode::ExportTrimmed => "导出被截断",
        ScrollDiagnosticCode::ArtifactDiscarded => "产物已丢弃",
        ScrollDiagnosticCode::InvariantViolated => "内部一致性被破坏",
    }
}

/// Every string the panel can put on screen, in the embedded family (docs/21 §5.21).
///
/// `win/d2d/tests.rs::overlay_drawn_strings()` extends its list with this, and that one list feeds
/// both the coverage gate and `subfont/drawn-text.txt`. The producers are *called* rather than
/// copied: the label constants below are the panel's own, the amount line is a computed
/// example, and the reason and diagnostic vocabularies are enumerated from the enums themselves, so
/// a new [`StopReason`] or [`ScrollDiagnosticCode`] cannot leave the font behind.
#[cfg(test)]
pub(crate) fn drawn_strings() -> Vec<String> {
    // The panel's own vocabulary. `ended_text` has no `ALL` to walk — `StopReason` is a session type
    // whose variants are matched exhaustively above — so the list of reasons is written out here,
    // once, in the one place that exists to enumerate them.
    const REASONS: [StopReason; 11] = [
        StopReason::UserStopped,
        StopReason::UserCancelled,
        StopReason::EndReached,
        StopReason::TargetLost,
        StopReason::CaptureFailed,
        StopReason::DeviceLost,
        StopReason::ActuatorFailed,
        StopReason::MemoryLimit,
        StopReason::ExportBudget,
        StopReason::Timeout,
        StopReason::InternalError,
    ];

    let mut drawn = vec![
        RUNNING_TEXT.to_owned(),
        UNADOPTED_TEXT.to_owned(),
        STOP_TEXT.to_owned(),
        CANCEL_TEXT.to_owned(),
        RETURN_TEXT.to_owned(),
        UNDO_TEXT.to_owned(),
    ];

    // The amount line is `format!`ed, so it is asked for as it will be drawn: the digits are ASCII
    // and covered anyway, and `×`, `步` and `弃` are the characters this needs the font to carry.
    let mut panel = ScrollPanel::new(1505, 1080);
    panel.on_update(PreviewUpdate::Span {
        primary_len: 2160,
        steps: 9,
        discarded: 3,
    });
    drawn.push(panel.answers().amount);
    drawn.push(ScrollPanel::badge_text(3));

    for reason in REASONS {
        drawn.push(ended_text(reason).to_owned());
    }
    for code in ScrollDiagnosticCode::ALL {
        drawn.push(trouble_text(code).to_owned());
    }
    drawn
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;
    use crate::scroll::displacement::Status;
    use crate::scroll::preview::PreviewUpdate;
    use crate::scroll::session::{ScrollDiagnosticCode, StopReason};

    const CROSS: u64 = 1505;
    const EXTENT: u64 = 1080;
    const STEP: u64 = 120;

    /// The strip the box is placed in. In DIP, like every other layout number here: the drawing
    /// layer is the only place that knows a DPI.
    fn strip() -> Rect {
        Rect::new(0, 0, PANEL_WIDTH_DIP, PANEL_STRIP_HEIGHT_DIP)
    }

    /// A session ten steps in: every step adopted, and `discarded` climbing by one every third step.
    fn ten_steps() -> ScrollPanel {
        let mut panel = ScrollPanel::new(CROSS, EXTENT);
        for step in 0..10u64 {
            panel.on_update(PreviewUpdate::Span {
                primary_len: EXTENT + step * STEP,
                steps: step as u32,
                discarded: (step / 3) as u32,
            });
            panel.on_update(PreviewUpdate::Viewport {
                band: step * STEP,
                status: Status::Confirmed { d: STEP as i32 },
            });
        }
        panel
    }

    #[test]
    fn the_eight_questions_have_concrete_answers_after_ten_steps() {
        let mut panel = ten_steps();
        let answers = panel.answers();

        // 1. What is it doing? 2. Where has it got to? 3. How much has it captured?
        assert_eq!(answers.doing, "正在滚动截取");
        assert_eq!(answers.capture_at, 9 * STEP);
        assert_eq!(answers.amount, "1505 × 2160   步 9 (弃 3)");
        // 4. Is the result valid? 5. Is it still going? 6/7. How does the user stop or cancel?
        assert_eq!(answers.valid, ViewState::Confirmed);
        assert!(answers.continuing);
        assert!(answers.can_stop, "a running session can always be stopped");
        assert!(answers.can_cancel, "and cancelled — they are different promises (§20.5)");
        // 8. Has anything gone wrong? Not yet.
        assert_eq!(answers.trouble, None);

        // A `Bands` update says where pixels are readable, and none of the eight questions is about
        // that: §19.1's list is what the *user* is being told, so this must not move any answer.
        let before = answers.clone();
        panel.on_update(PreviewUpdate::Bands {
            first_row: 0,
            rows: 2160,
            scale: 1,
        });
        assert_eq!(panel.answers(), before, "a Bands update is not one of the eight answers");

        // 8, with something to report: the code becomes a sentence, not a code.
        panel.note_diagnostic(ScrollDiagnosticCode::SceneCutStreak);
        assert_eq!(panel.answers().trouble.as_deref(), Some("页面内容正在变化"));

        // 4 and 1, for a step that was not adopted: the message names the session's continuation
        // rather than a failure, which is change ② above.
        panel.on_update(PreviewUpdate::Viewport {
            band: 9 * STEP,
            status: Status::Uncertain { d: 0 },
        });
        let answers = panel.answers();
        assert_eq!(answers.valid, ViewState::Unadopted);
        assert_eq!(answers.doing, "本步未被采用，会话继续");
        assert!(answers.continuing, "an unadopted step is not a stopped session (§16.10)");

        // And once it ends, questions 1, 4, 5, 6 and 7 all have the terminal answer.
        panel.on_update(PreviewUpdate::Ended {
            reason: StopReason::EndReached,
        });
        let answers = panel.answers();
        assert_eq!(answers.valid, ViewState::Ended);
        assert_eq!(answers.doing, "已结束：到达页面底部");
        assert!(!answers.continuing);
        assert!(!answers.can_stop);
        assert!(!answers.can_cancel);
    }

    #[test]
    fn uncertain_steps_are_drawn_dashed_with_a_counter_not_in_red() {
        let mut panel = ScrollPanel::new(CROSS, EXTENT);

        // An adopted step: solid, the accent, nothing to count.
        panel.on_update(PreviewUpdate::Viewport {
            band: 0,
            status: Status::Confirmed { d: 0 },
        });
        let adopted = panel.appearance();
        assert_eq!(adopted.state, ViewState::Confirmed);
        assert!(!adopted.dashed);
        assert_eq!(adopted.badge, None);
        assert_eq!(adopted.color, ADOPTED_RGB);

        // One that was not: dashed, neutral, with the count of steps that have gone this way.
        panel.on_update(PreviewUpdate::Span {
            primary_len: 2160,
            steps: 12,
            discarded: 3,
        });
        panel.on_update(PreviewUpdate::Viewport {
            band: STEP,
            status: Status::Uncertain { d: 0 },
        });
        let unadopted = panel.appearance();
        assert_eq!(unadopted.state, ViewState::Unadopted);
        assert!(
            unadopted.dashed,
            "an unadopted step is dashed, not merely a second hue"
        );
        assert_eq!(
            unadopted.badge,
            Some(3),
            "the badge counts unadopted steps, so it is not `1` for this one step"
        );
        assert_eq!(unadopted.color, UNADOPTED_RGB);

        // `Status::None` looks the same: §19.4 groups "unsure" and "nothing at all".
        panel.on_update(PreviewUpdate::Viewport {
            band: STEP,
            status: Status::None,
        });
        assert_eq!(panel.appearance().state, ViewState::Unadopted);

        // The mechanical form of "not red": the neutral grey's red channel is not the largest, and
        // it is not the capture green either. Red would be a colour that means "roll back", which is
        // an action §16.10 says a single unusable frame never requires.
        let color = UNADOPTED_RGB;
        assert!(
            color.r <= color.g && color.r <= color.b,
            "an unadopted step must not be drawn in red: {color:?}"
        );
        assert_ne!(
            color,
            PanelColor::new(27, 177, 95),
            "nor in the capture green, which means 'this is the one'"
        );

        // The end freezes the box; it does not look like a step that failed.
        panel.on_update(PreviewUpdate::Ended {
            reason: StopReason::EndReached,
        });
        let ended = panel.appearance();
        assert_eq!(ended.state, ViewState::Ended);
        assert!(!ended.dashed);
        assert_eq!(ended.color, ADOPTED_RGB);
    }

    #[test]
    fn the_viewport_box_never_gets_thinner_than_four_dip() {
        // While the content fits the viewport the box is the whole strip: there is nowhere to go.
        for primary_len in [0u64, 600, EXTENT] {
            assert_eq!(
                viewport_box_height(primary_len, EXTENT, PANEL_STRIP_HEIGHT_DIP),
                PANEL_STRIP_HEIGHT_DIP,
                "a canvas that fits is drawn as the whole strip (primary_len {primary_len})"
            );
        }

        // Beyond that it compresses by the ratio, but never past the floor §19.4 sets. Strictly
        // proportional, a 100,000-row canvas would put a 2 DIP box in a 220 DIP strip, and a box
        // nobody can see answers question 2 with nothing.
        for primary_len in [10_000u64, 30_000, 100_000, 500_000, 1_000_000] {
            let height = viewport_box_height(primary_len, EXTENT, PANEL_STRIP_HEIGHT_DIP);
            assert!(
                height >= VIEWPORT_MIN_HEIGHT_DIP,
                "the box has to stay visible at {primary_len} rows, got {height} DIP"
            );
            assert!(height <= PANEL_STRIP_HEIGHT_DIP, "and inside the strip");
        }
        assert_eq!(
            viewport_box_height(100_000, EXTENT, PANEL_STRIP_HEIGHT_DIP),
            VIEWPORT_MIN_HEIGHT_DIP,
            "100,000 rows is where the floor is what is being drawn, not the ratio"
        );
        assert_eq!(
            viewport_box_height(10_000, EXTENT, PANEL_STRIP_HEIGHT_DIP),
            23,
            "220 · 1080 / 10,000, which is above the floor and therefore the ratio"
        );

        // And wherever the band is, the box is inside the strip — including past the last one, which
        // a lagging update can still name.
        let strip = strip();
        for primary_len in [EXTENT, 10_000, 100_000] {
            let mut panel = ScrollPanel::new(CROSS, EXTENT);
            panel.on_update(PreviewUpdate::Span {
                primary_len,
                steps: 0,
                discarded: 0,
            });
            let last = primary_len.saturating_sub(EXTENT);
            for band in [0u64, last / 3, last / 2, last, last + 500] {
                panel.on_update(PreviewUpdate::Viewport {
                    band,
                    status: Status::Confirmed { d: STEP as i32 },
                });
                let placed = panel.viewport_box(strip);
                assert!(
                    strip.contains_rect(placed),
                    "the box left the strip at band {band} of {primary_len}: {placed:?}"
                );
                assert!(placed.height() >= VIEWPORT_MIN_HEIGHT_DIP);
            }
        }
    }

    #[test]
    fn the_panel_pieces_stay_inside_the_panel_and_do_not_overlap() {
        // The layout is the only place the panel's height is derived, so the pieces and the frame
        // have to agree: a strip drawn outside the backdrop is the same class of bug as a box drawn
        // outside the strip, one level up.
        let layout = PanelLayout::new();
        let pieces = [
            layout.status,
            layout.amount,
            layout.strip,
            layout.buttons[0],
            layout.buttons[1],
            layout.buttons[2],
            layout.buttons[3],
            layout.trouble,
        ];
        for piece in pieces {
            assert!(
                layout.panel.contains_rect(piece),
                "a panel piece left the panel: {piece:?} in {:?}",
                layout.panel
            );
            assert!(!piece.is_empty(), "and none of them is empty: {piece:?}");
        }

        // Read top to bottom, and then the buttons row by row: adjacent pieces never overlap, which is
        // what stops the amount line from being painted over by the strip.
        let stacked = [layout.status, layout.amount, layout.strip];
        assert!(stacked.windows(2).all(|pair| pair[0].bottom < pair[1].top));
        assert!(
            (0..4).all(|button| layout.buttons[button].height() == layout.buttons[0].height()),
            "one height for all four: they are one control block"
        );
        // Row-major, two per row: the view's two actions above the session's two promises (§19.5,
        // §20.5), which is the order §19.7's single row lists them in.
        assert_eq!(layout.buttons[0].top, layout.buttons[1].top);
        assert_eq!(layout.buttons[2].top, layout.buttons[3].top);
        assert!(layout.buttons[0].right < layout.buttons[1].left);
        assert!(layout.buttons[2].right < layout.buttons[3].left);
        assert!(layout.buttons[1].bottom <= layout.buttons[2].top, "the rows do not overlap");
        assert!(layout.buttons[0].left == layout.buttons[2].left, "the columns line up");
        assert!(
            layout.buttons[0].right == layout.buttons[2].right
                && layout.buttons[1].left == layout.buttons[3].left,
            "both rows are the same two columns"
        );
        assert!(
            layout.strip.bottom < layout.buttons[0].top,
            "and the button block starts below the strip"
        );
        assert!(
            layout.buttons[2].bottom < layout.trouble.top,
            "with the trouble line below it"
        );

        // And the strip is the one that answers question 2, so it has to be the strip height.
        assert_eq!(layout.strip.width(), PANEL_WIDTH_DIP - 2 * PANEL_PADDING_DIP);
        assert_eq!(layout.strip.height(), PANEL_STRIP_HEIGHT_DIP);
    }

    /// §19.5 gives zoom **exactly two levels**, and the reason it is two rather than a slider is
    /// §19.2's ruling against general tiled rendering: "the whole canvas fits the strip" and "half of
    /// it at twice the scale" are two mappings, and a continuous zoom would be a third thing to
    /// render.
    ///
    /// A level is not a label on a button: it changes the box, because the box's height is the
    /// viewport against **what the strip shows**, and doubling the scale halves that.
    #[test]
    fn zooming_has_exactly_two_levels() {
        // Two, mechanically: `ALL` is the set of variants, so a third level cannot be added without
        // disagreeing with `next` and with this line.
        assert_eq!(ZoomLevel::ALL.len(), 2);
        assert_eq!(ZoomLevel::Fit.next(), ZoomLevel::Double);
        assert_eq!(
            ZoomLevel::Double.next(),
            ZoomLevel::Fit,
            "and then back: there is no third stop to get stuck on"
        );

        let mut panel = ScrollPanel::new(CROSS, EXTENT);
        assert_eq!(
            panel.zoom(),
            ZoomLevel::Fit,
            "a session starts by fitting the whole canvas into the strip"
        );
        panel.toggle_zoom();
        assert_eq!(panel.zoom(), ZoomLevel::Double);
        panel.toggle_zoom();
        assert_eq!(panel.zoom(), ZoomLevel::Fit);

        // The two levels are two mappings. At 10,000 rows the fitted box is the ratio (23 DIP) and the
        // doubled one is 47: half the rows on screen, so twice the pixels per row.
        panel.on_update(PreviewUpdate::Span {
            primary_len: 10_000,
            steps: 0,
            discarded: 0,
        });
        panel.on_update(PreviewUpdate::Viewport {
            band: 5_000,
            status: Status::Confirmed { d: STEP as i32 },
        });
        let fitted = panel.viewport_box(strip());
        panel.set_zoom(ZoomLevel::Double);
        let doubled = panel.viewport_box(strip());

        assert_eq!(
            fitted.height(),
            23,
            "220 · 1080 / 10,000 — the fitted level shows every row, so the ratio is the height"
        );
        assert_eq!(
            doubled.height(),
            47,
            "half the rows are shown, so the same viewport is twice as tall: 220 · 1080 / 5,000"
        );
        assert!(
            strip().contains_rect(fitted) && strip().contains_rect(doubled),
            "zooming moves the box and must not push it out of the strip: {fitted:?} {doubled:?}"
        );
    }
}
