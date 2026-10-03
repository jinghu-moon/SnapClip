//! Tauri command adapters.
//!
//! Commands translate IPC payloads into application/domain calls and back. They
//! contain no SQL, no Win32 and no orchestration of their own.
//!
//! Each submodule keeps its commands public so `generate_handler!` can reference
//! them as `commands::<module>::<name>`; re-exporting them here would also re-export
//! the generated `__cmd__*` helpers and collide with the ones Tauri generates for
//! this module's own commands.

pub mod capture;
pub mod clipboard;
pub mod history;
pub mod ocr;

// `icons` intentionally has no `#[tauri::command]`: the icon command lives with the
// rest of this crate's icon module and is wrapped in `commands::clipboard`.
