// 过渡期转发（docs/23 T2.5）：ErrorCode/OcrStatus/OcrErrorCode 已搬到 `snapclip-model`，
// 这里只做名字转发。`IpcError` 留在壳里——它是**传输信封**（带 traceId 与一组 From 映射），
// 不是能力 crate 需要的值。删除条件：P2 结束时（T2.10）转发必须为零。

use serde::{Deserialize, Serialize};

pub use snapclip_model::error::ErrorCode;
pub use snapclip_model::recognition::{OcrErrorCode, OcrStatus};


#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: ErrorCode,
    pub message: Option<String>,
    pub trace_id: Option<String>,
}

impl IpcError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: Some(message.into()),
            trace_id: None,
        }
    }

    pub fn internal(code: ErrorCode, message: impl std::fmt::Display) -> Self {
        Self::new(code, message.to_string())
    }
}

impl From<snapclip_capture::CaptureError> for IpcError {
    fn from(error: snapclip_capture::CaptureError) -> Self {
        Self {
            code: error.code(),
            message: Some(error.to_string()),
            trace_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ErrorCode, IpcError};

    // OcrStatus/OcrErrorCode 的往返测试随类型搬到 `snapclip-model::recognition`。
    #[test]
    fn ipc_error_serializes_error_code_as_snake_case() {
        let error = IpcError::new(ErrorCode::Cancelled, "cancelled");
        let value = serde_json::to_value(error).unwrap();
        assert_eq!(value["code"], "cancelled");
        assert_eq!(value["message"], "cancelled");
        assert!(value["traceId"].is_null());
    }
}
