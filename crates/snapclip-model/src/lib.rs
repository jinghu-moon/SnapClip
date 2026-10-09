//! Stable, framework-free types shared by SnapClip's capability crates.
//!
//! This crate is the single source of truth for a small set of value objects and
//! low-frequency event summaries. Rules (docs/22 §2, §10.1):
//!
//! - **Only** the standard library and `serde`. No Win32, no D3D11, no GPUI, no
//!   SQLite, no Tauri — those belong to the shells and platform crates.
//! - **One definition per type.** A capability crate that needs one of these imports it
//!   from here; nothing re-exports it under a second path, and nothing below the shells
//!   depends on a shell (`tools/check-dependency-direction.ps1`, docs/31 §4.2).
//! - Only types that genuinely cross a capability seam belong here. Painter and
//!   state-machine internals stay inside their own crate.

pub mod artifact;
pub mod capture;
pub mod error;
pub mod events;
pub mod geometry;
pub mod history;
pub mod ids;
pub mod payload;
pub mod publication;
pub mod recognition;
pub mod time;

pub use geometry::{ImageDimensions, Point, Rect};
pub use capture::{CaptureArtifact, CapturePayload, CaptureState, PixelFormat};
pub use artifact::{ArtifactRef, CaptureMetadata, CaptureOutput};
pub use error::ErrorCode;
pub use history::{ClipSummary, HistoryPage};
pub use payload::{PayloadData, PayloadKind, PayloadRef};
pub use publication::{Publication, PublicationOrigin};
pub use recognition::{OcrErrorCode, OcrStatus};
pub use events::{AppEvent, CaptureEvent, ClipboardEvent, RecognitionEvent};
pub use time::unix_time_ms;
