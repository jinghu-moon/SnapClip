//! Snapshot input types and the two-stage filter policy (docs/14 §5.3).
//!
//! Building a snapshot is split so that **no DWM call can ever happen inside the
//! `EnumWindows` callback**:
//!
//! ```text
//! platform: EnumWindows            -> Vec<CheapProbe>   (user-mode checks only)
//! capture : pass 1 policy filter   -> Vec<CheapProbe>   (exclusions, shell surfaces)
//! platform: DwmGetWindowAttribute  -> Vec<DwmRead>      (batched, after the callback)
//! capture : pass 2 policy filter   -> candidates        (cloaked, empty, off-screen)
//! ```
//!
//! Both policy passes are pure functions over plain data, so the whole filter matrix
//! is unit-testable without a desktop; the platform passes are covered by real-window
//! tests in `platform::windows::capture::win::window`.

use super::model::{SnapshotEpoch, WindowCandidate, WindowIdentity};
use super::provider::{Exclusions, is_shell_surface_class};
use crate::capture::geometry::Rect;
use crate::capture::monitor_cache::MonitorCache;

/// One window that survived the cheap (`EnumWindows` callback) checks.
///
/// Still **before** any DWM read: it carries only material a same-process call can
/// produce without touching the compositor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheapProbe {
    pub hwnd: isize,
    pub process_id: u32,
    /// Class name as reported by `GetClassNameW`. Kept as text so the shell-surface
    /// blacklist can be matched exactly; the hash used for identity is derived once in
    /// [`WindowIdentity`] construction.
    pub class_name: String,
    /// `GWL_EXSTYLE`. The click-through test already ran in the callback; the raw
    /// value is kept so policy and diagnostics can re-inspect it without another call.
    pub extended_style: u32,
}

impl CheapProbe {
    pub fn new(
        hwnd: isize,
        process_id: u32,
        class_name: impl Into<String>,
        extended_style: u32,
    ) -> Self {
        Self {
            hwnd,
            process_id,
            class_name: class_name.into(),
            extended_style,
        }
    }
}

/// Result of the batched DWM pass for one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DwmRead {
    pub hwnd: isize,
    /// `DWMWA_CLOAKED != 0`: the window exists, `IsWindowVisible` may still be true,
    /// but the compositor is not showing it (closed store app, other virtual desktop,
    /// Snap Assist shadow copy).
    pub cloaked: bool,
    /// `DWMWA_EXTENDED_FRAME_BOUNDS`, or `GetWindowRect` when DWM could not supply a
    /// usable rectangle. `None` means both failed or produced an empty rectangle.
    pub frame_bounds: Option<Rect>,
}

impl DwmRead {
    pub const fn new(hwnd: isize, cloaked: bool, frame_bounds: Option<Rect>) -> Self {
        Self {
            hwnd,
            cloaked,
            frame_bounds,
        }
    }
}

/// Hash of a window class name, used as the recycle-detection component of
/// [`WindowIdentity`].
///
/// Any stable, collision-resistant digest works; the value never leaves the process
/// and is only ever compared for equality, so the first 8 bytes of a BLAKE3 digest are
/// plenty. Hashing (rather than storing the string) keeps `WindowIdentity` `Copy` and
/// `Send`, which the cross-thread result messages rely on.
pub fn class_name_hash(class_name: &str) -> u64 {
    let digest = blake3::hash(class_name.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest.as_bytes()[..8]);
    u64::from_le_bytes(bytes)
}

/// Pass 1: reject windows that never deserve a DWM read.
///
/// Two independent reasons:
///
/// * SnapClip's own surfaces ([`Exclusions`]) — by handle, and by process as a
///   fallback for windows created after the last snapshot;
/// * shell surfaces (desktop, taskbars, their flyout islands) by exact class name.
///
/// Styles are deliberately *not* consulted here beyond the click-through test that
/// already ran in the enumeration callback: `WS_EX_TOOLWINDOW` and `WS_EX_NOACTIVATE`
/// are legitimate windows the user may want to capture (docs/14 §5.3).
pub fn passes_cheap_policy(probe: &CheapProbe, exclusions: &Exclusions) -> bool {
    if exclusions.excludes(probe.hwnd, probe.process_id) {
        return false;
    }
    !is_shell_surface_class(&probe.class_name)
}

/// Pass 2: turn a DWM read into a usable screen rectangle, or reject the window.
///
/// Rejects, in order: cloaked windows, windows whose bounds could not be read, and
/// windows that do not overlap any active display (a helper parked far off-screen, or
/// a window on a monitor that was just unplugged).
pub fn passes_dwm_policy(read: &DwmRead, monitors: &MonitorCache) -> Option<Rect> {
    if read.cloaked {
        return None;
    }
    let bounds = read.frame_bounds?;
    if !monitors.intersects_any(bounds) {
        return None;
    }
    Some(bounds)
}

/// Assemble one identity from a surviving probe.
pub fn identity_for(probe: &CheapProbe) -> WindowIdentity {
    WindowIdentity::new(
        probe.hwnd,
        probe.process_id,
        class_name_hash(&probe.class_name),
    )
}

/// One fully classified window observation, ready to be ordered into a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedWindow {
    pub identity: WindowIdentity,
    pub screen_bounds: Rect,
}

/// Apply both passes to a probe batch and pair each survivor with its window identity.
///
/// `reads` comes from the batched DWM pass and is matched by handle; a probe with no
/// matching read is dropped (its DWM read failed outright).
pub fn classify(
    probes: &[CheapProbe],
    reads: &[DwmRead],
    exclusions: &Exclusions,
    monitors: &MonitorCache,
) -> Vec<ClassifiedWindow> {
    let mut classified = Vec::new();
    for probe in probes {
        if !passes_cheap_policy(probe, exclusions) {
            continue;
        }
        let Some(read) = reads.iter().find(|read| read.hwnd == probe.hwnd) else {
            continue;
        };
        let Some(bounds) = passes_dwm_policy(read, monitors) else {
            continue;
        };
        classified.push(ClassifiedWindow {
            identity: identity_for(probe),
            screen_bounds: bounds,
        });
    }
    classified
}

/// Turn classified windows into snapshot candidates, assigning `z_order` from the
/// probe order (frontmost first).
///
/// Kept separate from [`classify`] so the ordering and the epoch are applied in one
/// obvious place: the caller increments the epoch, this function stamps it onto every
/// candidate, and the snapshot built from them can never mix generations.
pub fn candidates_from_classified(
    classified: &[ClassifiedWindow],
    epoch: SnapshotEpoch,
) -> Vec<WindowCandidate> {
    classified
        .iter()
        .enumerate()
        .map(|(z_order, window)| {
            WindowCandidate::new(window.identity, window.screen_bounds, z_order as u32, epoch)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::window_detection::model::EpochCounter;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect::new(left, top, right, bottom)
    }

    fn probe(hwnd: isize, class_name: &str) -> CheapProbe {
        CheapProbe::new(hwnd, 100, class_name, 0)
    }

    fn monitors() -> MonitorCache {
        MonitorCache::from_bounds([rect(0, 0, 3840, 2160)])
    }

    #[test]
    fn class_name_hash_is_stable_and_class_sensitive() {
        assert_eq!(class_name_hash("Chrome_WidgetWin_1"), class_name_hash("Chrome_WidgetWin_1"));
        assert_ne!(class_name_hash("Chrome_WidgetWin_1"), class_name_hash("Chrome_WidgetWin_2"));
        assert_ne!(class_name_hash(""), class_name_hash(" "));
    }

    #[test]
    fn pass_one_rejects_our_own_windows_and_shell_surfaces() {
        let mut exclusions = Exclusions::new();
        exclusions.exclude_hwnd(0x10);
        exclusions.exclude_process(999);

        // Ordinary application window: kept.
        assert!(passes_cheap_policy(&probe(0x20, "Chrome_WidgetWin_1"), &exclusions));
        // The overlay itself.
        assert!(!passes_cheap_policy(&probe(0x10, "Chrome_WidgetWin_1"), &exclusions));
        // A window owned by SnapClip's process that we never registered.
        assert!(!passes_cheap_policy(
            &CheapProbe::new(0x30, 999, "SomeToolbarClass", 0),
            &exclusions
        ));
        // Shell surfaces.
        for class in ["Progman", "WorkerW", "Shell_TrayWnd", "XamlExplorerHostIslandWindow"] {
            assert!(!passes_cheap_policy(&probe(0x40, class), &exclusions), "{class}");
        }
    }

    #[test]
    fn pass_one_keeps_tool_and_no_activate_windows() {
        // `WS_EX_TOOLWINDOW` (0x80) and `WS_EX_NOACTIVATE` (0x0800_0000) are styles,
        // not disqualifiers: floating palettes and developer panels are real capture
        // targets (docs/14 §5.3).
        let exclusions = Exclusions::new();
        let tool = CheapProbe::new(0x50, 12, "DevTools_FloatingPanel", 0x0000_0080);
        let no_activate = CheapProbe::new(0x51, 12, "Chrome_SystemMessageWindow", 0x0800_0000);
        assert!(passes_cheap_policy(&tool, &exclusions));
        assert!(passes_cheap_policy(&no_activate, &exclusions));
    }

    #[test]
    fn pass_two_rejects_cloaked_unreadable_and_off_screen_windows() {
        let monitors = monitors();
        // Visible, on-monitor window.
        assert_eq!(
            passes_dwm_policy(&DwmRead::new(0x10, false, Some(rect(100, 100, 400, 300))), &monitors),
            Some(rect(100, 100, 400, 300))
        );
        // Cloaked: rejected even though the bounds are perfectly good.
        assert_eq!(
            passes_dwm_policy(&DwmRead::new(0x11, true, Some(rect(100, 100, 400, 300))), &monitors),
            None
        );
        // Both DWM and GetWindowRect failed.
        assert_eq!(passes_dwm_policy(&DwmRead::new(0x12, false, None), &monitors), None);
        // Entirely off every active display.
        assert_eq!(
            passes_dwm_policy(&DwmRead::new(0x13, false, Some(rect(5000, 5000, 5100, 5100))), &monitors),
            None
        );
        // Degenerate rectangle.
        assert_eq!(
            passes_dwm_policy(&DwmRead::new(0x14, false, Some(rect(50, 50, 50, 90))), &monitors),
            None
        );
    }

    #[test]
    fn classify_applies_both_passes_and_pairs_identities() {
        let mut exclusions = Exclusions::new();
        exclusions.exclude_hwnd(0x2);
        let probes = vec![
            probe(0x1, "Chrome_WidgetWin_1"),
            probe(0x2, "Chrome_WidgetWin_1"), // excluded
            probe(0x3, "Progman"),            // shell surface
            probe(0x4, "CascadiaWindow"),     // cloaked
            probe(0x5, "Notepad"),            // no DWM read at all
        ];
        let reads = vec![
            DwmRead::new(0x1, false, Some(rect(0, 0, 800, 600))),
            DwmRead::new(0x2, false, Some(rect(10, 10, 200, 200))),
            DwmRead::new(0x3, false, Some(rect(0, 0, 3840, 2160))),
            DwmRead::new(0x4, true, Some(rect(0, 0, 800, 600))),
        ];
        let classified = classify(&probes, &reads, &exclusions, &monitors());
        assert_eq!(classified.len(), 1);
        assert_eq!(classified[0].identity.hwnd, 0x1);
        assert_eq!(classified[0].identity.process_id, 100);
        assert_eq!(
            classified[0].identity.class_name_hash,
            class_name_hash("Chrome_WidgetWin_1")
        );
        assert_eq!(classified[0].screen_bounds, rect(0, 0, 800, 600));
    }

    #[test]
    fn candidates_keep_z_order_and_take_the_current_epoch() {
        let mut epochs = EpochCounter::new();
        let epoch = epochs.bump();
        let classified = vec![
            ClassifiedWindow {
                identity: WindowIdentity::new(0xA, 1, 11),
                screen_bounds: rect(0, 0, 100, 100),
            },
            ClassifiedWindow {
                identity: WindowIdentity::new(0xB, 2, 22),
                screen_bounds: rect(50, 50, 150, 150),
            },
        ];
        let candidates = candidates_from_classified(&classified, epoch);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].z_order, 0, "frontmost first");
        assert_eq!(candidates[1].z_order, 1);
        assert!(candidates.iter().all(|candidate| candidate.snapshot_epoch == epoch));
        assert!(candidates.iter().all(|candidate| candidate.client_bounds.is_none()));
    }

    #[test]
    fn a_window_spanning_off_screen_is_kept_while_its_visible_part_exists() {
        let monitors = monitors();
        // Hangs off the left edge but is still partly on the display.
        let read = DwmRead::new(0x60, false, Some(rect(-500, 100, 200, 400)));
        assert_eq!(passes_dwm_policy(&read, &monitors), Some(rect(-500, 100, 200, 400)));
        // Just past the right edge: the half-open rule makes this invisible.
        let read = DwmRead::new(0x61, false, Some(rect(3840, 100, 4000, 400)));
        assert_eq!(passes_dwm_policy(&read, &monitors), None);
    }
}
