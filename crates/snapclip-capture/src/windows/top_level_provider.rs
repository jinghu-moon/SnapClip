//! Windows top-level window provider (docs/14 §5.2, §5.3, §5.4).
//!
//! This is the only place where the platform FFI and the detection policy meet. It
//! lives in the platform layer on purpose: `capture/` must stay free of Win32 so the
//! contract, the filter and the hit-test algorithm can be unit tested without a desktop,
//! and so the dependency direction in `lib.rs` (platform → capture) is preserved.
//!
//! Every method here runs on the **detection worker**. `refresh` callers must not run it
//! from the overlay message thread: it performs `EnumWindows` plus one DWM round trip per
//! candidate, all of which block the caller.

use crate::geometry::Rect;
use crate::monitor_cache::MonitorCache;
use crate::window_detection::model::{
    EpochCounter, HoverValidity, WindowIdentity, WindowSnapshot, WindowTarget,
};
use crate::window_detection::provider::{
    Exclusions, WindowDetectionError, WindowTargetProvider,
};
use crate::window_detection::snapshot::{
    candidates_from_classified, class_name_hash, classify,
};

use super::monitor;
use super::win::window as win32;

/// Enumerates the desktop's top-level windows and validates snap targets.
#[derive(Debug, Default)]
pub struct TopLevelWindowProvider {
    /// Snapshot generations issued by this provider. Reset when a session ends so a
    /// new session cannot match a result from the previous one.
    epochs: EpochCounter,
    /// Display rectangles this provider's snapshots were filtered against.
    monitors: MonitorCache,
}

impl TopLevelWindowProvider {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget the current epoch and display topology.
    pub fn reset(&mut self) {
        self.epochs.reset();
        self.monitors.release();
    }

    /// Re-read the display topology once per refresh cycle, so window classification
    /// never issues a `MonitorFromRect` per candidate.
    fn refresh_monitors(&mut self) {
        self.monitors = MonitorCache::from_layouts(&monitor::enumerate());
    }

    /// Re-read one window's identity and frame, as the re-validation contract requires.
    ///
    /// Returns `None` when the window must be treated as gone: closed, hidden,
    /// minimised, cloaked, or recycled by a different process/window class.
    fn read_target(&self, identity: WindowIdentity) -> Option<Rect> {
        let hwnd = identity.hwnd;
        if !win32::is_window(hwnd)
            || !win32::is_window_visible(hwnd)
            || win32::is_iconic(hwnd)
            || win32::is_cloaked(hwnd)
        {
            return None;
        }
        // `HWND` reuse is the reason this check exists: the handle alone is not an
        // identity, so a recycled handle must not inherit the old rectangle.
        if win32::process_id(hwnd) != identity.process_id {
            return None;
        }
        let class_name = win32::class_name(hwnd)?;
        if class_name_hash(&class_name) != identity.class_name_hash {
            return None;
        }
        win32::frame_bounds(hwnd)
    }
}

impl WindowTargetProvider for TopLevelWindowProvider {
    fn refresh(
        &mut self,
        exclusions: &Exclusions,
    ) -> Result<WindowSnapshot, WindowDetectionError> {
        self.refresh_monitors();

        // Stage one: cheap user-mode enumeration (no DWM inside the callback).
        let probes = win32::enumerate_cheap_candidates()
            .map_err(WindowDetectionError::EnumerationFailed)?;

        // Stage two: one batched DWM pass over the survivors.
        let handles: Vec<isize> = probes.iter().map(|probe| probe.hwnd).collect();
        let reads = win32::read_dwm_batch(&handles);

        // Policy passes (exclusions, shell surfaces, cloaked, off-screen) run on plain
        // data; the epoch is stamped last so no candidate can carry a future value.
        let classified = classify(&probes, &reads, exclusions, &self.monitors);
        let epoch = self.epochs.bump();
        let candidates = candidates_from_classified(&classified, epoch);
        Ok(WindowSnapshot::new(epoch, candidates, self.monitors.clone()))
    }

    fn validate(&self, target: &WindowTarget) -> bool {
        self.read_target(target.identity())
            .is_some_and(|bounds| !bounds.is_empty())
    }

    fn revalidate_hover(&self, hover: &WindowTarget) -> HoverValidity {
        match self.read_target(hover.identity()) {
            None => HoverValidity::Invalid,
            Some(bounds) if bounds == hover.candidate.screen_bounds => HoverValidity::Valid,
            Some(bounds) => HoverValidity::BoundsChanged {
                epoch: hover.candidate.snapshot_epoch,
                identity: hover.identity(),
                new_bounds: bounds,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Point;
    use crate::window_detection::snapshot::CheapProbe;

    /// These tests read the real desktop, so they are `#[ignore]`d rather than skipped
    /// (`docs/31` D-14): running them with `--ignored` is asking for a live desktop, and a run
    /// that does not have one must fail loudly instead of passing having asserted nothing.
    fn require_desktop() {
        assert!(
            desktop_available(),
            "this test needs an interactive window station; run it with --ignored on a live \
             desktop (docs/31 D-14)"
        );
    }

    fn desktop_available() -> bool {
        let _ = win32::enumerate_cheap_candidates();
        monitor::set_per_monitor_v2_awareness().is_ok()
    }

    #[test]
    #[ignore = "L3 (docs/31 D-14): reads the real desktop topology; run with --ignored --test-threads=1 on a live desktop"]
    fn refresh_builds_a_labelled_snapshot_with_ordered_candidates() {
        require_desktop();
        let mut provider = TopLevelWindowProvider::new();
        let snapshot = provider.refresh(&Exclusions::new()).expect("refresh succeeds");
        assert!(snapshot.epoch() >= 1, "a refresh always issues a new epoch");
        assert!(!snapshot.monitors().is_empty(), "display topology was cached");
        assert!(
            !snapshot.is_empty(),
            "a desktop with at least this test process's window cannot be empty"
        );
        // Candidates are frontmost-first and all belong to this generation.
        let z_orders: Vec<u32> = snapshot.candidates().iter().map(|c| c.z_order).collect();
        assert!(z_orders.windows(2).all(|pair| pair[0] < pair[1]), "{z_orders:?}");
        assert!(snapshot.candidates().iter().all(|c| c.snapshot_epoch == snapshot.epoch()));
        // Every candidate is a real, non-degenerate, on-screen rectangle.
        for candidate in snapshot.candidates() {
            assert!(candidate.is_usable(), "{candidate:?}");
            assert!(
                snapshot.monitors().intersects_any(candidate.screen_bounds),
                "{candidate:?} is filtered to the visible area"
            );
        }
    }

    #[test]
    #[ignore = "L3 (docs/31 D-14): reads the real desktop topology; run with --ignored --test-threads=1 on a live desktop"]
    fn each_refresh_advances_the_epoch_and_drops_the_previous_snapshot() {
        require_desktop();
        let mut provider = TopLevelWindowProvider::new();
        let first = provider.refresh(&Exclusions::new()).expect("refresh succeeds");
        let second = provider.refresh(&Exclusions::new()).expect("refresh succeeds");
        assert!(second.epoch() > first.epoch());
        // A target from the previous generation is stale for the new snapshot.
        if let Some(target) = first.nearest_target(Point::new(0, 0), 4096) {
            assert!(target.is_stale_for(second.epoch()));
            // Even if the same window still exists, the candidate in the new snapshot
            // carries the new generation — never the old one.
            if let Some(candidate) = second.find(target.identity()) {
                assert_eq!(candidate.snapshot_epoch, second.epoch());
            }
        }
        // The old snapshot still answers for itself until it is dropped — that is what
        // makes the swap safe: the overlay reads one value and replaces it atomically.
        assert!(first.is_current(first.epoch()));
    }

    #[test]
    #[ignore = "L3 (docs/31 D-14): reads the real desktop topology; run with --ignored --test-threads=1 on a live desktop"]
    fn exclusions_remove_a_window_from_the_snapshot() {
        require_desktop();
        let mut provider = TopLevelWindowProvider::new();
        let snapshot = provider.refresh(&Exclusions::new()).expect("refresh succeeds");
        let victim = snapshot
            .candidates()
            .first()
            .copied()
            .expect("the desktop this test asked for must expose at least one top-level window");

        let mut exclusions = Exclusions::new();
        exclusions.exclude_hwnd(victim.identity.hwnd);
        let filtered = provider.refresh(&exclusions).expect("refresh succeeds");
        assert!(
            filtered.find(victim.identity).is_none(),
            "an excluded handle must not survive the filter"
        );
    }

    #[test]
    #[ignore = "L3 (docs/31 D-14): reads the real desktop topology; run with --ignored --test-threads=1 on a live desktop"]
    fn our_own_process_can_be_excluded_entirely() {
        require_desktop();
        let own_pid = std::process::id();
        let mut exclusions = Exclusions::new();
        exclusions.exclude_process(own_pid);
        let mut provider = TopLevelWindowProvider::new();
        let snapshot = provider.refresh(&exclusions).expect("refresh succeeds");
        assert!(
            snapshot
                .candidates()
                .iter()
                .all(|candidate| candidate.identity.process_id != own_pid),
            "the process exclusion is the fallback layer for SnapClip's own windows"
        );
    }

    #[test]
    fn a_filtered_probe_never_reaches_the_snapshot() {
        // Pure policy check with a synthetic batch: the provider's job is to run the
        // filters, and a shell surface must not appear even if its bounds are perfect.
        let probes = vec![CheapProbe::new(0x10, 1, "Progman", 0)];
        let reads = vec![crate::window_detection::snapshot::DwmRead::new(
            0x10,
            false,
            Some(Rect::new(0, 0, 1000, 1000)),
        )];
        let monitors = MonitorCache::from_bounds([Rect::new(0, 0, 1920, 1080)]);
        let classified = classify(&probes, &reads, &Exclusions::new(), &monitors);
        assert!(classified.is_empty());
    }

    #[test]
    fn validating_an_unknown_handle_fails_closed() {
        let provider = TopLevelWindowProvider::new();
        let target = WindowTarget::top_level_window_frame(
            crate::window_detection::model::WindowCandidate::new(
                WindowIdentity::new(0xDEAD, 1, 2),
                Rect::new(0, 0, 10, 10),
                0,
                1,
            ),
        );
        assert!(!provider.validate(&target));
        assert_eq!(provider.revalidate_hover(&target), HoverValidity::Invalid);
    }

    #[test]
    #[ignore = "L3 (docs/31 D-14): reads the real desktop topology; run with --ignored --test-threads=1 on a live desktop"]
    fn reset_clears_the_epoch_and_the_cached_topology() {
        require_desktop();
        let mut provider = TopLevelWindowProvider::new();
        let snapshot = provider.refresh(&Exclusions::new()).expect("refresh succeeds");
        assert!(snapshot.epoch() >= 1);
        assert!(!snapshot.monitors().is_empty());

        provider.reset();
        // The next session starts a fresh generation, so nothing from the previous one can
        // be matched, and the display cache is rebuilt from the current topology.
        let next = provider.refresh(&Exclusions::new()).expect("refresh succeeds");
        assert_eq!(next.epoch(), 1, "a reset restarts the epoch sequence");
        assert!(!next.monitors().is_empty(), "the cache is rebuilt for the new session");
    }

    #[test]
    #[ignore = "L3 (docs/31 D-14): reads the real desktop topology; run with --ignored --test-threads=1 on a live desktop"]
    fn hover_revalidation_reports_bounds_changes_and_stale_targets() {
        require_desktop();
        let mut provider = TopLevelWindowProvider::new();
        let snapshot = provider.refresh(&Exclusions::new()).expect("refresh succeeds");
        // The desktop is shared with everything else running on the machine, so a candidate
        // may legitimately disappear between the refresh and the validation below. Take the
        // first candidate that is *still* valid; a desktop where nothing at all is valid is not
        // the desktop this probe asked for, so it fails rather than passing vacuously.
        let candidate = snapshot
            .candidates()
            .iter()
            .filter(|candidate| candidate.is_usable())
            .map(|candidate| WindowTarget::top_level_window_frame(*candidate))
            .find(|target| provider.validate(target))
            .expect("no candidate from this refresh is still valid (docs/31 D-14)");
        let candidate = candidate.candidate;
        let target = WindowTarget::top_level_window_frame(candidate);

        // A freshly snapped target is still valid and has not moved.
        assert!(provider.validate(&target));
        assert_eq!(provider.revalidate_hover(&target), HoverValidity::Valid);

        // The same window with a stale rectangle reports the new bounds, tagged with
        // the snapshot generation the overlay must match.
        let mut moved = target;
        moved.candidate.screen_bounds = Rect::new(
            candidate.screen_bounds.left + 37,
            candidate.screen_bounds.top + 11,
            candidate.screen_bounds.right + 37,
            candidate.screen_bounds.bottom + 11,
        );
        match provider.revalidate_hover(&moved) {
            HoverValidity::BoundsChanged {
                epoch,
                identity,
                new_bounds,
            } => {
                assert_eq!(epoch, candidate.snapshot_epoch);
                assert_eq!(identity, candidate.identity);
                assert_eq!(new_bounds, candidate.screen_bounds);
            }
            other => panic!("expected a bounds change, got {other:?}"),
        }
    }
}
