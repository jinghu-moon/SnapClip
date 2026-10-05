//! Windows UI Automation deep-selection provider (docs/18 §4).
//!
//! Everything COM-related lives inside this file and runs on the refinement worker thread:
//! the automation object is created lazily on first use (never on the overlay thread, which
//! must not own apartment-bound objects) and only plain geometry crosses back through
//! [`DeepTarget`].
//!
//! A window that fails to produce a tree is **quarantined** until `release()`: retrying a
//! provider that has already hung is how a selector turns into a freeze, and the design
//! budgets for exactly that (docs/18 §3).

use std::collections::{HashMap, HashSet};

use ::windows::Win32::Foundation::{HWND, RECT};
use ::windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use ::windows::core::Interface;
use ::windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement, TreeScope_Children,
    UIA_BoundingRectanglePropertyId, UIA_ControlTypePropertyId, UIA_IsOffscreenPropertyId,
    UIA_NativeWindowHandlePropertyId,
};

use crate::capture::geometry::{Point, Rect};
use crate::capture::diagnostics::WindowDetectionMetrics;
use crate::capture::window_detection::deep::{
    DeepSelectionProvider, DeepTarget, QueryControl, RefinementJob, RefinementOutcome,
    StopReason,
};
use crate::capture::window_detection::model::{SnapshotEpoch, TargetKind};
use crate::capture::window_detection::uia::{
    WalkBudget, WalkNode, WalkOutcome, fallback_hit_path, is_descendable, is_structural_wrapper,
    merge_hit_paths,
};

use super::win::window as win32;

/// UIA provider: resolves the deepest element inside one window.
pub struct UiaDeepSelectionProvider {
    automation: Option<IUIAutomation>,
    /// Batched property request: one cross-process call per level instead of four per node
    /// (docs/18 §11). Built lazily on the refinement thread.
    cache: Option<IUIAutomationCacheRequest>,
    /// Expanded children per (window, parent bounds), with the forensic counters of the
    /// level they came from.
    ///
    /// Moving between controls of one window walks the same upper levels over and over;
    /// remembering them turns a repeat query into "fetch the deepest level only" instead of
    /// "walk from the window root again" (docs/18 §11 ②). The whole table is dropped when the
    /// snapshot generation changes, so no stale geometry can be served.
    children: HashMap<NodeKey, ExpandedLevel>,
    /// Snapshot generation the table belongs to.
    cache_epoch: Option<SnapshotEpoch>,
    /// Provider-free child-window rectangles per window, valid for `cache_epoch`.
    fallback_rects: HashMap<isize, Vec<Rect>>,
    /// Windows whose provider failed; skipped until the next snapshot generation.
    quarantined: HashSet<isize>,
    /// Diagnostics sink (same verbose gate as the rest of window detection).
    metrics: WindowDetectionMetrics,
    /// Level counter for one query, so the forensics can tell a same-bounds *chain* apart from
    /// a walk that is spinning on one node (docs/18 §12.7).
    level: u32,
}

/// How many children a level offered and why the others were dropped.
///
/// This is the evidence that decides whether resource-manager file items are missing
/// because the provider never exposes them, because they report **empty rectangles**
/// (virtualised DirectUI items do), or because they are simply outside the point
/// (docs/18 §12.7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LevelStats {
    /// Children the provider returned for this level.
    pub raw: usize,
    /// Dropped because their bounding rectangle was empty or unreadable.
    pub empty: usize,
    /// Reported as off-screen.
    pub offscreen: usize,
    /// Contained the cursor.
    pub containing: usize,
}

/// Identity of one expanded node: the element's own COM identity.
///
/// **Not its bounds.** The forensic log proved UIA hands back chains of *distinct* nodes with
/// **identical rectangles** (Explorer's content panel reports exactly one child occupying the
/// same rectangle as itself, and so on down). Keying the expansion cache by bounds made those
/// nodes share one entry, so the walk kept picking the same child — descending "into itself"
/// until the depth budget ran out and publishing the coarse panel. The reference selector keys
/// its cache by node index for the same reason. The pointer is a valid identity while the cache
/// holds a reference to the element, which it does.
type NodeKey = usize;

/// One expanded node: the node itself, its children and the forensic counters of the level.
///
/// The entry **owns the parent element**. A [`NodeKey`] is only an identity while that element
/// is alive; the walk's root comes fresh from `ElementFromHandle` and nothing else holds it, so
/// once the walk stepped below it the allocator could hand its address to a newly read child.
/// Measured on a maximized Edge window: that child was then filtered out as "already visited"
/// (or served another node's children from this table), and the same point resolved to the
/// control, the page or the whole window on consecutive queries.
struct ExpandedLevel {
    _parent: IUIAutomationElement,
    children: Vec<(IUIAutomationElement, WalkNode)>,
    stats: LevelStats,
}

impl UiaDeepSelectionProvider {
    pub fn new(metrics: WindowDetectionMetrics) -> Self {
        Self {
            automation: None,
            cache: None,
            children: HashMap::new(),
            cache_epoch: None,
            fallback_rects: HashMap::new(),
            quarantined: HashSet::new(),
            metrics,
            level: 0,
        }
    }

    /// The automation object, created on the worker thread at first use.
    ///
    /// `CoInitializeEx` is allowed to fail with `RPC_E_CHANGED_MODE` when another library
    /// already chose an apartment model for this thread; COM is initialised either way, so
    /// the failure is ignored and only `CoCreateInstance` decides whether UIA is usable.
    fn automation(&mut self) -> Option<&IUIAutomation> {
        if self.automation.is_none() {
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            self.automation =
                unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.ok();
        }
        self.automation.as_ref()
    }

    /// The batched property request, built once on the refinement thread.
    ///
    /// `TreeScope_Children` is what makes this a *batch*: fetching an element with this
    /// request also populates its direct children, so probing a level costs two
    /// cross-process calls instead of four per node (docs/18 §11).
    fn cache_request(&mut self) -> Option<&IUIAutomationCacheRequest> {
        if self.cache.is_none() {
            let automation = self.automation()?.clone();
            let request = unsafe { automation.CreateCacheRequest() }.ok()?;
            let configured = unsafe {
                request
                    .AddProperty(UIA_BoundingRectanglePropertyId)
                    .and_then(|()| request.AddProperty(UIA_ControlTypePropertyId))
                    .and_then(|()| request.AddProperty(UIA_IsOffscreenPropertyId))
                    .and_then(|()| request.AddProperty(UIA_NativeWindowHandlePropertyId))
                    .and_then(|()| request.SetTreeScope(TreeScope_Children))
            };
            if configured.is_err() {
                return None;
            }
            self.cache = Some(request);
        }
        self.cache.as_ref()
    }

    /// Reduce one element to the data the traversal policy needs, reading the values that
    /// `BuildUpdatedCache` already fetched for it.
    fn cached_node(element: &IUIAutomationElement) -> Option<WalkNode> {
        let bounds = unsafe { element.CachedBoundingRectangle() }.ok()?;
        let bounds = to_rect(bounds);
        if bounds.is_empty() {
            return None;
        }
        let control_type = unsafe { element.CachedControlType() }
            .map(|kind| kind.0)
            .unwrap_or(0);
        let offscreen = unsafe { element.CachedIsOffscreen() }
            .map(|value| value.as_bool())
            .unwrap_or(false);
        let native_window = unsafe { element.CachedNativeWindowHandle() }
            .map(|handle| handle.0 as isize)
            .unwrap_or(0);
        Some(WalkNode::new(bounds, control_type, offscreen, true).with_native_window(native_window))
    }

    /// Drop the expanded-children table when the snapshot generation moved on.
    fn sync_cache_epoch(&mut self, epoch: SnapshotEpoch) {
        if self.cache_epoch != Some(epoch) {
            self.children.clear();
            self.fallback_rects.clear();
            self.cache_epoch = Some(epoch);
        }
    }

    /// Fetch one node's direct children, with their properties already populated.
    ///
    /// One `BuildUpdatedCache` + one `GetCachedChildren` probe the whole level; the per-child
    /// geometry reads are then in-process (docs/18 §11 ①).
    fn expand(
        request: &IUIAutomationCacheRequest,
        parent: &IUIAutomationElement,
    ) -> (Vec<(IUIAutomationElement, WalkNode)>, LevelStats) {
        let mut stats = LevelStats::default();
        let Ok(parent) = (unsafe { parent.BuildUpdatedCache(request) }) else {
            return (Vec::new(), stats);
        };
        let Ok(children) = (unsafe { parent.GetCachedChildren() }) else {
            return (Vec::new(), stats);
        };
        let count = unsafe { children.Length() }.ok().unwrap_or(0).max(0) as usize;
        let mut expanded = Vec::with_capacity(count);
        for index in 0..count {
            stats.raw += 1;
            let Ok(child) = (unsafe { children.GetElement(index as i32) }) else {
                continue;
            };
            let Some(node) = Self::cached_node(&child) else {
                stats.empty += 1;
                continue;
            };
            if node.offscreen {
                stats.offscreen += 1;
            }
            expanded.push((child, node));
        }
        (expanded, stats)
    }

    /// Every descendable child of a node that contains `point`, topmost (last listed by the provider) first.
    ///
    /// Returning *all* containing candidates (not just the first) is what makes structural
    /// backtracking possible: when the first one dead-ends, the walk must be able to come
    /// back and try the next branch (docs/18 §12.6 ①). Children are read from the per-window
    /// table when that level was already expanded, so moving between controls of one window
    /// only pays for the levels it has not seen yet.
    fn containing_children(
        &mut self,
        hwnd: isize,
        request: &IUIAutomationCacheRequest,
        parent: &IUIAutomationElement,
        parent_bounds: Rect,
        point: Point,
        budget: &mut WalkBudget,
    ) -> Vec<(IUIAutomationElement, WalkNode)> {
        // The element's own identity, never its rectangle: two nodes may share a rectangle.
        let _ = hwnd;
        let key: NodeKey = parent.as_raw() as usize;
        let level = self.children.entry(key).or_insert_with(|| {
            let (children, stats) = Self::expand(request, parent);
            ExpandedLevel {
                _parent: parent.clone(),
                children,
                stats,
            }
        });
        let stats = level.stats;
        let children = &level.children;
        let mut candidates: Vec<(IUIAutomationElement, WalkNode)> = Vec::new();
        for (child, node) in children {
            if !budget.take_node() {
                break;
            }
            if !is_descendable(parent_bounds, *node) || !node.bounds.contains(point) {
                continue;
            }
            // `IsOffscreen` does not cover a child window hidden *behind a sibling window*:
            // File Explorer keeps every tab as a full-size, visible `ShellTabWindowClass` and
            // lists the hidden tabs after the active one, so the topmost-listed rule walked into
            // a background tab whose items are laid out differently (measured: 9 same-bounds tab
            // panes; rows of the visible list resolved to the whole content area). Only the
            // window manager knows which sibling is on screen at the point.
            if node.native_window != 0 && !win32::is_shown_child_at(node.native_window, point) {
                continue;
            }
            candidates.push((child.clone(), *node));
        }
        // The reference selector's order (`uia/cache.rs`: `children.hit_before(point, usize::MAX)`):
        // the child the provider lists **last** wins, i.e. the one painted on top. Ordering by area
        // instead ("smallest wins") descended into a childless dead leaf at a Chromium window root,
        // where two overlapping panes contain the cursor and the *smaller* one has no children —
        // measured: a browser page scored 3/23 fixtures with the area order and 21/23 with this one,
        // while a real File Explorer window also improved (3/25 → 12/25 points reached control level).
        candidates.reverse();
        self.level += 1;
        self.metrics.log_line(
            &format!(
                "refinement level #{} node={:#x} parent=({},{})->({},{}) raw={} empty={} offscreen={} containing={}",
                self.level,
                parent.as_raw() as usize,
                parent_bounds.left,
                parent_bounds.top,
                parent_bounds.right,
                parent_bounds.bottom,
                stats.raw,
                stats.empty,
                stats.offscreen,
                candidates.len()
            ),
            false,
        );
        candidates
    }

    /// Number of expanded levels held for the current generation (diagnostics and tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn cached_levels(&self) -> usize {
        self.children.len()
    }

    /// Snapshot generation the expanded levels belong to (diagnostics and tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn cached_epoch(&self) -> Option<SnapshotEpoch> {
        self.cache_epoch
    }

    /// Visible child-window rectangles of one window, cached for this generation.
    ///
    /// The enumeration is a cheap user-mode call, but it is still per-window work that has
    /// no reason to repeat while the user moves inside the same window.
    fn child_rects(&mut self, hwnd: isize, window_bounds: Rect) -> Vec<Rect> {
        if let Some(cached) = self.fallback_rects.get(&hwnd) {
            return cached.clone();
        }
        let rects = win32::visible_child_rects(hwnd, window_bounds);
        self.fallback_rects.insert(hwnd, rects.clone());
        rects
    }
}

impl DeepSelectionProvider for UiaDeepSelectionProvider {
    fn resolve(
        &mut self,
        job: &RefinementJob,
        window_bounds: Rect,
        control: &QueryControl<'_>,
    ) -> RefinementOutcome {
        if self.quarantined.contains(&job.window.hwnd) {
            self.metrics.record_refinement_quarantine_hit();
            return RefinementOutcome::Empty(StopReason::Unsupported);
        }
        // Clone the interface pointer so the immutable borrow of `self` ends before any
        // quarantine bookkeeping below.
        let Some(automation) = self.automation().cloned() else {
            self.quarantined.insert(job.window.hwnd);
            self.metrics.record_refinement_quarantine_added();
            return RefinementOutcome::Empty(StopReason::Unsupported);
        };

        let window = HWND(job.window.hwnd as *mut core::ffi::c_void);
        // The query is *about this window*: if UIA cannot resolve the handle there is nothing
        // truthful to publish. Falling back to a point hit test here would answer for
        // whichever window happens to be under the cursor, i.e. invent geometry for the
        // wrong window, so the failure is a quarantine instead.
        let root = match unsafe { automation.ElementFromHandle(window) } {
            Ok(element) => element,
            Err(_) => {
                self.quarantined.insert(job.window.hwnd);
                self.metrics.record_refinement_quarantine_added();
                return RefinementOutcome::Empty(StopReason::Unsupported);
            }
        };

        let mut outcome = WalkOutcome::window_only(window_bounds, StopReason::Complete);
        // Without a batched cache request we can still answer with the window frame.
        let Some(request) = self.cache_request().cloned() else {
            return RefinementOutcome::Target(Box::new(finish(outcome, job)));
        };
        // Expanded levels are only valid for the snapshot generation they were read in.
        self.sync_cache_epoch(job.epoch);
        // One level counter per query: the forensics must be readable per walk.
        self.level = 0;

        // Bounded depth-first descent with structural backtracking (docs/18 §11 ①/⑤, docs/20
        // §5.2). Each level takes the topmost containing child first; when a branch dead-ends,
        // the walk may back out of it only through pure wrappers (`is_structural_wrapper`) and
        // try the next earlier sibling that contains the point. Measured on a maximized Edge
        // window: the root's content pane holds two window-sized panes, the page under the
        // *first* and a childless one listed *last*, so without this the walk ended on the
        // wrapper and published the whole window. (An earlier backtracking attempt was
        // reverted because of a scheduler cache rule that has since been removed.)
        let mut budget = WalkBudget::new();
        let mut current = root;
        let mut current_bounds = window_bounds;
        // The accepted path below the window frame; each entry keeps the siblings not tried yet.
        let mut stack: Vec<Level> = Vec::new();
        // UIA can hand back cycles between same-bounds nodes (A → B → A). The forensic log
        // showed one such pair alternating until the depth budget ran out, which padded the
        // published path with duplicate levels. Visiting each node at most once bounds the
        // walk by the tree itself instead of by the budget.
        let mut visited: HashSet<usize> = HashSet::new();
        visited.insert(current.as_raw() as usize);
        'walk: loop {
            if control.is_cancelled() {
                outcome.stop_reason = StopReason::Cancelled;
                break;
            }
            // The declared total budget, checked where it can be honoured: between levels. A
            // single COM call below is uninterruptible, so the walk that keeps making progress
            // is bounded here and the one that never returns is covered by the scheduler's
            // in-flight timeout (docs/18 §3).
            if control.budget_exhausted() {
                outcome.stop_reason = StopReason::BudgetExhausted;
                break;
            }
            if !budget.enter_children() {
                outcome.stop_reason = StopReason::TraversalLimit;
                break;
            }
            // Cycle guard: never step into a node already on this walk. A genuine same-bounds
            // *child* is still allowed (that is how Explorer nests its panes), but the walk must
            // make progress.
            let mut candidates: Vec<(IUIAutomationElement, WalkNode)> = self
                .containing_children(
                    job.window.hwnd,
                    &request,
                    &current,
                    current_bounds,
                    job.point,
                    &mut budget,
                )
                .into_iter()
                .filter(|(child, _)| !visited.contains(&(child.as_raw() as usize)))
                .collect();
            if !candidates.is_empty() {
                let (child, node) = candidates.remove(0);
                visited.insert(child.as_raw() as usize);
                // Remaining siblings are popped from the back, so store them earliest-tried last.
                candidates.reverse();
                stack.push(Level {
                    node,
                    parent_bounds: current_bounds,
                    untried: candidates,
                });
                current = child;
                current_bounds = node.bounds;
                continue;
            }

            // Dead end: `current` has no containing child. The level entered for it is unused.
            budget.leave_children();
            while let Some(depth) = stack.len().checked_sub(1) {
                let level = &mut stack[depth];
                if !is_structural_wrapper(level.parent_bounds, level.node) {
                    break 'walk;
                }
                if let Some((sibling, node)) = level.untried.pop() {
                    self.metrics.log_line(
                        &format!(
                            "refinement backtrack depth={} from=({},{})->({},{}) to=({},{})->({},{})",
                            depth + 1,
                            level.node.bounds.left,
                            level.node.bounds.top,
                            level.node.bounds.right,
                            level.node.bounds.bottom,
                            node.bounds.left,
                            node.bounds.top,
                            node.bounds.right,
                            node.bounds.bottom
                        ),
                        false,
                    );
                    visited.insert(sibling.as_raw() as usize);
                    level.node = node;
                    current = sibling;
                    current_bounds = node.bounds;
                    continue 'walk;
                }
                // No sibling left here: climb out of this wrapper too, while it is one.
                stack.pop();
                budget.leave_children();
            }
            break;
        }
        for level in &stack {
            if !outcome.push(level.node.bounds) {
                break;
            }
        }

        // Provider-free fallback (docs/18 §12.2): older and custom-drawn controls never show
        // up in the accessibility tree, but their child windows do. Merging both sources is
        // what turns "a big content pane" into "the control the user is pointing at".
        let fallback = fallback_hit_path(
            &self.child_rects(job.window.hwnd, window_bounds),
            window_bounds,
            job.point,
        );
        let merged = merge_hit_paths(&outcome.path, &fallback, window_bounds, job.point);
        outcome.path = merged;
        outcome.target = outcome.path.last().copied().unwrap_or(window_bounds);
        RefinementOutcome::Target(Box::new(finish(outcome, job)))
    }

    fn release(&mut self) {
        // Quarantine lasts exactly one snapshot generation; a rebuilt snapshot retries.
        self.quarantined.clear();
        // Cached geometry must not survive into a new generation either.
        self.children.clear();
        self.fallback_rects.clear();
        self.cache_epoch = None;
    }
}

/// One accepted level of the walk: the node chosen there and the containing siblings that
/// remain to be tried if the branch under it dead-ends.
struct Level {
    node: WalkNode,
    parent_bounds: Rect,
    untried: Vec<(IUIAutomationElement, WalkNode)>,
}

/// Turn a bounded walk into the published target.
fn finish(outcome: WalkOutcome, job: &RefinementJob) -> DeepTarget {
    // `ClientArea`/`UiElement` only when the walk actually got below the window frame; a
    // window-only answer keeps saying "whole frame" so the overlay renders it exactly like a
    // v1 target.
    let kind = if outcome.path.len() > 1 {
        TargetKind::UiElement
    } else {
        TargetKind::TopLevelWindowFrame
    };
    DeepTarget {
        window: job.window,
        kind,
        screen_bounds: outcome.target,
        path: outcome.path,
        stop_reason: outcome.stop_reason,
    }
}

fn to_rect(rect: RECT) -> Rect {
    Rect::new(rect.left, rect.top, rect.right, rect.bottom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::windows::Win32::UI::WindowsAndMessaging::{
        DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, SW_SHOWNA, ShowWindow,
        TranslateMessage, WS_POPUP, WS_VISIBLE, CreateWindowExW, WINDOW_EX_STYLE,
    };
    use ::windows::core::w;
    use std::time::Duration;

    fn job(hwnd: isize, point: Point) -> RefinementJob {
        RefinementJob {
            request: crate::capture::window_detection::model::RequestGate::new().issue(),
            window: crate::capture::window_detection::model::WindowIdentity::new(hwnd, 1, 2),
            epoch: 1,
            point,
        }
    }

    fn pump(millis: u64) {
        let deadline = std::time::Instant::now() + Duration::from_millis(millis);
        let mut message = MSG::default();
        while std::time::Instant::now() < deadline {
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                let _ = unsafe { TranslateMessage(&message) };
                unsafe { DispatchMessageW(&message) };
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Resolve `point` in `hwnd`, waiting out the fixture's registration with the system.
    ///
    /// A window that was just created is registered with the compositor **and** with the UIA
    /// core provider asynchronously, so a query landing in the first milliseconds can be told
    /// the handle is unresolvable. That answer is indistinguishable from "this window has no
    /// tree", and it made these fixture tests flaky (measured once in ~16 suite runs, right
    /// after a cold rebuild) — the fixed 60 ms pump was a weaker version of this same wait.
    ///
    /// Retrying does not weaken any assertion: a genuine regression still fails, after the
    /// deadline, on exactly the assertion it failed on before. The quarantine a failed attempt
    /// leaves behind is released between attempts, because a cold start is not the window's
    /// fault.
    fn resolve_when_ready(
        provider: &mut UiaDeepSelectionProvider,
        hwnd: isize,
        point: Point,
        bounds: Rect,
    ) -> RefinementOutcome {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let control = QueryControl::refinement(&|| false);
            let outcome = provider.resolve(&job(hwnd, point), bounds, &control);
            if !matches!(outcome, RefinementOutcome::Empty(StopReason::Unsupported))
                || std::time::Instant::now() >= deadline
            {
                return outcome;
            }
            provider.release();
            pump(25);
        }
    }

    /// A real top-level window owned by this test process, so UIA has a tree to walk.
    struct FixtureWindow(HWND);

    impl FixtureWindow {
        fn create() -> Option<Self> {
            let window = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!("SnapClip uia fixture"),
                    WS_POPUP | WS_VISIBLE,
                    140,
                    140,
                    360,
                    260,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .ok()?;
            let _ = unsafe { ShowWindow(window, SW_SHOWNA) };
            pump(60);
            Some(Self(window))
        }

        fn handle(&self) -> isize {
            self.0 .0 as isize
        }
    }

    impl Drop for FixtureWindow {
        fn drop(&mut self) {
            let _ = unsafe { DestroyWindow(self.0) };
            pump(20);
        }
    }

    #[test]
    fn an_unknown_window_is_quarantined_and_never_invents_geometry() {
        let mut provider = UiaDeepSelectionProvider::new(WindowDetectionMetrics::new());
        let control = QueryControl::refinement(&|| false);
        // A fabricated handle: UIA cannot resolve it.
        let outcome = provider.resolve(
            &job(0xDEAD_BEEF, Point::new(10, 10)),
            Rect::new(0, 0, 100, 100),
            &control,
        );
        assert_eq!(outcome, RefinementOutcome::Empty(StopReason::Unsupported));
        // Second attempt hits the quarantine and returns immediately.
        assert_eq!(
            provider.resolve(
                &job(0xDEAD_BEEF, Point::new(10, 10)),
                Rect::new(0, 0, 100, 100),
                &control
            ),
            RefinementOutcome::Empty(StopReason::Unsupported)
        );
        // A new snapshot generation retries.
        provider.release();
        assert!(provider.quarantined.is_empty());
    }

    #[test]
    fn a_cancelled_query_stops_before_touching_the_tree() {
        let mut provider = UiaDeepSelectionProvider::new(WindowDetectionMetrics::new());
        let control = QueryControl::refinement(&|| true);
        // Cancellation is checked before the first descent, so an already-abandoned query
        // must not walk (and must not publish a deep target).
        match provider.resolve(
            &job(0xDEAD_BEEF, Point::new(10, 10)),
            Rect::new(0, 0, 100, 100),
            &control,
        ) {
            RefinementOutcome::Empty(_) => {}
            RefinementOutcome::Target(target) => {
                assert_eq!(target.stop_reason, StopReason::Cancelled);
            }
        }
    }

    #[test]
    fn the_real_pipeline_still_answers_in_the_second_capture_session() {
        // End-to-end over the real accessibility stack: scheduler → refinement worker → UIA.
        // Two sessions run back to back on **one** worker, exactly as two F5 presses do. The
        // scheduler resets its id space between them; the worker must adopt the new ids, or the
        // second session's answer is rejected as stale and deep selection dies silently
        // (docs/18 §12.14).
        use crate::capture::window_detection::deep::RefinementScheduler;
        use crate::capture::window_detection::model::WindowIdentity;
        use crate::platform::windows::capture::refinement_worker::RefinementWorker;

        let Some(fixture) = FixtureWindow::create() else {
            eprintln!("skipping: no interactive window station available");
            return;
        };
        let bounds = Rect::new(200, 200, 560, 460);
        let point = Point::new(300, 300);
        let identity = WindowIdentity::new(fixture.handle(), std::process::id(), 0x5E7);
        let metrics = WindowDetectionMetrics::new();
        // Wait until the fixture is registered with UIA *before* the pipeline runs: the point of
        // this test is the id plumbing between sessions, so a cold-start "no tree yet" answer
        // must not be mistaken for it.
        {
            let mut warmup = UiaDeepSelectionProvider::new(WindowDetectionMetrics::new());
            let outcome = resolve_when_ready(&mut warmup, fixture.handle(), point, bounds);
            assert!(
                matches!(outcome, RefinementOutcome::Target(_)),
                "the fixture must be resolvable before the pipeline is exercised, got {outcome:?}"
            );
        }
        let worker = RefinementWorker::new(0, metrics);
        let mut scheduler = RefinementScheduler::new();

        for (session, epoch) in [(1u32, 1u64), (2, 2)] {
            let actions = scheduler.on_cursor_moved(epoch, Some(identity), point);
            assert!(actions.arm_dwell);
            let job = scheduler
                .on_dwell_due()
                .unwrap_or_else(|| panic!("session {session} must issue a query"));
            worker.request(job, bounds);

            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut accepted = false;
            while std::time::Instant::now() < deadline {
                // The fixture window lives on *this* thread, so it has to keep pumping while the
                // refinement worker calls into it — exactly what the overlay does while it waits.
                pump(10);
                let Some(result) = worker.take_result() else {
                    continue;
                };
                if result.request != job.request {
                    continue;
                }
                let RefinementOutcome::Target(target) = result.outcome else {
                    panic!("session {session} produced no target: {:?}", result.outcome);
                };
                assert_eq!(target.window, identity);
                assert!(!target.screen_bounds.is_empty());
                accepted = scheduler.on_result(result.request, result.epoch, *target);
                break;
            }
            assert!(
                accepted,
                "session {session}'s answer must still be the current question"
            );
            assert!(scheduler.cached().is_some());

            // Session teardown, then the next F5: both sides start over.
            if session == 1 {
                scheduler.reset();
                worker.retire();
            }
        }
    }

    #[test]
    fn a_real_window_yields_a_path_that_starts_at_the_window_frame() {
        let Some(fixture) = FixtureWindow::create() else {
            eprintln!("skipping: no interactive window station available");
            return;
        };
        let mut provider = UiaDeepSelectionProvider::new(WindowDetectionMetrics::new());
        if provider.automation().is_none() {
            eprintln!("skipping: UI Automation is unavailable in this environment");
            return;
        }
        let bounds = Rect::new(200, 200, 560, 460);
        let outcome = resolve_when_ready(&mut provider, fixture.handle(), Point::new(280, 280), bounds);
        let RefinementOutcome::Target(target) = outcome else {
            panic!("a live window must produce a target, got {outcome:?}");
        };
        assert_eq!(target.window.hwnd, fixture.handle());
        assert_eq!(target.path[0], bounds, "the path always starts at the window frame");
        assert_eq!(target.path.last(), Some(&target.screen_bounds));
        assert!(!target.screen_bounds.is_empty());
        // The published rectangle must be inside the window's visible area.
        assert!(!target.screen_bounds.intersect(bounds).is_empty());
    }

    #[test]
    fn expanded_levels_are_reused_within_a_generation_and_dropped_across_them() {
        let Some(fixture) = FixtureWindow::create() else {
            eprintln!("skipping: no interactive window station available");
            return;
        };
        let mut provider = UiaDeepSelectionProvider::new(WindowDetectionMetrics::new());
        if provider.automation().is_none() {
            eprintln!("skipping: UI Automation is unavailable in this environment");
            return;
        }
        let bounds = Rect::new(200, 200, 560, 460);
        let control = QueryControl::refinement(&|| false);

        let _ = resolve_when_ready(&mut provider, fixture.handle(), Point::new(280, 280), bounds);
        let expanded = provider.cached_levels();
        assert_eq!(provider.cached_epoch(), Some(1));
        if expanded == 0 {
            // A window whose tree UIA exposes nothing has no level to cache; the reuse rule
            // then simply has nothing to do.
            return;
        }

        // Same generation, another control: the already-walked upper levels are reused, so
        // the table does not grow by re-fetching them.
        let _ = provider.resolve(&job(fixture.handle(), Point::new(300, 300)), bounds, &control);
        assert_eq!(
            provider.cached_epoch(),
            Some(1),
            "the same generation keeps its table"
        );
        assert!(
            provider.cached_levels() >= expanded,
            "a second query may add levels but never loses the cached ones"
        );

        // A rebuilt snapshot invalidates the table before anything else happens.
        let mut next = job(fixture.handle(), Point::new(280, 280));
        next.epoch = 2;
        let _ = provider.resolve(&next, bounds, &control);
        assert_eq!(
            provider.cached_epoch(),
            Some(2),
            "the table is rebuilt for the new generation"
        );
        assert!(
            provider.cached_levels() <= expanded,
            "levels from the previous generation cannot survive"
        );
    }
    /// One fixture from the demo page's manifest.
    #[derive(Debug, serde::Deserialize)]
    struct Fixture {
        id: String,
        /// Layout input for the absolutely positioned fixtures; generated children are measured
        /// by the page instead.
        #[serde(default)]
        rect: [i32; 4],
        #[serde(default)]
        role: String,
        #[serde(default)]
        name: String,
        /// `"self"`, another fixture id, or `"none"` (see the fixture file's header).
        expect: String,
        /// Offset inside the fixture's own box; defaults to its centre.
        #[serde(default)]
        probe: Option<[i32; 2]>,
        #[serde(default)]
        optional: bool,
    }

    /// What a correct deep selection must publish in a real web page.
    ///
    /// Run explicitly — it launches its own Chromium on a throwaway profile over the demo page in
    /// `tests/fixtures/browser-element-demo.html`, so the user's browser state is untouched:
    ///
    /// ```text
    /// cargo test --lib browser_element_probe -- --ignored --nocapture
    /// ```
    ///
    /// The page publishes its own measured boxes in the window title (`?truth=1`), so the
    /// expectations cannot drift from the CSS: every row of the report compares our published
    /// rectangle against the browser's own measurement, and against the raw accessibility chain
    /// at that point. `optional` rows are printed but not asserted (rotated boxes, canvas, svg,
    /// iframe, shadow DOM, layout-only wrappers). It is deliberately **not** part of the default
    /// suite: it needs a browser and a visible window.
    #[test]
    #[ignore = "launches a browser; run explicitly with --ignored --nocapture"]
    fn browser_element_probe() {
        let Some(browser) = find_chromium() else {
            eprintln!("skipping: no Chromium-based browser found");
            return;
        };
        // A test process is DPI-unaware by default, so user32 virtualises `GetClientRect` to
        // logical pixels while DWM keeps reporting physical ones — the two disagreed by the
        // display scale and every computed probe point was wrong. The app does exactly this on
        // its overlay thread at startup.
        let _ = crate::platform::windows::capture::monitor::set_per_monitor_v2_awareness();
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/browser-element-demo.html");
        let Ok(source) = std::fs::read_to_string(&fixture_path) else {
            eprintln!("skipping: cannot read {}", fixture_path.display());
            return;
        };
        let Some(manifest) = manifest_of(&source) else {
            eprintln!("skipping: the fixture has no parsable manifest");
            return;
        };

        let dir = std::env::temp_dir().join("snapclip-browser-probe");
        let _ = std::fs::create_dir_all(&dir);
        // A reused profile makes Chromium restore the previous session: it ignores
        // `--window-size`, opens a "restore pages?" bubble, and the page then renders in a window
        // smaller than itself — which showed up as every probe point missing.
        let profile = dir.join("profile");
        let _ = std::fs::remove_dir_all(&profile);
        let url = format!(
            "file:///{}?truth=1",
            fixture_path.display().to_string().replace('\\', "/")
        );
        let spilled = std::process::Command::new(&browser)
            .args([
                "--new-window",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-session-crashed-bubble",
                "--disable-features=Translate,TranslateUI,InfiniteSessionRestore",
                "--disable-background-networking",
                "--disable-popup-blocking",
                "--force-device-scale-factor=1",
                "--hide-scrollbars",
                // The requested size is the *outer* window, so it has to clear the fixture page
                // plus Chromium's frame and tab strip for the client area to contain 1600x1020.
                "--window-size=1800,1220",
                "--window-position=40,20",
            ])
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg(&url)
            .spawn();
        let Ok(mut child) = spilled else {
            eprintln!("skipping: cannot launch {browser}");
            return;
        };

        // Collect every candidate instead of taking the first match: a Chromium window left over
        // from an earlier probe run also carries the demo page (and its truth title) but keeps the
        // default — smaller — window size, and probing *that* window made every point miss.
        // Find the window first, and measure it *later*: `--window-size` is applied asynchronously,
        // so a client rect read while the window is still settling disagrees with the one the page
        // published its geometry against — which showed up as a viewport that could not be located.
        let deadline = std::time::Instant::now() + Duration::from_secs(40);
        let mut hwnd = None;
        while hwnd.is_none() && std::time::Instant::now() < deadline {
            pump(200);
            hwnd = win32::enumerate_cheap_candidates()
                .unwrap_or_default()
                .into_iter()
                .find(|probe| {
                    if probe.class_name != "Chrome_WidgetWin_1" {
                        return false;
                    }
                    let title = probe_title(probe.hwnd);
                    title.contains("SNAPCLIP_TRUTH:")
                        || title.contains("SNAPCLIP_ERROR:")
                        || title.contains("browser element demo")
                })
                .map(|probe| probe.hwnd);
        }
        let Some(hwnd) = hwnd else {
            let _ = child.kill();
            eprintln!("skipping: the demo page never appeared");
            return;
        };
        // Chromium throttles a background (or covered) tab's accessibility tree, so the probe window
        // is raised above whatever the terminal is showing. `SetForegroundWindow` alone fails here:
        // Windows only lets the foreground process call it, and the test binary is not it.
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                HWND_TOPMOST, SetForegroundWindow, SetWindowPos, SWP_SHOWWINDOW,
            };
            SetWindowPos(
                hwnd as *mut core::ffi::c_void,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_SHOWWINDOW | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                    | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOSIZE,
            );
            SetForegroundWindow(hwnd as *mut core::ffi::c_void);
        }
        // The tree materialises asynchronously after the window appears; give Chromium time to
        // answer for every fixture before measuring (see docs/18 §14.3 `AccessibilityPending`).
        pump(1500);

        let title = probe_title(hwnd);
        if let Some(message) = marker_payload(&title, "SNAPCLIP_ERROR:") {
            let _ = child.kill();
            panic!("the fixture page failed to build: {message}");
        }
        let (Some(truth), Some(frame), Some(client)) =
            (truth_of(&title), win32::frame_bounds(hwnd), client_origin_and_size(hwnd))
        else {
            let _ = child.kill();
            eprintln!("skipping: the demo page never reported its geometry");
            return;
        };
        if client.width() < 1600 || client.height() < 1020 {
            let _ = child.kill();
            eprintln!(
                "skipping: the browser client area is {}x{}, smaller than the 1600x1020 page",
                client.width(),
                client.height()
            );
            return;
        }
        println!(
            "[probe] browser={browser} hwnd={hwnd} frame={frame:?} client={client:?}"
        );

        let metrics = WindowDetectionMetrics::new();
        metrics.set_verbose(true);
        let mut provider = UiaDeepSelectionProvider::new(metrics);
        let walk = provider.automation().cloned().and_then(|auto| RawWalk::new(&auto));
        // Fixture coordinates are viewport-relative, so the page origin — not the browser's
        // client origin — is what they are relative to. Retried, because the tree can still be
        // filling in while the window has already settled.
        let viewport = (0..50).find_map(|attempt| {
            if attempt > 0 {
                pump(200);
            }
            walk.as_ref()
                .and_then(|walk| walk.viewport_ready(hwnd, client, 8))
        });
        let Some(viewport) = viewport else {
            let _ = child.kill();
            eprintln!(
                "skipping: Chromium did not expose a populated page tree within 10 s — the probe \
                 window is probably occluded or throttled, which says nothing about the walk"
            );
            return;
        };
        println!("[probe] viewport={viewport:?}");

        let mut asserted = 0_usize;
        let mut passed = 0_usize;
        let mut retries = 0_usize;
        let mut failures = Vec::new();
        let mut layout_drift = Vec::new();
        for fixture in &manifest {
            let Some(box_) = truth.get(&fixture.id).copied() else {
                println!("[probe] ?? {} not measured by the page", fixture.id);
                continue;
            };
            // The declared layout and the measured box must agree, otherwise the fixture is
            // describing something other than what it renders.
            // A fixture that is not shown measures to an all-zero box: that is the point of the
            // `none` cases, not a layout drift. Optional fixtures are exempt as well: a rotated
            // box is *supposed* to render outside the rectangle it was laid out in.
            if fixture.rect != [0, 0, 0, 0]
                && box_[2] > 0
                && box_[3] > 0
                && !fixture.optional
                && fixture
                    .rect
                    .iter()
                    .zip(box_.iter())
                    .any(|(declared, measured)| (declared - measured).abs() > 1)
            {
                layout_drift.push(format!(
                    "{} declared {:?} but rendered {box_:?}",
                    fixture.id, fixture.rect
                ));
            }
            // A hidden fixture measures to an all-zero box, so its *declared* layout is what says
            // where it would have been — that is the point the walk must not resolve to it.
            let own = if box_[2] > 0 && box_[3] > 0 {
                box_
            } else {
                fixture.rect
            };
            let [dx, dy] = fixture.probe.unwrap_or([own[2] / 2, own[3] / 2]);
            let point = Point::new(viewport.left + own[0] + dx, viewport.top + own[1] + dy);
            // DIAGNOSTIC: measure each fixture from a cold provider, to tell "the walk cannot get
            // there" apart from "a level was cached while Chromium's tree was still empty".
            if std::env::var_os("SNAPCLIP_PROBE_COLD").is_some() {
                provider.release();
            }
            // Bounded retries: Chromium builds its accessibility tree lazily, so the first query
            // for a window can legitimately answer "nothing below the page" and a later one finds
            // the element. The product re-queries on every dwell, so the *eventual* answer is what
            // the user sees; measuring a first try would report a readiness artefact as a defect.
            let mut attempts = 0_usize;
            let mut published = None;
            let mut last_stop = None;
            let mut last_depth = 0_usize;
            for attempt in 0..4 {
                if attempt > 0 {
                    pump(250);
                }
                attempts += 1;
                let outcome = provider
                    .resolve(&job(hwnd, point), frame, &QueryControl::refinement(&|| false))
                ;
                published = match &outcome {
                    RefinementOutcome::Target(target) => {
                        last_stop = Some(target.stop_reason);
                        last_depth = target.path.len();
                        Some(target.screen_bounds)
                    }
                    RefinementOutcome::Empty(reason) => {
                        last_stop = Some(*reason);
                        last_depth = 0;
                        None
                    }
                };
                // An answer that covers the whole page means the walk never got below it: nothing
                // to accept yet. Area, not corners: the page node's frame sits one pixel inside the
                // viewport frame, so a containment test silently disabled every retry.
                let stuck_on_page = published.is_some_and(|rect| {
                    let published_area = i64::from(rect.width()) * i64::from(rect.height());
                    let viewport_area = i64::from(viewport.width()) * i64::from(viewport.height());
                    published_area * 100 >= viewport_area * 95
                });
                if !stuck_on_page {
                    break;
                }
            }
            retries += attempts.saturating_sub(1);
            let expected = match fixture.expect.as_str() {
                "none" => None,
                "self" | "inside_self" | "covers_self" => Some((fixture.id.as_str(), own)),
                other => truth
                    .get(other)
                    .copied()
                    .map(|measured| (other, measured)),
            };
            let expected_rect = expected.map(|(_, measured)| {
                Rect::new(
                    viewport.left + measured[0],
                    viewport.top + measured[1],
                    viewport.left + measured[0] + measured[2],
                    viewport.top + measured[1] + measured[3],
                )
            });
            // For a fixture that must not be in the tree at all, the comparison target is the box
            // it would occupy — taken from the declared layout, since the page measures zeros.
            let judged = if fixture.expect == "none" {
                Some(Rect::new(
                    viewport.left + fixture.rect[0],
                    viewport.top + fixture.rect[1],
                    viewport.left + fixture.rect[0] + fixture.rect[2],
                    viewport.top + fixture.rect[1] + fixture.rect[3],
                ))
            } else {
                expected_rect
            };
            let (ok, detail) = judge(fixture.expect.as_str(), judged, published);
            let label = if fixture.optional {
                "opt"
            } else {
                asserted += 1;
                if ok {
                    passed += 1;
                } else {
                    failures.push(format!(
                        "{} (expect {}, got {})",
                        fixture.id,
                        fixture.expect,
                        published.map(|rect| format!("{rect:?}")).unwrap_or("none".into())
                    ));
                }
                "assert"
            };
            println!(
                "[probe] {label} {:<20} expect={:<16} expected={:<26} published={:<28} {}{} \
                 role={} name={}",
                fixture.id,
                fixture.expect,
                expected_rect
                    .map(|rect| format!(
                        "{}x{} @({},{})",
                        rect.width(),
                        rect.height(),
                        rect.left,
                        rect.top
                    ))
                    .unwrap_or("(not in the tree)".into()),
                published
                    .map(|rect| format!("{}x{} @({},{})", rect.width(), rect.height(), rect.left, rect.top))
                    .unwrap_or("none".into()),
                if ok { "OK  " } else { "MISS" },
                if ok {
                    String::new()
                } else {
                    format!(" ({detail}) ")
                },
                if fixture.role.is_empty() {
                    "-"
                } else {
                    fixture.role.as_str()
                },
                fixture.name
            );
            if !ok && let Some(rect) = published {
                println!(
                    "[probe]      got {rect:?} reason={last_stop:?} depth={last_depth} \
                     attempts={attempts} (viewport {viewport:?})"
                );
                if let Some(walk) = &walk {
                    // The system's own hit test: whichever element it names is the one the user
                    // would be pointing at, so it settles "the walk chose badly" against "the
                    // fixture sits under something else".
                    println!("[probe]      system hit test: {}", walk.hit_test(point));
                }
            }
            if let Some(walk) = &walk
                && !ok
            {
                walk.dump(hwnd, point, 9);
            }
        }

        let _ = child.kill();
        println!(
            "[probe] asserted={asserted} passed={passed} failed={} slow_fixtures={retries}",
            asserted - passed,
        );
        for failure in &failures {
            println!("[probe] FAIL {failure}");
        }
        for drift in &layout_drift {
            println!("[probe] LAYOUT {drift}");
        }
        // Optional rows never fail the run, but an asserted row must: the probe is the gate for
        // "web page element capture works", and a silently empty run would be worse than a
        // failing one.
        assert!(asserted > 0, "the fixture must assert something");
        assert!(
            layout_drift.is_empty(),
            "the fixture's declared layout does not match what it renders: {layout_drift:#?}"
        );
        assert!(
            failures.is_empty(),
            "{} of {asserted} asserted fixtures did not resolve to the expected box: {failures:#?}",
            asserted - passed
        );
    }

    /// Compare what we published against what the page measured.
    fn judge(expect: &str, expected: Option<Rect>, published: Option<Rect>) -> (bool, &'static str) {
        const TOLERANCE: i32 = 3;
        match (expect, expected, published) {
            // `none` means the fixture is not in the tree at all, so the answer must be coarser
            // than its box — anything else proves we captured a hidden element.
            ("none", Some(expected), Some(published)) => {
                let covers = published.left <= expected.left
                    && published.top <= expected.top
                    && published.right >= expected.right
                    && published.bottom >= expected.bottom;
                (covers, "a hidden fixture must not be selected")
            }
            // Some elements are not exposed by the browser's accessibility tree at all, so the
            // honest assertion is the *shape* of the answer rather than its exact box:
            // `inside_self` — the walk went into the element (its text run counts), and
            // `covers_self` — the element is not exposed, so the nearest exposed ancestor must be
            // returned, as long as it is local rather than the whole page.
            ("inside_self", Some(expected), Some(published)) => {
                let inside = published.left >= expected.left - TOLERANCE
                    && published.top >= expected.top - TOLERANCE
                    && published.right <= expected.right + TOLERANCE
                    && published.bottom <= expected.bottom + TOLERANCE;
                (inside, "the answer must sit inside the element")
            }
            ("covers_self", Some(expected), Some(published)) => {
                let covers = published.left <= expected.left
                    && published.top <= expected.top
                    && published.right >= expected.right
                    && published.bottom >= expected.bottom;
                let published_area = i64::from(published.width()) * i64::from(published.height());
                let expected_area = i64::from(expected.width()) * i64::from(expected.height());
                (
                    covers && published_area <= expected_area * 3,
                    "a coarse answer may cover the element, but must stay local",
                )
            }
            (_, None, _) => (false, "no expectation to compare"),
            (_, Some(_), None) => (false, "nothing was published"),
            (_, Some(expected), Some(published)) => {
                let close = (published.left - expected.left).abs() <= TOLERANCE
                    && (published.top - expected.top).abs() <= TOLERANCE
                    && (published.right - expected.right).abs() <= TOLERANCE
                    && (published.bottom - expected.bottom).abs() <= TOLERANCE;
                (close, "published box != measured box")
            }
        }
    }

    /// The fixture manifest, embedded in the demo page as a JSON script block.
    fn manifest_of(source: &str) -> Option<Vec<Fixture>> {
        let start = source.find("\"manifest\">")? + "\"manifest\">".len();
        let end = source[start..].find("</script>")? + start;
        serde_json::from_str(&source[start..end]).ok()
    }

    /// The page's measured boxes, as published in the window title.
    ///
    /// The browser appends its own suffix to the window title (`"… - Google Chrome"`), so the
    /// payload is located by its marker and cut at the matching brace rather than assumed to be
    /// the whole string. Our payload holds no strings, so counting braces is enough.
    fn truth_of(title: &str) -> Option<std::collections::HashMap<String, [i32; 4]>> {
        let json = marker_payload(title, "SNAPCLIP_TRUTH:")?;
        let start = json.find('{')?;
        let mut depth = 0_i32;
        for (index, character) in json[start..].char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return serde_json::from_str(&json[start..=start + index]).ok();
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Everything after `marker` in `title`, or `None` when the marker is absent.
    fn marker_payload<'a>(title: &'a str, marker: &str) -> Option<&'a str> {
        title.find(marker).map(|at| &title[at + marker.len()..])
    }

    /// Standard install locations of a Chromium-based browser on Windows.
    fn find_chromium() -> Option<String> {
        let roots = [
            std::env::var("ProgramFiles").ok(),
            std::env::var("ProgramFiles(x86)").ok(),
            std::env::var("LOCALAPPDATA").ok(),
        ];
        let relatives = [
            r"Microsoft\Edge\Application\msedge.exe",
            r"Google\Chrome\Application\chrome.exe",
        ];
        roots
            .iter()
            .flatten()
            .flat_map(|root| relatives.iter().map(move |rel| format!("{root}\\{rel}")))
            .find(|path| std::path::Path::new(path).is_file())
    }

    /// Does a rule change make Explorer's answers coarser? Measure, do not eyeball.
    ///
    /// Runs the **product** walk over a grid of points inside a File Explorer window and prints the
    /// published rectangle and depth for each. Two runs of this — one per candidate-order rule —
    /// are what tells "this rule fixes Chromium without costing Explorer", which is exactly the
    /// question docs/18 §14.5 left open when the browser work was rolled back.
    ///
    /// ```text
    /// cargo test --lib explorer_rule_probe -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "opens a File Explorer window; run explicitly with --ignored --nocapture"]
    fn explorer_rule_probe() {
        let _ = crate::platform::windows::capture::monitor::set_per_monitor_v2_awareness();
        let mut window = win32::enumerate_cheap_candidates()
            .unwrap_or_default()
            .into_iter()
            .find(|probe| probe.class_name == "CabinetWClass")
            .map(|probe| probe.hwnd);
        if window.is_none() {
            let home = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into());
            let _ = std::process::Command::new("explorer.exe").arg(home).spawn();
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            while window.is_none() && std::time::Instant::now() < deadline {
                pump(200);
                window = win32::enumerate_cheap_candidates()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|probe| probe.class_name == "CabinetWClass")
                    .map(|probe| probe.hwnd);
            }
        }
        let Some(hwnd) = window else {
            eprintln!("skipping: no File Explorer window available");
            return;
        };
        let Some(frame) = win32::frame_bounds(hwnd) else {
            eprintln!("skipping: no frame bounds for the Explorer window");
            return;
        };
        let Some(client) = client_origin_and_size(hwnd) else {
            eprintln!("skipping: no client rect for the Explorer window");
            return;
        };
        println!("[explorer] hwnd={hwnd} frame={frame:?} client={client:?}");

        let mut provider = UiaDeepSelectionProvider::new(WindowDetectionMetrics::new());
        let window_area = i64::from(client.width()) * i64::from(client.height());
        let mut areas = Vec::new();
        for fy in [30_i32, 40, 50, 60, 70] {
            for fx in [45_i32, 55, 65, 75, 85] {
                let point = Point::new(
                    client.left + client.width() * fx / 100,
                    client.top + client.height() * fy / 100,
                );
                let outcome = provider.resolve(
                    &job(hwnd, point),
                    frame,
                    &QueryControl::refinement(&|| false),
                );
                let (rect, depth, reason) = match &outcome {
                    RefinementOutcome::Target(target) => (
                        target.screen_bounds,
                        target.path.len(),
                        target.stop_reason,
                    ),
                    RefinementOutcome::Empty(reason) => (Rect::default(), 0, *reason),
                };
                let area = i64::from(rect.width()) * i64::from(rect.height());
                areas.push(area);
                println!(
                    "[explorer] point=({:>5},{:>5}) depth={:>2} box={}x{} at ({},{}) area_pct={:.1} \
                     reason={reason:?}",
                    point.x,
                    point.y,
                    depth,
                    rect.width(),
                    rect.height(),
                    rect.left,
                    rect.top,
                    (area as f64) * 100.0 / (window_area.max(1) as f64),
                );
            }
        }
        areas.sort_unstable();
        let median = areas[areas.len() / 2];
        let control_level = areas
            .iter()
            .filter(|area| **area * 5 < window_area)
            .count();
        println!(
            "[explorer] summary: median_area_pct={:.1} control_level_points={}/{} \
             (a coarser rule raises the median and lowers the count)",
            (median as f64) * 100.0 / (window_area.max(1) as f64),
            control_level,
            areas.len()
        );
    }

    /// Title of a top-level window, used to identify the probe page.
    fn probe_title(hwnd: isize) -> String {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW};
        let length = unsafe { GetWindowTextLengthW(hwnd as *mut core::ffi::c_void) };
        if length <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let written = unsafe {
            GetWindowTextW(
                hwnd as *mut core::ffi::c_void,
                buffer.as_mut_ptr(),
                buffer.len() as i32,
            )
        };
        String::from_utf16_lossy(&buffer[..written.max(0) as usize])
    }

    /// The client rectangle of `hwnd` in screen pixels.
    fn client_origin_and_size(hwnd: isize) -> Option<Rect> {
        use windows_sys::Win32::Foundation::{POINT as SYS_POINT, RECT as SYS_RECT};
        use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;
        let mut rect = SYS_RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if unsafe { GetClientRect(hwnd as *mut core::ffi::c_void, &mut rect) } == 0 {
            return None;
        }
        let mut origin = SYS_POINT { x: 0, y: 0 };
        if unsafe { ClientToScreen(hwnd as *mut core::ffi::c_void, &mut origin) } == 0 {
            return None;
        }
        Some(Rect::new(
            origin.x,
            origin.y,
            origin.x + (rect.right - rect.left),
            origin.y + (rect.bottom - rect.top),
        ))
    }

    struct RawWalk {
        request: IUIAutomationCacheRequest,
        automation: IUIAutomation,
    }

    impl RawWalk {
        fn new(automation: &IUIAutomation) -> Option<Self> {
            let request = unsafe { automation.CreateCacheRequest() }.ok()?;
            unsafe {
                request.AddProperty(UIA_BoundingRectanglePropertyId).ok()?;
                request.AddProperty(UIA_ControlTypePropertyId).ok()?;
                request
                    .AddProperty(::windows::Win32::UI::Accessibility::UIA_NamePropertyId)
                    .ok()?;
                request.SetTreeScope(TreeScope_Children).ok()?;
            }
            Some(Self {
                request,
                automation: automation.clone(),
            })
        }

        fn children(
            &self,
            element: &IUIAutomationElement,
        ) -> Vec<(IUIAutomationElement, Rect, i32, String)> {
            let mut out = Vec::new();
            let Ok(cached) = (unsafe { element.BuildUpdatedCache(&self.request) }) else {
                return out;
            };
            let Ok(array) = (unsafe { cached.GetCachedChildren() }) else {
                return out;
            };
            let count = unsafe { array.Length() }.ok().unwrap_or(0).max(0) as usize;
            for index in 0..count {
                let Ok(child) = (unsafe { array.GetElement(index as i32) }) else {
                    continue;
                };
                let Ok(rect) = (unsafe { child.CachedBoundingRectangle() }) else {
                    continue;
                };
                let kind = unsafe { child.CachedControlType() }.map(|kind| kind.0).unwrap_or(0);
                let name = unsafe { child.CachedName() }
                    .map(|name| name.to_string())
                    .unwrap_or_default();
                out.push((child, to_rect(rect), kind, name));
            }
            out
        }

        /// The page's viewport rectangle, in screen pixels.
        ///
        /// Chromium draws its own frame, so the *client* area is the whole window — tabs and
        /// toolbar included — and the page starts lower down. Guessing that offset put every probe
        /// point on Chrome's reload button, so the rectangle is read from the tree instead: walk
        /// the window's **own subtree** for the node that spans the client's full width, touches
        /// its bottom edge, and starts *below* its top edge. Chromium's window and its tab strip
        /// fail the last condition; the web area satisfies all three (measured:
        /// `Pane(48,107)-(1832,1232)` inside a client of `(48,20)-(1832,1232)`).
        ///
        /// Searching the subtree rather than the desktop is deliberate: a point-based hit test
        /// returns whatever window is *on top* at that spot, which made this step depend on the
        /// probe window being foreground — it silently returned another window's element whenever
        /// the terminal was covering it.
        /// The viewport, but only once `want` nodes are exposed *inside* it.
        ///
        /// Chromium builds its accessibility tree lazily and throttles it while the tab is not
        /// active: reading too early returns a placeholder (`Pane(0,0)-(0,0)`, measured) and every
        /// probe point then resolves to the page rectangle — which looked like a product failure
        /// and was only a measurement that started too soon. Requiring real content under the
        /// viewport is what makes the probe's verdict trustworthy.
        fn viewport_ready(&self, hwnd: isize, client: Rect, want: usize) -> Option<Rect> {
            let root = unsafe { self.automation.ElementFromHandle(HWND(hwnd as *mut _)) }.ok()?;
            let mut frontier = std::collections::VecDeque::from([(root, 0_usize)]);
            let mut visited = 0_usize;
            let mut viewport = None;
            let mut inside = 0_usize;
            while let Some((element, depth)) = frontier.pop_front() {
                visited += 1;
                if visited > 3000 || depth > 14 {
                    return None;
                }
                for (child, bounds, _, _) in self.children(&element) {
                    if bounds.is_empty() {
                        continue;
                    }
                    let is_viewport = bounds.left == client.left
                        && bounds.right == client.right
                        && bounds.bottom == client.bottom
                        && bounds.top > client.top;
                    match viewport {
                        None => {
                            if is_viewport {
                                viewport = Some(bounds);
                            }
                            frontier.push_back((child, depth + 1));
                        }
                        Some(found) => {
                            let strictly_inside = bounds.left >= found.left
                                && bounds.top >= found.top
                                && bounds.right <= found.right
                                && bounds.bottom <= found.bottom;
                            if strictly_inside {
                                inside += 1;
                                frontier.push_back((child, depth + 1));
                                if inside >= want {
                                    return viewport;
                                }
                            }
                        }
                    }
                }
            }
            // Only a *populated* subtree is a trustworthy basis for a verdict.
            if inside >= want { viewport } else { None }
        }

        /// Explore every containing branch from the window root, breadth-limited and depth-limited.
        ///
        /// Following only the first containing child hides the interesting case: Chromium's window
        /// root offers two overlapping `Pane` siblings, and the first one is a dead leaf.
        fn dump(&self, hwnd: isize, point: Point, max_levels: usize) {
            let Ok(root) =
                (unsafe { self.automation.ElementFromHandle(HWND(hwnd as *mut _)) })
            else {
                println!("[probe] raw: ElementFromHandle failed");
                return;
            };
            self.explore(&root, point, 0, max_levels, "");
        }

        /// What the system itself says is under `point`.
        fn hit_test(&self, point: Point) -> String {
            let system_point = ::windows::Win32::Foundation::POINT {
                x: point.x,
                y: point.y,
            };
            let Ok(element) = (unsafe { self.automation.ElementFromPoint(system_point) }) else {
                return "n/a".into();
            };
            let name = unsafe { element.CurrentName() }
                .map(|name| name.to_string())
                .unwrap_or_default();
            let kind = unsafe { element.CurrentControlType() }
                .map(|kind| kind.0)
                .unwrap_or(0);
            let bounds = to_rect(unsafe { element.CurrentBoundingRectangle() }.unwrap_or_default());
            format!(
                "{}({},{})-({},{}) {:?}",
                control_type_name(kind),
                bounds.left,
                bounds.top,
                bounds.right,
                bounds.bottom,
                name.chars().take(24).collect::<String>()
            )
        }

        fn explore(
            &self,
            element: &IUIAutomationElement,
            point: Point,
            level: usize,
            max_levels: usize,
            indent: &str,
        ) {
            if level >= max_levels {
                return;
            }
            let children = self.children(element);
            let containing: Vec<_> = children
                .iter()
                .filter(|(_, bounds, _, _)| bounds.contains(point))
                .collect();
            println!(
                "[probe] raw {indent}#{level} children={} containing={}",
                children.len(),
                containing.len()
            );
            // When nothing contains the point any more, print what *is* there: that is where the
            // page's own boxes live, and whether they carry real rectangles decides whether deep
            // selection can reach them at all.
            if containing.is_empty() {
                for (_, bounds, kind, name) in children.iter().take(6) {
                    println!(
                        "[probe] raw {indent}  ( ) {}({},{})-({},{}) {:?}",
                        control_type_name(*kind),
                        bounds.left,
                        bounds.top,
                        bounds.right,
                        bounds.bottom,
                        name.chars().take(28).collect::<String>()
                    );
                }
                return;
            }
            // At most two branches per level: enough to show the dead leaf *and* the content
            // branch, without letting a big page walk itself to death.
            let branch = containing.len() <= 2;
            for (child, bounds, kind, name) in containing {
                println!(
                    "[probe] raw {indent}  -> {}({},{})-({},{}) {:?}",
                    control_type_name(*kind),
                    bounds.left,
                    bounds.top,
                    bounds.right,
                    bounds.bottom,
                    name.chars().take(28).collect::<String>()
                );
                if branch {
                    self.explore(child, point, level + 1, max_levels, &format!("{indent}    "));
                }
            }
        }
    }

    /// The control types that matter when reading the probe output.
    fn control_type_name(kind: i32) -> &'static str {
        match kind {
            50000 => "Button",
            50003 => "ComboBox",
            50004 => "Edit",
            50005 => "Hyperlink",
            50006 => "Image",
            50007 => "ListItem",
            50008 => "List",
            50020 => "Text",
            50025 => "Custom",
            50026 => "Group",
            50029 => "DataItem",
            50030 => "Document",
            50032 => "Window",
            50033 => "Pane",
            50034 => "Header",
            0 => "Unknown",
            _ => "Other",
        }
    }
}
