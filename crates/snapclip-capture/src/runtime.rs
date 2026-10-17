//! Cross-thread entry point for the capture feature.

use crate::annotation::AnnotationCommand;
use crate::ports::OverlayPlatform;
use crate::{CaptureResult, CaptureState};

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

    /// The user's overlay preferences, applied from the next session on.
    pub fn set_preferences(&self, preferences: crate::preferences::OverlayPreferences) {
        self.platform.set_preferences(preferences);
    }

    /// The preferences the overlay ended its last session with (docs/23 §14.53).
    ///
    /// The shell calls this when a capture ends: the overlay's custom-region panel is the second
    /// place the display settings can be changed, and this is the only way its edit reaches the
    /// settings file (docs/23 §14.54).
    pub fn preferences(&self) -> Option<crate::preferences::OverlayPreferences> {
        self.platform.preferences()
    }

    /// Take the last selection the user confirmed, if one is still held.
    ///
    /// `F6`'s input: pin what was just captured, rather than reading the screen again
    /// while the overlay still covers it. Taking rather than cloning keeps a multi-megabyte
    /// buffer from being duplicated per pin (`OverlayPlatform::take_selection`).
    pub fn take_selection(
        &self,
    ) -> Option<std::sync::Arc<crate::artifact::SelectionPixels>> {
        self.platform.take_selection()
    }

    /// Forward one low-frequency toolbar instruction to the active session's
    /// annotation document.
    pub fn annotation_command(&self, command: AnnotationCommand) -> CaptureResult<()> {
        self.platform.request_annotation(command)
    }
}

impl Drop for CaptureRuntime {
    fn drop(&mut self) {
        eprintln!("[snapclip][capture] CaptureRuntime dropped; shutting down overlay");
        self.platform.shutdown();
    }
}
