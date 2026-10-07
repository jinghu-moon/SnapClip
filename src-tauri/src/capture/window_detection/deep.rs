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

/// Which level of a published [`DeepTarget::path`] the user has selected (docs/21 §5.17).
///
/// The chain is outermost-first (`path[0]` is the window frame, the last entry is the published box),
/// so "deeper" moves toward the box and "shallower" toward the frame. A fresh chain selects the
/// deepest level — exactly what the refinement produced — which is what makes the level walk a pure
/// addition: until the user asks for another level, nothing about the answer changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelChain {
    levels: usize,
    index: usize,
}

impl LevelChain {
    /// A chain over `levels` entries, selecting the deepest.
    pub fn new(levels: usize) -> Self {
        Self {
            levels,
            index: levels.saturating_sub(1),
        }
    }

    /// How many levels the chain was built for.
    pub fn len(&self) -> usize {
        self.levels
    }

    pub fn is_empty(&self) -> bool {
        self.levels == 0
    }

    /// Index of the selected level; `0` is the window frame.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Whether the selection is the deepest level (the refinement's own answer).
    pub fn is_deepest(&self) -> bool {
        self.levels == 0 || self.index + 1 == self.levels
    }

    /// Move one level toward the window frame. `false` when already there.
    pub fn shallower(&mut self) -> bool {
        if self.index == 0 {
            return false;
        }
        self.index -= 1;
        true
    }

    /// Move one level toward the published box. `false` when already there.
    pub fn deeper(&mut self) -> bool {
        if self.index + 1 >= self.levels {
            return false;
        }
        self.index += 1;
        true
    }

    /// Back to the deepest level.
    pub fn reset(&mut self) {
        self.index = self.levels.saturating_sub(1);
    }

    /// Move to `index`, clamped to the chain. `false` when that is where it already was.
    pub fn jump_to(&mut self, index: usize) -> bool {
        let index = index.min(self.levels.saturating_sub(1));
        if index == self.index {
            return false;
        }
        self.index = index;
        true
    }

    /// The selected level out of `path`, clamped to what `path` actually holds.
    ///
    /// Clamping rather than trusting the index: a chain built for one target can outlive it by a
    /// frame, and the worst thing a level walk can do is publish a box that is not in the chain.
    pub fn current(&self, path: &[Rect]) -> Option<Rect> {
        let index = self.index.min(path.len().checked_sub(1)?);
        path.get(index).copied()
    }
}

/// Which side of the selected level a chain ring sits on (docs/21 §5.22).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingRole {
    /// The level the user has walked to — the box that would be captured. The overlay paints this
    /// one itself, with the capture colour and a double stroke; the ring brush never draws it.
    Selected,
    /// Shallower than the selection: the context outside it.
    Outer,
    /// Deeper than the selection: the layers the user walked out of, including the answer.
    Inner,
}

/// One ring to paint: a level of the chain and the opacity it gets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChainRing {
    /// Index into `DeepTarget::path` (0 = window frame).
    pub index: usize,
    pub role: RingRole,
    /// Effective alpha, after the outer/inner base and the distance ramp.
    pub alpha: f32,
}

/// Which rings the level walk draws, and what was left out (docs/21 §5.22).
///
/// The three exclusion lists are diagnostics, not paint: the layer stack in the prototype shows
/// them, and the session log can name them when a user asks why a level has no ring.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainPlan {
    /// In path order, outermost first.
    pub rings: Vec<ChainRing>,
    /// Below [`RingOptions::collapse_gap_px`] from the previous ring — it would draw a double line.
    pub collapsed: Vec<usize>,
    /// Within [`RingOptions::merge_gap_px`] of the selection: it *is* the selection's edge, so
    /// painting it only thickens that border.
    pub merged: Vec<usize>,
    /// Dropped to respect [`RingOptions::max_rings`] (never an anchor, never the selection).
    pub dropped: Vec<usize>,
}

/// How the chain is drawn. Defaults are the values the v2 prototype settled on (docs/21 §5.22).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingOptions {
    /// Minimum gap between two drawn rings, per edge.
    pub collapse_gap_px: i32,
    /// A ring this close to the selection is the selection's own edge.
    pub merge_gap_px: i32,
    /// Hard ceiling on drawn rings, including the selection.
    pub max_rings: usize,
    pub outer_alpha: f32,
    pub inner_alpha: f32,
    /// Fade rings with their distance from the selection (the floor is 65% of the base).
    pub ramp: bool,
    /// Keep the deepest level visible while the walk is above it — the way back down.
    pub keep_answer: bool,
}

impl Default for RingOptions {
    fn default() -> Self {
        Self {
            collapse_gap_px: 8,
            merge_gap_px: 2,
            max_rings: 7,
            // Raised after the first real-machine look: at 0.60 the chain was visible in the
            // prototype (32% mask) and invisible in the product (45% mask), because a mid-tone blue
            // carries almost no *luminance* against a masked light page. 0.85 with a compressed ramp
            // keeps every ring in one readable band.
            outer_alpha: 0.85,
            inner_alpha: 0.85,
            ramp: true,
            keep_answer: true,
        }
    }
}

/// Smallest distance between the corresponding edges of two rectangles.
///
/// The *minimum* over the four edges, not the maximum: a ring that hugs the previous one on any
/// side reads as a doubled line, which is what the collapse rule exists to prevent.
fn edge_gap(a: Rect, b: Rect) -> i32 {
    let left = (a.left - b.left).abs();
    let top = (a.top - b.top).abs();
    let right = ((a.left + a.width()) - (b.left + b.width())).abs();
    let bottom = ((a.top + a.height()) - (b.top + b.height())).abs();
    left.min(top).min(right).min(bottom)
}

/// Largest displacement between the corresponding edges of two rectangles.
///
/// The *maximum*, where [`edge_gap`] takes the minimum: they answer different questions. Collapse
/// asks "would these two lines be one line?" (any edge close ⇒ yes), while a level walk asks "would
/// moving here visibly change the box?" (any edge far ⇒ yes). Using one for the other is how the
/// wheel ends up with notches that appear to do nothing.
fn edge_shift(a: Rect, b: Rect) -> i32 {
    let left = (a.left - b.left).abs();
    let top = (a.top - b.top).abs();
    let right = ((a.left + a.width()) - (b.left + b.width())).abs();
    let bottom = ((a.top + a.height()) - (b.top + b.height())).abs();
    left.max(top).max(right).max(bottom)
}

/// The next level a walk stops on, skipping levels that would look identical to this one
/// (v3 B1, docs/21 §5.24).
///
/// With collapse and the cap of seven, some levels have no ring of their own: walking onto one
/// changes nothing on screen, so a notch of the wheel appears to be dead. `direction` is `-1`
/// toward the window frame or `+1` toward the published box; the ends always stop.
pub fn next_visible_stop(path: &[Rect], current: usize, direction: i32, threshold_px: i32) -> usize {
    if path.is_empty() || direction == 0 {
        return current.min(path.len().saturating_sub(1));
    }
    let current = current.min(path.len() - 1);
    let step: i32 = if direction < 0 { -1 } else { 1 };
    let mut candidate = current;
    loop {
        let next = candidate as i32 + step;
        // The ends stop even if the last level is visually nested inside its neighbour: reaching the
        // frame (or the answer) is a state the user asked for, not a level to be skipped.
        if next <= 0 || next as usize >= path.len() - 1 {
            return next.clamp(0, path.len() as i32 - 1) as usize;
        }
        candidate = next as usize;
        if edge_shift(path[candidate], path[current]) >= threshold_px {
            return candidate;
        }
    }
}

/// How many stops the walk still has from `current` in `direction` (v3 A1, docs/21 §5.24).
///
/// This is what the level badge's `↑3 ↓5` counts: **not** the number of levels left — some of those
/// have no ring of their own and B1 skips them — but how many more notches of the wheel land
/// somewhere that looks different before the end. Zero means "already against the end", which the
/// badge draws dimmed.
pub fn stops_from(path: &[Rect], current: usize, direction: i32, threshold_px: i32) -> usize {
    if path.is_empty() {
        return 0;
    }
    let mut cursor = current.min(path.len() - 1);
    let mut stops = 0;
    loop {
        let next = next_visible_stop(path, cursor, direction, threshold_px);
        // The walk is finite and every iteration either moves or ends, so `stops > path.len()` is
        // only a guard against a future `next_visible_stop` that returns a cycle.
        if next == cursor || stops > path.len() {
            return stops;
        }
        cursor = next;
        stops += 1;
    }
}

/// Decide which rings the level walk paints (docs/21 §5.22).
///
/// The rules, in the order they are applied — the first three are what keep a nine-level chain
/// readable, the fourth is what keeps it bounded:
///
/// 1. **Anchors**: the window frame, the level just outside the selection, the selection itself,
///    the level just inside it, and (optionally) the deepest level — the way back down.
/// 2. **Collapse**: any other level closer than `collapse_gap_px` to the last drawn ring is
///    skipped; a 1 px-inset wrapper would only be a double line.
/// 3. **Merge**: a ring within `merge_gap_px` of the selection is that edge, so it is skipped —
///    drawing it thickens the selection's border and looks like a smeared stroke. This happens on
///    real pages (`cm-scroller` sits 1 px inside `code-block-viewer`), not only in fixtures.
/// 4. **Cap**: at most `max_rings` are drawn. Non-anchors are dropped first, the ones closest to
///    the selection kept. Anchors are at most five, so the cap is always reachable without
///    breaking the outermost/innermost references.
pub fn chain_rings(path: &[Rect], selected: usize, options: RingOptions) -> ChainPlan {
    let mut plan = ChainPlan {
        rings: Vec::new(),
        collapsed: Vec::new(),
        merged: Vec::new(),
        dropped: Vec::new(),
    };
    if path.is_empty() {
        return plan;
    }
    let selected = selected.min(path.len() - 1);
    let last = path.len() - 1;
    let anchor = |index: usize| {
        index == 0
            || index == selected
            || index + 1 == selected
            || index == selected + 1
            || (options.keep_answer && index == last)
    };
    let role_of = |index: usize| {
        if index == selected {
            RingRole::Selected
        } else if index < selected {
            RingRole::Outer
        } else {
            RingRole::Inner
        }
    };
    let alpha_of = |index: usize| {
        let base = match role_of(index) {
            // Painted by the overlay, at full strength; the plan's number is never used for it.
            RingRole::Selected => return 1.0,
            RingRole::Outer => options.outer_alpha,
            RingRole::Inner => options.inner_alpha,
        };
        if !options.ramp {
            return base;
        }
        // Distance 1 (the selection's neighbours) keeps the base; each further step costs 6%,
        // floored at 80% so the outermost ring — the "where am I" reference — stays visible. The
        // ramp is deliberately gentle: with a 45% mask the eye needs *all* the rings to read, and
        // the distance cue is carried by position far more than by opacity.
        let distance = index.abs_diff(selected).saturating_sub(1).min(6) as f32;
        base * (1.0 - 0.06 * distance).max(0.80)
    };

    let mut kept: Vec<usize> = Vec::new();
    for index in 0..path.len() {
        if index == selected {
            kept.push(index);
            plan.rings.push(ChainRing {
                index,
                role: RingRole::Selected,
                alpha: 1.0,
            });
            continue;
        }
        let previous = kept.last().copied();
        let gap = previous.map_or(i32::MAX, |previous| edge_gap(path[index], path[previous]));
        if anchor(index) {
            if edge_gap(path[index], path[selected]) < options.merge_gap_px {
                plan.merged.push(index);
                continue;
            }
            kept.push(index);
            plan.rings.push(ChainRing {
                index,
                role: role_of(index),
                alpha: alpha_of(index),
            });
            continue;
        }
        if gap >= options.collapse_gap_px {
            kept.push(index);
            plan.rings.push(ChainRing {
                index,
                role: role_of(index),
                alpha: alpha_of(index),
            });
        } else {
            plan.collapsed.push(index);
        }
    }

    while plan.rings.len() > options.max_rings {
        // Drop the non-anchor ring that is furthest from the selection; at equal distance, the one
        // with the smallest gap to its neighbour (it contributes the least shape).
        let victim = plan
            .rings
            .iter()
            .enumerate()
            .filter(|(_, ring)| ring.index != selected && !anchor(ring.index))
            .max_by_key(|(position, ring)| {
                (
                    ring.index.abs_diff(selected),
                    -(edge_gap(
                        path[ring.index],
                        path[plan.rings[position.saturating_sub(1)].index],
                    )),
                )
            })
            .map(|(position, _)| position);
        match victim {
            Some(position) => {
                let ring = plan.rings.remove(position);
                plan.dropped.push(ring.index);
            }
            // Every remaining ring is an anchor, and anchors are at most five.
            None => break,
        }
    }
    plan.dropped.sort_unstable();
    plan.collapsed.sort_unstable();
    plan.merged.sort_unstable();
    plan
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
///
/// `level` is the ancestor level the user walked to (docs/21 §5.17); it replaces the published box
/// while the target still applies, and `None` means "the published box itself".
pub fn preview_bounds(
    deep: Option<&DeepTarget>,
    level: Option<Rect>,
    window: WindowIdentity,
    point: Point,
    window_bounds: Rect,
    waiting: bool,
) -> Option<Rect> {
    match deep.filter(|deep| deep.window == window) {
        Some(deep) if deep.covers(point) => level.or(Some(deep.screen_bounds)),
        Some(deep) if waiting => level.or(Some(deep.screen_bounds)),
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

    #[test]
    fn a_fresh_chain_selects_the_deepest_level() {
        let path = vec![
            rect(0, 0, 800, 600),
            rect(100, 100, 500, 400),
            rect(200, 200, 300, 300),
        ];
        let chain = LevelChain::new(path.len());
        assert_eq!(chain.index(), 2);
        assert!(chain.is_deepest());
        assert_eq!(chain.current(&path), Some(rect(200, 200, 300, 300)));
    }

    #[test]
    fn stepping_up_stops_at_the_window_frame_and_down_at_the_box() {
        let path = vec![rect(0, 0, 800, 600), rect(100, 100, 500, 400)];
        let mut chain = LevelChain::new(path.len());
        assert!(chain.shallower(), "from the box to its parent");
        assert_eq!(chain.current(&path), Some(rect(0, 0, 800, 600)));
        assert!(!chain.shallower(), "the frame is the outermost level");
        assert_eq!(chain.index(), 0);
        assert!(chain.deeper());
        assert!(!chain.deeper(), "the published box is the innermost level");
        assert!(chain.is_deepest());
    }

    #[test]
    fn a_window_only_answer_has_nowhere_to_walk() {
        let path = vec![rect(0, 0, 800, 600)];
        let mut chain = LevelChain::new(path.len());
        assert!(chain.is_deepest());
        assert!(!chain.shallower());
        assert!(!chain.deeper());
        assert_eq!(chain.current(&path), Some(rect(0, 0, 800, 600)));
    }

    /// v3 B1 (docs/21 §5.24): a wheel notch has to land somewhere that *looks* different.
    ///
    /// Collapse and the cap of seven mean some levels have no ring of their own, and walking onto
    /// one of those repaints pixels that are already on screen — a notch that appears to do nothing.
    /// The walk therefore compares rectangles by their **largest** edge displacement, the opposite
    /// extreme of the collapse rule's smallest, and the two ends always stop.
    #[test]
    fn a_level_walk_skips_the_levels_that_look_identical() {
        let path = vec![
            rect(0, 0, 1000, 600),  // 0: the window frame
            rect(1, 1, 999, 599),   // 1: a 1 px lip — no ring of its own
            rect(2, 2, 998, 598),   // 2: 1 px more
            rect(3, 3, 997, 597),   // 3: 1 px more
            rect(80, 60, 900, 520), // 4: a box the user can actually see
            rect(81, 61, 899, 519), // 5: 1 px inside it — the published answer
        ];
        // Read the threshold from the options: it is a tuning knob, and this test is about the rule.
        let step = RingOptions::default().collapse_gap_px;

        // Three notches of 1 px each are one stop: from the frame, the walk lands on the box.
        assert_eq!(next_visible_stop(&path, 0, 1, step), 4);
        // …and the answer is always a stop, even one pixel inside the ring before it.
        assert_eq!(next_visible_stop(&path, 4, 1, step), 5);
        // Upward from the answer, level 4 looks the same, so the stop is 3.
        assert_eq!(next_visible_stop(&path, 5, -1, step), 3);
        // A further notch at an end absorbs rather than wraps.
        assert_eq!(next_visible_stop(&path, 0, -1, step), 0);
        assert_eq!(next_visible_stop(&path, 5, 1, step), 5);
        // Degenerate inputs are the no-op they look like, not a panic.
        assert_eq!(next_visible_stop(&[], 0, 1, step), 0);
        assert_eq!(next_visible_stop(&[rect(0, 0, 10, 10)], 0, 1, step), 0);
        assert_eq!(next_visible_stop(&path, 3, 0, step), 3);

        // The chain the walk moves, and what "nothing moved" means to the caller: an event that
        // lands back on the current level is not consumed by the wheel (docs/21 §5.17).
        let mut chain = LevelChain::new(path.len());
        assert_eq!(chain.index(), 5, "a fresh chain starts on the answer");
        assert!(chain.jump_to(0));
        assert_eq!(chain.index(), 0);
        assert!(!chain.jump_to(0));
        assert!(chain.jump_to(99), "clamped, not rejected");
        assert_eq!(chain.index(), 5);
    }

    /// v3 A1 (docs/21 §5.24): the badge counts *stops*, not levels — the wheel's remaining travel,
    /// which is what makes `↑3 ↓5` honest when some levels have no ring of their own.
    #[test]
    fn the_remaining_stops_count_notches_that_land_somewhere_new() {
        let path = vec![
            rect(0, 0, 1000, 600),   // 0: the window frame
            rect(1, 1, 999, 599),    // 1: 1 px in — no stop of its own
            rect(2, 2, 998, 598),    // 2: 1 px more
            rect(3, 3, 997, 597),    // 3: 1 px more
            rect(80, 60, 900, 520),  // 4: a box the user can see
            rect(81, 61, 899, 519),  // 5: the published answer
        ];
        let step = RingOptions::default().collapse_gap_px;

        // From the answer: one notch up lands on the box (4), the next on the frame (0) — three
        // 1 px levels in between are skipped, so two stops, not five levels.
        assert_eq!(stops_from(&path, 5, -1, step), 2);
        // From the answer there is nothing deeper to walk to.
        assert_eq!(stops_from(&path, 5, 1, step), 0);
        // Downward from the frame: the box, then the answer.
        assert_eq!(stops_from(&path, 0, 1, step), 2);
        assert_eq!(stops_from(&path, 0, -1, step), 0);
        // Mid-chain: one notch to the answer, and up is two — the 1 px lip (3) *is* a stop from the
        // box, because the box is 97 px inside it; the levels that collapse are the ones between the
        // stop you are on and the next one.
        assert_eq!(stops_from(&path, 4, 1, step), 1);
        assert_eq!(stops_from(&path, 4, -1, step), 2);
        // Standing on one of the collapsed levels is the same walk: the next stop is what matters.
        assert_eq!(stops_from(&path, 2, -1, step), 1);
        // Degenerate inputs answer rather than panic.
        assert_eq!(stops_from(&[], 0, 1, step), 0);
        assert_eq!(stops_from(&[rect(0, 0, 10, 10)], 0, 1, step), 0);
    }

    #[test]
    fn reset_goes_back_to_what_the_refinement_published() {
        let path = vec![rect(0, 0, 800, 600), rect(100, 100, 500, 400)];
        let mut chain = LevelChain::new(path.len());
        chain.shallower();
        assert!(!chain.is_deepest());
        chain.reset();
        assert!(chain.is_deepest());
        assert_eq!(chain.current(&path), Some(rect(100, 100, 500, 400)));
    }

    #[test]
    fn a_walked_level_overrides_the_published_box_in_the_preview() {
        let frame = rect(0, 0, 1000, 800);
        let control = deep_target(
            TargetKind::UiElement,
            rect(300, 300, 500, 400),
            StopReason::Complete,
        );
        // A level the user walked to wins over the published box while the target still answers…
        assert_eq!(
            preview_bounds(
                Some(&control),
                Some(rect(100, 100, 900, 700)),
                window(0x100),
                Point::new(350, 350),
                frame,
                false
            ),
            Some(rect(100, 100, 900, 700))
        );
        // …and with no level walked to, the published box is what shows.
        assert_eq!(
            preview_bounds(
                Some(&control),
                None,
                window(0x100),
                Point::new(350, 350),
                frame,
                false
            ),
            Some(rect(300, 300, 500, 400))
        );
        // A level never resurrects a target that belongs to another window.
        assert_eq!(
            preview_bounds(
                Some(&control),
                Some(rect(100, 100, 900, 700)),
                window(0x200),
                Point::new(350, 350),
                frame,
                false
            ),
            Some(frame)
        );
    }

    #[test]
    fn a_stale_chain_cannot_publish_a_box_outside_the_path() {
        // A chain built for a four-level answer, then a shorter path arrives first.
        let short = vec![rect(0, 0, 800, 600), rect(10, 10, 20, 20)];
        let mut chain = LevelChain::new(4);
        assert_eq!(chain.index(), 3);
        assert_eq!(
            chain.current(&short),
            Some(rect(10, 10, 20, 20)),
            "the index is clamped to the path it is asked about"
        );
        // An empty path has nothing to select at all.
        assert_eq!(chain.current(&[]), None);
        // Stepping up from an index that is still above the short path keeps answering with the
        // path's deepest level; only when the index itself reaches zero does the frame come back.
        assert!(chain.shallower(), "walking up from the clamped index still moves");
        assert_eq!(chain.current(&short), Some(rect(10, 10, 20, 20)));
        while chain.shallower() {}
        assert_eq!(chain.current(&short), Some(rect(0, 0, 800, 600)));
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
                None,
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
                None,
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
            preview_bounds(None, None, window(0x100), Point::new(10, 10), frame, true),
            None,
            "the first thing shown must be the control, not the whole frame"
        );
        assert_eq!(
            preview_bounds(None, None, window(0x100), Point::new(10, 10), frame, false),
            Some(frame),
            "an unsupported or failed provider still degrades to the v1 frame"
        );
    }

    #[test]
    fn a_verified_target_for_another_window_never_leaks_into_this_one() {
        let frame = rect(0, 0, 1000, 800);
        let elsewhere = deep_target(TargetKind::UiElement, rect(100, 100, 200, 200), StopReason::Complete);
        assert_eq!(
            preview_bounds(Some(&elsewhere), None, window(0x200), Point::new(150, 150), frame, true),
            None,
            "another window's path is not an answer for this one"
        );
        assert_eq!(
            preview_bounds(Some(&elsewhere), None, window(0x200), Point::new(150, 150), frame, false),
            Some(frame)
        );
    }

    #[test]
    fn a_target_that_still_covers_the_cursor_is_shown_even_when_nothing_is_pending() {
        let control = deep_target(TargetKind::UiElement, rect(300, 300, 500, 400), StopReason::Complete);
        assert_eq!(
            preview_bounds(
                Some(&control),
                None,
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

    /// The nine-level shape a real chat page produced (docs/21 §5.22), with the 1 px nesting the
    /// prototype's fixture mirrors: `cm-scroller` sits 1 px inside `code-block-viewer`, and
    /// `text-message` 1 px outside it.
    fn nine_level_page() -> Vec<Rect> {
        vec![
            rect(40, 24, 1240, 684),      // 1/9 window frame
            rect(41, 58, 1239, 684),      // 2/9 document
            rect(53, 70, 1227, 672),      // 3/9 side-pane shell
            rect(65, 82, 945, 648),       // 4/9 message list
            rect(69, 148, 917, 478),      // 5/9 text message
            rect(70, 196, 916, 468),      // 6/9 code block viewer (selected in these tests)
            rect(71, 197, 915, 467),      // 7/9 cm-scroller  ← 1 px inside the selection
            rect(87, 213, 899, 435),      // 8/9 pre.cm-content
            rect(115, 243, 214, 265),     // 9/9 code span (the answer)
        ]
    }

    /// The rule set as a whole, on the fixture the prototype uses — same numbers on both sides:
    /// `9 层 → 画 6（外 3 · 内 2）· 塌缩 1 · 与选中层合并 2`.
    #[test]
    fn the_chain_plan_matches_the_prototype_on_the_nine_level_page() {
        let path = nine_level_page();
        let plan = chain_rings(&path, 5, RingOptions::default());
        let drawn: Vec<usize> = plan.rings.iter().map(|ring| ring.index).collect();
        assert_eq!(drawn, vec![0, 2, 3, 5, 7, 8], "window, two ancestors, selection, two inners");
        assert_eq!(
            plan.rings
                .iter()
                .filter(|ring| ring.role == RingRole::Outer)
                .count(),
            3
        );
        assert_eq!(
            plan.rings
                .iter()
                .filter(|ring| ring.role == RingRole::Inner)
                .count(),
            2,
            "the answer and the layer above it are still visible: the way back down"
        );
        // 2/9 is 1 px from 1/9 → a double line; 5/9 and 7/9 are 1 px from the selection.
        assert_eq!(plan.collapsed, vec![1]);
        assert_eq!(plan.merged, vec![4, 6]);
        assert!(plan.dropped.is_empty(), "six rings fit under the cap of seven");
    }

    /// Ramp: the further a ring is from the selection, the fainter — with a floor so the window
    /// frame stays readable (docs/21 §5.22 measured this in the prototype).
    #[test]
    fn rings_fade_with_distance_from_the_selection_and_floor_at_65_percent() {
        let path = nine_level_page();
        let plan = chain_rings(&path, 5, RingOptions::default());
        let alpha = |index: usize| {
            plan.rings
                .iter()
                .find(|ring| ring.index == index)
                .map(|ring| ring.alpha)
                .unwrap_or_default()
        };
        // Read the base from the options rather than restating it: the numbers are a tuning knob and
        // the assertions below are about the *shape* of the ramp.
        let base = RingOptions::default().outer_alpha;
        // One step away is 94% of the base (the 6% ramp), and the two rings *adjacent* to the
        // selection in this fixture are exactly the ones the merge rule removes — which is why the
        // ramp starts biting at distance two.
        assert!((alpha(3) - base * 0.94).abs() < 1e-5, "one step out: {}", alpha(3));
        assert!((alpha(7) - base * 0.94).abs() < 1e-5, "one step in: {}", alpha(7));
        assert!(alpha(2) < alpha(3), "two steps out is fainter than one: {} vs {}", alpha(2), alpha(3));
        assert!(
            (alpha(0) - base * 0.80).abs() < 1e-5,
            "the window frame is floored at 80% of the base: {}",
            alpha(0)
        );

        // …and the ramp can be turned off, which is the comparison the prototype offers.
        let flat = chain_rings(
            &path,
            5,
            RingOptions {
                ramp: false,
                ..RingOptions::default()
            },
        );
        assert!(flat
            .rings
            .iter()
            .all(|ring| ring.role == RingRole::Selected || (ring.alpha - base).abs() < 1e-6));
    }

    /// Twelve evenly spaced wrappers: the cap has to bite, and it must bite the *non-anchors*
    /// furthest from the selection. Prototype: `12 层 → 画 7 · 丢弃 5`, keeping
    /// `{0, 3, 4, 5, 6, 7, 11}`.
    #[test]
    fn the_cap_drops_the_furthest_non_anchors() {
        let path: Vec<Rect> = (0..11)
            .map(|step| {
                let inset = step * 16;
                rect(24 + inset, 16 + inset, 1256 - inset, 704 - inset)
            })
            .chain(std::iter::once(rect(200, 216, 1080, 256)))
            .collect();
        let plan = chain_rings(&path, 5, RingOptions::default());
        let drawn: Vec<usize> = plan.rings.iter().map(|ring| ring.index).collect();
        assert_eq!(drawn, vec![0, 3, 4, 5, 6, 7, 11]);
        assert_eq!(plan.rings.len(), 7, "the cap holds");
        assert!(plan.dropped.contains(&1), "the furthest non-anchor goes first: {:?}", plan.dropped);
        // The anchors — window frame and the answer — never get dropped.
        assert!(drawn.contains(&0) && drawn.contains(&11));
    }

    /// Two levels: the window and the answer. Nothing to collapse, merge or drop — and no inner
    /// ring, which is why ③ changes nothing until the user actually walks.
    #[test]
    fn a_two_level_chain_draws_two_rings_and_no_inners() {
        let path = vec![rect(0, 0, 1280, 720), rect(460, 300, 820, 328)];
        let plan = chain_rings(&path, 1, RingOptions::default());
        let drawn: Vec<usize> = plan.rings.iter().map(|ring| ring.index).collect();
        assert_eq!(drawn, vec![0, 1]);
        assert!(
            plan.rings.iter().all(|ring| ring.role != RingRole::Inner),
            "no inner rings: nothing has been walked out of yet"
        );

        // Walking all the way up to the window frame keeps the answer ring: that is the way back
        // down, and it is the whole point of drawing the inner side at all.
        let at_window = chain_rings(&path, 0, RingOptions::default());
        assert_eq!(
            at_window.rings.iter().map(|ring| ring.index).collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(at_window.rings[1].role, RingRole::Inner);
        // The level immediately inside the selection is an unconditional anchor — you are standing
        // on it — so turning the "keep the answer" switch off changes nothing here. (The switch
        // matters when the answer would otherwise be collapsed; see the test below.)
        let without_answer = chain_rings(
            &path,
            0,
            RingOptions {
                keep_answer: false,
                ..RingOptions::default()
            },
        );
        assert_eq!(without_answer.rings.len(), 2);
    }

    /// `keep_answer` earns its keep only when the answer would otherwise be collapsed — a text run
    /// sitting a hair inside its own wrapper, *and* the walk further than one level above it. Closer
    /// than that, two other rules already cover the answer: the level just inside the selection is
    /// an unconditional anchor, and a ring within the merge gap of the selection is skipped anyway.
    #[test]
    fn the_answer_ring_survives_collapse_only_while_it_is_kept() {
        let path = vec![
            rect(0, 0, 400, 300),
            rect(40, 40, 360, 260),   // the level the walk is on
            rect(100, 100, 300, 200), // the shell inside it
            rect(104, 104, 296, 196), // the answer, 4 px inside that shell
        ];
        let kept = chain_rings(&path, 1, RingOptions::default());
        assert!(
            kept.rings.iter().any(|ring| ring.index == 3),
            "the way back down is visible: {:?}",
            kept.rings
        );
        let dropped = chain_rings(
            &path,
            1,
            RingOptions {
                keep_answer: false,
                ..RingOptions::default()
            },
        );
        assert!(!dropped.rings.iter().any(|ring| ring.index == 3));
        assert_eq!(
            dropped.collapsed,
            vec![3],
            "…and it is reported as collapsed, not silently gone"
        );
    }

    /// A degenerate path (one level: the window) and an empty one must not panic.
    #[test]
    fn degenerate_paths_are_answered_without_rings_or_panics() {
        assert!(chain_rings(&[], 0, RingOptions::default()).rings.is_empty());
        let single = chain_rings(&[rect(0, 0, 100, 100)], 0, RingOptions::default());
        assert_eq!(single.rings.len(), 1);
        assert_eq!(single.rings[0].index, 0);
        // An index past the end clamps to the deepest level rather than panicking.
        let clamped = chain_rings(&nine_level_page(), 99, RingOptions::default());
        assert!(clamped.rings.iter().any(|ring| ring.index == 8));
    }
}
