// 过渡期转发（docs/23 T2.5）：定义已搬到 `snapclip-model`，这里是唯一实现的转出口。
// 删除条件：P2 结束时（T2.10）转发必须为零。
pub use snapclip_model::publication::{Publication, PublicationOrigin};