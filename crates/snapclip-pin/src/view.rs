use crate::{CopyTarget, DocumentMode, FileKind, PinContent, PinManager, PinSnapshot, platform};
use gpui_kit::{
    base::{TextView, TextViewState},
    component::ActiveTheme,
    component::table::{DataTable, TableState},
    prelude::*,
    *,
};
use std::{sync::Arc, time::Duration};
use yue_component::{
    ButtonSize, ButtonVariant, ImagePreviewDocument, ImagePreviewEvent, ImagePreviewLoad,
    YueButton, YueContextMenu, YueDropdownMenu, YueImagePreview, YueImagePreviewState, YueMenuItem,
    current_theme, outline,
};

fn common_parent(files: &[crate::PinFile]) -> Option<String> {
    let mut parent = std::path::Path::new(files.first()?.path())
        .parent()?
        .to_path_buf();
    while !files.iter().all(|file| {
        std::path::Path::new(file.path())
            .parent()
            .is_some_and(|path| path.starts_with(&parent))
    }) {
        if !parent.pop() {
            return None;
        }
    }
    (!parent.as_os_str().is_empty()).then(|| parent.to_string_lossy().into_owned())
}

actions!(
    snapclip_pin,
    [
        ClosePin,
        CopyPin,
        CopyWholePin,
        SelectAllPin,
        OpenPinItem,
        NextPinItem,
        PreviousPinItem,
        UndoPin
    ]
);

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", ClosePin, Some("SnapClipPin")),
        KeyBinding::new("ctrl-c", CopyPin, Some("SnapClipPin")),
        KeyBinding::new("ctrl-shift-c", CopyWholePin, Some("SnapClipPin")),
        KeyBinding::new("ctrl-a", SelectAllPin, Some("SnapClipPin")),
        KeyBinding::new("enter", OpenPinItem, Some("SnapClipPin")),
        KeyBinding::new("down", NextPinItem, Some("SnapClipPin")),
        KeyBinding::new("up", PreviousPinItem, Some("SnapClipPin")),
        KeyBinding::new("ctrl-shift-t", UndoPin, Some("SnapClipPin")),
    ]);
}
struct Representation {
    label: &'static str,
    state: Entity<TextViewState>,
    copy: String,
}
pub(crate) struct PinView {
    id: u64,
    snapshot: Arc<PinSnapshot>,
    owner: WeakEntity<PinManager>,
    focus: FocusHandle,
    representations: Vec<Representation>,
    table: Option<Entity<TableState<crate::table::PinTableDelegate>>>,
    current: usize,
    hovered: bool,
    first_show: bool,
    menu_open: bool,
    topmost: bool,
    collapsed: bool,
    expanded_size: Size<Pixels>,
    copied: bool,
    status: Option<String>,
    selected: usize,
    scroll: UniformListScrollHandle,
    thumbnail: Option<Arc<Image>>,
    thumbnail_bytes: usize,
    full_preview_bytes: usize,
    image_mode: bool,
    image_pixels: Option<crate::PinImage>,
    image_preview: Entity<YueImagePreviewState>,
    _preview_subscription: Subscription,
    preview_task: Option<Task<()>>,
    _thumbnail_task: Option<Task<()>>,
    _first_show: Task<()>,
    feedback: Option<Task<()>>,
}
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn text_html(text: &str, code: bool) -> String {
    if code {
        format!("<pre><code>{}</code></pre>", escape(text))
    } else {
        // A span prevents HTML's special removal of a newline immediately after
        // <pre>. This preserves real whitespace rather than substituting NBSPs.
        format!("<pre><span>{}</span></pre>", escape(text))
    }
}

impl PinView {
    fn toggle_collapsed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.collapsed = !self.collapsed;
        if self.collapsed {
            self.expanded_size = window.viewport_size();
            window.resize(size(
                self.expanded_size.width,
                rems(3.).to_pixels(window.rem_size()),
            ));
        } else {
            window.resize(self.expanded_size);
        }
        cx.notify();
    }

    fn copy_path(&self, ix: usize, cx: &mut App) {
        if let PinContent::Files(files) = self.snapshot.content() {
            _ = self.owner.update(cx, |manager, _| {
                manager.copy(self.id, CopyTarget::Text(files[ix].path().into()))
            });
        }
    }
    fn select_representation(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.current = ix;
        self.image_mode = false;
        let focus = if let Some(table) = self.table.as_ref().filter(|_| ix == 0) {
            table.read(cx).focus_handle(cx)
        } else {
            self.representations[ix]
                .state
                .read(cx)
                .focus_handle()
                .clone()
        };
        window.focus(&focus, cx);
        cx.notify();
    }
    fn submit_image(&self, source: ImagePreviewLoad, window: &mut Window, cx: &mut Context<Self>) {
        let document =
            ImagePreviewDocument::new(format!("pin-image-{}", self.id), 0).with_source(source);
        self.image_preview.update(cx, |state, cx| {
            state
                .set_document(document, window, cx)
                .expect("stable pin image identity");
        });
    }
    fn view_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.image_mode = true;
        self.image_preview
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
        if self.image_pixels.is_some() || self.preview_task.is_some() {
            cx.notify();
            return;
        }
        self.load_image(window, cx);
    }
    fn load_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preview_task.is_some() {
            return;
        }
        let file = match self.snapshot.content() {
            PinContent::Files(files) if files.len() == 1 && files[0].kind() == FileKind::Image => {
                Some(files[0].path().to_string())
            }
            _ => None,
        };
        // A render/upload failure can be retried with the same decoded source.
        // Give the component a new RenderImage identity instead of returning
        // early merely because the original pixels are still available.
        let pixels = self
            .image_pixels
            .clone()
            .or_else(|| match self.snapshot.content() {
                PinContent::Image(image) => Some(image.clone()),
                _ => self.snapshot.image().cloned(),
            });
        self.submit_image(ImagePreviewLoad::Loading, window, cx);
        self.preview_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let pixels = match (pixels, file) {
                        (Some(pixels), _) => pixels,
                        (_, Some(file)) => crate::image_preview::load(&file)?,
                        _ => return Err("没有可用图片".into()),
                    };
                    let image = crate::image_preview::render(&pixels)?;
                    Ok::<_, String>((pixels, image))
                })
                .await;
            _ = this.update_in(cx, |this, window, cx| {
                this.preview_task = None;
                let source = match result {
                    Ok((pixels, image)) => {
                        // Snapshot pixels are already accounted for. A file preview additionally
                        // retains its decoded RGBA copy; both retain the BGRA render buffer.
                        let bytes = if matches!(this.snapshot.content(), PinContent::Image(_)) {
                            // Primary images already reserve a display buffer in retained_bytes().
                            0
                        } else {
                            pixels.pixels().as_bytes().len().saturating_mul(
                                if this.snapshot.image().is_some() {
                                    1
                                } else {
                                    2
                                },
                            )
                        };
                        if this
                            .owner
                            .update(cx, |manager, _| {
                                manager.retain_preview(
                                    this.id,
                                    bytes.saturating_add(this.thumbnail_bytes),
                                )
                            })
                            .unwrap_or(false)
                        {
                            this.full_preview_bytes = bytes;
                            this.image_pixels = Some(pixels);
                            ImagePreviewLoad::Ready(image)
                        } else {
                            ImagePreviewLoad::Failed(
                                "图片预览超过当前资源上限，请先关闭一些贴图".into(),
                            )
                        }
                    }
                    Err(message) => ImagePreviewLoad::Failed(message.into()),
                };
                this.submit_image(source, window, cx);
                cx.notify();
            });
        }));
        cx.notify();
    }
    pub(crate) fn new(
        id: u64,
        snapshot: Arc<PinSnapshot>,
        owner: WeakEntity<PinManager>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        window.set_window_title("SnapClip 贴图");
        let focus = cx.focus_handle().tab_stop(true);
        window.focus(&focus, cx); // local focus path, WindowOptions.focus=false prevents initial OS activation
        let mut representations = vec![];
        let table = if let PinContent::Document(doc) = snapshot.content() {
            doc.table().cloned().map(|table| {
                cx.new(|cx| {
                    TableState::new(
                        crate::table::PinTableDelegate::new(table, window),
                        window,
                        cx,
                    )
                    .cell_selectable(true)
                    .row_header(false)
                    .loop_selection(false)
                })
            })
        } else {
            None
        };
        if let PinContent::Document(doc) = snapshot.content() {
            if let Some(html) = doc.html() {
                representations.push(Representation {
                    label: if doc.mode() == DocumentMode::Table {
                        "表格"
                    } else {
                        "排版"
                    },
                    state: cx.new(|cx| TextViewState::html(html, cx).scrollable(true)),
                    copy: doc.text().into(),
                });
            } else if let Some(table) = doc.table() {
                let mut html = String::from("<table>");
                for (ix, row) in table.rows().iter().enumerate() {
                    html.push_str("<tr>");
                    let tag = if ix == 0 && table.has_header() {
                        "th"
                    } else {
                        "td"
                    };
                    for cell in row {
                        html.push_str(&format!("<{tag}>{}</{tag}>", escape(cell)));
                    }
                    html.push_str("</tr>");
                }
                html.push_str("</table>");
                representations.push(Representation {
                    label: "表格",
                    state: cx.new(|cx| TextViewState::html(&html, cx).scrollable(true)),
                    copy: doc.text().into(),
                });
            }
            let code = doc.mode() == DocumentMode::Code;
            let markup = if doc.mode() == DocumentMode::Link {
                format!(
                    "<p><a href=\"{}\">{}</a></p>",
                    escape(doc.text().trim()),
                    escape(doc.text())
                )
            } else {
                text_html(doc.text(), code)
            };
            representations.push(Representation {
                label: if code { "代码" } else { "文字" },
                state: cx.new(|cx| TextViewState::html(&markup, cx).scrollable(true)),
                copy: doc.text().into(),
            });
            if matches!(doc.mode(), DocumentMode::Text | DocumentMode::Code)
                && !doc.text().trim().is_empty()
            {
                representations.push(Representation {
                    label: if code { "文字" } else { "代码" },
                    state: cx.new(|cx| {
                        TextViewState::html(&text_html(doc.text(), !code), cx).scrollable(true)
                    }),
                    copy: doc.text().into(),
                });
            }
            for (name, source) in snapshot.sources() {
                representations.push(Representation {
                    label: if name == "HTML" {
                        "HTML 原文"
                    } else {
                        "RTF 原文"
                    },
                    state: cx.new(|cx| {
                        TextViewState::html(&text_html(&String::from_utf8_lossy(source), true), cx)
                            .scrollable(true)
                    }),
                    copy: String::from_utf8_lossy(source).into_owned(),
                });
            }
        }
        if matches!(snapshot.content(), PinContent::Image(_))
            && let Some(text) = snapshot.text().filter(|text| !text.is_empty())
        {
            representations.push(Representation {
                label: "文字",
                state: cx
                    .new(|cx| TextViewState::html(&text_html(text, false), cx).scrollable(true)),
                copy: text.to_string(),
            });
        }
        let first = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            _ = this.update(cx, |this, cx| {
                this.first_show = false;
                cx.notify();
            });
        });
        let thumbnail_task = if let PinContent::Files(files) = snapshot.content() {
            files
                .first()
                .filter(|file| {
                    files.len() == 1
                        && file.kind() == FileKind::Image
                        && !file.path().starts_with("\\\\")
                        && file.unavailable_reason().is_none()
                })
                .map(|file| {
                    let path = file.path().to_string();
                    cx.spawn(async move |this, cx| {
                        let preview = cx
                            .background_spawn(async move {
                                let image = crate::image_preview::load(&path).ok()?;
                                crate::image_preview::encode(&image, true).ok()
                            })
                            .await;
                        if let Some(preview) = preview {
                            let _ = this.update(cx, |this, cx| {
                                let bytes = preview.len().saturating_add(768 * 384 * 4);
                                if this
                                    .owner
                                    .update(cx, |manager, _| {
                                        manager.retain_preview(
                                            this.id,
                                            bytes.saturating_add(this.full_preview_bytes),
                                        )
                                    })
                                    .unwrap_or(false)
                                {
                                    this.thumbnail_bytes = bytes;
                                    this.thumbnail = Some(Arc::new(Image::from_bytes(
                                        ImageFormat::Png,
                                        preview,
                                    )));
                                }
                                cx.notify();
                            });
                        }
                    })
                })
        } else {
            None
        };
        let image_preview = cx.new(|cx| YueImagePreviewState::new("pin-image-preview", window, cx));
        let preview_subscription = cx.subscribe_in(
            &image_preview,
            window,
            |this, _, event: &ImagePreviewEvent, window, cx| {
                if matches!(
                    event,
                    ImagePreviewEvent::RetryRequested { result: false, .. }
                ) {
                    this.load_image(window, cx);
                }
            },
        );
        let starts_as_image = matches!(snapshot.content(), PinContent::Image(_));
        let status = snapshot.notice().map(str::to_owned);
        let mut view = Self {
            id,
            snapshot,
            owner,
            focus,
            representations,
            table,
            current: 0,
            hovered: false,
            first_show: true,
            menu_open: false,
            topmost: true,
            collapsed: false,
            expanded_size: window.viewport_size(),
            copied: false,
            status,
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            thumbnail: None,
            thumbnail_bytes: 0,
            full_preview_bytes: 0,
            image_mode: false,
            image_pixels: None,
            image_preview,
            _preview_subscription: preview_subscription,
            preview_task: None,
            _thumbnail_task: thumbnail_task,
            _first_show: first,
            feedback: None,
        };
        if starts_as_image {
            view.view_image(window, cx);
        }
        view
    }
    fn copy(&self, whole: bool, cx: &mut App) {
        let target = if whole {
            CopyTarget::Content(self.snapshot.clone())
        } else if self.image_mode && self.image_pixels.is_some() {
            CopyTarget::Pixels(self.image_pixels.as_ref().unwrap().clone())
        } else if let Some(rep) = self.representations.get(self.current) {
            let text = if let Some(table) = self.table.as_ref().filter(|_| self.current == 0) {
                let state = table.read(cx);
                // Some("") is a real cell selection. TextView selection belongs
                // to a different representation and must not override the table.
                state
                    .delegate()
                    .selected_text(state)
                    .unwrap_or_else(|| rep.copy.clone())
            } else {
                let state = rep.state.read(cx);
                let selection = state.selected_text();
                if selection.is_empty() || state.is_all_selected() {
                    rep.copy.clone()
                } else {
                    selection
                }
            };
            CopyTarget::Text(text)
        } else if let PinContent::Files(files) = self.snapshot.content() {
            CopyTarget::Files(vec![files[self.selected].path().into()])
        } else {
            CopyTarget::Content(self.snapshot.clone())
        };
        _ = self
            .owner
            .update(cx, |manager, _| manager.copy(self.id, target));
    }
    pub(crate) fn copied(
        &mut self,
        result: Result<(), String>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copied = result.is_ok();
        self.status = Some(result.err().unwrap_or_else(|| "已复制".into()));
        cx.notify();
        self.feedback = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            _ = this.update(cx, |this, cx| {
                this.copied = false;
                this.status = None;
                cx.notify();
            });
        }));
    }
    fn open_item(&mut self, reveal: bool, cx: &mut Context<Self>) {
        let target = match self.snapshot.content() {
            PinContent::Files(files) => files.get(self.selected).map(|f| f.path().to_string()),
            PinContent::Document(doc)
                if matches!(doc.mode(), DocumentMode::Link | DocumentMode::Path) =>
            {
                Some(doc.text().trim().into())
            }
            _ => None,
        };
        if let Some(target) = target
            && let Err(error) = platform::open(&target, reveal)
        {
            self.status = Some(error);
            cx.notify();
        }
    }
    fn file_menu(&self, ix: usize, cx: &Context<Self>) -> Vec<YueMenuItem> {
        let owner = cx.entity().downgrade();
        let owner2 = owner.clone();
        let owner3 = owner.clone();
        vec![
            YueMenuItem::new("打开…").on_click(move |_, _, cx| {
                _ = owner.update(cx, |this, cx| {
                    this.selected = ix;
                    this.open_item(false, cx);
                });
            }),
            YueMenuItem::new("复制路径").on_click(move |_, _, cx| {
                _ = owner2.update(cx, |this, cx| {
                    this.copy_path(ix, cx);
                });
            }),
            YueMenuItem::new("打开所在位置…").on_click(move |_, _, cx| {
                _ = owner3.update(cx, |this, cx| {
                    this.selected = ix;
                    this.open_item(true, cx);
                });
            }),
        ]
    }
    fn menu(&self, cx: &Context<Self>) -> Vec<YueMenuItem> {
        let mut items = vec![];
        let image = self
            .image_pixels
            .as_ref()
            .or_else(|| match self.snapshot.content() {
                PinContent::Image(image) => Some(image),
                _ => self.snapshot.image(),
            });
        if let Some(image) = image.filter(|image| image.is_hdr()) {
            let image = image.clone();
            let owner = self.owner.clone();
            let id = self.id;
            items.push(
                YueMenuItem::new("复制 HDR 图片（PNG）").on_click(move |_, _, cx| {
                    _ = owner.update(cx, |manager, _| {
                        manager.copy(id, CopyTarget::HdrPixels(image.clone()))
                    });
                }),
            );
        }
        let owner = cx.entity().downgrade();
        items.push(
            YueMenuItem::new("复制全部内容")
                .hotkey("Ctrl+Shift+C")
                .on_click(move |_, _, cx| {
                    _ = owner.update(cx, |this, cx| this.copy(true, cx));
                }),
        );
        for (ix, rep) in self.representations.iter().enumerate() {
            let owner = cx.entity().downgrade();
            items.push(
                YueMenuItem::new(format!("查看{}", rep.label))
                    .radio(ix == self.current && !self.image_mode)
                    .on_click(move |_, window, cx| {
                        _ = owner.update(cx, |this, cx| {
                            this.select_representation(ix, window, cx);
                        });
                    }),
            );
        }
        let is_image_file = matches!(self.snapshot.content(), PinContent::Files(files) if files.len()==1 && files[0].kind()==FileKind::Image);
        if matches!(self.snapshot.content(), PinContent::Image(_))
            || self.snapshot.image().is_some()
            || is_image_file
        {
            let owner = cx.entity().downgrade();
            items.push(
                YueMenuItem::new(if is_image_file {
                    "作为图片贴图"
                } else {
                    "查看图片"
                })
                .id("pin-view-image")
                .radio(self.image_mode)
                .on_click(move |_, window, cx| {
                    _ = owner.update(cx, |this, cx| this.view_image(window, cx));
                }),
            );
        }
        if is_image_file && self.image_mode {
            let owner = cx.entity().downgrade();
            items.push(
                YueMenuItem::new("恢复文件卡片")
                    .id("pin-restore-file")
                    .on_click(move |_, window, cx| {
                        _ = owner.update(cx, |this, cx| {
                            this.image_mode = false;
                            this.focus.focus(window, cx);
                            cx.notify();
                        });
                    }),
            );
        }
        if self.image_mode && self.image_pixels.is_some() {
            let owner = cx.entity().downgrade();
            items.push(YueMenuItem::new("复制图片").id("pin-copy-image").on_click(
                move |_, _, cx| {
                    _ = owner.update(cx, |this, cx| this.copy(false, cx));
                },
            ));
        }
        if matches!(self.snapshot.content(), PinContent::Files(_)) {
            items.extend(self.file_menu(self.selected, cx));
        }
        let owner = cx.entity().downgrade();
        items.push(
            YueMenuItem::new(if self.collapsed {
                "展开"
            } else {
                "收起为标题条"
            })
            .on_click(move |_, window, cx| {
                _ = owner.update(cx, |this, cx| {
                    this.toggle_collapsed(window, cx);
                });
            }),
        );
        items
    }
    fn file_row(&self, ix: usize, window: &mut Window, cx: &Context<Self>) -> AnyElement {
        let PinContent::Files(files) = self.snapshot.content() else {
            return div().into_any_element();
        };
        let file = &files[ix];
        let theme = current_theme(cx);
        let icon = match file.kind() {
            FileKind::Folder => yue_component::Icon::new(outline::Folder),
            FileKind::Image => yue_component::Icon::new(outline::Photo),
            _ => yue_component::Icon::new(outline::FileText),
        };
        let owner = cx.entity().downgrade();
        let selected = ix == self.selected;
        let copy_owner = cx.entity().downgrade();
        let reveal_owner = cx.entity().downgrade();
        let row = div()
            .id(("file", ix))
            .map(|s| {
                #[cfg(feature = "headless-tests")]
                let s = s.test_support();
                s
            })
            .role(Role::ListBoxOption)
            .aria_selected(selected)
            .group("pin-file-row")
            .aria_label(file.name().to_string())
            .flex()
            .items_center()
            .gap_3()
            .h(rems(3.5))
            .px_4()
            .min_w_0()
            .border_b_1()
            .border_color(cx.theme().border)
            .when(selected, |s| s.bg(cx.theme().accent))
            .on_click(move |event, _, cx| {
                _ = owner.update(cx, |this, cx| {
                    this.selected = ix;
                    if event.click_count() == 2 {
                        this.open_item(false, cx);
                    }
                    cx.notify();
                });
            })
            .child(icon.size(rems(1.5)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .text_ellipsis()
                            .child(file.name().to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .text_ellipsis()
                            .child(
                                file.unavailable_reason()
                                    .map(str::to_string)
                                    .unwrap_or_else(|| {
                                        file.bytes().map_or_else(
                                            || {
                                                if file.kind() == FileKind::Folder {
                                                    "文件夹".into()
                                                } else {
                                                    "文件".into()
                                                }
                                            },
                                            |n| format!("{:.1} KB", n as f64 / 1024.),
                                        )
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .gap_1()
                    .opacity(
                        if window.is_window_active() && self.focus.contains_focused(window, cx) {
                            1.
                        } else {
                            0.
                        },
                    )
                    .group_hover("pin-file-row", |s| s.opacity(1.))
                    .child(
                        YueButton::icon(("file-copy-path", ix), theme, outline::Copy, "复制路径")
                            .size(ButtonSize::Small)
                            .variant(ButtonVariant::Ghost)
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                _ = copy_owner.update(cx, |this, cx| this.copy_path(ix, cx));
                            }),
                    )
                    .child(
                        YueButton::icon(
                            ("file-reveal", ix),
                            theme,
                            outline::FolderOpen,
                            "打开所在位置",
                        )
                        .size(ButtonSize::Small)
                        .variant(ButtonVariant::Ghost)
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            _ = reveal_owner.update(cx, |this, cx| {
                                this.selected = ix;
                                this.open_item(true, cx);
                            });
                        }),
                    ),
            )
            .child(
                YueDropdownMenu::new(
                    ("file-menu", ix),
                    theme,
                    YueButton::icon(("file-more", ix), theme, outline::Dots, "文件操作")
                        .size(ButtonSize::Small)
                        .variant(ButtonVariant::Ghost),
                )
                .items(self.file_menu(ix, cx))
                .on_open_change(cx.listener(|this, open, _, cx| {
                    this.menu_open = *open;
                    cx.notify();
                })),
            );
        YueContextMenu::new(("file-context", ix), theme)
            .items(self.file_menu(ix, cx))
            .child(row)
            .into_any_element()
    }
}
impl Render for PinView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current_theme(cx);
        let show = self.first_show
            || self.hovered
            || self.collapsed
            || self.menu_open
            || (window.is_window_active() && self.focus.contains_focused(window, cx));
        let toolbar_opacity = gpui_kit::base::transition(
            "pin-toolbar-opacity",
            if show { 1.0_f32 } else { 0. },
            yue_component::motion::transition(
                theme,
                yue_component::MotionDuration::Interaction,
                yue_component::MotionEase::Fade,
            ),
            window,
            cx,
        );
        let title = match self.snapshot.content() {
            PinContent::Files(files) if files.len() == 1 => files[0].name().to_string(),
            PinContent::Files(files) => format!("{} 个项目", files.len()),
            PinContent::Document(doc) => match doc.mode() {
                DocumentMode::Text => "文本",
                DocumentMode::Rich => "富文本",
                DocumentMode::Table => "表格",
                DocumentMode::Code => "代码",
                DocumentMode::Link => "链接",
                DocumentMode::Path => "路径",
            }
            .into(),
            _ => "图片".into(),
        };
        let copy = if self.copied {
            YueButton::icon("pin-copy", theme, outline::Check, "已复制")
        } else {
            YueButton::icon("pin-copy", theme, outline::Copy, "复制")
        }
        .size(ButtonSize::Small)
        .variant(ButtonVariant::Ghost)
        .on_click(cx.listener(|this, _, _, cx| this.copy(true, cx)));
        let pin = YueButton::icon("pin-topmost", theme, outline::Pin, "始终置顶")
            .size(ButtonSize::Small)
            .variant(if self.topmost {
                ButtonVariant::Default
            } else {
                ButtonVariant::Ghost
            })
            .on_click(cx.listener(|this, _, window, cx| {
                this.topmost = !this.topmost;
                platform::topmost(window, this.topmost);
                cx.notify();
            }));
        let menu = YueDropdownMenu::new(
            "pin-more",
            theme,
            YueButton::icon("pin-more-button", theme, outline::Dots, "更多")
                .size(ButtonSize::Small)
                .variant(ButtonVariant::Ghost),
        )
        .items(self.menu(cx))
        .on_open_change(cx.listener(|this, open, _, cx| {
            this.menu_open = *open;
            cx.notify();
        }));
        let menu = if self.collapsed {
            YueButton::icon("pin-expand", theme, outline::ChevronDown, "展开贴图")
                .size(ButtonSize::Small)
                .variant(ButtonVariant::Ghost)
                .on_click(cx.listener(|this, _, window, cx| this.toggle_collapsed(window, cx)))
                .into_any_element()
        } else {
            menu.into_any_element()
        };
        let toolbar = div()
            .id("pin-toolbar")
            .map(|s| {
                #[cfg(feature = "headless-tests")]
                let s = s.test_support();
                s
            })
            .flex()
            .items_center()
            .gap_1()
            .h_10()
            .px_2()
            .flex_shrink_0()
            .opacity(toolbar_opacity)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .text_ellipsis()
                    .child(title)
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        platform::start_move(window);
                        cx.stop_propagation();
                    }),
            )
            .child(copy)
            .child(pin)
            .child(menu)
            .child(
                YueButton::icon("pin-close", theme, outline::X, "关闭")
                    .size(ButtonSize::Small)
                    .variant(ButtonVariant::Ghost)
                    .on_click(|_, window, _| window.remove_window()),
            );
        let mut body = div().flex_1().min_h_0().min_w_0().overflow_hidden();
        if !self.collapsed {
            body = if self.image_mode {
                div()
                    .size_full()
                    .px_2()
                    .pb_2()
                    .child(
                        YueImagePreview::new(&self.image_preview)
                            .theme(theme)
                            .show_modes(false),
                    )
                    .into_any_element()
            } else {
                match self.snapshot.content() {
                    PinContent::Document(doc) => {
                        let state = &self.representations[self.current].state;
                        let mut content = div()
                            .flex()
                            .flex_col()
                            .size_full()
                            .min_h_0()
                            .px_4()
                            .pb_4()
                            .gap_2();
                        if let Some(notice) = doc.notice() {
                            content = content.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(notice.to_string()),
                            );
                        }
                        if doc.text().trim().is_empty() {
                            content = content.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("空白文本（复制保留原始空格、Tab 和换行）"),
                            );
                        }
                        if matches!(doc.mode(), DocumentMode::Link | DocumentMode::Path) {
                            content = content.child(
                                YueButton::text(
                                    "pin-open-resource",
                                    theme,
                                    if doc.mode() == DocumentMode::Link {
                                        "打开链接…"
                                    } else {
                                        "打开路径…"
                                    },
                                )
                                .size(ButtonSize::Small)
                                .variant(ButtonVariant::Text)
                                .on_click(cx.listener(|this, _, _, cx| this.open_item(false, cx))),
                            );
                        }
                        content = content.child(
                            div()
                                .id("pin-document-content")
                                .map(|s| {
                                    #[cfg(feature = "headless-tests")]
                                    let s = s.test_support();
                                    s
                                })
                                .flex_1()
                                .min_h_0()
                                .w_full()
                                .line_height(relative(1.7))
                                .child(
                                    if let Some(table) =
                                        self.table.as_ref().filter(|_| self.current == 0)
                                    {
                                        let has_header = table.read(cx).delegate().has_header();
                                        DataTable::new(table)
                                            .when(!has_header, |s| s.header_row_height(px(0.)))
                                            .into_any_element()
                                    } else {
                                        TextView::new(state)
                                            .scrollable(true)
                                            .size_full()
                                            .into_any_element()
                                    },
                                ),
                        );
                        content
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!(
                                        "{} 个字符 · {} 行",
                                        doc.text().chars().count(),
                                        doc.text().lines().count().max(1)
                                    )),
                            )
                            .into_any_element()
                    }
                    PinContent::Image(_) if !self.representations.is_empty() => div()
                        .size_full()
                        .px_4()
                        .pb_4()
                        .child(
                            TextView::new(&self.representations[self.current].state)
                                .scrollable(true)
                                .size_full(),
                        )
                        .into_any_element(),
                    PinContent::Files(files) if files.len() == 1 => {
                        let file = &files[0];
                        let row = self.file_row(0, window, cx);
                        div()
                            .size_full()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(row)
                            .child(
                                div()
                                    .px_4()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(file.path().to_string()),
                            )
                            .when_some(self.thumbnail.clone(), |s, image| {
                                s.child(
                                    div().px_4().pb_4().flex_1().min_h_0().child(
                                        img(image).size_full().object_fit(ObjectFit::Contain),
                                    ),
                                )
                            })
                            .into_any_element()
                    }
                    PinContent::Files(files) => {
                        let count = files.len();
                        div()
                            .size_full()
                            .flex()
                            .flex_col()
                            .min_h_0()
                            .when_some(common_parent(files), |s, path| {
                                s.child(
                                    div()
                                        .px_4()
                                        .pb_2()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .text_ellipsis()
                                        .child(path),
                                )
                            })
                            .child(
                                uniform_list(
                                    "pin-files",
                                    count,
                                    cx.processor(
                                        |this, range: std::ops::Range<usize>, window, cx| {
                                            range.map(|ix| this.file_row(ix, window, cx)).collect()
                                        },
                                    ),
                                )
                                .track_scroll(&self.scroll)
                                .flex_1()
                                .min_h_0(),
                            )
                            .into_any_element()
                    }
                    _ => div().into_any_element(),
                }
            }
            .map(|element| div().flex_1().min_h_0().child(element));
        }
        let has_image = matches!(self.snapshot.content(), PinContent::Image(_))
            || self.snapshot.image().is_some();
        let choices = if !self.representations.is_empty()
            && (self.representations.len() > 1 || has_image)
        {
            let row = div().flex().flex_wrap().gap_1().px_4().pb_2().children(
                self.representations.iter().enumerate().map(|(ix, rep)| {
                    YueButton::text(("pin-representation", ix), theme, rep.label)
                        .size(ButtonSize::Small)
                        .variant(if self.current == ix && !self.image_mode {
                            ButtonVariant::Default
                        } else {
                            ButtonVariant::Ghost
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_representation(ix, window, cx)
                        }))
                }),
            );
            Some(row.when(has_image, |row| {
                row.child(
                    YueButton::text("pin-representation-image", theme, "图片")
                        .size(ButtonSize::Small)
                        .variant(if self.image_mode {
                            ButtonVariant::Default
                        } else {
                            ButtonVariant::Ghost
                        })
                        .on_click(cx.listener(|this, _, window, cx| this.view_image(window, cx))),
                )
            }))
        } else {
            None
        };

        div()
            .id("pin-surface")
            .map(|s| {
                #[cfg(feature = "headless-tests")]
                let s = s.test_support();
                s
            })
            .key_context("SnapClipPin")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .text_sm()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius)
            .overflow_hidden()
            .on_hover(cx.listener(|this, hovered, _, cx| {
                this.hovered = *hovered;
                cx.notify();
            }))
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, window, cx| {
                if event.button != MouseButton::Left || this.menu_open {
                    return;
                }
                let edge = rems(0.5).to_pixels(window.rem_size());
                let size = window.viewport_size();
                if event.modifiers.alt
                    || event.position.x < edge
                    || event.position.y < edge
                    || event.position.x > size.width - edge
                    || event.position.y > size.height - edge
                {
                    platform::start_move(window);
                    cx.stop_propagation();
                }
            }))
            .on_action(|_: &ClosePin, window, _| window.remove_window())
            .capture_action(
                cx.listener(|this, _: &gpui_kit::base::actions::Cancel, window, cx| {
                    if !this.menu_open {
                        window.remove_window();
                        cx.stop_propagation();
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &CopyPin, _, cx| this.copy(false, cx)))
            // TextView owns its own Copy action. Intercept it at this feature boundary so
            // selection copies still receive the self-write marker and completion feedback.
            .capture_action(cx.listener(|this, _: &gpui_kit::base::input::Copy, _, cx| {
                this.copy(false, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &CopyWholePin, _, cx| this.copy(true, cx)))
            .on_action(cx.listener(|this, _: &SelectAllPin, window, cx| {
                // Kit's table supports a cell/row selection, not a full-table
                // range. Show a real visible text selection for Select All.
                if this.current == 0
                    && this.table.is_some()
                    && let Some(ix) = this
                        .representations
                        .iter()
                        .position(|rep| rep.label == "文字")
                {
                    this.select_representation(ix, window, cx);
                }
                if let Some(rep) = this.representations.get(this.current) {
                    rep.state.update(cx, |state, cx| state.select_all(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &OpenPinItem, _, cx| this.open_item(false, cx)))
            .on_action(cx.listener(|this, _: &NextPinItem, _, cx| {
                if let PinContent::Files(files) = this.snapshot.content() {
                    this.selected = (this.selected + 1).min(files.len() - 1);
                    this.scroll
                        .scroll_to_item(this.selected, ScrollStrategy::Top);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &PreviousPinItem, _, cx| {
                this.selected = this.selected.saturating_sub(1);
                this.scroll
                    .scroll_to_item(this.selected, ScrollStrategy::Top);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &UndoPin, _, cx| {
                if let Some(owner) = this.owner.upgrade() {
                    _ = PinManager::undo(&owner, platform::cursor_position(), cx);
                }
            }))
            .child(toolbar)
            .when(!self.collapsed, |s| {
                s.when_some(choices, |s, choices| s.child(choices))
            })
            .when(!self.collapsed, |s| s.child(body))
            .when_some(self.status.clone(), |s, status| {
                s.child(
                    div()
                        .id("pin-status")
                        .px_4()
                        .pb_2()
                        .text_xs()
                        .role(Role::Status)
                        .aria_label(status.clone())
                        .child(status),
                )
            })
    }
}

#[cfg(all(test, feature = "headless-tests"))]
#[path = "view_tests.rs"]
mod tests;

#[cfg(test)]
mod logic_tests {
    use super::common_parent;
    use crate::{FileKind, PinFile};

    #[test]
    fn collection_parent_uses_complete_windows_path_components() {
        for (paths, expected) in [
            (
                vec![r"C:\设计\甲.txt", r"C:\设计\素材\乙.png"],
                Some(r"C:\设计"),
            ),
            (vec![r"C:\A\甲.txt", r"C:\AB\乙.txt"], Some(r"C:\")),
            (vec![r"C:\甲.txt", r"D:\乙.txt"], None),
            (
                vec![r"\\server\share\设计\甲.txt", r"\\server\share\设计\乙.txt"],
                Some(r"\\server\share\设计"),
            ),
            (vec![r"\\server\one\甲.txt", r"\\server\two\乙.txt"], None),
            (vec![], None),
        ] {
            let files = paths
                .into_iter()
                .map(|p| PinFile::new(p.into(), "fixture".into(), FileKind::Document))
                .collect::<Vec<_>>();
            assert_eq!(common_parent(&files).as_deref(), expected);
        }
    }
}
