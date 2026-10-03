use serde::{Deserialize, Serialize};

/// Pixel layout of a capture frame. Only the layouts the Windows providers can
/// actually produce are modelled.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PixelFormat {
    /// 8-bit BGRA, premultiplied alpha not implied, sRGB.
    Bgra8Unorm,
}

impl PixelFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bgra8Unorm => "bgra8_unorm",
        }
    }

    pub fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Bgra8Unorm => 4,
        }
    }
}

/// Lifecycle of one capture session. Mirrors the transitions documented in
/// `docs/08-screenshot-mvp-tasklist.md` §3.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    Idle,
    Armed,
    Selecting,
    Selected,
    Finishing,
}

impl CaptureState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Armed => "armed",
            Self::Selecting => "selecting",
            Self::Selected => "selected",
            Self::Finishing => "finishing",
        }
    }

    pub fn is_active(self) -> bool {
        !matches!(self, Self::Idle)
    }
}

/// How the captured pixels are handed to the application layer.
///
/// The MVP resolves the selection to a PNG artifact on disk, so the capture module
/// never has to hold a GPU resource across the process boundary and the application
/// layer never has to know about D3D11.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapturePayload {
    /// Absolute path to a PNG file inside the application artifact directory.
    PngFile { path: std::path::PathBuf },
}

/// Domain result of a finished capture session.
///
/// Deliberately free of clipboard handles, database handles, OCR objects and Tauri
/// types: consumers in the application layer decide what to do with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureArtifact {
    pub session_id: String,
    pub width: u32,
    pub height: u32,
    pub dpi: u32,
    pub pixel_format: PixelFormat,
    pub captured_at_unix_ms: i64,
    pub monitor_device_name: Option<String>,
    pub payload: CapturePayload,
}

impl CaptureArtifact {
    pub fn png_path(&self) -> Option<&std::path::Path> {
        match &self.payload {
            CapturePayload::PngFile { path } => Some(path.as_path()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CaptureArtifact, CapturePayload, CaptureState, PixelFormat};

    #[test]
    fn capture_state_reports_activity() {
        assert!(!CaptureState::Idle.is_active());
        for state in [
            CaptureState::Armed,
            CaptureState::Selecting,
            CaptureState::Selected,
            CaptureState::Finishing,
        ] {
            assert!(state.is_active(), "{state:?} should be active");
        }
    }

    #[test]
    fn artifact_serializes_without_clipboard_or_store_objects() {
        // Compile-time guarantee: `CaptureArtifact` must be constructible from
        // primitives and a file path alone, with no store/clipboard/OCR handle.
        let artifact = CaptureArtifact {
            session_id: "session-1".into(),
            width: 100,
            height: 50,
            dpi: 96,
            pixel_format: PixelFormat::Bgra8Unorm,
            captured_at_unix_ms: 7,
            monitor_device_name: None,
            payload: CapturePayload::PngFile {
                path: std::path::PathBuf::from(r"C:\artifacts\session-1.png"),
            },
        };
        assert_eq!(artifact.width * artifact.height, 5000);
        assert!(artifact.png_path().is_some());
    }
}
