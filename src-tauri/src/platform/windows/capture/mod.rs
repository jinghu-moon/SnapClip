//! 过渡期转发（docs/23 T1.5）：Windows 捕获适配器已搬到 `snapclip-capture/src/windows`。
//!
//! 只做名字转发，让壳侧仍在写的 `crate::platform::windows::capture::…` 路径可以编译
//! （组合根用的是 `…::monitor::set_per_monitor_v2_awareness` 与
//! `…::overlay::WindowsOverlay::spawn_overlay`）。
//! 删除条件：P1 结束时（T1.10）转发必须为零。

pub use snapclip_capture::windows::*;
