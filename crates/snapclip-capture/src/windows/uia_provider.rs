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
    CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement, TreeScope,
    TreeScope_Children, TreeScope_Element, UIA_BoundingRectanglePropertyId,
    UIA_ControlTypePropertyId, UIA_IsOffscreenPropertyId, UIA_NativeWindowHandlePropertyId,
};

use crate::geometry::{Point, Rect};
use crate::diagnostics::{PrecisionOutcome, WindowDetectionMetrics};
use crate::window_detection::deep::{
    DeepSelectionProvider, DeepTarget, QueryControl, RefinementJob, RefinementOutcome,
    StopReason,
};
use crate::window_detection::model::{PathLevel, SnapshotEpoch, TargetKind};
use crate::window_detection::uia::{
    WalkBudget, WalkNode, WalkOutcome, fallback_hit_path, is_descendable, is_structural_wrapper,
    is_text_run_inside_element, is_unspecific_hit, level_kind_of_control_type, merge_hit_paths,
    push_box_keeping_containment, should_adopt_provider_box,
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
    /// Passes the point hit test through the capture overlay for the duration of that one call
    /// (docs/21 §5.7). Inert in tests and probes, where nothing of ours covers the desktop.
    hit_test_pass_through: win32::HitTestPassThrough,
    /// What this provider's own point hit test decided for the most recent query (docs/21 §5.16).
    ///
    /// The *decision* is recorded once per query by the composite provider, which is also the side
    /// that can ask the MSAA transport; this field is how that side learns what the UIA hit saw
    /// without computing it twice.
    last_hit: Option<HitDecision>,
    /// Whether a bare text run may become the answer (docs/21 §5.19).
    adopt_text_runs: bool,
}

/// One transport's verdict on the point hit test, for the per-query forensics line.
#[derive(Debug, Clone)]
pub(crate) struct HitDecision {
    pub outcome: PrecisionOutcome,
    pub note: String,
    /// Whether the hit test named a *specific* element rather than the document or a window-sized
    /// container. `false` means UIA could not get below the page, which is the signal for the
    /// composite to ask MSAA for its own hit (docs/21 §5.19).
    pub answered_specifically: bool,
}

/// What the provider's own point hit test said about one query's position.
///
/// The "unusable" arm exists so the precision top-up can never fail silently: when verbose
/// logging is off (the normal case) a top-up that quietly did nothing looked exactly like a
/// top-up that worked, which is a diagnosis round spent on the wrong hypothesis. Every arm ends
/// up in the per-session forensics line.
enum ProviderHit {
    /// A box that belongs to the queried window. `bounds` is the part of it the user can actually
    /// see; `raw` is what the provider reported, kept for the forensics line.
    Box {
        bounds: Rect,
        raw: Rect,
        control_type: i32,
        class: String,
        /// The accessible name the page gave the box, for the forensics line (docs/21 §5.24, B6).
        name: String,
    },
    /// Nothing usable came back, and why.
    Unusable(String),
}

/// Append a hit box to the walk's published path, keeping the path a containment chain.
///
/// The hit comes from the provider's own point query, which is a different view of the page than the
/// walk that built the path; the levels that do not contain it are dropped (docs/21 §5.17) so that
/// "one level up" always lands on a box that contains the current answer.
fn push_hit_box(outcome: &mut WalkOutcome, level: PathLevel) -> bool {
    let bounds = level.rect;
    if !push_box_keeping_containment(&mut outcome.path, level) {
        outcome.stop_reason = StopReason::TraversalLimit;
        return false;
    }
    outcome.target = bounds;
    true
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
    /// Children without a usable rectangle, in provider order (see `containing_children`).
    hollow: Vec<IUIAutomationElement>,
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
            hit_test_pass_through: win32::HitTestPassThrough::default(),
            last_hit: None,
            adopt_text_runs: crate::window_detection::DEFAULT_ADOPT_TEXT_RUNS,
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
                    // `Element` **and** `Children`, as the reference selector does: with `Children`
                    // alone the element a request is applied to has no cached properties of its own,
                    // so the point hit test (docs/21 §5.7) would read no rectangle for it.
                    .and_then(|()| {
                        request.SetTreeScope(TreeScope(TreeScope_Element.0 | TreeScope_Children.0))
                    })
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
    fn expand(request: &IUIAutomationCacheRequest, parent: &IUIAutomationElement) -> ExpandedLevel {
        let mut level = ExpandedLevel {
            _parent: parent.clone(),
            children: Vec::new(),
            hollow: Vec::new(),
            stats: LevelStats::default(),
        };
        let Ok(cached) = (unsafe { parent.BuildUpdatedCache(request) }) else {
            return level;
        };
        let Ok(children) = (unsafe { cached.GetCachedChildren() }) else {
            return level;
        };
        let count = unsafe { children.Length() }.ok().unwrap_or(0).max(0) as usize;
        level.children.reserve(count);
        for index in 0..count {
            level.stats.raw += 1;
            let Ok(child) = (unsafe { children.GetElement(index as i32) }) else {
                continue;
            };
            let Some(node) = Self::cached_node(&child) else {
                level.stats.empty += 1;
                level.hollow.push(child);
                continue;
            };
            if node.offscreen {
                level.stats.offscreen += 1;
            }
            level.children.push((child, node));
        }
        level
    }

    /// Every descendable child of a node that contains `point`, topmost (last listed by the provider) first.
    ///
    /// Returning *all* containing candidates (not just the first) is what makes structural
    /// backtracking possible: when the first one dead-ends, the walk must be able to come
    /// back and try the next branch (docs/18 §12.6 ①).
    ///
    /// A child without a rectangle is not necessarily a leaf: File Explorer's navigation pane
    /// groups its tree items under nodes such as "桌面" that report `(0,0)-(0,0)` while their
    /// items have real rectangles under the cursor (measured: every point of the pane resolved to
    /// the whole pane). Such a node cannot be ranked against its siblings, so it is only looked
    /// through when nothing with a rectangle explains the point — topmost-listed first, like the
    /// rest of the walk, and through further hollow levels until a candidate turns up.
    fn containing_children(
        &mut self,
        request: &IUIAutomationCacheRequest,
        parent: &IUIAutomationElement,
        parent_bounds: Rect,
        point: Point,
        budget: &mut WalkBudget,
    ) -> Vec<(IUIAutomationElement, WalkNode)> {
        let mut candidates = Vec::new();
        let mut hollow = Vec::new();
        self.collect_level(request, parent, parent_bounds, point, budget, &mut candidates, &mut hollow);
        while candidates.is_empty() {
            let Some(next) = hollow.pop() else {
                break;
            };
            self.collect_level(request, &next, parent_bounds, point, budget, &mut candidates, &mut hollow);
        }
        candidates
    }

    /// Append one node's containing, on-screen children (topmost first) to `candidates` and its
    /// rectangle-less children to `hollow` (provider order, so `pop` takes the topmost first).
    ///
    /// Children are read from the per-window table when that level was already expanded, so
    /// moving between controls of one window only pays for the levels it has not seen yet.
    #[allow(clippy::too_many_arguments)]
    fn collect_level(
        &mut self,
        request: &IUIAutomationCacheRequest,
        parent: &IUIAutomationElement,
        parent_bounds: Rect,
        point: Point,
        budget: &mut WalkBudget,
        candidates: &mut Vec<(IUIAutomationElement, WalkNode)>,
        hollow: &mut Vec<IUIAutomationElement>,
    ) {
        // The element's own identity, never its rectangle: two nodes may share a rectangle.
        let key: NodeKey = parent.as_raw() as usize;
        // An **empty** level is not a result. Chromium materialises its accessibility tree lazily —
        // measured: a pane answers `raw=0` (or with a placeholder `Pane(0,0)-(0,0)`) and lists its
        // children a moment later — so remembering that answer would pin the whole snapshot
        // generation to "this node has no children" and every query would publish the coarse
        // ancestor. The expansion is used for *this* query and the level is re-read on the next one.
        if !self.children.contains_key(&key) {
            let expanded = Self::expand(request, parent);
            if expanded.children.is_empty() && expanded.hollow.is_empty() {
                self.log_level(parent, parent_bounds, &expanded.stats, 0);
                return;
            }
            self.children.insert(key, expanded);
        }
        let level = self
            .children
            .get(&key)
            .expect("the level was just inserted or was already cached");
        let stats = level.stats;
        let first = candidates.len();
        for (child, node) in &level.children {
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
        // Each hollow child costs a node of budget, which also bounds the look-through.
        for child in &level.hollow {
            if !budget.take_node() {
                break;
            }
            hollow.push(child.clone());
        }
        // The reference selector's order (`uia/cache.rs`: `children.hit_before(point, usize::MAX)`):
        // the child the provider lists **last** wins, i.e. the one painted on top. Ordering by area
        // instead ("smallest wins") descended into a childless dead leaf at a Chromium window root,
        // where two overlapping panes contain the cursor and the *smaller* one has no children —
        // measured: a browser page scored 3/23 fixtures with the area order and 21/23 with this one,
        // while a real File Explorer window also improved (3/25 → 12/25 points reached control level).
        candidates[first..].reverse();
        self.log_level(parent, parent_bounds, &stats, candidates.len() - first);
    }

    /// One verbose line per expanded level: the forensics that made every walk defect findable.
    fn log_level(
        &mut self,
        parent: &IUIAutomationElement,
        parent_bounds: Rect,
        stats: &LevelStats,
        containing: usize,
    ) {
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
                containing
            ),
            false,
        );
    }

    /// What the provider's own point hit test answers for `point`.
    ///
    /// The accessibility provider answers a point query with the **innermost** element there, which
    /// is exactly the "最内层可捕获区域" the product defines as the target — so this is both the
    /// yardstick and, when our walk stopped above it, a strictly finer answer we can adopt
    /// (docs/21 §5.7). Ownership is proved by walking up to the window's root element
    /// (`CompareElements`): a point hit test asks the desktop, so it could otherwise answer for
    /// whatever window happens to be on top. `NativeWindowHandle` alone is not enough — Chromium
    /// exposes it only on its outermost nodes (measured: 69 of 69 hits rejected when it was required).
    ///
    /// The capture overlay covers the desktop and would answer every query itself, so it is hidden
    /// from hit testing for the duration of the call.
    fn provider_hit(&mut self, hwnd: isize, point: Point, window_bounds: Rect) -> ProviderHit {
        let Some(automation) = self.automation().cloned() else {
            return ProviderHit::Unusable("UI Automation unavailable".into());
        };
        let root = match unsafe {
            automation.ElementFromHandle(HWND(hwnd as *mut core::ffi::c_void))
        } {
            Ok(root) => root,
            Err(error) => {
                return ProviderHit::Unusable(format!("ElementFromHandle failed: {error}"));
            }
        };
        // The overlay covers the desktop (it is what the user is looking at) and a point hit test
        // has no notion of Z order, so it would answer *us* for every query. The guard is scoped to
        // this one call: it is a real pass-through for mouse input too, and the shorter that window
        // is, the smaller the chance a click lands on the window below (docs/21 §5.7).
        let hit = {
            let _pass_through = self.hit_test_pass_through.guard();
            match unsafe {
                automation.ElementFromPoint(::windows::Win32::Foundation::POINT {
                    x: point.x,
                    y: point.y,
                })
            } {
                Ok(hit) => hit,
                Err(error) => {
                    return ProviderHit::Unusable(format!("ElementFromPoint failed: {error}"));
                }
            }
        };
        let bounds = to_rect(unsafe { hit.CurrentBoundingRectangle() }.unwrap_or_default());
        let class = unsafe { hit.CurrentClassName() }
            .map(|name| name.to_string())
            .unwrap_or_default();
        // The accessible name of the box the provider answered with (docs/21 §5.24, B6). One property
        // read on one element — the walk's nodes are not asked, so this costs nothing per level — and
        // it is the *other* half of "what does the page call this": the label shows the control type's
        // noun, the log can now show both transports' names side by side.
        let name = unsafe { hit.CurrentName() }
            .map(|name| name.to_string())
            .unwrap_or_default();
        let control_type = unsafe { hit.CurrentControlType() }
            .map(|kind| kind.0)
            .unwrap_or(0);
        if bounds.is_empty() || !bounds.contains(point) {
            return ProviderHit::Unusable(format!(
                "answer {class:?} {}x{} at ({},{}) does not cover the point",
                bounds.width(),
                bounds.height(),
                bounds.left,
                bounds.top
            ));
        }
        let Ok(walker) = (unsafe { automation.ControlViewWalker() }) else {
            return ProviderHit::Unusable("ControlViewWalker unavailable".into());
        };
        // What the user can actually capture is the part of the box that survives every ancestor:
        // Chromium reports a scrolled node's **layout** box — a chat page measured
        // `1153x22623 at (1567,-9870)` for a message column — so publishing it raw would cover the
        // whole screen, and comparing its *unclipped* area against the window made a box that is
        // finer than the window look coarser. Only a box that sticks out of the window needs that
        // work (`raw_inside_window`): reading a rectangle per ancestor is a cross-process call each,
        // so the ordinary answer must not pay for it (measured: +100 ms p95 on File Explorer's deep
        // chains).
        let mut visible = bounds;
        if bounds.intersect(window_bounds) != bounds {
            visible = bounds.intersect(window_bounds);
        }
        let mut current = hit.clone();
        for _ in 0..32 {
            if unsafe { automation.CompareElements(&current, &root) }
                .map(|equal| equal.as_bool())
                .unwrap_or(false)
            {
                return ProviderHit::Box {
                    bounds: visible,
                    raw: bounds,
                    control_type,
                    class,
                    name,
                };
            }
            match unsafe { walker.GetParentElement(&current) } {
                Ok(parent) => {
                    let parent_bounds =
                        to_rect(unsafe { parent.CurrentBoundingRectangle() }.unwrap_or_default());
                    // Shrink only while the result still covers the cursor: a virtualised list item
                    // can report an empty or stale rectangle (docs/18 §12.7), and an ancestor that
                    // disagrees with the hit test must not be allowed to clip the answer away.
                    if !parent_bounds.is_empty() {
                        let clipped = visible.intersect(parent_bounds);
                        if !clipped.is_empty() && clipped.contains(point) {
                            visible = clipped;
                        }
                    }
                    current = parent;
                }
                Err(_) => break,
            }
        }
        ProviderHit::Unusable(format!(
            "answer {class:?} {}x{} is not a descendant of hwnd={hwnd}",
            bounds.width(),
            bounds.height()
        ))
    }

    /// Borrow the flag that lets the point hit test fall through the capture overlay (docs/21 §5.7).
    pub fn with_hit_test_pass_through(mut self, pass_through: win32::HitTestPassThrough) -> Self {
        self.hit_test_pass_through = pass_through;
        self
    }

    /// Take what the point hit test decided for the last query (docs/21 §5.16).
    ///
    /// The composite provider records one decision per query, because it is also the side that can
    /// consult MSAA; taking this leaves `None` behind, so a later query cannot inherit it.
    pub(crate) fn take_hit_decision(&mut self) -> Option<HitDecision> {
        self.last_hit.take()
    }

    /// Whether a bare text run may become the answer (docs/21 §5.19).
    pub(crate) fn with_adopt_text_runs(mut self, adopt_text_runs: bool) -> Self {
        self.adopt_text_runs = adopt_text_runs;
        self
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
                // A `Text` run is the glyphs inside the element it labels, not the element the user
                // means (measured: a 168x56 `Hyperlink` with a 56x20 `Text` child; a `role=group`
                // span likewise). Stop on the element that owns it — `stack.last()` is the node the
                // candidates are children of, and the root has no control type, so the window frame
                // can never claim a run.
                if let Some(parent) = stack.last().map(|level| level.node)
                    && is_text_run_inside_element(parent, candidates[0].1)
                {
                    break 'walk;
                }
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
        // Publish the *visible* part of each accepted level: clipped by the level above it, which is
        // what a scrollport does to a node that is taller than it (see `provider_hit`). For a box
        // that already sits inside its parent this is the box itself, so nothing else changes.
        let mut visible_parent = window_bounds;
        for level in &stack {
            let visible = level.node.bounds.intersect(visible_parent);
            if visible.is_empty() {
                continue;
            }
            // The level carries what the node *is* (docs/21 §5.24, B6): the walk read the control
            // type anyway, and dropping it here is what used to leave every ancestor a "container".
            if !outcome.push(PathLevel::new(
                visible,
                level_kind_of_control_type(level.node.control_type),
            )) {
                break;
            }
            visible_parent = visible;
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
        outcome.target = outcome
            .path
            .last()
            .map(|level| level.rect)
            .unwrap_or(window_bounds);

        // **Precision top-up** (docs/21 §5.7). The provider answers a point query with the innermost
        // element there by construction; when our walk stopped above it, adopt that box. The rule is
        // a strict refinement — the hit must belong to this window (checked inside `provider_hit`) and
        // still contain the cursor — so it can only make the answer finer. Measured before it: the
        // browser fixtures had 4 of 43 sampling points where the provider was finer, File Explorer
        // none out of 25 (so Explorer is provably unaffected).
        //
        // Every arm is recorded, not just the successful one: "the top-up ran and did nothing" is
        // indistinguishable from "the top-up did not run" in a non-verbose log, and that ambiguity
        // is what a user reporting "the elements inside this box are not recognized" runs into.
        let walk = outcome.target;
        match self.provider_hit(job.window.hwnd, job.point, window_bounds) {
            ProviderHit::Box {
                bounds: hit,
                raw,
                control_type,
                class,
                name,
            } => {
                let finer = should_adopt_provider_box(
                    walk,
                    hit,
                    control_type,
                    job.point,
                    self.adopt_text_runs,
                );
                let adopted = finer
                    && push_hit_box(
                        &mut outcome,
                        PathLevel::new(hit, level_kind_of_control_type(control_type)),
                    );
                // Did this transport get below the page? A document or window-sized answer says no,
                // and that is what tells the composite to ask MSAA (docs/21 §5.19).
                let answered_specifically =
                    !is_unspecific_hit(hit, control_type, window_bounds);
                let mut note = format!(
                    "provider={}x{} at ({},{})",
                    raw.width(),
                    raw.height(),
                    raw.left,
                    raw.top
                );
                if hit != raw {
                    // Only worth printing when the ancestors trimmed it: this is the line that says
                    // "the provider answered a layout box that runs off the window".
                    note.push_str(&format!(
                        " visible={}x{} at ({},{})",
                        hit.width(),
                        hit.height(),
                        hit.left,
                        hit.top
                    ));
                }
                note.push_str(&format!(
                    " type={control_type} {} class={class:?} name={:?} walk={}x{} at ({},{})",
                    level_kind_of_control_type(control_type).debug_name(),
                    name.chars().take(60).collect::<String>(),
                    walk.width(),
                    walk.height(),
                    walk.left,
                    walk.top
                ));
                if adopted {
                    self.last_hit = Some(HitDecision {
                        outcome: PrecisionOutcome::Adopted,
                        note,
                        answered_specifically,
                    });
                    self.metrics.log_line(
                        &format!("refinement adopted provider box={hit:?} (walk was coarser)"),
                        false,
                    );
                } else if finer {
                    // Finer, but the path is full: the answer stays as the walk left it.
                    self.last_hit = Some(HitDecision {
                        outcome: PrecisionOutcome::Unavailable,
                        note: format!("{note}: the hit could not be appended to the path"),
                        answered_specifically,
                    });
                } else {
                    self.last_hit = Some(HitDecision {
                        outcome: PrecisionOutcome::NotFiner,
                        note,
                        answered_specifically,
                    });
                }
            }
            ProviderHit::Unusable(why) => {
                self.last_hit = Some(HitDecision {
                    outcome: PrecisionOutcome::Unavailable,
                    note: why,
                    // Nothing was answered at all, so there is nothing specific to prefer.
                    answered_specifically: false,
                });
            }
        }
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
    // `UiElement` only when the walk actually got below the window frame; a
    // window-only answer keeps saying "whole frame" so the overlay renders it exactly like a
    // window-snap target.
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
mod tests;
