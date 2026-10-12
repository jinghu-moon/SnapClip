//! What the user chose about the overlay's presentation and keys.
//!
//! The shell owns the settings file; capture only receives the result. A session reads the
//! preferences once, when it starts, so a change made in the settings window applies from the next
//! capture and never rearranges a session the user is in the middle of.

use std::sync::Arc;

use crate::keymap::Keymap;

#[derive(Debug, Clone, PartialEq)]
pub struct OverlayPreferences {
    /// Blur the frozen screen outside the selection, under the dimming. Presentation only: an
    /// exported image never contains it.
    pub mask_blur: bool,
    /// Show the shortcut guide in the bottom-left corner while picking.
    pub shortcut_guide: bool,
    pub keymap: Arc<Keymap>,
}

impl Default for OverlayPreferences {
    fn default() -> Self {
        Self {
            mask_blur: true,
            shortcut_guide: true,
            keymap: Arc::new(Keymap::default()),
        }
    }
}
