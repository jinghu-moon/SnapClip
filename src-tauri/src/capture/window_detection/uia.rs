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
}

impl WalkNode {
    pub const fn new(bounds: Rect, control_type: i32, offscreen: bool, enabled: bool) -> Self {
        Self {
            bounds,
            control_type,
            offscreen,
            enabled,
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

/// Pick the child that the cursor is inside, preferring the smallest such rectangle.
///
/// "Smallest wins" is the whole point of deep selection: when a control sits inside a
/// container that also contains the point, the user means the control. Ties go to the
/// first candidate, which preserves the tree's own sibling order.
pub fn deepest_child_at(point: Point, candidates: &[WalkNode]) -> Option<WalkNode> {
    candidates
        .iter()
        .copied()
        .filter(|node| !node.offscreen && !node.bounds.is_empty() && node.bounds.contains(point))
        .fold(None, |best: Option<WalkNode>, node| match best {
            Some(current) if current.bounds.area() <= node.bounds.area() => Some(current),
            _ => Some(node),
        })
}

/// Whether a node is a structural container that adds no selection value.
///
/// Such a node is still part of the published path (the path is "window → … → element"),
/// but it must never be the *final* answer while it has a descendable child under the
/// cursor.
pub fn is_structural_container(parent: Rect, node: WalkNode) -> bool {
    node.bounds == parent
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
    fn the_deepest_child_under_the_point_wins() {
        let point = Point::new(150, 150);
        let candidates = [
            node(0, 0, 1000, 1000),   // window-sized container
            node(100, 100, 400, 400), // pane
            node(140, 140, 200, 200), // the control the cursor is on
        ];
        assert_eq!(
            deepest_child_at(point, &candidates),
            Some(node(140, 140, 200, 200))
        );
        // A point outside every child resolves to nothing, so the caller keeps the parent.
        assert_eq!(deepest_child_at(Point::new(900, 900), &candidates[1..]), None);
    }

    #[test]
    fn off_screen_and_degenerate_children_are_never_chosen() {
        let point = Point::new(150, 150);
        let candidates = [
            WalkNode::new(rect(100, 100, 200, 200), 0, true, true), // off-screen
            WalkNode::new(rect(150, 150, 150, 150), 0, false, true), // empty
            node(120, 120, 300, 300),
        ];
        assert_eq!(
            deepest_child_at(point, &candidates),
            Some(node(120, 120, 300, 300))
        );
    }

    #[test]
    fn a_structural_container_is_descent_worthy_but_not_an_answer() {
        let window = rect(0, 0, 800, 600);
        let container = node(0, 0, 800, 600);
        assert!(is_descendable(window, container), "same-bounds panes must be entered");
        assert!(is_structural_container(window, container));
        // A real control is not a structural container.
        assert!(!is_structural_container(window, node(100, 100, 200, 200)));
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
}
