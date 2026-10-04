//! v2 sub-element deep selection: contracts and the pure scheduling state machine
//! (docs/18 §2, §3, §4).
//!
//! Nothing in this module touches COM, UIA or MSAA. The scheduler decides *when* a
//! refinement query is worth issuing and whether an arriving result may still be applied;
//! the platform refinement worker performs the query. Keeping the decision pure is what
//! lets every branch — 80 ms dwell, single-flight, latest-point coalescing, epoch
//! invalidation, cached-path reuse — be unit tested without an accessibility stack.

use super::model::{RequestGate, RequestId, SnapshotEpoch, WindowIdentity};
use crate::capture::geometry::{Point, Rect};

/// Cursor stillness before a refinement query is issued (docs/18 §2).
///
/// The reference selector uses the same 80 ms: the whole-window hover is available after
/// 120 ms of dwell, and the deeper query must not start until the user is clearly
/// holding still, or a passing cursor would hammer the accessibility providers.
pub const REFINEMENT_DWELL_MS: u32 = 80;

/// Total budget for one refinement query (docs/18 §3).
pub const REFINEMENT_BUDGET_MS: u32 = 1500;
/// Upper bound for a single provider call inside a refinement query.
pub const REFINEMENT_CALL_LIMIT_MS: u32 = 500;
/// How often a refinement query may publish an intermediate result.
pub const REFINEMENT_PUBLISH_INTERVAL_MS: u32 = 32;

/// Why a refinement query stopped (docs/18 §3).
///
/// A query that stops for any reason other than [`Self::Complete`] still publishes the
/// part of the path it verified, and the overlay falls back to the v1 whole-window frame
/// when nothing usable came back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StopReason {
    /// The path was resolved down to the deepest interactive element.
    Complete,
    /// The query ran out of its total budget.
    BudgetExhausted,
    /// A depth/node/rectangle limit was hit.
    TraversalLimit,
    /// One provider call exceeded its own limit.
    ProviderTimeout,
    /// The provider failed (crashed, refused, returned an error).
    ProviderFailure,
    /// The user moved, pressed, or the session ended before the query finished.
    Cancelled,
    /// No accessibility provider can serve this window at all.
    Unsupported,
}

impl StopReason {
    /// Whether the resolved path may be reused without asking the worker again.
    ///
    /// Only a complete path can answer "what is under this point" for every point inside
    /// it. A partial path must not suppress the next query, or a budget-exhausted result
    /// would freeze the selection at whatever depth it happened to reach.
    pub fn is_complete(self) -> bool {
        self == Self::Complete
    }
}

/// A refined target: the v1 window identity plus the path that was verified inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepTarget {
    /// The window this path belongs to; always a window from the v1 snapshot.
    pub window: WindowIdentity,
    pub kind: super::model::TargetKind,
    /// Virtual-desktop physical pixels — the same space as [`super::model::WindowTarget`].
    pub screen_bounds: Rect,
    /// Top-down path; `path[0]` is the window frame and the last entry equals
    /// `screen_bounds`. Empty only when nothing was resolved.
    pub path: Vec<Rect>,
    pub stop_reason: StopReason,
}

impl DeepTarget {
    /// Whether `point` is answered by this path.
    pub fn covers(&self, point: Point) -> bool {
        !self.screen_bounds.is_empty() && self.screen_bounds.contains(point)
    }
}

/// What the overlay must do after feeding the scheduler a cursor position.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SchedulerActions {
    /// Drop any in-flight refinement: its answer describes a position the cursor left.
    pub invalidate_in_flight: bool,
    /// (Re)arm the one-shot [`REFINEMENT_DWELL_MS`] timer for the current target.
    pub arm_dwell: bool,
}

/// A refinement query to hand to the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefinementJob {
    pub request: RequestId,
    pub window: WindowIdentity,
    pub epoch: SnapshotEpoch,
    pub point: Point,
}

/// The pending dwell target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingTarget {
    window: WindowIdentity,
    epoch: SnapshotEpoch,
    point: Point,
}

/// Pure scheduler for v2 refinement queries (docs/18 §2).
///
/// The overlay owns the timer; this type owns the *rules*:
///
/// * a query is only issued after the cursor has been still for [`REFINEMENT_DWELL_MS`];
/// * at most one query is in flight (single-flight);
/// * a new target invalidates the in-flight query instead of queueing it (latest point);
/// * a complete cached path answers further cursor movement without touching the worker;
/// * a snapshot epoch change invalidates both the query and the cache.
#[derive(Debug, Default)]
pub struct RefinementScheduler {
    pending: Option<PendingTarget>,
    in_flight: Option<RequestId>,
    requests: RequestGate,
    cached: Option<(SnapshotEpoch, DeepTarget)>,
}

impl RefinementScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a cursor position that resolved to `window` in snapshot `epoch`.
    ///
    /// `window` is `None` when the point is not over any window: there is nothing to
    /// refine, so the cache and any query are dropped.
    pub fn on_cursor_moved(
        &mut self,
        epoch: SnapshotEpoch,
        window: Option<WindowIdentity>,
        point: Point,
    ) -> SchedulerActions {
        let mut actions = SchedulerActions::default();
        if self.in_flight.is_some() {
            // Whatever the worker is computing describes where the cursor *was*.
            self.in_flight = None;
            actions.invalidate_in_flight = true;
        }

        let Some(window) = window else {
            self.pending = None;
            self.cached = None;
            return actions;
        };

        // The cached path belongs to a window *and* a snapshot generation; either moving
        // on invalidates it.
        if self
            .cached
            .as_ref()
            .is_some_and(|(cached_epoch, target)| *cached_epoch != epoch || target.window != window)
        {
            self.cached = None;
        }
        if let Some((_, cached)) = self.cached.as_ref()
            && cached.stop_reason.is_complete()
            && cached.covers(point)
        {
            // Moving inside a fully resolved path must not touch the worker.
            self.pending = None;
            return actions;
        }

        self.pending = Some(PendingTarget {
            window,
            epoch,
            point,
        });
        actions.arm_dwell = true;
        actions
    }

    /// The dwell timer expired: issue a query unless one is already running.
    pub fn on_dwell_due(&mut self) -> Option<RefinementJob> {
        if self.in_flight.is_some() {
            // Single-flight: the running query gets to finish; a newer point re-armed the
            // timer and will be picked up when its own dwell expires.
            return None;
        }
        let pending = self.pending?;
        let request = self.requests.issue();
        self.in_flight = Some(request);
        Some(RefinementJob {
            request,
            window: pending.window,
            epoch: pending.epoch,
            point: pending.point,
        })
    }

    /// Apply a worker result. Returns whether it was still current.
    ///
    /// A result is dropped when its request was superseded (a newer point arrived) or when
    /// the snapshot generation has moved on — the same `epoch + request id` rule the v1
    /// detection worker uses.
    pub fn on_result(&mut self, request: RequestId, epoch: SnapshotEpoch, target: DeepTarget) -> bool {
        if self.in_flight != Some(request) || self.requests.latest() != Some(request) {
            return false;
        }
        self.in_flight = None;
        if self.pending.is_some_and(|pending| pending.epoch != epoch) {
            // The snapshot was rebuilt while the query ran; the path may describe stale
            // geometry, so it is not published.
            return false;
        }
        self.cached = Some((epoch, target));
        true
    }

    /// The snapshot generation changed: drop the cache and invalidate any query.
    pub fn on_snapshot_changed(&mut self) -> SchedulerActions {
        self.pending = None;
        self.cached = None;
        let invalidate = self.in_flight.take().is_some();
        if invalidate {
            self.requests.retire();
        }
        SchedulerActions {
            invalidate_in_flight: invalidate,
            arm_dwell: false,
        }
    }

    /// The currently published deep target, if any.
    pub fn cached(&self) -> Option<&DeepTarget> {
        self.cached.as_ref().map(|(_, target)| target)
    }

    /// Whether a query is in flight.
    pub fn is_in_flight(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Forget everything. Used on session teardown so nothing leaks into the next F5.
    pub fn reset(&mut self) {
        self.pending = None;
        self.in_flight = None;
        self.cached = None;
        self.requests = RequestGate::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::model::TargetKind;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect::new(left, top, right, bottom)
    }

    fn window(hwnd: isize) -> WindowIdentity {
        WindowIdentity::new(hwnd, 4242, 0xC0FFEE)
    }

    fn deep_target(kind: TargetKind, bounds: Rect, stop_reason: StopReason) -> DeepTarget {
        DeepTarget {
            window: window(0x100),
            kind,
            screen_bounds: bounds,
            path: vec![rect(0, 0, 1000, 800), bounds],
            stop_reason,
        }
    }

    /// Move the cursor and immediately expire the dwell timer, returning the job.
    fn submit(scheduler: &mut RefinementScheduler, epoch: SnapshotEpoch, point: Point) -> RefinementJob {
        let actions = scheduler.on_cursor_moved(epoch, Some(window(0x100)), point);
        assert!(actions.arm_dwell, "a new target must arm the dwell timer");
        scheduler
            .on_dwell_due()
            .expect("dwell expiry issues a query")
    }

    #[test]
    fn a_query_is_only_issued_after_the_dwell_expires() {
        let mut scheduler = RefinementScheduler::new();
        let actions = scheduler.on_cursor_moved(1, Some(window(0x100)), Point::new(10, 10));
        assert_eq!(
            actions,
            SchedulerActions {
                invalidate_in_flight: false,
                arm_dwell: true
            }
        );
        // Nothing happens until the overlay's timer fires.
        assert!(!scheduler.is_in_flight());
        let job = scheduler.on_dwell_due().expect("a job");
        assert_eq!(job.window, window(0x100));
        assert_eq!(job.epoch, 1);
        assert_eq!(job.point, Point::new(10, 10));
        assert!(scheduler.is_in_flight());
        // A second expiry while the first is running must not issue another query.
        assert!(scheduler.on_dwell_due().is_none());
    }

    #[test]
    fn a_cursor_move_without_a_window_clears_everything() {
        let mut scheduler = RefinementScheduler::new();
        let job = submit(&mut scheduler, 1, Point::new(10, 10));
        let actions = scheduler.on_cursor_moved(1, None, Point::new(900, 900));
        assert!(actions.invalidate_in_flight, "the in-flight query is stale");
        assert!(!actions.arm_dwell, "there is nothing to refine");
        // Its late result must not resurrect a path for a point we left.
        assert!(!scheduler.on_result(
            job.request,
            1,
            deep_target(TargetKind::UiElement, rect(0, 0, 100, 100), StopReason::Complete)
        ));
        assert!(scheduler.cached().is_none());
    }

    #[test]
    fn only_the_newest_cursor_position_is_queried() {
        let mut scheduler = RefinementScheduler::new();
        let first = submit(&mut scheduler, 1, Point::new(10, 10));
        // The cursor moved before the worker answered.
        let actions = scheduler.on_cursor_moved(1, Some(window(0x100)), Point::new(50, 50));
        assert!(actions.invalidate_in_flight);
        assert!(actions.arm_dwell);

        // The stale result is rejected...
        assert!(!scheduler.on_result(
            first.request,
            1,
            deep_target(TargetKind::UiElement, rect(0, 0, 20, 20), StopReason::Complete)
        ));
        assert!(scheduler.cached().is_none());

        // ...and the new dwell produces a fresh query for the new point.
        let second = scheduler.on_dwell_due().expect("a job for the new point");
        assert_eq!(second.point, Point::new(50, 50));
        assert_ne!(second.request, first.request);
        assert!(scheduler.on_result(
            second.request,
            1,
            deep_target(TargetKind::UiElement, rect(40, 40, 200, 200), StopReason::Complete)
        ));
    }

    #[test]
    fn a_complete_cached_path_answers_movement_without_the_worker() {
        let mut scheduler = RefinementScheduler::new();
        let job = submit(&mut scheduler, 1, Point::new(150, 150));
        assert!(scheduler.on_result(
            job.request,
            1,
            deep_target(TargetKind::UiElement, rect(100, 100, 300, 300), StopReason::Complete)
        ));

        // Moving inside the resolved element: no dwell, no worker, the path still answers.
        let actions = scheduler.on_cursor_moved(1, Some(window(0x100)), Point::new(200, 200));
        assert_eq!(actions, SchedulerActions::default());
        assert!(scheduler.on_dwell_due().is_none());
        assert!(scheduler.cached().is_some());
        assert!(scheduler.cached().unwrap().covers(Point::new(200, 200)));

        // Leaving the element (but staying in the window) needs a new query.
        let actions = scheduler.on_cursor_moved(1, Some(window(0x100)), Point::new(700, 700));
        assert!(actions.arm_dwell);
    }

    #[test]
    fn a_partial_path_never_suppresses_the_next_query() {
        let mut scheduler = RefinementScheduler::new();
        let job = submit(&mut scheduler, 1, Point::new(150, 150));
        assert!(scheduler.on_result(
            job.request,
            1,
            deep_target(
                TargetKind::UiElement,
                rect(100, 100, 300, 300),
                StopReason::BudgetExhausted
            )
        ));
        assert!(scheduler.cached().is_some(), "the partial path is still published");
        // The same point must be queried again: the budget may allow a deeper answer now.
        let actions = scheduler.on_cursor_moved(1, Some(window(0x100)), Point::new(150, 150));
        assert!(actions.arm_dwell, "a partial path must not freeze the selection");
    }

    #[test]
    fn an_epoch_change_drops_the_cache_and_the_query() {
        let mut scheduler = RefinementScheduler::new();
        let job = submit(&mut scheduler, 1, Point::new(150, 150));
        assert!(scheduler.on_result(
            job.request,
            1,
            deep_target(TargetKind::UiElement, rect(100, 100, 300, 300), StopReason::Complete)
        ));
        assert!(scheduler.cached().is_some());

        let in_flight_job = submit(&mut scheduler, 2, Point::new(160, 160));
        let actions = scheduler.on_snapshot_changed();
        assert!(actions.invalidate_in_flight);
        assert!(scheduler.cached().is_none(), "a rebuilt snapshot invalidates the path");
        assert!(!scheduler.on_result(
            in_flight_job.request,
            2,
            deep_target(TargetKind::UiElement, rect(0, 0, 10, 10), StopReason::Complete)
        ));
    }

    #[test]
    fn a_result_from_another_epoch_is_not_published() {
        let mut scheduler = RefinementScheduler::new();
        let job = submit(&mut scheduler, 1, Point::new(150, 150));
        // The worker answers after the snapshot was rebuilt (same request id).
        assert!(!scheduler.on_result(
            job.request,
            2,
            deep_target(TargetKind::UiElement, rect(0, 0, 10, 10), StopReason::Complete)
        ));
        assert!(scheduler.cached().is_none());
    }

    #[test]
    fn reset_clears_the_pending_target_the_query_and_the_cache() {
        let mut scheduler = RefinementScheduler::new();
        let job = submit(&mut scheduler, 1, Point::new(150, 150));
        assert!(scheduler.on_result(
            job.request,
            1,
            deep_target(TargetKind::UiElement, rect(0, 0, 10, 10), StopReason::Complete)
        ));
        scheduler.reset();
        assert!(scheduler.cached().is_none());
        assert!(!scheduler.is_in_flight());
        assert!(scheduler.on_dwell_due().is_none(), "no pending target survives a reset");
    }

    #[test]
    fn the_published_path_starts_at_the_window_frame_and_ends_at_the_target() {
        let target = deep_target(
            TargetKind::UiElement,
            rect(100, 100, 300, 300),
            StopReason::Complete,
        );
        // Contract the overlay relies on when it draws the path outline.
        assert_eq!(target.path.first(), Some(&rect(0, 0, 1000, 800)));
        assert_eq!(target.path.last(), Some(&target.screen_bounds));
        assert!(target.kind.is_refined());
        assert!(!TargetKind::TopLevelWindowFrame.is_refined());
        assert!(StopReason::Complete.is_complete());
        for reason in [
            StopReason::BudgetExhausted,
            StopReason::TraversalLimit,
            StopReason::ProviderTimeout,
            StopReason::ProviderFailure,
            StopReason::Cancelled,
            StopReason::Unsupported,
        ] {
            assert!(!reason.is_complete(), "{reason:?}");
        }
    }
}
