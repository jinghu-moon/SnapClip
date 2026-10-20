use std::time::Duration;

/// Motion tokens shared by panel controls. Values follow the component reference demo.
pub const FAST: Duration = Duration::from_millis(100);
pub const NORMAL: Duration = Duration::from_millis(200);
pub const SLOW: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Easing {
    Linear,
    EaseOut,
    Spring,
}

/// A time-based transition with no window or renderer dependency.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    pub duration: Duration,
    pub easing: Easing,
}

impl Motion {
    pub const fn new(duration: Duration, easing: Easing) -> Self {
        Self { duration, easing }
    }

    /// Returns a normalized progress value in `[0, 1]`.
    pub fn progress(self, elapsed: Duration, reduced_motion: bool) -> f32 {
        if reduced_motion || self.duration.is_zero() {
            return 1.0;
        }
        let t = (elapsed.as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0);
        match self.easing {
            Easing::Linear => t,
            Easing::EaseOut => ease_out(t),
            Easing::Spring => spring(t),
        }
    }
}

pub const SEGMENT_SLIDE: Motion = Motion::new(SLOW, Easing::EaseOut);

/// A short, damped overshoot used for controls that settle into a new state.
///
/// The endpoint is handled explicitly by `Motion::progress`; the analytic curve is only used for
/// the open interval, where a small overshoot gives the same tactile landing as the demo's spring
/// cubic-bezier without introducing a frame clock into the shared crate.
pub fn spring(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t == 0.0 || t == 1.0 {
        return t;
    }
    let decay = (-7.0 * t).exp();
    let oscillation = (9.0 * t).cos() + (7.0 / 9.0) * (9.0 * t).sin();
    (1.0 - decay * oscillation).clamp(0.0, 1.06)
}

/// Cubic ease-out used by the demo's sliding indicator.
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_has_stable_endpoints() {
        let motion = Motion::new(NORMAL, Easing::EaseOut);
        assert_eq!(motion.progress(Duration::ZERO, false), 0.0);
        assert_eq!(motion.progress(NORMAL, false), 1.0);
        assert_eq!(motion.progress(NORMAL + Duration::from_secs(1), false), 1.0);
    }

    #[test]
    fn ease_out_is_monotonic_and_bounded() {
        let mut previous = 0.0;
        for step in 0..=100 {
            let value = ease_out(step as f32 / 100.0);
            assert!((0.0..=1.0).contains(&value));
            assert!(value >= previous);
            previous = value;
        }
    }

    #[test]
    fn reduced_motion_jumps_to_the_end() {
        let motion = Motion::new(SLOW, Easing::EaseOut);
        assert_eq!(motion.progress(Duration::from_millis(1), true), 1.0);
    }

    #[test]
    fn spring_starts_at_zero_and_settles_at_one_with_a_small_overshoot() {
        assert_eq!(spring(0.0), 0.0);
        assert_eq!(spring(1.0), 1.0);
        assert!(spring(0.35) > 1.0);
        assert!((0.0..=1.06).contains(&spring(0.5)));
    }
}
