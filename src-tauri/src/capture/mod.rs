//! 过渡期转发（docs/23 T1.2/T1.4/T1.5）：唯一实现已搬到 `snapclip-capture`。
//!
//! 这个模块现在**只做名字转发**，让壳侧仍在写的 `crate::capture::…` 路径可以编译。
//! 删除条件：P1 结束时（T1.10）转发必须为零——那时调用方直接 `use snapclip_capture::…`，
//! 本文件与这个目录一起删除。

pub use snapclip_capture::*;

/// 过渡期转发：新 crate 用 `ports`（端口）与 `runtime`（`CaptureRuntime`）取代了旧的
/// `capture::application` 子模块，这里把旧路径映射过去。
pub mod application {
    pub use snapclip_capture::ports::*;

    pub mod runtime {
        pub use snapclip_capture::runtime::*;
    }
}
