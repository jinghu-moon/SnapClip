//! What the user chose about the overlay's presentation, keys, and what a save should produce.
//!
//! The save block is not presentation — the overlay never draws it — but a scroll session has to
//! know the format before it starts scrolling (`docs/34` §15), so it rides along with the snapshot
//! the controller already receives.
//!
//! The shell owns the settings file; capture only receives the result. A session reads the
//! preferences once, when it starts, so a change made in the settings window applies from the next
//! capture and never rearranges a session the user is in the middle of.

use std::sync::Arc;

use crate::keymap::Keymap;
use crate::save_format::SavePreferences;
use crate::units::LabelDisplay;

#[derive(Debug, Clone, PartialEq)]
pub struct OverlayPreferences {
    /// Blur the frozen screen outside the selection, under the dimming. Presentation only: an
    /// exported image never contains it.
    pub mask_blur: bool,
    /// Show the shortcut guide in the bottom-left corner while picking.
    pub shortcut_guide: bool,
    /// How the size/coordinate badge reads — the unit, the origin, the unit's name — which is also
    /// what the badge's own menu changes. Presentation only: the capture measures and exports
    /// physical pixels whatever this says.
    pub label: LabelDisplay,
    pub keymap: Arc<Keymap>,
    /// What a save would produce: the format and the JPEG quality. Only the scroll pre-flight reads
    /// it today; the save path reads it when the panel lands.
    pub save: SavePreferences,
}

impl Default for OverlayPreferences {
    fn default() -> Self {
        Self {
            mask_blur: true,
            shortcut_guide: true,
            label: LabelDisplay::default(),
            keymap: Arc::new(Keymap::default()),
            save: SavePreferences::default(),
        }
    }
}
