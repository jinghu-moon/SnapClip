//! The settings model and its file on disk.
//!
//! Design rules this follows (Design Guides, "Forms and settings"): one named file in the
//! app's data directory, defaults that match the product's current behaviour, and a load
//! path that never fails the app because a file is missing or damaged.
//!
//! Only settings with a *real* consumer are modelled. There is deliberately no
//! `deep_select_visible_wrappers`: the plan named it, but no behaviour in the walker
//! corresponds to it yet, and a switch that changes nothing is worse than no switch.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use snapclip_capture::keymap::{Keymap, KeymapOverrides};
use snapclip_capture::preferences::OverlayPreferences;
use snapclip_capture::units::{LabelDisplay, LengthUnit};
use snapclip_capture::window_detection::{DEFAULT_ADOPT_TEXT_RUNS, DetectionOptions};

/// The user's preferences.
///
/// `#[serde(default)]` on the struct is what makes an older file (missing a field) and a
/// newer one (carrying a field this build does not know) both load: missing fields take
/// today's default, unknown fields are ignored.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Whether a bare text run may be published as the snap target (docs/21 §5.19).
    pub deep_select_text_runs: bool,
    /// Blur the screen outside the selection while picking.
    pub mask_blur: bool,
    /// Show the shortcut guide in the bottom-left corner while picking.
    pub shortcut_guide: bool,
    /// The unit the size/coordinate label is measured in (`px` or `dp`).
    pub length_unit: LengthUnit,
    /// Whether that label shows the selection's size.
    pub show_size: bool,
    /// Whether that label also shows the selection's origin.
    pub show_coordinates: bool,
    /// Whether that label prints the unit it is in (`1024×768 px`).
    pub show_unit: bool,
    /// The overlay's key bindings, as changes from the defaults.
    pub keymap: KeymapOverrides,
}

impl Default for Settings {
    fn default() -> Self {
        let overlay = OverlayPreferences::default();
        Self {
            deep_select_text_runs: DEFAULT_ADOPT_TEXT_RUNS,
            mask_blur: overlay.mask_blur,
            shortcut_guide: overlay.shortcut_guide,
            length_unit: overlay.label.length_unit,
            show_size: overlay.label.show_size,
            show_coordinates: overlay.label.show_coordinates,
            show_unit: overlay.label.show_unit,
            keymap: KeymapOverrides::default(),
        }
    }
}

impl Settings {
    /// The detection options these settings imply (docs/23 T4.4.1).
    pub fn detection_options(&self) -> DetectionOptions {
        DetectionOptions {
            adopt_text_runs: self.deep_select_text_runs,
        }
    }

    /// The overlay preferences these settings imply.
    ///
    /// Bindings that cannot be applied (a hand-edited file naming an unknown action, two actions
    /// on one chord) are reported and replaced by the default keymap as a whole, for the same
    /// reason a damaged file loads as defaults: a half-applied keymap is a set of keys nobody
    /// chose, and refusing to capture over it is worse.
    pub fn overlay_preferences(&self) -> OverlayPreferences {
        let keymap = Keymap::new(&self.keymap).unwrap_or_else(|error| {
            eprintln!(
                "[snapclip-app] key bindings in the settings are invalid ({error}); using defaults"
            );
            Keymap::default()
        });
        OverlayPreferences {
            mask_blur: self.mask_blur,
            shortcut_guide: self.shortcut_guide,
            label: LabelDisplay {
                length_unit: self.length_unit,
                show_size: self.show_size,
                show_coordinates: self.show_coordinates,
                show_unit: self.show_unit,
            },
            keymap: Arc::new(keymap),
        }
    }

    /// Adopt the display settings a capture ended with (docs/23 §14.53/§14.54); `true` when they differ.
    ///
    /// The badge's menu and the custom-region panel edit the same four values this file stores. The
    /// merge is here, on the value, so the rule can be tested without a window; the view is what
    /// writes the result out.
    pub fn adopt_label(&mut self, label: LabelDisplay) -> bool {
        if self.length_unit == label.length_unit
            && self.show_size == label.show_size
            && self.show_coordinates == label.show_coordinates
            && self.show_unit == label.show_unit
        {
            return false;
        }
        self.length_unit = label.length_unit;
        self.show_size = label.show_size;
        self.show_coordinates = label.show_coordinates;
        self.show_unit = label.show_unit;
        true
    }
}

/// Reads and writes `<app data>/settings.json`.
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new(app_data: impl AsRef<Path>) -> Self {
        Self {
            path: app_data.as_ref().join("settings.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load the settings, falling back to defaults.
    ///
    /// A missing file is the normal first run. A damaged file is reported and then treated
    /// as first run: refusing to start because a preference file has a stray byte is not a
    /// trade this product should make.
    pub fn load(&self) -> Settings {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
                eprintln!(
                    "[snapclip-app] settings at {} are unreadable ({error}); using defaults",
                    self.path.display()
                );
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    /// Write the settings atomically, so a crash cannot leave a half-written file behind.
    pub fn save(&self, settings: &Settings) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("create settings directory failed: {error}"))?;
        }
        let text = serde_json::to_string_pretty(settings)
            .map_err(|error| format!("serialise settings failed: {error}"))?;
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, text)
            .map_err(|error| format!("write settings failed: {error}"))?;
        std::fs::rename(&temporary, &self.path).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            format!("replace settings failed: {error}")
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use snapclip_capture::keymap::{Keymap, KeymapOverrides};
    use snapclip_capture::preferences::OverlayPreferences;
    use snapclip_capture::units::{LabelDisplay, LengthUnit};

    use super::{Settings, SettingsStore};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "snapclip-app-settings-{name}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn a_missing_file_is_first_run_and_keeps_todays_behaviour() {
        let dir = root("missing");
        let store = SettingsStore::new(&dir);
        let settings = store.load();
        assert_eq!(settings, Settings::default());
        // The default is the product's current behaviour, not a new choice.
        assert_eq!(
            settings.detection_options(),
            snapclip_capture::window_detection::DetectionOptions::default()
        );
        assert_eq!(
            settings.overlay_preferences(),
            OverlayPreferences::default()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_round_trip_through_the_file() {
        let dir = root("round-trip");
        let store = SettingsStore::new(&dir);
        let mut keymap = KeymapOverrides::default();
        keymap.set_visible("level", false);
        let wanted = Settings {
            deep_select_text_runs: false,
            mask_blur: false,
            shortcut_guide: false,
            length_unit: LengthUnit::Dp,
            show_size: true,
            show_coordinates: false,
            show_unit: false,
            keymap,
        };
        store.save(&wanted).expect("save");
        assert_eq!(store.load(), wanted);
        // The file spells the readout's unit the way the page does, so a hand-edited file needs no
        // translation table, and a field the user never touched keeps its default (above).
        let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(text.contains("\"lengthUnit\": \"dp\""), "{text}");
        assert!(text.contains("\"showCoordinates\": false"), "{text}");
        assert!(text.contains("\"showUnit\": false"), "{text}");
        // And the bridges to capture see the same values.
        assert!(!store.load().detection_options().adopt_text_runs);
        let overlay = store.load().overlay_preferences();
        assert!(!overlay.mask_blur && !overlay.shortcut_guide);
        assert_eq!(overlay.label.length_unit, LengthUnit::Dp);
        assert!(!overlay.label.show_coordinates);
        assert!(!overlay.label.show_unit);
        assert_ne!(*overlay.keymap, Keymap::default());
        // No temporary file is left behind.
        let leftovers = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_file_falls_back_to_defaults_instead_of_failing() {
        let dir = root("damaged");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), "{ not json").unwrap();
        let store = SettingsStore::new(&dir);
        assert_eq!(store.load(), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_from_another_version_still_loads() {
        let dir = root("forward-compatible");
        std::fs::create_dir_all(&dir).unwrap();
        // A field this build does not know, and the field it does.
        std::fs::write(
            dir.join("settings.json"),
            r#"{"deepSelectTextRuns": false, "somethingNew": 7}"#,
        )
        .unwrap();
        let store = SettingsStore::new(&dir);
        assert_eq!(
            store.load(),
            Settings {
                deep_select_text_runs: false,
                ..Settings::default()
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_key_bindings_fall_back_to_the_default_keymap_and_keep_the_rest() {
        let dir = root("bad-keymap");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"maskBlur": false, "keymap": {"chords": {"no-such-action": ["X"]}}}"#,
        )
        .unwrap();
        let overlay = SettingsStore::new(&dir).load().overlay_preferences();
        assert!(!overlay.mask_blur);
        assert_eq!(*overlay.keymap, Keymap::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn untouched_bindings_are_not_written_out() {
        let text = serde_json::to_string(&Settings::default()).unwrap();
        let back: Settings = serde_json::from_str(&text).unwrap();
        assert_eq!(back.keymap, KeymapOverrides::default());
        assert!(back.keymap.is_empty());
    }

    #[test]
    fn adopting_the_overlay_label_keeps_the_rest_of_the_settings() {
        let mut settings = Settings {
            deep_select_text_runs: false,
            mask_blur: false,
            shortcut_guide: false,
            ..Settings::default()
        };
        let from_the_menu = LabelDisplay {
            length_unit: LengthUnit::Dp,
            show_size: false,
            show_coordinates: false,
            show_unit: false,
        };
        assert!(settings.adopt_label(from_the_menu));
        assert_eq!(settings.length_unit, LengthUnit::Dp);
        assert!(!settings.show_size && !settings.show_coordinates && !settings.show_unit);
        // Everything the badge's menu says nothing about stays exactly as it was.
        assert!(!settings.deep_select_text_runs && !settings.mask_blur && !settings.shortcut_guide);
        // The same values again are not a change, so nothing is written and nothing is announced.
        assert!(!settings.adopt_label(from_the_menu));
    }
}
