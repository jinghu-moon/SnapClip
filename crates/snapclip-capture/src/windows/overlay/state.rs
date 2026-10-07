//! Session, hover, walk and hint state machines (docs/23 T1.6.3, value types).
//!
//! These are plain values with their own clocks: the controller holds them and drives
//! them from its message handlers, but none of them knows about Win32.

use super::*;

/// The units one clicky-wheel notch is reported in — Windows' `WHEEL_DELTA`.
pub(crate) const WHEEL_NOTCH_UNITS: i32 = 120;
/// Sub-notch wheel units that add up to one level step (v3 B2, docs/21 §5.24).
///
/// A notch is 120 units, so one notch is one level — exactly what the wheel did before. A touchpad
/// sends a long tail of small deltas instead, and without an accumulator one flick walks five
/// levels, which is the difference between "the wheel is precise" and "the wheel is possessed".
pub(crate) const WHEEL_UNITS_PER_STEP: i32 = 100;
/// A gap this long with no wheel input drops the accumulated remainder: separate gestures must not
/// add up.
pub(crate) const WHEEL_IDLE_RESET_MS: u64 = 220;
/// After a step, ignore *sub-notch* input for this long: a flick's inertia tail arrives as more of
/// those and must not walk another level.
pub(crate) const WHEEL_SETTLE_MS: u64 = 80;

/// Turns a stream of wheel deltas into level steps (v3 B2, docs/21 §5.24).
///
/// Pure apart from the clock it is handed, so the mouse and touchpad behaviours are testable rather
/// than a matter of feel.
#[derive(Debug, Default)]
pub(crate) struct WheelAccumulator {
    pub(crate) residue: i32,
    pub(crate) last_input: Option<Instant>,
    pub(crate) settled_until: Option<Instant>,
}

impl WheelAccumulator {
    /// Feed one wheel event; the result is how many levels to walk, signed, and `0` means none.
    ///
    /// A whole notch always steps, however fast the wheel is spun: the settle window exists for the
    /// touchpad's inertia tail, which never arrives as a notch, and letting it swallow notches would
    /// make a free-spinning wheel crawl. The window therefore gates only non-notch deltas.
    pub(crate) fn steps(&mut self, delta: i32, now: Instant) -> i32 {
        if delta == 0 {
            return 0;
        }
        // A clicky wheel reports whole multiples of `WHEEL_DELTA` — one per detent, so one message
        // can carry several notches (a coalesced fast spin) and each of them is a level. Dropping
        // the extra would make a fast wheel slower than a slow one.
        if delta % WHEEL_NOTCH_UNITS == 0 {
            self.residue = 0;
            self.last_input = Some(now);
            self.settled_until = Some(now + Duration::from_millis(WHEEL_SETTLE_MS));
            return delta / WHEEL_NOTCH_UNITS;
        }
        if self.settled_until.is_some_and(|until| now < until) {
            return 0;
        }
        if self
            .last_input
            .is_some_and(|last| now.duration_since(last).as_millis() as u64 > WHEEL_IDLE_RESET_MS)
        {
            self.residue = 0;
        }
        self.last_input = Some(now);
        self.residue += delta;
        // Exact division, so a hard flick that arrives as one large delta walks the levels it covers
        // instead of silently losing them.
        let steps = self.residue / WHEEL_UNITS_PER_STEP;
        if steps == 0 {
            return 0;
        }
        self.residue -= steps * WHEEL_UNITS_PER_STEP;
        self.settled_until = Some(now + Duration::from_millis(WHEEL_SETTLE_MS));
        steps
    }
}

/// The capture box's walk colour: how green it is, and the walk that put it there (docs/21 §5.24).
///
/// A type rather than three loose fields on the controller, and pure apart from the clock it is
/// handed, for the same reason [`WheelAccumulator`] is: the state that is easy to get wrong here is
/// "**no walk has happened yet**", which reads as zero but is not the same as "the colour has come
/// back down". The first cut guarded the step on `activity > 0.0` — true of the *chain* fade, whose
/// touch sets it to 1 immediately, and false at the bottom of a rise that starts from nothing, so
/// the box never turned green at all.
#[derive(Debug)]
pub(crate) struct WalkColour {
    /// The painted share of the capture green, `0.0..=1.0`.
    pub(crate) activity: f32,
    /// The value the current rise started from, so a walk that arrives mid-fade continues from what
    /// is on screen instead of flashing back to blue.
    pub(crate) from: f32,
    /// When the last walk was; `None` until the first one of the session, and again once the colour
    /// has finished coming back down.
    pub(crate) touched_at: Option<Instant>,
}

impl Default for WalkColour {
    fn default() -> Self {
        Self {
            activity: 0.0,
            from: 0.0,
            touched_at: None,
        }
    }
}

impl WalkColour {
    /// A walk happened: the rise starts on the next tick.
    pub(crate) fn touch(&mut self, now: Instant) {
        self.from = self.activity;
        self.touched_at = Some(now);
    }

    /// How green the box is.
    pub(crate) fn activity(&self) -> f32 {
        self.activity
    }

    /// Advance one tick; `true` when the painted value changed.
    ///
    /// Both halves are functions of the time since the walk, not accumulated steps: the rise is eased
    /// from where this walk started and arrives at [`WALK_RISE_MS`], and the fall is the shared eased
    /// envelope. That is what makes the colour continuous (docs/21 §5.24, ①) while the rings' alpha
    /// stays quantised.
    pub(crate) fn advance(&mut self, now: Instant) -> bool {
        let Some(touched) = self.touched_at else {
            // Nothing has walked: a tick must never be what turns the box green.
            return false;
        };
        let idle = now.saturating_duration_since(touched);
        let rise = self.from + (1.0 - self.from) * appear_share(idle, WALK_RISE_MS);
        let visible = 1.0 - fade_at(idle, CHAIN_FADE_AFTER_MS, CHAIN_FADE_MS);
        let activity = rise.min(visible).clamp(0.0, 1.0);
        if activity <= 0.0 {
            // The envelope is over: forget the walk, so the tick stops asking and the next walk gets
            // a fresh rise instead of inheriting a timestamp from the last one.
            self.touched_at = None;
        }
        if (activity - self.activity).abs() < f32::EPSILON {
            return false;
        }
        self.activity = activity;
        true
    }

    /// Whether the value is still moving, so the render tick has to keep itself armed.
    pub(crate) fn running(&self, now: Instant) -> bool {
        let Some(touched) = self.touched_at else {
            return false;
        };
        // From the instant of the touch: the first tick of a rise has nothing to compare against.
        self.activity < 1.0
            || now.saturating_duration_since(touched).as_millis() as u64 >= CHAIN_FADE_AFTER_MS
    }

    /// Whether the colour is on at all — what keeps *one* repaint alive to notice the hold expiring.
    pub(crate) fn pending(&self) -> bool {
        self.activity > 0.0
    }
}

/// How visible the chain of rings is: eased in when it is used, quantised away when it goes quiet
/// (docs/21 §5.22, §5.24 ②③).
///
/// The **rise** is new: the chain used to appear at full strength in the same frame as the answer
/// that created it, while its disappearance had been animated for two rounds — which is exactly the
/// asymmetry the user noticed. The **fall** stays quantised: eight steps of alpha are invisible and
/// each distinct value is a full-surface repaint, so the coarse ramp is the cost model (①).
#[derive(Debug, Default)]
pub(crate) struct ChainVisibility {
    pub(crate) visibility: f32,
    /// The value the current rise started from — a chain re-used mid-fade continues from what is on
    /// screen rather than jumping back to full.
    pub(crate) from: f32,
    /// When the chain was last used; `None` until the first walk or answer of the session.
    ///
    /// Explicit for the same reason [`WalkColour`]'s is: "not used yet" reads as zero and is not the
    /// same state as "faded away", and a tick must never be what makes the chain appear.
    pub(crate) touched_at: Option<Instant>,
}

impl ChainVisibility {
    /// The chain was just used; the rise starts on the next tick.
    pub(crate) fn touch(&mut self, now: Instant) {
        self.from = self.visibility;
        self.touched_at = Some(now);
    }

    pub(crate) fn value(&self) -> f32 {
        self.visibility
    }

    /// Advance one tick; `true` when the painted value changed.
    pub(crate) fn advance(&mut self, now: Instant) -> bool {
        let Some(touched) = self.touched_at else {
            return false;
        };
        let idle = now.saturating_duration_since(touched);
        let rise = self.from + (1.0 - self.from) * appear_share(idle, CHAIN_RISE_MS);
        let visible = chain_visibility_at(idle);
        let next = rise.min(visible).clamp(0.0, 1.0);
        if (next - self.visibility).abs() < f32::EPSILON {
            return false;
        }
        self.visibility = next;
        true
    }

    /// Whether the value is still moving, so the render tick has to keep itself armed.
    pub(crate) fn running(&self, now: Instant) -> bool {
        let Some(touched) = self.touched_at else {
            return false;
        };
        self.visibility < 1.0
            || now.saturating_duration_since(touched).as_millis() as u64 >= CHAIN_FADE_AFTER_MS
    }

    /// Whether there is still a fade to come or to finish — what keeps *one* repaint alive to notice
    /// the 1.2 s deadline while nothing else is happening (docs/21 §5.22).
    pub(crate) fn pending(&self) -> bool {
        self.visibility > 0.0
    }
}

/// Which ring rectangles are on screen, and since when (docs/21 §5.24, ②).
///
/// Keyed by the rectangle rather than by the level index: walking up a level drops the outermost
/// ring and adds one inside, and an index cannot tell "the same ring, still here" from "a different
/// ring" — while the rectangle can, which is what keeps the rings that survived a walk from
/// re-animating.
#[derive(Debug, Default)]
pub(crate) struct RingAppear {
    pub(crate) seen: Vec<(Rect, Instant)>,
}

impl RingAppear {
    /// How far in `rect` is, starting its clock the first time it is drawn.
    pub(crate) fn share(&mut self, rect: Rect, now: Instant) -> f32 {
        let since = match self.seen.iter_mut().find(|(seen, _)| *seen == rect) {
            Some((_, since)) => *since,
            None => {
                self.seen.push((rect, now));
                now
            }
        };
        appear_share(now.saturating_duration_since(since), RING_APPEAR_MS)
    }

    /// Forget the rings that are gone, so the next time one of them appears it fades in again.
    pub(crate) fn retain(&mut self, live: &[Rect]) {
        self.seen.retain(|(rect, _)| live.contains(rect));
    }

    pub(crate) fn clear(&mut self) {
        self.seen.clear();
    }
}

/// A one-shot hint that has been armed and is waiting for its deadline (docs/21 §5.21).
///
/// The **text is stored, not recomputed**: what gets painted has to be the sentence that was
/// armed. Recomputing it from live state would let a hint change under the user's eyes (a counter
/// that keeps ticking while the user has stopped walking), and the reading would no longer match
/// the moment it was shown for.
pub(crate) struct ArmedHint {
    pub(crate) until: Instant,
    pub(crate) text: String,
}
