use crate::{
    ImageSource, PinContent, PinEvent, PinHandle, PinRequest, PinSelection, PinSnapshot,
    host::NativeImages, platform, view::PinView,
};
use gpui_kit::{component::Root, *};
use std::{collections::HashMap, sync::Arc};

fn initial_image_zoom(image: (u32, u32), work_area: (i32, i32)) -> f64 {
    (f64::from(work_area.0.max(1)) * 0.7 / f64::from(image.0))
        .min(f64::from(work_area.1.max(1)) * 0.7 / f64::from(image.1))
        .min(1.)
}

#[derive(Debug, Clone, Copy)]
pub enum PinPlacement {
    Pointer((i32, i32)),
    Screenshot((i32, i32), i32),
}
#[derive(Debug, Clone)]
pub enum CopyTarget {
    Content(Arc<PinSnapshot>),
    Text(String),
    Files(Vec<String>),
    Pixels(crate::PinImage),
}
#[derive(Debug, Clone)]
pub enum PinCommand {
    Copy { id: u64, target: CopyTarget },
}

enum Surface {
    Image(PinHandle),
    View(WindowHandle<Root>, WeakEntity<PinView>),
}
struct OpenPin {
    snapshot: Arc<PinSnapshot>,
    surface: Surface,
    _closed: Option<Subscription>,
    derived_bytes: usize,
}
/// One application-thread owner of live pins, pending native creations, undo and command delivery.
pub struct PinManager {
    native: NativeImages,
    open: HashMap<u64, OpenPin>,
    pending: HashMap<u64, Arc<PinSnapshot>>,
    closed: Vec<Arc<PinSnapshot>>,
    commands: async_channel::Sender<PinCommand>,
    owner: Option<WeakEntity<Self>>,
    undo_feedback: Option<WindowHandle<Root>>,
}
impl PinManager {
    #[cfg(all(test, feature = "headless-tests"))]
    pub(crate) fn view_for_test(&self, id: u64) -> (WindowHandle<Root>, WeakEntity<PinView>) {
        match &self.open[&id].surface {
            Surface::View(window, view) => (*window, view.clone()),
            _ => panic!("expected GPUI pin"),
        }
    }
    pub fn new() -> anyhow::Result<(Self, async_channel::Receiver<PinCommand>)> {
        let (commands, receiver) = async_channel::unbounded();
        Ok((
            Self {
                native: NativeImages::start()?,
                open: HashMap::new(),
                pending: HashMap::new(),
                closed: vec![],
                commands,
                owner: None,
                undo_feedback: None,
            },
            receiver,
        ))
    }
    pub fn open(
        snapshot: Arc<PinSnapshot>,
        placement: PinPlacement,
        owner: &Entity<Self>,
        cx: &mut App,
    ) -> anyhow::Result<PinHandle> {
        match snapshot.content() {
            PinContent::Document(doc) if doc.text().is_empty() => {
                anyhow::bail!("文档没有可显示的文字")
            }
            PinContent::Files(files)
                if files.is_empty()
                    || files.len() > 1024
                    || files
                        .iter()
                        .any(|f| f.path().is_empty() || f.path().contains('\0')) =>
            {
                anyhow::bail!("文件引用无效")
            }
            _ => {}
        }
        // Bound retained source + decoded pixels including undo; reject honestly rather than truncate.
        owner.update(cx, |this, cx| {
            this.owner = Some(owner.downgrade());
            this.poll(cx)
        });
        let admitted = owner
            .read(cx)
            .retained_bytes()
            .checked_add(snapshot.retained_bytes())
            .is_some_and(|bytes| bytes <= 256 * 1024 * 1024);
        if !admitted || owner.read(cx).open.len() + owner.read(cx).pending.len() >= 32 {
            anyhow::bail!("贴图已达到当前资源上限，请先关闭一些贴图");
        }
        let cursor = match placement {
            PinPlacement::Pointer(p) | PinPlacement::Screenshot(p, _) => p,
        };
        // F6 is an original-position, non-activating screenshot surface. F3 is
        // an inspectable GPUI viewer and shares YueImagePreview with file previews.
        if let (PinContent::Image(image), PinPlacement::Screenshot(position, radius)) =
            (snapshot.content(), placement)
        {
            let image = ImageSource::from_rgba(image.rgba(), image.width(), image.height())?;
            let handle = owner.read(cx).native.create_pin(
                PinRequest::new(image, position)
                    .radius(radius)
                    .text_available(snapshot.text().is_some_and(|text| !text.is_empty()))
                    .initial_zoom(1.),
            )?;
            owner.update(cx, |this, _| {
                this.pending.insert(handle.id(), snapshot);
            });
            return Ok(handle);
        }
        let handle = owner.read(cx).native.reserve_handle()?;
        let mut width = if matches!(snapshot.content(), PinContent::Files(_)) {
            384.
        } else {
            448.
        };
        let mut height = match snapshot.content() {
            PinContent::Files(files)
                if files.len() == 1 && files[0].kind() == crate::FileKind::Image =>
            {
                360.
            }
            PinContent::Files(files) => {
                (144. + files.len().saturating_sub(1).min(6) as f32 * 56.).clamp(240., 480.)
            }
            PinContent::Document(doc) => {
                (128. + doc.text().lines().count().min(16) as f32 * 24.).clamp(240., 540.)
            }
            _ => 360.,
        };
        if let PinContent::Image(image) = snapshot.content() {
            let (area, scale) = platform::work_area(cursor);
            let zoom = initial_image_zoom(
                (image.width(), image.height()),
                (area.2 - area.0, area.3 - area.1),
            ) as f32;
            width = (image.width() as f32 * zoom / scale + 32.).max(320.);
            height = (image.height() as f32 * zoom / scale + 112.).max(240.);
        }
        let weak = owner.downgrade();
        let mut view = None;
        let window = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(width), px(height)),
                ))),
                titlebar: None,
                focus: false,
                window_min_size: Some(size(px(320.), px(200.))),
                is_resizable: true,
                ..Default::default()
            },
            |window, cx| {
                platform::configure(window, cursor, (width, height), true);
                let pin =
                    cx.new(|cx| PinView::new(handle.id(), snapshot.clone(), weak, window, cx));
                view = Some(pin.downgrade());
                cx.new(|cx| Root::new(pin, window, cx))
            },
        )?;
        let weak = owner.downgrade();
        let window_id = window.window_id();
        let closed = cx.on_window_closed(move |cx, closed| {
            if closed == window_id {
                let weak = weak.clone();
                cx.defer(move |cx| {
                    _ = weak.update(cx, |manager, cx| manager.released(handle.id(), cx));
                });
            }
        });
        owner.update(cx, |this, _| {
            this.open.insert(
                handle.id(),
                OpenPin {
                    snapshot,
                    surface: Surface::View(window, view.expect("constructed pin")),
                    _closed: Some(closed),
                    derived_bytes: 0,
                },
            );
        });
        Ok(handle)
    }
    pub fn open_selection(
        selection: PinSelection,
        owner: &Entity<Self>,
        cx: &mut App,
    ) -> anyhow::Result<PinHandle> {
        let region = selection.region();
        let w = u32::try_from(region.width())?;
        let h = u32::try_from(region.height())?;
        let expected = u64::from(w) * u64::from(h) * 4;
        if expected != selection.bytes().len() as u64 {
            anyhow::bail!("截图像素与尺寸不符");
        }
        let mut rgba = selection.bytes().to_vec();
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
            pixel[3] = 255;
        }
        let image = crate::PinImage::new(w, h, rgba).ok_or_else(|| anyhow::anyhow!("截图为空"))?;
        Self::open(
            Arc::new(PinSnapshot::new(PinContent::Image(image))),
            PinPlacement::Screenshot((region.left, region.top), selection.corner_radius()),
            owner,
            cx,
        )
    }
    pub fn poll(&mut self, cx: &mut App) -> crate::PinReport {
        let mut observed = vec![];
        while let Some(event) = self.native.try_recv_event() {
            observed.push(event);
            match event {
                PinEvent::Created { id } => {
                    if let Some(snapshot) = self.pending.remove(&id) {
                        self.open.insert(
                            id,
                            OpenPin {
                                snapshot,
                                surface: Surface::Image(PinHandle::from_id(id)),
                                _closed: None,
                                derived_bytes: 0,
                            },
                        );
                    }
                }
                PinEvent::Closed { id } => self.released(id, cx),
                PinEvent::Failed { id } => {
                    eprintln!("[snapclip-pin] image window failed id={id}");
                    if self.pending.remove(&id).is_some() {
                        crate::show_feedback(
                            "无法创建图片贴图，请稍后重试",
                            platform::cursor_position(),
                            cx,
                        );
                    }
                }
                PinEvent::CopyRequested { id } => {
                    if let Some(pin) = self.open.get(&id) {
                        self.copy(id, CopyTarget::Content(pin.snapshot.clone()));
                    }
                }
                PinEvent::TextRequested { id } => {
                    if let Some(pin) = self.open.get(&id) {
                        if let Some(text) = pin.snapshot.text().filter(|t| !t.is_empty()) {
                            let mut snapshot = PinSnapshot::new(PinContent::Document(
                                crate::PinDocument::new(text.into(), crate::DocumentMode::Text),
                            ));
                            if let PinContent::Image(image) = pin.snapshot.content() {
                                snapshot = snapshot.with_image(image.clone());
                            }
                            let weak = self.owner.clone();
                            cx.defer(move |cx| {
                                if let Some(owner) = weak.and_then(|w| w.upgrade())
                                    && let Err(error) = Self::open(
                                        Arc::new(snapshot),
                                        PinPlacement::Pointer(platform::cursor_position()),
                                        &owner,
                                        cx,
                                    )
                                {
                                    crate::show_feedback(
                                        error.to_string(),
                                        platform::cursor_position(),
                                        cx,
                                    );
                                }
                            });
                        } else {
                            crate::show_feedback(
                                "该图片没有文字备选表示",
                                platform::cursor_position(),
                                cx,
                            );
                        }
                    }
                }
            }
        }
        crate::host::summarize(observed)
    }

    pub fn is_open(&self, id: u64) -> bool {
        self.open.contains_key(&id) || self.pending.contains_key(&id)
    }
    pub fn raise(&self, id: u64, cx: &mut App) -> bool {
        match self.open.get(&id).map(|pin| &pin.surface) {
            Some(Surface::Image(handle)) => self.native.raise(*handle).is_ok(),
            Some(Surface::View(handle, _)) => handle
                .update(cx, |_, window, _| {
                    platform::visible(window, true);
                    platform::raise(window);
                })
                .is_ok(),
            None => self.pending.contains_key(&id),
        }
    }
    pub(crate) fn released(&mut self, id: u64, cx: &mut App) {
        if let Some(pin) = self.open.remove(&id) {
            self.closed.push(pin.snapshot);
            while self.closed.len() > 8 || self.retained_bytes() > 256 * 1024 * 1024 {
                self.closed.remove(0);
            }
            if let Some(handle) = self.undo_feedback.take() {
                _ = handle.update(cx, |_, window, _| window.remove_window());
            }
            if !self.closed.is_empty() {
                self.undo_feedback = crate::feedback::show(
                    "贴图已关闭".into(),
                    platform::cursor_position(),
                    self.owner.clone(),
                    std::time::Duration::from_secs(5),
                    cx,
                );
            }
        }
    }
    pub fn close(&mut self, id: u64, cx: &mut App) {
        if self.pending.remove(&id).is_some() {
            _ = self.native.close_pin(PinHandle::from_id(id));
        }
        if let Some(pin) = self.open.get(&id) {
            match &pin.surface {
                Surface::Image(handle) => {
                    _ = self.native.close_pin(*handle);
                }
                Surface::View(handle, _) => {
                    _ = handle.update(cx, |_, window, _| window.remove_window());
                }
            }
        }
    }
    pub fn close_all(&mut self, cx: &mut App) {
        let ids: Vec<_> = self
            .open
            .keys()
            .chain(self.pending.keys())
            .copied()
            .collect();
        for id in ids {
            self.close(id, cx);
        }
    }
    pub fn set_visible(&mut self, visible: bool, cx: &mut App) {
        // These commands follow Create in the native host's FIFO even if Created
        // has not reached the application registry yet.
        for id in self.pending.keys() {
            _ = self.native.set_visible(PinHandle::from_id(*id), visible);
        }
        for pin in self.open.values() {
            match &pin.surface {
                Surface::Image(handle) => {
                    _ = self.native.set_visible(*handle, visible);
                }
                Surface::View(handle, _) => {
                    _ = handle.update(cx, |_, window, _| platform::visible(window, visible));
                }
            }
        }
    }
    pub fn undo(owner: &Entity<Self>, cursor: (i32, i32), cx: &mut App) -> anyhow::Result<()> {
        if let Some(handle) = owner.update(cx, |this, _| this.undo_feedback.take()) {
            _ = handle.update(cx, |_, window, _| window.remove_window());
        }
        if let Some(snapshot) = owner.update(cx, |this, _| this.closed.pop())
            && let Err(error) =
                Self::open(snapshot.clone(), PinPlacement::Pointer(cursor), owner, cx)
        {
            owner.update(cx, |this, _| this.closed.push(snapshot));
            return Err(error);
        }
        Ok(())
    }
    pub(crate) fn copy(&self, id: u64, target: CopyTarget) {
        _ = self.commands.try_send(PinCommand::Copy { id, target });
    }
    pub(crate) fn retain_preview(&mut self, id: u64, bytes: usize) -> bool {
        let old = self.open.get(&id).map_or(0, |pin| pin.derived_bytes);
        if self
            .retained_bytes()
            .saturating_sub(old)
            .checked_add(bytes)
            .is_none_or(|total| total > 256 * 1024 * 1024)
        {
            return false;
        }
        if let Some(pin) = self.open.get_mut(&id) {
            pin.derived_bytes = bytes;
            true
        } else {
            false
        }
    }
    pub fn report_copy(&self, id: u64, result: Result<(), String>, cx: &mut App) {
        if self
            .open
            .get(&id)
            .is_some_and(|pin| matches!(pin.surface, Surface::Image(_)))
        {
            crate::show_feedback(
                result.err().unwrap_or_else(|| "已复制图片".into()),
                platform::cursor_position(),
                cx,
            );
            return;
        }
        if let Some(OpenPin {
            surface: Surface::View(handle, view),
            ..
        }) = self.open.get(&id)
            && let Some(view) = view.upgrade()
        {
            _ = handle.update(cx, |_, window, cx| {
                view.update(cx, |this, cx| this.copied(result, window, cx))
            });
        }
    }
    fn retained_bytes(&self) -> usize {
        self.open
            .values()
            .map(|pin| pin.snapshot.retained_bytes() + pin.derived_bytes)
            .chain(self.pending.values().map(|s| s.retained_bytes()))
            .chain(self.closed.iter().map(|s| s.retained_bytes()))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn clipboard_images_fit_the_monitor_without_mutating_original_pixels() {
        assert_eq!(super::initial_image_zoom((100, 100), (1920, 1080)), 1.);
        let zoom = super::initial_image_zoom((4000, 3000), (1920, 1080));
        assert!(4000. * zoom <= 1920. * 0.7);
        assert!(3000. * zoom <= 1080. * 0.7);
        assert!(super::initial_image_zoom((1, 24_000_000), (1920, 1080)) > 0.);
    }
}
