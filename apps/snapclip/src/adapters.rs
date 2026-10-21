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
    /// 「请把刚截的那张贴出去」的投递器（延时时到点用）。
    ///
    /// 存成闭包而不是直接存 `PinPoster`：接线发生在**贴图管理器起来之后**（两份东西各自
    /// 在半路上创建），闭包让"到底怎么贴"留在接线处一行，也让这件事可以被测——测试塞一个
    /// 记账的闭包进来，断言延时到点时确实发了这一条（docs/23 §14.55.8）。
    pin_poster: std::sync::Mutex<
        Option<std::sync::Arc<dyn Fn() -> Result<(), String> + Send + Sync>>,
    >,
    /// 「请为这张选区开保存面板」的投递器，与 `pin_poster` 同一形状、同一理由：接线发生在外壳
    /// 起来之后，存成闭包让"怎么开面板"留在接线处一行，测试也能塞一个记账闭包进来断言确实收到了那张图。
    save_opener: std::sync::Mutex<
        Option<
            std::sync::Arc<
                dyn Fn(&snapclip_capture::ports::SelectionPixels) -> Result<(), String>
                    + Send
                    + Sync,
            >,
        >,
    >,
}

impl CaptureEvents {
    pub fn new(bus: EventBus) -> Self {
        Self {
            bus,
            pin_poster: std::sync::Mutex::new(None),
            save_opener: std::sync::Mutex::new(None),
        }
    }

    /// 接上"怎么贴图"。外壳在贴图管理器起来之后调用一次（见 `lib.rs`）。
    pub fn set_pin_poster(
        &self,
        poster: std::sync::Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
    ) {
        *self
            .pin_poster
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(poster);
    }

    /// 接上「怎么开保存面板」。外壳在启动时调用一次（见 `lib.rs`），与贴图那条同一个位置。
    pub fn set_save_opener(
        &self,
        opener: std::sync::Arc<
            dyn Fn(&snapclip_capture::ports::SelectionPixels) -> Result<(), String> + Send + Sync,
        >,
    ) {
        *self
            .save_opener
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(opener);
    }

    /// 把一次"请贴图"交给接好的投递器；没接上时返回 `None`（外壳的接线问题，要说出来）。
    fn post_pin(&self) -> Option<Result<(), String>> {
        let poster = self
            .pin_poster
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()?;
        Some(poster())
    }

    /// 把一次"请开保存面板"交给接好的投递器；没接上时返回 `None`（外壳的接线问题，要说出来）。
    fn post_save(
        &self,
        pixels: &snapclip_capture::ports::SelectionPixels,
    ) -> Option<Result<(), String>> {
        let opener = self
            .save_opener
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()?;
        Some(opener(pixels))
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

    /// 用户按了 `Ctrl+S`：选区像素已经交到这里，请打开保存面板。
    ///
    /// 与 `on_pin_requested` 同一个分工：这里**只说"要开面板"并给出那张图**，
    /// 面板怎么开、开在哪里由接线处决定（`lib.rs` 接的是 `save::window::open`）。
    fn on_save_requested(
        &self,
        session_id: &str,
        pixels: &snapclip_capture::ports::SelectionPixels,
    ) {
        match self.post_save(pixels) {
            Some(Ok(())) => {}
            Some(Err(error)) => eprintln!(
                "[snapclip-app][capture] session={session_id} 打开保存面板失败：{error}"
            ),
            None => eprintln!(
                "[snapclip-app][capture] session={session_id} 收到保存请求，但保存面板没接上"
            ),
        }
    }

    /// 延时截图到点：图已经在贴图槽位里，请贴图那条链路把它贴出去。
    ///
    /// 这里**只发一次请求**，贴图怎么做由接线处决定：`lib.rs` 接的是"投一条 `WM_HOTKEY`"，
    /// 也就是用户按 `F6` 的同一条路（docs/23 §14.55.8）。
    fn on_pin_requested(&self, session_id: &str) {
        match self.post_pin() {
            Some(Ok(())) => {}
            Some(Err(error)) => eprintln!(
                "[snapclip-app][capture] session={session_id} 延时截图请求贴图失败：{error}"
            ),
            None => eprintln!(
                "[snapclip-app][capture] session={session_id} 延时截图到点，但贴图功能没接上"
            ),
        }
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

#[cfg(test)]
mod delay_pin_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 延时截图到点：事件出口必须**正好发一次**"请贴图"。
    ///
    /// 这是这条路径上能自动化验证的那一半——另一半（投一条 `WM_HOTKEY`、贴图窗口真的出现）
    /// 要真机与真窗口，只能人工看（docs/23 §14.55.8）。
    #[test]
    fn a_pin_request_reaches_the_poster_exactly_once() {
        let events = CaptureEvents::new(EventBus::new());
        let posted = Arc::new(AtomicUsize::new(0));
        let counter = posted.clone();
        events.set_pin_poster(Arc::new(move || {
            counter.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }));
        events.on_pin_requested("session-1");
        assert_eq!(posted.load(Ordering::Acquire), 1);
    }

    /// 没接上投递器（贴图功能没起来）时不能 panic：那是接线问题，日志里说清楚就够了。
    #[test]
    fn a_pin_request_without_a_poster_is_not_a_crash() {
        let events = CaptureEvents::new(EventBus::new());
        events.on_pin_requested("session-1");
    }

    /// 投递器报错也不能 panic：失败要说出来（日志），但不该把消息泵带走。
    #[test]
    fn a_failing_poster_is_reported_rather_than_propagated() {
        let events = CaptureEvents::new(EventBus::new());
        events.set_pin_poster(Arc::new(|| Err("宿主线程已经不在了".to_string())));
        events.on_pin_requested("session-1");
    }
}
