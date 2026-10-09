//! SQLite connection, migrations and repositories (docs/23 T2.5).
//!
//! The store used to be one module; it now lives in [`crate::store`], split into a
//! connection, the migration runner and one repository per table family
//! (`clip_repository`, `artifact_repository`, `recognition_repository`).
