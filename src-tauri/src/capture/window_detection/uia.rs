//! Bounded traversal policy for the v2 UIA provider (docs/18 §3).
//!
//! The accessibility tree is walked on the refinement worker, but the *decisions* —
//! which child contains the cursor, when a node is a structural container to descend
//! through, when the walk must stop — live here as pure functions over plain node data.
//! That keeps the expensive part (COM) tiny and makes every branch unit testable without
//! an accessibility stack, which matters because a provider that hangs or nests too deeply
//! is exactly the failure mode the design budgets for.

use super::deep::StopReason;
use super::model::{LevelKind, PathLevel};
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

/// `UIA_TextControlTypeId`: a run of glyphs, not a control.
pub const TEXT_CONTROL_TYPE: i32 = 50020;

/// UIA's `Document` control type: the page/web area. It is what the provider answers when it cannot
/// get below the document, so it is an *unspecific* answer rather than a target (docs/21 §5.19).
pub const DOCUMENT_CONTROL_TYPE: i32 = 50030;

/// Whether an answer is too coarse to count as "the element under the cursor".
///
/// Two shapes qualify: the document/web area itself, and a box that covers essentially the whole
/// window. Both mean the transport did not get below the page — which is exactly when the second
/// opinion (MSAA's renderer hit test) is worth asking, and exactly when it is *not*: a button whose
/// own box came back is a real answer, so its label must not outrank it just for being smaller.
pub fn is_unspecific_hit(bounds: Rect, control_type: i32, window_bounds: Rect) -> bool {
    control_type == DOCUMENT_CONTROL_TYPE
        || (window_bounds.area() > 0 && bounds.area() * 10 >= window_bounds.area() * 9)
}

/// MSAA roles of controls whose label is *part of the control* rather than a target of its own.
///
/// Pointing at a button must give the button, not the word on it — a 74x16 crop of a label is not a
/// screenshot anyone wants — while a line of text inside a container or an editor is its own box.
/// That is the line this list draws (docs/21 §5.19).
pub fn is_interactive_control_role(role: i32) -> bool {
    matches!(
        role,
        0x1E // ROLE_SYSTEM_LINK
            | 0x2B // ROLE_SYSTEM_PUSHBUTTON
            | 0x2C // ROLE_SYSTEM_CHECKBUTTON
            | 0x2D // ROLE_SYSTEM_RADIOBUTTON
            | 0x2E // ROLE_SYSTEM_COMBOBOX
            | 0x2F // ROLE_SYSTEM_DROPLIST
            | 0x0C // ROLE_SYSTEM_MENUITEM
            | 0x22 // ROLE_SYSTEM_LISTITEM
            | 0x25 // ROLE_SYSTEM_PAGETAB
            | 0x33 // ROLE_SYSTEM_SLIDER
            | 0x34 // ROLE_SYSTEM_SPINBUTTON
    )
}

/// MSAA roles that describe a bare text run rather than a box (docs/21 §5.15).
///
/// `ROLE_SYSTEM_STATICTEXT` / `ROLE_SYSTEM_TEXT` are the MSAA equivalents of the UIA `Text`
/// control type the product refuses to adopt: publishing the glyphs inside a control instead of
/// the control is a product decision, and it has to hold for both transports.
pub const MSAA_STATIC_TEXT_ROLE: i32 = 0x29;
pub const MSAA_TEXT_ROLE: i32 = 0x2a;

/// The label vocabulary for a UIA control type (docs/21 §5.24, B6).
///
/// One mapping, at the boundary: the walk reads this for every node it accepts, so each published
/// level carries what it is. Anything unrecognised — including the structural `Pane`/`Group`/`Custom`
/// wrappers that dominate a browser's tree — maps to [`LevelKind::Unknown`], which the label reads as
/// "keep saying 容器/元素".
pub fn level_kind_of_control_type(control_type: i32) -> LevelKind {
    match control_type {
        50000 => LevelKind::Button,
        50002 => LevelKind::CheckBox,
        50003 => LevelKind::ComboBox,
        50004 => LevelKind::Edit,
        50005 => LevelKind::Hyperlink,
        50006 => LevelKind::Image,
        50007 => LevelKind::ListItem,
        50008 => LevelKind::List,
        50009 => LevelKind::Menu,
        50012 => LevelKind::ProgressBar,
        50013 => LevelKind::RadioButton,
        50014 => LevelKind::ScrollBar,
        50015 => LevelKind::Slider,
        50018 => LevelKind::Tab,
        50020 => LevelKind::Text,
        50021 => LevelKind::ToolBar,
        50023 => LevelKind::Tree,
        50025 => LevelKind::Custom,
        50026 => LevelKind::Group,
        50028 => LevelKind::DataGrid,
        50029 => LevelKind::DataItem,
        50030 => LevelKind::Document,
        50032 => LevelKind::Window,
        50033 => LevelKind::Pane,
        50034 => LevelKind::Header,
        50036 => LevelKind::Table,
        50037 => LevelKind::TitleBar,
        _ => LevelKind::Unknown,
    }
}

/// The same vocabulary for an MSAA role — the transport that answers when UIA stops above the box
/// (docs/21 §5.16). Roles are `ROLE_SYSTEM_*`; only the ones a label has something to say about are
/// listed, and everything else stays [`LevelKind::Unknown`].
pub fn level_kind_of_msaa_role(role: i32) -> LevelKind {
    match role {
        0x02 => LevelKind::Menu,        // ROLE_SYSTEM_MENUBAR
        0x03 => LevelKind::ScrollBar,   // ROLE_SYSTEM_SCROLLBAR
        0x0c => LevelKind::Menu,        // ROLE_SYSTEM_MENUITEM
        0x0f => LevelKind::Document,    // ROLE_SYSTEM_DOCUMENT
        0x10 => LevelKind::Pane,        // ROLE_SYSTEM_PANE
        0x14 => LevelKind::Group,       // ROLE_SYSTEM_GROUPING
        0x16 => LevelKind::ToolBar,     // ROLE_SYSTEM_TOOLBAR
        0x18 => LevelKind::Table,       // ROLE_SYSTEM_TABLE
        0x1d => LevelKind::DataItem,    // ROLE_SYSTEM_CELL
        0x1e => LevelKind::Hyperlink,   // ROLE_SYSTEM_LINK
        0x21 => LevelKind::List,        // ROLE_SYSTEM_LIST
        0x22 => LevelKind::ListItem,    // ROLE_SYSTEM_LISTITEM
        0x23 => LevelKind::Tree,        // ROLE_SYSTEM_OUTLINE
        0x24 => LevelKind::Tree,        // ROLE_SYSTEM_OUTLINEITEM
        0x25 => LevelKind::Tab,         // ROLE_SYSTEM_PAGETAB
        0x28 => LevelKind::Image,       // ROLE_SYSTEM_GRAPHIC
        0x29 => LevelKind::Text,        // ROLE_SYSTEM_STATICTEXT
        0x2a => LevelKind::Text,        // ROLE_SYSTEM_TEXT
        0x2b => LevelKind::Button,      // ROLE_SYSTEM_PUSHBUTTON
        0x2c => LevelKind::CheckBox,    // ROLE_SYSTEM_CHECKBUTTON
        0x2d => LevelKind::RadioButton, // ROLE_SYSTEM_RADIOBUTTON
        0x2e | 0x2f => LevelKind::ComboBox, // ROLE_SYSTEM_COMBOBOX / DROPLIST
        0x30 => LevelKind::ProgressBar, // ROLE_SYSTEM_PROGRESSBAR
        0x33 => LevelKind::Slider,      // ROLE_SYSTEM_SLIDER
        _ => LevelKind::Unknown,
    }
}

/// The core of every refinement rule: the provider's box is used only when it is a strict
/// refinement of the walk's answer (non-empty, still under the cursor, strictly smaller).
pub fn is_finer_refinement(walk: Rect, hit: Rect, point: Point) -> bool {
    !hit.is_empty() && hit.contains(point) && hit.area() < walk.area()
}

/// Whether `child` is the glyph run *inside* the element it labels rather than a target of its own.
///
/// Measured on Chromium: `<a>Link Two</a>` is exposed as a 168x56 `Hyperlink` **with a 56x20
/// `Text` child**, and a `role=group` span as the group plus the text run inside it. Publishing the
/// run selects two words instead of the element the user is pointing at, which is why the browser
/// fixture marked those cases as failures (`docs/21 §5.5`).
///
/// Only a **framed container** (`Pane`/`Group`) may claim its text run.
///
/// Letting *interactive controls* claim theirs was measured to halve File Explorer's control-level
/// points (12/25 → 6/25): Explorer exposes a whole virtualised list as one `DataItem` whose text
/// child is far finer, so "the control owns its text" would answer with the container. A framed
/// `Pane`/`Group`, by contrast, is the element the user sees, which is what the `role=group` case in
/// the browser fixture needs (docs/21 §5.6).
pub fn is_text_run_inside_element(parent: WalkNode, child: WalkNode) -> bool {
    child.control_type == TEXT_CONTROL_TYPE
        && matches!(parent.control_type, PANE_CONTROL_TYPE | GROUP_CONTROL_TYPE)
}

/// Whether the provider's own point hit test should replace the walk's answer (docs/21 §5.7).
///
/// The accessibility provider answers a point query with the **innermost** element there by
/// construction, which makes it the yardstick for "did our walk stop above the innermost capturable
/// box?". Adopting it is a strict refinement — the caller only passes a hit that belongs to the same
/// window and still contains the point — so the rule is simply "adopt it when it is finer".
///
/// A bare `Text` run is not adopted: publishing the glyphs inside a control instead of the control is
/// a product decision (docs/21 §5.6 A), not a precision win. An equal or larger box is not adopted
/// either: the walk's answer already reflects stacking order and the backtracking rules, which a raw
/// hit box knows nothing about.
pub fn should_adopt_provider_box(
    walk: Rect,
    hit: Rect,
    hit_control_type: i32,
    point: Point,
    adopt_text_runs: bool,
) -> bool {
    is_finer_refinement(walk, hit, point)
        && (adopt_text_runs || hit_control_type != TEXT_CONTROL_TYPE)
}

/// The same rule for a box whose "is it a bare text run?" answer comes from an MSAA role.
///
/// MSAA and UIA are separate transports over the same page, and the candidate they offer is
/// compared against the walk's answer with the same rule (docs/21 §5.16).
pub fn should_adopt_msaa_box(
    walk: Rect,
    hit: Rect,
    hit_role: i32,
    parent_role: i32,
    point: Point,
    adopt_text_runs: bool,
) -> bool {
    if !is_finer_refinement(walk, hit, point) {
        return false;
    }
    if !is_bare_text_role(hit_role) {
        return true;
    }
    // A text run is a target only when the preference is on *and* it is not a control's label
    // (docs/21 §5.19): `parent_role` is the role of the box it sits in.
    adopt_text_runs && !is_interactive_control_role(parent_role)
}

/// Whether a control type / role describes a bare text run rather than a box.
///
/// Used by the probes to know whether a finer answer that was not adopted is a defect or the
/// deliberate exemption (docs/21 §5.18/§5.19).
pub fn is_bare_text_control_type(control_type: i32) -> bool {
    control_type == TEXT_CONTROL_TYPE
}

/// The MSAA spelling of [`is_bare_text_control_type`].
pub fn is_bare_text_role(role: i32) -> bool {
    matches!(role, MSAA_TEXT_ROLE | MSAA_STATIC_TEXT_ROLE)
}

/// Append `bounds` to a level chain, dropping the levels that do not contain it (docs/21 §5.17).
///
/// The published box can come from a point hit test, which is a different view of the page than the
/// walk that built the chain: the levels below the last one that contains the box are **not** its
/// ancestors, and leaving them in would break the containment invariant the ancestor walk depends on
/// (each level must contain the next, and the last one is the published box). Returns `false` when
/// the chain is already at [`MAX_PATH_LEN`].
pub fn push_box_keeping_containment(path: &mut Vec<PathLevel>, level: PathLevel) -> bool {
    let bounds = level.rect;
    if path.last().map(|last| last.rect) == Some(bounds) {
        return true;
    }
    while path.len() > 1 {
        let last = path.last().copied().expect("the loop keeps one level").rect;
        if last.contains_rect(bounds) {
            break;
        }
        path.pop();
    }
    if path.len() >= MAX_PATH_LEN {
        return false;
    }
    path.push(level);
    true
}

/// Result of a bounded walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkOutcome {
    /// Top-down path; `[0]` is the window frame.
    pub path: Vec<PathLevel>,
    /// The deepest element the walk accepted.
    pub target: Rect,
    pub stop_reason: StopReason,
}

impl WalkOutcome {
    /// A walk that could not get past the window frame.
    pub fn window_only(window_bounds: Rect, stop_reason: StopReason) -> Self {
        Self {
            path: vec![PathLevel::new(window_bounds, LevelKind::Window)],
            target: window_bounds,
            stop_reason,
        }
    }

    /// Push a deeper level, keeping the path bounded.
    ///
    /// Duplicate rectangles are not appended: a container that reports the same bounds as its parent
    /// would otherwise pad the path with entries the overlay draws on top of each other.
    pub fn push(&mut self, level: PathLevel) -> bool {
        if self.path.len() >= MAX_PATH_LEN {
            self.stop_reason = StopReason::TraversalLimit;
            return false;
        }
        if self.path.last().map(|last| last.rect) == Some(level.rect) {
            return true;
        }
        self.path.push(level);
        self.target = level.rect;
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
fn push_if_useful(path: &mut Vec<PathLevel>, level: PathLevel, point: Point) -> bool {
    if level.rect.is_empty() || !level.rect.contains(point) {
        return false;
    }
    if path.last().map(|last| last.rect) == Some(level.rect) {
        return false;
    }
    path.push(level);
    true
}

/// The provider-free hit path: every visible child window under the point, plus the frame.
///
/// Returned in the order they were collected; [`merge_hit_paths`] imposes the order.
///
/// The levels carry no kind: they are rectangles read off child **windows**, which say nothing about
/// what is inside them. `Unknown` is what keeps the label from calling a Chromium widget host a
/// `窗口` — the fallback is a geometric guess, and the words stay the generic ones.
pub fn fallback_hit_path(child_rects: &[Rect], window_bounds: Rect, point: Point) -> Vec<PathLevel> {
    let mut containing: Vec<PathLevel> = child_rects
        .iter()
        .copied()
        .filter(|rect| rect.contains(point))
        .map(PathLevel::unknown)
        .collect();
    push_if_useful(
        &mut containing,
        PathLevel::new(window_bounds, LevelKind::Window),
        point,
    );
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
    primary: &[PathLevel],
    fallback: &[PathLevel],
    window_bounds: Rect,
    point: Point,
) -> Vec<PathLevel> {
    // 1. The primary path, cleaned: usable entries only, frame first.
    let mut path: Vec<PathLevel> = primary
        .iter()
        .copied()
        .filter(|level| !level.rect.is_empty() && level.rect.contains(point))
        .collect();
    if path.is_empty() {
        path.push(PathLevel::new(window_bounds, LevelKind::Window));
    }
    if path.first().map(|first| first.rect) != Some(window_bounds) && window_bounds.contains(point) {
        path.insert(0, PathLevel::new(window_bounds, LevelKind::Window));
    }

    // 2. Extend downward with fallback rectangles that are strictly inside the current tail.
    let mut deeper: Vec<PathLevel> = fallback
        .iter()
        .copied()
        .filter(|level| !level.rect.is_empty() && level.rect.contains(point))
        .collect();
    deeper.sort_unstable_by_key(|level| rect_sort_key(level.rect));
    for candidate in deeper {
        let tail = path.last().expect("the path is never empty").rect;
        if !same_rect(candidate.rect, tail) && contains_rect(tail, candidate.rect) {
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

    /// A path level with no kind. These tests are about the walk's *policy* — descend, adopt,
    /// contain, merge, order — so plain rectangles dominate and the level wrapper is the rare form;
    /// the kind a provider attaches is tested where it is produced.
    fn lvl(left: i32, top: i32, right: i32, bottom: i32) -> PathLevel {
        PathLevel::unknown(rect(left, top, right, bottom))
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
    fn a_text_run_inside_an_element_is_not_a_target() {
        let text = rect(100, 100, 160, 120);
        let frame = rect(80, 80, 240, 180);
        let mut run = node(100, 100, 160, 120);
        run.control_type = TEXT_CONTROL_TYPE;
        let with = |control_type| WalkNode::new(frame, control_type, false, true);

        // Chromium exposes a `role=group` span as the group plus the text run inside it.
        for element in [GROUP_CONTROL_TYPE, PANE_CONTROL_TYPE] {
            assert!(
                is_text_run_inside_element(with(element), run),
                "{element} must claim its text run"
            );
        }
        // An interactive control does **not** claim its text: Explorer exposes a whole virtualised
        // list as one DataItem whose text child is far finer (measured 12/25 → 6/25 control-level
        // points when controls were allowed to claim it).
        for not_a_frame in [50000, 50005, 50004, 50007, 50029] {
            assert!(
                !is_text_run_inside_element(with(not_a_frame), run),
                "{not_a_frame} must not claim the text run"
            );
        }
        // Body text under the document is still the deepest truthful answer.
        for not_an_element in [50030 /* Document */, 50032 /* Window */, 0] {
            assert!(
                !is_text_run_inside_element(with(not_an_element), run),
                "{not_an_element} must not claim the text run"
            );
        }
        // A real child keeps its own box.
        assert!(!is_text_run_inside_element(
            with(50005),
            WalkNode::new(text, 50006, false, true)
        ));
    }

    #[test]
    fn the_providers_finer_box_is_adopted_unless_it_is_a_text_run() {
        let point = Point::new(150, 150);
        let walk = rect(100, 100, 400, 300);
        let finer = rect(120, 120, 260, 200);
        // A strictly finer box that still covers the cursor is a precision win.
        assert!(should_adopt_provider_box(walk, finer, 50033, point, false));
        // …unless it is the glyph run inside the element (a product decision, not precision).
        assert!(!should_adopt_provider_box(
            walk,
            finer,
            TEXT_CONTROL_TYPE,
            point,
            false
        ));
        // …and with the text-run preference on (docs/21 §5.19) the run is adopted like any other
        // finer box, which is what makes per-line snapping work.
        assert!(should_adopt_provider_box(
            walk,
            finer,
            TEXT_CONTROL_TYPE,
            point,
            true
        ));
        // An equal, larger or off-point answer must never replace the walk's own answer.
        assert!(!should_adopt_provider_box(walk, walk, 50033, point, true));
        assert!(!should_adopt_provider_box(
            walk,
            rect(50, 50, 900, 900),
            50033,
            point,
            true
        ));
        assert!(!should_adopt_provider_box(
            walk,
            rect(500, 500, 900, 900),
            50033,
            point,
            true
        ));
        // The MSAA transport answers the same question with a role instead of a control type.
        assert!(
            should_adopt_msaa_box(walk, finer, 0x14, 0x14, point, false),
            "a grouping"
        );
        assert!(
            !should_adopt_msaa_box(walk, finer, MSAA_STATIC_TEXT_ROLE, 0x14, point, false),
            "ROLE_SYSTEM_STATICTEXT is the text run inside the box"
        );
        assert!(!should_adopt_msaa_box(
            walk,
            finer,
            MSAA_TEXT_ROLE,
            0x14,
            point,
            false
        ));
        assert!(should_adopt_msaa_box(
            walk,
            finer,
            MSAA_STATIC_TEXT_ROLE,
            0x14,
            point,
            true
        ));
        // The label of a control is not a target of its own (docs/21 §5.19): pointing at a button
        // gives the button, while the same text run inside a plain container is adopted.
        assert!(
            !should_adopt_msaa_box(walk, finer, MSAA_STATIC_TEXT_ROLE, 0x2B, point, true),
            "ROLE_SYSTEM_PUSHBUTTON owns its label"
        );
        assert!(!should_adopt_msaa_box(
            walk,
            finer,
            MSAA_STATIC_TEXT_ROLE,
            0x22,
            point,
            true
        ));
        assert!(
            !should_adopt_msaa_box(walk, walk, 0x14, 0x14, point, true),
            "equal is not finer"
        );
        assert!(
            !should_adopt_msaa_box(walk, rect(500, 500, 900, 900), 0x14, 0x14, point, true),
            "off the cursor is not an answer for it"
        );
    }

    #[test]
    fn an_answer_that_covers_the_page_is_not_an_answer() {
        let window = rect(46, 20, 1834, 1234);
        // The page/web area: UIA's word for "I could not get below the document".
        assert!(is_unspecific_hit(rect(48, 107, 1832, 1232), DOCUMENT_CONTROL_TYPE, window));
        // A window-sized pane is the same statement in a different control type.
        assert!(is_unspecific_hit(rect(46, 20, 1834, 1234), 50033, window));
        // Anything meaningfully smaller is a real target, even if it is still a container.
        assert!(!is_unspecific_hit(rect(72, 211, 240, 267), 50000, window));
        assert!(!is_unspecific_hit(rect(1412, 907, 1624, 1027), 50026, window));
    }

    #[test]
    fn adopting_a_hit_keeps_the_chain_a_containment_chain() {
        let frame = lvl(0, 0, 1000, 800);
        // A walk that went in through a sibling branch: 400x300 does not contain the hit.
        let mut path = vec![frame, lvl(100, 100, 500, 400), lvl(300, 200, 420, 260)];
        assert!(push_box_keeping_containment(
            &mut path,
            lvl(600, 300, 700, 360)
        ));
        assert_eq!(
            path,
            vec![frame, lvl(600, 300, 700, 360)],
            "only the levels that contain the hit survive; the frame always does"
        );
        // A hit that the whole walk already contains keeps every level.
        let mut nested = vec![frame, lvl(100, 100, 500, 400)];
        assert!(push_box_keeping_containment(
            &mut nested,
            lvl(150, 150, 200, 200)
        ));
        assert_eq!(nested.len(), 3);
        // The same box twice is not a second level.
        assert!(push_box_keeping_containment(
            &mut nested,
            lvl(150, 150, 200, 200)
        ));
        assert_eq!(nested.len(), 3);
        // A full chain refuses rather than truncating.
        let mut full: Vec<PathLevel> = (0..MAX_PATH_LEN)
            .map(|step| lvl(0, 0, 1000 - step as i32, 800 - step as i32))
            .collect();
        assert!(!push_box_keeping_containment(&mut full, lvl(1, 1, 2, 2)));
        assert_eq!(full.len(), MAX_PATH_LEN);
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
        // The frame's level carries the one kind a walk can state without asking anyone (§5.24 B6).
        assert_eq!(outcome.path, vec![PathLevel::new(window, LevelKind::Window)]);
        assert!(outcome.push(lvl(100, 100, 900, 700)));
        assert!(outcome.push(lvl(300, 300, 500, 500)));
        assert_eq!(outcome.target, rect(300, 300, 500, 500));
        assert_eq!(outcome.path.len(), 3);

        // A container repeating its parent's bounds does not pad the path...
        assert!(outcome.push(lvl(300, 300, 500, 500)));
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
            assert!(outcome.push(lvl(offset, offset, 900 + offset, 900 + offset)));
        }
        assert_eq!(outcome.path.len(), MAX_PATH_LEN);
        assert!(!outcome.push(lvl(-1, -1, 899, 899)), "the path is full");
        assert_eq!(outcome.stop_reason, StopReason::TraversalLimit);
        assert!(!outcome.push(lvl(5000, 5000, 5100, 5100)));
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
        assert!(path.iter().any(|level| level.rect == rect(200, 100, 1000, 700)));
        assert!(
            path.iter().any(|level| level.rect == window),
            "the frame closes the path"
        );
        assert!(
            !path.iter().any(|level| level.rect == rect(0, 800, 100, 900)),
            "off-window children stay out"
        );
        // A child *window* is a rectangle, not a kind: the fallback may only refine geometry, so its
        // levels publish as unknown and the label keeps its generic words (§5.24 B6).
        assert!(
            path.iter().all(|level| level.kind == LevelKind::Unknown
                || level.kind == LevelKind::Window)
        );
        // A point on no child still yields the frame.
        let bare = fallback_hit_path(&children, window, Point::new(900, 750));
        assert_eq!(bare, vec![PathLevel::new(window, LevelKind::Window)]);
    }

    #[test]
    fn merging_extends_the_primary_path_downwards_and_never_coarsens() {
        let window = rect(0, 0, 1000, 800);
        let pane = lvl(100, 100, 900, 700);
        let control = lvl(300, 300, 500, 400);
        let finer = lvl(320, 320, 480, 380);
        let point = Point::new(400, 350);
        let frame = PathLevel::new(window, LevelKind::Window);

        // The accessibility path runs frame → pane → control; the fallback knows something
        // deeper, so it is appended and the published rectangle gets finer.
        let merged = merge_hit_paths(&[frame, pane, control], &[finer], window, point);
        assert_eq!(merged, vec![frame, pane, control, finer]);
        assert_eq!(merged.last(), Some(&finer), "the deepest entry is published");

        // A *coarser* fallback rectangle must never replace the fine control — that was the
        // regression that made every publish the whole window.
        let unchanged = merge_hit_paths(&[frame, pane, control], &[frame], window, point);
        assert_eq!(unchanged, vec![frame, pane, control]);
        assert_eq!(unchanged.last(), Some(&control));

        // With neither, the path is just the frame.
        let bare = merge_hit_paths(&[], &[], window, point);
        assert_eq!(bare, vec![frame]);
    }

    #[test]
    fn merging_deduplicates_and_keeps_a_nested_order() {
        let window = rect(0, 0, 1000, 800);
        let outer = lvl(100, 100, 900, 700);
        let inner = lvl(300, 300, 500, 400);
        let deepest = lvl(350, 330, 450, 370);
        let point = Point::new(400, 350);
        let frame = PathLevel::new(window, LevelKind::Window);
        // Same rectangles from both sources, in different orders.
        let merged = merge_hit_paths(
            &[outer, inner],
            &[deepest, inner, outer],
            window,
            point,
        );
        let unique: Vec<PathLevel> = {
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
            Some(&frame),
            "the path is published frame-first"
        );
        assert_eq!(
            merged.last(),
            Some(&deepest),
            "the deepest entry ends the path, so `target = last()` is the control"
        );
        // Each level is contained by the one before it: a nested, outermost-first path.
        for pair in merged.windows(2) {
            let (outer, inner) = (pair[0].rect, pair[1].rect);
            assert!(
                !outer.is_empty() && outer != inner,
                "levels must differ: {merged:?}"
            );
            assert!(
                outer.left <= inner.left
                    && outer.top <= inner.top
                    && outer.right >= inner.right
                    && outer.bottom >= inner.bottom,
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
        let seed = lvl(300, 300, 500, 400);
        let sibling = lvl(700, 600, 900, 780);
        let merged = merge_hit_paths(&[seed], &[sibling], window, Point::new(400, 350));
        // Frame first (the documented order), then the deepest entry that was published.
        assert_eq!(
            merged,
            vec![PathLevel::new(window, LevelKind::Window), seed]
        );
        assert_eq!(merged.last(), Some(&seed), "the sibling never becomes a level");
    }

}
