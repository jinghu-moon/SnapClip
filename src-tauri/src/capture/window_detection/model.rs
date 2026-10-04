//! Data contracts for window detection (docs/14 §5.1, §5.2, §5.4).
//!
//! Everything here is plain data plus pure versioning helpers, so the contracts can
//! be unit tested without a window station and reused across the overlay thread and
//! the detection worker.
//!
//! ## Why a window needs an identity, not just an `HWND`
//!
//! `HWND` values are recycled. A snapshot entry that only remembered `hwnd + rect`
//! could be "confirmed" after the original window closed if another window had
//! already reused the handle, and the overlay would snap to a rectangle that no
//! longer belongs to the window the user pointed at. [`WindowIdentity`] therefore
//! pairs the handle with the owning process id and a hash of the window class name;
//! confirming a target re-checks all three (docs/14 §5.4).
//!
//! ## Why a snapshot needs an epoch
//!
//! A refresh runs on the detection worker while the overlay keeps serving input
//! against the previous snapshot. Results of the previous generation must not leak
//! into the new one, so every candidate records the [`SnapshotEpoch`] it came from
//! and every asynchronous result is checked against the epoch it was produced for.

use crate::capture::geometry::Rect;

/// Version of one [`WindowSnapshot`].
///
/// Epochs are issued by [`EpochCounter`] and increase strictly within a capture
/// session. `0` is reserved for "no snapshot yet" so a default-initialised epoch can
/// never match a real one.
pub type SnapshotEpoch = u64;

/// Issues strictly increasing snapshot epochs.
///
/// The counter intentionally never wraps back to `0`: a stale result that was
/// produced against an old epoch can therefore never be mistaken for a fresh one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EpochCounter {
    next: SnapshotEpoch,
}

impl EpochCounter {
    pub const fn new() -> Self {
        Self { next: 0 }
    }

    /// Take the next epoch. The first returned value is `1`.
    pub fn bump(&mut self) -> SnapshotEpoch {
        self.next = self.next.saturating_add(1).max(1);
        self.next
    }

    /// The most recently issued epoch, or `None` before the first [`Self::bump`].
    pub fn current(&self) -> Option<SnapshotEpoch> {
        (self.next != 0).then_some(self.next)
    }

    /// Forget the current epoch. Used when a capture session ends: the next session
    /// starts from `1` again, and nothing from the previous session can match.
    pub fn reset(&mut self) {
        self.next = 0;
    }
}

/// Identifier of one request handed to the detection worker (docs/14 §5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(u64);

impl RequestId {
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Single-flight gate for the detection worker mailbox.
///
/// The worker only ever needs the *newest* request: a hover re-validation for a point
/// the cursor has already left is worthless. The gate issues ids, remembers the
/// latest one and answers whether an arriving result is still current. That is the
/// mechanism behind `stale result dropped` in docs/14 §10.2.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RequestGate {
    next: u64,
    latest: Option<RequestId>,
}

impl RequestGate {
    pub const fn new() -> Self {
        Self {
            next: 0,
            latest: None,
        }
    }

    /// Issue a new request id and make it the only one that may still apply.
    pub fn issue(&mut self) -> RequestId {
        self.next = self.next.saturating_add(1).max(1);
        let id = RequestId(self.next);
        self.latest = Some(id);
        id
    }

    /// The newest outstanding request, if any.
    pub fn latest(&self) -> Option<RequestId> {
        self.latest
    }

    /// Whether a result tagged `id` may still be applied.
    pub fn accepts(&self, id: RequestId) -> bool {
        self.latest == Some(id)
    }

    /// Retire the current request without issuing a new one.
    pub fn retire(&mut self) {
        self.latest = None;
    }
}

/// Stable identity of a top-level window (docs/14 §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowIdentity {
    /// `HWND` stored as an integer so the value stays `Send` across threads.
    pub hwnd: isize,
    /// Owning process, from `GetWindowThreadProcessId`.
    pub process_id: u32,
    /// Hash of `GetClassNameW`; distinguishes a recycled handle from its predecessor.
    pub class_name_hash: u64,
}

impl WindowIdentity {
    pub const fn new(hwnd: isize, process_id: u32, class_name_hash: u64) -> Self {
        Self {
            hwnd,
            process_id,
            class_name_hash,
        }
    }

    /// Whether two observations describe the same window.
    ///
    /// A handle that survived a close/open cycle fails this check because the
    /// process id or the class hash changes; see the module docs.
    pub fn matches(self, other: Self) -> bool {
        self == other
    }
}

/// What kind of region a [`WindowTarget`] describes.
///
/// v1 has exactly one variant. `ClientArea` and `UiElement` are v2 and must be added
/// together with their provider, never by widening the v1 whole-window path
/// (docs/14 §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetKind {
    /// DWM top-level window frame: includes the title bar, excludes the invisible
    /// resize border.
    TopLevelWindowFrame,
}

/// One window observation inside a snapshot (docs/14 §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowCandidate {
    pub identity: WindowIdentity,
    /// DWM extended frame bounds in **virtual-desktop physical pixels**.
    ///
    /// Never monitor-local: converting to the overlay's client space is
    /// `geometry::window_rect_to_local`, and only the overlay does it (docs/14 §5.2).
    pub screen_bounds: Rect,
    /// Reserved for the v2 client-area target; always `None` in v1.
    pub client_bounds: Option<Rect>,
    /// `EnumWindows` visit order, `0` = frontmost. Lower wins on overlap.
    pub z_order: u32,
    /// Snapshot this candidate was built for.
    pub snapshot_epoch: SnapshotEpoch,
}

impl WindowCandidate {
    pub const fn new(
        identity: WindowIdentity,
        screen_bounds: Rect,
        z_order: u32,
        snapshot_epoch: SnapshotEpoch,
    ) -> Self {
        Self {
            identity,
            screen_bounds,
            client_bounds: None,
            z_order,
            snapshot_epoch,
        }
    }

    /// A degenerate rectangle can never be snapped to.
    pub fn is_usable(&self) -> bool {
        !self.screen_bounds.is_empty()
    }
}

/// A candidate the overlay may preview or confirm (docs/14 §5.1).
///
/// Deliberately carries no monitor-local geometry: the target is expressed in
/// virtual-desktop physical pixels and the overlay clips it to its own monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowTarget {
    pub candidate: WindowCandidate,
    pub target_kind: TargetKind,
}

impl WindowTarget {
    pub const fn top_level_window_frame(candidate: WindowCandidate) -> Self {
        Self {
            candidate,
            target_kind: TargetKind::TopLevelWindowFrame,
        }
    }

    pub fn identity(&self) -> WindowIdentity {
        self.candidate.identity
    }

    pub fn screen_bounds(&self) -> Rect {
        self.candidate.screen_bounds
    }

    /// Whether this target still belongs to the snapshot generation `epoch`.
    pub fn is_stale_for(&self, epoch: SnapshotEpoch) -> bool {
        self.candidate.snapshot_epoch != epoch
    }
}

/// Result of re-validating the hovered window on the detection worker
/// (docs/14 §5.2, §5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoverValidity {
    /// The window is still visible, uncloaked, same identity, same bounds.
    Valid,
    /// Same window, new frame bounds. The overlay must write them back into the
    /// snapshot before re-hit-testing, otherwise hover/preview jump to the old
    /// rectangle on the next mouse move.
    BoundsChanged {
        epoch: SnapshotEpoch,
        identity: WindowIdentity,
        new_bounds: Rect,
    },
    /// Window closed, hidden, minimised, cloaked, or its identity was reused.
    Invalid,
}

impl HoverValidity {
    /// Whether this result may still be applied to the snapshot/hover it was
    /// produced for. A mismatched epoch or identity is a no-op, never a mutation.
    pub fn applies_to(&self, epoch: SnapshotEpoch, identity: WindowIdentity) -> bool {
        match self {
            Self::Valid | Self::Invalid => true,
            Self::BoundsChanged {
                epoch: result_epoch,
                identity: result_identity,
                ..
            } => *result_epoch == epoch && *result_identity == identity,
        }
    }

    /// The bounds the snapshot should adopt, when the result carries new geometry.
    pub fn changed_bounds(&self) -> Option<Rect> {
        match self {
            Self::BoundsChanged { new_bounds, .. } => Some(*new_bounds),
            Self::Valid | Self::Invalid => None,
        }
    }
}

/// One immutable window observation set for a capture session (docs/14 §5.1).
///
/// Built on the detection worker and read on the overlay thread; the overlay swaps
/// the whole value when a refresh lands. The monitor cache and the optional spatial
/// index join this struct in the geometry/index phases.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowSnapshot {
    epoch: SnapshotEpoch,
    candidates: Vec<WindowCandidate>,
}

impl WindowSnapshot {
    /// An empty placeholder for "no snapshot yet". Its epoch is `0`, which
    /// [`EpochCounter`] never issues, so it can never be mistaken for live data.
    pub const fn empty() -> Self {
        Self {
            epoch: 0,
            candidates: Vec::new(),
        }
    }

    /// Adopt `candidates` in the given generation.
    ///
    /// Callers pass candidates already ordered by `z_order` (frontmost first); the
    /// ordering is asserted in tests rather than silently repaired, because a
    /// mis-ordered snapshot would make overlap resolution pick the wrong window.
    pub fn new(epoch: SnapshotEpoch, candidates: Vec<WindowCandidate>) -> Self {
        debug_assert!(
            candidates
                .windows(2)
                .all(|pair| pair[0].z_order <= pair[1].z_order),
            "window snapshot candidates must be ordered by z_order (frontmost first)"
        );
        Self { epoch, candidates }
    }

    pub fn epoch(&self) -> SnapshotEpoch {
        self.epoch
    }

    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    pub fn candidates(&self) -> &[WindowCandidate] {
        &self.candidates
    }

    /// Whether this snapshot carries data for `epoch`.
    pub fn is_current(&self, epoch: SnapshotEpoch) -> bool {
        self.epoch != 0 && self.epoch == epoch
    }

    /// The candidate with this identity, if the snapshot still holds one.
    pub fn find(&self, identity: WindowIdentity) -> Option<&WindowCandidate> {
        self.candidates
            .iter()
            .find(|candidate| candidate.identity == identity)
    }

    /// Drop every candidate and release the backing allocation. Called when a
    /// session ends so the next session cannot hit-test against stale windows.
    pub fn release(&mut self) {
        self.epoch = 0;
        self.candidates = Vec::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::geometry::Point;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect::new(left, top, right, bottom)
    }

    fn identity(hwnd: isize) -> WindowIdentity {
        WindowIdentity::new(hwnd, 4242, 0xC0FFEE)
    }

    fn candidate(hwnd: isize, z_order: u32, epoch: SnapshotEpoch) -> WindowCandidate {
        WindowCandidate::new(identity(hwnd), rect(0, 0, 100, 100), z_order, epoch)
    }

    #[test]
    fn epochs_start_at_one_and_increase_strictly() {
        let mut counter = EpochCounter::new();
        assert_eq!(counter.current(), None, "no epoch before the first bump");
        assert_eq!(counter.bump(), 1);
        assert_eq!(counter.bump(), 2);
        assert_eq!(counter.bump(), 3);
        assert_eq!(counter.current(), Some(3));
        counter.reset();
        assert_eq!(counter.current(), None);
        assert_eq!(counter.bump(), 1, "a new session restarts at 1");
    }

    #[test]
    fn the_zero_epoch_never_matches_a_live_snapshot() {
        // A default-constructed snapshot must not answer `is_current(0)` as if it
        // held data: that is what makes "no snapshot yet" distinguishable.
        let snapshot = WindowSnapshot::empty();
        assert_eq!(snapshot.epoch(), 0);
        assert!(!snapshot.is_current(0));

        let mut counter = EpochCounter::new();
        let epoch = counter.bump();
        assert_ne!(epoch, 0);
        let live = WindowSnapshot::new(epoch, vec![candidate(1, 0, epoch)]);
        assert!(live.is_current(epoch));
        // The placeholder still cannot impersonate the live snapshot.
        assert!(!snapshot.is_current(epoch));
    }

    #[test]
    fn request_gate_keeps_only_the_newest_request() {
        let mut gate = RequestGate::new();
        assert_eq!(gate.latest(), None);
        assert!(!gate.accepts(RequestId(1)), "nothing outstanding yet");

        let first = gate.issue();
        assert!(gate.accepts(first));
        assert_eq!(gate.latest(), Some(first));

        let second = gate.issue();
        assert!(gate.accepts(second));
        assert!(
            !gate.accepts(first),
            "a superseded request must be dropped when it lands"
        );
        // Ids are strictly increasing, so "newest" is also "greatest".
        assert!(second > first);

        gate.retire();
        assert_eq!(gate.latest(), None);
        assert!(!gate.accepts(second), "no request is current after retiring");
    }

    #[test]
    fn identity_mismatch_means_a_reused_handle_not_the_same_window() {
        let original = WindowIdentity::new(0x1000, 7, 0xAAAA);
        assert!(original.matches(original));
        // Recycled handle, different process.
        assert!(!original.matches(WindowIdentity::new(0x1000, 9, 0xAAAA)));
        // Recycled handle, same process, different window class.
        assert!(!original.matches(WindowIdentity::new(0x1000, 7, 0xBBBB)));
        assert!(!original.matches(WindowIdentity::new(0x2000, 7, 0xAAAA)));
    }

    #[test]
    fn a_target_knows_when_its_snapshot_generation_is_gone() {
        let epoch = 5;
        let target = WindowTarget::top_level_window_frame(candidate(0x10, 0, epoch));
        assert_eq!(target.target_kind, TargetKind::TopLevelWindowFrame);
        assert_eq!(target.identity(), identity(0x10));
        assert_eq!(target.screen_bounds(), rect(0, 0, 100, 100));
        assert!(!target.is_stale_for(epoch));
        assert!(target.is_stale_for(epoch + 1));
    }

    #[test]
    fn bounds_changed_is_ignored_unless_epoch_and_identity_both_match() {
        let epoch = 3;
        let window = identity(0x20);
        let changed = HoverValidity::BoundsChanged {
            epoch,
            identity: window,
            new_bounds: rect(10, 10, 210, 210),
        };
        assert!(changed.applies_to(epoch, window));
        assert_eq!(changed.changed_bounds(), Some(rect(10, 10, 210, 210)));

        // An old snapshot epoch must not rewrite the live snapshot.
        assert!(!changed.applies_to(epoch + 1, window));
        // A recycled HWND must not move the hover highlight.
        assert!(!changed.applies_to(epoch, WindowIdentity::new(0x20, 1, 2)));
        // Identity/epoch-free verdicts carry no geometry to write back.
        assert_eq!(HoverValidity::Valid.changed_bounds(), None);
        assert!(HoverValidity::Valid.applies_to(epoch, window));
        assert!(HoverValidity::Invalid.applies_to(epoch, window));
    }

    #[test]
    fn snapshot_lookup_is_by_identity_and_release_drops_everything() {
        let epoch = 4;
        let mut snapshot = WindowSnapshot::new(
            epoch,
            vec![candidate(0x100, 0, epoch), candidate(0x200, 1, epoch)],
        );
        assert_eq!(snapshot.len(), 2);
        assert!(snapshot.find(identity(0x200)).is_some());
        assert!(snapshot.find(identity(0x999)).is_none());
        assert!(snapshot.find(identity(0x100)).is_some());

        snapshot.release();
        assert_eq!(snapshot.epoch(), 0);
        assert!(snapshot.is_empty());
        assert!(
            snapshot.find(identity(0x100)).is_none(),
            "a released snapshot must not keep answering hits"
        );
        assert!(!snapshot.is_current(epoch));
    }

    #[test]
    fn degenerate_candidates_are_marked_unusable() {
        let mut zero_width = candidate(0x300, 0, 1);
        zero_width.screen_bounds = rect(50, 50, 50, 120);
        assert!(!zero_width.is_usable());

        let mut inverted = candidate(0x301, 1, 1);
        inverted.screen_bounds = rect(120, 50, 50, 120);
        assert!(!inverted.is_usable());

        assert!(candidate(0x302, 2, 1).is_usable());
    }

    #[test]
    fn candidate_bounds_stay_in_screen_space_and_are_half_open() {
        // The contract the overlay relies on: the candidate keeps virtual-desktop
        // coordinates (negative origins included) and `Rect::contains` is half-open,
        // so a point on the right/bottom edge belongs to the neighbour.
        let mut outside_primary = candidate(0x400, 0, 1);
        outside_primary.screen_bounds = rect(-1920, -100, 0, 980);
        assert_eq!(outside_primary.screen_bounds.left, -1920);
        assert!(
            outside_primary
                .screen_bounds
                .contains(Point::new(-1920, 0)),
            "the left/top edge belongs to the window"
        );
        assert!(outside_primary.screen_bounds.contains(Point::new(-1, 100)));
        assert!(
            !outside_primary.screen_bounds.contains(Point::new(0, 100)),
            "right edge is exclusive"
        );
        assert!(
            !outside_primary.screen_bounds.contains(Point::new(-500, 980)),
            "bottom edge is exclusive"
        );
    }
}
