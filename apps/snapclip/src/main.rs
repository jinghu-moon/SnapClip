//! Binary entry point. Everything it does lives in the library so the shell can be
//! tested (docs/23 T4.3, the GPUI test layer).
//!
//! The development entry that opens the save panel (`docs/41` S2) is decided in
//! [`snapclip_app::startup`], never here. A binary with control flow of its own is exactly how
//! the entry once replaced the whole launch and took the global hotkeys with it, so this file
//! stays a single call and a test in `startup` keeps it that way.

fn main() {
    snapclip_app::run(snapclip_app::startup::StartupOptions::from_env());
}
