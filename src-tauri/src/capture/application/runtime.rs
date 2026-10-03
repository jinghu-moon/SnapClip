//! Cross-thread entry point for the capture feature.

use super::OverlayPlatform;
use crate::capture::{CaptureResult, CaptureState};

#[cfg(not(windows))]
use crate::capture::CaptureError;

/// Owns the platform overlay controller and exposes it as the process-wide capture
/// handle. All methods are safe to call from any thread.
pub struct CaptureRuntime {
    platform: Box<dyn OverlayPlatform>,
}

impl CaptureRuntime {
    pub fn new(platform: Box<dyn OverlayPlatform>) -> Self {
        Self { platform }
    }

    /// Wrap an already-constructed platform controller.
    ///
    /// Used by the composition root, which needs to hand the controller its
    /// dependencies (artifact service, event sink) at construction time.
    pub fn from_platform(platform: Box<dyn OverlayPlatform>) -> Self {
        Self::new(platform)
    }

    #[cfg(not(windows))]
    pub fn start() -> CaptureResult<Self> {
        Ok(Self::new(Box::new(UnsupportedOverlay)))
    }

    pub fn state(&self) -> CaptureState {
        self.platform.state()
    }

    /// `F5`: start a session, or report that one is already running.
    pub fn start_capture(&self) -> CaptureResult<bool> {
        self.platform.request_start()
    }

    /// `Esc`: cancel whatever the session is doing.
    pub fn cancel_capture(&self) -> CaptureResult<()> {
        self.platform.request_cancel()
    }

    /// `Enter`: confirm the current selection.
    pub fn confirm_capture(&self) -> CaptureResult<()> {
        self.platform.request_confirm()
    }
}

impl Drop for CaptureRuntime {
    fn drop(&mut self) {
        self.platform.shutdown();
    }
}

/// Placeholder used on non-Windows targets so the module graph stays complete.
#[cfg(not(windows))]
struct UnsupportedOverlay;

#[cfg(not(windows))]
impl OverlayPlatform for UnsupportedOverlay {
    fn state(&self) -> CaptureState {
        CaptureState::Idle
    }

    fn request_start(&self) -> CaptureResult<bool> {
        Err(CaptureError::Unsupported)
    }

    fn request_cancel(&self) -> CaptureResult<()> {
        Err(CaptureError::Unsupported)
    }

    fn request_confirm(&self) -> CaptureResult<()> {
        Err(CaptureError::Unsupported)
    }

    fn shutdown(&self) {}
}
