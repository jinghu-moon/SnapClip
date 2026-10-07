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

use snapclip_capture::window_detection::{DetectionOptions, DEFAULT_ADOPT_TEXT_RUNS};

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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            deep_select_text_runs: DEFAULT_ADOPT_TEXT_RUNS,
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_round_trip_through_the_file() {
        let dir = root("round-trip");
        let store = SettingsStore::new(&dir);
        let wanted = Settings {
            deep_select_text_runs: false,
        };
        store.save(&wanted).expect("save");
        assert_eq!(store.load(), wanted);
        // And the bridge to capture sees the same value.
        assert!(!store.load().detection_options().adopt_text_runs);
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
                deep_select_text_runs: false
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
