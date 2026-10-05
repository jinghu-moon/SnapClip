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
};

use crate::capture::geometry::{Point, Rect};
use crate::capture::diagnostics::WindowDetectionMetrics;
use crate::capture::window_detection::deep::{
    DeepSelectionProvider, DeepTarget, QueryControl, RefinementJob, RefinementOutcome,
    StopReason,
};
use crate::capture::window_detection::model::{SnapshotEpoch, TargetKind};
use crate::capture::window_detection::uia::{
    WalkBudget, WalkNode, WalkOutcome, fallback_hit_path, is_descendable, merge_hit_paths,
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
    children: HashMap<NodeKey, (Vec<(IUIAutomationElement, WalkNode)>, LevelStats)>,
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
        // The element's own identity, never its rectangle: two nodes may share a rectangle.
        let _ = hwnd;
        let key: NodeKey = parent.as_raw() as usize;
        if !self.children.contains_key(&key) {
            let expanded = Self::expand(request, parent);
            self.children.insert(key, expanded);
        }
        let Some((children, stats)) = self.children.get(&key) else {
            return Vec::new();
        };
        let stats = *stats;
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

        // Bounded descent (docs/18 §11 ①/⑤).
        //
        // The depth-first variant with structural backtracking (docs/18 §12.7) was reverted:
        // it measured *worse* on real hardware — published targets became coarse enough that
        // the scheduler's "moving inside the published path answers from cache" rule
        // suppressed most re-queries (`refinement_submitted` fell below the window-switch
        // count, and controls stopped following the cursor). Backtracking will be re-attempted
        // only together with per-query instrumentation that can show what it changes.
        let mut budget = WalkBudget::new();
        let mut current = root;
        let mut current_bounds = window_bounds;
        // UIA can hand back cycles between same-bounds nodes (A → B → A). The forensic log
        // showed one such pair alternating until the depth budget ran out, which padded the
        // published path with duplicate levels. Visiting each node at most once bounds the
        // walk by the tree itself instead of by the budget.
        let mut visited: HashSet<usize> = HashSet::new();
        visited.insert(current.as_raw() as usize);
        loop {
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
            let Some((child, node)) = self
                .containing_children(
                    job.window.hwnd,
                    &request,
                    &current,
                    current_bounds,
                    job.point,
                    &mut budget,
                )
                .into_iter()
                .next()
            else {
                break;
            };
            // Cycle guard: never step into the node we are already standing on. A genuine
            // same-bounds *child* is still allowed (that is how Explorer nests its panes), but
            // the walk must make progress.
            let child_id = child.as_raw() as usize;
            if !visited.insert(child_id) {
                break;
            }
            if !outcome.push(node.bounds) {
                break;
            }
            current = child;
            current_bounds = node.bounds;
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

    /// The page the browser probe opens: a known grid of elements plus a nested pair.
    const PROBE_PAGE: &str = r##"<!doctype html>
<html><head><meta charset="utf-8"><title>SnapClip UIA probe</title>
<style>
  html,body{margin:0;padding:0;background:#fff;font:16px/1.2 "Segoe UI",sans-serif}
  #grid{display:grid;grid-template-columns:repeat(3,220px);gap:16px;padding:24px}
  .cell{height:120px;display:flex;align-items:center;justify-content:center;
        border:2px solid #333;background:#eef}
  #row2{margin:24px;padding:8px;border:2px dashed #c00}
  #nested{padding:18px;border:2px solid #090;background:#efe}
</style></head><body>
<div id="grid">
  <button class="cell" id="b1">Button One</button>
  <div class="cell" id="d1">Div Two</div>
  <a class="cell" id="a1" href="#">Link Three</a>
</div>
<div id="row2"><div id="nested"><span id="s1">Nested Span</span></div></div>
<input id="i1" style="margin:24px;width:320px;height:40px" value="Input Field">
</body></html>
"##;

    /// What our walk reaches inside a Chromium window.
    ///
    /// Run explicitly — it launches its own browser on a throwaway profile, so the user's
    /// browser state is untouched:
    ///
    /// ```text
    /// cargo test --lib browser_element_probe -- --ignored --nocapture
    /// ```
    ///
    /// For each probe point it prints the raw accessibility chain Chromium exposes (control
    /// type + name + rectangle per level) next to what [`UiaDeepSelectionProvider`] publishes.
    /// That is the evidence behind "how far does deep selection get inside a web page". It is
    /// deliberately **not** part of the default suite: it needs a browser and a visible window.
    #[test]
    #[ignore = "launches a browser; run explicitly with --ignored --nocapture"]
    fn browser_element_probe() {
        let Some(browser) = find_chromium() else {
            eprintln!("skipping: no Chromium-based browser found");
            return;
        };
        let dir = std::env::temp_dir().join("snapclip-browser-probe");
        let _ = std::fs::create_dir_all(&dir);
        let page = dir.join("probe.html");
        if std::fs::write(&page, PROBE_PAGE).is_err() {
            eprintln!("skipping: cannot write the probe page");
            return;
        }
        let url = format!("file:///{}", page.display().to_string().replace('\\', "/"));
        let spilled = std::process::Command::new(&browser)
            .args([
                "--new-window",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-session-crashed-bubble",
                "--window-size=1280,980",
                "--window-position=120,60",
            ])
            .arg(format!("--user-data-dir={}", dir.join("profile").display()))
            .arg(&url)
            .spawn();
        let Ok(mut child) = spilled else {
            eprintln!("skipping: cannot launch {browser}");
            return;
        };

        let deadline = std::time::Instant::now() + Duration::from_secs(40);
        let mut window = None;
        while window.is_none() && std::time::Instant::now() < deadline {
            pump(100);
            window = win32::enumerate_cheap_candidates()
                .unwrap_or_default()
                .into_iter()
                .find(|probe| {
                    probe.class_name == "Chrome_WidgetWin_1"
                        && probe_title(probe.hwnd).contains("probe")
                })
                .map(|probe| probe.hwnd);
        }
        let Some(hwnd) = window else {
            let _ = child.kill();
            eprintln!("skipping: the browser window never appeared");
            return;
        };
        // Let the page lay out and Chromium bring its accessibility tree up.
        pump(1500);
        let Some(frame) = win32::frame_bounds(hwnd) else {
            let _ = child.kill();
            eprintln!("skipping: no frame bounds for the browser window");
            return;
        };
        let client = client_origin_and_size(hwnd).unwrap_or(frame);
        println!(
            "[probe] browser={browser} hwnd={hwnd} frame={frame:?} client=({}, {}) size={}x{}",
            client.left,
            client.top,
            client.right,
            client.bottom
        );

        let metrics = WindowDetectionMetrics::new();
        metrics.set_verbose(true);
        let mut provider = UiaDeepSelectionProvider::new(metrics);
        let walk = provider.automation().cloned().and_then(|auto| RawWalk::new(&auto));

        for (label, fx, fy) in [
            ("button-cell", 0.15_f32, 0.15_f32),
            ("div-cell", 0.43, 0.15),
            ("link-cell", 0.70, 0.15),
            ("nested-span", 0.30, 0.42),
            ("blank-body", 0.50, 0.92),
        ] {
            let point = Point::new(
                client.left + (client.width() as f32 * fx) as i32,
                client.top + (client.height() as f32 * fy) as i32,
            );
            println!("[probe] === {label} at ({}, {}) ===", point.x, point.y);
            if let Some(walk) = &walk {
                walk.dump(hwnd, point, 8);
            }
            let control = QueryControl::refinement(&|| false);
            match provider.resolve(&job(hwnd, point), frame, &control) {
                RefinementOutcome::Target(target) => println!(
                    "[probe] provider -> {:?} {}x{} depth={} reason={:?}",
                    target.screen_bounds,
                    target.screen_bounds.width(),
                    target.screen_bounds.height(),
                    target.path.len(),
                    target.stop_reason
                ),
                RefinementOutcome::Empty(reason) => {
                    println!("[probe] provider -> empty reason={reason:?}")
                }
            }
        }
        let _ = child.kill();
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

    /// Client origin (screen pixels) and client size of `hwnd`.
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
            rect.right - rect.left,
            rect.bottom - rect.top,
        ))
    }

    /// Raw accessibility walk, for comparing our policy against what Chromium exposes.
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
