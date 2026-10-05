//! Bounded traversal policy for the v2 UIA provider (docs/18 §3).
//!
//! The accessibility tree is walked on the refinement worker, but the *decisions* —
//! which child contains the cursor, when a node is a structural container to descend
//! through, when the walk must stop — live here as pure functions over plain node data.
//! That keeps the expensive part (COM) tiny and makes every branch unit testable without
//! an accessibility stack, which matters because a provider that hangs or nests too deeply
//! is exactly the failure mode the design budgets for.

use super::deep::StopReason;
use crate::capture::geometry::{Point, Rect};

/// Bound on how deep the walk may descend before it gives up.
pub const MAX_DEPTH: usize = 24;
/// Bound on how many nodes may be examined in one query.
pub const MAX_NODES: usize = 4096;
/// Bound on how many rectangles the published path may contain.
pub const MAX_PATH_LEN: usize = 24;

/// One traversed element, already reduced to the data the policy needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalkNode {
    /// Screen bounds in virtual-desktop physical pixels.
    pub bounds: Rect,
    /// `UIA_ControlTypePropertyId`; `0` means "unknown".
    pub control_type: i32,
    /// `CurrentIsOffscreen`.
    pub offscreen: bool,
    /// `CurrentIsEnabled`; a disabled element is still a legitimate target (the user sees
    /// it), so this only feeds diagnostics today.
    pub enabled: bool,
    /// `NativeWindowHandle`; `0` for elements that are not backed by their own window.
    pub native_window: isize,
}

impl WalkNode {
    pub const fn new(bounds: Rect, control_type: i32, offscreen: bool, enabled: bool) -> Self {
        Self {
            bounds,
            control_type,
            offscreen,
            enabled,
            native_window: 0,
        }
    }

    pub const fn with_native_window(self, native_window: isize) -> Self {
        Self {
            native_window,
            ..self
        }
    }
}

/// The state of one walk: where we are and how much budget is left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalkBudget {
    nodes_left: usize,
    depth_left: usize,
}

impl WalkBudget {
    pub const fn new() -> Self {
        Self {
            nodes_left: MAX_NODES,
            depth_left: MAX_DEPTH,
        }
    }

    pub fn nodes_left(&self) -> usize {
        self.nodes_left
    }

    pub fn depth_left(&self) -> usize {
        self.depth_left
    }

    /// Consume one node. Returns `false` when the budget is exhausted (the caller must
    /// stop and publish what it has with [`StopReason::TraversalLimit`]).
    pub fn take_node(&mut self) -> bool {
        if self.nodes_left == 0 || self.depth_left == 0 {
            return false;
        }
        self.nodes_left -= 1;
        true
    }

    /// Consume one level of depth before descending into children.
    pub fn enter_children(&mut self) -> bool {
        if self.depth_left == 0 {
            return false;
        }
        self.depth_left -= 1;
        true
    }

    /// Give one level of depth back when the walk backtracks out of a dead branch: depth
    /// bounds how deep the current path is, not how many branches were tried (the node
    /// budget bounds that).
    pub fn leave_children(&mut self) {
        self.depth_left = (self.depth_left + 1).min(MAX_DEPTH);
    }
}

impl Default for WalkBudget {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether a candidate child is worth descending into.
///
/// Two rules from the reference selector:
///
/// * off-screen elements are skipped — the compositor is not showing them, so the user
///   cannot be pointing at them;
/// * a **structural container** (a `Pane`/`Group` whose bounds equal its parent's) is not
///   a stopping point: the walk must keep descending through it. Refusing to descend
///   through same-bounds containers is what hides real content in Chromium-family apps.
pub fn is_descendable(parent: Rect, child: WalkNode) -> bool {
    if child.offscreen || child.bounds.is_empty() {
        return false;
    }
    // Must actually overlap the parent; UIA occasionally reports children outside.
    !child.bounds.intersect(parent).is_empty()
}

/// `UIA_PaneControlTypeId`.
pub const PANE_CONTROL_TYPE: i32 = 50033;
/// `UIA_GroupControlTypeId`.
pub const GROUP_CONTROL_TYPE: i32 = 50026;

/// Whether a dead end may be backtracked out of to an earlier sibling.
///
/// UIA sibling order is not a stacking guarantee: Chromium lists a childless, window-sized
/// `Pane` *after* the pane that holds the page, so taking the last-listed child first ends the
/// walk on the wrapper and publishes the whole window. The reference selector
/// (`uia/cache.rs: structural_alternative`) only lets a walk leave a node that is a pure
/// wrapper — a `Pane`/`Group` with exactly its parent's bounds. Real controls and containers
/// with a frame of their own keep their precedence, so the rule cannot coarsen a fine answer.
pub fn is_structural_wrapper(parent: Rect, node: WalkNode) -> bool {
    matches!(node.control_type, PANE_CONTROL_TYPE | GROUP_CONTROL_TYPE) && node.bounds == parent
}

/// Result of a bounded walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkOutcome {
    /// Top-down path; `[0]` is the window frame.
    pub path: Vec<Rect>,
    /// The deepest element the walk accepted.
    pub target: Rect,
    pub stop_reason: StopReason,
}

impl WalkOutcome {
    /// A walk that could not get past the window frame.
    pub fn window_only(window_bounds: Rect, stop_reason: StopReason) -> Self {
        Self {
            path: vec![window_bounds],
            target: window_bounds,
            stop_reason,
        }
    }

    /// Push a deeper rectangle, keeping the path bounded.
    ///
    /// Duplicate rectangles are not appended: a container that reports the same bounds as
    /// its parent would otherwise pad the path with entries the overlay draws on top of
    /// each other.
    pub fn push(&mut self, bounds: Rect) -> bool {
        if self.path.len() >= MAX_PATH_LEN {
            self.stop_reason = StopReason::TraversalLimit;
            return false;
        }
        if self.path.last() == Some(&bounds) {
            return true;
        }
        self.path.push(bounds);
        self.target = bounds;
        true
    }
}

// ── 兜底命中路径与合并（docs/18 §12.2）───────────────────────────────────────
//
// Accessibility trees miss older and custom-drawn controls entirely. The reference
// selector therefore keeps a second, provider-free source of rectangles — the window's
// visible child *windows* — and merges both paths. These functions are that policy, kept
// pure so the merge rules can be tested without a COM or window station.

fn contains_rect(outer: Rect, inner: Rect) -> bool {
    !outer.is_empty()
        && !inner.is_empty()
        && inner.left >= outer.left
        && inner.top >= outer.top
        && inner.right <= outer.right
        && inner.bottom <= outer.bottom
}

fn same_rect(left: Rect, right: Rect) -> bool {
    left == right
}

/// Smallest-first ordering; ties broken by position so the result is deterministic.
fn rect_sort_key(rect: Rect) -> (i64, i32, i32, i32, i32) {
    (
        rect.area(),
        rect.left,
        rect.top,
        rect.right,
        rect.bottom,
    )
}

/// Push `rect` onto `path` when it is a usable, not-already-present entry.
///
/// A path entry must contain the point (otherwise it does not describe what the cursor is
/// on) and must differ from the current tail (otherwise the path would repeat a node).
fn push_if_useful(path: &mut Vec<Rect>, rect: Rect, point: Point) -> bool {
    if rect.is_empty() || !rect.contains(point) {
        return false;
    }
    if path.last() == Some(&rect) {
        return false;
    }
    path.push(rect);
    true
}

/// The provider-free hit path: every visible child window under the point, plus the frame.
///
/// Returned in the order they were collected; [`merge_hit_paths`] imposes the order.
pub fn fallback_hit_path(child_rects: &[Rect], window_bounds: Rect, point: Point) -> Vec<Rect> {
    let mut containing: Vec<Rect> = child_rects
        .iter()
        .copied()
        .filter(|rect| rect.contains(point))
        .collect();
    push_if_useful(&mut containing, window_bounds, point);
    containing
}

/// Merge an accessibility path with the fallback path into one ordered path.
///
/// **The fallback may only refine, never coarsen.** The primary path already runs
/// frame → deepest, so its last entry is the accessibility provider's most specific answer;
/// starting the merged path from the fallback (as an earlier revision did) let a coarse
/// child-window rectangle replace a fine control — and because the published rectangle is
/// `path.last()`, that silently turned every refinement back into "the whole window".
/// The fallback is therefore only ever *appended* when it lies strictly inside the current
/// tail.
///
/// The result keeps [`super::model::WindowTarget`]'s documented order: `path[0]` is the
/// window frame and the last entry is the most specific one, so `target = path.last()` is the
/// control under the cursor.
pub fn merge_hit_paths(
    primary: &[Rect],
    fallback: &[Rect],
    window_bounds: Rect,
    point: Point,
) -> Vec<Rect> {
    // 1. The primary path, cleaned: usable entries only, frame first.
    let mut path: Vec<Rect> = primary
        .iter()
        .copied()
        .filter(|rect| !rect.is_empty() && rect.contains(point))
        .collect();
    if path.is_empty() {
        path.push(window_bounds);
    }
    if path.first() != Some(&window_bounds) && window_bounds.contains(point) {
        path.insert(0, window_bounds);
    }

    // 2. Extend downward with fallback rectangles that are strictly inside the current tail.
    let mut deeper: Vec<Rect> = fallback
        .iter()
        .copied()
        .filter(|rect| !rect.is_empty() && rect.contains(point))
        .collect();
    deeper.sort_unstable_by_key(|rect| rect_sort_key(*rect));
    for candidate in deeper {
        let tail = *path.last().expect("the path is never empty");
        if !same_rect(candidate, tail) && contains_rect(tail, candidate) {
            path.push(candidate);
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect::new(left, top, right, bottom)
    }

    fn node(left: i32, top: i32, right: i32, bottom: i32) -> WalkNode {
        WalkNode::new(rect(left, top, right, bottom), 0, false, true)
    }

    #[test]
    fn a_same_bounds_container_is_descent_worthy() {
        let window = rect(0, 0, 800, 600);
        let container = node(0, 0, 800, 600);
        assert!(is_descendable(window, container), "same-bounds panes must be entered");
    }

    #[test]
    fn children_outside_the_parent_frame_are_rejected() {
        let window = rect(0, 0, 800, 600);
        // UIA sometimes reports children parked far outside; they must not be entered.
        assert!(!is_descendable(window, node(2000, 2000, 2100, 2100)));
        // Touching the edge only is not an overlap (half-open rectangles).
        assert!(!is_descendable(window, node(800, 0, 900, 100)));
        assert!(is_descendable(window, node(799, 0, 900, 100)));
        assert!(!is_descendable(window, WalkNode::new(rect(10, 10, 20, 20), 0, true, true)));
    }

    #[test]
    fn only_same_bounds_panes_and_groups_are_structural_wrappers() {
        let parent = rect(0, 0, 800, 600);
        let typed = |bounds: Rect, control_type| WalkNode::new(bounds, control_type, false, true);
        assert!(is_structural_wrapper(parent, typed(parent, PANE_CONTROL_TYPE)));
        assert!(is_structural_wrapper(parent, typed(parent, GROUP_CONTROL_TYPE)));
        // A container with a frame of its own is a real level, not a wrapper.
        assert!(!is_structural_wrapper(parent, typed(rect(0, 40, 800, 600), PANE_CONTROL_TYPE)));
        // A real control keeps its precedence even when it fills the parent (50000 = Button).
        assert!(!is_structural_wrapper(parent, typed(parent, 50000)));
        // Unknown control types are not assumed to be structural.
        assert!(!is_structural_wrapper(parent, typed(parent, 0)));
    }

    #[test]
    fn backtracking_gives_depth_back_but_never_beyond_the_bound() {
        let mut budget = WalkBudget::new();
        assert!(budget.enter_children());
        assert!(budget.enter_children());
        budget.leave_children();
        assert_eq!(budget.depth_left(), MAX_DEPTH - 1);
        budget.leave_children();
        budget.leave_children();
        assert_eq!(budget.depth_left(), MAX_DEPTH, "depth is capped at the declared bound");
    }

    #[test]
    fn the_node_and_depth_budgets_stop_the_walk() {
        let mut budget = WalkBudget::new();
        assert_eq!(budget.nodes_left(), MAX_NODES);
        assert_eq!(budget.depth_left(), MAX_DEPTH);
        for _ in 0..MAX_NODES {
            assert!(budget.take_node());
        }
        assert!(!budget.take_node(), "the node budget is exhausted");

        let mut budget = WalkBudget::new();
        for _ in 0..MAX_DEPTH {
            assert!(budget.enter_children());
        }
        assert!(!budget.enter_children(), "the depth budget is exhausted");
    }

    #[test]
    fn the_published_path_stays_ordered_and_bounded() {
        let window = rect(0, 0, 1000, 800);
        let mut outcome = WalkOutcome::window_only(window, StopReason::Complete);
        assert_eq!(outcome.path, vec![window]);
        assert!(outcome.push(rect(100, 100, 900, 700)));
        assert!(outcome.push(rect(300, 300, 500, 500)));
        assert_eq!(outcome.target, rect(300, 300, 500, 500));
        assert_eq!(outcome.path.len(), 3);

        // A container repeating its parent's bounds does not pad the path...
        assert!(outcome.push(rect(300, 300, 500, 500)));
        assert_eq!(outcome.path.len(), 3);
        // ...and a window-only walk is still a usable answer.
        let only = WalkOutcome::window_only(window, StopReason::Unsupported);
        assert_eq!(only.target, window);
        assert_eq!(only.path.len(), 1);
        assert_eq!(only.stop_reason, StopReason::Unsupported);
    }

    #[test]
    fn an_over_long_path_reports_the_traversal_limit() {
        let mut outcome = WalkOutcome::window_only(rect(0, 0, 10, 10), StopReason::Complete);
        // The window frame already occupies one slot, so MAX_PATH_LEN - 1 more fit.
        for index in 0..MAX_PATH_LEN - 1 {
            let offset = index as i32;
            assert!(outcome.push(rect(offset, offset, 900 + offset, 900 + offset)));
        }
        assert_eq!(outcome.path.len(), MAX_PATH_LEN);
        assert!(!outcome.push(rect(-1, -1, 899, 899)), "the path is full");
        assert_eq!(outcome.stop_reason, StopReason::TraversalLimit);
        assert!(!outcome.push(rect(5000, 5000, 5100, 5100)));
        assert_eq!(outcome.path.len(), MAX_PATH_LEN);
    }

    #[test]
    fn the_fallback_path_collects_the_children_under_the_point_and_the_frame() {
        let window = rect(0, 0, 1000, 800);
        let children = [
            rect(0, 0, 200, 800),   // navigation pane
            rect(200, 100, 1000, 700), // file list
            rect(0, 800, 100, 900), // outside the window entirely
        ];
        let path = fallback_hit_path(&children, window, Point::new(600, 400));
        assert!(path.contains(&rect(200, 100, 1000, 700)));
        assert!(path.contains(&window), "the frame closes the path");
        assert!(!path.contains(&rect(0, 800, 100, 900)), "off-window children stay out");
        // A point on no child still yields the frame.
        let bare = fallback_hit_path(&children, window, Point::new(900, 750));
        assert_eq!(bare, vec![window]);
    }

    #[test]
    fn merging_extends_the_primary_path_downwards_and_never_coarsens() {
        let window = rect(0, 0, 1000, 800);
        let pane = rect(100, 100, 900, 700);
        let control = rect(300, 300, 500, 400);
        let finer = rect(320, 320, 480, 380);
        let point = Point::new(400, 350);

        // The accessibility path runs frame → pane → control; the fallback knows something
        // deeper, so it is appended and the published rectangle gets finer.
        let merged = merge_hit_paths(&[window, pane, control], &[finer], window, point);
        assert_eq!(merged, vec![window, pane, control, finer]);
        assert_eq!(merged.last(), Some(&finer), "the deepest entry is published");

        // A *coarser* fallback rectangle must never replace the fine control — that was the
        // regression that made every publish the whole window.
        let unchanged = merge_hit_paths(&[window, pane, control], &[window], window, point);
        assert_eq!(unchanged, vec![window, pane, control]);
        assert_eq!(unchanged.last(), Some(&control));

        // With neither, the path is just the frame.
        let bare = merge_hit_paths(&[], &[], window, point);
        assert_eq!(bare, vec![window]);
    }

    #[test]
    fn merging_deduplicates_and_keeps_a_nested_order() {
        let window = rect(0, 0, 1000, 800);
        let outer = rect(100, 100, 900, 700);
        let inner = rect(300, 300, 500, 400);
        let deepest = rect(350, 330, 450, 370);
        let point = Point::new(400, 350);
        // Same rectangles from both sources, in different orders.
        let merged = merge_hit_paths(
            &[outer, inner],
            &[deepest, inner, outer],
            window,
            point,
        );
        let unique: Vec<Rect> = {
            let mut seen = Vec::new();
            for entry in &merged {
                if !seen.contains(entry) {
                    seen.push(*entry);
                }
            }
            seen
        };
        assert_eq!(unique.len(), merged.len(), "no duplicates: {merged:?}");
        assert_eq!(
            merged.first(),
            Some(&window),
            "the path is published frame-first"
        );
        assert_eq!(
            merged.last(),
            Some(&deepest),
            "the deepest entry ends the path, so `target = last()` is the control"
        );
        // Each level is contained by the one before it: a nested, outermost-first path.
        for pair in merged.windows(2) {
            assert!(
                !pair[0].is_empty() && pair[0] != pair[1],
                "levels must differ: {merged:?}"
            );
            assert!(
                pair[0].left <= pair[1].left
                    && pair[0].top <= pair[1].top
                    && pair[0].right >= pair[1].right
                    && pair[0].bottom >= pair[1].bottom,
                "each level must contain the next: {merged:?}"
            );
        }
    }

    #[test]
    fn merging_never_adds_a_level_that_does_not_contain_the_current_tail() {
        let window = rect(0, 0, 1000, 800);
        // The seed is the caller's trusted first entry (the providers filter by containment
        // before calling in); a sibling that contains the point but *not* the seed is not a
        // level of this path and must not be spliced in.
        let seed = rect(300, 300, 500, 400);
        let sibling = rect(700, 600, 900, 780);
        let merged = merge_hit_paths(&[seed], &[sibling], window, Point::new(400, 350));
        // Frame first (the documented order), then the deepest entry that was published.
        assert_eq!(merged, vec![window, seed]);
        assert_eq!(merged.last(), Some(&seed), "the sibling never becomes a level");
    }

}
