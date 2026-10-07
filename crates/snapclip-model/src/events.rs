//! Low-frequency event summaries shared by the shells (docs/23 T4.5, docs/22 §8).
//!
//! These types are the **only** shape in which one capability's news reaches another. They
//! carry identity, state, size and an [`ArtifactRef`] — never pixels, never a mouse move,
//! never a database handle. A shell adapts them to whatever its transport is: the Tauri
//! host wraps them in its versioned `EventEnvelope`, the GPUI shell passes them through a
//! typed channel. That is why the envelope does **not** live here: it is transport, and
//! `tauri::Emitter` must never be reachable from a capability crate.
//!
//! `generation` is a per-process counter the caller stamps in; a consumer drops anything
//! older than what it has already seen. It is deliberately a plain number rather than a
//! shared clock, so this crate stays free of synchronisation machinery.

use crate::artifact::ArtifactRef;
use crate::capture::{CaptureState, PixelFormat};
use crate::payload::PayloadKind;
use crate::recognition::{OcrErrorCode, OcrStatus};
use crate::ErrorCode;

/// Something a shell may need to react to, at a frequency a human can perceive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    Capture(CaptureEvent),
    Clipboard(ClipboardEvent),
    Recognition(RecognitionEvent),
}

impl AppEvent {
    /// The generation this event was stamped with, whatever its kind.
    pub fn generation(&self) -> u64 {
        match self {
            Self::Capture(event) => event.generation,
            Self::Clipboard(event) => event.generation,
            Self::Recognition(event) => event.generation,
        }
    }
}

/// One step of a capture session's lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureEvent {
    pub session_id: String,
    pub state: CaptureState,
    /// Present once the session produced a file; the reference is what a consumer needs to
    /// open it, so the bytes themselves never travel through an event.
    pub artifact: Option<ArtifactRef>,
    /// Present when the session failed.
    pub error_code: Option<ErrorCode>,
    pub generation: u64,
}

/// A clipboard entry that became visible to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardEvent {
    pub clip_id: String,
    pub kind: PayloadKind,
    /// Present for image payloads; `ImageDimensions` lives in the artifact module's
    /// vocabulary but is the same value type the store writes.
    pub dimensions: Option<crate::geometry::ImageDimensions>,
    /// Pixel layout, for consumers that care how the bytes are arranged.
    pub pixel_format: Option<PixelFormat>,
    pub generation: u64,
}

/// A recognition job changing state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecognitionEvent {
    pub clip_id: String,
    pub status: OcrStatus,
    pub error_code: Option<OcrErrorCode>,
    pub generation: u64,
}

#[cfg(test)]
mod tests {
    use super::{AppEvent, CaptureEvent, ClipboardEvent, RecognitionEvent};
    use crate::capture::CaptureState;
    use crate::payload::PayloadKind;
    use crate::recognition::OcrStatus;
    use std::mem::size_of;

    #[test]
    fn an_event_carries_identity_and_state_but_no_bulk_data() {
        let event = AppEvent::Capture(CaptureEvent {
            session_id: "capture-1-1".into(),
            state: CaptureState::Selected,
            artifact: None,
            error_code: None,
            generation: 7,
        });
        assert_eq!(event.generation(), 7);

        // The point of these events is that they stay cheap to send: identity, state and a
        // reference. A `Vec<u8>` of pixels or a screenshot path's worth of text inline would
        // blow this bound, which is the mistake the design is trying to prevent.
        assert!(
            size_of::<AppEvent>() <= 256,
            "AppEvent grew to {} bytes; something bulk is being carried inline",
            size_of::<AppEvent>()
        );
    }

    #[test]
    fn each_kind_reports_the_generation_it_was_stamped_with() {
        let clipboard = AppEvent::Clipboard(ClipboardEvent {
            clip_id: "clip-1".into(),
            kind: PayloadKind::Text,
            dimensions: None,
            pixel_format: None,
            generation: 41,
        });
        let recognition = AppEvent::Recognition(RecognitionEvent {
            clip_id: "clip-1".into(),
            status: OcrStatus::Queued,
            error_code: None,
            generation: 42,
        });
        assert_eq!(clipboard.generation(), 41);
        assert_eq!(recognition.generation(), 42);
        // A consumer drops the older one; this is what makes that decision possible without
        // a shared clock.
        assert!(recognition.generation() > clipboard.generation());
    }
}
