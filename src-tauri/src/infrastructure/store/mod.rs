//! 过渡期转发（docs/23 T2.5）：`Store` 已搬到 `snapclip-history::store`。
//!
//! 这个模块现在只做两件事：转发名字（让壳侧仍在写的
//! `crate::infrastructure::store::…` 路径可以编译），以及保留
//! `From<StoreError> for IpcError` —— 那是**传输胶水**（把存储失败翻成 IPC 错误码），
//! 能力 crate 里没有 IPC，所以它留在这儿。
//!
//! 删除条件：P2 结束时（T2.10）转发必须为零。

pub use snapclip_history::blob_store::{BlobRef, ClipboardBlobStore as BlobStore};
pub use snapclip_history::store::*;
pub use snapclip_history::StoreError;

use crate::domain::{ErrorCode, IpcError};

impl From<StoreError> for IpcError {
    fn from(error: StoreError) -> Self {
        let code = match error {
            StoreError::InvalidCursor | StoreError::InvalidPublication(_) => {
                ErrorCode::InvalidArgument
            }
            _ => ErrorCode::Storage,
        };
        Self {
            code,
            message: Some(error.to_string()),
            trace_id: None,
        }
    }
}
