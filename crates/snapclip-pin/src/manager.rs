use crate::{
    PinContent, PinSelection, PinSnapshot, platform, screenshot::ScreenshotView, view::PinView,
};
use gpui_kit::{component::Root, *};
use std::{collections::HashMap, sync::Arc};

fn initial_image_zoom(image: (u32, u32), work_area: (i32, i32)) -> f64 {
    (f64::from(work_area.0.max(1)) * 0.7 / f64::from(image.0))
        .min(f64::from(work_area.1.max(1)) * 0.7 / f64::from(image.1))
        .min(1.)
}
fn validate_selection(selection: &PinSelection) -> anyhow::Result<()> {
    let region = selection.region();
    let dimensions = [
        u32::try_from(region.width())?,
        u32::try_from(region.height())?,
    ];
    anyhow::ensure!(
        !region.is_empty() && dimensions == selection.image().pixels().size(),
        "截图像素与尺寸不符"
    );
    Ok(())
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PinHandle {
    id: u64,
}
impl PinHandle {
    pub fn id(self) -> u64 {
        self.id
    }
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
    HdrPixels(crate::PinImage),
}
#[derive(Debug, Clone)]
pub enum PinCommand {
    Copy { id: u64, target: CopyTarget },
}
enum Surface {
    Screenshot(WindowHandle<Root>, WeakEntity<ScreenshotView>),
    View(WindowHandle<Root>, WeakEntity<PinView>),
}
impl Surface {
    fn window(&self) -> WindowHandle<Root> {
        match self {
            Self::Screenshot(w, _) | Self::View(w, _) => *w,
        }
    }
}
struct OpenPin {
    snapshot: Arc<PinSnapshot>,
    surface: Surface,
    _closed: Subscription,
    derived_bytes: usize,
    ready: bool,
    on_created: Option<Box<dyn FnOnce()>>,
}
/// One application-thread owner of windows, preparation receipts, undo and commands.
/// F3/F6 share GPUI/Yue resources; there is no private image thread or polling loop.
pub struct PinManager {
    next_id: u64,
    open: HashMap<u64, OpenPin>,
    closed: Vec<Arc<PinSnapshot>>,
    commands: async_channel::Sender<PinCommand>,
    owner: Option<WeakEntity<Self>>,
    undo_feedback: Option<WindowHandle<Root>>,
}
impl PinManager {
    #[cfg(all(test, feature = "headless-tests"))]
    pub(crate) fn screenshot_for_test(
        &self,
        id: u64,
    ) -> (WindowHandle<Root>, WeakEntity<ScreenshotView>, bool) {
        match &self.open[&id].surface {
            Surface::Screenshot(w, v) => (*w, v.clone(), self.open[&id].ready),
            _ => panic!("expected screenshot"),
        }
    }
    #[cfg(all(test, feature = "headless-tests"))]
    pub(crate) fn view_for_test(&self, id: u64) -> (WindowHandle<Root>, WeakEntity<PinView>) {
        match &self.open[&id].surface {
            Surface::View(w, v) => (*w, v.clone()),
            _ => panic!("expected inspectable pin"),
        }
    }
    pub fn new() -> anyhow::Result<(Self, async_channel::Receiver<PinCommand>)> {
        let (commands, receiver) = async_channel::unbounded();
        Ok((
            Self {
                next_id: 1,
                open: HashMap::new(),
                closed: vec![],
                commands,
                owner: None,
                undo_feedback: None,
            },
            receiver,
        ))
    }
    fn allocate_id(&mut self) -> anyhow::Result<PinHandle> {
        let next = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("贴图编号已用尽"))?;
        let handle = PinHandle { id: self.next_id };
        self.next_id = next;
        Ok(handle)
    }
    pub fn open(
        snapshot: Arc<PinSnapshot>,
        placement: PinPlacement,
        owner: &Entity<Self>,
        cx: &mut App,
    ) -> anyhow::Result<PinHandle> {
        Self::open_with_ready(snapshot, placement, owner, cx, None)
    }
    fn open_with_ready(
        snapshot: Arc<PinSnapshot>,
        placement: PinPlacement,
        owner: &Entity<Self>,
        cx: &mut App,
        on_created: Option<Box<dyn FnOnce()>>,
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
        owner.update(cx, |this, _| this.owner = Some(owner.downgrade()));
        let admitted = owner
            .read(cx)
            .retained_bytes()
            .checked_add(snapshot.retained_bytes())
            .is_some_and(|bytes| bytes <= 256 * 1024 * 1024);
        if !admitted || owner.read(cx).open.len() >= 32 {
            anyhow::bail!("贴图已达到当前资源上限，请先关闭一些贴图");
        }
        let handle = owner.update(cx, |this, _| this.allocate_id())?;
        let cursor = match placement {
            PinPlacement::Pointer(p) | PinPlacement::Screenshot(p, _) => p,
        };
        let mut surface = None;
        let weak = owner.downgrade();
        let window = if let (PinContent::Image(image), PinPlacement::Screenshot(position, radius)) =
            (snapshot.content(), placement)
        {
            let (_, scale) = platform::work_area(position);
            let border = crate::screenshot_geometry::border_px((scale * 96.).round() as u32);
            let dimensions =
                crate::screenshot_geometry::window_size(image.width(), image.height(), 1., border);
            cx.open_window(
                WindowOptions {
                    color_policy: WindowColorPolicy::AutomaticHdr,
                    physical_bounds: Some(crate::screenshot::physical_bounds(position, dimensions)),
                    keep_physical_size_on_dpi_change: true,
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(position.0 as f32 / scale), px(position.1 as f32 / scale)),
                        size(
                            px(dimensions.0 as f32 / scale),
                            px(dimensions.1 as f32 / scale),
                        ),
                    ))),
                    kind: WindowKind::PopUp,
                    titlebar: None,
                    focus: false,
                    is_activatable: false,
                    is_resizable: false,
                    is_minimizable: false,
                    window_background: WindowBackgroundAppearance::Transparent,
                    inactive_frame_interval: None,
                    ..Default::default()
                },
                |window, cx| {
                    let pin = cx.new(|cx| {
                        ScreenshotView::new(
                            handle.id(),
                            snapshot.clone(),
                            weak,
                            position,
                            radius,
                            window,
                            cx,
                        )
                    });
                    surface = Some(Surface::Screenshot(
                        window
                            .window_handle()
                            .downcast::<Root>()
                            .expect("root handle"),
                        pin.downgrade(),
                    ));
                    cx.new(|cx| Root::new(pin, window, cx).bg(transparent_black()))
                },
            )?
        } else {
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
            cx.open_window(
                WindowOptions {
                    color_policy: WindowColorPolicy::AutomaticHdr,
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
                    surface = Some(Surface::View(
                        window
                            .window_handle()
                            .downcast::<Root>()
                            .expect("root handle"),
                        pin.downgrade(),
                    ));
                    cx.new(|cx| Root::new(pin, window, cx))
                },
            )?
        };
        let window_id = window.window_id();
        let weak = owner.downgrade();
        let closed = cx.on_window_closed(move |cx, closed| {
            if closed == window_id {
                let weak = weak.clone();
                cx.defer(move |cx| {
                    _ = weak.update(cx, |m, cx| m.released(handle.id(), cx));
                });
            }
        });
        let surface = surface.expect("constructed pin");
        let ready = matches!(surface, Surface::View(..));
        owner.update(cx, |this, _| {
            this.open.insert(
                handle.id(),
                OpenPin {
                    snapshot,
                    surface,
                    _closed: closed,
                    derived_bytes: 0,
                    ready,
                    on_created,
                },
            );
        });
        if ready {
            owner.update(cx, |this, _| {
                if let Some(callback) = this
                    .open
                    .get_mut(&handle.id())
                    .and_then(|p| p.on_created.take())
                {
                    callback();
                }
            });
        }
        Ok(handle)
    }
    /// Acknowledge the first successful image upload/paint, once. Failure or
    /// cancellation releases the source claim and never adds an empty undo entry.
    pub(crate) fn screenshot_ready(&mut self, id: u64, result: Result<(), String>, cx: &mut App) {
        if !self.open.get(&id).is_some_and(|p| !p.ready) {
            return;
        }
        match result {
            Ok(()) => {
                let pin = self.open.get_mut(&id).unwrap();
                pin.ready = true;
                if let Some(callback) = pin.on_created.take() {
                    callback();
                }
            }
            Err(error) => {
                if let Some(pin) = self.open.remove(&id) {
                    _ = pin.surface.window().update(cx, |_, w, _| w.remove_window());
                    eprintln!("[snapclip-pin] image preparation/paint failed id={id}: {error}");
                    crate::show_feedback(
                        format!("无法创建图片贴图：{error}"),
                        platform::cursor_position(),
                        cx,
                    );
                }
            }
        }
    }
    pub fn open_selection(
        selection: PinSelection,
        owner: &Entity<Self>,
        cx: &mut App,
        on_created: impl FnOnce() + 'static,
    ) -> anyhow::Result<PinHandle> {
        validate_selection(&selection)?;
        let region = selection.region();
        let image = selection.image().clone();
        Self::open_with_ready(
            Arc::new(PinSnapshot::new(PinContent::Image(image))),
            PinPlacement::Screenshot((region.left, region.top), selection.corner_radius()),
            owner,
            cx,
            Some(Box::new(on_created)),
        )
    }

    pub fn is_open(&self, id: u64) -> bool {
        self.open.contains_key(&id)
    }
    pub fn raise(&self, id: u64, cx: &mut App) -> bool {
        self.open.get(&id).is_some_and(|pin| {
            pin.surface
                .window()
                .update(cx, |_, w, cx| {
                    if let Surface::Screenshot(_, view) = &pin.surface {
                        if let Some(view) = view.upgrade() {
                            view.update(cx, |v, cx| {
                                v.set_visible(true, w);
                                cx.notify();
                            });
                        }
                    } else {
                        platform::visible(w, true);
                    }
                    platform::raise(w);
                })
                .is_ok()
        })
    }
    pub(crate) fn released(&mut self, id: u64, cx: &mut App) {
        if let Some(pin) = self.open.remove(&id) {
            if !pin.ready {
                return;
            }
            self.closed.push(pin.snapshot);
            while self.closed.len() > 8 || self.retained_bytes() > 256 * 1024 * 1024 {
                self.closed.remove(0);
            }
            if let Some(handle) = self.undo_feedback.take() {
                _ = handle.update(cx, |_, w, _| w.remove_window());
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
        // Remove pending state first: a queued paint receipt cannot resurrect it.
        if self.open.get(&id).is_some_and(|p| !p.ready) {
            if let Some(pin) = self.open.remove(&id) {
                _ = pin.surface.window().update(cx, |_, w, _| w.remove_window());
            }
        } else if let Some(pin) = self.open.get(&id) {
            _ = pin.surface.window().update(cx, |_, w, _| w.remove_window());
        }
    }
    pub fn close_all(&mut self, cx: &mut App) {
        for id in self.open.keys().copied().collect::<Vec<_>>() {
            self.close(id, cx);
        }
    }
    pub fn set_visible(&mut self, visible: bool, cx: &mut App) {
        for pin in self.open.values() {
            let window = pin.surface.window();
            match &pin.surface {
                Surface::Screenshot(_, view) => {
                    if let Some(view) = view.upgrade() {
                        _ = window.update(cx, |_, w, cx| {
                            view.update(cx, |v, cx| {
                                v.set_visible(visible, w);
                                cx.notify();
                            })
                        });
                    }
                }
                Surface::View(..) => {
                    _ = window.update(cx, |_, w, _| platform::visible(w, visible));
                }
            }
        }
    }
    pub(crate) fn show_text(&self, id: u64, cx: &mut App) {
        let Some(pin) = self.open.get(&id) else {
            return;
        };
        let Some(text) = pin.snapshot.text().filter(|t| !t.is_empty()) else {
            return;
        };
        let mut snapshot = PinSnapshot::new(PinContent::Document(crate::PinDocument::new(
            text.into(),
            crate::DocumentMode::Text,
        )));
        if let PinContent::Image(image) = pin.snapshot.content() {
            snapshot = snapshot.with_image(image.clone());
        }
        let owner = self.owner.clone();
        cx.defer(move |cx| {
            if let Some(owner) = owner.and_then(|w| w.upgrade())
                && let Err(error) = Self::open(
                    Arc::new(snapshot),
                    PinPlacement::Pointer(platform::cursor_position()),
                    &owner,
                    cx,
                )
            {
                crate::show_feedback(error.to_string(), platform::cursor_position(), cx);
            }
        });
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
        if let Some(pin) = self.open.get(&id) {
            match &pin.surface {
                Surface::Screenshot(..) => crate::show_feedback(
                    result.err().unwrap_or_else(|| "已复制图片".into()),
                    platform::cursor_position(),
                    cx,
                ),
                Surface::View(handle, view) => {
                    if let Some(view) = view.upgrade() {
                        _ = handle.update(cx, |_, w, cx| {
                            view.update(cx, |v, cx| v.copied(result, w, cx))
                        });
                    }
                }
            }
        }
    }
    fn retained_bytes(&self) -> usize {
        self.open
            .values()
            .map(|p| p.snapshot.retained_bytes().saturating_add(p.derived_bytes))
            .chain(self.closed.iter().map(|s| s.retained_bytes()))
            .fold(0, usize::saturating_add)
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn ids_start_at_one_and_both_window_kinds_share_the_same_sequence() {
        let (mut manager, _) = super::PinManager::new().unwrap();
        assert_eq!(manager.allocate_id().unwrap().id(), 1);
        assert_eq!(manager.allocate_id().unwrap().id(), 2);
        assert_eq!(manager.allocate_id().unwrap().id(), 3);
    }
    #[test]
    fn exhausted_ids_never_wrap_or_become_reusable() {
        let (mut manager, _) = super::PinManager::new().unwrap();
        manager.next_id = u64::MAX - 1;
        assert_eq!(manager.allocate_id().unwrap().id(), u64::MAX - 1);
        assert!(manager.allocate_id().is_err());
        assert!(manager.allocate_id().is_err());
        assert_eq!(manager.next_id, u64::MAX);
    }
    #[test]
    fn image_copy_commands_retain_the_exact_shared_original_source() {
        let (manager, commands) = super::PinManager::new().unwrap();
        let image = crate::PinImage::new(1, 1, vec![17, 31, 91, 127]).unwrap();
        manager.copy(7, super::CopyTarget::Pixels(image.clone()));
        let super::PinCommand::Copy {
            id: 7,
            target: super::CopyTarget::Pixels(copy),
        } = commands.try_recv().unwrap()
        else {
            panic!("wrong copy intent")
        };
        assert!(std::sync::Arc::ptr_eq(image.pixels(), copy.pixels()));
        assert_eq!(copy.pixels().as_bytes(), [91, 31, 17, 127]);
    }
    #[test]
    fn valid_selections_keep_negative_origins_and_source_radius() {
        let selection = crate::PinSelection::new(
            snapclip_model::geometry::Rect::new(-11, -22, -10, -21),
            crate::PinImage::new(1, 1, vec![0; 4]).unwrap(),
        )
        .radius(12);
        super::validate_selection(&selection).unwrap();
        assert_eq!(
            (selection.region().left, selection.region().top),
            (-11, -22)
        );
        assert_eq!(selection.corner_radius(), 12);
    }
    #[test]
    fn empty_selection_is_rejected_before_a_window_or_id_is_allocated() {
        let selection = crate::PinSelection::new(
            snapclip_model::geometry::Rect::new(1, 1, 1, 2),
            crate::PinImage::new(1, 1, vec![0; 4]).unwrap(),
        );
        assert!(super::validate_selection(&selection).is_err());
    }
    #[test]
    fn mismatched_selection_dimensions_never_reinterpret_original_pixels() {
        let selection = crate::PinSelection::new(
            snapclip_model::geometry::Rect::new(0, 0, 2, 2),
            crate::PinImage::new(1, 1, vec![0; 4]).unwrap(),
        );
        assert!(super::validate_selection(&selection).is_err());
        assert_eq!(selection.image().pixels().as_bytes().len(), 4);
    }
    #[cfg(feature = "headless-tests")]
    #[gpui_kit::test]
    fn hdr_screenshot_is_admitted_without_consuming_its_source_at_window_creation(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use super::*;
        use snapclip_model::pixels::{
            AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding, PixelSource,
        };
        use std::{cell::Cell, rc::Rc};
        let committed = Rc::new(Cell::new(0));
        cx.update(|cx| {
            gpui_kit::init(cx);
            yue_component::init(
                cx,
                yue_component::YueTheme::new(
                    yue_component::ThemeMode::Light,
                    yue_component::Accent::Azure,
                ),
            )
            .unwrap();
            let descriptor = PixelDescriptor::new(
                snapclip_model::PixelFormat::Rgba16Float,
                PixelEncoding::ScRgb,
                AlphaMode::Straight,
            )
            .unwrap();
            let pixels = Arc::new(
                PixelBuffer::from_tight(
                    1,
                    1,
                    descriptor,
                    [-0.25_f32, 12.5, 0.5, 0.5]
                        .into_iter()
                        .flat_map(|v| half::f16::from_f32(v).to_le_bytes())
                        .collect(),
                )
                .unwrap(),
            );
            let image =
                crate::PinImage::from_pixels(PixelSource::new(descriptor, 91), pixels).unwrap();
            let (manager, _) = PinManager::new().unwrap();
            let owner = cx.new(|_| manager);
            let commit = committed.clone();
            let result = PinManager::open_selection(
                PinSelection::new(
                    snapclip_model::geometry::Rect::new(-10, -20, -9, -19),
                    image,
                ),
                &owner,
                cx,
                move || commit.set(commit.get() + 1),
            );
            assert!(
                result.is_ok(),
                "HDR original-position admission failed: {result:?}"
            );
            assert_eq!(
                committed.get(),
                0,
                "an empty HWND must not consume a screenshot source"
            );
            owner.update(cx, |manager, cx| manager.close_all(cx));
        });
        cx.run_until_parked();
        assert_eq!(
            committed.get(),
            0,
            "cancelled creation must release its claim without consuming it"
        );
    }
    #[test]
    fn clipboard_images_fit_the_monitor_without_mutating_original_pixels() {
        assert_eq!(super::initial_image_zoom((100, 100), (1920, 1080)), 1.);
        let zoom = super::initial_image_zoom((4000, 3000), (1920, 1080));
        assert!(4000. * zoom <= 1920. * 0.7);
        assert!(3000. * zoom <= 1080. * 0.7);
        assert!(super::initial_image_zoom((1, 24_000_000), (1920, 1080)) > 0.);
    }
}
