//! Saving a finished capture to the user's disk (`docs/41` S1).
//!
//! The save step is deliberately **dumb**: capture produces exactly one lossless PNG master,
//! and saving either copies it (PNG) or hands it to the transcoder for another format. Nothing
//! here knows how to capture, and nothing here decides what the panel shows.
//!
//! The path rules (validation, template expansion, conflict) are separate from the bytes,
//! because a changed name must not force a re-encode (`docs/42` D9).

pub mod service;

pub use service::{ConflictPolicy, SaveError, SaveOutcome, SaveRequest};
