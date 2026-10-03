//! Platform capture adapters.
//!
//! Only the Windows adapter exists today; the module boundary is what keeps
//! `capture::application` and `capture::session` free of Win32 types.

#[cfg(windows)]
pub mod windows;