//! Yue save panel: the application owns export state; Yue owns control interaction.
mod directories;
mod model;
mod preview;
mod source;
#[cfg(test)]
mod tests;
mod view;

use crate::capture::pixel_export::{self, ColorExport};
use crate::save::{
    assemble,
    scale::{Size, SizeMode},
    service::{ConflictPolicy, SaveError, SaveOutcome},
};
use directories::DirectoryPresets;
use gpui_kit::*;
use model::Dimensions;
use preview::Preview;
use snapclip_capture::save_format::SaveFormat;
pub use source::Picture;
use std::{path::PathBuf, sync::Arc, time::Duration};
use yue_component::{
    ColorPickerEvent, ImagePreviewEvent, ImagePreviewLoad, InputEvent, InputState, SelectEvent,
    SelectMode, SelectOption, SelectValue, YueColorPickerState, YueSelectState,
};

pub const WIDTH: f32 = 1000.;
pub const HEIGHT: f32 = 720.;
pub const TITLE: &str = "保存图片 · SnapClip";
gpui_kit::actions!(
    yue_save_panel,
    [Save, Cancel, CopyImage, NextField, PreviousField]
);

/// Keep one editable capture panel. A panel committing a save remains alive until completion.
#[derive(Default)]
pub(crate) struct PanelWindows {
    current: Option<(
        WindowHandle<gpui_kit::component::Root>,
        WeakEntity<SavePanel>,
    )>,
}

impl PanelWindows {
    pub(crate) fn open(&mut self, picture: Picture, cx: &mut App) -> Result<()> {
        if let Some((previous, view)) = self.current.take()
            && view.upgrade().is_none_or(|view| !view.read(cx).busy)
        {
            _ = previous.update(cx, |_, window, _| window.remove_window());
        }
        let options = WindowOptions {
            color_policy: WindowColorPolicy::AutomaticHdr,
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(WIDTH), px(HEIGHT)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some(TITLE.into()),
                ..Default::default()
            }),
            window_min_size: Some(size(px(800.), px(520.))),
            ..Default::default()
        };
        let mut view = None;
        let handle = cx.open_window(options, |window, cx| {
            let panel = cx.new(|cx| SavePanel::new(picture, window, cx));
            view = Some(panel.downgrade());
            cx.new(|cx| gpui_kit::component::Root::new(panel, window, cx))
        })?;
        self.current = Some((handle, view.expect("panel was constructed")));
        Ok(())
    }
}

pub fn init(cx: &mut App, dark: bool) {
    if !cx.has_global::<DirectoryPresets>() {
        cx.set_global(DirectoryPresets::load(
            crate::app_data_dir().join("save-panel-paths.json"),
        ));
    }
    yue_component::init(
        cx,
        yue_component::YueTheme::new(
            if dark {
                yue_component::ThemeMode::Dark
            } else {
                yue_component::ThemeMode::Light
            },
            yue_component::Accent::Azure,
        ),
    )
    .expect("initialize Yue components");
    cx.bind_keys([
        KeyBinding::new("enter", Save, Some("YueSavePanel")),
        KeyBinding::new("ctrl-enter", Save, Some("YueSavePanel")),
        KeyBinding::new("escape", Cancel, Some("YueSavePanel")),
        KeyBinding::new("ctrl-shift-c", CopyImage, Some("YueSavePanel")),
        KeyBinding::new("tab", NextField, Some("YueSaveField > Input")),
        KeyBinding::new("shift-tab", PreviousField, Some("YueSaveField > Input")),
    ]);
}

/// One immutable export request, shared by background preview and save jobs.
#[derive(Clone)]
struct Export {
    color_export: ColorExport,
    picture: Arc<Picture>,
    output: Size,
    directory: String,
    name: String,
    format: SaveFormat,
    tier: usize,
    fill: [u8; 3],
    date: String,
    time: String,
    sequence: u32,
    conflict: ConflictPolicy,
    limit: Option<u64>,
}

impl Export {
    fn fit_under(&self, limit: u64) -> Option<Size> {
        crate::save::estimate::fit_under(self.output, limit, |size| {
            let mut candidate = self.clone();
            candidate.output = size;
            candidate
                .encoded_bytes()
                .ok()
                .map(|bytes| bytes.len() as u64)
        })
        .map(|(size, _)| size)
    }
    fn encoded_bytes(&self) -> Result<Vec<u8>, SaveError> {
        if self.format == SaveFormat::Avif {
            #[cfg(feature = "avif")]
            {
                return self.avif_bytes(snapclip_avif::StopToken::new(snapclip_avif::Unstoppable));
            }
            #[cfg(not(feature = "avif"))]
            return Err(SaveError::UnsupportedFormat(self.format));
        }
        if self.color_export == ColorExport::PreserveHdr {
            if self.format != SaveFormat::Png {
                return Err(SaveError::Io(
                    "保留 HDR 仅支持 PNG16；其他格式请明确选择转换为 SDR".into(),
                ));
            }
            let pixels = self
                .picture
                .resized_pixels(self.output)
                .map_err(SaveError::Io)?;
            return pixel_export::encode_pixels(&pixels, self.color_export).map_err(SaveError::Io);
        }
        assemble::encoded_bytes(&self.values(), &self.picture.frame(self.output))
    }
    #[cfg(feature = "avif")]
    fn avif_bytes(&self, stop: snapclip_avif::StopToken) -> Result<Vec<u8>, SaveError> {
        use snapclip_avif::Stop;
        if stop.check().is_err() {
            return Err(SaveError::Cancelled);
        }
        if self.color_export != ColorExport::PreserveHdr {
            return Err(SaveError::Io(
                "AVIF 当前仅支持保留 HDR，请选择 PNG 导出 SDR".into(),
            ));
        }
        // This byte buffer exists only for interactive round-trip previews.
        // Refuse before resizing/encoding; large file saves use the row source
        // and streaming commit path with their separate export geometry budget.
        if u64::from(self.output.width) * u64::from(self.output.height)
            > u64::from(snapclip_avif::MAX_PREVIEW_PIXELS)
        {
            return Err(SaveError::Io("AVIF 预览最多支持 2400 万像素".into()));
        }
        crate::save::caps::check(
            self.format,
            u64::from(self.output.width),
            u64::from(self.output.height),
        )
        .map_err(SaveError::Capability)?;
        let pixels = self
            .picture
            .resized_pixels(self.output)
            .map_err(SaveError::Io)?;
        let mut bytes = Vec::new();
        let mut bounded = crate::save::avif::Limited::new(
            &mut bytes,
            Some(snapclip_avif::MAX_INPUT_BYTES as u64),
        );
        crate::save::avif::write_pixels(&pixels, &mut bounded, stop).map_err(|e| {
            if e.kind() == std::io::ErrorKind::Interrupted {
                SaveError::Cancelled
            } else {
                SaveError::Io(e.to_string())
            }
        })?;
        Ok(bytes)
    }
    fn values(&self) -> assemble::PanelValues<'_> {
        source::values_with(
            &self.directory,
            &self.name,
            self.format,
            &self.date,
            &self.time,
            self.tier,
            self.fill,
            self.sequence,
            self.conflict,
        )
    }
    fn save(&self) -> Result<SaveOutcome, SaveError> {
        if self.format == SaveFormat::Avif {
            #[cfg(feature = "avif")]
            {
                if self.color_export != ColorExport::PreserveHdr {
                    return Err(SaveError::Io(
                        "AVIF 当前仅支持保留 HDR，请选择 PNG 导出 SDR".into(),
                    ));
                }
                let request = assemble::request_for(
                    &self.values(),
                    u64::from(self.output.width),
                    u64::from(self.output.height),
                )?;
                let pixels = self
                    .picture
                    .resized_pixels(self.output)
                    .map_err(SaveError::Io)?;
                return crate::save::avif::save_pixels(
                    &pixels,
                    &request,
                    self.limit,
                    snapclip_avif::StopToken::new(snapclip_avif::Unstoppable),
                );
            }
            #[cfg(not(feature = "avif"))]
            return Err(SaveError::UnsupportedFormat(self.format));
        }
        if self.color_export == ColorExport::PreserveHdr {
            let bytes = self.encoded_bytes()?;
            if self.limit.is_some_and(|limit| bytes.len() as u64 > limit) {
                return Err(SaveError::Io(
                    "HDR PNG 超出体积上限，请先自动缩小尺寸".into(),
                ));
            }
            let request = assemble::request_for(
                &self.values(),
                u64::from(self.output.width),
                u64::from(self.output.height),
            )?;
            return crate::save::service::commit_bytes(&request, &bytes);
        }
        source::save_within(&self.picture, &self.values(), self.output, self.limit).unwrap_or_else(
            || {
                Err(SaveError::Validation {
                    copy: crate::save::copy::ValidationCopy::NotEnoughSpace,
                    detail: Some("无法缩小到指定体积，请提高上限或更换格式".into()),
                })
            },
        )
    }
}

pub struct SavePanel {
    preserve_hdr: bool,
    picture: Arc<Picture>,
    dimensions: Dimensions,
    directory: Entity<InputState>,
    name: Entity<InputState>,
    width: Entity<InputState>,
    height: Entity<InputState>,
    limit_input: Entity<InputState>,
    format_select: Entity<YueSelectState>,
    fill_picker: Entity<YueColorPickerState>,
    format: SaveFormat,
    tier: usize,
    fill: [u8; 3],
    fill_choice: SharedString,
    after: [bool; 2],
    conflict: ConflictPolicy,
    limit_on: bool,
    preview: Preview,
    bytes: Option<u64>,
    measuring: bool,
    busy: bool,
    status: Option<String>,
    status_error: bool,
    dimension_errors: [Option<String>; 2],
    submitted: bool,
    saved_path: Option<PathBuf>,
    revision: u64,
    _original_task: Option<Task<()>>,
    adding_directory: bool,
    preview_task: Option<Task<()>>,
    #[cfg(feature = "avif")]
    preview_cancel: Option<preview::CancelOnDrop>,
    operation: Option<Task<()>>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl SavePanel {
    fn set_preserve_hdr(&mut self, preserve: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.picture.is_hdr() || self.preserve_hdr == preserve {
            return;
        }
        self.preserve_hdr = preserve;
        if preserve || self.format == SaveFormat::Avif {
            self.format = SaveFormat::Png;
            self.format_select.update(cx, |state, cx| {
                state
                    .set_value(SelectValue::Single(Some("png".into())), window, cx)
                    .expect("single PNG format");
            });
        }
        self.sync_color_formats(cx);
        self.schedule_preview(window, cx);
    }

    fn sync_color_formats(&mut self, cx: &mut Context<Self>) {
        if !self.picture.is_hdr() {
            return;
        }
        let preserve = self.preserve_hdr;
        self.format_select.update(cx, |state, cx| {
            let options = vec![
                SelectOption::new("png", ".PNG").with_description(if preserve {
                    "HDR · 16 位 PQ"
                } else {
                    "无损 · 支持透明"
                }),
                SelectOption::new("jpeg", ".JPG")
                    .with_description("需转换为 SDR")
                    .disabled(preserve),
                SelectOption::new("webp", ".WebP")
                    .with_description("需转换为 SDR")
                    .disabled(preserve),
            ];
            #[cfg(feature = "avif")]
            let options = {
                let mut options = options;
                options.push(
                    SelectOption::new("avif", ".AVIF")
                        .with_description("HDR · PQ10 · 编码较慢")
                        .disabled(!preserve),
                );
                options
            };
            state
                .set_options(options, cx)
                .expect("unique color-policy formats");
        });
    }
    pub fn new(picture: Picture, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let owner = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            owner.upgrade().is_none_or(|panel| !panel.read(cx).busy)
        });
        let dimensions = Dimensions::new(picture.size());
        let input = |value: String, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).default_value(value))
        };
        let (w, h) = dimensions.text();
        let directory = input(crate::save::dirs::desktop(), window, cx);
        let name = input(source::remembered_template(), window, cx);
        let width = input(w, window, cx);
        let height = input(h, window, cx);
        let limit_input = input("500".into(), window, cx);
        let format_select = cx.new(|cx| {
            let mut state = YueSelectState::new(SelectMode::Single, window, cx);
            state
                .set_options(
                    [
                        SelectOption::new("png", ".PNG").with_description("无损 · 支持透明"),
                        SelectOption::new("jpeg", ".JPG").with_description("有损 · 体积较小"),
                        SelectOption::new("webp", ".WebP").with_description("无损 · 支持透明"),
                    ],
                    cx,
                )
                .expect("unique formats");
            state
                .set_value(SelectValue::Single(Some("png".into())), window, cx)
                .expect("single PNG");
            state
        });
        let fill_picker =
            cx.new(|cx| YueColorPickerState::new(window, cx).default_value(rgba(0xffffffff)));
        let mut subscriptions = vec![cx.observe_global::<DirectoryPresets>(|_, cx| cx.notify())];
        for (field, axis) in [(&width, true), (&height, false)] {
            subscriptions.push(cx.subscribe_in(
                field,
                window,
                move |this, state, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        let draft = state.read(cx).value().to_string();
                        this.edit_dimension(axis, &draft, window, cx);
                    } else if matches!(event, InputEvent::PressEnter { .. }) {
                        this.save(window, cx);
                    }
                },
            ));
        }
        for field in [&directory, &name, &limit_input] {
            subscriptions.push(cx.subscribe_in(
                field,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        this.status = None;
                        this.saved_path = None;
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.save(window, cx),
                    _ => {}
                },
            ));
        }
        subscriptions.push(cx.subscribe_in(
            &format_select,
            window,
            |this, _, event: &SelectEvent, window, cx| {
                if let SelectEvent::Change(change) = event {
                    this.format = match change.after().keys().first().map(|s| s.as_ref()) {
                        Some("jpeg") => SaveFormat::Jpeg,
                        Some("webp") => SaveFormat::Webp,
                        Some("avif") => SaveFormat::Avif,
                        _ => SaveFormat::Png,
                    };
                    this.schedule_preview(window, cx);
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &fill_picker,
            window,
            |this, _, event: &ColorPickerEvent, window, cx| {
                if let ColorPickerEvent::Change(color) = event {
                    this.fill = [
                        (color.b * 255.).round() as u8,
                        (color.g * 255.).round() as u8,
                        (color.r * 255.).round() as u8,
                    ];
                    this.fill_choice = "custom".into();
                    this.schedule_preview(window, cx);
                }
            },
        ));
        let preview = Preview::new(window, cx);
        subscriptions.push(cx.subscribe_in(
            &preview.state,
            window,
            |this, _, event: &ImagePreviewEvent, window, cx| {
                if let ImagePreviewEvent::RetryRequested { result, .. } = event {
                    if *result {
                        this.schedule_preview(window, cx);
                    } else {
                        this.load_original(window, cx);
                    }
                }
            },
        ));
        let mut panel = Self {
            preserve_hdr: picture.is_hdr(),
            picture: Arc::new(picture),
            dimensions,
            directory,
            name,
            width,
            height,
            limit_input,
            format_select,
            fill_picker,
            format: SaveFormat::Png,
            tier: 0,
            fill: [255; 3],
            fill_choice: "white".into(),
            after: source::remembered_after(),
            conflict: ConflictPolicy::AutoRename,
            limit_on: false,
            preview,
            bytes: None,
            measuring: false,
            busy: false,
            status: None,
            status_error: false,
            dimension_errors: [None, None],
            submitted: false,
            saved_path: None,
            revision: 0,
            _original_task: None,
            adding_directory: false,
            preview_task: None,
            #[cfg(feature = "avif")]
            preview_cancel: None,
            operation: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        panel.focus.focus(window, cx);
        panel.sync_color_formats(cx);
        panel.load_original(window, cx);
        panel.schedule_preview(window, cx);
        panel
    }

    fn export(&self, cx: &App) -> Export {
        let (date, time) = source::now();
        Export {
            color_export: if self.preserve_hdr && self.picture.is_hdr() {
                ColorExport::PreserveHdr
            } else {
                ColorExport::Sdr {
                    source_peak_nits: self.picture.peak_nits(),
                }
            },
            picture: self.picture.clone(),
            output: self.dimensions.output,
            directory: self.directory.read(cx).value().to_string(),
            name: self.name.read(cx).value().to_string(),
            format: self.format,
            tier: self.tier,
            fill: self.fill,
            date,
            time,
            sequence: source::next_sequence(),
            conflict: self.conflict,
            limit: self
                .limit_on
                .then(|| {
                    self.limit_input
                        .read(cx)
                        .value()
                        .parse::<u64>()
                        .ok()
                        .and_then(|n| n.checked_mul(1024))
                })
                .flatten(),
        }
    }

    fn sync_dimensions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (w, h) = self.dimensions.text();
        self.width
            .update(cx, |state, cx| state.set_value(w, window, cx));
        self.height
            .update(cx, |state, cx| state.set_value(h, window, cx));
        self.dimension_errors = [None, None];
    }

    fn edit_dimension(
        &mut self,
        width: bool,
        draft: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = usize::from(!width);
        match self.dimensions.edit(width, draft) {
            Ok(()) => {
                self.dimension_errors[index] = None;
                if self.dimensions.locked {
                    let (w, h) = self.dimensions.text();
                    let other = if width { &self.height } else { &self.width };
                    other.update(cx, |state, cx| {
                        state.set_value(if width { h } else { w }, window, cx)
                    });
                    self.dimension_errors[1 - index] = None;
                }
                self.schedule_preview(window, cx);
            }
            Err(error) => {
                self.dimension_errors[index] = Some(error.into());
                // Cancel stale preview jobs as well as refusing save.
                self.revision += 1;
                self.preview_task = None;
                #[cfg(feature = "avif")]
                {
                    self.preview_cancel = None;
                }
                self.measuring = false;
                self.bytes = None;
                self.preview.encoded = ImagePreviewLoad::Empty;
                self.publish_preview(window, cx);
                cx.notify();
            }
        }
    }

    fn preset(&mut self, percent: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.dimensions.preset(percent);
        self.sync_dimensions(window, cx);
        self.schedule_preview(window, cx);
    }

    fn set_mode(&mut self, mode: SizeMode, window: &mut Window, cx: &mut Context<Self>) {
        self.dimensions.mode = mode;
        self.sync_dimensions(window, cx);
        cx.notify();
    }

    fn load_original(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picture = self.picture.clone();
        self.preview.original = ImagePreviewLoad::Loading;
        self.publish_preview(window, cx);
        self._original_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { preview::original(&picture) })
                .await;
            _ = this.update_in(cx, |panel, window, cx| {
                panel.preview.original = match result {
                    Ok(image) => ImagePreviewLoad::Ready(image),
                    Err(error) => ImagePreviewLoad::Failed(error.into()),
                };
                panel.publish_preview(window, cx);
                cx.notify();
            });
        }));
    }

    fn schedule_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(feature = "avif")]
        {
            self.preview_cancel = None;
        }
        self.revision += 1;
        self.status = None;
        self.saved_path = None;
        self.bytes = None;
        if self.dimension_errors.iter().any(Option::is_some) {
            self.preview_task = None;
            self.preview.encoded = ImagePreviewLoad::Empty;
            self.measuring = false;
            self.publish_preview(window, cx);
            cx.notify();
            return;
        }
        if let Err(error) = model::validate_preview_size(self.dimensions.output) {
            self.preview_task = None;
            self.preview.encoded = ImagePreviewLoad::Empty;
            self.measuring = false;
            self.dimension_errors[0] = Some(error.into());
            self.status_error = true;
            self.status = Some(error.into());
            self.publish_preview(window, cx);
            cx.notify();
            return;
        }
        self.measuring = true;
        let revision = self.revision;
        let export = self.export(cx);
        #[cfg(feature = "avif")]
        let avif_stop = {
            let cancel = preview::CancelOnDrop::new();
            let token = cancel.token();
            self.preview_cancel = Some(cancel);
            token
        };
        self.preview.encoded = ImagePreviewLoad::Loading;
        self.publish_preview(window, cx);
        self.preview_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(180))
                .await;
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(feature = "avif")]
                    if export.format == SaveFormat::Avif {
                        return preview::encode_avif(&export, avif_stop);
                    }
                    preview::encode(&export)
                })
                .await;
            _ = this.update_in(cx, |panel, window, cx| {
                if panel.revision != revision {
                    return;
                }
                panel.measuring = false;
                match result {
                    Ok(result) => {
                        panel.bytes = Some(result.bytes);
                        panel.preview.encoded = ImagePreviewLoad::Ready(result.encoded);
                    }
                    Err(error) => {
                        panel.status_error = true;
                        panel.preview.encoded = ImagePreviewLoad::Failed(error.clone().into());
                        panel.status = Some(error);
                    }
                }
                panel.publish_preview(window, cx);
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn validation(&self, cx: &App) -> Result<(), String> {
        if let Some(error) = self.dimension_errors.iter().flatten().next() {
            return Err(error.clone());
        }
        if self.limit_on && self.export(cx).limit.is_none_or(|limit| limit == 0) {
            return Err("体积上限必须是大于 0 的整数（KB）".into());
        }
        let export = self.export(cx);
        model::validate_preview_size(export.output).map_err(str::to_owned)?;
        crate::save::caps::check(
            export.format,
            u64::from(export.output.width),
            u64::from(export.output.height),
        )
        .map_err(|e| SaveError::Capability(e).message())?;
        let request = assemble::request_for(
            &export.values(),
            u64::from(export.output.width),
            u64::from(export.output.height),
        )
        .map_err(|e| e.message())?;
        crate::save::service::plain_target(&request).map_err(|e| e.message())?;
        Ok(())
    }

    fn set_busy(&mut self, busy: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = busy;
        self.format_select
            .update(cx, |state, cx| state.set_disabled(busy, window, cx));
    }

    fn composing(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        [
            &self.directory,
            &self.name,
            &self.width,
            &self.height,
            &self.limit_input,
        ]
        .into_iter()
        .any(|input| {
            input.update(cx, |input, cx| {
                input.marked_text_range(window, cx).is_some()
            })
        })
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy
            || self.composing(window, cx)
            || gpui_kit::base::GlobalState::is_in_deferred_context(cx)
        {
            return;
        }
        self.submitted = true;
        if let Err(error) = self.validation(cx) {
            self.status_error = true;
            self.status = Some(error);
            cx.notify();
            return;
        }
        if self.measuring || self.bytes.is_none() {
            self.status_error = true;
            self.status = Some("请等待有效的编码预览后再保存".into());
            cx.notify();
            return;
        }
        if self
            .export(cx)
            .limit
            .zip(self.bytes)
            .is_some_and(|(limit, bytes)| bytes > limit)
        {
            self.status_error = true;
            self.status = Some("超出体积上限，请先自动缩小尺寸".into());
            cx.notify();
            return;
        }
        let export = self.export(cx);
        let after = self.after;
        source::remember_template(&export.name);
        source::remember_after(after);
        self.set_busy(true, window, cx);
        self.status_error = false;
        self.status = Some("正在保存…".into());
        self.operation = Some(cx.spawn_in(window, async move |this, cx| {
            let template = export.name.clone();
            let (result, archive_error) = cx
                .background_executor()
                .spawn(async move {
                    let result = export.save();
                    let archive_error = result
                        .as_ref()
                        .ok()
                        .and_then(|_| export.picture.archive_original().err());
                    (result, archive_error)
                })
                .await;
            source::note_attempt(&result, &template);
            // Closing is disabled while saving, so completion and remembered sequence stay paired.
            if let Ok(outcome) = &result {
                source::run_after_save(after, &outcome.path);
            }
            _ = this.update_in(cx, |panel, window, cx| {
                panel.set_busy(false, window, cx);
                match result {
                    Ok(outcome) => {
                        panel.status_error = false;
                        panel.status = Some(format!(
                            "已保存 {} · {}",
                            outcome
                                .path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy(),
                            crate::save::estimate::format_bytes(outcome.bytes)
                        ));
                        panel.saved_path = Some(outcome.path);
                        if let Some(error) = archive_error {
                            panel.status_error = true;
                            panel.status = Some(format!("图片已保存，但历史原图未归档：{error}"));
                        }
                    }
                    Err(error) => {
                        panel.status_error = true;
                        panel.status = Some(error.message());
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let Some(error) = self.dimension_errors.iter().flatten().next() {
            self.status_error = true;
            self.status = Some(error.clone());
            cx.notify();
            return;
        }
        let picture = self.picture.clone();
        let output = self.dimensions.output;
        self.set_busy(true, window, cx);
        self.operation = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let pixels = crate::save::scale::scaled(&picture.frame(output));
                    crate::clipboard::SystemClipboard.copy_image(
                        &crate::clipboard::ClipboardImage {
                            width: output.width,
                            height: output.height,
                            pixels: &pixels,
                            channels: crate::clipboard::Channels::Bgra,
                            png: None,
                        },
                    )
                })
                .await;
            _ = this.update_in(cx, |panel, window, cx| {
                panel.set_busy(false, window, cx);
                panel.status_error = result.is_err();
                panel.status = Some(match result {
                    Ok(()) => "已复制图片".into(),
                    Err(error) => error,
                });
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn set_directory(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.directory
            .update(cx, |state, cx| state.set_value(path, window, cx));
        self.submitted = false;
        self.status = None;
        self.saved_path = None;
        cx.notify();
    }

    fn add_directory_preset(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.adding_directory || cx.global::<DirectoryPresets>().writing {
            return;
        }
        let path = self.directory.read(cx).value().to_string();
        let presets = cx.global::<DirectoryPresets>().clone();
        cx.global_mut::<DirectoryPresets>().writing = true;
        self.adding_directory = true;
        // The preference write belongs to the application, even if this panel closes.
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { presets.add(&path) })
                .await;
            let (message, error) = cx.update(|cx| {
                let error = result.is_err();
                cx.global_mut::<DirectoryPresets>().writing = false;
                let message = match result {
                    Ok(Some(presets)) => {
                        cx.set_global(presets);
                        "已添加路径预设".into()
                    }
                    Ok(None) => "该路径已在预设中".into(),
                    Err(error) => error,
                };
                (message, error)
            });
            _ = this.update(cx, |panel, cx| {
                panel.adding_directory = false;
                panel.status_error = error;
                panel.status = Some(message);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn choose_directory(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("选择保存文件夹".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = paths.await;
            _ = this.update_in(cx, |panel, window, cx| match result {
                Ok(Ok(Some(paths))) if !paths.is_empty() => {
                    let path = paths[0].to_string_lossy().into_owned();
                    panel.set_directory(path, window, cx);
                }
                Ok(Ok(_)) => {}
                error => {
                    panel.status_error = true;
                    panel.status = Some(format!("无法选择文件夹：{error:?}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

impl Focusable for SavePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
