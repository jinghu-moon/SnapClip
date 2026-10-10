//! Which window a scroll session reads from (`docs/32` §4.2; task `P7.02`).
//!
//! The selection comes from an ordinary capture session; the window comes from the
//! window-detection snapshot. This module is the rule that turns one into the other, and it is
//! deliberately platform-free (`docs/30` §28.4): a window is an opaque `u64` here, and the
//! `HWND` → `u64` conversion together with the screen → monitor-local conversion live in
//! `windows/scroll_target.rs`.
//!
//! Five rules, in evaluation order. Each one is a test in this file:
//!
//! 1. a candidate that does not overlap the selection is not a candidate;
//! 2. the largest intersection area wins;
//! 3. an **exact** tie in area is broken by `z_order` (lower = frontmost, the order
//!    `window_detection::snapshot` assigns) — a deterministic answer exists, so the rule takes
//!    it rather than asking;
//! 4. the winner must reach `min_extent` on the session's axis, else `TooSmall`. Judged **before**
//!    ambiguity, because "this window is too small to scroll" is a fact about the winner alone
//!    and stays true whoever it is compared against;
//! 5. a runner-up within [`TIE_RATIO`] of the winner — and not exactly equal, which rule 3
//!    already answered — is `Ambiguous`: the rule refuses to guess, and the caller asks the user
//!    which window they meant.
//!
//! `crop` is the selection clipped to the winner's bounds: frames arrive as whole windows, so the
//! part the user picked is an intersection rather than a second capture.
//!
//! Nothing here is reachable from production yet: the caller is the assembly root (`docs/32`
//! `P7.04`). The module therefore carries its own `allow(dead_code)`, the way `scroll/ports.rs`
//! does for its unread corners — the alternative (letting the warning appear now and disappear
//! later) loses the reason.

#![allow(dead_code)] // The caller is the assembly root (docs/32 P7.04); this is what P7.02 owes.

use crate::geometry::Rect;
use crate::scroll::observation::Axis;

/// Smallest primary-axis extent a target may have (`docs/30` §16.2.1).
///
/// Derived there from `MIN_TILES = 4` tiles plus the overlap floor: below this the loop cannot
/// gather enough independent evidence per step for the closed loop to be worth starting.
pub const MIN_TARGET_EXTENT: u32 = 448;

/// Two candidates closer than this fraction of the winner's area are a near tie.
pub const TIE_RATIO: f64 = 0.05;

/// A window the selection could belong to, in the **same coordinate space as the selection**
/// (monitor-local physical pixels — the overlay's space, not the snapshot's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetCandidate {
    /// Opaque window identity (`docs/32` ADR-25). The session only hands it back to the platform
    /// unchanged, so nothing on this side needs to know it is a window handle.
    pub window: u64,
    /// The window's visible bounds — the rectangle WGC delivers (`docs/31` `P3.10`).
    pub bounds: Rect,
    /// Enumeration order, `0` = frontmost. Lower wins an exact tie.
    pub z_order: u32,
}

/// The window a scroll session reads from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollTarget {
    window: u64,
    bounds: Rect,
    crop: Rect,
}

impl ScrollTarget {
    pub fn new(window: u64, bounds: Rect, crop: Rect) -> Self {
        Self {
            window,
            bounds,
            crop,
        }
    }

    pub fn window(&self) -> u64 {
        self.window
    }

    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// The selection clipped to [`Self::bounds`]: the session's cross axis.
    pub fn crop(&self) -> Rect {
        self.crop
    }
}

/// What the rule decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetChoice {
    Accepted(ScrollTarget),
    /// The selection overlaps no usable candidate at all.
    NoWindow,
    /// The winner cannot supply enough independent evidence per step.
    TooSmall { window: u64, extent: u32 },
    /// Two candidates are within [`TIE_RATIO`]; the caller asks which one.
    Ambiguous { first: u64, second: u64 },
}

/// Choose the window with the shipped floor.
pub fn choose_target(selection: Rect, candidates: &[TargetCandidate], axis: Axis) -> TargetChoice {
    choose_target_with_floor(selection, candidates, axis, MIN_TARGET_EXTENT)
}

/// The same rule with an injectable floor.
///
/// Both limits of this pipeline stay injectable so a test can state a three-line case
/// (`docs/30` §17.9).
pub fn choose_target_with_floor(
    selection: Rect,
    candidates: &[TargetCandidate],
    axis: Axis,
    min_extent: u32,
) -> TargetChoice {
    // Rules 1 and 2: overlap decides candidacy, area decides the order. `z_order` is the
    // secondary key, which is rule 3.
    //
    // The filter asks `is_empty` rather than `area() > 0`, and that is not a style choice:
    // `Rect::width`/`height` use `saturating_sub`, which guards against i32 overflow but does
    // **not** clamp to zero (`snapclip-model/src/geometry.rs` says so in the test that pins it),
    // so a disjoint pair multiplies two negative extents into a positive "area" and would pass a
    // naive `> 0` test. The first RED run of this module failed exactly there.
    let mut scored: Vec<(Rect, TargetCandidate)> = candidates
        .iter()
        .copied()
        .map(|candidate| (selection.intersect(candidate.bounds), candidate))
        .filter(|(overlap, _)| !overlap.is_empty())
        .collect();
    scored.sort_by(|left, right| {
        right
            .0
            .area()
            .cmp(&left.0.area())
            .then(left.1.z_order.cmp(&right.1.z_order))
    });

    let Some(&(overlap, best)) = scored.first() else {
        return TargetChoice::NoWindow;
    };

    // Rule 4. `primary_extent` takes physical width/height in that order.
    let extent = axis.primary_extent(
        best.bounds.width().max(0) as u32,
        best.bounds.height().max(0) as u32,
    );
    if extent < min_extent {
        return TargetChoice::TooSmall {
            window: best.window,
            extent,
        };
    }

    // Rule 5, on top of rule 3: an exact tie was already decided by `z_order` above, so only a
    // *near* tie is ambiguous.
    if let Some(&(second_overlap, second)) = scored.get(1) {
        let best_area = overlap.area();
        let second_area = second_overlap.area();
        if second_area != best_area && near_tie(best_area, second_area) {
            return TargetChoice::Ambiguous {
                first: best.window,
                second: second.window,
            };
        }
    }

    TargetChoice::Accepted(ScrollTarget::new(best.window, best.bounds, overlap))
}

fn near_tie(best: i64, second: i64) -> bool {
    (best - second) as f64 <= TIE_RATIO * best as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(window: u64, bounds: Rect, z_order: u32) -> TargetCandidate {
        TargetCandidate {
            window,
            bounds,
            z_order,
        }
    }

    /// Rule 2.
    #[test]
    fn the_containing_window_wins_by_intersection_area() {
        let selection = Rect::new(100, 100, 300, 400);
        let behind = candidate(0xA, Rect::new(0, 0, 1000, 1000), 1);
        let front = candidate(0xB, Rect::new(250, 0, 1000, 1000), 0);
        // 0xA overlaps 100×300, 0xB overlaps 50×300 ⇒ the larger intersection wins even though
        // it is not frontmost.
        assert_eq!(
            choose_target(selection, &[front, behind], Axis::Vertical),
            TargetChoice::Accepted(ScrollTarget::new(
                0xA,
                Rect::new(0, 0, 1000, 1000),
                Rect::new(100, 100, 300, 400)
            ))
        );
    }

    /// Rule 3.
    #[test]
    fn z_order_breaks_an_exact_tie() {
        let selection = Rect::new(0, 0, 100, 100);
        let front = candidate(0x1, Rect::new(0, 0, 100, 1000), 0);
        let behind = candidate(0x2, Rect::new(0, 0, 100, 1000), 3);
        match choose_target(selection, &[front, behind], Axis::Vertical) {
            TargetChoice::Accepted(target) => assert_eq!(target.window(), 0x1),
            other => panic!("an exact tie has a deterministic answer, got {other:?}"),
        }
        // …and the answer follows `z_order` rather than input order.
        match choose_target(selection, &[behind, front], Axis::Vertical) {
            TargetChoice::Accepted(target) => assert_eq!(target.window(), 0x1),
            other => panic!("a deterministic tie must not depend on input order, got {other:?}"),
        }
    }

    /// Rule 5.
    #[test]
    fn a_near_tie_is_ambiguous_rather_than_guessed() {
        let selection = Rect::new(0, 0, 100, 100);
        // 100×100 vs 99×100 → 1% apart, inside TIE_RATIO: a 1 px change would flip the winner.
        let bigger = candidate(0x1, Rect::new(0, 0, 100, 1000), 0);
        let smaller = candidate(0x2, Rect::new(1, 0, 100, 1000), 1);
        assert_eq!(
            choose_target(selection, &[bigger, smaller], Axis::Vertical),
            TargetChoice::Ambiguous {
                first: 0x1,
                second: 0x2
            }
        );
    }

    /// Rule 1.
    #[test]
    fn no_overlap_is_no_window() {
        let selection = Rect::new(0, 0, 100, 100);
        let elsewhere = candidate(0x1, Rect::new(500, 500, 900, 900), 0);
        assert_eq!(
            choose_target(selection, &[elsewhere], Axis::Vertical),
            TargetChoice::NoWindow
        );
        assert_eq!(choose_target(selection, &[], Axis::Vertical), TargetChoice::NoWindow);
        // Touching edges do not overlap: `right`/`bottom` are exclusive.
        let touching = candidate(0x1, Rect::new(100, 0, 400, 400), 0);
        assert_eq!(
            choose_target(selection, &[touching], Axis::Vertical),
            TargetChoice::NoWindow
        );
    }

    /// Rule 4.
    #[test]
    fn a_candidate_below_the_viewport_floor_is_too_small() {
        let selection = Rect::new(0, 0, 600, 300);
        let short = candidate(0x1, Rect::new(0, 0, 600, 300), 0);
        let tall = candidate(0x2, Rect::new(0, 0, 600, MIN_TARGET_EXTENT as i32), 1);
        assert_eq!(
            choose_target(selection, &[short], Axis::Vertical),
            TargetChoice::TooSmall {
                window: 0x1,
                extent: 300
            }
        );
        // The floor itself is enough: the comparison is `<`, not `<=`.
        match choose_target(selection, &[tall], Axis::Vertical) {
            TargetChoice::Accepted(target) => assert_eq!(target.window(), 0x2),
            other => panic!("the floor is inclusive, got {other:?}"),
        }
    }

    /// Rule 4's place in the order: size is judged before ambiguity.
    #[test]
    fn too_small_is_judged_before_ambiguity() {
        let selection = Rect::new(0, 0, 100, 100);
        let short_a = candidate(0x1, Rect::new(0, 0, 100, 100), 0);
        let short_b = candidate(0x2, Rect::new(1, 0, 100, 100), 1);
        // Nearly tied *and* both too small ⇒ the actionable fact is the size.
        assert_eq!(
            choose_target(selection, &[short_a, short_b], Axis::Vertical),
            TargetChoice::TooSmall {
                window: 0x1,
                extent: 100
            }
        );
    }

    /// The axis decides which extent the floor measures.
    #[test]
    fn the_axis_decides_which_extent_is_measured() {
        let selection = Rect::new(0, 0, 600, 300);
        let wide_and_short = candidate(0x1, Rect::new(0, 0, 600, 300), 0);
        assert!(matches!(
            choose_target(selection, &[wide_and_short], Axis::Vertical),
            TargetChoice::TooSmall { extent: 300, .. }
        ));
        match choose_target(selection, &[wide_and_short], Axis::Horizontal) {
            TargetChoice::Accepted(target) => assert_eq!(target.window(), 0x1),
            other => panic!("600 px across is enough horizontally, got {other:?}"),
        }
    }

    /// `crop` is the intersection, not the window and not the raw selection.
    #[test]
    fn the_crop_is_the_selection_inside_the_window() {
        // The selection hangs 50 px past the window's right edge.
        let selection = Rect::new(500, 10, 700, 900);
        let window = candidate(0x1, Rect::new(0, 0, 650, 1000), 0);
        match choose_target(selection, &[window], Axis::Vertical) {
            TargetChoice::Accepted(target) => {
                assert_eq!(target.bounds(), Rect::new(0, 0, 650, 1000));
                assert_eq!(target.crop(), Rect::new(500, 10, 650, 900));
            }
            other => panic!("{other:?}"),
        }
    }
}
