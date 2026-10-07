//! 过渡期转发（docs/23 T1.3）：定义已搬到 `snapclip-model`，这里是唯一实现的转出口。
//! 删除条件：P1 结束时（T1.10）转发必须为零——那时调用方直接从 `snapclip_model`
//! 引用这些值，本文件删除。
//!
//! 线上契约（`snake_case` 状态名、`PixelFormat::Bgra8Unorm` 的序列化形式）由
//! `snapclip_model::capture` 上的 derive 保证，未改动。
pub use snapclip_model::capture::{CaptureArtifact, CapturePayload, CaptureState, PixelFormat};
