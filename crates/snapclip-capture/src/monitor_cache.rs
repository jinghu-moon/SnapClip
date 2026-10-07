//! Monitor rectangles for one snapshot refresh cycle (docs/14 §5.1, §5.3).
//!
//! Classifying a window as "on screen" would otherwise cost a `MonitorFromRect` (and
//! then `GetMonitorInfoW`) per candidate, every refresh. Both are cheap individually
//! and wasteful in aggregate — the reference selector replaced exactly that pattern
//! with a cached rectangle list and an in-process intersection test, and that is what
//! this type reproduces.
//!
//! The cache is part of the [`crate::window_detection::WindowSnapshot`]
//! lifecycle: it is replaced when a refresh lands and released when the session ends,
//! so a cancelled session cannot leave a stale display topology behind.
//!
//! Everything here is platform neutral: enumerating the displays is
//! `platform::windows::capture::monitor`'s job, and this module only holds the result.

use super::geometry::{MonitorLayout, Rect};

/// Physical rectangles of every active display in the virtual desktop.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MonitorCache {
    bounds: Vec<Rect>,
}

impl MonitorCache {
    /// No displays — a placeholder that classifies every rectangle as off-screen.
    pub const fn empty() -> Self {
        Self { bounds: Vec::new() }
    }

    /// Build from physical rectangles, dropping degenerate ones.
    ///
    /// A zero-area monitor rectangle would make every window look off-screen, so an
    /// unusable entry is dropped rather than trusted.
    pub fn from_bounds(bounds: impl IntoIterator<Item = Rect>) -> Self {
        let bounds = bounds
            .into_iter()
            .filter(|rect| !rect.is_empty())
            .collect();
        Self { bounds }
    }

    /// Build from the monitor layouts the capture path already resolved.
    pub fn from_layouts(layouts: &[MonitorLayout]) -> Self {
        Self::from_bounds(layouts.iter().map(|layout| layout.bounds))
    }

    pub fn len(&self) -> usize {
        self.bounds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bounds.is_empty()
    }

    pub fn bounds(&self) -> &[Rect] {
        &self.bounds
    }

    /// Whether any part of `rect` falls on an active display.
    ///
    /// Useless windows — a helper parked far off-screen, a window on a monitor that
    /// was just unplugged — are rejected with this before any DWM call. The test is
    /// half-open, so a rectangle that only *touches* a monitor edge does not count as
    /// visible.
    pub fn intersects_any(&self, rect: Rect) -> bool {
        !rect.is_empty()
            && self
                .bounds
                .iter()
                .any(|bounds| !rect.intersect(*bounds).is_empty())
    }

    /// Drop the cached topology and release the backing allocation.
    pub fn release(&mut self) {
        self.bounds = Vec::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect::new(left, top, right, bottom)
    }

    #[test]
    fn an_empty_cache_classifies_every_rectangle_as_invisible() {
        let cache = MonitorCache::empty();
        assert!(cache.is_empty());
        assert!(!cache.intersects_any(rect(0, 0, 100, 100)));
    }

    #[test]
    fn degenerate_monitor_rectangles_are_dropped() {
        let cache = MonitorCache::from_bounds([
            rect(0, 0, 0, 1080),
            rect(0, 0, 1920, 1080),
            rect(100, 100, 100, 100),
        ]);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bounds()[0], rect(0, 0, 1920, 1080));
    }

    #[test]
    fn negative_virtual_desktop_origins_are_handled() {
        // A monitor left of and above the primary one.
        let cache = MonitorCache::from_bounds([rect(-1920, -200, 0, 880), rect(0, 0, 1920, 1080)]);
        assert!(cache.intersects_any(rect(-1000, -100, -900, 0)));
        assert!(cache.intersects_any(rect(10, 10, 20, 20)));
        // The gap between the two displays is not visible.
        assert!(!cache.intersects_any(rect(-1900, 900, -1800, 1000)));
        assert!(!cache.intersects_any(rect(2000, 0, 2100, 100)));
    }

    #[test]
    fn intersection_is_half_open_at_monitor_edges() {
        let cache = MonitorCache::from_bounds([rect(0, 0, 100, 100)]);
        // Touching the right/bottom edge is not an overlap.
        assert!(!cache.intersects_any(rect(100, 0, 200, 100)));
        assert!(!cache.intersects_any(rect(0, 100, 100, 200)));
        // One pixel inside is.
        assert!(cache.intersects_any(rect(99, 99, 200, 200)));
        // A degenerate rectangle never overlaps.
        assert!(!cache.intersects_any(rect(50, 50, 50, 60)));
    }

    #[test]
    fn a_window_spanning_two_monitors_is_visible_on_both() {
        let cache = MonitorCache::from_bounds([rect(0, 0, 1920, 1080), rect(1920, 0, 3840, 1080)]);
        assert!(cache.intersects_any(rect(1800, 100, 2100, 400)));
    }

    #[test]
    fn layouts_convert_to_their_bounds() {
        let layouts = [
            MonitorLayout {
                bounds: rect(-1920, 0, 0, 1080),
                work_area: rect(-1920, 0, 0, 1040),
                dpi: 96,
                primary: false,
            },
            MonitorLayout {
                bounds: rect(0, 0, 2560, 1440),
                work_area: rect(0, 0, 2560, 1400),
                dpi: 144,
                primary: true,
            },
        ];
        let cache = MonitorCache::from_layouts(&layouts);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.bounds()[0], layouts[0].bounds);
        assert_eq!(cache.bounds()[1], layouts[1].bounds);
    }

    #[test]
    fn release_drops_the_topology() {
        let mut cache = MonitorCache::from_bounds([rect(0, 0, 1920, 1080)]);
        assert!(cache.intersects_any(rect(0, 0, 10, 10)));
        cache.release();
        assert!(cache.is_empty());
        assert!(
            !cache.intersects_any(rect(0, 0, 10, 10)),
            "a released cache must not keep classifying windows as visible"
        );
    }
}
