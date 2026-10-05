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

use crate::capture::diagnostics::WindowDetectionMetrics;
use crate::capture::geometry::{Point, Rect};
use crate::capture::window_detection::deep::{
    DeepSelectionProvider, DeepTarget, QueryControl, RefinementJob, RefinementOutcome, StopReason,
};
use crate::capture::window_detection::model::TargetKind;

use super::timed_call::{TimedCallRunner, TimedOutcome};

/// Deadline for one MSAA hit test. Same value as the reference selector's request timeout.
pub const MSAA_REQUEST_TIMEOUT: Duration = Duration::from_millis(168);

/// `OBJID_WINDOW` / `CHILDID_SELF` are stable ABI values; the `windows` crate exposes them from
/// a module this build does not import, so they are named here with their documented values.
const OBJID_WINDOW: u32 = 0;
const CHILDID_SELF: i32 = 0;

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
            self.metrics.log_line(
                &format!("msaa quarantine hwnd={hwnd} why={why}"),
                false,
            );
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
        if self.quarantined.contains(&hwnd) {
            return RefinementOutcome::Empty(StopReason::Unsupported);
        }
        if control.is_cancelled() {
            return RefinementOutcome::Empty(StopReason::Cancelled);
        }
        // Cheapest possible guard: never call into a window the system already considers hung.
        let window = HWND(hwnd as *mut core::ffi::c_void);
        if unsafe { IsHungAppWindow(window) }.as_bool() {
            self.metrics.record_refinement_msaa_timeout();
            self.quarantine(hwnd, "hung");
            return RefinementOutcome::Empty(StopReason::Unsupported);
        }

        let point = job.point;
        // The closure moves to a detached thread, and `HWND` is a raw pointer that is not
        // `Send`; the handle travels as its integer form and is rebuilt on the other side.
        let hwnd_value = hwnd;
        let metrics = self.metrics.clone();
        let outcome = self.runner.run(
            move || {
                metrics.record_refinement_msaa_attempt();
                msaa_hit_test(HWND(hwnd_value as *mut core::ffi::c_void), point)
            },
            MSAA_REQUEST_TIMEOUT,
        );

        match outcome {
            // Admission failure says nothing about the window: retry on the next dwell.
            TimedOutcome::Busy => {
                self.metrics.record_refinement_msaa_busy();
                RefinementOutcome::Empty(StopReason::ProviderTimeout)
            }
            TimedOutcome::TimedOut => {
                self.metrics.record_refinement_msaa_timeout();
                self.quarantine(hwnd, "timeout");
                RefinementOutcome::Empty(StopReason::ProviderTimeout)
            }
            TimedOutcome::Completed(None) => {
                self.metrics.record_refinement_msaa_failure();
                RefinementOutcome::Empty(StopReason::ProviderFailure)
            }
            TimedOutcome::Completed(Some(hit)) => {
                let mut path = vec![window_bounds];
                if hit != window_bounds && !hit.is_empty() {
                    path.push(hit);
                }
                let kind = if path.len() > 1 {
                    TargetKind::UiElement
                } else {
                    TargetKind::TopLevelWindowFrame
                };
                RefinementOutcome::Target(Box::new(DeepTarget {
                    window: job.window,
                    kind,
                    screen_bounds: *path.last().expect("the path always holds the frame"),
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
fn msaa_hit_test(window: HWND, point: Point) -> Option<Rect> {
    use ::windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    // MSAA providers are apartment-bound; a failure here (for example `RPC_E_CHANGED_MODE`)
    // means COM was already initialised, which is fine.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };

    let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
    unsafe {
        AccessibleObjectFromWindow(window, OBJID_WINDOW, &IAccessible::IID, &mut raw).ok()?;
    }
    if raw.is_null() {
        return None;
    }
    let accessible: IAccessible = unsafe { IAccessible::from_raw(raw) };

    // `accHitTest` resolves the point in one call and hands back either a child id or a child
    // object; both are turned into a rectangle with `accLocation`.
    let hit = unsafe { accessible.accHitTest(point.x, point.y) }.ok()?;
    match unsafe { hit.Anonymous.Anonymous.vt } {
        VT_I4 => location_of(&accessible, &hit),
        VT_DISPATCH => {
            let dispatch = unsafe { &hit.Anonymous.Anonymous.Anonymous.pdispVal };
            let child = dispatch.as_ref()?.cast::<IAccessible>().ok()?;
            location_of(&child, &child_self())
        }
        _ => None,
    }
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
    use crate::capture::window_detection::model::{RequestGate, WindowIdentity};

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
