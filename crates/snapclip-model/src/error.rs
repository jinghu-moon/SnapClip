//! Error codes that cross a capability seam.
//!
//! Only the codes themselves live here. The IPC envelope (`IpcError`, with its
//! `traceId` and its `From<CaptureError>` impl) stays in the shell — it is a transport
//! type, not a value the capability crates need.

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
