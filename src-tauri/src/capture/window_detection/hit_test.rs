//! Snapshot hit testing and nearest-window resolution (docs/14 §5.4).
//!
//! Everything here is a pure read over a [`WindowSnapshot`]: no `EnumWindows`, no DWM,
//! no allocation on the query path. That is what lets the overlay call it from
//! `WM_MOUSEMOVE` without breaking the promise that mouse movement never blocks the
//! message loop.
//!
//! ## Half-open rectangles
//!
//! A window covers `[left, right) × [top, bottom)`. Adjacent windows share an edge, and
//! a half-open interval makes the shared pixel belong to exactly one of them instead of
//! both. Distance, on the other hand, is measured to the *closed* rectangle, because a
//! geometric distance has to stay symmetric on both sides of an edge. The two
//! conventions meet in [`WindowSnapshot::nearest_target`]: equal distances are broken in
//! favour of the candidate that actually contains the point, and only then by Z order.
//! Without that rule a point sitting exactly on a shared edge would be "0 px from" two
//! windows and the Z order alone would silently decide which one the user meant.
//!
//! ## Z order
//!
//! Candidates are stored frontmost-first (`z_order` ascending). When several windows
//! contain the point — or are equally far away — the lowest `z_order` wins, i.e. the one
//! the user sees on top.

use crate::capture::geometry::Point;
use crate::capture::window_detection::model::{
    HoverValidity, WindowCandidate, WindowSnapshot, WindowTarget,
};

/// Squared Euclidean distance from `point` to a rectangle, in physical pixels².
///
/// Zero for every point inside or on the rectangle's border; otherwise the distance to
/// the nearest point of its closed hull, so the value is symmetric across an edge
/// (a point 5 px to the left and 5 px to the right of an edge both measure 5).
pub fn rect_distance_squared(point: Point, rect: crate::capture::geometry::Rect) -> i64 {
    if rect.is_empty() {
        // A degenerate rectangle has no pixels to be close to. Report the distance to
        // the nearest of its two corners so the value stays finite and monotone.
        let dx = i64::from(rect.left) - i64::from(point.x);
        let dy = i64::from(rect.top) - i64::from(point.y);
        return dx * dx + dy * dy;
    }
    let dx = axis_distance(rect.left, rect.right, point.x);
    let dy = axis_distance(rect.top, rect.bottom, point.y);
    dx * dx + dy * dy
}

/// Distance along one axis to the closed interval `[lo, hi]`.
fn axis_distance(lo: i32, hi: i32, value: i32) -> i64 {
    if value < lo {
        i64::from(lo) - i64::from(value)
    } else if value > hi {
        i64::from(value) - i64::from(hi)
    } else {
        0
    }
}

impl WindowSnapshot {
    /// The frontmost current candidate containing `point`, or `None`.
    ///
    /// This is the hover query. It never leaves the snapshot: the reference selector
    /// established that `WindowFromPoint` is the wrong tool while a full-screen overlay
    /// exists (it would return the overlay itself), and that a Z-ordered scan answers
    /// the question the user is really asking — *which window am I pointing at?*
    pub fn hit_test(&self, point: Point) -> Option<WindowTarget> {
        self.candidates
            .iter()
            .filter(|candidate| self.is_live(candidate))
            .find(|candidate| candidate.screen_bounds.contains(point))
            .map(|candidate| WindowTarget::top_level_window_frame(*candidate))
    }

    /// The nearest live candidate within `snap_radius` physical pixels, or `None`.
    ///
    /// Distance is zero inside a window, so the point's own window always beats a
    /// neighbour. Equal distances — a point exactly on a shared edge, or in the gap
    /// between two windows — are resolved by preferring the candidate that contains the
    /// point, and only then by Z order. `snap_radius` is the documented initial 24 px
    /// ([`crate::capture::window_detection::DEFAULT_SNAP_RADIUS_PX`]).
    pub fn nearest_target(&self, point: Point, snap_radius: u32) -> Option<WindowTarget> {
        let limit = i64::from(snap_radius) * i64::from(snap_radius);
        // (distance², 0 when the point is inside, z_order) — lexicographic.
        let mut best: Option<(i64, u8, u32, WindowCandidate)> = None;
        for candidate in self.candidates.iter().filter(|c| self.is_live(c)) {
            if !candidate.is_usable() {
                continue;
            }
            let distance = rect_distance_squared(point, candidate.screen_bounds);
            if distance > limit {
                continue;
            }
            let outside = u8::from(!candidate.screen_bounds.contains(point));
            let better = match best {
                None => true,
                Some((best_distance, best_outside, best_z, _)) => {
                    (distance, outside, candidate.z_order) < (best_distance, best_outside, best_z)
                }
            };
            if better {
                best = Some((distance, outside, candidate.z_order, *candidate));
            }
        }
        best.map(|(_, _, _, candidate)| WindowTarget::top_level_window_frame(candidate))
    }

    /// Write a re-validation result back into the snapshot (docs/14 §5.5).
    ///
    /// Returns whether anything changed. `BoundsChanged` is applied only when both the
    /// epoch and the identity match the live snapshot: a result produced against a
    /// previous generation, or for a handle that has since been recycled, must not move
    /// a candidate. Skipping this write-back is the root cause of the "hover jumps
    /// back to the old rectangle" defect — the next `hit_test` would keep reading the
    /// stale bounds.
    pub fn apply_candidate_update(&mut self, result: &HoverValidity) -> bool {
        let HoverValidity::BoundsChanged {
            epoch,
            identity,
            new_bounds,
        } = result
        else {
            return false;
        };
        if *epoch != self.epoch || !self.is_current(*epoch) {
            return false;
        }
        let Some(candidate) = self
            .candidates
            .iter_mut()
            .find(|candidate| candidate.identity == *identity)
        else {
            return false;
        };
        if candidate.screen_bounds == *new_bounds {
            return false;
        }
        candidate.screen_bounds = *new_bounds;
        true
    }

    /// Whether a candidate belongs to this snapshot's generation.
    ///
    /// A snapshot whose epoch has been released (`0`) is live for nothing, and a
    /// candidate stamped with another generation is invisible to every query — the
    /// mechanism that makes "a stale snapshot can never answer a hit" a property of the
    /// data rather than of the caller.
    fn is_live(&self, candidate: &WindowCandidate) -> bool {
        self.is_current(candidate.snapshot_epoch) && candidate.is_usable()
    }
}

/// Squared Euclidean distance between two points, named for symmetry with
/// [`rect_distance_squared`].
pub fn point_distance_squared(left: Point, right: Point) -> i64 {
    let dx = i64::from(left.x) - i64::from(right.x);
    let dy = i64::from(left.y) - i64::from(right.y);
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::geometry::Rect;
    use crate::capture::monitor_cache::MonitorCache;
    use crate::capture::window_detection::model::{EpochCounter, SnapshotEpoch, WindowIdentity};

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect::new(left, top, right, bottom)
    }

    fn candidate(hwnd: isize, z_order: u32, bounds: Rect, epoch: SnapshotEpoch) -> WindowCandidate {
        WindowCandidate::new(
            WindowIdentity::new(hwnd, 1000 + hwnd as u32, 0xC0FFEE),
            bounds,
            z_order,
            epoch,
        )
    }

    /// Snapshot with the given `(hwnd, z_order, bounds)` entries, all in one generation.
    fn snapshot(entries: &[(isize, u32, Rect)]) -> WindowSnapshot {
        let mut counter = EpochCounter::new();
        let epoch = counter.bump();
        snapshot_in(epoch, entries)
    }

    fn snapshot_in(epoch: SnapshotEpoch, entries: &[(isize, u32, Rect)]) -> WindowSnapshot {
        let candidates = entries
            .iter()
            .map(|(hwnd, z_order, bounds)| candidate(*hwnd, *z_order, *bounds, epoch))
            .collect();
        WindowSnapshot::new(epoch, candidates, MonitorCache::from_bounds([rect(0, 0, 3840, 2160)]))
    }

    #[test]
    fn distance_is_zero_inside_a_rectangle_and_positive_outside() {
        let window = rect(100, 200, 400, 500);
        for point in [
            Point::new(100, 200),
            Point::new(250, 300),
            Point::new(399, 499),
        ] {
            assert_eq!(rect_distance_squared(point, window), 0, "{point:?}");
            assert!(window.contains(point));
        }
        // The border itself is distance 0 (the closed hull), and one pixel past it is
        // one pixel away — symmetrically with the left/top case below.
        assert_eq!(rect_distance_squared(Point::new(400, 300), window), 0);
        assert_eq!(rect_distance_squared(Point::new(401, 300), window), 1);
        assert_eq!(rect_distance_squared(Point::new(250, 501), window), 1);
        // Diagonal distance uses both axes.
        assert_eq!(rect_distance_squared(Point::new(403, 504), window), 9 + 16);
        // Negative-direction distance.
        assert_eq!(rect_distance_squared(Point::new(96, 200), window), 16);
    }

    #[test]
    fn a_shared_edge_is_zero_distance_from_both_but_belongs_to_one() {
        // Distance is symmetric across the edge, so the tie-break — not the distance —
        // has to decide, and it does so in favour of the window that contains the point.
        let left = rect(0, 0, 100, 100);
        let right = rect(100, 0, 200, 100);
        let shared_edge = Point::new(100, 50);
        assert!(!left.contains(shared_edge));
        assert!(right.contains(shared_edge));
        assert_eq!(rect_distance_squared(shared_edge, left), 0);
        assert_eq!(rect_distance_squared(shared_edge, right), 0);

        let snapshot = snapshot(&[
            (0xA, 0, left),
            (0xB, 1, right),
        ]);
        // hit_test follows containment...
        assert_eq!(snapshot.hit_test(shared_edge).unwrap().identity().hwnd, 0xB);
        // ...and nearest_target agrees even though 0xA is frontmost.
        assert_eq!(
            snapshot.nearest_target(shared_edge, 24).unwrap().identity().hwnd,
            0xB
        );
    }

    #[test]
    fn hit_test_picks_the_frontmost_overlapping_window() {
        let snapshot = snapshot(&[
            (0xA, 0, rect(0, 0, 500, 500)),
            (0xB, 1, rect(100, 100, 400, 400)),
            (0xC, 2, rect(200, 200, 300, 300)),
        ]);
        // All three overlap this point: the frontmost (lowest z_order) wins.
        assert_eq!(snapshot.hit_test(Point::new(250, 250)).unwrap().identity().hwnd, 0xA);
        // Outside the frontmost, inside the second.
        assert_eq!(snapshot.hit_test(Point::new(150, 150)).unwrap().identity().hwnd, 0xA);
        // Outside every window.
        assert_eq!(snapshot.hit_test(Point::new(900, 900)), None);
    }

    #[test]
    fn hit_test_uses_half_open_edges() {
        let snapshot = snapshot(&[
            (0xA, 0, rect(0, 0, 100, 100)),
            (0xB, 1, rect(100, 0, 200, 100)),
        ]);
        // Left/top edge belongs to the window...
        assert_eq!(snapshot.hit_test(Point::new(0, 0)).unwrap().identity().hwnd, 0xA);
        // ...right/bottom edge belongs to the neighbour.
        assert_eq!(snapshot.hit_test(Point::new(100, 50)).unwrap().identity().hwnd, 0xB);
        assert_eq!(snapshot.hit_test(Point::new(99, 50)).unwrap().identity().hwnd, 0xA);
        // Past the last window there is nothing.
        assert_eq!(snapshot.hit_test(Point::new(200, 50)), None);
    }

    #[test]
    fn nearest_target_prefers_the_window_under_the_cursor() {
        let snapshot = snapshot(&[
            (0xA, 0, rect(0, 0, 500, 500)),
            (0xB, 1, rect(600, 0, 700, 100)),
        ]);
        // Inside the big window: distance 0 beats the nearer-edge neighbour's 100.
        let target = snapshot.nearest_target(Point::new(500, 500), 24).unwrap();
        assert_eq!(target.identity().hwnd, 0xA);
        // 5 px to the right of the big window and far from everything else: still A.
        let target = snapshot.nearest_target(Point::new(504, 250), 24).unwrap();
        assert_eq!(target.identity().hwnd, 0xA);
    }

    #[test]
    fn nearest_target_respects_the_snap_radius() {
        let snapshot = snapshot(&[(0xA, 0, rect(0, 0, 100, 100))]);
        // Exactly on the radius: included (`<=`).
        assert!(snapshot.nearest_target(Point::new(124, 50), 24).is_some());
        // One pixel beyond: excluded.
        assert!(snapshot.nearest_target(Point::new(125, 50), 24).is_none());
        // Nothing nearby at all.
        assert!(snapshot.nearest_target(Point::new(2000, 2000), 24).is_none());
    }

    #[test]
    fn nearest_target_breaks_ties_by_z_order() {
        // Two windows equally far from the point (the point is in the gap between
        // them): the frontmost wins.
        let snapshot = snapshot(&[
            (0xA, 0, rect(200, 0, 300, 100)),
            (0xB, 3, rect(0, 0, 100, 100)),
        ]);
        // 50 px to the right of the first window's edge and 50 px to the left of the
        // second's: equal distance, so Z order decides (0xA, the frontmost).
        let target = snapshot.nearest_target(Point::new(150, 50), 60).unwrap();
        assert_eq!(target.identity().hwnd, 0xA);
    }

    #[test]
    fn a_point_in_two_overlapping_windows_belongs_to_the_frontmost() {
        let snapshot = snapshot(&[
            (0xA, 0, rect(0, 0, 200, 200)),
            (0xB, 1, rect(100, 100, 300, 300)),
        ]);
        // Both contain (150,150); distance 0 each, so Z order decides.
        assert_eq!(snapshot.nearest_target(Point::new(150, 150), 24).unwrap().identity().hwnd, 0xA);
        // Only the second contains (250,250).
        assert_eq!(snapshot.nearest_target(Point::new(250, 250), 24).unwrap().identity().hwnd, 0xB);
    }

    #[test]
    fn degenerate_rectangles_are_never_hit_or_snapped_to() {
        let snapshot = snapshot(&[
            (0xA, 0, rect(50, 50, 50, 90)),
            (0xB, 1, rect(200, 200, 150, 150)),
            (0xC, 2, rect(300, 300, 400, 400)),
        ]);
        // A zero-width window and an inverted window both occupy no pixels: a point in
        // their nominal span resolves to nothing at all.
        assert_eq!(snapshot.hit_test(Point::new(50, 60)), None);
        assert_eq!(snapshot.hit_test(Point::new(200, 200)), None);
        assert_eq!(snapshot.hit_test(Point::new(350, 350)).unwrap().identity().hwnd, 0xC);
        // And they never win a snap even though the point sits inside their nominal
        // bounds: an unusable rectangle is skipped before any distance is computed.
        assert_eq!(snapshot.nearest_target(Point::new(50, 60), 64), None);
        assert_eq!(
            snapshot.nearest_target(Point::new(295, 300), 8).unwrap().identity().hwnd,
            0xC
        );
    }

    #[test]
    fn a_released_snapshot_answers_nothing() {
        let mut snapshot = snapshot(&[(0xA, 0, rect(0, 0, 100, 100))]);
        assert!(snapshot.hit_test(Point::new(50, 50)).is_some());
        snapshot.release();
        assert_eq!(snapshot.hit_test(Point::new(50, 50)), None);
        assert_eq!(snapshot.nearest_target(Point::new(50, 50), 24), None);
    }

    #[test]
    fn candidates_from_another_generation_are_invisible() {
        let mut counter = EpochCounter::new();
        let old = counter.bump();
        let new = counter.bump();
        // A snapshot that accidentally contains a leftover candidate from the previous
        // generation must not answer with it.
        let mixed = WindowSnapshot::new(
            new,
            vec![
                candidate(0x01, 0, rect(0, 0, 100, 100), old),
                candidate(0x02, 1, rect(0, 0, 100, 100), new),
            ],
            MonitorCache::empty(),
        );
        assert_eq!(mixed.hit_test(Point::new(50, 50)).unwrap().identity().hwnd, 0x02);
        assert_eq!(mixed.nearest_target(Point::new(50, 50), 24).unwrap().identity().hwnd, 0x02);
    }

    #[test]
    fn bounds_changed_is_written_back_into_the_snapshot() {
        let mut counter = EpochCounter::new();
        let epoch = counter.bump();
        let mut snapshot = snapshot_in(epoch, &[(0xA, 0, rect(0, 0, 100, 100))]);
        let identity = snapshot.candidates()[0].identity;

        // The window moved: the new rectangle must be readable by the very next query.
        let moved = HoverValidity::BoundsChanged {
            epoch,
            identity,
            new_bounds: rect(500, 400, 600, 500),
        };
        assert!(snapshot.apply_candidate_update(&moved));
        assert_eq!(snapshot.hit_test(Point::new(550, 450)).unwrap().identity().hwnd, 0xA);
        assert_eq!(
            snapshot.hit_test(Point::new(50, 50)),
            None,
            "the old rectangle must stop answering"
        );
        assert_eq!(
            snapshot.nearest_target(Point::new(50, 50), 24),
            None,
            "nearest_target reads the same data as hit_test"
        );

        // Applying the same update again is a no-op, not a second mutation.
        assert!(!snapshot.apply_candidate_update(&moved));
        // Verdicts without geometry never mutate.
        assert!(!snapshot.apply_candidate_update(&HoverValidity::Valid));
        assert!(!snapshot.apply_candidate_update(&HoverValidity::Invalid));
    }

    #[test]
    fn stale_or_mismatched_updates_are_ignored() {
        let mut counter = EpochCounter::new();
        let epoch = counter.bump();
        let mut snapshot = snapshot_in(epoch, &[(0xA, 0, rect(0, 0, 100, 100))]);
        let identity = snapshot.candidates()[0].identity;

        // Wrong epoch.
        assert!(!snapshot.apply_candidate_update(&HoverValidity::BoundsChanged {
            epoch: epoch + 1,
            identity,
            new_bounds: rect(1, 1, 2, 2),
        }));
        // Correct epoch, recycled handle (different pid/class hash).
        assert!(!snapshot.apply_candidate_update(&HoverValidity::BoundsChanged {
            epoch,
            identity: WindowIdentity::new(identity.hwnd, identity.process_id + 1, 7),
            new_bounds: rect(1, 1, 2, 2),
        }));
        // Correct epoch, unknown window.
        assert!(!snapshot.apply_candidate_update(&HoverValidity::BoundsChanged {
            epoch,
            identity: WindowIdentity::new(0x999, 1, 2),
            new_bounds: rect(1, 1, 2, 2),
        }));
        // Nothing moved.
        assert_eq!(snapshot.candidates()[0].screen_bounds, rect(0, 0, 100, 100));
    }

    #[test]
    fn hit_test_and_nearest_target_stay_inside_the_latency_budget() {
        // Baseline evidence for the design's "no spatial index in v1" decision: a
        // linear scan stays far below the §10.2 budget (P95 < 0.1 ms) even at three
        // times the candidate count the design worries about.
        for count in [50usize, 100, 200] {
            let entries: Vec<(isize, u32, Rect)> = (0..count)
                .map(|index| {
                    let index = index as i32;
                    (
                        index as isize,
                        index as u32,
                        rect(index * 20, index * 10, index * 20 + 300, index * 10 + 200),
                    )
                })
                .collect();
            let snapshot = snapshot(&entries);

            let iterations = 2_000;
            let mut samples = Vec::with_capacity(iterations);
            for step in 0..iterations {
                let point = Point::new((step as i32 * 37) % 6000, (step as i32 * 53) % 4000);
                let started = std::time::Instant::now();
                let _ = snapshot.hit_test(point);
                samples.push(started.elapsed().as_nanos());
            }
            samples.sort_unstable();
            let p50 = samples[samples.len() / 2];
            let p95 = samples[samples.len() * 95 / 100];
            eprintln!("hit_test candidates={count} p50={p50}ns p95={p95}ns");
            assert!(
                p95 < 100_000,
                "hit_test p95 {p95}ns exceeds the 0.1 ms budget at {count} candidates"
            );

            let mut samples = Vec::with_capacity(iterations);
            for step in 0..iterations {
                let point = Point::new((step as i32 * 41) % 6000, (step as i32 * 59) % 4000);
                let started = std::time::Instant::now();
                let _ = snapshot.nearest_target(point, 24);
                samples.push(started.elapsed().as_nanos());
            }
            samples.sort_unstable();
            let p50 = samples[samples.len() / 2];
            let p95 = samples[samples.len() * 95 / 100];
            eprintln!("nearest_target candidates={count} p50={p50}ns p95={p95}ns");
            assert!(
                p95 < 100_000,
                "nearest_target p95 {p95}ns exceeds the 0.1 ms budget at {count} candidates"
            );
        }
    }

    #[test]
    fn point_distance_squared_matches_the_pythagorean_value() {
        assert_eq!(point_distance_squared(Point::new(0, 0), Point::new(3, 4)), 25);
        assert_eq!(point_distance_squared(Point::new(-1, -1), Point::new(-1, -1)), 0);
    }
}
