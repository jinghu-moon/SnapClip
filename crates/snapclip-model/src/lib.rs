//! Stable, framework-free types shared by SnapClip's capability crates.
//!
//! This crate is the single source of truth for a small set of value objects and
//! low-frequency event summaries. Rules (docs/22 §2, §10.1):
//!
//! - **Only** the standard library and `serde`. No Win32, no D3D11, no GPUI, no
//!   SQLite, no Tauri — those belong to the shells and platform crates.
//! - **One definition per type.** While the migration is in flight the old
//!   `src-tauri` modules re-export these types; those forwarders must be gone by
//!   the end of P1 (docs/23 T1.10).
//! - Only types that genuinely cross a capability seam belong here. Painter and
//!   state-machine internals stay inside their own crate.

pub mod artifact;
pub mod error;
pub mod events;
pub mod geometry;
pub mod ids;
pub mod recognition;

pub use geometry::{ImageDimensions, Point, Rect};
