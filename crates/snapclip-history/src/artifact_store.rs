//! The one owner of screenshot artifacts on disk (docs/23 T2.3).
//!
//! Takes a [`snapclip_model::CaptureOutput`] (bytes + metadata), writes it atomically
//! under a naming scheme it owns, computes the `blake3` fingerprint while writing, and
//! hands back a [`snapclip_model::ArtifactRef`].
//!
//! Today this logic is split between `src-tauri`'s `CaptureService::write_artifact` and
//! `infrastructure::image`'s encoder; T2.3/T2.4 move it here in one step so there is
//! exactly one writer.
