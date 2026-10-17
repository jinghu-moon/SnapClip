//! Overlay tests (docs/23 T1.6.1 moved them out of the production file).

use super::*;
use crate::geometry::Rect;
use crate::window_detection::{LevelChain, LevelKind};

/// `WS_EX_NOACTIVATE`.
///
/// A plain `u32` style mask, compared against the `isize` that
/// `GetWindowLongPtrW` returns.
const WS_EX_NOACTIVATE_MASK: isize = 0x0800_0000;

/// Regression guard for the `Esc` / `Enter` key path.
///
/// Both keys arrive as `WM_KEYDOWN`, which a window only receives once it can be
/// activated. `WS_EX_NOACTIVATE` would suppress that, so the style combination the
/// overlay is created with must never contain it.
#[test]
fn overlay_style_is_activatable_so_escape_reaches_it() {
    use ::windows::Win32::UI::WindowsAndMessaging::{
        WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    };

    // Mirror of the style expression in `overlay_thread`.
    let style = (WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0 | WS_EX_NOREDIRECTIONBITMAP.0) as isize;

    assert_eq!(
        style & WS_EX_NOACTIVATE_MASK,
        0,
        "the overlay must be activatable, otherwise Esc and Enter are dead keys"
    );
    // The styles that must stay, so the fix above is not "delete everything".
    assert_ne!(style & WS_EX_TOOLWINDOW.0 as isize, 0);
    assert_ne!(style & WS_EX_TOPMOST.0 as isize, 0);
    assert_ne!(style & WS_EX_NOREDIRECTIONBITMAP.0 as isize, 0);
}

/// `Esc` and `Enter` must be handled as plain key-down messages.
#[test]
fn escape_and_enter_map_to_cancel_and_confirm() {
    use crate::windows::hotkey;

    // Guard the virtual-key constants the handler matches on.
    assert_eq!(hotkey::ESCAPE_VIRTUAL_KEY, 0x1B);
    assert_eq!(hotkey::RETURN_VIRTUAL_KEY, 0x0D);
}

/// `docs/30` §21.4: the scroll session's notification is `WM_APP + 45`, and it must collide
/// with nothing that is already posted to a thread.
///
/// The precedent is `export_worker.rs:670`, which asserts the same thing for one pair
/// (`EXPORT_READY_MESSAGE != FRAME_READY_MESSAGE`). This one covers every id that is taken,
/// including the two the app crate owns — `apps/snapclip/src/tray.rs:43-44` has
/// `TRAY_CALLBACK_MESSAGE = WM_APP + 1` and `WM_STOP_TRAY = WM_APP + 2` — which is why the
/// assertion is written as an offset table rather than a list of `assert_ne!`s: those two
/// cannot be named from here, so they are recorded by their offsets.
#[test]
fn the_scroll_message_id_collides_with_nothing_that_is_already_posted() {
    use crate::windows::{capture_worker, detection_worker, export_worker, refinement_worker};

    assert_ne!(SCROLL_READY_MESSAGE, WM_OVERLAY_COMMAND);
    assert_ne!(SCROLL_READY_MESSAGE, capture_worker::FRAME_READY_MESSAGE);
    assert_ne!(SCROLL_READY_MESSAGE, export_worker::EXPORT_READY_MESSAGE);
    assert_ne!(
        SCROLL_READY_MESSAGE,
        detection_worker::DETECTION_READY_MESSAGE
    );
    assert_ne!(
        SCROLL_READY_MESSAGE,
        refinement_worker::REFINEMENT_READY_MESSAGE
    );

    let taken = [1, 2, 17, 18, 19, 43, 44];
    let offset = SCROLL_READY_MESSAGE - WM_APP;
    assert_eq!(
        offset, 45,
        "§21.4: the scroll session's message is `WM_APP + 45`"
    );
    assert!(
        !taken.contains(&offset),
        "`WM_APP + {offset}` is already posted by someone: {taken:?}"
    );
}

#[test]
fn lparam_decodes_signed_client_coordinates() {
    // (10, 20)
    let packed = (20i32 << 16) | 10i32;
    let point = point_from_lparam(packed as isize);
    assert_eq!((point.x, point.y), (10, 20));

    // (-5, -7) must survive the signed round trip.
    let negative = ((-7i32 & 0xFFFF) << 16) | (-5i32 & 0xFFFF);
    let point = point_from_lparam(negative as isize);
    assert_eq!((point.x, point.y), (-5, -7));
}

#[test]
fn command_round_trips_through_wparam() {
    for command in [
        OverlayCommand::Start,
        OverlayCommand::Cancel,
        OverlayCommand::Confirm,
        OverlayCommand::Shutdown,
        OverlayCommand::FrameReady,
    ] {
        assert_eq!(OverlayCommand::from_wparam(command as usize), Some(command));
    }
    assert_eq!(OverlayCommand::from_wparam(9999), None);
}

/// The deepest answer — what every preview starts as — is named, not counted.
#[test]
fn the_preview_label_names_the_element_it_snapped_to() {
    let rect = Rect::new(10, 10, 410, 810);
    assert_eq!(
        preview_label(rect, false, None, false, false),
        "400×800 px  元素"
    );
}

/// The label says *what* the box is — the size alone cannot tell "a wide element" from "the
/// container around the element" — and it says nothing about *where* on the chain, which is the
/// badge's job (docs/21 §5.22).
#[test]
fn the_preview_label_calls_a_walked_to_box_a_container_without_counting_levels() {
    let rect = Rect::new(0, 0, 100, 50);
    assert_eq!(
        preview_label(rect, false, None, false, false),
        "100×50 px  元素"
    );
    assert_eq!(
        preview_label(rect, false, None, true, false),
        "100×50 px  容器"
    );
    // The window frame is reported through `is_window` (walking to it is the same box as the
    // v1 fallback), and it wins over the walked-up noun.
    assert_eq!(
        preview_label(rect, true, None, true, false),
        "100×50 px  窗口"
    );
}

/// B6 (docs/21 §5.24): when the transport said what the box *is*, that noun replaces the label's
/// own words — and only then. A `Pane` is a container and stays `容器`.
#[test]
fn the_preview_label_names_the_control_when_the_transport_knows_it() {
    let rect = Rect::new(0, 0, 846, 272);
    // The case the prototype's `846×272 px 代码块` stands for: the walk named the level.
    assert_eq!(
        preview_label(rect, false, Some(LevelKind::Button), false, false),
        "846×272 px  按钮"
    );
    // …including a level the user walked up to, which is the whole point: `容器` was all the
    // label could say about an ancestor before the kind travelled with it.
    assert_eq!(
        preview_label(rect, false, Some(LevelKind::ListItem), true, false),
        "846×272 px  列表项"
    );
    // A generic wrapper and an unknown answer keep the label's own vocabulary.
    assert_eq!(
        preview_label(rect, false, Some(LevelKind::Pane), true, false),
        "846×272 px  容器"
    );
    assert_eq!(
        preview_label(rect, false, Some(LevelKind::Unknown), false, false),
        "846×272 px  元素"
    );
    // The whole-window fallback outranks any kind, and the degraded mark still applies.
    assert_eq!(
        preview_label(rect, true, Some(LevelKind::Button), false, true),
        "846×272 px  窗口?"
    );
}

/// The window name and the fallback mark are independent of the counter, so "the whole
/// window, and nothing answered for it" is a state the label can say.
#[test]
fn the_preview_label_names_the_window_and_the_unsupported_fallback() {
    let rect = Rect::new(0, 0, 3840, 2088);
    assert_eq!(
        preview_label(rect, true, None, false, false),
        "3840×2088 px  窗口"
    );
    assert_eq!(
        preview_label(rect, false, None, false, true),
        "3840×2088 px  元素?",
        "a fallback must not look like a confident answer"
    );
    assert_eq!(
        preview_label(rect, true, None, false, true),
        "3840×2088 px  窗口?"
    );
    assert_eq!(
        preview_label(rect, false, None, true, true),
        "3840×2088 px  容器?"
    );
}

/// ③b's shape: flat while the chain is being used, then a ramp to nothing — quantised, because
/// the number of distinct values in that ramp *is* its cost (each one is a full-surface repaint)
/// and an alpha step of `1/8` is below the eye's threshold (docs/21 §5.22, §5.24 ①).
#[test]
fn the_chain_fade_is_flat_then_quantised_into_a_bounded_number_of_steps() {
    use std::time::Duration;

    // Flat for the whole idle window, including the instant before the deadline.
    assert_eq!(chain_visibility_at(Duration::ZERO), 1.0);
    assert_eq!(chain_visibility_at(Duration::from_millis(1199)), 1.0);
    // Then it starts coming down, in steps of 1/8 — and it starts *moving*: the ramp is eased
    // (docs/21 §5.24 ③), so it has already taken a step within the first eighth of the fade
    // rather than creeping at a constant rate.
    let first = chain_visibility_at(Duration::from_millis(CHAIN_FADE_AFTER_MS));
    assert_eq!(first, 1.0, "the fade begins at the deadline, not before it");
    let a_step_down = chain_visibility_at(Duration::from_millis(CHAIN_FADE_AFTER_MS + 30));
    assert!(
        a_step_down < 1.0,
        "the fade has to have moved 30 ms in: {a_step_down}",
    );
    // Eased, not linear: half way through the 240 ms it is more than half way down.
    let halfway = chain_visibility_at(Duration::from_millis(
        CHAIN_FADE_AFTER_MS + CHAIN_FADE_MS / 2,
    ));
    assert!(
        halfway <= 0.5 + 1.0 / CHAIN_FADE_STEPS as f32,
        "an eased fade is ahead of a linear one at the half way point: {halfway}",
    );
    // Gone at the end of the fade, and it stays gone.
    assert_eq!(
        chain_visibility_at(Duration::from_millis(CHAIN_FADE_AFTER_MS + CHAIN_FADE_MS)),
        0.0
    );
    assert_eq!(chain_visibility_at(Duration::from_secs(60)), 0.0);

    // The bound: at most one distinct value per step, which is at most eight repaints however
    // often the fade is sampled. A per-frame alpha would have been ~16 values over the same
    // 240 ms (docs/21 §5.22).
    let sampled: std::collections::BTreeSet<u32> = (0..=CHAIN_FADE_AFTER_MS + CHAIN_FADE_MS + 100)
        .map(|ms| (chain_visibility_at(Duration::from_millis(ms)) * 1000.0) as u32)
        .collect();
    assert!(
        sampled.len() as u32 <= CHAIN_FADE_STEPS + 1,
        "the fade took {} distinct values",
        sampled.len(),
    );
}

/// The teaching sentence's two rules, as a truth table: once per session, but it follows a
/// walk while it is still on screen.
#[test]
fn the_hint_is_taught_once_and_follows_a_walk_only_while_it_shows() {
    // The first walk of the session: teach.
    assert!(should_teach(false, false));
    // Walking again inside the same showing: the numbers have to move with the label.
    assert!(should_teach(true, true));
    // After it has gone it does not come back — the label carries the numbers by then.
    assert!(!should_teach(true, false));
}

/// The badge and the sentence that teaches it have to agree on one numbering, and that numbering
/// is **stops** (docs/21 §5.24, A1) — the `level=8/9` in the log is a different language, for a
/// different reader: it says where in the chain the session ended up, not how much wheel is left.
#[test]
fn the_level_hint_counts_the_way_the_confirm_line_does() {
    // The state a real session reaches by rolling up one level out of nine (a real log line
    // reads `depth=9 reason=Complete level=8/9`).
    let mut chain = LevelChain::new(9);
    assert!(chain.shallower());
    assert_eq!(chain.index() + 1, 8);
    assert_eq!(chain.len(), 9);
    // The confirm line (after a confirmation) speaks in levels, which is what a log needs…
    assert_eq!(super::describe_level(Some(chain)), "8/9");
    // …while the badge speaks in *stops* (docs/21 §5.24, A1): nine evenly spaced levels, walked
    // up to the 8th, leaves seven notches toward the frame and one back toward the answer.
    let path: Vec<crate::window_detection::PathLevel> = (0..9)
        .map(|i| {
            crate::window_detection::PathLevel::unknown(Rect::new(
                i * 10,
                i * 10,
                400 - i * 10,
                300 - i * 10,
            ))
        })
        .collect();
    let reach = super::level_badge_reach(Some(chain), &path).expect("a walked chain has stops");
    assert_eq!(reach, crate::geometry::LevelReach { up: 7, down: 1 });
    // …and the sentence that explains the chip quotes those same two numbers.
    assert_eq!(
        level_hint(reach),
        "↑7 ↓1 · ↑ = 窗口 · 滚轮 / Ctrl+↑↓ 切换"
    );
    // The label says nothing about the level any more, so its text cannot drift from the badge.
    assert_eq!(
        preview_label(Rect::new(0, 0, 100, 50), false, None, true, false),
        "100×50 px  容器"
    );
    // …and the deepest level has no badge: it is the state "nothing has been walked".
    assert_eq!(
        super::level_badge_reach(Some(LevelChain::new(9)), &path),
        None
    );
    assert_eq!(
        super::level_badge_reach(Some(LevelChain::new(1)), &path),
        None
    );
    assert_eq!(super::level_badge_reach(None, &path), None);
}

/// v3 B2 (docs/21 §5.24): wheel units become level steps at the rate the device that sent them
/// deserves.
///
/// Three behaviours have to hold at once, and each of them is something the prototype showed:
/// a notch is one level (a mouse), small deltas add up to one (a touchpad), and the inertia tail
/// that follows a step is swallowed instead of walking four more levels.
#[test]
fn wheel_units_become_one_level_per_notch_and_add_up_for_a_touchpad() {
    use std::time::{Duration, Instant};

    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);

    // A notch, either way, is one level.
    let notch = WHEEL_NOTCH_UNITS;
    assert_eq!(notch, 120, "Windows' WHEEL_DELTA");
    let mut wheel = WheelAccumulator::default();
    assert_eq!(wheel.steps(notch, at(0)), 1);
    assert_eq!(wheel.steps(-notch, at(100)), -1);
    // A coalesced spin carries several notches in one message; every one of them walks.
    assert_eq!(wheel.steps(notch * 3, at(200)), 3);

    // Notches closer together than the settle window still step: the window is for inertia,
    // which never arrives as a notch, and a free-spinning wheel must not crawl because of it.
    let mut wheel = WheelAccumulator::default();
    for ms in [0, 30, 60, 90] {
        assert_eq!(wheel.steps(notch, at(ms)), 1, "notch at {ms} ms");
    }

    // A touchpad: small deltas add up, and the step lands on the delta that crosses the
    // threshold rather than on the first one.
    let small = 30;
    let mut wheel = WheelAccumulator::default();
    assert_eq!(wheel.steps(small, at(0)), 0);
    assert_eq!(wheel.steps(small, at(16)), 0);
    assert_eq!(wheel.steps(small, at(32)), 0);
    assert_eq!(wheel.steps(small, at(48)), 1);
    // …and the inertia tail of that same gesture is swallowed, not banked.
    assert!(
        WHEEL_SETTLE_MS < 120,
        "the tail of a flick, not the next flick"
    );
    assert_eq!(wheel.steps(small, at(64)), 0);
    assert_eq!(wheel.steps(small, at(120)), 0);

    // A separate gesture does not inherit the remainder: the idle gap drops it. (60 + 60 would
    // have been a step if the two added up.)
    let mut wheel = WheelAccumulator::default();
    assert_eq!(wheel.steps(60, at(0)), 0);
    assert_eq!(wheel.steps(60, at(WHEEL_IDLE_RESET_MS + 1)), 0);
    assert_eq!(wheel.steps(60, at(WHEEL_IDLE_RESET_MS + 17)), 1);

    // A hard flick that arrives as one large delta walks the levels it covers, and the remainder
    // is carried into the next event instead of being lost.
    let mut wheel = WheelAccumulator::default();
    assert_eq!(wheel.steps(WHEEL_UNITS_PER_STEP * 2 + 80, at(0)), 2);
    assert_eq!(wheel.steps(30, at(100)), 1, "the 80 left over were kept");
}

/// A3 (docs/21 §5.24): the capture box's colour rises in about a tenth of a second, holds for
/// the same 1.2 s the chain does, and then comes down — **continuously**, which is the point of
/// ①: a hue quantised into eight steps shows as eight visible jumps, so the colour is the one
/// thing here that is *not* allowed to be cheap.
#[test]
fn the_walk_colour_rises_in_a_tenth_of_a_second_and_holds_for_one_point_two() {
    use std::time::{Duration, Instant};

    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let tick = Duration::from_millis(16);

    // Rising: from nothing, the eased ramp arrives *exactly* at `WALK_RISE_MS` (not 16 ms late,
    // which is what an accumulating step would do).
    let mut walk = WalkColour::default();
    walk.touch(t0);
    let mut now = t0;
    while now < at(WALK_RISE_MS) {
        now += tick;
        walk.advance(now);
    }
    assert_eq!(
        walk.activity(),
        1.0,
        "the rise has to finish by {WALK_RISE_MS} ms"
    );
    // Eased, not linear: half way through the rise it is already past half the distance.
    let mut half = WalkColour::default();
    half.touch(t0);
    half.advance(at(WALK_RISE_MS / 2));
    assert!(
        half.activity() > 0.5,
        "an eased rise is ahead of a linear one: {}",
        half.activity()
    );

    // Holding: flat at full for the whole 1.2 s, including the deadline itself.
    for ms in [WALK_RISE_MS, 600, CHAIN_FADE_AFTER_MS] {
        let mut walk = WalkColour::default();
        walk.touch(t0);
        walk.advance(at(ms));
        assert_eq!(
            walk.activity(),
            1.0,
            "the hold must not start before {CHAIN_FADE_AFTER_MS} ms"
        );
    }

    // Falling: monotone, continuous, and it ends at the brand blue.
    let mut walk = WalkColour::default();
    walk.touch(t0);
    walk.advance(at(CHAIN_FADE_AFTER_MS));
    let mut sampled: Vec<f32> = Vec::new();
    let mut previous = walk.activity();
    for ms in (CHAIN_FADE_AFTER_MS..=CHAIN_FADE_AFTER_MS + CHAIN_FADE_MS + 100).step_by(16) {
        walk.advance(at(ms));
        let next = walk.activity();
        assert!(
            next <= previous + 1e-6,
            "the colour rose during the fade: {previous} -> {next}"
        );
        previous = next;
        sampled.push(next);
    }
    assert_eq!(previous, 0.0, "it has to arrive at the brand blue");
    // ①: more distinct values than the rings' eight steps — the colour is interpolated, not
    // snapped, and the frames it costs are bounded by the 240 ms fade (≈15 at a 16 ms tick).
    let distinct: std::collections::BTreeSet<u32> = sampled
        .iter()
        .map(|value| (value * 1000.0) as u32)
        .collect();
    assert!(
        distinct.len() > CHAIN_FADE_STEPS as usize,
        "the colour took {} distinct values — that is a staircase, not a fade",
        distinct.len()
    );
    assert!(
        distinct.len() <= 20,
        "the fade may not cost more than ~20 frames"
    );

    // A walk that arrives mid-fade continues from what is on screen: it must not flash back to
    // blue, which is what restarting the rise from zero would do.
    let mut walk = WalkColour::default();
    walk.touch(t0);
    walk.advance(at(CHAIN_FADE_AFTER_MS));
    walk.advance(at(CHAIN_FADE_AFTER_MS + CHAIN_FADE_MS / 2));
    let mid_fade = walk.activity();
    assert!(mid_fade > 0.0 && mid_fade < 1.0, "mid-fade: {mid_fade}");
    walk.touch(at(CHAIN_FADE_AFTER_MS + CHAIN_FADE_MS / 2));
    walk.advance(at(CHAIN_FADE_AFTER_MS + CHAIN_FADE_MS / 2 + 16));
    assert!(
        walk.activity() >= mid_fade,
        "a re-touch must continue from {mid_fade}, not from zero: {}",
        walk.activity()
    );
}

/// ② (docs/21 §5.24): the chain eases in instead of appearing at full strength, and a chain
/// re-used mid-fade comes back from where it is rather than jumping.
#[test]
fn the_chain_eases_in_and_re_using_it_continues_from_what_is_on_screen() {
    use std::time::{Duration, Instant};

    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);

    // Nothing has touched it yet: a tick must not make the chain appear by itself.
    let mut chain = ChainVisibility::default();
    assert!(!chain.advance(at(16)));
    assert_eq!(chain.value(), 0.0);
    assert!(!chain.pending());

    // Touched: it rises over `CHAIN_RISE_MS`, arriving exactly.
    chain.touch(t0);
    assert!(chain.running(t0), "the rise needs the tick to be seen");
    chain.advance(at(CHAIN_RISE_MS));
    assert_eq!(chain.value(), 1.0);
    // Eased: half way through the rise it is past half way up.
    let mut half = ChainVisibility::default();
    half.touch(t0);
    half.advance(at(CHAIN_RISE_MS / 2));
    assert!(half.value() > 0.5, "an eased rise leads a linear one");

    // Hold, then the quantised fall (①: alpha may stay coarse, the colour may not).
    chain.advance(at(CHAIN_FADE_AFTER_MS));
    assert_eq!(chain.value(), 1.0, "the hold lasts the full 1.2 s");
    chain.advance(at(CHAIN_FADE_AFTER_MS + CHAIN_FADE_MS));
    assert_eq!(chain.value(), 0.0);
    assert!(!chain.pending(), "and it stops asking for repaints");

    // A chain re-used while it is fading comes back from the value on screen: the rise starts at
    // `from`, so the first frame after the touch cannot jump past what the eye was looking at.
    let mut chain = ChainVisibility::default();
    chain.touch(t0);
    chain.advance(at(CHAIN_FADE_AFTER_MS + CHAIN_FADE_MS / 2));
    let mid_fade = chain.value();
    assert!(mid_fade > 0.0 && mid_fade < 1.0, "mid-fade: {mid_fade}");
    chain.touch(at(CHAIN_FADE_AFTER_MS + CHAIN_FADE_MS / 2));
    chain.advance(at(CHAIN_FADE_AFTER_MS + CHAIN_FADE_MS / 2 + 16));
    assert!(
        chain.value() >= mid_fade,
        "the rise must continue from {mid_fade} rather than restarting at zero"
    );
}

/// ② (docs/21 §5.24): a ring that is new fades in, one that was already on screen does not, and
/// one that left and came back starts over.
#[test]
fn only_a_ring_that_is_new_fades_in() {
    use std::time::{Duration, Instant};

    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let inner = Rect::new(100, 100, 200, 200);
    let outer = Rect::new(0, 0, 300, 300);

    // First sighting: starts at nothing and arrives over `RING_APPEAR_MS`.
    let mut appear = RingAppear::default();
    assert_eq!(appear.share(inner, t0), 0.0);
    assert_eq!(appear.share(inner, at(RING_APPEAR_MS)), 1.0);
    // A second ring appears while the first is already there: only the new one starts at 0.
    assert_eq!(appear.share(inner, at(500)), 1.0, "the old ring stays put");
    assert_eq!(appear.share(outer, at(500)), 0.0, "the new ring fades in");

    // A ring that is gone is forgotten, so if it comes back it fades in again rather than
    // inheriting the clock of its previous life.
    appear.retain(&[outer]);
    assert_eq!(appear.share(inner, at(1000)), 0.0);

    // The shared appear curve is the one the preview box uses too: same shape, one definition.
    assert_eq!(appear_share(Duration::ZERO, RING_APPEAR_MS), 0.0);
    assert_eq!(
        appear_share(Duration::from_millis(RING_APPEAR_MS), RING_APPEAR_MS),
        1.0
    );
    assert_eq!(
        appear_share(Duration::from_millis(RING_APPEAR_MS), PREVIEW_APPEAR_MS),
        1.0
    );
}

/// A3 (docs/21 §5.24): a tick is never what turns the box green — a **walk** is — and once a
/// walk has happened the first tick has to move the colour off zero.
///
/// This is the bug the first cut shipped: the step was guarded on `activity > 0.0`, copied from
/// the chain fade whose touch sets its value to 1 immediately. For a rise that starts from
/// nothing, "the colour is zero" is the *starting* state, so the guard made the green
/// unreachable — the box stayed brand blue through every wheel notch.
#[test]
fn the_box_only_turns_green_after_a_walk_and_the_first_tick_moves_it() {
    use std::time::{Duration, Instant};

    let t0 = Instant::now();
    let tick = Duration::from_millis(16);
    let mut walk = WalkColour::default();

    // Before any walk: ticks do nothing, and there is nothing to schedule or draw.
    assert!(!walk.advance(t0 + tick));
    assert_eq!(walk.activity(), 0.0);
    assert!(!walk.pending());
    assert!(!walk.running(t0 + tick));

    // A walk, then the very next tick: the colour has to leave zero, and the tick has to stay
    // armed while it climbs.
    walk.touch(t0);
    assert!(
        walk.running(t0),
        "the rise needs the tick before it can be seen"
    );
    assert!(walk.advance(t0 + tick), "the rise starts from nothing");
    assert!(walk.activity() > 0.0);
    assert!(walk.pending());

    // …it arrives in about a tenth of a second…
    let mut now = t0 + tick;
    while walk.activity() < 1.0 {
        now += tick;
        walk.advance(now);
        assert!(
            now < t0 + Duration::from_millis(400),
            "the rise has to finish"
        );
    }
    // …holds still (nothing to paint, but the deadline is still being watched)…
    assert!(!walk.running(t0 + Duration::from_millis(600)));
    assert!(walk.pending());

    // …comes back down, and then forgets the walk: the next tick has nothing to do, and the next
    // walk gets a rise of its own rather than inheriting this one's clock.
    while walk.activity() > 0.0 {
        now += tick;
        walk.advance(now);
    }
    assert!(!walk.pending());
    assert!(!walk.running(now));
    assert!(!walk.advance(now + Duration::from_millis(1000)));
    assert_eq!(walk.activity(), 0.0);
    walk.touch(now);
    assert!(walk.advance(now + tick));
}

#[test]
fn the_guide_fades_in_over_its_span_and_out_faster() {
    let start = Instant::now();
    let mut fade = GuideFade::default();
    // The first tick only starts the clock.
    assert!(!fade.advance(true, start));
    assert!(fade.running());
    let mut now = start;
    let mut ticks = 0;
    while fade.running() {
        now += Duration::from_millis(16);
        fade.advance(true, now);
        ticks += 1;
    }
    assert_eq!(fade.alpha(), 1.0);
    assert_eq!(ticks, GUIDE_FADE_IN_MS.div_ceil(16) as i32);

    // Out: the same clock, the shorter span.
    fade.advance(false, now);
    let mut out = 0;
    while fade.running() {
        now += Duration::from_millis(16);
        fade.advance(false, now);
        out += 1;
    }
    assert_eq!(fade.alpha(), 0.0);
    assert_eq!(out, GUIDE_FADE_OUT_MS.div_ceil(16) as i32);
}

#[test]
fn a_stalled_tick_does_not_make_the_guide_jump() {
    let start = Instant::now();
    let mut fade = GuideFade::default();
    fade.advance(true, start);
    fade.advance(true, start + Duration::from_secs(2));
    let expected = GUIDE_FADE_MAX_STEP_MS as f32 / GUIDE_FADE_IN_MS as f32;
    assert!((fade.alpha() - expected).abs() < 1e-4, "{}", fade.alpha());
}

#[test]
fn a_guide_at_rest_does_not_keep_the_tick_alive() {
    let mut fade = GuideFade::default();
    assert!(!fade.advance(false, Instant::now()));
    assert!(!fade.running());
}

/// 倒计时期间窗口只剩"屏幕上真有 UI 的地方"：选框环、角标、倒计时小框。
/// 选区内部（除这三处）不再属于窗口 —— 这正是点击能跨进程落到别的应用的原因
/// （`HTTRANSPARENT` 只覆盖同线程；收窄到整个选区也不成立，选区常覆盖大半屏；只留小框又会让
/// "截图框消失"。docs/23 §14.55.9）。
#[test]
fn the_countdown_keeps_only_the_ui_that_is_on_screen() {
    use crate::windows::win::window::WindowRegion;
    let selection = Rect::new(120, 80, 620, 480);
    let badge = Rect::new(120, 44, 300, 70);
    let pieces = super::session::countdown_window_pieces(selection, Some(badge), 96);

    let (outer, inner) = pieces
        .iter()
        .find_map(|piece| match piece {
            WindowRegion::Ring { outer, inner } => Some((*outer, *inner)),
            WindowRegion::Rect(_) => None,
        })
        .expect("选框环必须在里面");
    assert!(
        outer.left < selection.left && outer.right > selection.right,
        "{outer:?}"
    );
    assert!(
        inner.left > selection.left && inner.right < selection.right,
        "{inner:?}"
    );
    assert!(inner.left - outer.left >= 8, "环宽要够放框线与手柄圆点");
    assert_eq!(outer.center(), selection.center(), "环与选区同心");

    assert!(
        pieces.contains(&WindowRegion::Rect(badge)),
        "角标要留在窗口里，否则倒计时期间它会消失"
    );
    let number = pieces
        .iter()
        .filter_map(|piece| match piece {
            WindowRegion::Rect(rect) => Some(*rect),
            WindowRegion::Ring { .. } => None,
        })
        .find(|rect| rect.contains(selection.center()))
        .expect("倒计时小框");
    assert_eq!(number.center(), selection.center());
    assert!(number.width() < selection.width() / 2, "{number:?}");

    // 选区内部、但既不在环带里也不在小框里的那一点，不属于窗口。
    // 环的内孔里、且离小框足够远的一点（小框以选区中心为中心）。
    let probe = Point::new(selection.right - 40, selection.bottom - 30);
    assert!(
        probe.x > inner.left && probe.x < inner.right && probe.y > inner.top && probe.y < inner.bottom,
        "探针点落在环的内孔里"
    );
    assert!(!number.contains(probe), "探针点不应当落在倒计时小框里");

    // 高 DPI 下跟着放大，中心不变。
    let hidpi = super::session::countdown_window_pieces(selection, Some(badge), 192);
    assert_eq!(hidpi.len(), pieces.len());
    assert_eq!(
        hidpi
            .iter()
            .find_map(|piece| match piece {
                WindowRegion::Ring { outer, .. } => Some(outer.center()),
                WindowRegion::Rect(_) => None,
            })
            .expect("环"),
        selection.center()
    );

    // 空选区：空集合（调用方据此还原整块窗口）。
    assert!(
        super::session::countdown_window_pieces(Rect::new(10, 10, 10, 400), None, 96).is_empty()
    );
}

/// 倒计时 HUD 的光标：**边框与手柄上是改大小光标，别处一律箭头**。
///
/// 断言只比较"光标之间的关系"，不点名具体常量（那两套常量在这里类型不同，而这条测试要钉的是
/// 映射结构）：同一方位必须同一种光标，不同方位必须不同；HUD 里只有 `Resize` 配得上改大小光标。
#[test]
fn the_countdown_only_offers_a_resize_cursor_on_the_frame() {
    use super::input::{countdown_cursor, selection_cursor};
    use crate::geometry::{Handle, SelectionGeometry};

    let resize = SelectionGeometry::Resize;
    let arrow = countdown_cursor(SelectionGeometry::Move);
    // HUD 里：边框/手柄沿用普通状态那套映射，且都不等于箭头。
    for handle in Handle::ALL {
        assert_eq!(countdown_cursor(resize(handle)), selection_cursor(resize(handle)));
        assert_ne!(countdown_cursor(resize(handle)), arrow, "{handle:?} 该是改大小光标");
    }
    // 边框之外的三种命中在 HUD 里都是箭头（窗口只在环上，那里已经不属于它）。
    for other in [
        SelectionGeometry::Move,
        SelectionGeometry::Create,
        SelectionGeometry::Outside,
    ] {
        assert_eq!(countdown_cursor(other), arrow, "{other:?} 在 HUD 里该是普通箭头");
    }
    // 普通状态那套映射的结构原样保留（回归）：方位相同→同一种，方位不同→不同种，
    // 空白与新建都是十字，移动是四向箭头。
    assert_eq!(selection_cursor(resize(Handle::TopLeft)), selection_cursor(resize(Handle::BottomRight)));
    assert_eq!(selection_cursor(resize(Handle::TopRight)), selection_cursor(resize(Handle::BottomLeft)));
    assert_eq!(selection_cursor(resize(Handle::Top)), selection_cursor(resize(Handle::Bottom)));
    assert_eq!(selection_cursor(resize(Handle::Left)), selection_cursor(resize(Handle::Right)));
    assert_ne!(selection_cursor(resize(Handle::TopLeft)), selection_cursor(resize(Handle::TopRight)));
    assert_ne!(selection_cursor(resize(Handle::Left)), selection_cursor(resize(Handle::Top)));
    assert_eq!(selection_cursor(SelectionGeometry::Create), selection_cursor(SelectionGeometry::Outside));
    assert_ne!(selection_cursor(SelectionGeometry::Move), selection_cursor(SelectionGeometry::Outside));
    assert_ne!(selection_cursor(SelectionGeometry::Move), selection_cursor(resize(Handle::Left)));
}

/// 倒计时里拖完边框：**到点贴图用的区域必须是新选区**，不是点「确定」那一刻的旧区域。
///
/// 这是这条链路最容易漏的一环：改了框却不更新 `pending_region`，贴出来还是原来那块。
#[test]
fn resizing_during_the_countdown_moves_the_pin_region_with_the_frame() {
    use crate::geometry::{Point, Rect};
    let origin = Point::new(0, 412);
    let old = Rect::new(0, 412, 1698, 1815);
    let new = Rect::new(40, 500, 1200, 1400);

    let (pin, pieces) = OverlayController::countdown_after_resize(old, origin, None, 96);
    let (pin_after, pieces_after) = OverlayController::countdown_after_resize(new, origin, None, 96);

    assert_eq!(
        pin.map(|region| region.to_rect(origin)),
        Some(old),
        "起手贴图区域就是点「确定」那一刻的选区"
    );
    assert_eq!(
        pin_after.map(|region| region.to_rect(origin)),
        Some(new),
        "拖完之后贴图区域必须跟着新选区走"
    );
    assert_ne!(
        pin_after.map(|region| region.to_rect(origin)),
        pin.map(|region| region.to_rect(origin)),
        "两份区域必须真的不同，否则这条测试没有区分力"
    );
    // 窗口区域同源跟随：环的内孔跟着新选区走。
    assert_eq!(
        pieces_after,
        super::session::countdown_window_pieces(new, None, 96),
        "HUD 的窗口区域也要按新选区重算"
    );
    assert_ne!(pieces, pieces_after);
    // 退化选区不给贴图区域（会话会拒绝，不该静默贴一块空图）。
    let (degenerate, _) =
        OverlayController::countdown_after_resize(Rect::new(10, 10, 10, 400), origin, None, 96);
    assert!(degenerate.is_none());
}

/// 拖动中指示方向要**锁住**：指针离开边框后命中的是"外面"，按命中结果算光标会跳回箭头。
#[test]
fn the_cursor_stays_on_the_dragged_direction_while_dragging() {
    use super::input::{countdown_cursor_while, selection_cursor};
    use crate::geometry::{Handle, SelectionGeometry};

    let dragged = countdown_cursor_while(
        Some(Handle::Left),
        SelectionGeometry::Outside,
    );
    assert_eq!(
        dragged,
        selection_cursor(SelectionGeometry::Resize(Handle::Left)),
        "拖着左边框时指针跑到外面，光标仍该是左右改大小"
    );
    assert_eq!(
        countdown_cursor_while(Some(Handle::Right), SelectionGeometry::Outside),
        dragged,
        "左右两条边共用同一个水平改大小光标"
    );
    assert_ne!(
        countdown_cursor_while(Some(Handle::Top), SelectionGeometry::Outside),
        dragged,
        "正交方向要显示不同的光标（垂直 ≠ 水平）"
    );
    // 不拖的时候才按命中结果走（外面是箭头、边框上是改大小）。
    assert_eq!(
        countdown_cursor_while(None, SelectionGeometry::Outside),
        countdown_cursor_while(None, SelectionGeometry::Move),
        "不拖时外面/内部都是箭头"
    );
    assert_ne!(
        countdown_cursor_while(None, SelectionGeometry::Resize(Handle::Top)),
        countdown_cursor_while(None, SelectionGeometry::Outside),
        "不拖时边框上仍是改大小光标"
    );
}

/// 退出倒计时的**顺序**必须钉住（docs/23 §14.55.9.5）：
///
/// * 到点 / 失败：**先让窗口不可见**，再清倒计时、还原区域，且**不请求重绘** —— 那一帧普通画面
///   （冻结画面 + 遮罩）就是用户看到的"框线外侧突然出现一圈暗灰"；
/// * Esc 取消：窗口要继续可见，所以没有 Hide 那一步，最后重画一帧普通画面。
///
/// 这条测的是三个出口的**交错先后**，不是结果。上一版只断言了结果，抓不到中间那一帧。
#[test]
fn leaving_the_countdown_hides_the_window_before_it_stops_the_hud() {
    use super::session::{CountdownExit, ExitStep, countdown_exit_steps};

    let index = |steps: &[ExitStep], step: ExitStep| steps.iter().position(|s| *s == step);

    for exit in [CountdownExit::Fire, CountdownExit::Fail] {
        let steps = countdown_exit_steps(exit);
        let hide = index(steps, ExitStep::Hide)
            .unwrap_or_else(|| panic!("{exit:?} 应当先隐藏窗口：{steps:?}"));
        let clear = index(steps, ExitStep::ClearCountdown)
            .unwrap_or_else(|| panic!("{exit:?} 应当清倒计时：{steps:?}"));
        assert!(
            hide < clear,
            "{exit:?} 必须先不可见、再退出 HUD 渲染模式：{steps:?}"
        );
        assert_eq!(
            index(steps, ExitStep::Repaint),
            None,
            "{exit:?} 不该请求重绘 —— 那一帧就是灰底：{steps:?}"
        );
    }

    let steps = countdown_exit_steps(CountdownExit::Cancel);
    assert_eq!(
        index(steps, ExitStep::Hide),
        None,
        "取消时窗口要继续可见：{steps:?}"
    );
    assert_eq!(
        index(steps, ExitStep::Repaint),
        Some(steps.len() - 1),
        "取消要重画一帧普通画面，且是最后一步：{steps:?}"
    );
    assert!(
        index(steps, ExitStep::ClearCountdown) < index(steps, ExitStep::RestoreRegion),
        "取消也要先清状态、再还原区域：{steps:?}"
    );
    // 遮罩层也必须跟着"退出可见状态"一起收，不能留到最后（否则它会盖着下一个会话）。
    for exit in [CountdownExit::Fire, CountdownExit::Cancel, CountdownExit::Fail] {
        let steps = countdown_exit_steps(exit);
        assert!(
            steps.contains(&ExitStep::HideDim),
            "{exit:?} 必须收掉遮罩层: {steps:?}"
        );
        let hide_dim = steps.iter().position(|s| *s == ExitStep::HideDim).unwrap();
        let clear = steps
            .iter()
            .position(|s| *s == ExitStep::ClearCountdown)
            .unwrap();
        assert!(
            hide_dim < clear,
            "{exit:?} 必须先收遮罩层再清状态: {steps:?}"
        );
    }
}

/// 进入倒计时的顺序：**先换画面，再改窗口形状/叠加层**（docs/23 §14.55.9.9）。
///
/// 反过来（先收窄窗口区域）会在那一刻把屏幕上旧画面的遮罩切掉一半、而新遮罩还没出现 ——
/// 用户看到的就是"面板消失时选区外的暗色闪一下"。
#[test]
fn entering_the_countdown_paints_the_hud_before_it_reshapes_the_window() {
    use super::session::{EnterStep, countdown_enter_steps};

    let steps = countdown_enter_steps();
    let index = |step: EnterStep| {
        steps
            .iter()
            .position(|s| *s == step)
            .unwrap_or_else(|| panic!("{step:?} 必须在进入顺序里：{steps:?}"))
    };

    // 暗色不能断档：遮罩先就位，再由 HUD 帧把它"露"出来。
    assert!(
        index(EnterStep::ShowDim) < index(EnterStep::PaintHud),
        "先叠遮罩、再换画面，否则旧遮罩先消失一段时间：{steps:?}"
    );
    // 也不能叠层：提层排在换画面之前，不透明的旧画面会立刻盖住刚出现的遮罩。
    assert!(
        index(EnterStep::ShowDim) < index(EnterStep::RaiseOverlay),
        "遮罩就位之后才谈得上提层：{steps:?}"
    );
    assert!(
        index(EnterStep::RaiseOverlay) < index(EnterStep::PaintHud),
        "提层必须在换画面之前：{steps:?}"
    );
    // HUD 帧在选区外透明，收窄区域放在换画面之后即可。
    assert!(
        index(EnterStep::PaintHud) < index(EnterStep::NarrowRegion),
        "先换画面再收窄区域：{steps:?}"
    );
    // 同步换画面只能发生一次：多一次就等于把交接顺序交还给消息循环。
    assert_eq!(
        steps
            .iter()
            .filter(|step| **step == EnterStep::PaintHud)
            .count(),
        1,
        "进入路径里 PaintHud 只能出现一次：{steps:?}"
    );
}

/// 倒计时期间**结构上**不许画普通帧（docs/23 §14.55.9.6）。
///
/// 不是"别让谁 invalidate"的约定：闸生效着、倒计时却已结束（窗口还没隐藏）时，渲染入口把
/// 这一帧直接压掉 —— "先画一帧再隐藏"就是用户看到的那下灰底。
#[test]
fn the_countdown_gate_never_paints_a_normal_frame() {
    use super::render_submit::{frame_kind, FrameKind};

    assert_eq!(frame_kind(false, None), FrameKind::Normal, "平时就是普通帧");
    assert_eq!(frame_kind(false, Some(3)), FrameKind::Hud, "在数就是 HUD");
    assert_eq!(frame_kind(true, Some(3)), FrameKind::Hud, "在数且闸生效：HUD");
    assert_eq!(
        frame_kind(true, None),
        FrameKind::Suppressed,
        "闸还生效、倒计时已结束（窗口未隐藏）⇒ 压掉"
    );
}


// ─── 倒计时遮罩层：带正向对照的活桌面回归（docs/23 §14.55.9.8）────────────────

/// 让测试进程与产品一致地 per-monitor-v2 感知。
///
/// 不这么做整个观测都无效：`cargo test` 默认不是 DPI 感知的，窗口坐标是**虚拟化**的，
/// 而屏幕 `BitBlt` 取**物理**像素 ⇒ 取样矩形与窗口错位（上一版"逐字节相同"的结论
/// 就是这么来的，§14.55.9.8 的差分实验纠正了它）。
fn dim_test_be_dpi_aware() {
    use windows_sys::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

/// 屏幕 `BitBlt`（带 `CAPTUREBLT`，它会包含 layered 窗口）取矩形的平均亮度。
/// 抓一块屏幕的像素（BGRA、自上而下）。`CAPTUREBLT` 让 layered 窗口也进得来 ——
/// 观测遮罩层时必须先临时摘掉它的"排除出截图"，否则这里永远看不到它。
fn dim_test_screen_bgra(rect: Rect) -> (Vec<u8>, u32) {
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
        SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, SRCCOPY,
    };
    let width = rect.width().max(1) as u32;
    let height = rect.height().max(1) as u32;
    unsafe {
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };
        let screen = GetDC(std::ptr::null_mut());
        let memory = CreateCompatibleDC(screen);
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let bitmap =
            CreateDIBSection(memory, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        assert!(!bitmap.is_null() && !bits.is_null(), "观测用 DIB 创建失败");
        SelectObject(memory, bitmap as _);
        let ok = BitBlt(
            memory,
            0,
            0,
            width as i32,
            height as i32,
            screen,
            rect.left,
            rect.top,
            SRCCOPY | CAPTUREBLT,
        );
        assert_ne!(ok, 0, "BitBlt 失败：{}", GetLastError());
        let bytes = std::slice::from_raw_parts(bits.cast::<u8>(), (width * height * 4) as usize);
        let pixels = bytes.to_vec();
        DeleteObject(bitmap as _);
        DeleteDC(memory);
        ReleaseDC(std::ptr::null_mut(), screen);
        (pixels, width)
    }
}

/// 相邻像素的水平平均绝对差（只看亮度）：模糊把它压下去，纯压暗只按比例缩小 ——
/// 这就是"模糊"与"只是变暗"能区分开的原因。
fn dim_local_contrast(pixels: &[u8], width: u32) -> f64 {
    let rows = (pixels.len() / 4) as u32 / width.max(1);
    let mut sum = 0f64;
    let mut count = 0u64;
    for y in 0..rows {
        for x in 1..width {
            let at = ((y * width + x) * 4) as usize;
            let before = ((y * width + x - 1) * 4) as usize;
            let luma = |i: usize| pixels[i] as f64 + pixels[i + 1] as f64 + pixels[i + 2] as f64;
            sum += (luma(at) - luma(before)).abs();
            count += 1;
        }
    }
    if count == 0 { 0.0 } else { sum / count as f64 }
}

/// 棋盘内容（BGRA，不透明）：局部对比度极高，用于证明"模糊确实到了屏幕上"。
fn dim_test_checkerboard(width: u32, height: u32, cell: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let value: u8 = if ((x / cell) + (y / cell)) % 2 == 0 { 230 } else { 26 };
            let at = ((y * width + x) * 4) as usize;
            pixels[at] = value;
            pixels[at + 1] = value;
            pixels[at + 2] = value;
            pixels[at + 3] = 255;
        }
    }
    pixels
}

/// 把遮罩那一层（`dim_pixel`，premultiplied）叠到内容上 —— 与产品渲染器里的两步画刷等价。
fn dim_test_wash(pixels: &mut [u8]) {
    let [b, g, r, a] = super::dim_layer::dim_pixel();
    let keep = 255u32 - a as u32;
    for pixel in pixels.chunks_exact_mut(4) {
        pixel[0] = ((pixel[0] as u32 * keep + 127) / 255) as u8 + b;
        pixel[1] = ((pixel[1] as u32 * keep + 127) / 255) as u8 + g;
        pixel[2] = ((pixel[2] as u32 * keep + 127) / 255) as u8 + r;
        pixel[3] = 255;
    }
}

fn dim_test_mean(pixels: &[u8]) -> f64 {
    let sum: f64 = pixels
        .chunks_exact(4)
        .map(|pixel| pixel[0] as f64 + pixel[1] as f64 + pixel[2] as f64)
        .sum();
    sum / (pixels.len() / 4).max(1) as f64
}

/// 测试用的盒式模糊：代替产品那条 D2D 高斯模糊（产品那条由 L4 测试单独证明）。
fn dim_box_blur(pixels: &[u8], width: u32, height: u32, radius: i32) -> Vec<u8> {
    let mut out = pixels.to_vec();
    let radius = radius.max(1);
    let (width_i, height_i) = (width as i32, height as i32);
    for y in 0..height_i {
        for x in 0..width_i {
            let mut acc = [0u32; 3];
            let mut n = 0u32;
            for dy in -radius..=radius {
                let sy = y + dy;
                if !(0..height_i).contains(&sy) {
                    continue;
                }
                for dx in -radius..=radius {
                    let sx = x + dx;
                    if !(0..width_i).contains(&sx) {
                        continue;
                    }
                    let at = ((sy as u32 * width + sx as u32) * 4) as usize;
                    acc[0] += pixels[at] as u32;
                    acc[1] += pixels[at + 1] as u32;
                    acc[2] += pixels[at + 2] as u32;
                    n += 1;
                }
            }
            let at = ((y as u32 * width + x as u32) * 4) as usize;
            out[at] = (acc[0] / n.max(1)) as u8;
            out[at + 1] = (acc[1] / n.max(1)) as u8;
            out[at + 2] = (acc[2] / n.max(1)) as u8;
            out[at + 3] = 255;
        }
    }
    out
}

fn dim_test_screen_mean(rect: Rect) -> f64 {
    let (pixels, _) = dim_test_screen_bgra(rect);
    let sum: f64 = pixels
        .chunks_exact(4)
        .map(|pixel| pixel[0] as f64 + pixel[1] as f64 + pixel[2] as f64)
        .sum();
    sum / (pixels.len() / 4).max(1) as f64
}

fn dim_test_pump(ms: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// 遮罩层必须**真的把选区之外压暗**，而且这个压暗必须：
/// 不进截图（排除出捕获）、不进选区（洞是对的）、隐藏后消失。
///
/// 观测方式自带**正向对照**：同一块矩形在"显示前 / 显示后 / 排除后 / 隐藏后"四个时刻的
/// 亮度对比 —— 前两次必须变暗，后两次必须回到基线。没有这条，就只能像上一版那样用
/// 一张"逐字节相同"的截图得出反结论。
#[test]
#[ignore = "L3 (docs/31 D-14): needs a live desktop; shows a real layered window"]
fn the_countdown_dim_layer_actually_darkens_outside_the_selection() {
    use windows_sys::Win32::Graphics::Gdi::{GetWindowRgn, PtInRegion};

    dim_test_be_dpi_aware();
    // 整块窗口 600×400，洞 400×200 居中；探针在环里与洞里各一个。
    let monitor = Rect::new(160, 160, 760, 560);
    let selection = Rect::new(260, 260, 660, 460);
    let outside = Rect::new(180, 180, 220, 220);
    let inside = Rect::new(440, 340, 480, 380);
    let (width, height) = (monitor.width() as u32, monitor.height() as u32);

    let baseline_outside = dim_test_screen_mean(outside);
    let baseline_inside = dim_test_screen_mean(inside);

    // 内容：棋盘（对比度极高）→ 模糊 → 压暗。三样都是确定量，观感不依赖桌面内容。
    let mut sharp = dim_test_checkerboard(width, height, 8);
    dim_test_wash(&mut sharp);
    let mut blurred = dim_box_blur(&sharp, width, height, 8);
    dim_test_wash(&mut blurred);
    let sharp_mean = dim_test_mean(&sharp);
    let sharp_contrast = dim_local_contrast(&sharp, width);
    let blur_mean = dim_test_mean(&blurred);
    let blur_contrast = dim_local_contrast(&blurred, width);
    eprintln!(
        "[dim-live] content: sharp mean={sharp_mean:.1} contrast={sharp_contrast:.1}; blurred mean={blur_mean:.1} contrast={blur_contrast:.1}"
    );
    let blurred_content = super::super::win::d2d::DimSource {
        width,
        height,
        pixels: blurred.clone(),
    };
    let sharp_content = super::super::win::d2d::DimSource {
        width,
        height,
        pixels: sharp.clone(),
    };

    let mut layer = super::dim_layer::DimLayer::default();
    layer
        .show(monitor, selection, Some(&blurred_content))
        .expect("遮罩层应当显示");
    // 临时摘掉"排除出截图"，否则截图看不见它（这正是上一轮观测失效的一半原因）。
    layer.set_capture_exclusion(false);
    dim_test_pump(260);

    let shown_outside = dim_test_screen_mean(outside);
    let shown_inside = dim_test_screen_mean(inside);
    let (blur_pixels, blur_width) = dim_test_screen_bgra(outside);
    let shown_blur_contrast = dim_local_contrast(&blur_pixels, blur_width);
    eprintln!(
        "[dim-live] blurred content on screen: mean {baseline_outside:.1} -> {shown_outside:.1} (content {blur_mean:.1}); contrast {shown_blur_contrast:.1} (content {blur_contrast:.1})"
    );

    // 1) 交接确实发生：屏幕上是"我们给的内容"，不是原来的桌面（棋盘内容压暗后与桌面差很远）。
    assert!(
        (shown_outside - blur_mean).abs() <= 12.0,
        "屏幕上应当是遮罩层给的内容：content={blur_mean:.1} screen={shown_outside:.1}"
    );
    // 2) 压暗：内容本身就比未压暗的棋盘暗得多。
    assert!(
        blur_mean < dim_test_mean(&dim_test_checkerboard(width, height, 8)) - 100.0,
        "遮罩内容必须明显压暗"
    );
    // 3) 洞里（选区内）仍然是实时画面：不能被压暗，也不该出现棋盘。
    assert!(
        (shown_inside - baseline_inside).abs() <= 6.0,
        "洞里（选区内）不能被压暗：{baseline_inside:.1} -> {shown_inside:.1}"
    );

    // 4) 模糊真的到了屏幕上：换成"未模糊"的同一份内容，对比度必须大幅跳起来。
    layer
        .show(monitor, selection, Some(&sharp_content))
        .expect("遮罩层应当显示");
    layer.set_capture_exclusion(false);
    dim_test_pump(260);
    let (sharp_pixels, sharp_width) = dim_test_screen_bgra(outside);
    let shown_sharp_contrast = dim_local_contrast(&sharp_pixels, sharp_width);
    eprintln!(
        "[dim-live] sharp content on screen: contrast {shown_sharp_contrast:.1} vs blurred {shown_blur_contrast:.1}"
    );
    assert!(
        shown_sharp_contrast > shown_blur_contrast * 8.0,
        "屏幕上必须能看出模糊：sharp={shown_sharp_contrast:.1} blurred={shown_blur_contrast:.1}"
    );

    // 5) 尺寸对不上（例如显示器刚换过）时退回纯色，而不是画出一块透明：
    let small_content = super::super::win::d2d::DimSource {
        width: width / 4,
        height: height / 4,
        pixels: dim_test_checkerboard(width / 4, height / 4, 2),
    };
    layer
        .show(monitor, selection, Some(&small_content))
        .expect("遮罩层应当显示");
    layer.set_capture_exclusion(false);
    dim_test_pump(260);
    let fallback_outside = dim_test_screen_mean(outside);
    eprintln!("[dim-live] 尺寸不匹配的兜底：mean {baseline_outside:.1} -> {fallback_outside:.1}");
    assert!(
        fallback_outside <= baseline_outside - 25.0,
        "尺寸不匹配时应当退回纯色（仍然压暗）：{baseline_outside:.1} -> {fallback_outside:.1}"
    );

    // 6) 区域形状：环在内、洞不在内（客户区坐标 —— 上一版就是在这里传错坐标系）。
    let hwnd = layer.hwnd_for_test();
    let rgn = unsafe { windows_sys::Win32::Graphics::Gdi::CreateRectRgn(0, 0, 0, 0) };
    let copied = unsafe { GetWindowRgn(hwnd, rgn) };
    assert!(copied != 0, "窗口应当有区域");
    let client = |rect: Rect| (rect.left + 20 - monitor.left, rect.top + 20 - monitor.top);
    let inside_point = client(inside);
    let outside_point = client(outside);
    let inside_in_region = unsafe { PtInRegion(rgn, inside_point.0, inside_point.1) } != 0;
    let outside_in_region = unsafe { PtInRegion(rgn, outside_point.0, outside_point.1) } != 0;
    unsafe { windows_sys::Win32::Graphics::Gdi::DeleteObject(rgn as _) };
    assert!(outside_in_region, "环应当留在区域里");
    assert!(!inside_in_region, "洞必须是空的（客户区坐标）");

    // 7) 排除出截图：延时贴图不会把遮罩带上。
    layer
        .show(monitor, selection, Some(&blurred_content))
        .expect("遮罩层应当显示");
    layer.set_capture_exclusion(true);
    dim_test_pump(200);
    let excluded_outside = dim_test_screen_mean(outside);
    assert!(
        (excluded_outside - baseline_outside).abs() <= 6.0,
        "排除出截图后，截图里应当看不到遮罩：{baseline_outside:.1} -> {excluded_outside:.1}"
    );

    // 8) 隐藏后回到基线。
    layer.set_capture_exclusion(false);
    layer.hide();
    dim_test_pump(200);
    let hidden_outside = dim_test_screen_mean(outside);
    assert!(
        (hidden_outside - baseline_outside).abs() <= 6.0,
        "隐藏后必须回到基线：{baseline_outside:.1} -> {hidden_outside:.1}"
    );
}
