//! Win32 clipboard adapter.
//!
//! Responsibilities are intentionally narrow:
//! * own the `WM_CLIPBOARDUPDATE` listener window and its message loop,
//! * read the clipboard into platform-neutral payload bytes,
//! * resolve the source application from the clipboard owner / foreground window.
//!
//! Deduplication, retry policy, persistence, OCR enqueueing and Tauri event
//! publication live in [`crate::application::clipboard_ingest`]. This module must
//! never touch the store, OCR or Tauri.

pub mod formats;
pub mod image_norm;
pub mod reader;
pub mod source_app;

mod listener;

pub use listener::{ClipboardEvent, ClipboardUpdateListener, mark_clipboard_excluded};
