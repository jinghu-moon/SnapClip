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
use std::time::Duration;

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

/// Longest a single refinement query may hold the single-flight slot (docs/18 §3).
///
/// The provider enforces [`REFINEMENT_BUDGET_MS`] itself between nodes, which covers every
/// traversal that keeps making progress. This is the outer guarantee for the one case it cannot
/// cover: a COM call that never returns. Without it a wedged provider keeps the slot for the rest
/// of the session and deep selection stops silently — the same user-visible failure as a lost
/// result, which is why the slot is released by rule rather than by hope.
pub const REFINEMENT_INFLIGHT_TIMEOUT_MS: u32 = REFINEMENT_BUDGET_MS + 500;

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

/// How a newly resolved target relates to the one currently displayed (docs/18 §13.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replacement {
    /// Replace the displayed target now.
    Immediate,
    /// The target got **shallower while still containing** the displayed one: this is what a
    /// cursor merely passing through a parent container looks like. Wait for the next dwell to
    /// reproduce it before displaying anything, so a transit never expands the frame.
    NeedsConfirmation,
}

/// Decide whether a result may replace the displayed target.
///
/// The reference selector sidesteps this entirely: its base path is computed **per point** and
/// its refinement may only ever *deepen* it, so a shallower answer can never be displayed. In
/// SnapClip the whole answer comes from the deep query, so the equivalent rule is expressed as
/// a one-step confirmation for downgrades only:
///
/// * a **deeper** target (parent → child) applies immediately;
/// * a **lateral** target (A → B, neither containing the other) applies immediately;
/// * a **shallower target that contains the current one** waits for a second dwell.
pub fn classify_replacement(current: Option<&DeepTarget>, next: &DeepTarget) -> Replacement {
    let Some(current) = current else {
        return Replacement::Immediate;
    };
    if current.window != next.window {
        // A different window is a different question; never hold it back.
        return Replacement::Immediate;
    }
    if current.screen_bounds == next.screen_bounds {
        // Same answer: nothing to confirm, nothing to display.
        return Replacement::Immediate;
    }
    let shallower = next.path.len() < current.path.len();
    // The downgrade case: the new (shallower) target *covers* the one on screen, i.e. the
    // cursor is inside a parent container of what it was pointing at.
    let contains_current = contains(next.screen_bounds, current.screen_bounds);
    if shallower && contains_current {
        Replacement::NeedsConfirmation
    } else {
        Replacement::Immediate
    }
}

/// Half-open-free containment used only for this decision: the container must cover the whole
/// of the inner rectangle. Both are screen rectangles of real UI, so inclusive bounds are the
/// intended semantics here.
fn contains(outer: Rect, inner: Rect) -> bool {
    !outer.is_empty()
        && !inner.is_empty()
        && inner.left >= outer.left
        && inner.top >= outer.top
        && inner.right <= outer.right
        && inner.bottom <= outer.bottom
}

/// What the overlay must do after feeding the scheduler a cursor position.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SchedulerActions {
    /// Drop any in-flight refinement: its answer describes a position the cursor left.
    pub invalidate_in_flight: bool,
    /// (Re)arm the one-shot [`REFINEMENT_DWELL_MS`] timer for the current target.
    pub arm_dwell: bool,
}

/// Which rectangle the auto-snap preview may show for a cursor at `point` inside `window`
/// (docs/18 §13.5).
///
/// `deep` is the last verified deep target and `waiting` says whether an answer for the
/// **current** position is still expected. The whole-window frame is *not* a neutral filler:
/// showing it while a deeper answer is on its way is exactly the "expand to the window, then
/// shrink to the box" the product rejected, and it is what made an ordinary move from a control
/// into its parent container look like a wrong intermediate target.
///
/// * an answer that still covers the point is the answer → show it;
/// * a verified rectangle from this window is kept while a better one is coming, so moving
///   between controls of one window never falls back to the frame;
/// * with nothing verified for this window yet, the preview is **withheld** rather than filled
///   with the frame, so the first thing seen is the control under the cursor;
/// * once the wait is over — nothing is pending any more — the v1 whole-window frame is the
///   honest floor and is shown.
///
/// `None` means "paint no preview at all".
pub fn preview_bounds(
    deep: Option<&DeepTarget>,
    window: WindowIdentity,
    point: Point,
    window_bounds: Rect,
    waiting: bool,
) -> Option<Rect> {
    match deep.filter(|deep| deep.window == window) {
        Some(deep) if deep.covers(point) => Some(deep.screen_bounds),
        Some(deep) if waiting => Some(deep.screen_bounds),
        _ if waiting => None,
        _ => Some(window_bounds),
    }
}

/// A refinement query to hand to the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefinementJob {
    pub request: RequestId,
    pub window: WindowIdentity,
    pub epoch: SnapshotEpoch,
    pub point: Point,
}

/// Cooperative control handed to one provider call (docs/18 §3).
///
/// The worker cannot interrupt a COM call, so the provider polls this instead: the
/// reference selector passes the same shape (`budget`, per-call limit, `cancelled`
/// closure) into its UIA/MSAA queries, and the budget is enforced by the traversal
/// checking it between nodes rather than by killing the call.
pub struct QueryControl<'a> {
    /// Total budget for the whole query.
    pub budget: Duration,
    /// Upper bound for a single provider call inside the query.
    pub call_limit: Duration,
    /// Whether the caller has already abandoned this query.
    pub cancelled: &'a dyn Fn() -> bool,
    /// When the query was handed to the provider, for [`Self::budget_exhausted`].
    started: std::time::Instant,
}

impl<'a> QueryControl<'a> {
    /// The documented refinement budgets (docs/18 §3).
    pub fn refinement(cancelled: &'a dyn Fn() -> bool) -> Self {
        Self {
            budget: Duration::from_millis(u64::from(REFINEMENT_BUDGET_MS)),
            call_limit: Duration::from_millis(u64::from(REFINEMENT_CALL_LIMIT_MS)),
            cancelled,
            started: std::time::Instant::now(),
        }
    }

    /// Whether the provider should stop and publish what it has verified so far.
    pub fn is_cancelled(&self) -> bool {
        (self.cancelled)()
    }

    /// Whether the query has spent its total budget (docs/18 §3).
    ///
    /// A COM call cannot be interrupted, so the traversal checks this between nodes — the
    /// cooperative half of the budget. Together with [`RefinementScheduler::on_in_flight_timeout`]
    /// there is no path that lets one query hold the worker forever.
    pub fn budget_exhausted(&self) -> bool {
        self.started.elapsed() >= self.budget
    }
}

/// Result of one refinement query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefinementOutcome {
    /// A verified path, possibly partial (`stop_reason` says how far it got).
    Target(Box<DeepTarget>),
    /// Nothing usable: the overlay keeps the v1 whole-window frame.
    Empty(StopReason),
}

/// Resolves the deepest element under a point inside one window (docs/18 §4).
///
/// Implementations are **constructed on the refinement thread** (the worker takes a factory,
/// not a value) and run only there, so every COM/UIA/MSAA object they create is created and
/// dropped inside `resolve`. Nothing they produce may carry a native handle across the thread
/// boundary — [`DeepTarget`] is plain geometry plus a stop reason for exactly that reason.
pub trait DeepSelectionProvider {
    /// Resolve `job.point` inside `window_bounds`.
    ///
    /// Must publish a partial path when the budget is exhausted or the query is cancelled,
    /// and must return within the control's budget as far as the platform allows.
    fn resolve(
        &mut self,
        job: &RefinementJob,
        window_bounds: Rect,
        control: &QueryControl<'_>,
    ) -> RefinementOutcome;

    /// Drop cached batches. Called when the snapshot generation changes or the session
    /// ends, so no stale tree survives into the next query.
    fn release(&mut self) {}
}

/// The provider used until the UIA provider lands (v2-P2).
///
/// It reports [`StopReason::Unsupported`] so the overlay keeps publishing the v1
/// whole-window frame: the refinement pipeline is live and measurable, and enabling deep
/// selection is exactly "swap this provider", not "add a branch somewhere".
#[derive(Debug, Default)]
pub struct UnsupportedDeepSelection;

impl DeepSelectionProvider for UnsupportedDeepSelection {
    fn resolve(
        &mut self,
        _job: &RefinementJob,
        _window_bounds: Rect,
        _control: &QueryControl<'_>,
    ) -> RefinementOutcome {
        RefinementOutcome::Empty(StopReason::Unsupported)
    }
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
/// * **every** cursor position is re-queried on its own dwell: the point decides the target,
///   never the previous result (docs/18 §13). The provider's expanded-level cache is what makes
///   those repeats cheap (measured 2.3–6.7 ms), and it is the only way a point that moves from a
///   container into a child element can ever resolve to the child;
/// * a snapshot epoch change invalidates both the query and the cache.
#[derive(Debug, Default)]
pub struct RefinementScheduler {
    pending: Option<PendingTarget>,
    in_flight: Option<RequestId>,
    /// The point the in-flight request was issued for. A hand jitters constantly, so a query is
    /// never cancelled just because the cursor moved a little; the *result* is judged against
    /// this point when it arrives (docs/18 §13.4).
    in_flight_point: Option<Point>,
    /// When the in-flight query was issued, for the abandonment rule below.
    in_flight_since: Option<std::time::Instant>,
    /// Most recent cursor point, used for that judgement.
    last_point: Option<Point>,
    /// Window the last cursor position resolved to, so a *window* change is what cancels.
    last_window: Option<WindowIdentity>,
    requests: RequestGate,
    cached: Option<(SnapshotEpoch, DeepTarget)>,
    /// A dwell expired for a new position while a query was in flight; see
    /// [`Self::take_follow_up`].
    deferred: bool,
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
        self.last_point = Some(point);
        let window_changed = self.last_window != window;
        self.last_window = window;
        // Only a *different question* cancels a running query: a hand that jitters one pixel
        // must not keep killing work that is almost done, or precise targets never resolve.
        if window_changed && self.in_flight.is_some() {
            self.in_flight = None;
            self.in_flight_point = None;
            self.in_flight_since = None;
            actions.invalidate_in_flight = true;
        }

        let Some(window) = window else {
            self.pending = None;
            self.deferred = false;
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
        // **Every** cursor position that has a window is a new question.
        //
        // An earlier revision skipped the query while the point stayed inside the published
        // path, on the reference selector's "moving through complete cached paths must not
        // touch the worker" rule. That rule is only sound when the *whole tree* under the point
        // is cached: the published path is just the resolved chain, so once the target was a
        // container every point inside it was "covered" and the deeper element under the
        // cursor could never be found — the reported "parent → child never switches" defect.
        // Re-querying is cheap because the provider keeps its expanded levels.
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
            // Single-flight: the running query gets to finish. A *different* position whose
            // dwell expired meanwhile is remembered and issued when the slot frees.
            if let (Some(pending), Some(issued)) = (self.pending, self.in_flight_point)
                && !points_close(pending.point, issued)
            {
                self.deferred = true;
            }
            return None;
        }
        let pending = self.pending?;
        let request = self.requests.issue();
        self.in_flight = Some(request);
        self.in_flight_point = Some(pending.point);
        self.in_flight_since = Some(std::time::Instant::now());
        Some(RefinementJob {
            request,
            window: pending.window,
            epoch: pending.epoch,
            point: pending.point,
        })
    }

    /// Give up on a query that has outlived [`REFINEMENT_INFLIGHT_TIMEOUT_MS`] (docs/18 §3).
    ///
    /// Returns the abandoned request id, or `None` while the query is still within its budget.
    /// The caller retires the worker's gate: a COM call cannot be interrupted, so the thread stays
    /// wedged until it returns on its own, and the overlay carries on degrading to the v1 frame.
    /// `now` comes from the caller so the rule is testable without sleeping.
    pub fn on_in_flight_timeout(&mut self, now: std::time::Instant) -> Option<RequestId> {
        let started = self.in_flight_since?;
        if now.saturating_duration_since(started)
            < Duration::from_millis(u64::from(REFINEMENT_INFLIGHT_TIMEOUT_MS))
        {
            return None;
        }
        self.in_flight_since = None;
        self.in_flight_point = None;
        self.requests.retire();
        self.in_flight.take()
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
        let issued_point = self.in_flight_point.take();
        self.in_flight = None;
        self.in_flight_since = None;
        if self.pending.is_some_and(|pending| pending.epoch != epoch) {
            // The snapshot was rebuilt while the query ran; the path may describe stale
            // geometry, so it is not published.
            return false;
        }
        // The answer must still describe where the cursor *is*. Comparing cursor pixels is the
        // wrong test: a hand jitters continuously, so a 50 ms query would look "stale" almost
        // every time and refinement would never publish (that was the "second F5 only snaps the
        // window" defect). What matters is whether the resolved rectangle still covers the
        // cursor — if it does, the answer is still the answer.
        if let Some(current) = self.last_point {
            let still_describes_cursor = target.covers(current)
                || issued_point.is_some_and(|issued| points_close(issued, current));
            if !still_describes_cursor {
                return false;
            }
        }
        self.cached = Some((epoch, target));
        true
    }

    /// The worker produced nothing usable for `request`.
    ///
    /// Clears the single-flight slot so the next dwell can issue a fresh query; without
    /// this a single `Unsupported` answer would wedge refinement for the rest of the
    /// session. Returns whether the request was still current.
    pub fn on_failure(&mut self, request: RequestId) -> bool {
        if self.in_flight != Some(request) {
            return false;
        }
        self.in_flight = None;
        self.in_flight_point = None;
        self.in_flight_since = None;
        true
    }

    /// The snapshot generation changed: drop the cache and invalidate any query.
    pub fn on_snapshot_changed(&mut self) -> SchedulerActions {
        self.pending = None;
        self.deferred = false;
        self.cached = None;
        self.in_flight_point = None;
        self.in_flight_since = None;
        let invalidate = self.in_flight.take().is_some();
        if invalidate {
            self.requests.retire();
        }
        SchedulerActions {
            invalidate_in_flight: invalidate,
            arm_dwell: false,
        }
    }

    /// The query that a finished, failed or abandoned one was holding back, if any.
    ///
    /// A dwell that expires while a query is in flight is deferred by single-flight, and the
    /// one-shot timer that carried it is gone. Without this, a cursor that came to rest on a
    /// child while its parent's query was still running was never asked about again: the
    /// parent's answer covers the child's point, so it was published and stayed — the
    /// "stuck on the parent box" defect. Call this whenever the slot frees.
    pub fn take_follow_up(&mut self) -> Option<RefinementJob> {
        if !self.deferred || self.in_flight.is_some() {
            return None;
        }
        self.deferred = false;
        self.on_dwell_due()
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
        self.deferred = false;
        self.in_flight = None;
        self.in_flight_point = None;
        self.in_flight_since = None;
        self.last_point = None;
        self.last_window = None;
        self.cached = None;
        self.requests = RequestGate::new();
    }
}

/// Points within this many physical pixels count as "the cursor has not moved", for judging a
/// finished query. Wide enough to absorb hand jitter, far below the 24 px snap radius.
pub const POINT_MATCH_TOLERANCE_PX: i32 = 4;

fn points_close(left: Point, right: Point) -> bool {
    (left.x - right.x).abs() <= POINT_MATCH_TOLERANCE_PX
        && (left.y - right.y).abs() <= POINT_MATCH_TOLERANCE_PX
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
        // The cursor moved (inside the same window) before the worker answered. A hand jitters
        // constantly, so the query is *not* cancelled — it is allowed to finish, and the result
        // is judged against where the cursor is by then.
        let actions = scheduler.on_cursor_moved(1, Some(window(0x100)), Point::new(50, 50));
        assert!(
            !actions.invalidate_in_flight,
            "a same-window jitter must not cancel work that is almost done"
        );
        assert!(actions.arm_dwell);

        // The stale result is rejected because the cursor is no longer at the point it was
        // issued for...
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
    fn a_position_whose_dwell_expired_during_a_query_is_queried_afterwards() {
        // The cursor rests on a container (query 1 runs), then moves into a child and rests
        // there. The child's dwell expires while query 1 is still in flight, so single-flight
        // must defer it — and the deferral must not lose the question: once query 1 lands, the
        // child's position is queried. Losing it published the container's answer for the
        // child's point (it covers it) and nothing ever asked again: "stuck on the parent".
        let mut scheduler = RefinementScheduler::new();
        let parent = submit(&mut scheduler, 1, Point::new(150, 150));
        let actions = scheduler.on_cursor_moved(1, Some(window(0x100)), Point::new(320, 320));
        assert!(actions.arm_dwell);
        assert!(scheduler.on_dwell_due().is_none(), "single-flight defers the child's query");
        assert!(scheduler.take_follow_up().is_none(), "nothing is issued while query 1 runs");

        assert!(scheduler.on_result(
            parent.request,
            1,
            deep_target(TargetKind::UiElement, rect(100, 100, 900, 700), StopReason::Complete)
        ));
        let child = scheduler
            .take_follow_up()
            .expect("the deferred position is queried once the slot is free");
        assert_eq!(child.point, Point::new(320, 320));
        assert!(scheduler.take_follow_up().is_none(), "the follow-up is issued once");
    }

    #[test]
    fn a_deferred_position_survives_a_failed_or_abandoned_query() {
        let mut scheduler = RefinementScheduler::new();
        let first = submit(&mut scheduler, 1, Point::new(150, 150));
        scheduler.on_cursor_moved(1, Some(window(0x100)), Point::new(320, 320));
        assert!(scheduler.on_dwell_due().is_none());
        assert!(scheduler.on_failure(first.request));
        assert_eq!(
            scheduler.take_follow_up().map(|job| job.point),
            Some(Point::new(320, 320))
        );

        let mut scheduler = RefinementScheduler::new();
        let first = submit(&mut scheduler, 1, Point::new(150, 150));
        scheduler.on_cursor_moved(1, Some(window(0x100)), Point::new(320, 320));
        assert!(scheduler.on_dwell_due().is_none());
        let expired = std::time::Instant::now()
            + Duration::from_millis(u64::from(REFINEMENT_INFLIGHT_TIMEOUT_MS));
        assert_eq!(scheduler.on_in_flight_timeout(expired), Some(first.request));
        assert_eq!(
            scheduler.take_follow_up().map(|job| job.point),
            Some(Point::new(320, 320))
        );
    }

    #[test]
    fn no_follow_up_without_a_deferred_dwell() {
        // A query whose position did not change needs no repeat, and leaving the window drops
        // the deferred question with everything else.
        let mut scheduler = RefinementScheduler::new();
        let job = submit(&mut scheduler, 1, Point::new(150, 150));
        assert!(scheduler.on_dwell_due().is_none());
        assert!(scheduler.on_result(
            job.request,
            1,
            deep_target(TargetKind::UiElement, rect(100, 100, 300, 300), StopReason::Complete)
        ));
        assert!(scheduler.take_follow_up().is_none(), "the same position was just answered");

        let _job = submit(&mut scheduler, 1, Point::new(150, 150));
        scheduler.on_cursor_moved(1, Some(window(0x100)), Point::new(320, 320));
        assert!(scheduler.on_dwell_due().is_none());
        scheduler.on_cursor_moved(1, None, Point::new(5000, 5000));
        assert!(scheduler.take_follow_up().is_none(), "no window, no question");
    }

    #[test]
    fn moving_inside_the_published_path_still_re_queries() {
        // The published path is only the *resolved chain*, not the whole tree under the point:
        // a container that covers the cursor also covers every child inside it, so reusing it
        // for a new point is exactly how "parent → child never switches" was produced
        // (docs/18 §13). Every position gets its own dwell; the provider's expanded-level cache
        // is what keeps the repeats cheap.
        let mut scheduler = RefinementScheduler::new();
        let job = submit(&mut scheduler, 1, Point::new(150, 150));
        assert!(scheduler.on_result(
            job.request,
            1,
            deep_target(TargetKind::UiElement, rect(100, 100, 300, 300), StopReason::Complete)
        ));

        // Moving *inside the resolved element* arms a fresh dwell: the deeper element under the
        // new point may still be a child of it.
        let actions = scheduler.on_cursor_moved(1, Some(window(0x100)), Point::new(200, 200));
        assert!(
            actions.arm_dwell,
            "a point inside the published path is a new question, not a cache hit"
        );
        assert!(!actions.invalidate_in_flight);
        let job = scheduler
            .on_dwell_due()
            .expect("the dwell expiry issues a query for the new point");
        assert_eq!(job.point, Point::new(200, 200));
        assert_eq!(job.window, window(0x100));

        // A point inside a *different* part of the window behaves the same way.
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
    fn a_wedged_query_is_abandoned_once_its_budget_is_spent() {
        // A provider stuck inside a COM call never publishes. Without this rule the single-flight
        // slot would stay held for the rest of the session (docs/18 §3).
        let mut scheduler = RefinementScheduler::new();
        let job = submit(&mut scheduler, 1, Point::new(150, 150));
        let now = std::time::Instant::now();
        assert_eq!(
            scheduler.on_in_flight_timeout(now),
            None,
            "a query inside its budget is left alone"
        );
        assert!(scheduler.is_in_flight());

        let expired = now + Duration::from_millis(u64::from(REFINEMENT_INFLIGHT_TIMEOUT_MS));
        assert_eq!(scheduler.on_in_flight_timeout(expired), Some(job.request));
        assert!(!scheduler.is_in_flight(), "the slot is released");
        assert_eq!(
            scheduler.on_in_flight_timeout(expired),
            None,
            "the rule fires once per query"
        );

        // The slot is genuinely reusable: the next dwell issues a fresh query.
        let next = submit(&mut scheduler, 1, Point::new(160, 160));
        assert_ne!(next.request, job.request);
    }

    #[test]
    fn an_abandoned_query_cannot_publish_its_answer_afterwards() {
        let mut scheduler = RefinementScheduler::new();
        let job = submit(&mut scheduler, 1, Point::new(150, 150));
        let expired =
            std::time::Instant::now() + Duration::from_millis(u64::from(REFINEMENT_INFLIGHT_TIMEOUT_MS));
        assert!(scheduler.on_in_flight_timeout(expired).is_some());
        // The wedged call may still return long after the overlay gave up on it.
        assert!(!scheduler.on_result(
            job.request,
            1,
            deep_target(TargetKind::UiElement, rect(100, 100, 200, 200), StopReason::Complete)
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
    fn an_empty_result_frees_the_single_flight_slot() {
        let mut scheduler = RefinementScheduler::new();
        let job = submit(&mut scheduler, 1, Point::new(150, 150));
        assert!(scheduler.is_in_flight());
        // The worker could not serve this window (`Unsupported`): the slot must be freed,
        // otherwise refinement would be wedged for the whole session.
        assert!(scheduler.on_failure(job.request));
        assert!(!scheduler.is_in_flight());
        assert!(scheduler.cached().is_none());

        // A superseded request must not be able to free the slot of a newer one.
        let first = submit(&mut scheduler, 1, Point::new(150, 150));
        // Moving to another *window* supersedes the running query instead.
        let _second = scheduler.on_cursor_moved(1, Some(window(0x200)), Point::new(160, 160));
        assert!(!scheduler.on_failure(first.request));
    }

    #[test]
    fn the_unsupported_provider_never_produces_a_path() {
        // The placeholder used until v2-P2 lands must degrade to the v1 frame, never
        // invent geometry.
        let mut provider = UnsupportedDeepSelection;
        let request = RequestGate::new().issue();
        let job = RefinementJob {
            request,
            window: window(0x100),
            epoch: 1,
            point: Point::new(10, 10),
        };
        let control = QueryControl::refinement(&|| false);
        assert_eq!(
            provider.resolve(&job, rect(0, 0, 100, 100), &control),
            RefinementOutcome::Empty(StopReason::Unsupported)
        );
        assert!(!control.is_cancelled());
        assert_eq!(control.budget.as_millis(), u128::from(REFINEMENT_BUDGET_MS));
        assert_eq!(
            control.call_limit.as_millis(),
            u128::from(REFINEMENT_CALL_LIMIT_MS)
        );
        assert!(!control.budget_exhausted(), "a fresh query has its budget");
    }

    /// The preview never falls back to the whole window frame while an answer for the current
    /// position is on its way (docs/18 §13.5).
    #[test]
    fn a_pending_answer_keeps_the_last_verified_rectangle_of_the_window() {
        let control = deep_target(TargetKind::UiElement, rect(300, 300, 500, 400), StopReason::Complete);
        let frame = rect(0, 0, 1000, 800);
        // The cursor left the control for its parent container: the frame must not flash while
        // the refinement answers, even though the control no longer covers the point.
        assert_eq!(
            preview_bounds(
                Some(&control),
                window(0x100),
                Point::new(600, 600),
                frame,
                true
            ),
            Some(rect(300, 300, 500, 400)),
            "the last verified rectangle covers the wait, not the window frame"
        );
        // The wait is bounded: once nothing is pending, the frame is the honest floor.
        assert_eq!(
            preview_bounds(
                Some(&control),
                window(0x100),
                Point::new(600, 600),
                frame,
                false
            ),
            Some(frame)
        );
    }

    #[test]
    fn the_first_hover_of_a_window_withholds_the_preview_until_it_answers() {
        let frame = rect(0, 0, 1000, 800);
        assert_eq!(
            preview_bounds(None, window(0x100), Point::new(10, 10), frame, true),
            None,
            "the first thing shown must be the control, not the whole frame"
        );
        assert_eq!(
            preview_bounds(None, window(0x100), Point::new(10, 10), frame, false),
            Some(frame),
            "an unsupported or failed provider still degrades to the v1 frame"
        );
    }

    #[test]
    fn a_verified_target_for_another_window_never_leaks_into_this_one() {
        let frame = rect(0, 0, 1000, 800);
        let elsewhere = deep_target(TargetKind::UiElement, rect(100, 100, 200, 200), StopReason::Complete);
        assert_eq!(
            preview_bounds(Some(&elsewhere), window(0x200), Point::new(150, 150), frame, true),
            None,
            "another window's path is not an answer for this one"
        );
        assert_eq!(
            preview_bounds(Some(&elsewhere), window(0x200), Point::new(150, 150), frame, false),
            Some(frame)
        );
    }

    #[test]
    fn a_target_that_still_covers_the_cursor_is_shown_even_when_nothing_is_pending() {
        let control = deep_target(TargetKind::UiElement, rect(300, 300, 500, 400), StopReason::Complete);
        assert_eq!(
            preview_bounds(
                Some(&control),
                window(0x100),
                Point::new(350, 350),
                rect(0, 0, 1000, 800),
                false
            ),
            Some(rect(300, 300, 500, 400))
        );
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

    fn deep_with_depth(bounds: Rect, depth: usize) -> DeepTarget {
        let mut path = vec![rect(0, 0, 1000, 800)];
        for _ in 1..depth.saturating_sub(1) {
            path.push(rect(50, 50, 950, 750));
        }
        path.push(bounds);
        DeepTarget {
            window: window(0x100),
            kind: TargetKind::UiElement,
            screen_bounds: *path.last().expect("path"),
            path,
            stop_reason: StopReason::Complete,
        }
    }

    #[test]
    fn a_deeper_target_replaces_immediately() {
        // Parent → child: exactly the case that must never lag.
        let parent = deep_with_depth(rect(100, 100, 900, 700), 3);
        let child = deep_with_depth(rect(300, 300, 500, 400), 6);
        assert_eq!(
            classify_replacement(Some(&parent), &child),
            Replacement::Immediate
        );
    }

    #[test]
    fn a_lateral_target_replaces_immediately() {
        // A → B side by side: same depth, neither contains the other.
        let a = deep_with_depth(rect(100, 100, 300, 200), 5);
        let b = deep_with_depth(rect(400, 100, 700, 300), 5);
        assert_eq!(classify_replacement(Some(&a), &b), Replacement::Immediate);
    }

    #[test]
    fn a_shallower_target_that_contains_the_current_one_needs_confirmation() {
        // The cursor drifted into a parent container while travelling between two controls:
        // this is the "expand then shrink" artefact, so it waits one dwell.
        let control = deep_with_depth(rect(300, 300, 500, 400), 6);
        let container = deep_with_depth(rect(100, 100, 900, 700), 3);
        assert_eq!(
            classify_replacement(Some(&control), &container),
            Replacement::NeedsConfirmation
        );
    }

    #[test]
    fn returning_to_a_parent_that_does_not_contain_the_old_target_is_immediate() {
        // Scenario 2 from the defect report: the cursor leaves the small box for a *different*
        // part of the parent. The new rect does not cover the old one, so nothing is delayed.
        let small = deep_with_depth(rect(100, 100, 200, 200), 6);
        let elsewhere = deep_with_depth(rect(600, 400, 900, 700), 3);
        assert_eq!(
            classify_replacement(Some(&small), &elsewhere),
            Replacement::Immediate
        );
    }

    #[test]
    fn the_first_target_and_an_unchanged_target_are_immediate() {
        let target = deep_with_depth(rect(300, 300, 500, 400), 6);
        assert_eq!(classify_replacement(None, &target), Replacement::Immediate);
        assert_eq!(
            classify_replacement(Some(&target), &target),
            Replacement::Immediate
        );
        // A path in another window is a different question entirely.
        let mut other = target;
        other.window = window(0x200);
        assert_eq!(
            classify_replacement(Some(&deep_with_depth(rect(300, 300, 500, 400), 6)), &other),
            Replacement::Immediate
        );
    }
}
