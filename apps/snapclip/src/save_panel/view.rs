use super::*;
use gpui_kit::base::{Popover, TestSupportExt as _};
use gpui_kit::prelude::FluentBuilder;
use yue_component::{
    ButtonSize, ButtonVariant, InputSize, InputStatus, InputTextAlign, SegmentedDistribution,
    SegmentedSize, SelectSize, SwitchSize, TagShape, TagSize, YueButton, YueDropdownMenu, YueInput,
    YueMenuItem, YueSegmented, YueSegmentedOption, YueSelect, YueSwatchGroup, YueSwatchOption,
    YueSwitch, YueTag, YueTheme, outline,
};

fn row() -> Div {
    div().flex().items_center().gap_2().min_w_0()
}
fn column() -> Div {
    div().flex().flex_col().gap_2().min_w_0()
}
fn label(text: &'static str) -> Div {
    div().text_sm().font_weight(FontWeight::MEDIUM).child(text)
}
fn text_button(id: &'static str, text: &'static str, theme: YueTheme) -> YueButton {
    YueButton::text(id, theme, text)
        .size(ButtonSize::Small)
        .variant(ButtonVariant::Ghost)
}

fn toggle(
    cx: &Context<SavePanel>,
    handler: impl Fn(&mut SavePanel, bool, &mut Window, &mut Context<SavePanel>) + 'static,
) -> impl Fn(bool, &ClickEvent, &mut Window, &mut App) + 'static {
    let owner = cx.entity().downgrade();
    move |checked, _, window, cx| {
        _ = owner.update(cx, |this, cx| handler(this, checked, window, cx));
    }
}

impl SavePanel {
    fn name_menu(&self, theme: YueTheme, cx: &mut Context<Self>) -> YueDropdownMenu {
        let input = self.name.clone();
        YueDropdownMenu::new(
            "naming-variables",
            theme,
            YueButton::icon("naming-trigger", theme, outline::Braces, "插入命名变量")
                .size(ButtonSize::Small)
                .variant(ButtonVariant::Ghost)
                .disabled(self.busy),
        )
        .label("命名变量")
        .items(
            [
                ("[日期]", "日期"),
                ("[时间]", "时间"),
                ("[序号]", "序号"),
                ("[分辨率]", "分辨率"),
                ("[应用]", "应用名"),
            ]
            .into_iter()
            .map(|(token, title)| {
                let input = input.clone();
                let owner = cx.entity().downgrade();
                YueMenuItem::new(title)
                    .id(token)
                    .on_click(move |_, window, cx| {
                        input.update(cx, |state, cx| {
                            state.focus(window, cx);
                            state.replace(token, window, cx);
                        });
                        _ = owner.update(cx, |this, cx| {
                            this.status = None;
                            cx.notify();
                        });
                    })
            }),
        )
    }

    fn form(&self, theme: YueTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = rgb(theme.text_secondary());
        let border = rgb(theme.border_default());
        let focus_inset = rems(
            ["--input-focus-ring-width", "--input-focus-ring-offset"]
                .into_iter()
                .map(|name| {
                    theme
                        .token(name)
                        .expect("input focus token")
                        .pixels()
                        .expect("focus length")
                })
                .sum::<f32>()
                / 16.,
        );
        let limit = self.export(cx).limit;
        let over_limit = limit
            .zip(self.bytes)
            .is_some_and(|(limit, bytes)| bytes > limit);
        let unit = if self.dimensions.mode == SizeMode::Pixels {
            "px"
        } else {
            "%"
        };
        let mode = if self.dimensions.mode == SizeMode::Pixels {
            "pixels"
        } else {
            "percent"
        };
        let invalid_directory = self.submitted && self.directory.read(cx).value().trim().is_empty();
        let invalid_name = self.submitted && self.name.read(cx).value().trim().is_empty();
        column()
            .id("save-form")
            .test_support()
            .w(rems(21.5))
            .flex_shrink_0()
            .min_h_0()
            .overflow_y_scroll()
            .gap_4()
            .pb_2()
            .child(
                column()
                    .key_context("YueSaveField")
                    // Reserve the outward ring inside the scrolling viewport.
                    .px(focus_inset)
                    .py(focus_inset)
                    .child(
                        row().justify_between().child(label("保存路径")).child(
                            text_button("choose-directory", "更改", theme)
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.choose_directory(window, cx)
                                })),
                        ),
                    )
                    .child(
                        YueInput::new("save-directory", theme, &self.directory)
                            .size(InputSize::Small)
                            .block()
                            .accessibility_label("保存路径")
                            .disabled(self.busy)
                            .status(if invalid_directory {
                                InputStatus::Error
                            } else {
                                InputStatus::Normal
                            })
                            .trailing(
                                YueButton::icon(
                                    "open-directory",
                                    theme,
                                    outline::FolderOpen,
                                    "打开保存文件夹",
                                )
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Small)
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        cx.open_with_system(&PathBuf::from(
                                            this.directory.read(cx).value().as_ref(),
                                        ));
                                    },
                                )),
                            ),
                    )
                    .child(
                        row()
                            .flex_wrap()
                            .children(
                                [
                                    (
                                        "directory-desktop",
                                        "桌面",
                                        crate::save::dirs::desktop as fn() -> String,
                                    ),
                                    (
                                        "directory-pictures",
                                        "图片",
                                        crate::save::dirs::pictures as fn() -> String,
                                    ),
                                    (
                                        "directory-downloads",
                                        "下载",
                                        crate::save::dirs::downloads as fn() -> String,
                                    ),
                                ]
                                .into_iter()
                                .map(|(id, text, folder)| {
                                    div().child(
                                        YueTag::action(id, text)
                                            .theme(theme)
                                            .size(TagSize::Small)
                                            .shape(TagShape::Pill)
                                            .disabled(self.busy)
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.set_directory(folder(), window, cx);
                                            })),
                                    )
                                }),
                            )
                            .children(cx.global::<DirectoryPresets>().paths.iter().map(|path| {
                                let path = path.to_string_lossy().into_owned();
                                let text = PathBuf::from(&path)
                                    .file_name()
                                    .map(|name| name.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| path.clone());
                                let chosen = path.clone();
                                div().child(
                                    YueTag::action(
                                        (ElementId::from("directory-preset"), path.clone()),
                                        text,
                                    )
                                    .theme(theme)
                                    .size(TagSize::Small)
                                    .shape(TagShape::Pill)
                                    .max_width(rems(9.))
                                    .overflow_tooltip(true)
                                    .accessibility_description(path)
                                    .disabled(self.busy)
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            this.set_directory(chosen.clone(), window, cx);
                                        },
                                    )),
                                )
                            }))
                            .child(
                                YueButton::icon(
                                    "add-directory-preset",
                                    theme,
                                    outline::Plus,
                                    "添加路径预设",
                                )
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Small)
                                .tooltip("将当前保存路径添加为预设")
                                .disabled(self.busy || cx.global::<DirectoryPresets>().writing)
                                .loading(self.adding_directory)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.add_directory_preset(cx)),
                                ),
                            ),
                    )
                    .child(
                        column()
                            .key_context("YueSaveField")
                            .child(label("文件名"))
                            .child(
                                row()
                                    .child(
                                        div().flex_1().min_w_0().child(
                                            YueInput::new("save-name", theme, &self.name)
                                                .size(InputSize::Small)
                                                .block()
                                                .accessibility_label("文件名模板")
                                                .disabled(self.busy)
                                                .status(if invalid_name {
                                                    InputStatus::Error
                                                } else {
                                                    InputStatus::Normal
                                                })
                                                .trailing(self.name_menu(theme, cx)),
                                        ),
                                    )
                                    .child(
                                        YueSelect::new("save-format", &self.format_select)
                                            .theme(theme)
                                            .size(SelectSize::Small)
                                            .accessibility_label("图片格式")
                                            .width(rems(6.5)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("变量在保存时展开，同名文件默认自动重命名"),
                            )
                            .child(
                                YueTag::checkable(
                                    "overwrite-existing",
                                    "覆盖同名文件",
                                    self.conflict == ConflictPolicy::Overwrite,
                                )
                                .theme(theme)
                                .size(TagSize::Small)
                                .shape(TagShape::Pill)
                                .disabled(self.busy)
                                .on_change(toggle(
                                    cx,
                                    |this, checked, _, cx| {
                                        this.conflict = if checked {
                                            ConflictPolicy::Overwrite
                                        } else {
                                            ConflictPolicy::AutoRename
                                        };
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                    .child(
                        row()
                            .child(
                                label("保存后")
                                    .id("after-save-label")
                                    .test_support()
                                    .flex_shrink_0(),
                            )
                            .child(
                                row().flex_wrap().children(
                                    [
                                        ("after-copy-path", "复制路径"),
                                        ("after-open-folder", "打开文件夹"),
                                    ]
                                    .into_iter()
                                    .enumerate()
                                    .map(
                                        |(index, (id, text))| {
                                            YueTag::checkable(id, text, self.after[index])
                                                .theme(theme)
                                                .size(TagSize::Small)
                                                .shape(TagShape::Pill)
                                                .disabled(self.busy)
                                                .on_change(toggle(
                                                    cx,
                                                    move |this, checked, _, cx| {
                                                        this.after[index] = checked;
                                                        source::remember_after(this.after);
                                                        cx.notify();
                                                    },
                                                ))
                                        },
                                    ),
                                ),
                            ),
                    )
                    .child(
                        column()
                            .key_context("YueSaveField")
                            .border_t_1()
                            .border_color(border)
                            .pt_3()
                            .child(
                                row().justify_between().child(label("尺寸")).child(
                                    YueSegmented::new("size-unit", theme, mode)
                                        .size(SegmentedSize::Small)
                                        .disabled(self.busy)
                                        .accessibility_label("尺寸单位")
                                        .options([
                                            YueSegmentedOption::new("pixels", "数值"),
                                            YueSegmentedOption::new("percent", "百分比"),
                                        ])
                                        .on_change(cx.listener(
                                            |this, value: &SharedString, window, cx| {
                                                this.set_mode(
                                                    if value == "percent" {
                                                        SizeMode::Percent
                                                    } else {
                                                        SizeMode::Pixels
                                                    },
                                                    window,
                                                    cx,
                                                )
                                            },
                                        )),
                                ),
                            )
                            .child(
                                row()
                                    .child(div().text_sm().text_color(muted).child("宽"))
                                    .child(
                                        div().flex_1().min_w_0().child(
                                            YueInput::new("save-width", theme, &self.width)
                                                .size(InputSize::Small)
                                                .block()
                                                .text_align(InputTextAlign::Right)
                                                .accessibility_label("输出宽度")
                                                .disabled(self.busy)
                                                .status(if self.dimension_errors[0].is_some() {
                                                    InputStatus::Error
                                                } else {
                                                    InputStatus::Normal
                                                })
                                                .trailing(
                                                    div().text_xs().text_color(muted).child(unit),
                                                ),
                                        ),
                                    )
                                    .child(
                                        (if self.dimensions.locked {
                                            YueButton::icon(
                                                "lock-ratio",
                                                theme,
                                                outline::Lock,
                                                "解锁宽高比例",
                                            )
                                        } else {
                                            YueButton::icon(
                                                "lock-ratio",
                                                theme,
                                                outline::LockOpen,
                                                "锁定宽高比例",
                                            )
                                        })
                                        .size(ButtonSize::Small)
                                        .variant(if self.dimensions.locked {
                                            ButtonVariant::Default
                                        } else {
                                            ButtonVariant::Ghost
                                        })
                                        .disabled(self.busy)
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.dimensions.set_locked(!this.dimensions.locked);
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                    .child(div().text_sm().text_color(muted).child("高"))
                                    .child(
                                        div().flex_1().min_w_0().child(
                                            YueInput::new("save-height", theme, &self.height)
                                                .size(InputSize::Small)
                                                .block()
                                                .text_align(InputTextAlign::Right)
                                                .accessibility_label("输出高度")
                                                .disabled(self.busy)
                                                .status(if self.dimension_errors[1].is_some() {
                                                    InputStatus::Error
                                                } else {
                                                    InputStatus::Normal
                                                })
                                                .trailing(
                                                    div().text_xs().text_color(muted).child(unit),
                                                ),
                                        ),
                                    ),
                            )
                            .child(
                                row()
                                    .children([100u32, 75, 50, 25].into_iter().map(|percent| {
                                        YueTag::action(
                                            SharedString::from(format!("preset-{percent}")),
                                            format!("{percent}%"),
                                        )
                                        .theme(theme)
                                        .size(TagSize::Small)
                                        .shape(TagShape::Pill)
                                        .disabled(self.busy)
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                this.preset(percent, window, cx)
                                            }),
                                        )
                                    }))
                                    .child(
                                        YueButton::icon(
                                            "reset-size",
                                            theme,
                                            outline::Restore,
                                            "重置为原始尺寸",
                                        )
                                        .size(ButtonSize::Small)
                                        .variant(ButtonVariant::Ghost)
                                        .disabled(self.busy)
                                        .on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.preset(100, window, cx)
                                            }),
                                        ),
                                    ),
                            )
                            .when(
                                self.dimensions.output.width > self.dimensions.source.width
                                    || self.dimensions.output.height
                                        > self.dimensions.source.height,
                                |this| {
                                    this.child(
                                        div()
                                            .text_xs()
                                            .text_color(
                                                theme
                                                    .token("--warning")
                                                    .unwrap()
                                                    .rgba()
                                                    .expect("warning token"),
                                            )
                                            .child("超过原始分辨率，放大不会增加图像细节"),
                                    )
                                },
                            )
                            .children(self.dimension_errors.iter().flatten().take(1).map(
                                |error| {
                                    div()
                                        .text_xs()
                                        .text_color(error_color(theme))
                                        .child(error.clone())
                                },
                            )),
                    )
                    .child(
                        column()
                            .border_t_1()
                            .border_color(border)
                            .pt_3()
                            .child(
                                row()
                                    .justify_between()
                                    .child(row().child(label("质量")).child(self.advanced(theme)))
                                    .child(
                                        row()
                                            .child(
                                                YueSwitch::new(
                                                    "limit-enabled",
                                                    theme,
                                                    self.limit_on,
                                                    "限制文件体积",
                                                )
                                                .size(SwitchSize::Small)
                                                .disabled(self.busy)
                                                .on_change(toggle(cx, |this, checked, _, cx| {
                                                    this.limit_on = checked;
                                                    cx.notify();
                                                })),
                                            )
                                            .child(
                                                div().text_xs().text_color(muted).child("限制体积"),
                                            ),
                                    ),
                            )
                            .child(if self.format == SaveFormat::Jpeg {
                                YueSegmented::new(
                                    "jpeg-quality",
                                    theme,
                                    ["high", "medium", "low"][self.tier],
                                )
                                .size(SegmentedSize::Small)
                                .distribution(SegmentedDistribution::Equal)
                                .width(relative(1.))
                                .disabled(self.busy)
                                .accessibility_label("JPEG 编码质量")
                                .options([
                                    YueSegmentedOption::new("high", "高 · 92"),
                                    YueSegmentedOption::new("medium", "中 · 75"),
                                    YueSegmentedOption::new("low", "低 · 50"),
                                ])
                                .on_change(cx.listener(|this, value: &SharedString, window, cx| {
                                    this.tier = match value.as_ref() {
                                        "medium" => 1,
                                        "low" => 2,
                                        _ => 0,
                                    };
                                    this.schedule_preview(window, cx);
                                }))
                                .into_any_element()
                            } else {
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(if self.format == SaveFormat::Png {
                                        "PNG 是无损压缩，不支持质量调整"
                                    } else {
                                        "WebP 使用无损编码，不支持质量调整"
                                    })
                                    .into_any_element()
                            })
                            .when(self.limit_on, |this| {
                                this.child(
                                    row()
                                        .key_context("YueSaveField")
                                        .child(div().text_sm().text_color(muted).child("不超过"))
                                        .child(
                                            YueInput::new("save-limit", theme, &self.limit_input)
                                                .size(InputSize::Small)
                                                .width(rems(6.))
                                                .text_align(InputTextAlign::Right)
                                                .accessibility_label("文件体积上限 KB")
                                                .disabled(self.busy)
                                                .trailing(
                                                    div().text_xs().text_color(muted).child("KB"),
                                                ),
                                        )
                                        .child(
                                            text_button("fit-limit", "自动缩小", theme)
                                                .disabled(
                                                    self.busy || self.measuring || !over_limit,
                                                )
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.fit_limit(window, cx)
                                                })),
                                        ),
                                )
                            })
                            .when(over_limit, |this| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(error_color(theme))
                                        .child("超出体积上限，请自动缩小尺寸后保存"),
                                )
                            })
                            .when(self.format == SaveFormat::Jpeg, |this| {
                                this.child(
                                    row()
                                        .flex_wrap()
                                        .child(
                                            div().text_xs().text_color(muted).child("透明区域填充"),
                                        )
                                        .child(
                                            YueSwatchGroup::new(
                                                "jpeg-fill",
                                                theme,
                                                self.fill_choice.clone(),
                                            )
                                            .label("JPEG 背景填充")
                                            .options([
                                                YueSwatchOption::solid(
                                                    "white",
                                                    "白色",
                                                    rgba(0xffffffff),
                                                ),
                                                YueSwatchOption::solid(
                                                    "black",
                                                    "黑色",
                                                    rgba(0x000000ff),
                                                ),
                                            ])
                                            .custom_color(
                                                "custom",
                                                "自定义填充色",
                                                &self.fill_picker,
                                            )
                                            .disabled(self.busy)
                                            .on_change(cx.listener(
                                                |this, value: &SharedString, window, cx| {
                                                    this.fill_choice = value.clone();
                                                    if value != "custom" {
                                                        this.fill = if value == "black" {
                                                            [0; 3]
                                                        } else {
                                                            [255; 3]
                                                        };
                                                        this.schedule_preview(window, cx);
                                                    }
                                                },
                                            )),
                                        ),
                                )
                            }),
                    )
                    .child(
                        column()
                            .border_t_1()
                            .border_color(border)
                            .pt_3()
                            .child(row().justify_between().child(label("预估大小")).child(
                                div().text_xs().text_color(muted).child(if self.measuring {
                                    "正在编码…"
                                } else {
                                    "实际编码测量"
                                }),
                            ))
                            .child(
                                div()
                                    .id("encoded-byte-count")
                                    .test_support()
                                    .text_2xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(
                                        self.bytes
                                            .map(crate::save::estimate::format_bytes)
                                            .unwrap_or_else(|| "—".into()),
                                    ),
                            )
                            .child(div().text_xs().text_color(muted).child(format!(
                                "输出 {} × {} px",
                                self.dimensions.output.width, self.dimensions.output.height
                            ))),
                    ),
            )
    }

    fn advanced(&self, theme: YueTheme) -> Popover {
        Popover::new("encoding-info")
            .anchor(Anchor::TopRight)
            .offset(px(8.))
            .trigger_with(move |open, _, _| {
                YueButton::icon(
                    "encoding-info-trigger",
                    theme,
                    outline::AdjustmentsHorizontal,
                    "编码说明",
                )
                .size(ButtonSize::Small)
                .variant(ButtonVariant::Ghost)
                .expanded(open)
                .into_any_element()
            })
            .content(move |_, _, _| {
                column()
                    .w(rems(18.))
                    .p_4()
                    .bg(rgb(theme.surface()))
                    .border_1()
                    .border_color(rgb(theme.border_default()))
                    .rounded_lg()
                    .child(label("编码说明"))
                    .child(div().text_sm().child("PNG / WebP：无损，保留透明通道。"))
                    .child(
                        div()
                            .text_sm()
                            .child("JPEG：质量 92 / 75 / 50，透明区域先填充背景色。"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .child("缩小使用面积采样，放大使用双线性插值。"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.text_secondary()))
                            .child("预览纹理最长边 2048 px；输出文件保持所选完整分辨率。"),
                    )
            })
    }

    fn fit_limit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let export = self.export(cx);
        let Some(limit) = export.limit else {
            return;
        };
        self.set_busy(true, window, cx);
        self.operation = Some(cx.spawn_in(window, async move |this, cx| {
            let fitted = cx
                .background_executor()
                .spawn(async move {
                    source::shrink_to_fit(&export.picture, &export.values(), export.output, limit)
                })
                .await;
            _ = this.update_in(cx, |panel, window, cx| {
                panel.set_busy(false, window, cx);
                if let Some(size) = fitted {
                    panel.dimensions.output = size;
                    panel.sync_dimensions(window, cx);
                    panel.schedule_preview(window, cx);
                } else {
                    panel.status_error = true;
                    panel.status = Some("无法满足体积上限，请提高上限或更换格式".into());
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }
}

fn error_color(theme: YueTheme) -> Rgba {
    theme.token("--error").unwrap().rgba().expect("error color")
}

impl Render for SavePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = yue_component::current_theme(cx);
        let font = yue_component::typography::ui_font(theme);
        let muted = rgb(theme.text_secondary());
        let border = rgb(theme.border_default());
        let can_save = !self.busy
            && self.validation(cx).is_ok()
            && !self.measuring
            && !self
                .export(cx)
                .limit
                .zip(self.bytes)
                .is_some_and(|(limit, bytes)| bytes > limit);
        div()
            .id("yue-save-panel")
            .test_support()
            .size_full()
            .flex()
            .flex_col()
            .min_w(rems(46.))
            .min_h(rems(26.))
            .bg(rgb(theme.page()))
            .text_color(rgb(theme.text_primary()))
            .font_family(font.family.clone())
            .when_some(font.font_fallbacks(), |mut this, fallbacks| {
                this.text_style().font_fallbacks = Some(fallbacks);
                this
            })
            .text_sm()
            .key_context("YueSavePanel")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Save, window, cx| this.save(window, cx)))
            .on_action(cx.listener(|this, _: &NextField, window, cx| {
                if !this.composing(window, cx)
                    && !gpui_kit::base::GlobalState::is_in_deferred_context(cx)
                {
                    window.focus_next(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &PreviousField, window, cx| {
                if !this.composing(window, cx)
                    && !gpui_kit::base::GlobalState::is_in_deferred_context(cx)
                {
                    window.focus_prev(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CopyImage, window, cx| this.copy(window, cx)))
            .on_action(cx.listener(|this, _: &Cancel, window, cx| {
                if !this.busy
                    && !this.composing(window, cx)
                    && !gpui_kit::base::GlobalState::is_in_deferred_context(cx)
                {
                    window.remove_window();
                }
            }))
            .child(
                row()
                    .flex_shrink_0()
                    .justify_between()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(border)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("保存图片"))
                    .child(div().text_xs().text_color(muted).child(format!(
                        "原始 {} × {} px",
                        self.picture.size().width,
                        self.picture.size().height
                    ))),
            )
            .child(
                row()
                    .items_stretch()
                    .flex_1()
                    .min_h_0()
                    .gap_5()
                    .p_4()
                    .child(
                        column()
                            .flex_1()
                            .min_h_0()
                            .gap_3()
                            .child(self.viewport(cx))
                            .child(div().text_xs().text_color(muted).child(
                                "滚轮缩放 · 拖动平移 · 双击适应窗口 · 宽高比改变时不可对比",
                            )),
                    )
                    .child(self.form(theme, cx)),
            )
            .child(
                row()
                    .flex_shrink_0()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        div()
                            .id("save-status")
                            .test_support()
                            .role(Role::Status)
                            .aria_label(self.status.clone().unwrap_or_default())
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(if self.status_error {
                                error_color(theme)
                            } else {
                                rgb(theme.text_secondary())
                            })
                            .child(self.status.clone().unwrap_or_default()),
                    )
                    .when_some(self.saved_path.clone(), |this, path| {
                        this.child(
                            text_button("reveal-saved", "查看文件", theme)
                                .on_click(move |_, _, cx| cx.reveal_path(&path)),
                        )
                    })
                    .child(
                        text_button("copy-image", "复制图片", theme)
                            .tooltip("复制图片 (Ctrl+Shift+C)")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.copy(window, cx))),
                    )
                    .child(div().h(rems(1.25)).w_px().bg(border).mx_1())
                    .child(
                        YueButton::text("cancel-save", theme, "取消")
                            .size(ButtonSize::Small)
                            .disabled(self.busy)
                            .on_click(|_, window, _| window.remove_window()),
                    )
                    .child(
                        YueButton::text("commit-save", theme, "保存")
                            .size(ButtonSize::Small)
                            .variant(ButtonVariant::Primary)
                            .loading(self.busy)
                            .disabled(!can_save)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}
