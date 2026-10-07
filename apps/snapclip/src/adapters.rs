//! Ports the capability crates declare, implemented by the shell (docs/23 T4.5).
//!
//! The capability crates are the ones that *know* something happened; this is where that
//! knowledge becomes an [`AppEvent`] and enters the shell. Each adapter is `Send + Sync`
//! because the producers call it from their own threads (the clipboard listener thread, the
//! capture export worker), and publishing is a non-blocking channel send.
//!
//! Nothing here is UI: these adapters know the bus and `snapclip-model`, not `gpui_kit`.

use snapclip_capture::ports::CaptureEventSink;
use snapclip_model::{AppEvent, CaptureEvent, CaptureState, ClipboardEvent};

use crate::events::EventBus;

/// Turns a saved clipboard publication into `AppEvent::Clipboard`.
///
/// Implements the history crate's sink port, which is what its ingest service calls after a
/// publication is durably stored — so an event here always describes a row that exists.
pub struct ClipboardEvents {
    bus: EventBus,
}

impl ClipboardEvents {
    pub fn new(bus: EventBus) -> Self {
        Self { bus }
    }
}

impl snapclip_history::ingest::ClipboardEventSink for ClipboardEvents {
    fn on_publication_saved(&self, publication: &snapclip_model::Publication) {
        // The row's kind and image size come from the same payload the store calls primary,
        // so the event can never describe a different payload than the list will show.
        let (kind, dimensions) = match publication.primary_payload() {
            Some(payload) => (payload.kind.clone(), payload.image_dimensions.clone()),
            None => return,
        };
        self.bus.publish(AppEvent::Clipboard(ClipboardEvent {
            clip_id: publication.publication_id.clone(),
            kind,
            dimensions,
            // The store keeps PNG bytes for image payloads; this records the layout the
            // bytes decode to rather than a guess. Capture is the only producer that has
            // raw pixels, and it reports them through its own event.
            pixel_format: None,
            generation: 0,
        }));
    }
}

/// Turns capture's lifecycle callbacks into `AppEvent::Capture`.
///
/// The overlay and its capture loop stay outside the UI process's event path: this adapter
/// only hears about session boundaries (armed, selected, exported, cancelled), never about
/// a mouse move or a frame. That is the property T4.5's "零 IPC 高频路径" is about.
pub struct CaptureEvents {
    bus: EventBus,
}

impl CaptureEvents {
    pub fn new(bus: EventBus) -> Self {
        Self { bus }
    }

    fn state(&self, session_id: &str, state: CaptureState) -> u64 {
        self.bus.publish(AppEvent::Capture(CaptureEvent {
            session_id: session_id.to_string(),
            state,
            artifact: None,
            error_code: None,
            generation: 0,
        }))
    }
}

impl CaptureEventSink for CaptureEvents {
    fn on_started(&self, session_id: &str, _layout: &snapclip_capture::geometry::MonitorLayout) {
        self.state(session_id, CaptureState::Preparing);
    }

    fn on_state(
        &self,
        session_id: &str,
        state: CaptureState,
        _layout: Option<&snapclip_capture::geometry::MonitorLayout>,
    ) {
        self.state(session_id, state);
    }

    fn on_completed(&self, artifact: &snapclip_model::CaptureArtifact) {
        self.bus.publish(AppEvent::Capture(CaptureEvent {
            session_id: artifact.session_id.clone(),
            state: CaptureState::Idle,
            // No reference yet, and deliberately not invented: a `CaptureArtifact` carries
            // the domain result (size, DPI, where the PNG is), while the `ArtifactRef` with
            // its write-time fingerprint is what the shell's own `ArtifactWriter` port
            // receives. When T6 moves the composition root here, the writer is what
            // publishes the completion event with that reference; this callback keeps the
            // session's state honest until then.
            artifact: None,
            error_code: None,
            generation: 0,
        }));
    }

    fn on_cancelled(&self, session_id: &str, _reason: &str) {
        self.state(session_id, CaptureState::Idle);
    }

    fn on_failed(
        &self,
        session_id: Option<&str>,
        error: &snapclip_capture::CaptureError,
        _provider: &str,
    ) {
        self.bus.publish(AppEvent::Capture(CaptureEvent {
            session_id: session_id.unwrap_or_default().to_string(),
            state: CaptureState::Idle,
            artifact: None,
            // The error's code, not its message: the message is for the log, and a
            // user-facing sentence is the UI's to write.
            error_code: Some(error.code()),
            generation: 0,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::{CaptureEvents, ClipboardEvents};
    use crate::events::EventBus;
    use snapclip_capture::ports::CaptureEventSink;
    use snapclip_model::{
        AppEvent, CaptureState, ImageDimensions, PayloadKind, PayloadRef, Publication,
        PublicationOrigin,
    };

    fn payload(kind: PayloadKind, dimensions: Option<ImageDimensions>) -> PayloadRef {
        PayloadRef {
            payload_id: "payload-1".into(),
            content_hash: "hash-1".into(),
            kind,
            size_bytes: 4,
            mime_type: None,
            image_dimensions: dimensions,
        }
    }

    fn publication(payloads: Vec<PayloadRef>) -> Publication {
        Publication {
            publication_id: "clip-1".into(),
            origin: PublicationOrigin::Clipboard,
            captured_at_unix_ms: 1,
            source_app: None,
            source_exe_path: None,
            payloads,
        }
    }

    #[test]
    fn a_saved_publication_becomes_a_clipboard_event_about_its_primary_payload() {
        use snapclip_history::ingest::ClipboardEventSink;

        let bus = EventBus::new();
        let mut stream = bus.subscribe();
        let sink = ClipboardEvents::new(bus);
        // The store's rule is "first payload is primary"; the event must agree with it, so
        // a publication whose second payload is the image still reports the text kind.
        sink.on_publication_saved(&publication(vec![
            payload(PayloadKind::Text, None),
            payload(
                PayloadKind::Image,
                Some(ImageDimensions {
                    width: 4,
                    height: 2,
                }),
            ),
        ]));

        match stream.try_next() {
            Some(AppEvent::Clipboard(event)) => {
                assert_eq!(event.clip_id, "clip-1");
                assert_eq!(event.kind, PayloadKind::Text);
                assert_eq!(event.dimensions, None);
            }
            other => panic!("expected a clipboard event, got {other:?}"),
        }
    }

    #[test]
    fn an_image_publication_reports_the_size_the_store_wrote() {
        use snapclip_history::ingest::ClipboardEventSink;

        let bus = EventBus::new();
        let mut stream = bus.subscribe();
        let sink = ClipboardEvents::new(bus);
        sink.on_publication_saved(&publication(vec![payload(
            PayloadKind::Image,
            Some(ImageDimensions {
                width: 1920,
                height: 1080,
            }),
        )]));

        match stream.try_next() {
            Some(AppEvent::Clipboard(event)) => {
                assert_eq!(event.kind, PayloadKind::Image);
                assert_eq!(
                    event.dimensions,
                    Some(ImageDimensions {
                        width: 1920,
                        height: 1080
                    })
                );
            }
            other => panic!("expected a clipboard event, got {other:?}"),
        }
    }

    #[test]
    fn a_publication_without_payloads_says_nothing_instead_of_guessing() {
        use snapclip_history::ingest::ClipboardEventSink;

        let bus = EventBus::new();
        let mut stream = bus.subscribe();
        let sink = ClipboardEvents::new(bus.clone());
        sink.on_publication_saved(&publication(Vec::new()));
        // Nothing was published, so the bus has not advanced and no event is waiting.
        assert_eq!(bus.published(), 0);
        assert!(stream.try_next().is_none());
    }

    #[test]
    fn capture_boundaries_reach_the_bus_as_states_and_never_as_pixels() {
        let bus = EventBus::new();
        let mut stream = bus.subscribe();
        let sink = CaptureEvents::new(bus);
        sink.on_started("capture-1-1", &layout());
        sink.on_state("capture-1-1", CaptureState::Selecting, None);
        sink.on_cancelled("capture-1-1", "escape");

        let mut states = Vec::new();
        while let Some(event) = stream.try_next() {
            match event {
                AppEvent::Capture(event) => {
                    assert_eq!(event.session_id, "capture-1-1");
                    assert!(event.artifact.is_none());
                    states.push(event.state);
                }
                other => panic!("expected a capture event, got {other:?}"),
            }
        }
        assert_eq!(
            states,
            [
                CaptureState::Preparing,
                CaptureState::Selecting,
                CaptureState::Idle
            ]
        );
    }

    fn layout() -> snapclip_capture::geometry::MonitorLayout {
        let bounds = snapclip_model::Rect::new(0, 0, 1920, 1080);
        snapclip_capture::geometry::MonitorLayout {
            bounds,
            work_area: snapclip_model::Rect::new(0, 0, 1920, 1040),
            dpi: 96,
            primary: true,
        }
    }
}
