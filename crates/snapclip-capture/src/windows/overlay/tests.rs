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
            chain_visibility_at(Duration::from_millis(
                CHAIN_FADE_AFTER_MS + CHAIN_FADE_MS
            )),
            0.0
        );
        assert_eq!(chain_visibility_at(Duration::from_secs(60)), 0.0);

        // The bound: at most one distinct value per step, which is at most eight repaints however
        // often the fade is sampled. A per-frame alpha would have been ~16 values over the same
        // 240 ms (docs/21 §5.22).
        let sampled: std::collections::BTreeSet<u32> =
            (0..=CHAIN_FADE_AFTER_MS + CHAIN_FADE_MS + 100)
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
        assert_eq!(level_hint(reach), "↑7 ↓1 · ↑ = 窗口 · 滚轮 / ↑↓ 切换");
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
        assert!(WHEEL_SETTLE_MS < 120, "the tail of a flick, not the next flick");
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
        assert_eq!(walk.activity(), 1.0, "the rise has to finish by {WALK_RISE_MS} ms");
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
        assert!(distinct.len() <= 20, "the fade may not cost more than ~20 frames");

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
        assert_eq!(appear_share(Duration::from_millis(RING_APPEAR_MS), PREVIEW_APPEAR_MS), 1.0);
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
        assert!(walk.running(t0), "the rise needs the tick before it can be seen");
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
