//! Capture, hosted by the GPUI shell (docs/23 P6).
//!
//! While the Tauri host is still alive, capture runs there and this module is the shell's
//! half of the move: the ports capture declares, implemented by the composition root that
//! will own them after the old shell is deleted. Moving a piece at a time keeps the tree
//! green; the duplicate is temporary and both copies are the same code.

pub mod artifact_writer;

pub use artifact_writer::HistoryArtifactWriter;
