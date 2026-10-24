//! Saving a finished capture to the user's disk (`docs/41` S1).
//!
//! The save step is deliberately **dumb**: capture produces exactly one lossless PNG master,
//! and saving either copies it (PNG) or hands it to the transcoder for another format. Nothing
//! here knows how to capture, and nothing here decides what the panel shows.
//!
//! The path rules (validation, template expansion, conflict) are separate from the bytes,
//! because a changed name must not force a re-encode (`docs/42` D9).

pub mod assemble;
pub mod bands;
pub mod caps;
pub mod copy;
// Where the path field points before the user says otherwise. It is the shell's answer to
// "where is the desktop", so it lives beside the panel that shows it rather than in the UI crate,
// which must not ask Windows anything a drawing cannot explain.
pub mod dirs;
pub mod service;
pub mod template;

// The panel's window shell (`docs/41` S2). It knows nothing about paths or bytes; it owns the
// window and the lifecycle, and asks the modules above to do the work.
pub mod window;

pub use assemble::{
    DEFAULT_TEMPLATE, PanelValues, Remembered, Sequence, request_for as request_for_panel,
    save_panel,
};
pub use caps::{CapabilityError, check as check_capability};
pub use copy::{ConflictLink, Level, ValidationCopy};
pub use service::{ConflictPolicy, SaveError, SaveOutcome, SaveRequest};
pub use template::{TemplateError, TemplateValues, expand as expand_template};
