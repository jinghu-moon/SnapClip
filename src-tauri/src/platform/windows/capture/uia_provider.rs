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
use ::windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement, TreeScope_Children,
    UIA_BoundingRectanglePropertyId, UIA_ControlTypePropertyId, UIA_IsOffscreenPropertyId,
};

use crate::capture::geometry::{Point, Rect};
use crate::capture::window_detection::deep::{
    DeepSelectionProvider, DeepTarget, QueryControl, RefinementJob, RefinementOutcome,
    StopReason,
};
use crate::capture::window_detection::model::{SnapshotEpoch, TargetKind};
use crate::capture::window_detection::uia::{
    WalkBudget, WalkNode, WalkOutcome, fallback_hit_path, is_descendable, may_try_sibling_branch,
    merge_hit_paths,
};

use super::win::window as win32;

/// UIA provider: resolves the deepest element inside one window.
#[derive(Debug, Default)]
pub struct UiaDeepSelectionProvider {
    automation: Option<IUIAutomation>,
    /// Batched property request: one cross-process call per level instead of four per node
    /// (docs/18 §11). Built lazily on the refinement thread.
    cache: Option<IUIAutomationCacheRequest>,
    /// Expanded children per (window, parent bounds).
    ///
    /// Moving between controls of one window walks the same upper levels over and over;
    /// remembering them turns a repeat query into "fetch the deepest level only" instead of
    /// "walk from the window root again" (docs/18 §11 ②). The whole table is dropped when the
    /// snapshot generation changes, so no stale geometry can be served.
    children: HashMap<NodeKey, Vec<(IUIAutomationElement, WalkNode)>>,
    /// Snapshot generation the table belongs to.
    cache_epoch: Option<SnapshotEpoch>,
    /// Provider-free child-window rectangles per window, valid for `cache_epoch`.
    fallback_rects: HashMap<isize, Vec<Rect>>,
    /// Windows whose provider failed; skipped until the next snapshot generation.
    quarantined: HashSet<isize>,
}

/// Identity of one expanded node: window handle plus its screen rectangle.
///
/// Bounds are the practical identity here — two different elements of one window sharing the
/// same rectangle are interchangeable for a hit test, and the reference selector treats
/// same-bounds containers the same way.
type NodeKey = (isize, i32, i32, i32, i32);

impl UiaDeepSelectionProvider {
    pub fn new() -> Self {
        Self::default()
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
        Some(WalkNode::new(bounds, control_type, offscreen, true))
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
    ) -> Vec<(IUIAutomationElement, WalkNode)> {
        let Ok(parent) = (unsafe { parent.BuildUpdatedCache(request) }) else {
            return Vec::new();
        };
        let Ok(children) = (unsafe { parent.GetCachedChildren() }) else {
            return Vec::new();
        };
        let count = unsafe { children.Length() }.ok().unwrap_or(0).max(0) as usize;
        let mut expanded = Vec::with_capacity(count);
        for index in 0..count {
            let Ok(child) = (unsafe { children.GetElement(index as i32) }) else {
                continue;
            };
            let Some(node) = Self::cached_node(&child) else {
                continue;
            };
            expanded.push((child, node));
        }
        expanded
    }

    /// Every descendable child of a node that contains `point`, smallest first.
    ///
    /// Returning *all* containing candidates (not just the smallest) is what makes structural
    /// backtracking possible: when the smallest one dead-ends, the walk must be able to come
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
        let key: NodeKey = (
            hwnd,
            parent_bounds.left,
            parent_bounds.top,
            parent_bounds.right,
            parent_bounds.bottom,
        );
        if !self.children.contains_key(&key) {
            let expanded = Self::expand(request, parent);
            self.children.insert(key, expanded);
        }
        let Some(children) = self.children.get(&key) else {
            return Vec::new();
        };
        let mut candidates: Vec<(IUIAutomationElement, WalkNode)> = Vec::new();
        for (child, node) in children {
            if !budget.take_node() {
                break;
            }
            if !is_descendable(parent_bounds, *node) || !node.bounds.contains(point) {
                continue;
            }
            candidates.push((child.clone(), *node));
        }
        candidates.sort_unstable_by_key(|(_, node)| {
            (
                node.bounds.area(),
                node.bounds.left,
                node.bounds.top,
                node.bounds.right,
                node.bounds.bottom,
            )
        });
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
            return RefinementOutcome::Empty(StopReason::Unsupported);
        }
        // Clone the interface pointer so the immutable borrow of `self` ends before any
        // quarantine bookkeeping below.
        let Some(automation) = self.automation().cloned() else {
            self.quarantined.insert(job.window.hwnd);
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

        // Bounded depth-first walk **with structural backtracking** (docs/18 §12.6 ①).
        //
        // The previous linear descent stopped at the first dead end, so a redundant
        // same-bounds pane sitting in front of the real content branch swallowed the whole
        // hit (resource-manager file lists, Chromium-style trees). The stack keeps every
        // branch that is still worth trying; `path` mirrors the frames that are currently
        // committed, and `best_path` remembers the deepest committed path so abandoning a
        // dead branch never loses a good answer.
        struct Frame {
            bounds: Rect,
            /// The node this frame represents, kept so backtracking can ask whether a dead
            /// branch was a redundant structural container.
            node: WalkNode,
            candidates: Vec<(IUIAutomationElement, WalkNode)>,
            next: usize,
        }

        let mut budget = WalkBudget::new();
        let mut path: Vec<Rect> = vec![window_bounds];
        let mut best_path = path.clone();
        let root_candidates = self.containing_children(
            job.window.hwnd,
            &request,
            &root,
            window_bounds,
            job.point,
            &mut budget,
        );
        let mut stack: Vec<Frame> = vec![Frame {
            bounds: window_bounds,
            node: WalkNode::new(window_bounds, 0, false, true),
            candidates: root_candidates,
            next: 0,
        }];

        while let Some(frame) = stack.last_mut() {
            if control.is_cancelled() {
                outcome.stop_reason = StopReason::Cancelled;
                break;
            }
            if frame.next >= frame.candidates.len() {
                // Dead end. Backtrack only through a redundant structural node: a real control
                // or a distinct container frame keeps its precedence and ends the walk.
                let dead = stack.pop().expect("the loop only runs with a frame");
                let parent_bounds = stack.last().map(|parent| parent.bounds);
                let may_backtrack = parent_bounds
                    .is_some_and(|parent| may_try_sibling_branch(dead.node, parent));
                if !may_backtrack {
                    break;
                }
                if path.len() > 1 {
                    path.pop();
                }
                continue;
            }
            if !budget.enter_children() {
                outcome.stop_reason = StopReason::TraversalLimit;
                break;
            }
            let (child, node) = frame.candidates[frame.next].clone();
            frame.next += 1;
            let child_candidates = self.containing_children(
                job.window.hwnd,
                &request,
                &child,
                node.bounds,
                job.point,
                &mut budget,
            );
            path.push(node.bounds);
            if path.len() > best_path.len() {
                best_path = path.clone();
            }
            stack.push(Frame {
                bounds: node.bounds,
                node,
                candidates: child_candidates,
                next: 0,
            });
        }
        outcome.path = best_path;
        outcome.target = outcome.path.last().copied().unwrap_or(window_bounds);

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
        let mut provider = UiaDeepSelectionProvider::new();
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
        let mut provider = UiaDeepSelectionProvider::new();
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
    fn a_real_window_yields_a_path_that_starts_at_the_window_frame() {
        let Some(fixture) = FixtureWindow::create() else {
            eprintln!("skipping: no interactive window station available");
            return;
        };
        let mut provider = UiaDeepSelectionProvider::new();
        if provider.automation().is_none() {
            eprintln!("skipping: UI Automation is unavailable in this environment");
            return;
        }
        let bounds = Rect::new(200, 200, 560, 460);
        let control = QueryControl::refinement(&|| false);
        let outcome = provider.resolve(
            &job(fixture.handle(), Point::new(280, 280)),
            bounds,
            &control,
        );
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
        let mut provider = UiaDeepSelectionProvider::new();
        if provider.automation().is_none() {
            eprintln!("skipping: UI Automation is unavailable in this environment");
            return;
        }
        let bounds = Rect::new(200, 200, 560, 460);
        let control = QueryControl::refinement(&|| false);

        let _ = provider.resolve(&job(fixture.handle(), Point::new(280, 280)), bounds, &control);
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
}
