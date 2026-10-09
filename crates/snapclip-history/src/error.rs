//! The storage error every store in this crate speaks.
//!
//! Extracted (docs/23 T2.3) so the stores own their failure modes instead of borrowing the
//! shell's. The shell consumes the variants as they are: there is no IPC envelope left to
//! translate them into, so a `InvalidCursor` reaches the UI as itself.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("storage I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("database operation failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("invalid history cursor")]
    InvalidCursor,
    #[error("invalid publication: {0}")]
    InvalidPublication(String),
    #[error("writer thread is unavailable")]
    WriterUnavailable,
    #[error("store initialization failed: {0}")]
    Initialization(String),
    #[error("store operation failed: {0}")]
    Internal(String),
}
