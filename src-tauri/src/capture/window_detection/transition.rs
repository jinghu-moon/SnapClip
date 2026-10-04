//! Preview-rectangle transition (docs/18 §10).
//!
//! Deep selection follows the nearest control, so a cursor crossing a window retargets the
//! highlight many times per second. Painting the raw target would make the frame jump
//! between control sizes; the reference selector solved this with a 101 ms OutQuad animation
//! (`screenshotsmartselectiontransition.cpp`) and this is the same behaviour expressed as a
//! pure value function.
//!
//! The type deliberately owns no timer and no window: the overlay drives it from its
//! existing 15 ms coalescing render tick, which keeps the animation on the same clock as
//! every other repaint and makes every branch testable at an arbitrary time point.

use std::time::{Duration, Instant};

use crate::capture::geometry::Rect;

/// Duration of one preview transition, matching the reference implementation.
pub const PREVIEW_TRANSITION_MS: u32 = 101;

/// OutQuad easing: quick at the start, settling at the end
/// (`QEasingCurve::OutQuad`, i.e. `1 - (1 - t)²`).
pub fn out_quad(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t)
}

/// Interpolate two rectangles with `amount` in `0.0..=1.0`.
///
/// Edges are interpolated independently, so a transition between two control sizes
/// animates both position and size smoothly (the reference animates a `QRectF` the same way).
pub fn lerp_rect(from: Rect, to: Rect, amount: f32) -> Rect {
    let amount = amount.clamp(0.0, 1.0);
    let mix = |a: i32, b: i32| (a as f32 + (b as f32 - a as f32) * amount).round() as i32;
    Rect::new(
        mix(from.left, to.left),
        mix(from.top, to.top),
        mix(from.right, to.right),
        mix(from.bottom, to.bottom),
    )
}

/// An interruptible transition of the preview rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectTransition {
    from: Rect,
    to: Rect,
    started_at: Instant,
    duration: Duration,
}

impl RectTransition {
    /// A transition that is already at `rect` and not animating.
    pub fn settled(rect: Rect, now: Instant) -> Self {
        Self {
            from: rect,
            to: rect,
            started_at: now,
            duration: Duration::from_millis(u64::from(PREVIEW_TRANSITION_MS)),
        }
    }

    /// Start easing from `from` to `to`.
    ///
    /// `from` is the rectangle that is *currently displayed*, not the previous target: an
    /// interrupted transition therefore continues from where the eye last saw the frame,
    /// which is what keeps re-targeting smooth instead of snapping back.
    pub fn start(&mut self, from: Rect, to: Rect, now: Instant) {
        self.from = from;
        self.to = to;
        self.started_at = now;
    }

    /// Stop animating and jump to `rect`.
    pub fn present(&mut self, rect: Rect, now: Instant) {
        *self = Self::settled(rect, now);
    }

    /// The rectangle to paint at `now`.
    pub fn value_at(&self, now: Instant) -> Rect {
        if self.from == self.to {
            return self.to;
        }
        let elapsed = now.saturating_duration_since(self.started_at);
        let total = self.duration.as_secs_f32().max(f32::EPSILON);
        lerp_rect(self.from, self.to, out_quad(elapsed.as_secs_f32() / total))
    }

    /// Whether the transition is still moving at `now`.
    pub fn is_running(&self, now: Instant) -> bool {
        self.from != self.to && now.saturating_duration_since(self.started_at) < self.duration
    }

    /// The rectangle the transition is heading for.
    pub fn target(&self) -> Rect {
        self.to
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect::new(left, top, right, bottom)
    }

    #[test]
    fn out_quad_is_a_normalised_decelerating_curve() {
        assert_eq!(out_quad(0.0), 0.0);
        assert_eq!(out_quad(1.0), 1.0);
        // OutQuad reaches 75 % at the midpoint: it starts fast and settles.
        assert!((out_quad(0.5) - 0.75).abs() < 1e-6);
        // Monotone and clamped outside the unit interval.
        assert!(out_quad(0.25) < out_quad(0.75));
        assert_eq!(out_quad(-3.0), 0.0);
        assert_eq!(out_quad(9.0), 1.0);
    }

    #[test]
    fn interpolation_hits_both_ends_and_blends_both_size_and_position() {
        let from = rect(0, 0, 100, 100);
        let to = rect(50, 20, 350, 220);
        assert_eq!(lerp_rect(from, to, 0.0), from);
        assert_eq!(lerp_rect(from, to, 1.0), to);
        assert_eq!(lerp_rect(from, to, 0.5), rect(25, 10, 225, 160));
    }

    #[test]
    fn a_transition_starts_at_the_current_value_and_ends_at_the_target() {
        let now = Instant::now();
        let from = rect(0, 0, 100, 100);
        let to = rect(0, 0, 200, 200);
        let mut transition = RectTransition::settled(from, now);
        transition.start(from, to, now);

        assert_eq!(transition.value_at(now), from, "t=0 is the displayed rectangle");
        assert!(transition.is_running(now));
        // Half way through the duration the OutQuad curve is already ~75 % of the way to
        // the target: 100 + round(0.745 * 100) = 175. That fast-then-settling shape is what
        // keeps a re-target mid-flight from looking like a jump.
        let half = now + Duration::from_millis(u64::from(PREVIEW_TRANSITION_MS / 2));
        assert_eq!(transition.value_at(half), rect(0, 0, 175, 175));
        let end = now + Duration::from_millis(u64::from(PREVIEW_TRANSITION_MS));
        assert_eq!(transition.value_at(end), to);
        assert!(
            !transition.is_running(end),
            "the animation is finished exactly at the duration"
        );
        // Past the end it stays at the target rather than overshooting.
        assert_eq!(
            transition.value_at(end + Duration::from_millis(500)),
            to
        );
    }

    #[test]
    fn retargeting_mid_flight_continues_from_what_is_displayed() {
        let now = Instant::now();
        let start = rect(0, 0, 100, 100);
        let first_target = rect(0, 0, 300, 300);
        let mut transition = RectTransition::settled(start, now);
        transition.start(start, first_target, now);

        // The cursor moved to another control 40 ms in: take over from the displayed value.
        let midway = now + Duration::from_millis(40);
        let displayed = transition.value_at(midway);
        assert_ne!(displayed, start);
        assert_ne!(displayed, first_target);
        let second_target = rect(500, 400, 700, 600);
        transition.start(displayed, second_target, midway);

        assert_eq!(
            transition.value_at(midway),
            displayed,
            "no jump back to the previous target when retargeting"
        );
        assert_eq!(transition.target(), second_target);
        let end = midway + Duration::from_millis(u64::from(PREVIEW_TRANSITION_MS));
        assert_eq!(transition.value_at(end), second_target);
        assert!(!transition.is_running(end));
    }

    #[test]
    fn a_settled_transition_is_never_running() {
        let now = Instant::now();
        let only = rect(10, 10, 20, 20);
        let transition = RectTransition::settled(only, now);
        assert!(!transition.is_running(now));
        assert_eq!(transition.value_at(now), only);
        assert_eq!(transition.target(), only);
        // Even arbitrarily far in the future it stays put.
        assert!(!transition.is_running(now + Duration::from_secs(10)));
        assert_eq!(transition.value_at(now + Duration::from_secs(10)), only);
    }

    #[test]
    fn starting_and_ending_on_the_same_rectangle_does_not_animate() {
        // The caller uses `is_running` to decide whether to keep scheduling repaints, so a
        // same-target update must not keep the render tick alive forever.
        let now = Instant::now();
        let only = rect(10, 10, 20, 20);
        let mut transition = RectTransition::settled(only, now);
        transition.start(only, only, now);
        assert!(!transition.is_running(now));
        assert_eq!(transition.value_at(now + Duration::from_millis(1)), only);
    }

    #[test]
    fn presenting_directly_cancels_a_running_transition() {
        let now = Instant::now();
        let mut transition = RectTransition::settled(rect(0, 0, 10, 10), now);
        transition.start(rect(0, 0, 10, 10), rect(0, 0, 400, 400), now);
        assert!(transition.is_running(now));
        let direct = rect(7, 7, 9, 9);
        transition.present(direct, now + Duration::from_millis(10));
        assert!(!transition.is_running(now + Duration::from_millis(11)));
        assert_eq!(transition.value_at(now + Duration::from_millis(11)), direct);
    }
}
