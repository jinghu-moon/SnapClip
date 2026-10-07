//! Detection provider contract, exclusion sets and shell-surface policy
//! (docs/14 §5.2, §5.3, §7).
//!
//! The provider talks **only** in virtual-desktop physical pixels. It never receives
//! a `MonitorLayout`, never converts to client coordinates and never learns which
//! display is being captured: the overlay is the only party that knows that, and it
//! performs the conversion with `geometry::window_rect_to_local`.
//!
//! Every method that touches Win32/DWM is documented as worker-only. The overlay
//! thread may only call the pure cache operations on
//! [`crate::window_detection::WindowSnapshot`]; running a cross-process DWM
//! read there would block the message loop for the duration of the call, which is
//! exactly the promise the design makes it must not break.

use std::collections::HashSet;
use std::fmt;

use super::model::{HoverValidity, WindowSnapshot, WindowTarget};

/// Why a detection provider could not produce a snapshot.
///
/// A failure is never answered with a permanent empty snapshot: the caller keeps the
/// previous snapshot and retries on the next refresh trigger, so a transient
/// enumeration failure cannot silently turn window snapping off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowDetectionError {
    /// `EnumWindows` itself failed, or the post-enumeration probe batch did.
    EnumerationFailed(String),
}

impl fmt::Display for WindowDetectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EnumerationFailed(message) => {
                write!(formatter, "window enumeration failed: {message}")
            }
        }
    }
}

impl std::error::Error for WindowDetectionError {}

/// Windows that must never be offered as snap targets (docs/14 §5.3, §7).
///
/// Three layers keep SnapClip out of its own selection and out of the capture:
///
/// 1. the capture layer hides/excludes its own content (`WDA_EXCLUDEFROMCAPTURE`);
/// 2. this set keeps the overlay, toolbar, colour panel and main window from being
///    *hit* by detection;
/// 3. the capture backend keeps its own native exclusion list.
///
/// The three are independent on purpose: display affinity does **not** remove a window
/// from `EnumWindows`, so a snapshot filtered only by affinity would still offer the
/// overlay as a snap target.
///
/// The set carries an `epoch` that increments on every mutation. Detectors compare it
/// against the epoch their snapshot was built with; a mismatch invalidates the
/// snapshot, so a newly created toolbar cannot keep being a candidate for the rest of
/// the session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Exclusions {
    excluded_hwnds: HashSet<isize>,
    excluded_process_ids: HashSet<u32>,
    epoch: u64,
}

impl Exclusions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every exclusion currently registered, as a comparable version number.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn len_hwnds(&self) -> usize {
        self.excluded_hwnds.len()
    }

    pub fn len_process_ids(&self) -> usize {
        self.excluded_process_ids.len()
    }

    pub fn hwnds(&self) -> impl Iterator<Item = isize> + '_ {
        self.excluded_hwnds.iter().copied()
    }

    pub fn process_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.excluded_process_ids.iter().copied()
    }

    /// Exclude one window handle (overlay, toolbar, colour panel, ...).
    ///
    /// Returns whether the set actually changed; unchanged sets keep their epoch so a
    /// redundant registration does not invalidate a perfectly good snapshot.
    pub fn exclude_hwnd(&mut self, hwnd: isize) -> bool {
        if self.excluded_hwnds.insert(hwnd) {
            self.bump();
            return true;
        }
        false
    }

    /// Exclude an entire process. This is the belt-and-braces layer for windows
    /// SnapClip creates *after* the last snapshot refresh, where an HWND set would
    /// always be one step behind.
    pub fn exclude_process(&mut self, process_id: u32) -> bool {
        if self.excluded_process_ids.insert(process_id) {
            self.bump();
            return true;
        }
        false
    }

    pub fn forget_hwnd(&mut self, hwnd: isize) -> bool {
        if self.excluded_hwnds.remove(&hwnd) {
            self.bump();
            return true;
        }
        false
    }

    pub fn forget_process(&mut self, process_id: u32) -> bool {
        if self.excluded_process_ids.remove(&process_id) {
            self.bump();
            return true;
        }
        false
    }

    /// Whether a window must be skipped by detection.
    pub fn excludes(&self, hwnd: isize, process_id: u32) -> bool {
        self.excluded_hwnds.contains(&hwnd) || self.excluded_process_ids.contains(&process_id)
    }

    /// Drop every entry. Used when a session ends so nothing leaks into the next one.
    pub fn clear(&mut self) -> bool {
        if self.excluded_hwnds.is_empty() && self.excluded_process_ids.is_empty() {
            return false;
        }
        self.excluded_hwnds.clear();
        self.excluded_process_ids.clear();
        self.bump();
        true
    }

    fn bump(&mut self) {
        self.epoch = self.epoch.saturating_add(1).max(1);
    }
}

/// Shell surfaces that look like ordinary windows but are never "the window the user
/// pointed at": the desktop, the taskbars and the XAML islands hosting their flyouts.
///
/// This is an **exact** name list, not a prefix or style test. A style-based
/// rule (`WS_EX_TOOLWINDOW`, `WS_EX_NOACTIVATE`) would also discard browser popups,
/// developer tool windows and floating palettes that are genuinely visible and
/// genuinely screenshot-able (docs/14 §5.3).
///
/// `Windows.UI.Core.CoreWindow` covers UWP hosts; the two `*OverflowWindow*` /
/// `*OverflowXamlIsland` entries come from the reference implementation's list and
/// cover the tray overflow flyout, which is a top-level window on current Windows.
pub const SHELL_SURFACE_CLASSES: [&str; 8] = [
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "NotifyIconOverflowWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "Windows.UI.Core.CoreWindow",
    "XamlExplorerHostIslandWindow",
];

/// Whether a window class name is a shell surface that must be skipped.
///
/// Win32 class names are compared case-insensitively by the system itself, so the
/// comparison here is too; it stays exact otherwise (no prefixes, no wildcards).
pub fn is_shell_surface_class(class_name: &str) -> bool {
    SHELL_SURFACE_CLASSES
        .iter()
        .any(|surface| surface.eq_ignore_ascii_case(class_name))
}

/// Produces and re-validates window targets.
///
/// Implementations are owned by the detection worker. Implementations must not be
/// `Sync`: the v1 provider caches probe state between calls.
pub trait WindowTargetProvider {
    /// Enumerate the desktop and build a fresh snapshot.
    ///
    /// Runs on the detection worker. `EnumWindows` and every DWM read happen here and
    /// nowhere else.
    fn refresh(
        &mut self,
        exclusions: &Exclusions,
    ) -> Result<WindowSnapshot, WindowDetectionError>;

    /// Full re-validation of one target before an automatic snap is confirmed.
    ///
    /// Runs on the detection worker; the overlay only checks
    /// `epoch + identity + confirmation_id` on the returned verdict.
    fn validate(&self, target: &WindowTarget) -> bool;

    /// Lightweight re-validation of the hovered window (docs/14 §5.5).
    ///
    /// Runs on the detection worker, never on the overlay thread: it performs a
    /// synchronous `DwmGetWindowAttribute` that would otherwise stall the message loop.
    fn revalidate_hover(&self, hover: &WindowTarget) -> HoverValidity;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusions_match_by_hwnd_and_by_process() {
        let mut exclusions = Exclusions::new();
        assert!(!exclusions.excludes(0x100, 42));

        assert!(exclusions.exclude_hwnd(0x100));
        assert!(exclusions.excludes(0x100, 42));
        assert!(!exclusions.excludes(0x101, 42), "only the handle is excluded");

        assert!(exclusions.exclude_process(42));
        assert!(exclusions.excludes(0x999, 42), "process fallback covers new windows");
        assert!(!exclusions.excludes(0x999, 43));
        assert_eq!(exclusions.len_hwnds(), 1);
        assert_eq!(exclusions.len_process_ids(), 1);
    }

    #[test]
    fn the_exclusion_epoch_only_moves_when_the_set_changes() {
        let mut exclusions = Exclusions::new();
        assert_eq!(exclusions.epoch(), 0);

        assert!(exclusions.exclude_hwnd(0x10));
        let after_first = exclusions.epoch();
        assert!(after_first >= 1);

        // Registering the same handle again changes nothing, so a live snapshot
        // keeps its epoch and is not needlessly invalidated.
        assert!(!exclusions.exclude_hwnd(0x10));
        assert_eq!(exclusions.epoch(), after_first);

        assert!(exclusions.exclude_process(7));
        assert!(exclusions.epoch() > after_first);
        let after_process = exclusions.epoch();

        assert!(exclusions.forget_hwnd(0x10));
        assert!(exclusions.epoch() > after_process);
        assert!(!exclusions.forget_hwnd(0x10), "already gone");

        assert!(exclusions.clear());
        assert_eq!(exclusions.len_hwnds(), 0);
        assert_eq!(exclusions.len_process_ids(), 0);
        assert!(!exclusions.clear(), "clearing an empty set changes nothing");
    }

    #[test]
    fn shell_surfaces_are_matched_exactly() {
        for class in SHELL_SURFACE_CLASSES {
            assert!(is_shell_surface_class(class), "{class}");
            // Win32 compares class names case-insensitively.
            assert!(is_shell_surface_class(&class.to_ascii_lowercase()), "{class}");
        }
        // Real application windows are not shell surfaces, even when the name starts
        // with the same letters: the match is exact, not a prefix test.
        for class in [
            "Chrome_WidgetWin_1",
            "CascadiaWindow",
            "ProgmanChild",
            "Shell_TrayWndEx",
            "Notepad",
            "",
        ] {
            assert!(!is_shell_surface_class(class), "{class}");
        }
    }

    #[test]
    fn tool_window_and_no_activate_styles_are_not_shell_surfaces() {
        // The policy is name based on purpose: a tool window or a no-activate palette
        // is still a legitimate snap target (docs/14 §5.3). Styles never appear in
        // this decision, so a name-only helper is enough to pin the behaviour.
        assert!(!is_shell_surface_class("Chrome_SystemMessageWindow"));
        assert!(!is_shell_surface_class("DevTools_FloatingPanel"));
    }

    #[test]
    fn the_provider_contract_is_usable_without_win32() {
        // The trait must be implementable by a pure test double: the overlay only
        // consumes its results, never its internals.
        struct Stub;
        impl WindowTargetProvider for Stub {
            fn refresh(
                &mut self,
                _exclusions: &Exclusions,
            ) -> Result<WindowSnapshot, WindowDetectionError> {
                Ok(WindowSnapshot::empty())
            }
            fn validate(&self, _target: &WindowTarget) -> bool {
                false
            }
            fn revalidate_hover(&self, _hover: &WindowTarget) -> HoverValidity {
                HoverValidity::Invalid
            }
        }
        let mut stub = Stub;
        assert!(stub.refresh(&Exclusions::new()).unwrap().is_empty());
    }
}
