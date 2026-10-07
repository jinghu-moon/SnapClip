//! The storage error every store in this crate speaks.
//!
//! Moved here from `src-tauri/src/infrastructure/store/mod.rs` (docs/23 T2.3) so the
//! stores own their failure modes instead of borrowing the shell's. The shell keeps the
//! `From<StoreError> for IpcError` mapping — that is transport glue and stays with the IPC
//! boundary, which is exactly where a code like `InvalidCursor` becomes `invalid_argument`.

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
