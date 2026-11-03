use super::*;
#[cfg(feature = "test-support")]
use core::prelude::v1::test;

#[::core::prelude::v1::test]
fn hdr_picture_default_preserves_source_and_explicit_sdr_is_a_derivative() {
    use snapclip_model::pixels::{AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding};
    let descriptor = PixelDescriptor::new(
        snapclip_model::PixelFormat::Rgba16Float,
        PixelEncoding::ScRgb,
        AlphaMode::Straight,
    )
    .unwrap();
    let bytes: Vec<u8> = [1.25_f32, 1.25, 1.25, 0.5, 12.5, 12.5, 12.5, 1.0]
        .into_iter()
        .flat_map(|v| half::f16::from_f32(v).to_le_bytes())
        .collect();
    let picture =
        Picture::from_pixels(PixelBuffer::from_tight(2, 1, descriptor, bytes.clone()).unwrap());
    assert!(picture.is_hdr());
    assert_eq!(picture.original_pixels().as_bytes(), bytes);
    let original = preview::original(&picture).unwrap();
    assert_eq!(
        original.color_space(),
        yue_component::ImagePreviewColorSpace::ScRgb
    );
    assert_eq!(original.image().as_bytes(0).unwrap(), bytes);
    let sdr = picture.sdr_pixels();
    assert_eq!(sdr.descriptor(), PixelDescriptor::bgra8());
    assert_eq!(picture.original_pixels().as_bytes(), bytes);
    let smaller = picture.resized_pixels(Size::new(1, 1)).unwrap();
    assert_eq!(smaller.size(), [1, 1]);
    assert_eq!(smaller.descriptor(), descriptor);
    assert!(
        snapclip_model::pixel_color::linear_rgba(&smaller)
            .next()
            .unwrap()[0]
            > 1.0
    );
}

#[::core::prelude::v1::test]
fn hdr_formats_are_gated_by_the_production_encoder_not_transitive_codec_dependencies() {
    use snapclip_model::pixels::{AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding};
    let descriptor = PixelDescriptor::new(
        snapclip_model::PixelFormat::Rgba16Float,
        PixelEncoding::ScRgb,
        AlphaMode::Straight,
    )
    .unwrap();
    let bytes = [12.5_f32, 2.5375, 0.25, 0.5]
        .into_iter()
        .flat_map(|v| half::f16::from_f32(v).to_le_bytes())
        .collect::<Vec<_>>();
    let picture = Arc::new(Picture::from_pixels(
        PixelBuffer::from_tight(1, 1, descriptor, bytes.clone()).unwrap(),
    ));
    let mut export = Export {
        color_export: ColorExport::PreserveHdr,
        picture: picture.clone(),
        output: Size::new(1, 1),
        directory: std::env::temp_dir().to_string_lossy().into_owned(),
        name: "hdr-gate-test".into(),
        format: SaveFormat::Png,
        tier: 1,
        fill: [255; 3],
        date: "2026-11-04".into(),
        time: "1200".into(),
        sequence: 1,
        conflict: ConflictPolicy::Overwrite,
        limit: None,
    };
    let encoded = export.encoded_bytes().unwrap();
    let decoded = snapclip_pin::decode_image(&encoded).unwrap();
    assert!(decoded.is_hdr());
    assert_eq!(decoded.pixels().size(), [1, 1]);
    let pixel = snapclip_model::pixel_color::linear_rgba(decoded.pixels())
        .next()
        .unwrap();
    assert!(pixel[0] > 1.0);
    assert!((pixel[3] - 0.5).abs() < 0.001);
    for format in [SaveFormat::Jpeg, SaveFormat::Webp, SaveFormat::Bmp] {
        export.format = format;
        assert!(
            export
                .encoded_bytes()
                .unwrap_err()
                .message()
                .contains("保留 HDR 仅支持 PNG16")
        );
    }
    assert!(
        serde_json::from_str::<SaveFormat>("\"avif\"").is_err(),
        "AVIF must stay unavailable until a production HDR encoder/decoder is verified"
    );
    assert_eq!(picture.original_pixels().as_bytes(), bytes);
}

#[::core::prelude::v1::test]
fn preview_decodes_the_exact_bytes_written_by_each_encoder() {
    let directory = std::env::temp_dir().join(format!(
        "snapclip-yue-preview-roundtrip-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let picture = Arc::new(Picture::with_transparent_middle());
    for format in [SaveFormat::Png, SaveFormat::Jpeg, SaveFormat::Webp] {
        let export = Export {
            color_export: ColorExport::Sdr {
                source_peak_nits: 100.0,
            },
            picture: picture.clone(),
            output: Size::new(96, 80),
            directory: directory.to_string_lossy().into(),
            name: format!("preview-{format:?}"),
            format,
            tier: 1,
            fill: [17, 70, 180],
            date: "2026-11-02".into(),
            time: "1200".into(),
            sequence: 1,
            conflict: ConflictPolicy::Overwrite,
            limit: None,
        };
        let preview = preview::encode(&export).unwrap();
        let outcome = export.save().unwrap();
        assert_eq!(
            preview.bytes, outcome.bytes,
            "preview measures the same bytes the file contains"
        );
        let bytes = std::fs::read(&outcome.path).unwrap();
        assert_eq!(bytes.len() as u64, preview.bytes);
        let decoded = image::load_from_memory(&bytes).unwrap().into_rgba8();
        assert_eq!(decoded.dimensions(), (96, 80));
        let center = decoded.get_pixel(48, 40).0;
        if format == SaveFormat::Jpeg {
            assert_eq!(center[3], 255);
            assert!(
                (i16::from(center[0]) - 180).abs() <= 4,
                "the JPEG really applies the selected background"
            );
        } else {
            assert_eq!(center[3], 0);
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[::core::prelude::v1::test]
fn raw_preview_preserves_bgra_alpha_and_bounds_its_texture() {
    let pixels = vec![17, 70, 180, 0, 93, 21, 45, 255];
    let picture = Picture::from_selection(pixels.clone(), Size::new(2, 1)).unwrap();
    let original = preview::original(&picture).unwrap();
    assert_eq!(original.image().as_bytes(0).unwrap(), pixels);
    assert_eq!(original.pixel_size(), [2, 1]);
    assert_eq!(original.texture_size(), [2, 1]);
    assert!(!original.is_proxy());

    let picture =
        Picture::from_selection([17, 70, 180, 128].repeat(8192 * 2), Size::new(8192, 2)).unwrap();
    let original = preview::original(&picture).unwrap();
    assert_eq!(original.pixel_size(), [8192, 2]);
    assert_eq!(original.texture_size(), [8192, 2]);
    assert!(
        !original.is_proxy(),
        "a thin 8192px capture fits both budgets"
    );
    assert!(
        original
            .image()
            .as_bytes(0)
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [17, 70, 180, 128])
    );
}

#[::core::prelude::v1::test]
fn raw_preview_keeps_4k_pixels_without_cpu_resampling() {
    let mut pixels = [17, 70, 180, 128].repeat(3840 * 2160);
    pixels[..4].copy_from_slice(&[1, 2, 3, 0]);
    let last = pixels.len() - 4;
    pixels[last..].copy_from_slice(&[4, 5, 6, 255]);
    let picture = Picture::from_selection(pixels, Size::new(3840, 2160)).unwrap();
    let original = preview::original(&picture).unwrap();
    assert_eq!(original.texture_size(), [3840, 2160]);
    assert_eq!(original.image().as_bytes(0).unwrap(), picture.pixels().0);
}

#[::core::prelude::v1::test]
fn long_preview_declares_proxy_without_relabelling_source_pixels() {
    let picture =
        Picture::from_selection([17, 70, 180, 128].repeat(64 * 32768), Size::new(64, 32768))
            .unwrap();
    let preview = preview::original(&picture).unwrap();
    assert_eq!(preview.pixel_size(), [64, 32768]);
    assert_eq!(preview.texture_size(), [16, 8192]);
    assert!(preview.is_proxy());
    assert!(
        preview
            .image()
            .as_bytes(0)
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|v| *v == [17, 70, 180, 128])
    );
}

#[::core::prelude::v1::test]
fn encoded_preview_keeps_native_detail_above_the_old_2048_cap() {
    let export = Export {
        color_export: ColorExport::Sdr {
            source_peak_nits: 100.0,
        },
        picture: Arc::new(Picture::with_transparent_middle()),
        output: Size::new(3072, 2),
        directory: String::new(),
        name: "preview-only".into(),
        format: SaveFormat::Png,
        tier: 1,
        fill: [17, 70, 180],
        date: "2026-11-03".into(),
        time: "1200".into(),
        sequence: 1,
        conflict: ConflictPolicy::Overwrite,
        limit: None,
    };
    let preview = preview::encode(&export).unwrap();
    assert_eq!(preview.encoded.pixel_size(), [3072, 2]);
    assert_eq!(preview.encoded.texture_size(), [3072, 2]);
    assert!(!preview.encoded.is_proxy());
    let bytes = crate::save::assemble::encoded_bytes(
        &export.values(),
        &export.picture.frame(export.output),
    )
    .unwrap();
    let decoded = image::load_from_memory(&bytes).unwrap().into_rgba8();
    let mut bgra = decoded.into_raw();
    for pixel in bgra.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    assert_eq!(preview.encoded.image().as_bytes(0).unwrap(), bgra);
    assert_eq!(preview.bytes, bytes.len() as u64);
}

#[::core::prelude::v1::test]
fn raw_preview_rejects_inconsistent_capture_buffers() {
    for (source, pixels) in [
        (Size::new(0, 1), vec![]),
        (Size::new(1, 1), vec![0; 3]),
        (Size::new(1, 1), vec![0; 8]),
    ] {
        assert!(Picture::from_selection(pixels, source).is_err());
    }
}

#[cfg(feature = "test-support")]
mod ui {
    use super::*;
    use gpui_kit::{
        component::Root,
        test::{TestQueryExt as _, TestWindowExt},
    };
    use yue_component::{ImagePreviewBackground, ImagePreviewFit, ImagePreviewMode};

    fn setup(
        cx: &mut TestAppContext,
        width: f32,
        height: f32,
    ) -> (WindowHandle<Root>, Entity<SavePanel>) {
        setup_picture(
            cx,
            width,
            height,
            Picture::from_selection(vec![255; 800 * 600 * 4], Size::new(800, 600)).unwrap(),
        )
    }

    fn setup_picture(
        cx: &mut TestAppContext,
        width: f32,
        height: f32,
        picture: Picture,
    ) -> (WindowHandle<Root>, Entity<SavePanel>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(DirectoryPresets::default());
            init(cx, true);
        });
        let mut view = None;
        let handle = cx.open_window(size(px(width), px(height)), |window, cx| {
            let panel = cx.new(|cx| SavePanel::new(picture, window, cx));
            view = Some(panel.clone());
            Root::new(panel, window, cx)
        });
        (handle, view.unwrap())
    }

    fn settle_preview(cx: &mut TestAppContext) {
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(250));
        cx.run_until_parked();
    }

    fn save_archived_picture(cx: &mut TestAppContext, invalid_metadata: bool) {
        let directory = std::env::temp_dir().join(format!(
            "save-archive-ui-{}-{invalid_metadata}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let history_root = directory.join("history");
        let store = snapclip_history::store::Store::open(&history_root).unwrap();
        let picture = Picture::synthetic();
        let descriptor = picture.original_pixels().descriptor();
        let archive = crate::history::capture::CaptureArchive::new(
            history_root,
            store.clone(),
            snapclip_model::CaptureMetadata {
                session_id: "panel-save".into(),
                width: if invalid_metadata { 1 } else { 64 },
                height: 64,
                dpi: 0,
                source: snapclip_model::pixels::PixelSource::new(descriptor, 73),
                captured_at_unix_ms: 100,
                monitor_device_name: None,
            },
        );
        let (handle, panel) = setup_picture(cx, 1000., 720., picture.with_archive(archive));
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |panel, _| panel.after = [false; 2]);
            window.render_frame(cx);
            window.click("preset-50", cx);
            window.click("save-directory", cx);
            window.press("ctrl-a", cx);
            window.input(&directory.to_string_lossy(), cx);
            window.click("save-name", cx);
            window.press("ctrl-a", cx);
            window.input("archived", cx);
        })
        .unwrap();
        settle_preview(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(!panel.read(cx).measuring);
            window.click("commit-save", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let panel = panel.read(cx);
            assert!(!panel.busy);
            let output = image::open(
                panel
                    .saved_path
                    .as_ref()
                    .expect("export remains successful"),
            )
            .unwrap();
            assert_eq!((output.width(), output.height()), (32, 32));
            assert_eq!(panel.status_error, invalid_metadata);
            let page = store.history_page(None, None).unwrap();
            if invalid_metadata {
                assert!(
                    panel
                        .status
                        .as_deref()
                        .unwrap()
                        .contains("图片已保存，但历史原图未归档")
                );
                assert!(page.items.is_empty());
            } else {
                let source = page.items[0].capture_source.as_ref().unwrap();
                assert_eq!(
                    (source.metadata.width, source.metadata.height),
                    (64, 64),
                    "archive retains original size, not resized export"
                );
                assert_eq!(source.metadata.source.generation(), 73);
            }
        });
        let _ = std::fs::remove_dir_all(directory);
    }

    #[gpui_kit::test]
    fn saving_a_resized_export_archives_the_unchanged_original(cx: &mut TestAppContext) {
        save_archived_picture(cx, false);
    }

    #[gpui_kit::test]
    fn archive_failure_reports_the_saved_file_separately(cx: &mut TestAppContext) {
        save_archived_picture(cx, true);
    }

    #[gpui_kit::test]
    fn failed_capture_uses_the_component_error_and_real_retry_action(cx: &mut TestAppContext) {
        let (handle, panel) = setup_picture(cx, 1000., 720., Picture::synthetic());
        cx.run_until_parked();
        // Drop the completed task so the assertion detects a missing host retry subscription.
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |panel, cx| {
                panel._original_task = None;
                panel.preview.original = ImagePreviewLoad::Failed("图片资源读取失败".into());
                panel.publish_preview(window, cx);
                cx.notify();
            });
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(matches!(&panel.read(cx).preview.original, ImagePreviewLoad::Failed(message) if message.contains("图片资源")));
            let retry = (ElementId::from("save-image-preview"), "retry");
            assert!(window.find(retry.clone()).visible());
            window.click(retry, cx);
        }).unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                panel.read(cx)._original_task.is_some(),
                "retry must schedule real image preparation"
            );
            assert!(matches!(
                panel.read(cx).preview.original,
                ImagePreviewLoad::Ready(_)
            ));
            assert!(
                !window
                    .try_find((ElementId::from("save-image-preview"), "retry"))
                    .is_some_and(|node| node.visible())
            );
            window.remove_window();
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn yue_viewer_owns_zoom_pan_comparison_background_and_keyboard_scope(cx: &mut TestAppContext) {
        let (handle, panel) = setup(cx, 1000., 720.);
        cx.update(|cx| cx.set_reduce_motion(true));
        settle_preview(cx);
        let state = cx.update(|cx| panel.read(cx).preview.state.clone());
        let child = |name| (ElementId::from("save-image-preview"), name);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let initial = state.read(cx).snapshot();
            assert_eq!(initial.reference_size(), Some([800, 600]));
            assert_eq!(initial.fit(), ImagePreviewFit::FitAll);
            assert!(initial.zoom() > 0. && initial.zoom() <= 1.);
            let viewport = window.find(child("viewport")).bounds();
            assert!(viewport.size.width > px(220.) && viewport.size.height > px(200.));
            assert!(
                800. * initial.zoom() / window.scale_factor() <= f32::from(viewport.size.width)
            );
            assert!(
                600. * initial.zoom() / window.scale_factor() <= f32::from(viewport.size.height)
            );
            window.click(child("actual-size"), cx);
            assert_eq!(state.read(cx).snapshot().zoom(), 1.);
            window.click(child("viewport"), cx);
            window.press("right", cx);
            assert_ne!(state.read(cx).snapshot().center(), [0.5, 0.5]);
            window.click(child("fit-all"), cx);
            assert_eq!(state.read(cx).snapshot().center(), [0.5, 0.5]);
            assert!((state.read(cx).snapshot().zoom() - initial.zoom()).abs() < 0.001);
            window.click(child("viewport"), cx);
            window.press("=", cx);
            let zoom = state.read(cx).snapshot().zoom();
            assert!(zoom > initial.zoom());
            window.click("save-name", cx);
            window.press("ctrl-a", cx);
            window.input("1", cx);
            assert_eq!(panel.read(cx).name.read(cx).value().as_ref(), "1");
            assert_eq!(
                state.read(cx).snapshot().zoom(),
                zoom,
                "preview keys must stay local"
            );
            let compare = window.find_by_label("对比").path().last().unwrap().clone();
            window.click(compare, cx);
            assert_eq!(state.read(cx).snapshot().mode(), ImagePreviewMode::Compare);
            let grip = window.find(child("split")).bounds().center();
            let split = state.read(cx).snapshot().split();
            window.drag(grip, grip + point(px(80.), px(0.)), cx);
            assert!(state.read(cx).snapshot().split() > split);
            window.click(child("more-trigger"), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let black = window.find_by_label("纯黑").path().last().unwrap().clone();
            window.click(black, cx);
            assert_eq!(
                state.read(cx).snapshot().background(),
                ImagePreviewBackground::Black
            );
            window.click(child("more-trigger"), cx);
            window.press("escape", cx);
            assert!(
                window.find("yue-save-panel").visible(),
                "Escape dismisses the preview menu first"
            );
            let result = window
                .find_by_label("编码后")
                .path()
                .last()
                .unwrap()
                .clone();
            window.click(result, cx);
            assert_eq!(state.read(cx).snapshot().mode(), ImagePreviewMode::Result);
            window.click("lock-ratio", cx);
            window.click("save-width", cx);
            window.press("ctrl-a", cx);
            window.input("500", cx);
        })
        .unwrap();
        settle_preview(cx);
        cx.update(|cx| {
            let state = state.read(cx);
            assert!(
                !state.is_mode_available(ImagePreviewMode::Compare),
                "unaligned frames must not be stretched into a comparison"
            );
            assert!(state.is_mode_available(ImagePreviewMode::Result));
            assert_eq!(
                state.snapshot().result_revision(),
                Some(panel.read(cx).revision)
            );
            assert_eq!(panel.read(cx).dimensions.output, Size::new(500, 600));
        });
    }

    #[gpui_kit::test]
    fn the_format_focus_ring_fits_inside_the_form_scroll_clip(cx: &mut TestAppContext) {
        let (handle, _) = setup(cx, 1000., 720.);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("save-format", cx);
            window.press("escape", cx);
            window.render_frame(cx);
            let frame = window
                .find("save-format")
                .bounds()
                .scale(window.scale_factor());
            let ring_color: Hsla = yue_component::current_theme(cx)
                .token("--input-focus-ring-color")
                .unwrap()
                .rgba()
                .unwrap()
                .into();
            let rings = window
                .painted_quads()
                .into_iter()
                .filter(|q| {
                    q.border_color == ring_color
                        && q.bounds.left() < frame.left()
                        && q.bounds.right() > frame.right()
                        && q.bounds.top() < frame.top()
                        && q.bounds.bottom() > frame.bottom()
                })
                .collect::<Vec<_>>();
            assert!(!rings.is_empty(), "the complete focus ring must be painted");
            // GPUI splits a rounded border into multiple clipped paint quads.
            let border = rings[0].bounds;
            let half_width = rings[0].border_widths.top / 2.;
            for point in [
                point(border.left() + half_width, border.center().y),
                point(border.right() - half_width, border.center().y),
                point(border.center().x, border.top() + half_width),
                point(border.center().x, border.bottom() - half_width),
            ] {
                assert!(
                    rings
                        .iter()
                        .any(|quad| quad.content_mask.bounds.contains(&point)),
                    "focus border at {point:?} must survive clipping: {rings:?}"
                );
            }
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn tags_and_the_after_save_caption_share_a_vertical_center(cx: &mut TestAppContext) {
        let (handle, panel) = setup(cx, 1000., 720.);
        for checked in [false, true] {
            panel.update(cx, |panel, cx| {
                panel.after = [checked; 2];
                cx.notify();
            });
            cx.update_window(handle.into(), |_, window, cx| {
                window.render_frame(cx);
                let caption = window.find("after-save-label").bounds();
                for id in ["after-copy-path", "after-open-folder"] {
                    let tag = window.find(id).bounds();
                    // GPUI rounds fractional line boxes to device-pixel positions.
                    assert!(
                        (tag.center().y - caption.center().y).abs() <= px(0.5),
                        "{tag:?} vs {caption:?}"
                    );
                }
            })
            .unwrap();
        }
    }

    #[gpui_kit::test]
    fn save_panel_tags_have_pill_corners(cx: &mut TestAppContext) {
        let (handle, _) = setup(cx, 1000., 720.);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            for id in [
                "directory-desktop",
                "directory-pictures",
                "directory-downloads",
                "overwrite-existing",
                "after-copy-path",
                "after-open-folder",
                "preset-100",
            ] {
                let bounds = window.find(id).bounds().scale(window.scale_factor());
                assert!(
                    window.painted_quads().iter().any(|quad| {
                        quad.bounds == bounds
                            && quad.background.as_solid().is_some_and(|color| color.a > 0.)
                            && quad.corner_radii.top_left >= bounds.size.height / 2.
                            && quad.corner_radii.top_right >= bounds.size.height / 2.
                    }),
                    "{id} must use pill corners"
                );
            }
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn the_add_path_button_adds_a_working_shortcut_and_rejects_invalid_drafts(
        cx: &mut TestAppContext,
    ) {
        let directory =
            std::env::temp_dir().join(format!("snapclip-path-ui-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let (handle, panel) = setup(cx, 1000., 720.);
        let path = directory.to_string_lossy().into_owned();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("save-directory", cx);
            window.press("ctrl-a", cx);
            window.input(&path, cx);
            window.click("add-directory-preset", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                cx.global::<DirectoryPresets>().paths.as_slice(),
                std::slice::from_ref(&directory)
            );
            assert!(!panel.read(cx).adding_directory);
            assert!(!panel.read(cx).status_error);
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("directory-pictures", cx);
            window.render_frame(cx);
            let id: ElementId = (ElementId::from("directory-preset"), path.clone()).into();
            window.click(id, cx);
            assert_eq!(panel.read(cx).directory.read(cx).value().as_ref(), path);
            window.click("add-directory-preset", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(cx.global::<DirectoryPresets>().paths.len(), 1));
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("save-directory", cx);
            window.press("ctrl-a", cx);
            window.input("not-an-absolute-directory", cx);
            window.click("add-directory-preset", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(cx.global::<DirectoryPresets>().paths.len(), 1);
            assert!(panel.read(cx).status_error);
        });
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[gpui_kit::test]
    fn closing_the_panel_does_not_cancel_a_requested_path_preset(cx: &mut TestAppContext) {
        let directory =
            std::env::temp_dir().join(format!("snapclip-path-close-ui-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let (handle, panel) = setup(cx, 1000., 720.);
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_directory(directory.to_string_lossy().into_owned(), window, cx);
                panel.add_directory_preset(cx);
            });
            window.remove_window();
        })
        .unwrap();
        drop(panel);
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                cx.global::<DirectoryPresets>().paths.as_slice(),
                std::slice::from_ref(&directory)
            )
        });
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[gpui_kit::test]
    fn original_is_ready_before_the_export_debounce_and_survives_invalid_edits(
        cx: &mut TestAppContext,
    ) {
        let (handle, panel) = setup(cx, 1000., 720.);
        // Drain immediate work without advancing the 180ms export timer.
        cx.run_until_parked();
        let original = cx.update(|cx| {
            let panel = panel.read(cx);
            assert!(panel.measuring);
            assert!(panel.bytes.is_none());
            assert!(panel.preview.encoded.ready().is_none());
            panel
                .preview
                .original
                .ready()
                .expect("raw capture must not wait for encoding")
                .image()
                .clone()
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .find((ElementId::from("save-image-preview"), "viewport"))
                    .visible()
            );
            window.click("save-width", cx);
            window.press("ctrl-a", cx);
            window.input("0", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let panel = panel.read(cx);
            assert!(panel.dimension_errors[0].is_some());
            assert!(!panel.measuring);
            assert!(Arc::ptr_eq(
                panel.preview.original.ready().unwrap().image(),
                &original
            ));
        });
    }

    #[gpui_kit::test]
    fn cancelling_export_before_it_starts_does_not_cancel_the_original_capture(
        cx: &mut TestAppContext,
    ) {
        let (handle, panel) = setup(cx, 1000., 720.);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("save-width", cx);
            window.press("ctrl-a", cx);
            window.input("0", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let panel = panel.read(cx);
            assert!(panel.preview.original.ready().is_some());
            assert!(panel.preview.encoded.ready().is_none());
            assert!(panel.bytes.is_none());
            assert!(panel.dimension_errors[0].is_some());
        });
    }

    #[gpui_kit::test]
    fn theme_colors_reach_the_painted_panel_and_preview_without_channel_shifts(
        cx: &mut TestAppContext,
    ) {
        let (handle, _) = setup(cx, 1000., 720.);
        for mode in [
            yue_component::ThemeMode::Light,
            yue_component::ThemeMode::Dark,
        ] {
            let theme = yue_component::YueTheme::new(mode, yue_component::Accent::Azure);
            cx.update(|cx| yue_component::set_theme(cx, theme));
            cx.update_window(handle.into(), |_, window, cx| {
                window.render_frame(cx);
                let scale = window.scale_factor();
                let panel = window.find("yue-save-panel").bounds().scale(scale);
                let preview_bounds = window
                    .find((ElementId::from("save-image-preview"), "viewport"))
                    .bounds();
                let preview = preview_bounds.scale(scale);
                let checker = preview_bounds.inset(px(1.)).scale(scale);
                let quads = window.painted_quads();
                let panel_fill = quads
                    .iter()
                    .rev()
                    .find(|quad| quad.bounds == panel)
                    .expect("panel background is painted")
                    .background
                    .as_solid();
                assert_eq!(panel_fill, Some(rgb(theme.page()).into()), "{mode:?}");
                assert!(
                    quads.iter().any(|quad| {
                        quad.bounds == checker
                            && quad.background.as_solid()
                                == Some(
                                    theme
                                        .token("--image-preview-checker-dark-a")
                                        .unwrap()
                                        .rgba()
                                        .unwrap()
                                        .into(),
                                )
                    }),
                    "preview surface must preserve the theme's RGB channels: {mode:?}"
                );
                // GPUI submits the background and border as separate quads.
                assert!(
                    quads.iter().any(|quad| {
                        quad.bounds == preview
                            && quad.border_color == Hsla::from(rgb(theme.border_default()))
                            && quad.border_widths.top > ScaledPixels(0.)
                    }),
                    "preview border must preserve the theme's RGB channels: {mode:?}"
                );

                // The headless text system does not rasterize ordinary glyphs.
                // Assert real surface paint, including the existing YueInput,
                // rather than claiming these quads prove text-pixel contrast.
                let input = window.find("save-name").bounds().scale(scale);
                let input_fill: Hsla = theme
                    .token("--input-background")
                    .unwrap()
                    .rgba()
                    .unwrap()
                    .into();
                assert!(
                    quads.iter().any(|quad| quad.bounds == input
                        && quad.background.as_solid() == Some(input_fill)),
                    "the input surface must follow the same theme as the panel: {mode:?}",
                );
            })
            .unwrap();
        }
    }

    #[gpui_kit::test]
    fn file_format_options_start_at_the_left_padding_and_remain_selectable(
        cx: &mut TestAppContext,
    ) {
        let (handle, panel) = setup(cx, 1000., 720.);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("save-format", cx);
            window.render_frame(cx);
            let nodes = gpui_kit::base::test_support::snapshots(window);
            let theme = yue_component::current_theme(cx);
            let padding = px(theme
                .token("--input-padding-inline-sm")
                .unwrap()
                .pixels()
                .unwrap()
                * f32::from(window.rem_size())
                / 16.);
            for key in ["png", "jpeg", "webp"] {
                let id: ElementId =
                    (ElementId::from("save-format"), format!("option:{key}")).into();
                let text_id: ElementId = (id.clone(), "text").into();
                let row = nodes
                    .iter()
                    .find(|n| n.path().last() == Some(&id))
                    .unwrap()
                    .bounds();
                let text = nodes
                    .iter()
                    .find(|n| n.path().last() == Some(&text_id))
                    .unwrap()
                    .bounds();
                assert!(
                    (text.left() - row.left() - padding).abs() < px(0.01),
                    "{key}: text starts at the normal left inset"
                );
            }
            window.press("down", cx);
            window.press("enter", cx);
            assert!(!panel.read(cx).format_select.read(cx).is_open());
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(panel.read(cx).format, SaveFormat::Jpeg));
    }

    #[gpui_kit::test]
    fn opening_a_new_capture_closes_the_previous_idle_panel(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            init(cx, true);
        });
        let mut panels = PanelWindows::default();
        let previous = cx.update(|cx| {
            panels.open(Picture::synthetic(), cx).unwrap();
            panels.current.as_ref().unwrap().0
        });
        cx.update(|cx| panels.open(Picture::synthetic(), cx).unwrap());
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(previous.update(cx, |_, _, _| ()).is_err());
            assert_eq!(cx.windows().len(), 1);
        });
    }

    #[gpui_kit::test]
    fn composition_enter_does_not_export_an_uncommitted_filename(cx: &mut TestAppContext) {
        let directory =
            std::env::temp_dir().join(format!("snapclip-yue-ime-ui-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let (handle, panel) = setup(cx, 1000., 720.);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("save-directory", cx);
            window.press("ctrl-a", cx);
            window.input(&directory.to_string_lossy(), cx);
        })
        .unwrap();
        settle_preview(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("save-name", cx);
            let name = panel.read(cx).name.clone();
            name.update(cx, |input, cx| {
                let len = input.value().encode_utf16().count();
                input.replace_and_mark_text_in_range(Some(0..len), "候选", Some(2..2), window, cx);
            });
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(!panel.read(cx).busy);
            assert!(panel.read(cx).saved_path.is_none());
            assert_eq!(panel.read(cx).name.read(cx).value().as_ref(), "候选");
        });
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[gpui_kit::test]
    fn rapid_edits_publish_only_the_latest_output_preview(cx: &mut TestAppContext) {
        let (handle, panel) = setup(cx, 1000., 720.);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("preset-25", cx);
            window.click("preset-75", cx);
            window.click("preset-50", cx);
        })
        .unwrap();
        settle_preview(cx);
        cx.update(|cx| {
            let panel = panel.read(cx);
            assert_eq!(panel.dimensions.output, Size::new(400, 300));
            assert!(!panel.measuring);
            let export = panel.export(cx);
            let expected =
                assemble::weigh(&export.values(), &export.picture.frame(export.output)).unwrap();
            assert_eq!(panel.bytes, Some(expected));
            assert!(panel.preview.encoded.ready().is_some());
            let preview = panel.preview.state.read(cx).snapshot();
            assert_eq!(preview.revision(), panel.revision);
            assert_eq!(preview.result_revision(), Some(panel.revision));
        });
    }

    #[gpui_kit::test]
    fn save_writes_the_ui_size_once_and_reports_io_failure_without_losing_edits(
        cx: &mut TestAppContext,
    ) {
        let directory =
            std::env::temp_dir().join(format!("snapclip-yue-save-ui-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let blocked = directory.join("not-a-directory");
        std::fs::write(&blocked, b"unchanged").unwrap();
        let (handle, panel) = setup(cx, 1000., 720.);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("preset-50", cx);
            window.click("save-directory", cx);
            window.press("ctrl-a", cx);
            window.input(&blocked.to_string_lossy(), cx);
            window.click("save-name", cx);
            window.press("ctrl-a", cx);
            window.input("Yue_保存测试", cx);
        })
        .unwrap();
        settle_preview(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(!panel.read(cx).measuring);
            assert!(panel.read(cx).bytes.is_some());
            window.click("commit-save", cx);
            assert!(panel.read(cx).busy);
            assert!(panel.read(cx).format_select.read(cx).is_disabled());
            window.click("commit-save", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(!panel.read(cx).busy);
            assert!(panel.read(cx).saved_path.is_none());
            assert!(panel.read(cx).status.is_some());
            assert_eq!(window.find("save-name").value(), Some("Yue_保存测试"));
            assert_eq!(window.find("save-width").value(), Some("400"));
            window.click("save-directory", cx);
            window.press("ctrl-a", cx);
            window.input(&directory.to_string_lossy(), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("commit-save", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let path = cx.update(|cx| panel.read(cx).saved_path.clone().expect("recovered save"));
        let image = image::open(path).unwrap();
        assert_eq!((image.width(), image.height()), (400, 300));
        assert_eq!(std::fs::read(&blocked).unwrap(), b"unchanged");
        assert_eq!(
            std::fs::read_dir(&directory).unwrap().count(),
            2,
            "double activation writes one file"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[gpui_kit::test]
    fn presets_units_and_ratio_are_driven_through_yue_controls(cx: &mut TestAppContext) {
        let (handle, panel) = setup(cx, 1000., 720.);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("save-width").value(), Some("800"));
            window.click("preset-50", cx);
            assert_eq!(panel.read(cx).dimensions.output, Size::new(400, 300));
            assert_eq!(window.find("save-height").value(), Some("300"));
            // Segmented option IDs are derived from the stable value.
            window.click((ElementId::from("size-unit"), "percent"), cx);
            assert_eq!(window.find("save-width").value(), Some("50"));
            assert_eq!(panel.read(cx).dimensions.output, Size::new(400, 300));
            window.click("reset-size", cx);
            assert_eq!(panel.read(cx).dimensions.output, Size::new(800, 600));
            assert_eq!(window.find("save-width").value(), Some("100"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn tab_in_this_form_moves_focus_without_indenting_the_number(cx: &mut TestAppContext) {
        let (handle, _) = setup(cx, 1000., 720.);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("save-width", cx);
            window.press("tab", cx);
            assert_eq!(window.find("save-width").value(), Some("800"));
            assert_eq!(window.find("lock-ratio").focused(), Some(true));
            window.press("shift-tab", cx);
            assert_eq!(window.find("save-width").focused(), Some(true));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn typing_dimensions_updates_output_and_invalid_drafts_cannot_save(cx: &mut TestAppContext) {
        let (handle, panel) = setup(cx, 1000., 720.);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("save-width", cx);
            window.press("ctrl-a", cx);
            window.input("400", cx);
            assert_eq!(window.find("save-width").value(), Some("400"));
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(panel.read(cx).dimensions.output, Size::new(400, 300));
            assert_eq!(window.find("save-height").value(), Some("300"));
            window.press("ctrl-a", cx);
            window.input("0", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(panel.read(cx).dimensions.output, Size::new(400, 300));
            assert!(panel.read(cx).dimension_errors[0].is_some());
            assert_eq!(window.find("commit-save").disabled(), Some(true));
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(!panel.read(cx).busy);
            assert!(panel.read(cx).saved_path.is_none());
        });
    }

    #[gpui_kit::test]
    fn responsive_layout_keeps_footer_outside_the_scrolling_form(cx: &mut TestAppContext) {
        for (width, height) in [(1000., 720.), (800., 520.), (1280., 900.)] {
            let (handle, _) = setup(cx, width, height);
            cx.update_window(handle.into(), |_, window, cx| {
                window.render_frame(cx);
                let form = window.find("save-form").bounds();
                let preview = window.find("save-preview").bounds();
                let save = window.find("commit-save").bounds();
                assert!(preview.right() <= form.left(), "columns must never overlap");
                assert!(
                    form.bottom() <= save.top(),
                    "footer must remain below form viewport"
                );
                assert!(
                    save.bottom() <= px(height),
                    "save action stays inside the window"
                );
                assert!(preview.size.width >= px(220.));
                assert!(form.size.width >= px(300.));
            })
            .unwrap();
        }
    }

    #[gpui_kit::test]
    fn format_selection_changes_quality_controls_and_escape_only_closes_the_popup(
        cx: &mut TestAppContext,
    ) {
        let (handle, panel) = setup(cx, 1000., 720.);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("save-format", cx);
            window.press("down", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(panel.read(cx).format, SaveFormat::Jpeg);
            assert!(window.find("jpeg-quality").visible());
            assert!(
                !panel.read(cx).busy,
                "choosing a format must not submit the parent panel"
            );
            window.click("save-format", cx);
            window.press("escape", cx);
            assert!(!panel.read(cx).format_select.read(cx).is_open());
            assert!(window.find("yue-save-panel").visible());
        })
        .unwrap();
    }
}
