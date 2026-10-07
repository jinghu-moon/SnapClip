//! 过渡期转发（docs/23 T2.7）：剪贴板 ingest 流水线已搬到 `snapclip-history::ingest`。
//!
//! 只做名字转发，让壳侧仍在写的 `crate::application::clipboard_ingest::…` 路径可以编译
//! （组合根要那些端口：`ClipboardSource`/`ClipboardEventBridge`/`OcrQueue`/`StopSignal`…）。
//! 删除条件：P2 结束时（T2.10）。

pub use snapclip_history::ingest::*;
