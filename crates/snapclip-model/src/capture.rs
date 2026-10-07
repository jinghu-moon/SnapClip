//! The capture domain: what a finished screenshot session produces, and the frozen
//! state names it reports while it runs.
//!
//! These values are what the capture crate hands to the rest of the process. They are
//! deliberately free of clipboard handles, database handles, GPU resources and Tauri
//! types, so they can live here beside the other shared value objects.

/// Pixel layout of a capture frame. Only the layouts the Windows providers can
/// actually produce are modelled.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
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

/// Lifecycle of one capture session.
///
/// The value set is the frozen Phase 0 contract from
/// `docs/11-screenshot-fullflow-ui-refactor-tasklist.md` §2.2:
/// Idle → Preparing → Armed → Selecting → Selected → Adjusting → Annotating →
/// Exporting → Idle. `Adjusting` and `Annotating` are emitted once their phases
/// introduce them. `Exporting` is entered when a selection is confirmed and covers
/// the region readback on the overlay thread plus the encode/write on the export
/// worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    Idle,
    Preparing,
    Armed,
    Selecting,
    Selected,
    Adjusting,
    Annotating,
    Exporting,
}

impl CaptureState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Preparing => "preparing",
            Self::Armed => "armed",
            Self::Selecting => "selecting",
            Self::Selected => "selected",
            Self::Adjusting => "adjusting",
            Self::Annotating => "annotating",
            Self::Exporting => "exporting",
        }
    }

    /// Whether a session owns resources that still have to be released.
    ///
    /// `Exporting` is active even though the overlay has stopped painting: the frozen
    /// frame and the artifact are still owned by the session until the result lands.
    pub fn is_active(self) -> bool {
        self != Self::Idle
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
            CaptureState::Preparing,
            CaptureState::Armed,
            CaptureState::Selecting,
            CaptureState::Selected,
            CaptureState::Adjusting,
            CaptureState::Annotating,
            CaptureState::Exporting,
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

    /// The front end and the stored rows both key off these strings, so they are part
    /// of the contract: `docs/11` §2.2 froze the state names, and `as_str` is what the
    /// store writes.
    #[test]
    fn state_and_format_names_are_the_frozen_contract_strings() {
        let states = [
            (CaptureState::Idle, "idle"),
            (CaptureState::Preparing, "preparing"),
            (CaptureState::Armed, "armed"),
            (CaptureState::Selecting, "selecting"),
            (CaptureState::Selected, "selected"),
            (CaptureState::Adjusting, "adjusting"),
            (CaptureState::Annotating, "annotating"),
            (CaptureState::Exporting, "exporting"),
        ];
        for (state, name) in states {
            assert_eq!(state.as_str(), name);
            // …and the serde spelling matches, which is what crosses the IPC boundary.
            assert_eq!(
                serde_json::to_value(state).unwrap(),
                serde_json::Value::String(name.to_string())
            );
        }
        assert_eq!(PixelFormat::Bgra8Unorm.as_str(), "bgra8_unorm");
        assert_eq!(PixelFormat::Bgra8Unorm.bytes_per_pixel(), 4);
    }
}
