//! Window-boundary detection for the screenshot overlay (docs/14 §5).
//!
//! This module owns the *contract* between the overlay message thread, the window
//! detection worker and the render path. It deliberately contains no Win32 calls:
//! the platform layer (`platform::windows::capture::win::window`) performs the
//! enumeration/DWM reads and hands plain data back.
//!
//! ## Ownership and threading (docs/14 §5.2)
//!
//! ```text
//! overlay thread   hit_test / nearest_target / apply_candidate_update — pure cache reads
//! detection worker refresh / revalidate_hover / validate — EnumWindows + DWM live here
//! result transfer  worker -> overlay: post + epoch/identity/request-id check
//! ```
//!
//! Two rules make the split safe:
//!
//! * a snapshot is only ever *read* on the overlay thread; the worker builds a new
//!   one and the overlay swaps it in, so a refresh never blocks the message loop;
//! * every asynchronous result carries the epoch/identity it was produced against,
//!   and the overlay drops it when either has moved on.
//!
//! ## Frozen constants (docs/14 §4.1, §5.4, §5.5)
//!
//! | Constant | Value | Meaning |
//! | --- | --- | --- |
//! | [`DEFAULT_DWELL_MS`] | 120 ms | cursor must rest this long before a preview appears |
//! | [`DEFAULT_SNAP_RADIUS_PX`] | 24 px | nearest-window search radius, physical pixels |
//! | [`DEFAULT_HOVER_REVALIDATE_MS`] | 250 ms | worker re-validation cadence for the hovered window |
//!
//! ## v1 scope
//!
//! Only [`TargetKind::TopLevelWindowFrame`] exists: the DWM top-level window frame
//! including its title bar. Client-area and UIA/MSAA sub-element targets are v2 and
//! must never enter this path (docs/14 §5.2, §11).

pub mod gesture;
pub mod hit_test;
pub mod deep;
pub mod model;
pub mod provider;
pub mod snapshot;

pub use gesture::{
    GestureState, MoveOutcome, PointerGesture, PressOutcome, ReleaseOutcome, SnapPreview,
    should_start_manual_drag,
};
pub use deep::{
    DeepSelectionProvider, DeepTarget, QueryControl, RefinementJob, RefinementOutcome,
    RefinementScheduler, SchedulerActions, StopReason, UnsupportedDeepSelection,
    REFINEMENT_BUDGET_MS, REFINEMENT_CALL_LIMIT_MS, REFINEMENT_DWELL_MS,
    REFINEMENT_PUBLISH_INTERVAL_MS,
};
pub use hit_test::{point_distance_squared, rect_distance_squared};
pub use model::{
    EpochCounter, HoverValidity, RequestGate, RequestId, SnapshotEpoch, TargetKind, WindowCandidate,
    WindowIdentity, WindowSnapshot, WindowTarget,
};
pub use provider::{Exclusions, WindowDetectionError, WindowTargetProvider, is_shell_surface_class};
pub use snapshot::{
    CheapProbe, ClassifiedWindow, DwmRead, candidates_from_classified, class_name_hash, classify,
    identity_for, passes_cheap_policy, passes_dwm_policy,
};

/// Cursor rest time before an automatic-snap preview may appear (docs/14 §4.1).
pub const DEFAULT_DWELL_MS: u32 = 120;

/// Radius, in physical pixels, inside which a window may be snapped to.
pub const DEFAULT_SNAP_RADIUS_PX: u32 = 24;

/// How often the overlay asks the detection worker to re-validate the hovered
/// window (docs/14 §5.5). The timer only enqueues; the DWM read happens on the
/// worker thread.
pub const DEFAULT_HOVER_REVALIDATE_MS: u32 = 250;
