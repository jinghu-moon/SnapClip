//! F3 orchestration: one reader, immutable snapshots, expiration and reliable copy completion.
use gpui_kit::*;
use snapclip_clipboard::{PreparedSnapshot, ReadError, ReadPurpose, SystemClipboard};
use snapclip_pin::{CopyTarget, PinCommand, PinManager, PinPlacement};
use std::{
    collections::HashMap,
    sync::{Arc, mpsc},
    time::Duration,
};

#[derive(Clone, Copy, Debug)]
struct Request {
    id: u64,
    sequence: u32,
    position: (i32, i32),
}
#[derive(Default)]
struct Requests {
    next: u64,
    active: Option<Request>,
    wanted: Option<Request>,
    expired: bool,
}
impl Requests {
    fn request(&mut self, sequence: u32, position: (i32, i32)) -> Option<Request> {
        if self.wanted.is_some_and(|r| r.sequence == sequence) && !self.expired {
            return None;
        }
        self.next = self.next.checked_add(1).expect("request id exhausted");
        let request = Request {
            id: self.next,
            sequence,
            position,
        };
        self.wanted = Some(request);
        self.expired = false;
        if self.active.is_none() {
            self.active = Some(request);
            Some(request)
        } else {
            None
        }
    }
    fn accepts(&self, id: u64) -> bool {
        !self.expired && self.wanted.is_some_and(|r| r.id == id)
    }
    fn complete(&mut self, id: u64) -> Option<Request> {
        if !self.active.is_some_and(|r| r.id == id) {
            return None;
        }
        self.active = None;
        if let Some(next) = self.wanted.filter(|r| r.id != id && !self.expired) {
            self.active = Some(next);
            Some(next)
        } else {
            self.wanted = None;
            None
        }
    }
    fn expire(&mut self, id: u64) -> bool {
        if self.accepts(id) {
            self.expired = true;
            true
        } else {
            false
        }
    }
    fn cancel(&mut self) {
        self.wanted = None;
        self.expired = true;
    }
}
struct Completion {
    request: Request,
    result: Result<PreparedSnapshot, ReadError>,
}
pub struct ClipboardPins {
    manager: Entity<PinManager>,
    requests: Requests,
    sender: Option<mpsc::SyncSender<Request>>,
    /// Keys include actual bytes as well as the observed sequence, not a permanent Win32 id.
    opened: HashMap<(u32, [u8; 32]), u64>,
    watchdog: Option<Task<()>>,
}
impl ClipboardPins {
    pub fn start(manager: Entity<PinManager>, cx: &mut App) -> Result<Entity<Self>, String> {
        let (sender, incoming) = mpsc::sync_channel::<Request>(1);
        let (done, completed) = async_channel::unbounded();
        std::thread::Builder::new()
            .name("snapclip-pin-reader".into())
            .spawn(move || {
                while let Ok(request) = incoming.recv() {
                    let result = snapclip_clipboard::windows::reader::read_snapshot(
                        ReadPurpose::ExplicitPin,
                        Some(request.sequence),
                    )
                    .and_then(|snapshot| {
                        snapclip_clipboard::prepare_with_image_decoder(snapshot, decode_pin_image)
                    });
                    if done.send_blocking(Completion { request, result }).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| format!("clipboard reader: {e}"))?;
        let entity = cx.new(|cx: &mut Context<Self>| {
            cx.spawn(async move |this, cx| {
                while let Ok(completion) = completed.recv().await {
                    if this
                        .update(cx, |this, cx| this.complete(completion, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
            Self {
                manager,
                requests: Requests::default(),
                sender: Some(sender),
                opened: HashMap::new(),
                watchdog: None,
            }
        });
        Ok(entity)
    }
    pub fn request(&mut self, sequence: u32, position: (i32, i32), cx: &mut Context<Self>) {
        let previous = self.requests.next;
        let dispatch = self.requests.request(sequence, position);
        if self.requests.next == previous {
            return;
        }
        let request = self.requests.wanted.expect("new request");
        // The deadline starts at the user's command, including time waiting behind a busy provider.
        self.watchdog = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            _ = this.update(cx, |this, cx| {
                if this.requests.expire(request.id) {
                    snapclip_pin::show_feedback(
                        "读取剪贴板超时，请稍后重新按 F3",
                        request.position,
                        cx,
                    );
                }
            });
        }));
        if let Some(request) = dispatch {
            self.dispatch(request, cx);
        }
    }
    pub fn cancel(&mut self) {
        self.requests.cancel();
        self.watchdog = None;
    }
    fn dispatch(&mut self, request: Request, cx: &mut Context<Self>) {
        if self
            .sender
            .as_ref()
            .is_none_or(|sender| sender.try_send(request).is_err())
        {
            self.requests.cancel();
            self.requests.complete(request.id);
            self.watchdog = None;
            snapclip_pin::show_feedback("剪贴板读取服务不可用", request.position, cx);
        }
    }
    fn complete(&mut self, completion: Completion, cx: &mut Context<Self>) {
        let request = completion.request;
        if self.requests.accepts(request.id) {
            match completion.result {
                Ok(prepared) => {
                    let snapshot = prepared.snapshot();
                    let key = (snapshot.completed_sequence(), prepared.fingerprint());
                    self.manager.update(cx, |manager, cx| manager.poll(cx));
                    self.opened
                        .retain(|_, id| self.manager.read(cx).is_open(*id));
                    let raised = self.opened.get(&key).is_some_and(|id| {
                        self.manager
                            .update(cx, |manager, cx| manager.raise(*id, cx))
                    });
                    if !raised {
                        match PinManager::open(
                            Arc::new(prepared.into_pin()),
                            PinPlacement::Pointer(request.position),
                            &self.manager,
                            cx,
                        ) {
                            Ok(handle) => {
                                self.opened.insert(key, handle.id());
                            }
                            Err(error) => {
                                snapclip_pin::show_feedback(error.to_string(), request.position, cx)
                            }
                        }
                    }
                }
                Err(error) => snapclip_pin::show_feedback(error.to_string(), request.position, cx),
            }
        }
        if let Some(next) = self.requests.complete(request.id) {
            self.dispatch(next, cx);
        } else {
            self.watchdog = None;
        }
    }
}

pub fn drain_copies(
    manager: &Entity<PinManager>,
    commands: async_channel::Receiver<PinCommand>,
    cx: &mut App,
) {
    let manager = manager.downgrade();
    cx.spawn(async move |cx| {
        while let Ok(PinCommand::Copy { id, target }) = commands.recv().await {
            let result = cx
                .background_spawn(async move {
                    match target {
                        CopyTarget::Text(text) => SystemClipboard.copy_text(&text),
                        CopyTarget::Files(paths) => SystemClipboard.copy_files(&paths),
                        CopyTarget::Content(snapshot) => SystemClipboard.copy_pin(&snapshot),
                        CopyTarget::Pixels(image) => SystemClipboard.copy_pin_image(&image),
                        CopyTarget::HdrPixels(image) => {
                            let bytes = crate::capture::pixel_export::encode_pixels(
                                image.pixels(),
                                crate::capture::pixel_export::ColorExport::PreserveHdr,
                            )?;
                            SystemClipboard.copy_hdr_png(&bytes)
                        }
                    }
                })
                .await;
            if manager
                .update(cx, |manager, cx| manager.report_copy(id, result, cx))
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
}

fn decode_pin_image(
    bytes: &[u8],
    encoding: snapclip_clipboard::ImageEncoding,
) -> Result<snapclip_pin::PinImage, ReadError> {
    if bytes.len() > snapclip_clipboard::MAX_BYTES {
        return Err(ReadError::TooLarge);
    }
    let bmp;
    let encoded = if encoding == snapclip_clipboard::ImageEncoding::Dib {
        bmp = snapclip_clipboard::image::dib_to_bmp(bytes).map_err(|_| ReadError::InvalidImage)?;
        bmp.as_slice()
    } else {
        bytes
    };
    snapclip_pin::decode_image(encoded).map_err(ReadError::Platform)
}

#[cfg(test)]
mod tests {
    use super::Requests;
    #[test]
    fn f3_png_uses_the_color_managed_hdr_decoder() {
        use snapclip_model::pixels::{AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding};
        let descriptor = PixelDescriptor::new(
            snapclip_model::PixelFormat::Rgba16Float,
            PixelEncoding::ScRgb,
            AlphaMode::Straight,
        )
        .unwrap();
        let bytes = [12.5_f32, 12.5, 12.5, 0.5]
            .into_iter()
            .flat_map(|v| half::f16::from_f32(v).to_le_bytes())
            .collect();
        let pixels = PixelBuffer::from_tight(1, 1, descriptor, bytes).unwrap();
        let png = crate::capture::pixel_export::encode_pixels(
            &pixels,
            crate::capture::pixel_export::ColorExport::PreserveHdr,
        )
        .unwrap();
        let prepared = snapclip_clipboard::prepare_with_image_decoder(
            snapclip_clipboard::Snapshot::new(7)
                .with_image(snapclip_clipboard::ImageEncoding::Png, png),
            super::decode_pin_image,
        )
        .unwrap();
        let snapclip_pin::PinContent::Image(image) = prepared.content() else {
            panic!("expected image");
        };
        assert!(image.is_hdr());
        let r =
            half::f16::from_le_bytes(image.pixels().as_bytes()[..2].try_into().unwrap()).to_f32();
        assert!((r - 12.5).abs() < 0.02);
        assert!(prepared.into_pin().notice().is_none());
    }
    #[test]
    fn repeated_same_pending_snapshot_is_coalesced() {
        let mut r = Requests::default();
        let a = r.request(1, (0, 0)).unwrap();
        assert!(r.request(1, (10, 10)).is_none());
        assert!(r.accepts(a.id));
        assert!(r.complete(a.id).is_none());
    }
    #[test]
    fn newer_request_supersedes_late_result_and_only_latest_is_queued() {
        let mut r = Requests::default();
        let a = r.request(1, (0, 0)).unwrap();
        assert!(r.request(2, (0, 0)).is_none());
        assert!(!r.accepts(a.id));
        r.request(3, (0, 0));
        let b = r.complete(a.id).unwrap();
        assert_eq!(b.sequence, 3);
        assert!(r.accepts(b.id));
    }
    #[test]
    fn timed_out_reader_is_not_replaced_with_another_thread() {
        let mut r = Requests::default();
        let a = r.request(1, (0, 0)).unwrap();
        assert!(r.expire(a.id));
        assert!(!r.accepts(a.id));
        assert!(r.request(2, (0, 0)).is_none());
        let b = r.complete(a.id).unwrap();
        assert_eq!(b.sequence, 2);
    }
    #[test]
    fn queued_request_can_expire_before_the_blocked_read_finishes() {
        let mut r = Requests::default();
        let a = r.request(1, (0, 0)).unwrap();
        r.request(2, (0, 0));
        let b = r.wanted.unwrap();
        assert!(r.expire(b.id));
        assert!(!r.accepts(a.id));
        assert!(!r.accepts(b.id));
        assert!(r.complete(a.id).is_none());
        assert!(r.request(3, (0, 0)).is_some());
    }
    #[test]
    fn close_all_cancels_current_and_queued_results() {
        let mut r = Requests::default();
        let a = r.request(1, (0, 0)).unwrap();
        r.request(2, (0, 0));
        r.cancel();
        assert!(!r.accepts(a.id));
        assert!(r.complete(a.id).is_none());
        assert!(r.request(2, (0, 0)).is_some());
    }
}
