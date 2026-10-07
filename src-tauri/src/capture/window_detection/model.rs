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
use crate::capture::monitor_cache::MonitorCache;

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

    /// Adopt an id that was issued elsewhere as the newest request.
    ///
    /// Only the component that owns the *question* may issue ids, and there must be exactly one
    /// such component per pipeline: the refinement worker holds the scheduler's id instead of
    /// minting its own, because the result travels back tagged with it and the scheduler is the
    /// side that decides whether it is still the current question. Two independent counters
    /// matched only until the first session ended, and every result after that was rejected as
    /// stale.
    pub fn adopt(&mut self, id: RequestId) {
        self.next = self.next.max(id.0);
        self.latest = Some(id);
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
/// v1 uses [`Self::TopLevelWindowFrame`] only. `ClientArea` and `UiElement` belong to v2
/// (docs/18): they are produced by the refinement worker and must never widen or alter
/// the v1 whole-window path — a v2 failure falls back to the v1 frame, never the other
/// way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetKind {
    /// DWM top-level window frame: includes the title bar, excludes the invisible
    /// resize border.
    TopLevelWindowFrame,
    /// v2: the window's client area (no title bar, no borders).
    ClientArea,
    /// v2: the deepest interactive UI element under the cursor.
    UiElement,
}

impl TargetKind {
    /// Whether this target comes from the v2 refinement path.
    ///
    /// Used by the overlay to keep the two layers apart: v2 targets may be refined
    /// further or dropped back to the v1 frame, while a v1 frame is always the fallback
    /// and never needs a refinement query of its own.
    pub fn is_refined(self) -> bool {
        matches!(self, Self::ClientArea | Self::UiElement)
    }
}

/// One level of a deep path: where it is, and what kind of thing it is (docs/21 §5.24, B6).
///
/// The walk always knew both — `WalkNode.control_type` is read for every node — but the published
/// path kept only the rectangles, so every ancestor could be described as no more than "a container"
/// and the size label had nothing better to say than `容器`. Carrying the kind per level is what
/// lets the label name the thing the user is looking at (`846×272 px 代码块`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathLevel {
    /// Screen bounds in virtual-desktop physical pixels, the same space as
    /// [`WindowCandidate::screen_bounds`].
    pub rect: Rect,
    pub kind: LevelKind,
}

impl PathLevel {
    pub const fn new(rect: Rect, kind: LevelKind) -> Self {
        Self { rect, kind }
    }

    /// A level whose kind the transport could not say — an MSAA ancestor chain, or a provider that
    /// answered with a box but no control type.
    pub const fn unknown(rect: Rect) -> Self {
        Self {
            rect,
            kind: LevelKind::Unknown,
        }
    }
}

/// What a level is, normalised across the providers (docs/21 §5.24, B6).
///
/// UIA reports a `ControlTypeId` and MSAA a `Role`, and the two numberings share nothing — so each
/// provider maps its own magic numbers into this one vocabulary at its own boundary, and the walk,
/// the label, the log and the tests speak one language from there on.
///
/// Two views, two readers: [`Self::debug_name`] is what a session log prints (the UIA spelling) and
/// [`Self::noun_zh`] is what the size label prints. A kind may have one and not the other — `Pane`
/// is worth naming in a log and worth *not* naming on screen, where `容器` already says it better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LevelKind {
    #[default]
    Unknown,
    Button,
    CheckBox,
    ComboBox,
    Edit,
    Hyperlink,
    Image,
    ListItem,
    List,
    Menu,
    ProgressBar,
    RadioButton,
    ScrollBar,
    Slider,
    Tab,
    Text,
    ToolBar,
    Tree,
    Table,
    DataGrid,
    DataItem,
    Document,
    Window,
    TitleBar,
    Header,
    Custom,
    Group,
    Pane,
}

impl LevelKind {
    /// Every kind, for the exhaustive tests and for the font-subset list (docs/21 §5.23).
    ///
    /// A list rather than a derive: the two views below are `match`es, and a match cannot be
    /// enumerated at runtime — so this is the one place that has to be kept in step, and a test
    /// asserts it covers exactly the variants the matches do.
    pub const ALL: [LevelKind; 28] = [
        Self::Unknown,
        Self::Button,
        Self::CheckBox,
        Self::ComboBox,
        Self::Edit,
        Self::Hyperlink,
        Self::Image,
        Self::ListItem,
        Self::List,
        Self::Menu,
        Self::ProgressBar,
        Self::RadioButton,
        Self::ScrollBar,
        Self::Slider,
        Self::Tab,
        Self::Text,
        Self::ToolBar,
        Self::Tree,
        Self::Table,
        Self::DataGrid,
        Self::DataItem,
        Self::Document,
        Self::Window,
        Self::TitleBar,
        Self::Header,
        Self::Custom,
        Self::Group,
        Self::Pane,
    ];

    /// The nouns a label can show, for the font subset and its coverage gate (docs/21 §5.23).
    ///
    /// Derived from [`Self::noun_zh`], so a noun added there cannot be forgotten here — which is
    /// exactly the failure the gate exists to catch: a label that draws a glyph the embedded font
    /// does not have.
    pub fn label_nouns() -> impl Iterator<Item = &'static str> {
        Self::ALL.iter().filter_map(|kind| kind.noun_zh())
    }

    /// The name a session log prints: the UIA spelling, so a log line reads like the tree it came
    /// from. The kinds without a noun still have one — the log is exactly where "it was a `Group`"
    /// is worth saying.
    pub fn debug_name(self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::Button => "Button",
            Self::CheckBox => "CheckBox",
            Self::ComboBox => "ComboBox",
            Self::Edit => "Edit",
            Self::Hyperlink => "Hyperlink",
            Self::Image => "Image",
            Self::ListItem => "ListItem",
            Self::List => "List",
            Self::Menu => "Menu",
            Self::ProgressBar => "ProgressBar",
            Self::RadioButton => "RadioButton",
            Self::ScrollBar => "ScrollBar",
            Self::Slider => "Slider",
            Self::Tab => "Tab",
            Self::Text => "Text",
            Self::ToolBar => "ToolBar",
            Self::Tree => "Tree",
            Self::Table => "Table",
            Self::DataGrid => "DataGrid",
            Self::DataItem => "DataItem",
            Self::Document => "Document",
            Self::Window => "Window",
            Self::TitleBar => "TitleBar",
            Self::Header => "Header",
            Self::Custom => "Custom",
            Self::Group => "Group",
            Self::Pane => "Pane",
        }
    }

    /// The word the size label shows, or `None` where the label's own vocabulary already says it
    /// better (docs/21 §5.24, B6).
    ///
    /// Conservative on purpose: generic boxes — `Pane`, `Group`, `Custom` — get no noun, so the
    /// label keeps `容器`/`元素` and only becomes more specific where the transport actually knows
    /// more. Every noun is a new glyph in the embedded font subset (§5.23), which is why the list is
    /// short and the long tail (日历/状态栏/分割按钮…) is deliberately absent.
    pub fn noun_zh(self) -> Option<&'static str> {
        Some(match self {
            Self::Button => "按钮",
            Self::CheckBox => "复选框",
            Self::ComboBox => "下拉框",
            Self::Edit => "输入框",
            Self::Hyperlink => "链接",
            Self::Image => "图像",
            Self::ListItem => "列表项",
            Self::List => "列表",
            // `DataItem` is "an item in a list, grid or tree" — the same row a forum's topic list or
            // Explorer's file list reports. It shares `列表项` with `ListItem` rather than getting a
            // noun of its own: the type cannot say whether the row is a list item or a grid cell, and
            // a real-machine session pointed at a forum topic row is what asked for it.
            Self::DataItem => "列表项",
            Self::Menu => "菜单",
            Self::ProgressBar => "进度条",
            Self::RadioButton => "单选框",
            Self::ScrollBar => "滚动条",
            Self::Slider => "滑块",
            Self::Tab => "选项卡",
            Self::Text => "文本",
            Self::ToolBar => "工具栏",
            Self::Tree => "树",
            Self::Table | Self::DataGrid => "表格",
            Self::Document => "文档",
            Self::Window => "窗口",
            Self::Unknown
            | Self::Custom
            | Self::Group
            | Self::Pane
            | Self::Header
            | Self::TitleBar => return None,
        })
    }
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
/// the whole value when a refresh lands. Everything the snapshot needs — Z-ordered
/// candidates and the monitor rectangles they were filtered against — is owned by the
/// snapshot, so [`Self::release`] is a complete teardown and the next session cannot
/// inherit a single value.
///
/// There is deliberately **no** spatial index field: measurements in
/// [`crate::capture::window_detection::hit_test`] show a linear scan over realistic
/// candidate counts is far inside the latency budget, and the design forbids adding an
/// index without evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowSnapshot {
    /// Visible to the sibling algorithm modules (`hit_test`), never outside
    /// `window_detection`: callers read the snapshot through its accessors so the
    /// "queries only ever see live candidates" rule cannot be bypassed.
    pub(super) epoch: SnapshotEpoch,
    pub(super) candidates: Vec<WindowCandidate>,
    pub(super) monitors: MonitorCache,
}

impl WindowSnapshot {
    /// An empty placeholder for "no snapshot yet". Its epoch is `0`, which
    /// [`EpochCounter`] never issues, so it can never be mistaken for live data.
    pub const fn empty() -> Self {
        Self {
            epoch: 0,
            candidates: Vec::new(),
            monitors: MonitorCache::empty(),
        }
    }

    /// Adopt `candidates` in the given generation.
    ///
    /// Callers pass candidates already ordered by `z_order` (frontmost first); the
    /// ordering is asserted in tests rather than silently repaired, because a
    /// mis-ordered snapshot would make overlap resolution pick the wrong window.
    pub fn new(
        epoch: SnapshotEpoch,
        candidates: Vec<WindowCandidate>,
        monitors: MonitorCache,
    ) -> Self {
        debug_assert!(
            candidates
                .windows(2)
                .all(|pair| pair[0].z_order <= pair[1].z_order),
            "window snapshot candidates must be ordered by z_order (frontmost first)"
        );
        Self {
            epoch,
            candidates,
            monitors,
        }
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

    /// The monitor rectangles this snapshot was filtered against.
    pub fn monitors(&self) -> &MonitorCache {
        &self.monitors
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

    /// Drop every candidate and every cached monitor rectangle. Called when a session
    /// ends so the next session cannot hit-test against stale windows or a stale
    /// display topology.
    pub fn release(&mut self) {
        self.epoch = 0;
        self.candidates = Vec::new();
        self.monitors.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::geometry::Point;

    /// The vocabulary's two views, and the one list that has to stay in step with them
    /// (docs/21 §5.24, B6).
    #[test]
    fn every_level_kind_has_one_debug_name_and_at_most_one_noun() {
        // The list covers every variant: a kind added to the enum without being added here would
        // slip past both the font list and these checks.
        assert_eq!(LevelKind::ALL.len(), 28);
        let mut names: Vec<&str> = LevelKind::ALL.iter().map(|kind| kind.debug_name()).collect();
        names.sort_unstable();
        let unique = names.len();
        names.dedup();
        assert_eq!(names.len(), unique, "two kinds share a debug name");
        assert!(
            LevelKind::ALL.iter().any(|kind| kind.noun_zh().is_none()),
            "the generic kinds must stay un-named, or every box would get a noun"
        );
        // The nouns are the label's whole vocabulary: what the font subset is built from.
        let nouns: Vec<&str> = LevelKind::label_nouns().collect();
        assert!(nouns.contains(&"按钮") && nouns.contains(&"文本") && nouns.contains(&"窗口"));
        assert_eq!(
            nouns.len(),
            LevelKind::ALL
                .iter()
                .filter(|kind| kind.noun_zh().is_some())
                .count(),
            "one noun per kind that has one"
        );
        // A generic wrapper and an unknown box keep the label's own words.
        assert_eq!(LevelKind::Pane.noun_zh(), None);
        assert_eq!(LevelKind::Group.noun_zh(), None);
        assert_eq!(LevelKind::Unknown.noun_zh(), None);
        assert_eq!(LevelKind::Button.noun_zh(), Some("按钮"));
        // The row of a list/grid — a forum topic row is what put this on the list (docs/21 §5.24.9).
        assert_eq!(LevelKind::DataItem.noun_zh(), Some("列表项"));
        assert_eq!(LevelKind::ListItem.noun_zh(), LevelKind::DataItem.noun_zh());
        // The same kind under either name of a table.
        assert_eq!(LevelKind::DataGrid.noun_zh(), LevelKind::Table.noun_zh());
    }

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
        let live = WindowSnapshot::new(epoch, vec![candidate(1, 0, epoch)], MonitorCache::empty());
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
    fn an_adopted_id_becomes_the_newest_request_and_keeps_issue_monotonic() {
        // The refinement worker adopts the scheduler's id instead of issuing its own: the
        // result travels back tagged with it, and the scheduler — the side that owns the
        // question — is the one comparing. Two id spaces matched only by accident, and the
        // mismatch silently rejected every result after the first session (docs/18 §2).
        let mut gate = RequestGate::new();
        let adopted = RequestGate::new().issue();
        gate.adopt(adopted);
        assert_eq!(gate.latest(), Some(adopted), "the adopted id is current");
        assert!(gate.accepts(adopted));

        // Issuing after adopting must not hand out an id that was already used.
        let issued = gate.issue();
        assert!(issued > adopted);
        assert!(!gate.accepts(adopted), "the adopted id was superseded");
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
            MonitorCache::from_bounds([rect(0, 0, 1920, 1080)]),
        );
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot.monitors().len(), 1);
        assert!(snapshot.find(identity(0x200)).is_some());
        assert!(snapshot.find(identity(0x999)).is_none());
        assert!(snapshot.find(identity(0x100)).is_some());

        snapshot.release();
        assert_eq!(snapshot.epoch(), 0);
        assert!(snapshot.is_empty());
        assert!(snapshot.monitors().is_empty(), "the monitor cache is released too");
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
