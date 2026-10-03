//! Domain contracts shared by platform adapters, application services and storage.
//!
//! The domain deliberately contains no clipboard, Win32, Tauri or database types.
//! `capture` and `clipboard` both depend on this module and on nothing else from
//! each other.

pub mod capture;
pub mod error;
pub mod history;
pub mod payload;
pub mod publication;

pub use capture::{CaptureArtifact, CapturePayload, CaptureState, PixelFormat};
pub use error::{ErrorCode, IpcError, OcrErrorCode, OcrStatus};
pub use history::{ClipSummary, HistoryPage};
pub use payload::{ImageDimensions, PayloadData, PayloadKind, PayloadRef};
pub use publication::{Publication, PublicationOrigin};

/// Version of the whole front-end/back-end message contract.
pub const IPC_SCHEMA_VERSION: u16 = 3;
