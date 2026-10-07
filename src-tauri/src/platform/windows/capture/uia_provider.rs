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

use crate::capture::geometry::{Point, Rect};
use crate::capture::diagnostics::{PrecisionOutcome, WindowDetectionMetrics};
use crate::capture::window_detection::deep::{
    DeepSelectionProvider, DeepTarget, QueryControl, RefinementJob, RefinementOutcome,
    StopReason,
};
use crate::capture::window_detection::model::{PathLevel, SnapshotEpoch, TargetKind};
use crate::capture::window_detection::uia::{
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
            adopt_text_runs: crate::capture::window_detection::DEFAULT_ADOPT_TEXT_RUNS,
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
    // Only the source-comparison probe needs these: the raw view and the document's text pattern.
    use ::windows::Win32::UI::Accessibility::{
        IUIAutomationTextPattern, TreeScope_Descendants, UIA_TextPatternId,
    };
    use ::windows::Win32::UI::WindowsAndMessaging::{
        DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, SW_SHOWNA, ShowWindow,
        TranslateMessage, WS_POPUP, WS_VISIBLE, CreateWindowExW, WINDOW_EX_STYLE,
        GWL_EXSTYLE, GetSystemMetrics, HWND_TOPMOST, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE,
        SWP_SHOWWINDOW, SetWindowLongPtrW, SetWindowPos, WS_EX_LAYERED, WS_EX_NOREDIRECTIONBITMAP,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
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

    /// What the stand-in's owning thread applies to its window.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum OverlayStyle {
        /// Exactly the flags the capture overlay is created with (docs/14 §7).
        Visible,
        /// Plus `WS_EX_TRANSPARENT`: what the product sets for the duration of a hit test
        /// (docs/21 §5.7).
        Transparent,
        /// Plus `WS_EX_LAYERED` as well — the combination that is click-through for real, and
        /// therefore the fallback if the transparent flag alone is not enough.
        LayeredTransparent,
    }

    /// The capture overlay's window shape on its own thread, without a renderer.
    ///
    /// A point hit test has no notion of Z order, so "who answers while our overlay covers the
    /// desktop?" is a question about style, visibility and stacking, not about pixels: a window
    /// with the overlay's own flags models it faithfully. Created and re-styled by its own thread,
    /// as the product does.
    struct OverlayStandIn {
        window: isize,
        /// Shared with the stand-in's window procedure. The probe takes a guard from it exactly as
        /// the refinement worker does in the product, so the lab exercises the real read path.
        pass_through: win32::HitTestPassThrough,
        mode: std::sync::Arc<std::sync::atomic::AtomicU8>,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    /// Class name and title of the stand-in's own window class.
    ///
    /// The class matters: a stand-in on the system `STATIC` class was skipped by
    /// `ElementFromPoint` even while it was the topmost window, the foreground window and
    /// `WindowFromPoint`'s own answer — so UIA is not asking the window manager, and a system
    /// class is not the same animal as the overlay's registered class.
    const STAND_IN_CLASS: ::windows::core::PCWSTR = w!("SnapClipOverlayHitTestStandIn");
    const STAND_IN_TITLE: ::windows::core::PCWSTR = w!("SnapClip overlay hit-test stand-in");

    thread_local! {
        /// The stand-in thread's copy of the pass-through flag, exactly as the overlay's window
        /// procedure holds one: a worker thread sets the flag, the owning thread reads it here.
        static STAND_IN_PASS_THROUGH: std::cell::RefCell<win32::HitTestPassThrough> =
            std::cell::RefCell::new(win32::HitTestPassThrough::default());
    }

    /// Replace the stand-in thread's pass-through flag (called on the thread that owns the window).
    fn set_stand_in_pass_through(flag: win32::HitTestPassThrough) {
        STAND_IN_PASS_THROUGH.with(|slot| *slot.borrow_mut() = flag);
    }

    /// Whether the stand-in's window procedure should let the hit test through.
    ///
    /// `try_borrow`, because this runs inside a window procedure: a reentrant `WM_NCHITTEST` must
    /// answer "the overlay owns the point", never panic.
    fn stand_in_pass_through_active() -> bool {
        STAND_IN_PASS_THROUGH
            .with(|slot| slot.try_borrow().map(|flag| flag.is_active()).unwrap_or(false))
    }

    unsafe extern "system" fn stand_in_proc(
        window: ::windows::Win32::Foundation::HWND,
        message: u32,
        wparam: ::windows::Win32::Foundation::WPARAM,
        lparam: ::windows::Win32::Foundation::LPARAM,
    ) -> ::windows::Win32::Foundation::LRESULT {
        use ::windows::Win32::UI::WindowsAndMessaging::{
            DefWindowProcW, HTCLIENT, HTTRANSPARENT, WM_NCHITTEST,
        };
        if message == WM_NCHITTEST {
            return if stand_in_pass_through_active() {
                ::windows::Win32::Foundation::LRESULT(HTTRANSPARENT as isize)
            } else {
                ::windows::Win32::Foundation::LRESULT(HTCLIENT as isize)
            };
        }
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    impl OverlayStandIn {
        /// `None` when the window or its thread could not be created.
        fn create() -> Option<Self> {
            use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
            let mode = std::sync::Arc::new(AtomicU8::new(OverlayStyle::Visible as u8));
            let stop = std::sync::Arc::new(AtomicBool::new(false));
            // Created here, cloned into the stand-in thread: the two sides of the product's flag,
            // one seen by the caller and one read by the window procedure.
            let pass_through = win32::HitTestPassThrough::default();
            let (ready_tx, ready_rx) = std::sync::mpsc::channel::<isize>();
            let thread = {
                let mode = std::sync::Arc::clone(&mode);
                let stop = std::sync::Arc::clone(&stop);
                let thread_pass_through = pass_through.clone();
                std::thread::spawn(move || {
                    set_stand_in_pass_through(thread_pass_through);
                    let width = unsafe { GetSystemMetrics(SM_CXSCREEN) }.max(1);
                    let height = unsafe { GetSystemMetrics(SM_CYSCREEN) }.max(1);
                    // A registered class, exactly like the overlay's: see STAND_IN_CLASS.
                    let window_class = ::windows::Win32::UI::WindowsAndMessaging::WNDCLASSW {
                        lpfnWndProc: Some(stand_in_proc),
                        lpszClassName: STAND_IN_CLASS,
                        ..Default::default()
                    };
                    unsafe {
                        ::windows::Win32::UI::WindowsAndMessaging::RegisterClassW(&window_class)
                    };
                    let created = unsafe {
                        CreateWindowExW(
                            WINDOW_EX_STYLE(
                                (WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP).0,
                            ),
                            STAND_IN_CLASS,
                            STAND_IN_TITLE,
                            WS_POPUP | WS_VISIBLE,
                            0,
                            0,
                            width,
                            height,
                            None,
                            None,
                            None,
                            None,
                        )
                    };
                    let Ok(window) = created else {
                        let _ = ready_tx.send(0);
                        return;
                    };
                    unsafe {
                        let _ = SetWindowPos(
                            window,
                            Some(HWND_TOPMOST),
                            0,
                            0,
                            width,
                            height,
                            SWP_NOACTIVATE | SWP_SHOWWINDOW,
                        );
                    }
                    let _ = ready_tx.send(window.0 as isize);
                    let base = (WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP).0
                        as isize;
                    let mut applied = OverlayStyle::Visible;
                    let mut message = MSG::default();
                    while !stop.load(Ordering::Relaxed) {
                        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                            let _ = unsafe { TranslateMessage(&message) };
                            unsafe { DispatchMessageW(&message) };
                        }
                        let wanted = match mode.load(Ordering::Relaxed) {
                            value if value == OverlayStyle::Transparent as u8 => {
                                OverlayStyle::Transparent
                            }
                            value if value == OverlayStyle::LayeredTransparent as u8 => {
                                OverlayStyle::LayeredTransparent
                            }
                            _ => OverlayStyle::Visible,
                        };
                        if wanted != applied {
                            let style = match wanted {
                                OverlayStyle::Visible => base,
                                OverlayStyle::Transparent => base | WS_EX_TRANSPARENT.0 as isize,
                                OverlayStyle::LayeredTransparent => {
                                    base | WS_EX_TRANSPARENT.0 as isize | WS_EX_LAYERED.0 as isize
                                }
                            };
                            unsafe { SetWindowLongPtrW(window, GWL_EXSTYLE, style) };
                            applied = wanted;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    let _ = unsafe { DestroyWindow(window) };
                })
            };
            match ready_rx.recv_timeout(Duration::from_secs(10)) {
                Ok(window) if window != 0 => Some(Self {
                    window,
                    pass_through,
                    mode,
                    stop,
                    thread: Some(thread),
                }),
                _ => {
                    stop.store(true, Ordering::Relaxed);
                    let _ = thread.join();
                    None
                }
            }
        }

        /// Hand the owning thread a new style and let it commit before the next hit test.
        fn set_style(&self, style: OverlayStyle) {
            self.mode
                .store(style as u8, std::sync::atomic::Ordering::Relaxed);
            pump(150);
        }

        fn hwnd(&self) -> isize {
            self.window
        }

        /// Lever 2: cut a few pixels out of the window's *region* around `point`.
        ///
        /// A window region is what defines a shaped window for hit testing, so if anything in the
        /// hit-test chain honours the shape, the point falls through to the page. Screen and window
        /// coordinates coincide here: the stand-in sits at (0,0) and covers the primary screen.
        fn punch_hole(&self, point: Point) {
            use windows_sys::Win32::Graphics::Gdi::{
                CombineRgn, CreateRectRgn, DeleteObject, RGN_DIFF, SetWindowRgn,
            };
            const HOLE: i32 = 2;
            unsafe {
                let full = CreateRectRgn(i32::MIN, i32::MIN, i32::MAX, i32::MAX);
                let hole = CreateRectRgn(
                    point.x - HOLE,
                    point.y - HOLE,
                    point.x + HOLE,
                    point.y + HOLE,
                );
                CombineRgn(full, full, hole, RGN_DIFF);
                DeleteObject(hole);
                SetWindowRgn(self.window as *mut core::ffi::c_void, full, 1);
            }
            pump(250);
        }

        /// Drop the region again (`hwnd, NULL` = "no shape").
        fn heal(&self) {
            use windows_sys::Win32::Graphics::Gdi::SetWindowRgn;
            unsafe {
                SetWindowRgn(self.window as *mut core::ffi::c_void, std::ptr::null_mut(), 1);
            }
            pump(250);
        }
    }

    impl Drop for OverlayStandIn {
        fn drop(&mut self) {
            self.stop
                .store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
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
        let worker = RefinementWorker::new(
            0,
            win32::HitTestPassThrough::default(),
            crate::capture::window_detection::DEFAULT_ADOPT_TEXT_RUNS,
            metrics,
        );
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
        assert_eq!(
            target.path[0].rect, bounds,
            "the path always starts at the window frame"
        );
        assert_eq!(
            target.path.last().map(|level| level.rect),
            Some(target.screen_bounds)
        );
        assert!(!target.screen_bounds.is_empty());
        // The published rectangle must be inside the window's visible area.
        assert!(!target.screen_bounds.intersect(bounds).is_empty());
    }

    /// An empty expansion must not be remembered (docs/21 §5.6).
    ///
    /// Chromium materialises its accessibility tree lazily: a node answers `raw=0` (or with the
    /// placeholder `Pane(0,0)-(0,0)`) and lists its children a moment later. Caching that answer
    /// pinned the whole snapshot generation to "this node has no children", so every query published
    /// the coarse ancestor — one of the "sometimes stays on the parent box" causes. Reading it again
    /// on the next query is what makes the walk recover on its own.
    #[test]
    fn an_empty_uia_level_is_never_remembered() {
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
        assert!(
            matches!(outcome, RefinementOutcome::Target(_)),
            "a live window must still resolve, got {outcome:?}"
        );
        let empty = provider
            .children
            .values()
            .filter(|level| level.children.is_empty() && level.hollow.is_empty())
            .count();
        assert_eq!(
            empty, 0,
            "a level that listed nothing must be re-read, not cached for the generation"
        );
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
        /// The answer must be **strictly inside** this fixture's own box, not the box itself: the
        /// assertion for "the text run under the cursor was adopted" (docs/21 §5.19).
        #[serde(default)]
        finer_than_self: bool,
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
        // The cross-origin frame's URL comes from a loopback server this test runs (docs/21 §5.20).
        let url = match serve_cross_origin_fixture() {
            Some(port) => format!(
                "file:///{}?truth=1&cross=127.0.0.1:{port}",
                fixture_path.display().to_string().replace('\\', "/")
            ),
            None => format!(
                "file:///{}?truth=1",
                fixture_path.display().to_string().replace('\\', "/")
            ),
        };
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
        // The cross-origin frame measures itself and reports over `postMessage` — its parent cannot
        // read it. That report is also the only proof the frame's content ever loaded, so the probe
        // refuses to measure without it: a still-blank frame answers the frame node for every point
        // inside it, which is indistinguishable from "cross-origin content is unreachable". That is
        // the wrong conclusion this fixture produced before the report existed (docs/21 §5.20).
        let mut truth = truth;
        if url.contains("cross=") {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while !truth.contains_key("cross-ready") && std::time::Instant::now() < deadline {
                pump(250);
                if let Some(refreshed) = truth_of(&probe_title(hwnd)) {
                    truth = refreshed;
                }
            }
            if !truth.contains_key("cross-ready") {
                let _ = child.kill();
                panic!("the cross-origin frame never reported that its content loaded");
            }
        }
        println!(
            "[probe] page published {} boxes; frame/shadow ones: {:?}; cross-ready={:?}",
            truth.len(),
            truth
                .keys()
                .filter(|id| id.contains("iframe") || id.contains("shadow"))
                .collect::<Vec<_>>(),
            truth.get("cross-ready")
        );
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
        let mut provider = UiaDeepSelectionProvider::new(metrics.clone());
        // The assertion loop resolves through the **product's** provider chain, not the UIA provider
        // alone: the MSAA second opinion lives in the composite (docs/21 §5.16), and a gate that
        // skipped it would report the old behaviour no matter what the product does.
        let mut pipeline = super::super::refinement_worker::FallbackDeepSelection::new(
            metrics.clone(),
            win32::HitTestPassThrough::default(),
            crate::capture::window_detection::DEFAULT_ADOPT_TEXT_RUNS,
        );
        let walk = provider.automation().cloned().and_then(|auto| RawWalk::new(&auto));
        let hit_owner = provider.automation().cloned();
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
        // Per-query latency of the **product** walk (backtracking, hollow look-through and the
        // child-window check all run here). The refinement budget is 1500 ms per query.
        let mut latencies: Vec<f64> = Vec::new();
        // Precision yardstick: how often the provider's own point hit test answers with a strictly
        // smaller box that still contains the point — a place our walk stopped above the innermost
        // capturable element.
        let mut provider_available = 0_usize;
        let mut provider_finer = 0_usize;
        // A finer box that was *not* adopted, and hit tests that answered nothing usable: both
        // mean the precision top-up is not doing its job, and both were invisible while the probe
        // only counted.
        let mut finer_not_adopted: Vec<String> = Vec::new();
        let mut provider_unusable: Vec<String> = Vec::new();
        // Published boxes that stick out of the window. A screenshot can only contain what is on
        // screen, so this is a gate for the layout-box shape (a tall node inside a scrollport)
        // whichever side produced the box - the walk or the provider's hit test.
        let mut off_window: Vec<String> = Vec::new();
        // Published paths that are not a containment chain. The ancestor walk (docs/21 §5.17) steps
        // through `path`, so a level that does not contain the next one would make "one level up"
        // jump to a box that does not contain the answer.
        let mut broken_chains: Vec<String> = Vec::new();
        // Rows that must resolve *below* their own box: the text-run adoption (docs/21 §5.19).
        let mut finer_than_self_failures: Vec<String> = Vec::new();
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
                pipeline.release();
            }
            // Bounded retries: Chromium builds its accessibility tree lazily, so the first query
            // for a window can legitimately answer "nothing below the page" and a later one finds
            // the element. The product re-queries on every dwell, so the *eventual* answer is what
            // the user sees; measuring a first try would report a readiness artefact as a defect.
            let mut attempts = 0_usize;
            let mut published = None;
            let mut last_stop = None;
            let mut last_depth = 0_usize;
            let mut last_path: Option<Vec<PathLevel>> = None;
            for attempt in 0..4 {
                if attempt > 0 {
                    pump(250);
                }
                attempts += 1;
                let query_started = std::time::Instant::now();
                let outcome = pipeline
                    .resolve(&job(hwnd, point), frame, &QueryControl::refinement(&|| false));
                latencies.push(query_started.elapsed().as_secs_f64() * 1000.0);
                published = match &outcome {
                    RefinementOutcome::Target(target) => {
                        last_stop = Some(target.stop_reason);
                        last_depth = target.path.len();
                        last_path = Some(target.path.clone());
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
            if let Some(note) = metrics.last_precision() {
                // The composite's own decision for this query, uia/msaa both: the counters say
                // *whether* a finer answer was taken, this says what the transports answered.
                println!("[probe]   decision {}: {note}", fixture.id);
            }
            // The provider's own hit test, through the **production** code path, so the gate below
            // covers the mechanism the product actually runs (`provider_hit` + the adoption rule)
            // rather than a probe-local copy of it.
            match provider.provider_hit(hwnd, point, frame) {
                ProviderHit::Box {
                    bounds: hit_bounds,
                    control_type: hit_kind,
                    ..
                } => {
                    provider_available += 1;
                    let published_area = published
                        .map(|rect| i64::from(rect.width()) * i64::from(rect.height()))
                        .unwrap_or(i64::MAX);
                    let hit_area = i64::from(hit_bounds.width()) * i64::from(hit_bounds.height());
                    if hit_area < published_area {
                        provider_finer += 1;
                        println!(
                            "[probe]   provider is finer on {}: {}x{} at ({},{}) type={hit_kind} \
                             (walk {})",
                            fixture.id,
                            hit_bounds.width(),
                            hit_bounds.height(),
                            hit_bounds.left,
                            hit_bounds.top,
                            published
                                .map(|rect| format!("{}x{}", rect.width(), rect.height()))
                                .unwrap_or_else(|| "none".into())
                        );
                        // With the text-run preference on (docs/21 §5.19) the old exemption is gone:
                        // any strictly finer hit, text run included, has to have become the answer.
                        let exempt = !crate::capture::window_detection::DEFAULT_ADOPT_TEXT_RUNS
                            && crate::capture::window_detection::is_bare_text_control_type(hit_kind);
                        if !exempt {
                            finer_not_adopted.push(fixture.id.clone());
                        }
                    }
                }
                ProviderHit::Unusable(why) => provider_unusable.push(format!("{}: {why}", fixture.id)),
            }
            let expected = match fixture.expect.as_str() {
                "none" => None,
                "self" | "inside_self" | "covers_self" => Some((fixture.id.as_str(), own)),
                // `within:<id>`: the answer must sit inside the referenced fixture's box. Used where
                // the accessibility tree's own granularity is not stable from run to run — the
                // cross-origin frame answers either its own node or the button inside it, and both
                // are correct (docs/21 §5.20).
                other if other.starts_with("within:") => {
                    let target = &other["within:".len()..];
                    truth
                        .get(target)
                        .copied()
                        .map(|measured| (target, measured))
                }
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
            if let Some(rect) = published
                && rect.intersect(frame) != rect
            {
                off_window.push(format!(
                    "{}: {rect:?} is not inside the window {frame:?}",
                    fixture.id
                ));
            }
            if let Some(path) = &last_path {
                // Two pixels of slack: the fixture's own measured box and the accessibility tree's
                // rectangle disagree by one pixel in places, and this gate is about the *shape* of
                // the chain, not about that measurement gap.
                const SLACK: i32 = 2;
                let slack = |outer: Rect, inner: Rect| {
                    inner.left >= outer.left - SLACK
                        && inner.top >= outer.top - SLACK
                        && inner.right <= outer.right + SLACK
                        && inner.bottom <= outer.bottom + SLACK
                };
                if path.is_empty() {
                    broken_chains.push(format!("{}: empty path", fixture.id));
                }
                if let Some(pair) = path
                    .windows(2)
                    .find(|pair| !slack(pair[0].rect, pair[1].rect))
                {
                    broken_chains.push(format!(
                        "{}: {:?} does not contain {:?} (path: {path:?})",
                        fixture.id, pair[0], pair[1]
                    ));
                }
                if path.last().map(|level| level.rect) != published {
                    broken_chains.push(format!(
                        "{}: path ends at {:?}, published {:?}",
                        fixture.id,
                        path.last(),
                        published
                    ));
                }
            }
            if fixture.finer_than_self {
                let own_box = Rect::new(
                    viewport.left + own[0],
                    viewport.top + own[1],
                    viewport.left + own[0] + own[2],
                    viewport.top + own[1] + own[3],
                );
                let ok = published
                    .is_some_and(|rect| own_box.contains_rect(rect) && rect.area() < own_box.area());
                if !ok {
                    finer_than_self_failures.push(format!(
                        "{}: published {:?} is not strictly inside its own box {own_box:?}",
                        fixture.id, published
                    ));
                }
            }
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

        // --- Does our own overlay hide the page from the provider's hit test? ---
        //
        // The precision top-up (docs/21 §5.7) asks `ElementFromPoint` what is under the cursor,
        // and the product asks that with the capture overlay covering the desktop. This phase
        // builds the same shape over the live fixture — a topmost, full-screen, tool-window overlay
        // owned by another thread — and reports who answers in four states. It is the measurement
        // §6 row 9 made from a branch that was rolled back, which is why the product-side failure it
        // would explain ("the top-up works here and does nothing there") went unseen for a round.
        if let Some(automation) = hit_owner.as_ref() {
            let sample = manifest
                .iter()
                .filter_map(|fixture| {
                    let measured = *truth.get(&fixture.id)?;
                    (measured[2] > 0 && measured[3] > 0).then(|| {
                        let [dx, dy] = fixture.probe.unwrap_or([measured[2] / 2, measured[3] / 2]);
                        (
                            fixture.id.clone(),
                            Point::new(
                                viewport.left + measured[0] + dx,
                                viewport.top + measured[1] + dy,
                            ),
                            Rect::new(
                                viewport.left + measured[0],
                                viewport.top + measured[1],
                                viewport.left + measured[0] + measured[2],
                                viewport.top + measured[1] + measured[3],
                            ),
                        )
                    })
                })
                .next();
            if let Some((id, point, expected)) = sample {
                // Returns whether the hit test answered our own stand-in: that is the state the
                // product is in whenever it asks with the overlay up.
                let answer = |label: &str| -> bool {
                    let hit = unsafe {
                        automation.ElementFromPoint(::windows::Win32::Foundation::POINT {
                            x: point.x,
                            y: point.y,
                        })
                    };
                    let (uia, ours) = match hit {
                        Ok(element) => {
                            let class = unsafe { element.CurrentClassName() }
                                .map(|name| name.to_string())
                                .unwrap_or_default();
                            let kind = unsafe { element.CurrentControlType() }
                                .map(|kind| kind.0)
                                .unwrap_or(0);
                            let bounds = to_rect(
                                unsafe { element.CurrentBoundingRectangle() }.unwrap_or_default(),
                            );
                            (
                                format!(
                                    "{}({},{})-({},{}) type={kind} class={class:?}",
                                    control_type_name(kind),
                                    bounds.left,
                                    bounds.top,
                                    bounds.right,
                                    bounds.bottom
                                ),
                                class == "SnapClipOverlayHitTestStandIn",
                            )
                        }
                        Err(error) => (format!("ElementFromPoint failed: {error}"), false),
                    };
                    // What the *window manager* thinks is at the point, and who owns the
                    // foreground: UIA answers on the page while the overlay is up, and the
                    // difference between the two answers is what says how to fix it.
                    let window_at = unsafe {
                        ::windows::Win32::UI::WindowsAndMessaging::WindowFromPoint(
                            ::windows::Win32::Foundation::POINT {
                                x: point.x,
                                y: point.y,
                            },
                        )
                    };
                    let foreground = unsafe {
                        ::windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow()
                    };
                    println!(
                        "[overlay] {label}: UIA={uia} | WindowFromPoint={:?} | foreground={:?}",
                        describe_window(window_at.0 as isize),
                        describe_window(foreground.0 as isize)
                    );
                    // MSAA is a separate transport with its own hit test, so it needs its own
                    // answer to "does the overlay hide this one too?".
                    println!("[overlay] {label}: MSAA={}", report_msaa_hit(point));
                    ours
                };
                println!(
                    "[overlay] sample fixture={id} point=({},{}) fixture_box={}x{} at ({},{})",
                    point.x,
                    point.y,
                    expected.width(),
                    expected.height(),
                    expected.left,
                    expected.top
                );
                let page_answers = !answer("no overlay");
                assert!(
                    page_answers,
                    "the page must answer before any overlay exists, or this phase measures nothing"
                );
                match OverlayStandIn::create() {
                    Some(overlay) => {
                        println!(
                            "[overlay] stand-in window: {}",
                            describe_window(overlay.hwnd())
                        );
                        pump(300);
                        // The reproduction, and the reason this phase exists: a window carrying the
                        // overlay's shape *and* its own registered class is what UIA answers. A
                        // stand-in on a system class is skipped instead (measured), which is how the
                        // product-only failure stayed invisible for a round.
                        assert!(
                            answer("overlay topmost"),
                            "the stand-in must be what the hit test answers, or this phase no \
                             longer models the product"
                        );
                        overlay.set_style(OverlayStyle::Transparent);
                        let style_helped = !answer("overlay +WS_EX_TRANSPARENT");
                        println!(
                            "[overlay] verdict: WS_EX_TRANSPARENT alone {}",
                            if style_helped {
                                "lets the hit test through"
                            } else {
                                "does NOT let the hit test through"
                            }
                        );
                        overlay.set_style(OverlayStyle::LayeredTransparent);
                        answer("overlay +WS_EX_LAYERED|TRANSPARENT");
                        overlay.set_style(OverlayStyle::Visible);
                        let pass_through = overlay.pass_through.guard();
                        pump(150);
                        // The lever the product pulls (docs/21 §5.7), through the product's own
                        // plumbing: the guard is taken here, the stand-in's window procedure reads
                        // the flag. If Windows ever stops honouring it the precision top-up silently
                        // dies again, so fail here instead.
                        assert!(
                            !answer("overlay +HTTRANSPARENT on WM_NCHITTEST"),
                            "HTTRANSPARENT must let the hit test through to the page, or the \
                             precision top-up does nothing in the product"
                        );
                        drop(pass_through);
                        pump(150);
                        // …and the guard puts it back: the overlay owns the hit test again the
                        // moment the accessibility call it wrapped is over.
                        assert!(
                            answer("overlay after the pass-through guard is dropped"),
                            "the flag must be cleared again, or the overlay stops taking clicks"
                        );
                        overlay.punch_hole(point);
                        answer("overlay + a hole in its region at the point");
                        overlay.heal();
                        answer("overlay visible again");
                    }
                    None => println!("[overlay] the stand-in window could not be created"),
                }
            }
        }

        // Which source could give the user the box they are pointing at? The control view is what
        // the product uses; a layout-only `<div>` is the case that decides whether the raw view or
        // the document's TextPattern is worth more (docs/21 §10).
        if let Some(automation) = hit_owner.as_ref() {
            for id in [
                "plain-div",
                "meter-shell",
                "deep-item",
                "checkbox",
                "radio",
                "para",
                "code-box",
                "table-cell-1",
                "code-run",
                "code-pane",
            ] {
                let Some(measured) = truth.get(id).copied() else {
                    continue;
                };
                if measured[2] <= 0 || measured[3] <= 0 {
                    continue;
                }
                // Manifest entries carry their probe offset; ids that only exist inside a fixture
                // (a `pre` or a text run, for instance) are sampled at their centre.
                let [dx, dy] = manifest
                    .iter()
                    .find(|fixture| fixture.id == id)
                    .and_then(|fixture| fixture.probe)
                    .unwrap_or([measured[2] / 2, measured[3] / 2]);
                let point = Point::new(
                    viewport.left + measured[0] + dx,
                    viewport.top + measured[1] + dy,
                );
                report_sources(automation, hwnd, id, point);
            }
        }

        let _ = child.kill();
        println!(
            "[probe] asserted={asserted} passed={passed} failed={} slow_fixtures={retries}",
            asserted - passed,
        );
        println!(
            "[probe] precision: provider_hit_available={provider_available} \
             provider_hit_is_finer_on={provider_finer}"
        );
        for (label, offenders) in [
            ("NOT ADOPTED (the provider was finer and we published the coarser box)", &finer_not_adopted),
            ("UNUSABLE HIT TEST", &provider_unusable),
        ] {
            for offender in offenders {
                println!("[probe] {label}: {offender}");
            }
        }
        print_latency_summary("probe", &mut latencies);
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
        // The precision top-up and the window invariant are gates too, not counters: a strictly
        // finer box that we failed to adopt, a hit test that answered nothing usable, or a published
        // box that no screenshot could contain are each a real defect that used to be a printed line.
        assert!(
            finer_not_adopted.is_empty(),
            "{} sampling points published a coarser box although the provider's hit test was \
             strictly finer: {finer_not_adopted:#?}",
            finer_not_adopted.len()
        );
        assert!(
            provider_unusable.is_empty(),
            "{} sampling points could not use the provider's hit test at all: {provider_unusable:#?}",
            provider_unusable.len()
        );
        assert!(
            off_window.is_empty(),
            "{} published boxes extend outside the window, so no screenshot could contain them: \
             {off_window:#?}",
            off_window.len()
        );
        // The ancestor walk steps through `path`, so the chain has to hold everywhere, not only on
        // the rows that were asserted.
        assert!(
            broken_chains.is_empty(),
            "{} published paths are not containment chains ending at the published box: \
             {broken_chains:#?}",
            broken_chains.len()
        );
        assert!(
            finer_than_self_failures.is_empty(),
            "{} rows did not resolve to something strictly inside their own box, so the text run \
             under the cursor was not adopted: {finer_than_self_failures:#?}",
            finer_than_self_failures.len()
        );
    }

    /// Compare what we published against what the page measured.
    fn judge(expect: &str, expected: Option<Rect>, published: Option<Rect>) -> (bool, &'static str) {
        const TOLERANCE: i32 = 3;
        // `within:<id>` (see the expectation lookup): containment instead of equality, because the
        // accessibility tree's granularity inside a cross-origin frame is not stable run to run.
        if expect.starts_with("within:") {
            return match (expected, published) {
                (Some(expected), Some(published)) => {
                    let inside = published.left >= expected.left - TOLERANCE
                        && published.top >= expected.top - TOLERANCE
                        && published.right <= expected.right + TOLERANCE
                        && published.bottom <= expected.bottom + TOLERANCE;
                    (inside, "the answer must sit inside the referenced box")
                }
                (_, None) => (false, "nothing was published"),
                (None, _) => (false, "no expectation to compare"),
            };
        }
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

    /// `Type(x,y)-(x,y) class="…" control=… content=…` for one element.
    fn describe_element(element: &IUIAutomationElement) -> String {
        let kind = unsafe { element.CurrentControlType() }
            .map(|kind| kind.0)
            .unwrap_or(0);
        let rect = to_rect(unsafe { element.CurrentBoundingRectangle() }.unwrap_or_default());
        let class = unsafe { element.CurrentClassName() }
            .map(|name| name.to_string())
            .unwrap_or_default();
        // The accessible name is the property the product's label would show (docs/21 §5.24 B6), so
        // every dump that already describes an element says what the page calls it.
        let name = unsafe { element.CurrentName() }
            .map(|name| name.to_string())
            .unwrap_or_default();
        let control = unsafe { element.CurrentIsControlElement() }
            .map(|value| value.as_bool())
            .unwrap_or(false);
        let content = unsafe { element.CurrentIsContentElement() }
            .map(|value| value.as_bool())
            .unwrap_or(false);
        format!(
            "{}({},{})-({},{}) class={class:?} name={:?} control={control} content={content}",
            control_type_name(kind),
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            name.chars().take(60).collect::<String>()
        )
    }

    /// The integer inside a `VT_I4` variant, if that is what it holds.
    fn variant_i4(variant: &::windows::Win32::System::Variant::VARIANT) -> Option<i32> {
        use ::windows::Win32::System::Variant::VT_I4;
        let vt = unsafe { variant.Anonymous.Anonymous.vt };
        (vt == VT_I4).then(|| unsafe { variant.Anonymous.Anonymous.Anonymous.lVal })
    }

    /// The `vt` field of a variant.
    fn variant_vt(
        variant: &::windows::Win32::System::Variant::VARIANT,
    ) -> ::windows::Win32::System::Variant::VARENUM {
        unsafe { variant.Anonymous.Anonymous.vt }
    }

    /// MSAA's own hit test, the way PixPin asks for it on Chrome (its `UiSpy` carries
    /// `AccessibleObjectFromWindow`, `accLocation` and the literal string `chrome.exe`).
    ///
    /// `AccessibleObjectFromPoint` runs `accHitTest` recursively until an object answers
    /// `CHILDID_SELF`; in Chromium that ends in `BrowserAccessibilityWin::accHitTest` ->
    /// `CachingAsyncHitTest`, i.e. the **renderer's** hit test, where `ElementFromPoint` is answered
    /// by rectangle comparison. If the two disagree, the finer answer is the renderer's.
    fn report_msaa_hit(point: Point) -> String {
        use ::windows::Win32::UI::Accessibility::{AccessibleObjectFromPoint, IAccessible};
        let mut accessible: Option<IAccessible> = None;
        let mut child: ::windows::Win32::System::Variant::VARIANT =
            unsafe { std::mem::zeroed() };
        let called = unsafe {
            AccessibleObjectFromPoint(
                ::windows::Win32::Foundation::POINT {
                    x: point.x,
                    y: point.y,
                },
                &mut accessible,
                &mut child,
            )
        };
        let Some(accessible) = called.ok().and(accessible) else {
            return "failed".into();
        };
        let role = unsafe { accessible.get_accRole(&child) }
            .ok()
            .and_then(|variant| variant_i4(&variant))
            .unwrap_or(-1);
        let name = unsafe { accessible.get_accName(&child) }
            .ok()
            .map(|name| name.to_string())
            .unwrap_or_default();
        let (mut left, mut top, mut width, mut height) = (0, 0, 0, 0);
        let located = unsafe {
            accessible.accLocation(&mut left, &mut top, &mut width, &mut height, &child)
        }
        .is_ok();
        format!(
            "role=0x{role:x} name={:?} {}{width}x{height} at ({left},{top})",
            name.chars().take(24).collect::<String>(),
            if located { "" } else { "NO-RECT " }
        )
    }

    /// The window's **own** accessible object answering `accHitTest`, the way `UiSpy.dll` does it for
    /// Chrome (`AccessibleObjectFromWindow` + `accLocation`, plus a literal `chrome.exe` branch).
    ///
    /// This is the same renderer hit test as `AccessibleObjectFromPoint`, minus the part that decides
    /// *whose* window the point belongs to: no global hit test is taken, so the capture overlay —
    /// which MSAA answers for every point (measured: `HTTRANSPARENT` fixes UIA but not MSAA) — never
    /// enters the picture.
    fn report_msaa_window_hit(hwnd: isize, point: Point) -> String {
        use ::windows::Win32::System::Variant::{
            VARIANT, VARIANT_0, VARIANT_0_0, VT_DISPATCH, VT_I4,
        };
        use ::windows::Win32::UI::Accessibility::{AccessibleObjectFromWindow, IAccessible};
        use ::windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT;
        let started = std::time::Instant::now();
        let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
        let called = unsafe {
            AccessibleObjectFromWindow(
                HWND(hwnd as *mut core::ffi::c_void),
                OBJID_CLIENT.0 as u32,
                &IAccessible::IID,
                &mut raw,
            )
        };
        if called.is_err() || raw.is_null() {
            return "AccessibleObjectFromWindow failed".into();
        }
        let mut current: IAccessible = unsafe { IAccessible::from_raw(raw) };
        let mut depth = 0;
        loop {
            let Ok(hit) = (unsafe { current.accHitTest(point.x, point.y) }) else {
                break;
            };
            let vt = variant_vt(&hit);
            if vt == VT_I4 {
                break; // CHILDID_SELF: this object is the answer.
            }
            if vt != VT_DISPATCH {
                break;
            }
            let dispatch = unsafe { (*hit.Anonymous.Anonymous.Anonymous.pdispVal).clone() };
            let Some(dispatch) = dispatch else { break };
            let Ok(next) = dispatch.cast::<IAccessible>() else {
                break;
            };
            current = next;
            depth += 1;
            if depth > 32 {
                break;
            }
        }
        // CHILDID_SELF: ask this object about itself rather than one of its children.
        let child = VARIANT {
            Anonymous: VARIANT_0 {
                Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
                    vt: VT_I4,
                    ..Default::default()
                }),
            },
        };
        let role = unsafe { current.get_accRole(&child) }
            .ok()
            .and_then(|variant| variant_i4(&variant))
            .unwrap_or(-1);
        let name = unsafe { current.get_accName(&child) }
            .ok()
            .map(|name| name.to_string())
            .unwrap_or_default();
        let (mut left, mut top, mut width, mut height) = (0, 0, 0, 0);
        let located = unsafe {
            current.accLocation(&mut left, &mut top, &mut width, &mut height, &child)
        }
        .is_ok();
        format!(
            "role=0x{role:x} name={:?} depth={depth} {}{width}x{height} at ({left},{top}) {} ms",
            name.chars().take(24).collect::<String>(),
            if located { "" } else { "NO-RECT " },
            started.elapsed().as_millis()
        )
    }

    /// What every candidate source says is under `point` (docs/21 §10).
    ///
    /// Three sources are compared, because the product's ceiling is set by which one it can use:
    ///
    /// * the **control view** — what `ElementFromPoint` and the current walk use. Chromium prunes
    ///   layout-only `<div>`s out of it, which is exactly the box the user is asking for;
    /// * the **raw view** — documented by Chromium as a *superset* of the control view, so an
    ///   "ignored" node (`role=kIgnored`, nameless) may still be there with its geometry;
    /// * the document's **TextPattern** — `RangeFromPoint` answers with the text at that point, which
    ///   can be finer than any node the tree exposes.
    fn report_sources(automation: &IUIAutomation, hwnd: isize, label: &str, point: Point) {
        let screen = ::windows::Win32::Foundation::POINT {
            x: point.x,
            y: point.y,
        };
        println!("[sources] {label} point=({},{})", point.x, point.y);
        let hit = unsafe { automation.ElementFromPoint(screen) }.ok();
        match &hit {
            Some(element) => println!("[sources]   control hit : {}", describe_element(element)),
            None => println!("[sources]   control hit : failed"),
        }
        println!("[sources]   msaa hit    : {}", report_msaa_hit(point));
        println!(
            "[sources]   msaa window : {}",
            report_msaa_window_hit(hwnd, point)
        );

        // The raw view. Read every property straight off the node: batching the request returned
        // empty rectangles for every node (a `FindAllBuildCache` + `TreeScope_Descendants` combination
        // Chromium does not fill in), which made the first version of this measurement useless. The
        // cost printed here is also the reason a production descent has to stay bounded rather than
        // enumerate a page.
        let raw = (|| -> Option<()> {
            let root = unsafe { automation.ElementFromHandle(HWND(hwnd as *mut core::ffi::c_void)) }
                .ok()?;
            let condition = unsafe { automation.RawViewCondition() }.ok()?;
            let started = std::time::Instant::now();
            let all = unsafe { root.FindAll(TreeScope_Descendants, &condition) }.ok()?;
            let count = unsafe { all.Length() }.ok().unwrap_or(0).max(0) as usize;
            let mut with_rect = 0_usize;
            let mut containing = 0_usize;
            let mut raw_only = 0_usize;
            let mut smallest: Option<(Rect, i32, String, bool)> = None;
            for index in 0..count {
                let Ok(element) = (unsafe { all.GetElement(index as i32) }) else {
                    continue;
                };
                let rect = to_rect(unsafe { element.CurrentBoundingRectangle() }.unwrap_or_default());
                if rect.is_empty() {
                    continue;
                }
                with_rect += 1;
                let control = unsafe { element.CurrentIsControlElement() }
                    .map(|value| value.as_bool())
                    .unwrap_or(false);
                if !control {
                    raw_only += 1;
                }
                if !rect.contains(point) {
                    continue;
                }
                containing += 1;
                if smallest
                    .as_ref()
                    .map(|(best, ..)| rect.area() < best.area())
                    .unwrap_or(true)
                {
                    let kind = unsafe { element.CurrentControlType() }
                        .map(|kind| kind.0)
                        .unwrap_or(0);
                    let class = unsafe { element.CurrentClassName() }
                        .map(|name| name.to_string())
                        .unwrap_or_default();
                    smallest = Some((rect, kind, class, control));
                }
            }
            println!(
                "[sources]   raw view    : {count} nodes, {with_rect} with a rectangle, {raw_only} \
                 raw-only, {containing} contain the point, {} ms",
                started.elapsed().as_millis()
            );
            if let Some((rect, kind, class, control)) = smallest {
                println!(
                    "[sources]   raw smallest: {}({},{})-({},{}) class={class:?} \
                     control={control}{}",
                    control_type_name(kind),
                    rect.left,
                    rect.top,
                    rect.right,
                    rect.bottom,
                    if control { "" } else { "  <- RAW-ONLY" }
                );
            }
            Some(())
        })();
        if raw.is_none() {
            println!("[sources]   raw view    : unavailable");
        }

        // TextPattern: walk up from the hit until a text provider answers, then ask it what text is
        // under the cursor.
        let mut current = hit.clone();
        for depth in 0..8 {
            let Some(element) = current else { break };
            let pattern = unsafe {
                element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            };
            let range = pattern.ok().and_then(|text| {
                unsafe { text.RangeFromPoint(::windows::Win32::Foundation::POINT {
                    x: point.x,
                    y: point.y,
                }) }
                .ok()
            });
            if let Some(range) = range {
                let snippet = unsafe { range.GetText(60) }
                    .map(|text| text.to_string())
                    .unwrap_or_default();
                let enclosing = unsafe { range.GetEnclosingElement() }.ok();
                let described = enclosing
                    .as_ref()
                    .map(describe_element)
                    .unwrap_or_else(|| "none".into());
                println!(
                    "[sources]   text range  : depth={depth} enclosing={described} \
                     text={:?}",
                    snippet.chars().take(24).collect::<String>()
                );
                return;
            }
            current = unsafe { automation.ControlViewWalker() }
                .ok()
                .and_then(|walker| unsafe { walker.GetParentElement(&element) }.ok());
        }
        println!("[sources]   text range  : no TextPattern on the hit or its ancestors");
    }

    /// Every name UIA offers at the cursor (docs/21 §5.24, B6).
    ///
    /// The label's *noun* comes from the control type; the *name* — if one is ever shown — comes from
    /// whichever of four places the page used: `aria-label`, a `title` on an icon-only control, an
    /// `alt`, or the text inside a link/button/heading/cell. A layout-only `<div>` has none, which is
    /// exactly where the label says `容器`.
    ///
    /// Put the cursor on the box you are curious about, then run:
    ///
    /// ```text
    /// cargo test --lib dump_uia_names_under_the_cursor -- --ignored --nocapture
    /// ```
    ///
    /// It prints the window, the ancestor chain with each level's type / name / class / rectangle,
    /// what the label would call the thing under the cursor, and then the source-comparison dump
    /// (control hit, MSAA hit, raw view, text range) for that same point.
    #[test]
    #[ignore = "probe: needs a window under the cursor; prints the names UIA exposes there"]
    fn dump_uia_names_under_the_cursor() {
        use ::windows::Win32::Foundation::POINT;
        use ::windows::Win32::UI::WindowsAndMessaging::{
            GA_ROOT, GetAncestor, GetCursorPos, WindowFromPoint,
        };

        let mut cursor = POINT::default();
        if unsafe { GetCursorPos(&mut cursor) }.is_err() {
            eprintln!("GetCursorPos failed; there is nothing to look at");
            return;
        }
        let at = POINT {
            x: cursor.x,
            y: cursor.y,
        };
        let root = unsafe { GetAncestor(WindowFromPoint(at), GA_ROOT) };
        if root.0.is_null() {
            eprintln!("no top-level window under the cursor");
            return;
        }
        let hwnd = root.0 as isize;
        let point = Point::new(cursor.x, cursor.y);
        let frame = win32::frame_bounds(hwnd).unwrap_or_default();
        println!(
            "[names] cursor=({},{}) window={} frame={}x{} at ({},{})",
            cursor.x,
            cursor.y,
            describe_window(hwnd),
            frame.width(),
            frame.height(),
            frame.left,
            frame.top
        );

        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let automation: Option<IUIAutomation> =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.ok();
        let Some(automation) = automation else {
            eprintln!("UI Automation is unavailable");
            return;
        };

        match unsafe { automation.ElementFromPoint(at) } {
            Ok(element) => {
                // What the label would show for this very box: the noun its control type maps to, and
                // the name the page gave it (empty when nobody gave it one).
                let kind = unsafe { element.CurrentControlType() }
                    .map(|kind| kind.0)
                    .unwrap_or(0);
                let level = level_kind_of_control_type(kind);
                let name = unsafe { element.CurrentName() }
                    .map(|name| name.to_string())
                    .unwrap_or_default();
                println!(
                    "[names] label would say: {} ({}) + name {:?}",
                    level.noun_zh().unwrap_or("容器/元素"),
                    level.debug_name(),
                    name.chars().take(60).collect::<String>()
                );
                // …and the chain it sits in: every level the ancestor walk could publish, innermost
                // first. A level whose `name` is empty is one the label can only call a container.
                let walker = unsafe { automation.ControlViewWalker() }.ok();
                let mut current = Some(element);
                for depth in 0..24 {
                    let Some(node) = current else { break };
                    println!("[names]   {depth:>2} {}", describe_element(&node));
                    current = walker
                        .as_ref()
                        .and_then(|walker| unsafe { walker.GetParentElement(&node) }.ok());
                }
            }
            Err(error) => eprintln!("ElementFromPoint failed: {error}"),
        }
        report_sources(&automation, hwnd, "cursor", point);
    }

    /// `class="…" WxH at (x,y) visible=…` for a window handle, or `none` for "no window".
    fn describe_window(hwnd: isize) -> String {
        if hwnd == 0 {
            return "none".into();
        }
        let class = win32::class_name(hwnd).unwrap_or_default();
        let rect = win32::frame_bounds(hwnd).unwrap_or_default();
        format!(
            "{class:?} {}x{} at ({},{}) visible={}",
            rect.width(),
            rect.height(),
            rect.left,
            rect.top,
            win32::is_window_visible(hwnd)
        )
    }

    /// n / p50 / p95 / max of the product walk's per-query latency, in milliseconds.
    /// The correctness gates say nothing about cost, and the walk grew (backtracking, look-through,
    /// a `ChildWindowFromPointEx` per window-backed candidate), so every probe reports it: the
    /// refinement budget is 1500 ms per query, and a regression here is as real as a wrong box.
    fn print_latency_summary(label: &str, latencies: &mut Vec<f64>) {
        if latencies.is_empty() {
            return;
        }
        latencies.sort_by(f64::total_cmp);
        let pick = |percent: usize| {
            let index = (latencies.len() * percent).div_ceil(100).saturating_sub(1);
            latencies[index.min(latencies.len() - 1)]
        };
        println!(
            "[{label}] latency_ms n={} p50={:.1} p95={:.1} max={:.1}",
            latencies.len(),
            pick(50),
            pick(95),
            latencies[latencies.len() - 1]
        );
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
        // **Deterministic content.** The grid metric depends entirely on what the window shows, so
        // pointing it at the user's own Explorer window made the numbers move on their own
        // (measured for the same code: 9/25, 11/25, 12/25 on different days and folders) — which is
        // exactly the confound that made a rule change look like a regression. The probe now opens
        // its own folder with a fixed set of files and measures *that* window.
        let fixture = std::env::temp_dir().join("snapclip-explorer-fixture");
        let _ = std::fs::create_dir_all(&fixture);
        for index in 0..24 {
            let file = fixture.join(format!("file-{index:02}.txt"));
            if !file.exists() {
                let _ = std::fs::write(&file, format!("snapclip fixture file {index}\n"));
            }
        }
        let fixture_title = fixture
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let _ = std::process::Command::new("explorer.exe")
            .arg(&fixture)
            .spawn();
        let deadline = std::time::Instant::now() + Duration::from_secs(25);
        let mut window = None;
        while window.is_none() && std::time::Instant::now() < deadline {
            pump(250);
            window = win32::enumerate_cheap_candidates()
                .unwrap_or_default()
                .into_iter()
                .find(|probe| {
                    probe.class_name == "CabinetWClass"
                        && probe_title(probe.hwnd).contains(&fixture_title)
                })
                .map(|probe| probe.hwnd);
        }
        let Some(hwnd) = window else {
            eprintln!("skipping: the deterministic Explorer fixture window never appeared");
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
        // Raised so the provider's hit test can see this window at all: a covered window is never
        // answered by `ElementFromPoint`, which would make the precision comparison below vacuous.
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos,
            };
            SetWindowPos(
                hwnd as *mut core::ffi::c_void,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_SHOWWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        pump(400);

        let explorer_metrics = WindowDetectionMetrics::new();
        let mut provider = UiaDeepSelectionProvider::new(explorer_metrics.clone());
        // Resolve through the product's provider chain, not the UIA provider alone: the MSAA second
        // opinion (docs/21 §5.16) and the containment invariant (§5.17) both live there.
        let mut pipeline = super::super::refinement_worker::FallbackDeepSelection::new(
            explorer_metrics,
            win32::HitTestPassThrough::default(),
            crate::capture::window_detection::DEFAULT_ADOPT_TEXT_RUNS,
        );
        let window_area = i64::from(client.width()) * i64::from(client.height());
        let mut areas = Vec::new();
        let mut latencies: Vec<f64> = Vec::new();
        let mut provider_finer = 0_usize;
        let mut provider_available = 0_usize;
        let mut broken_chains: Vec<String> = Vec::new();
        for fy in [30_i32, 40, 50, 60, 70] {
            for fx in [45_i32, 55, 65, 75, 85] {
                let point = Point::new(
                    client.left + client.width() * fx / 100,
                    client.top + client.height() * fy / 100,
                );
                let query_started = std::time::Instant::now();
                let outcome = pipeline.resolve(
                    &job(hwnd, point),
                    frame,
                    &QueryControl::refinement(&|| false),
                );
                latencies.push(query_started.elapsed().as_secs_f64() * 1000.0);
                let (rect, depth, reason) = match &outcome {
                    RefinementOutcome::Target(target) => (
                        target.screen_bounds,
                        target.path.len(),
                        target.stop_reason,
                    ),
                    RefinementOutcome::Empty(reason) => (Rect::default(), 0, *reason),
                };
                if let RefinementOutcome::Target(target) = &outcome {
                    // The ancestor walk steps through this chain (§5.17): each level has to contain
                    // the next, and the last one has to be the published box.
                    let slack = |outer: Rect, inner: Rect| {
                        inner.left >= outer.left - 2
                            && inner.top >= outer.top - 2
                            && inner.right <= outer.right + 2
                            && inner.bottom <= outer.bottom + 2
                    };
                    if let Some(pair) = target
                        .path
                        .windows(2)
                        .find(|pair| !slack(pair[0].rect, pair[1].rect))
                    {
                        broken_chains.push(format!(
                            "point=({},{}): {:?} does not contain {:?}",
                            point.x, point.y, pair[0], pair[1]
                        ));
                    }
                    if target.path.last().map(|level| level.rect) != Some(target.screen_bounds) {
                        broken_chains.push(format!(
                            "point=({},{}): path ends at {:?}, published {:?}",
                            point.x,
                            point.y,
                            target.path.last(),
                            target.screen_bounds
                        ));
                    }
                }
                let area = i64::from(rect.width()) * i64::from(rect.height());
                areas.push(area);
                // Precision yardstick: what the provider's own hit test answers, through the
                // production path. A hit box that is strictly smaller but still contains the point
                // is a place our walk stopped above the innermost capturable box — and the
                // adoption rule is supposed to remove exactly those.
                let hit = match provider.provider_hit(hwnd, point, frame) {
                    ProviderHit::Box {
                        bounds,
                        control_type,
                        ..
                    } => Some((bounds, control_type)),
                    ProviderHit::Unusable(_) => None,
                };
                if hit.is_some() {
                    provider_available += 1;
                }
                if let Some((hit_bounds, hit_kind)) = hit
                    && i64::from(hit_bounds.width()) * i64::from(hit_bounds.height()) < area
                {
                    provider_finer += 1;
                    println!(
                        "[explorer]   provider is finer: {}x{} at ({},{}) type={hit_kind}",
                        hit_bounds.width(),
                        hit_bounds.height(),
                        hit_bounds.left,
                        hit_bounds.top
                    );
                }
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
        println!(
            "[explorer] precision: provider_hit_available={provider_available}/{} \
             provider_hit_is_finer_on={provider_finer}",
            areas.len()
        );
        print_latency_summary("explorer", &mut latencies);
        // The same containment invariant the browser gate asserts, on the other window class.
        assert!(
            broken_chains.is_empty(),
            "{} Explorer sampling points published a path that is not a containment chain ending \
             at the published box: {broken_chains:#?}",
            broken_chains.len()
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
    /// The log's spelling of a control type.
    ///
    /// The ids live in one place now (`level_kind_of_control_type`, which the walk uses to publish
    /// what each level *is*); this is that vocabulary's English view, so a log line and the label can
    /// never disagree about what the provider said.
    fn control_type_name(kind: i32) -> &'static str {
        level_kind_of_control_type(kind).debug_name()
    }

    /// The page the cross-origin fixture frame loads (docs/21 §5.20).
    ///
    /// One button at a fixed place with `margin: 0`. The frame measures itself and reports over
    /// `postMessage`, which is both the box its parent cannot read *and* the only proof that its
    /// content ever loaded: without that report a blank frame answers "the frame node" for every
    /// point inside it, which reads exactly like "cross-origin content is unreachable" — the wrong
    /// conclusion this fixture produced before the report existed.
    const CROSS_ORIGIN_FIXTURE: &str = "<!doctype html><body style=\"margin:0\">\
<button id=\"cross-button\" style=\"position:absolute;left:40px;top:40px;width:160px;\
height:48px\">Cross Button</button>\
<script>\
fetch('/cross-ping');\
const box = document.getElementById('cross-button').getBoundingClientRect();\
parent.postMessage({snapclip: 'cross-ready', html: document.body.innerHTML.length,\
 button: [box.left, box.top, box.width, box.height]}, '*');\
</script></body>";

    /// Serve [`CROSS_ORIGIN_FIXTURE`] on a loopback port and keep serving until the process ends.
    ///
    /// The fixture page is loaded over `file://`, so `http://127.0.0.1:<port>` is a real second
    /// origin: the parent cannot reach into the frame through `contentDocument`, which is what makes
    /// this the cross-origin case rather than the same-origin one `iframe-box` covers.
    fn serve_cross_origin_fixture() -> Option<u16> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?;
        let port = listener.local_addr().ok()?.port();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                // Every request is logged by path: the probe refuses to measure without the child's
                // report, and this says whether "no report" means the frame never asked for the page,
                // the page was served but its script never ran, or the message was lost on the way
                // back (docs/21 §5.20).
                let mut buffer = [0_u8; 1024];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .to_owned();
                eprintln!("[probe] cross-origin server: {path}");
                if path != "/cross.html" {
                    // The child's own liveness ping: proof its script ran, independent of whether
                    // the message back to the parent arrives.
                    let _ = stream.write_all(
                        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    let _ = stream.flush();
                    continue;
                }
                let body = CROSS_ORIGIN_FIXTURE.as_bytes();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body);
                let _ = stream.flush();
            }
        });
        Some(port)
    }
}
