use thiserror::Error;

use crate::domain::ErrorCode;

/// Structured capture error codes. These travel over the versioned Tauri event
/// contract, so the string form is part of the public surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureErrorCode {
    Unsupported,
    HotkeyUnavailable,
    HotkeyConflict,
    MonitorUnavailable,
    ProviderUnavailable,
    CaptureFailed,
    DeviceRemoved,
    WindowFailed,
    RenderFailed,
    EncodeFailed,
    InvalidState,
    Cancelled,
    Internal,
}

impl CaptureErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::HotkeyUnavailable => "hotkey_unavailable",
            Self::HotkeyConflict => "hotkey_conflict",
            Self::MonitorUnavailable => "monitor_unavailable",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::CaptureFailed => "capture_failed",
            Self::DeviceRemoved => "device_removed",
            Self::WindowFailed => "window_failed",
            Self::RenderFailed => "render_failed",
            Self::EncodeFailed => "encode_failed",
            Self::InvalidState => "invalid_state",
            Self::Cancelled => "cancelled",
            Self::Internal => "internal",
        }
    }

    pub fn to_ipc(self) -> ErrorCode {
        match self {
            Self::InvalidState | Self::MonitorUnavailable => ErrorCode::InvalidArgument,
            Self::Unsupported | Self::ProviderUnavailable => ErrorCode::Unsupported,
            Self::HotkeyConflict => ErrorCode::Conflict,
            Self::Cancelled => ErrorCode::Cancelled,
            _ => ErrorCode::Internal,
        }
    }
}

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("capture is not supported on this platform")]
    Unsupported,
    #[error("global hotkey registration failed: {0}")]
    HotkeyUnavailable(String),
    #[error("global hotkey is already taken by another application: {0}")]
    HotkeyConflict(String),
    #[error("no monitor is available at the cursor position: {0}")]
    MonitorUnavailable(String),
    #[error("no capture provider succeeded: {0}")]
    ProviderUnavailable(String),
    #[error("capture failed: {0}")]
    CaptureFailed(String),
    #[error("graphics device was removed: {0}")]
    DeviceRemoved(String),
    #[error("overlay window failed: {0}")]
    WindowFailed(String),
    #[error("overlay rendering failed: {0}")]
    RenderFailed(String),
    #[error("capture artifact encoding failed: {0}")]
    EncodeFailed(String),
    #[error("capture session is in an invalid state: {0}")]
    InvalidState(String),
    #[error("capture session was cancelled")]
    Cancelled,
    #[error("capture failed: {0}")]
    Internal(String),
}

impl CaptureError {
    pub fn code(&self) -> ErrorCode {
        self.error_code().to_ipc()
    }

    pub fn error_code(&self) -> CaptureErrorCode {
        match self {
            Self::Unsupported => CaptureErrorCode::Unsupported,
            Self::HotkeyUnavailable(_) => CaptureErrorCode::HotkeyUnavailable,
            Self::HotkeyConflict(_) => CaptureErrorCode::HotkeyConflict,
            Self::MonitorUnavailable(_) => CaptureErrorCode::MonitorUnavailable,
            Self::ProviderUnavailable(_) => CaptureErrorCode::ProviderUnavailable,
            Self::CaptureFailed(_) => CaptureErrorCode::CaptureFailed,
            Self::DeviceRemoved(_) => CaptureErrorCode::DeviceRemoved,
            Self::WindowFailed(_) => CaptureErrorCode::WindowFailed,
            Self::RenderFailed(_) => CaptureErrorCode::RenderFailed,
            Self::EncodeFailed(_) => CaptureErrorCode::EncodeFailed,
            Self::InvalidState(_) => CaptureErrorCode::InvalidState,
            Self::Cancelled => CaptureErrorCode::Cancelled,
            Self::Internal(_) => CaptureErrorCode::Internal,
        }
    }

    /// HRESULT/Win32 code when the failure came from a native API.
    pub fn native_code(&self) -> Option<i32> {
        let message = match self {
            Self::HotkeyUnavailable(message)
            | Self::HotkeyConflict(message)
            | Self::MonitorUnavailable(message)
            | Self::ProviderUnavailable(message)
            | Self::CaptureFailed(message)
            | Self::DeviceRemoved(message)
            | Self::WindowFailed(message)
            | Self::RenderFailed(message)
            | Self::EncodeFailed(message)
            | Self::InvalidState(message)
            | Self::Internal(message) => message,
            Self::Unsupported | Self::Cancelled => return None,
        };
        message
            .rsplit('#')
            .next()
            .and_then(|tail| tail.strip_prefix("code="))
            .and_then(|value| value.parse::<i32>().ok())
    }
}

pub type CaptureResult<T> = Result<T, CaptureError>;

#[cfg(test)]
mod tests {
    use super::{CaptureError, CaptureErrorCode};
    use crate::domain::ErrorCode;

    #[test]
    fn error_codes_map_to_ipc_codes() {
        assert_eq!(
            CaptureError::HotkeyConflict("taken".into()).code(),
            ErrorCode::Conflict
        );
        assert_eq!(
            CaptureError::Unsupported.code(),
            ErrorCode::Unsupported
        );
        assert_eq!(CaptureError::Cancelled.code(), ErrorCode::Cancelled);
        assert_eq!(
            CaptureError::CaptureFailed("boom".into()).code(),
            ErrorCode::Internal
        );
    }

    #[test]
    fn native_code_is_parsed_from_tagged_message() {
        let error = CaptureError::DeviceRemoved("swap chain lost #code=-2005270523".into());
        assert_eq!(error.native_code(), Some(-2005270523));
        assert_eq!(error.error_code(), CaptureErrorCode::DeviceRemoved);
        assert_eq!(CaptureError::Cancelled.native_code(), None);
    }
}
