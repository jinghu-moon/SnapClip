//! Settings: the model, its file, and the bridge to capture's detection options.
//!
//! This is the first slice of docs/23 T4.4: the model and its persistence, with the
//! bridge that turns a setting into the capture crate's `DetectionOptions` (T4.4.1).
//! The settings *page* and the hot-update path come next — see the note at the bottom of
//! this module about why "hot-update the overlay" cannot be end-to-end yet.

pub mod model;
pub mod settings_view;

pub use model::{Settings, SettingsStore};
pub use settings_view::SettingsView;
