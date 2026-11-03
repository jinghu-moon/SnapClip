//! System-owned handles are copied while the clipboard is locked; parsing happens afterwards.
pub mod formats;
mod listener;
pub mod reader;
pub mod source_app;
pub use listener::{ClipboardEvent, ClipboardUpdateListener, mark_clipboard_excluded};
