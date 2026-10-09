//! Error codes that cross a capability seam.
//!
//! Only the codes themselves live here. Nothing wraps them into an envelope on the way
//! out: an error type that has to be understood by the UI exposes its own
//! `code()` (see `snapclip_capture::CaptureError::code`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidArgument,
    NotFound,
    Conflict,
    Unsupported,
    Cancelled,
    Storage,
    Internal,
}
