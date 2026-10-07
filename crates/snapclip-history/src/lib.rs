//! Clipboard history and capture artifacts: storage, dedup and the clipboard adapters.
//!
//! # Two stores, not one (docs/23 §6)
//!
//! The two things this crate writes are **not the same kind of thing**, and merging their
//! storage would break both:
//!
//! | Store | Content | Identity |
//! | --- | --- | --- |
//! | [`artifact_store::CaptureArtifactStore`] | a screenshot PNG | where it is, plus a `blake3` fingerprint |
//! | [`blob_store::ClipboardBlobStore`] | clipboard payload bytes | the `blake3` of the bytes *is* its address |
//!
//! A capture artifact has a human-facing name and a place on disk; a clipboard blob is
//! content-addressed, deduplicated and garbage-collected. They share a file system, not a
//! design.
//!
//! # Seam
//!
//! Public API: the two stores, the history/clipboard services (T2.8) and the value types
//! they speak (`snapclip_model::{ArtifactRef, CaptureOutput, ...}`). The database
//! connection, migrations and repositories stay private — callers hold a service, not a
//! connection.

pub mod artifact_store;
pub mod blob_store;
pub mod db;
pub mod error;
pub mod image;
pub mod store;

pub use error::StoreError;
