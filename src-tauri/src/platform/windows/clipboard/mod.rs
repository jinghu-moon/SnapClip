//! 过渡期转发（docs/23 T2.6）：Win32 剪贴板适配器已搬到 `snapclip-history::windows`。
//!
//! 只做名字转发，让壳侧仍在写的 `crate::platform::windows::clipboard::…` 路径可以编译
//! （组合根要 `ClipboardUpdateListener`/`ClipboardEvent`/`read_clipboard` 与
//! `mark_clipboard_excluded`）。删除条件：P2 结束时（T2.10）。

pub use snapclip_history::windows::*;
