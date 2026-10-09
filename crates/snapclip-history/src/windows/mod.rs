//! Win32 clipboard adapter.
//!
//! Responsibilities are intentionally narrow:
//! * own the `WM_CLIPBOARDUPDATE` listener window and its message loop,
//! * read the clipboard into platform-neutral payload bytes,
//! * resolve the source application from the clipboard owner / foreground window.
//!
//! Deduplication, retry policy, persistence, OCR enqueueing and UI notification live
//! in `snapclip-history`'s service layer ([`crate::ingest`], whose [`crate::ingest::ClipboardStore`],
//! [`crate::ingest::OcrQueue`] and [`crate::ingest::ClipboardEventSink`] traits the host
//! implements). This module must never touch the store, the OCR queue or the shell's
//! event bus.

pub mod formats;
pub mod image_norm;
pub mod reader;
pub mod source_app;

mod listener;

pub use listener::{ClipboardEvent, ClipboardUpdateListener, mark_clipboard_excluded};
