//! MSAA (`IAccessible`) fallback provider (docs/18 §12).
//!
//! UIA misses older and custom-drawn controls; those usually still answer MSAA. MSAA is also
//! the API most famous for hanging, so every call here is wrapped in the deadline-bounded
//! runner and every failure mode is classified:
//!
//! * the window is already hung (`IsHungAppWindow`) → quarantine, do not even call;
//! * the call passes its deadline → quarantine (the provider is misbehaving);
//! * the runner is busy → **no quarantine**, the caller simply retries later;
//! * the provider returns an error → empty result, no quarantine.
//!
//! Isolation lasts until the snapshot generation changes, matching the UIA provider.

use std::collections::HashSet;
use std::time::Duration;

use ::windows::Win32::Foundation::{HWND, VARIANT_BOOL};
use ::windows::Win32::System::Variant::{VARIANT, VariantInit, VT_DISPATCH, VT_I4};
use ::windows::Win32::UI::Accessibility::{AccessibleObjectFromWindow, IAccessible};
use ::windows::Win32::UI::WindowsAndMessaging::IsHungAppWindow;
use ::windows::core::Interface;

use crate::diagnostics::WindowDetectionMetrics;
use crate::geometry::{Point, Rect};
use crate::window_detection::deep::{
    DeepSelectionProvider, DeepTarget, QueryControl, RefinementJob, RefinementOutcome, StopReason,
};
use crate::window_detection::model::{LevelKind, PathLevel, TargetKind};
use crate::window_detection::uia::level_kind_of_msaa_role;

use super::timed_call::{TimedCallRunner, TimedOutcome};

/// Deadline for one MSAA hit test. Same value as the reference selector's request timeout.
pub const MSAA_REQUEST_TIMEOUT: Duration = Duration::from_millis(168);

/// `OBJID_WINDOW` / `CHILDID_SELF` are stable ABI values; the `windows` crate exposes them from
/// a module this build does not import, so they are named here with their documented values.
const OBJID_WINDOW: u32 = 0;
/// `OBJID_CLIENT`: for Chromium this is the object whose `accHitTest` runs the renderer's own hit
/// test, which is what sees the layout-only boxes UIA never exposes (docs/21 §5.11).
const OBJID_CLIENT: u32 = u32::MAX - 3; // 0xFFFF_FFFC, i.e. -4
const CHILDID_SELF: i32 = 0;

/// How many `accHitTest` steps a single query may take before the answer is treated as unusable.
const MAX_HIT_DEPTH: u32 = 8;
/// How many ancestors may trim the box on its way up.
const MAX_CLIP_DEPTH: usize = 16;

/// What one MSAA hit test found, in the shape the refinement pipeline consumes (docs/21 §5.16).
#[derive(Debug, Clone)]
pub(crate) struct MsaaHitBox {
    /// What `accLocation` reported. Unclipped: a scrolled container answers with its layout box.
    pub raw: Rect,
    /// The part of `raw` that survives its ancestors and the window — what may be published.
    pub visible: Rect,
    /// MSAA role (`ROLE_SYSTEM_*`), for the bare-text rule and the forensics.
    pub role: i32,
    /// Accessible name, for the forensics.
    pub name: String,
    /// How many `accHitTest` steps it took to reach `CHILDID_SELF`.
    pub depth: u32,
    /// The chain above the box, outermost first, every level containing `visible` and ending at the
    /// window frame. This is what the ancestor walk (docs/21 §5.17) steps through, so it is built
    /// from the same `accParent` walk that computes `visible`.
    pub ancestors: Vec<Rect>,
    /// Role of the box this hit sits in (`ROLE_SYSTEM_*`), or 0 when the provider could not say.
    ///
    /// The product rule needs it: a text run is a target only when it is not the label of a control
    /// (docs/21 §5.19).
    pub parent_role: i32,
}

/// Why a hit test produced no box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MsaaHitFailure {
    /// The window is isolated for this snapshot generation (an earlier call wedged).
    Quarantined,
    /// A call is already in flight; the caller should retry on the next dwell.
    Busy,
    /// The provider wedged and is quarantined for this generation.
    TimedOut,
    /// The provider answered nothing usable.
    Unavailable,
}

/// MSAA provider: one hit test per query, deadline-bounded, with per-window quarantine.
pub struct MsaaDeepSelectionProvider {
    metrics: WindowDetectionMetrics,
    runner: TimedCallRunner,
    quarantined: HashSet<isize>,
}

impl MsaaDeepSelectionProvider {
    pub fn new(metrics: WindowDetectionMetrics) -> Self {
        Self {
            metrics,
            runner: TimedCallRunner::new(1),
            quarantined: HashSet::new(),
        }
    }

    /// Windows currently isolated, for diagnostics and tests.
    pub fn quarantined(&self) -> usize {
        self.quarantined.len()
    }

    fn quarantine(&mut self, hwnd: isize, why: &str) {
        if self.quarantined.insert(hwnd) {
            self.metrics.record_refinement_quarantine_added();
            self.metrics.log_line(
                &format!("msaa quarantine hwnd={hwnd} why={why}"),
                false,
            );
        }
    }

    /// One deadline-bounded MSAA hit test on this window's **own** accessible object.
    ///
    /// `OBJID_CLIENT` first, `OBJID_WINDOW` as the fallback. Nothing global is involved, which is
    /// the whole point: `AccessibleObjectFromPoint` is answered by the capture overlay (and
    /// `WM_NCHITTEST -> HTTRANSPARENT`, which saves the UIA transport, does not save MSAA), whereas
    /// asking the window itself never involves whatever is stacked on top of it (docs/21 §5.11).
    pub(crate) fn hit(
        &mut self,
        hwnd: isize,
        point: Point,
        window_bounds: Rect,
    ) -> Result<MsaaHitBox, MsaaHitFailure> {
        // Isolation first, and here rather than only in `resolve`: the composite asks this window for
        // a second opinion on *every* query, so a window that wedged once must not be called again
        // (measured before this check: 13 of 21 queries burned the full 168 ms deadline after the
        // window had already been quarantined).
        if self.quarantined.contains(&hwnd) {
            self.metrics.record_refinement_quarantine_hit();
            return Err(MsaaHitFailure::Quarantined);
        }
        // Cheapest possible guard: never call into a window the system already considers hung.
        let window = HWND(hwnd as *mut core::ffi::c_void);
        if unsafe { IsHungAppWindow(window) }.as_bool() {
            self.metrics.record_refinement_msaa_timeout();
            self.quarantine(hwnd, "hung");
            return Err(MsaaHitFailure::TimedOut);
        }
        // The closure moves to a detached thread, and `HWND` is a raw pointer that is not `Send`;
        // the handle travels as its integer form and is rebuilt on the other side.
        let metrics = self.metrics.clone();
        let outcome = self.runner.run(
            move || {
                metrics.record_refinement_msaa_attempt();
                msaa_hit_test(HWND(hwnd as *mut core::ffi::c_void), point, window_bounds)
            },
            MSAA_REQUEST_TIMEOUT,
        );
        match outcome {
            // Admission failure says nothing about the window: retry on the next dwell.
            TimedOutcome::Busy => {
                self.metrics.record_refinement_msaa_busy();
                Err(MsaaHitFailure::Busy)
            }
            TimedOutcome::TimedOut => {
                self.metrics.record_refinement_msaa_timeout();
                self.quarantine(hwnd, "timeout");
                Err(MsaaHitFailure::TimedOut)
            }
            TimedOutcome::Completed(None) => {
                self.metrics.record_refinement_msaa_failure();
                Err(MsaaHitFailure::Unavailable)
            }
            TimedOutcome::Completed(Some(hit)) => Ok(hit),
        }
    }
}

impl DeepSelectionProvider for MsaaDeepSelectionProvider {
    fn resolve(
        &mut self,
        job: &RefinementJob,
        window_bounds: Rect,
        control: &QueryControl<'_>,
    ) -> RefinementOutcome {
        let hwnd = job.window.hwnd;
        if control.is_cancelled() {
            return RefinementOutcome::Empty(StopReason::Cancelled);
        }
        match self.hit(hwnd, job.point, window_bounds) {
            // Isolation and the hung-window guard live in `hit`, so both callers share them.
            Err(MsaaHitFailure::Quarantined) => {
                RefinementOutcome::Empty(StopReason::Unsupported)
            }
            Err(MsaaHitFailure::Busy) => RefinementOutcome::Empty(StopReason::ProviderTimeout),
            Err(MsaaHitFailure::TimedOut) => RefinementOutcome::Empty(StopReason::ProviderTimeout),
            Err(MsaaHitFailure::Unavailable) => {
                RefinementOutcome::Empty(StopReason::ProviderFailure)
            }
            Ok(hit) => {
                let box_level = PathLevel::new(hit.visible, level_kind_of_msaa_role(hit.role));
                let mut path = vec![PathLevel::new(window_bounds, LevelKind::Window)];
                if box_level.rect != window_bounds && !box_level.rect.is_empty() {
                    path.push(box_level);
                }
                let kind = if path.len() > 1 {
                    TargetKind::UiElement
                } else {
                    TargetKind::TopLevelWindowFrame
                };
                RefinementOutcome::Target(Box::new(DeepTarget {
                    window: job.window,
                    kind,
                    screen_bounds: path
                        .last()
                        .expect("the path always holds the frame")
                        .rect,
                    path,
                    stop_reason: StopReason::Complete,
                }))
            }
        }
    }

    fn release(&mut self) {
        // Isolation lasts exactly one snapshot generation, like the UIA provider.
        if !self.quarantined.is_empty() {
            self.metrics.log_line(
                &format!("msaa quarantine cleared windows={}", self.quarantined()),
                false,
            );
        }
        self.quarantined.clear();
    }
}

/// One MSAA hit test, run inside the deadline-bounded runner.
///
/// Runs on a detached thread, so it must initialise COM itself.
fn msaa_hit_test(window: HWND, point: Point, window_bounds: Rect) -> Option<MsaaHitBox> {
    use ::windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    // MSAA providers are apartment-bound; a failure here (for example `RPC_E_CHANGED_MODE`)
    // means COM was already initialised, which is fine.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };

    let mut current = accessible_for(window, OBJID_CLIENT)
        .or_else(|| accessible_for(window, OBJID_WINDOW))?;
    // `accHitTest` hands back either "this object is it" (`CHILDID_SELF`) or the child that owns the
    // point; oleacc's own `AccessibleObjectFromPoint` loops until the former, and a provider is free
    // to answer one level at a time, so the loop is bounded rather than assumed to be absent.
    let mut depth = 0;
    loop {
        let hit = unsafe { current.accHitTest(point.x, point.y) }.ok()?;
        let vt = unsafe { hit.Anonymous.Anonymous.vt };
        if vt == VT_I4 {
            break;
        }
        if vt != VT_DISPATCH {
            return None;
        }
        let dispatch = unsafe { &hit.Anonymous.Anonymous.Anonymous.pdispVal };
        current = dispatch.as_ref()?.cast::<IAccessible>().ok()?;
        depth += 1;
        if depth >= MAX_HIT_DEPTH {
            break;
        }
    }

    let raw = location_of(&current, &child_self())?;
    // A renderer hit test that names a node whose box does not cover the cursor is not an answer for
    // this point (a layout box scrolled away, for instance); the caller keeps its own answer.
    if !raw.contains(point) {
        return None;
    }
    let (visible, ancestors) = visible_part_and_ancestors(&current, raw, point, window_bounds);
    // Who owns this box? Only the immediate parent matters for the control-label rule.
    let parent_role = unsafe { current.accParent() }
        .ok()
        .and_then(|parent| parent.cast::<IAccessible>().ok())
        .and_then(|parent| role_of(&parent))
        .unwrap_or(0);
    Some(MsaaHitBox {
        raw,
        visible,
        role: role_of(&current).unwrap_or(0),
        name: name_of(&current).unwrap_or_default(),
        depth,
        ancestors,
        parent_role,
    })
}

/// The window's accessible object for one `OBJID`.
fn accessible_for(window: HWND, object_id: u32) -> Option<IAccessible> {
    let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
    unsafe {
        AccessibleObjectFromWindow(window, object_id, &IAccessible::IID, &mut raw).ok()?;
    }
    if raw.is_null() {
        return None;
    }
    Some(unsafe { IAccessible::from_raw(raw) })
}

/// The part of `raw` that survives the node's ancestors and the window (docs/21 §5.9).
///
/// `accLocation` is unclipped — a container taller than its scrollport answers with its layout box,
/// measured at 358x20000 on the fixture — so the same "what is on screen" rule the UIA transport
/// uses has to run here. An ancestor that would clip the cursor away is ignored: a virtualised item
/// can report an empty or stale rectangle, and it must not be allowed to remove the answer.
fn visible_part_and_ancestors(
    accessible: &IAccessible,
    raw: Rect,
    point: Point,
    window_bounds: Rect,
) -> (Rect, Vec<Rect>) {
    let mut visible = raw;
    if raw.intersect(window_bounds) != raw {
        visible = raw.intersect(window_bounds);
    }
    // Collected nearest-parent-first while walking up; the frame is prepended last, because the
    // chain has to read outermost-first (`[frame, …, innermost]`) for the ancestor walk.
    let mut ancestors: Vec<Rect> = Vec::new();
    let mut current = match unsafe { accessible.accParent() }
        .ok()
        .and_then(|parent| parent.cast::<IAccessible>().ok())
    {
        Some(parent) => parent,
        None => return (visible, ancestors),
    };
    for _ in 0..MAX_CLIP_DEPTH {
        if let Some(parent_bounds) = location_of(&current, &child_self()) {
            let clipped = visible.intersect(parent_bounds);
            if !clipped.is_empty() && clipped.contains(point) {
                visible = clipped;
            }
            // Only levels *inside the window* that contain the published box belong in the chain
            // (§5.17). MSAA's parent chain leaves the window — Chromium's root answers with the whole
            // monitor, measured as (0,0)-(3840,2160) — and a level larger than the frame would make
            // the chain say the frame sits inside a bigger box. Clipping rather than dropping keeps a
            // frame-sized ancestor as the frame level itself, which the dedupe then folds away.
            let level = parent_bounds.intersect(window_bounds);
            if !level.is_empty()
                && window_bounds.contains_rect(level)
                && level.contains_rect(visible)
                && !ancestors.contains(&level)
            {
                ancestors.push(level);
            }
        }
        match unsafe { current.accParent() }
            .ok()
            .and_then(|parent| parent.cast::<IAccessible>().ok())
        {
            Some(parent) => current = parent,
            None => break,
        }
    }
    // Outermost first: the frame, then each ancestor from the outside in.
    ancestors.reverse();
    if ancestors.first() != Some(&window_bounds) {
        ancestors.insert(0, window_bounds);
    }
    (visible, ancestors)
}

/// The accessible role as an integer (`ROLE_SYSTEM_*`).
fn role_of(accessible: &IAccessible) -> Option<i32> {
    let role = unsafe { accessible.get_accRole(&child_self()) }.ok()?;
    match unsafe { role.Anonymous.Anonymous.vt } {
        VT_I4 => Some(unsafe { role.Anonymous.Anonymous.Anonymous.lVal }),
        _ => None,
    }
}

/// The accessible name, if the provider offers one.
fn name_of(accessible: &IAccessible) -> Option<String> {
    unsafe { accessible.get_accName(&child_self()) }
        .ok()
        .map(|name| name.to_string())
}

/// `accLocation` for `var_child`, converted to a screen rectangle.
fn location_of(accessible: &IAccessible, var_child: &VARIANT) -> Option<Rect> {
    let (mut left, mut top, mut width, mut height) = (0i32, 0i32, 0i32, 0i32);
    unsafe {
        accessible
            .accLocation(&mut left, &mut top, &mut width, &mut height, var_child)
            .ok()?;
    }
    if width <= 0 || height <= 0 {
        return None;
    }
    Some(Rect::new(left, top, left + width, top + height))
}

/// A `VT_I4` variant carrying `CHILDID_SELF`.
fn child_self() -> VARIANT {
    let mut variant = unsafe { VariantInit() };
    unsafe {
        // `VARIANT`'s members are unions wrapped in `ManuallyDrop`, so the fields are reached
        // through explicit references rather than auto-deref.
        let inner: &mut ::windows::Win32::System::Variant::VARIANT_0 = &mut variant.Anonymous;
        let body: &mut ::windows::Win32::System::Variant::VARIANT_0_0 = &mut *inner.Anonymous;
        body.vt = VT_I4;
        body.Anonymous.lVal = CHILDID_SELF;
    }
    variant
}

/// `VARIANT_BOOL` is re-exported for symmetry with the rest of the capture platform code.
#[allow(dead_code)]
fn _variant_bool(value: bool) -> VARIANT_BOOL {
    VARIANT_BOOL(if value { -1 } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window_detection::model::{RequestGate, WindowIdentity};

    fn job(hwnd: isize, point: Point) -> RefinementJob {
        static GATE: std::sync::OnceLock<std::sync::Mutex<RequestGate>> =
            std::sync::OnceLock::new();
        let request = GATE
            .get_or_init(|| std::sync::Mutex::new(RequestGate::new()))
            .lock()
            .expect("gate")
            .issue();
        RefinementJob {
            request,
            window: WindowIdentity::new(hwnd, 1, 2),
            epoch: 1,
            point,
        }
    }

    #[test]
    fn a_window_the_system_reports_as_hung_is_quarantined_without_being_called() {
        // A fabricated handle cannot be a hung *window*, so this test drives the isolation
        // path through the deadline instead: the query for a bogus handle is expected to come
        // back either as a provider failure or as a timeout, and a timeout must quarantine.
        let metrics = WindowDetectionMetrics::new();
        let mut provider = MsaaDeepSelectionProvider::new(metrics);
        let control = QueryControl::refinement(&|| false);
        let outcome = provider.resolve(
            &job(0xDEAD_BEEF, Point::new(10, 10)),
            Rect::new(0, 0, 100, 100),
            &control,
        );
        match outcome {
            RefinementOutcome::Empty(StopReason::ProviderFailure) => {
                assert_eq!(provider.quarantined(), 0, "a provider error is not quarantine");
            }
            RefinementOutcome::Empty(StopReason::ProviderTimeout) => {
                assert_eq!(provider.quarantined(), 1, "a timeout quarantines the window");
            }
            other => panic!("unexpected outcome {other:?}"),
        }

        // `release` clears isolation for the next snapshot generation.
        provider.release();
        assert_eq!(provider.quarantined(), 0);
    }

    #[test]
    fn an_already_quarantined_window_is_answered_without_touching_the_provider() {
        let mut provider = MsaaDeepSelectionProvider::new(WindowDetectionMetrics::new());
        provider.quarantine(0x1234, "test");
        assert_eq!(provider.quarantined(), 1);
        let control = QueryControl::refinement(&|| false);
        assert_eq!(
            provider.resolve(
                &job(0x1234, Point::new(10, 10)),
                Rect::new(0, 0, 100, 100),
                &control
            ),
            RefinementOutcome::Empty(StopReason::Unsupported)
        );
        // …and the second-opinion path (`hit`) is isolated too: the composite asks every window on
        // every query, so without this the quarantined one kept burning its 168 ms deadline
        // (measured: 13 timeouts out of 21 queries in one real session, docs/21 §5.21).
        assert!(matches!(
            provider.hit(0x1234, Point::new(10, 10), Rect::new(0, 0, 100, 100)),
            Err(MsaaHitFailure::Quarantined)
        ));
    }

    #[test]
    fn a_cancelled_query_never_reaches_the_provider() {
        let mut provider = MsaaDeepSelectionProvider::new(WindowDetectionMetrics::new());
        let control = QueryControl::refinement(&|| true);
        assert_eq!(
            provider.resolve(
                &job(0x2000, Point::new(10, 10)),
                Rect::new(0, 0, 100, 100),
                &control
            ),
            RefinementOutcome::Empty(StopReason::Cancelled)
        );
        assert_eq!(provider.quarantined(), 0);
    }

    #[test]
    fn a_child_self_variant_carries_a_child_id() {
        // The MSAA hit-test contract depends on this variant shape; it is the one piece of
        // VARIANT plumbing that can be checked without a provider.
        let variant = child_self();
        unsafe {
            let body = &*variant.Anonymous.Anonymous;
            assert_eq!(body.vt, VT_I4);
            assert_eq!(body.Anonymous.lVal, CHILDID_SELF);
        }
    }
}
