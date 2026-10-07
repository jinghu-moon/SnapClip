use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OcrStatus {
    None,
    Queued,
    Running,
    Done,
    Failed,
    Skipped,
}

impl OcrStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "done" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            "skipped" => Some(Self::Skipped),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OcrErrorCode {
    LanguageUnavailable,
    DecodeFailed,
    Timeout,
    Cancelled,
    EngineFailed,
}

impl OcrErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LanguageUnavailable => "language_unavailable",
            Self::DecodeFailed => "decode_failed",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::EngineFailed => "engine_failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "language_unavailable" => Some(Self::LanguageUnavailable),
            "decode_failed" => Some(Self::DecodeFailed),
            "timeout" => Some(Self::Timeout),
            "cancelled" => Some(Self::Cancelled),
            "engine_failed" => Some(Self::EngineFailed),
            _ => None,
        }
    }
}

// 过渡期转发（docs/23 T1.3）：定义已搬到 `snapclip-model`，这里是唯一实现的转出口。
// 删除条件：P1 结束时（T1.10）转发必须为零。
pub use snapclip_model::error::ErrorCode;

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

impl From<crate::capture::CaptureError> for IpcError {
    fn from(error: crate::capture::CaptureError) -> Self {
        Self {
            code: error.code(),
            message: Some(error.to_string()),
            trace_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ErrorCode, IpcError, OcrErrorCode, OcrStatus};

    #[test]
    fn ocr_status_round_trips_through_storage_strings() {
        for status in [
            OcrStatus::None,
            OcrStatus::Queued,
            OcrStatus::Running,
            OcrStatus::Done,
            OcrStatus::Failed,
            OcrStatus::Skipped,
        ] {
            assert_eq!(OcrStatus::parse(status.as_str()), Some(status));
        }
    }

    #[test]
    fn ocr_error_code_round_trips_through_storage_strings() {
        for code in [
            OcrErrorCode::LanguageUnavailable,
            OcrErrorCode::DecodeFailed,
            OcrErrorCode::Timeout,
            OcrErrorCode::Cancelled,
            OcrErrorCode::EngineFailed,
        ] {
            assert_eq!(OcrErrorCode::parse(code.as_str()), Some(code));
        }
    }

    #[test]
    fn ipc_error_serializes_error_code_as_snake_case() {
        let error = IpcError::new(ErrorCode::Cancelled, "cancelled");
        let value = serde_json::to_value(error).unwrap();
        assert_eq!(value["code"], "cancelled");
        assert_eq!(value["message"], "cancelled");
        assert!(value["traceId"].is_null());
    }
}
