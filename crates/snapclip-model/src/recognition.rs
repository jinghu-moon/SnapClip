//! Recognition status and result value types.
//!
//! Moved here in P2 (docs/23 T2.5): the store writes these strings and the front end
//! reads them, so they sit with the other wire values. The engine-side result types
//! (`RecognitionResult`) arrive with T3.0.

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
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

#[cfg(test)]
mod tests {
    use super::{OcrErrorCode, OcrStatus};

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
}
