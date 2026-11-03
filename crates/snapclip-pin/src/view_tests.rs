use super::{PinContent, PinManager, PinSnapshot};
use crate::{
    CopyTarget, DocumentMode, FileKind, PinCommand, PinDocument, PinFile, PinPlacement, PinTable,
};
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, TestAppContext};
use gpui_kit::{point, px};
use std::sync::Arc;

// Uses production views and the real command channel; never reads/writes the desktop clipboard.
#[gpui_kit::test]
fn hdr_view_retains_shared_source_and_sends_explicit_copy_intents(cx: &mut TestAppContext) {
    use snapclip_model::pixels::{
        AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding, PixelSource,
    };
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
        crate::init(cx);
    });
    let descriptor = PixelDescriptor::new(
        snapclip_model::PixelFormat::Rgba16Float,
        PixelEncoding::ScRgb,
        AlphaMode::Straight,
    )
    .unwrap();
    let bytes = [-0.25_f32, 2.5, 0.5, 0.5]
        .into_iter()
        .flat_map(|v| half::f16::from_f32(v).to_le_bytes())
        .collect();
    let pixels = Arc::new(PixelBuffer::from_tight(1, 1, descriptor, bytes).unwrap());
    let image =
        crate::PinImage::from_pixels(PixelSource::new(descriptor, 82), pixels.clone()).unwrap();
    let (owner, commands) = cx.update(|cx| {
        let (manager, commands) = PinManager::new().unwrap();
        (cx.new(|_| manager), commands)
    });
    let id = cx.update(|cx| {
        PinManager::open(
            Arc::new(PinSnapshot::new(PinContent::Image(image))),
            PinPlacement::Pointer((0, 0)),
            &owner,
            cx,
        )
        .unwrap()
        .id()
    });
    let (window, view) = cx.update(|cx| owner.read(cx).view_for_test(id));
    let view = view.upgrade().unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(Arc::ptr_eq(
            view.read(cx).image_pixels.as_ref().unwrap().pixels(),
            &pixels
        ));
        assert_eq!(
            view.read(cx)
                .image_preview
                .read(cx)
                .snapshot()
                .reference_size(),
            Some([1, 1])
        );
        assert!(
            window
                .find((gpui_kit::ElementId::from("pin-image-preview"), "hdr-info"))
                .visible()
        );
        window.press("ctrl-c", cx);
        window.click("pin-more-button", cx);
    })
    .unwrap();
    assert!(
        matches!(commands.try_recv().unwrap(), PinCommand::Copy { target: CopyTarget::Pixels(image), .. }
        if image.is_hdr() && Arc::ptr_eq(image.pixels(), &pixels))
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let target = window
            .find_by_label("复制 HDR 图片（PNG）")
            .path()
            .last()
            .unwrap()
            .clone();
        window.click(target, cx);
    })
    .unwrap();
    assert!(
        matches!(commands.try_recv().unwrap(), PinCommand::Copy { target: CopyTarget::HdrPixels(image), .. }
        if image.pixel_source().generation() == 82 && Arc::ptr_eq(image.pixels(), &pixels))
    );
    cx.update_window(window.into(), |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert!(!owner.read(cx).is_open(id)));
}

#[gpui_kit::test]
fn production_pin_selection_copy_files_and_close_flow(cx: &mut TestAppContext) {
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
        crate::init(cx);
    });
    let (owner, commands) = cx.update(|cx| {
        let (manager, commands) = PinManager::new().unwrap();
        (cx.new(|_| manager), commands)
    });
    let text = "中文 English 😀\n第二行";
    let snapshot = Arc::new(PinSnapshot::new(PinContent::Document(PinDocument::new(
        text.into(),
        DocumentMode::Text,
    ))));
    let id = cx.update(|cx| {
        PinManager::open(snapshot.clone(), PinPlacement::Pointer((0, 0)), &owner, cx)
            .unwrap()
            .id()
    });
    let (window, view) = cx.update(|cx| {
        let (window, view) = owner.read(cx).view_for_test(id);
        (window, view.upgrade().unwrap())
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.find("pin-toolbar").bounds().bottom()
                <= window.find("pin-surface").bounds().bottom()
        );
        window.press("ctrl-a", cx);
        assert!(
            view.read(cx).representations[0]
                .state
                .read(cx)
                .selected_text()
                .contains("中文 English 😀")
        );
        window.press("ctrl-c", cx);
    })
    .unwrap();
    let PinCommand::Copy { id: copied, target } = commands.try_recv().expect("copy intent");
    assert_eq!(copied, id);
    assert!(
        matches!(target, CopyTarget::Text(t) if t.contains("中文 English 😀") && t.contains("第二行"))
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.click_at("pin-document-content", point(px(20.), px(10.)), cx);
        window.press("ctrl-a", cx);
        window.press("ctrl-c", cx);
    })
    .unwrap();
    assert!(
        matches!(commands.try_recv().expect("focused document copy uses application channel"), PinCommand::Copy { target: CopyTarget::Text(t), ..} if t.contains("第二行"))
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.click_with_modifiers(
            "pin-copy",
            gpui_kit::Modifiers {
                alt: true,
                ..Default::default()
            },
            cx,
        );
    })
    .unwrap();
    assert!(
        commands.try_recv().is_err(),
        "Alt-drag owns the pointer before nested commands can activate"
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.click("pin-copy", cx);
    })
    .unwrap();
    assert!(
        matches!(commands.try_recv().unwrap(), PinCommand::Copy {target: CopyTarget::Content(s),..} if Arc::ptr_eq(&s,&snapshot))
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        cx.update_window(window.into(), |_, _, _| ()).is_err(),
        "Escape closes the actual window"
    );
    cx.update(|cx| assert!(!owner.read(cx).is_open(id)));
    drop(view);
    let id = cx.update(|cx| {
        PinManager::open(
            Arc::new(PinSnapshot::new(PinContent::Files(Arc::new(vec![
                PinFile::new(r"C:\A.txt".into(), "A.txt".into(), FileKind::Document),
                PinFile::new(r"C:\B.txt".into(), "B.txt".into(), FileKind::Document),
            ])))),
            PinPlacement::Pointer((0, 0)),
            &owner,
            cx,
        )
        .unwrap()
        .id()
    });
    let window = cx.update(|cx| owner.read(cx).view_for_test(id).0);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("down", cx);
        window.press("ctrl-c", cx);
    })
    .unwrap();
    assert!(
        matches!(commands.try_recv().unwrap(), PinCommand::Copy {target: CopyTarget::Files(p), ..} if p==vec![r"C:\B.txt"])
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.hover(("file", 0usize), cx);
        let action = window.find(("file-copy-path", 0usize));
        assert!(action.bounds().right() < window.find("pin-surface").bounds().right());
        window.click(("file-copy-path", 0usize), cx);
    })
    .unwrap();
    assert!(
        matches!(commands.try_recv().unwrap(), PinCommand::Copy{target:CopyTarget::Text(path),..} if path == r"C:\A.txt")
    );
    cx.update_window(window.into(), |_, window, cx| window.click("pin-close", cx))
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert!(!owner.read(cx).is_open(id)));
    // Table projection uses the production Kit table; representation switches preserve source.
    let table = PinTable::new(
        vec![
            vec!["名称".into(), "数量".into()],
            vec!["甲".into(), "123".into()],
            vec!["乙".into(), String::new()],
        ],
        true,
    )
    .unwrap();
    let original = "名称\t数量\n甲\t123\n乙\t";
    let snapshot = Arc::new(PinSnapshot::new(PinContent::Document(
        PinDocument::new(original.into(), DocumentMode::Table).with_table(table),
    )));
    let id = cx.update(|cx| {
        PinManager::open(snapshot.clone(), PinPlacement::Pointer((0, 0)), &owner, cx)
            .unwrap()
            .id()
    });
    let (window, view) = cx.update(|cx| owner.read(cx).view_for_test(id));
    let view = view.upgrade().unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("table").visible());
        window.click_at("table", point(px(150.), px(50.)), cx);
        assert_eq!(
            view.read(cx)
                .table
                .as_ref()
                .unwrap()
                .read(cx)
                .selected_cell(),
            Some((0, 1))
        );
        window.press("ctrl-c", cx);
    })
    .unwrap();
    assert!(
        matches!(commands.try_recv().unwrap(),PinCommand::Copy{target:CopyTarget::Text(t),..} if t=="123")
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.press("down", cx);
        assert_eq!(
            view.read(cx)
                .table
                .as_ref()
                .unwrap()
                .read(cx)
                .selected_cell(),
            Some((1, 1))
        );
        window.press("ctrl-c", cx);
    })
    .unwrap();
    assert!(
        matches!(commands.try_recv().unwrap(),PinCommand::Copy{target:CopyTarget::Text(t),..} if t.is_empty()),
        "an empty selected cell must not fall back to copying the whole table"
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.press("ctrl-a", cx);
        assert_eq!(
            view.read(cx).current,
            1,
            "select all exposes a visible text selection"
        );
        window.press("ctrl-c", cx);
    })
    .unwrap();
    assert!(
        matches!(commands.try_recv().unwrap(),PinCommand::Copy{target:CopyTarget::Text(t),..} if t==original),
        "Ctrl+A must override the earlier cell selection"
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.click(("pin-representation", 1usize), cx);
        assert_eq!(view.read(cx).current, 1);
        assert!(window.try_find("table").is_none());
        window.press("ctrl-c", cx);
    })
    .unwrap();
    assert!(
        matches!(commands.try_recv().unwrap(),PinCommand::Copy{target:CopyTarget::Text(t),..} if t==original)
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.click(("pin-representation", 0usize), cx);
        assert!(window.find("table").visible());
        window.click("pin-more-button", cx);
        assert!(view.read(cx).menu_open);
    })
    .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find((gpui_kit::ElementId::from("pin-more"), "yue-dropdown-panel"))
                .visible()
        );
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(!view.read(cx).menu_open, "Escape dismisses the menu first");
        assert!(owner.read(cx).is_open(id));
        window.click_at("table", point(px(150.), px(50.)), cx);
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert!(!owner.read(cx).is_open(id)));
    // Image files retain their file identity. Only an explicit menu action
    // prepares the full image, and restoring the card restores file copying.
    let pixels = crate::PinImage::new(2, 1, vec![255, 0, 0, 0, 0, 255, 0, 127]).unwrap();
    let image_path =
        std::env::temp_dir().join(format!("snapclip-pin-view-test-{}.png", std::process::id()));
    std::fs::write(
        &image_path,
        crate::image_preview::encode(&pixels, false).unwrap(),
    )
    .unwrap();
    let image_file = crate::PinFile::new(
        image_path.to_str().unwrap().to_string(),
        "透明图片.png".into(),
        FileKind::Image,
    );
    let id = cx.update(|cx| {
        PinManager::open(
            Arc::new(PinSnapshot::new(PinContent::Files(Arc::new(vec![
                image_file,
            ])))),
            PinPlacement::Pointer((0, 0)),
            &owner,
            cx,
        )
        .unwrap()
        .id()
    });
    let (window, view) = cx.update(|cx| owner.read(cx).view_for_test(id));
    let view = view.upgrade().unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(!view.read(cx).image_mode);
        assert!(view.read(cx).image_pixels.is_none());
        window.click("pin-more-button", cx);
    })
    .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let item = window
            .find_by_label("作为图片贴图")
            .path()
            .last()
            .unwrap()
            .clone();
        window.click(item, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(view.read(cx).image_mode);
        assert!(
            window
                .find((gpui_kit::ElementId::from("pin-image-preview"), "viewport"))
                .visible()
        );
        assert_eq!(
            view.read(cx)
                .image_pixels
                .as_ref()
                .unwrap()
                .sdr_rgba()
                .unwrap(),
            pixels.sdr_rgba().unwrap()
        );
        window.click(
            (gpui_kit::ElementId::from("pin-image-preview"), "viewport"),
            cx,
        );
        window.press("1", cx);
        window.press("=", cx);
        let preview = view.read(cx).image_preview.read(cx).snapshot();
        assert!((preview.zoom() - 1.25).abs() < 0.001);
        assert_eq!(preview.fit(), yue_component::ImagePreviewFit::Manual);
        window.press("ctrl-c", cx);
        window.click("pin-more-button", cx);
    })
    .unwrap();
    assert!(
        matches!(commands.try_recv().unwrap(),PinCommand::Copy{target:CopyTarget::Pixels(image),..} if image.sdr_rgba().unwrap()==pixels.sdr_rgba().unwrap())
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let item = window
            .find_by_label("恢复文件卡片")
            .path()
            .last()
            .unwrap()
            .clone();
        window.click(item, cx);
        assert!(!view.read(cx).image_mode);
        window.press("ctrl-c", cx);
    })
    .unwrap();
    assert!(
        matches!(commands.try_recv().unwrap(),PinCommand::Copy{target:CopyTarget::Files(paths),..} if paths==vec![image_path.to_str().unwrap()])
    );
    // Restoring the card does not recreate the retained preview or lose its camera.
    cx.update_window(window.into(), |_, window, cx| {
        window.click("pin-more-button", cx)
    })
    .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let item = window
            .find_by_label("作为图片贴图")
            .path()
            .last()
            .unwrap()
            .clone();
        window.click(item, cx);
        assert!((view.read(cx).image_preview.read(cx).snapshot().zoom() - 1.25).abs() < 0.001);
        assert_eq!(
            view.read(cx)
                .image_pixels
                .as_ref()
                .unwrap()
                .sdr_rgba()
                .unwrap(),
            pixels.sdr_rgba().unwrap()
        );
    })
    .unwrap();
    cx.update_window(window.into(), |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    std::fs::remove_file(&image_path).unwrap();
    // F3 primary images use the same real component; image/text representations
    // share the source, but the focused representation controls Ctrl+C.
    let image_snapshot =
        Arc::new(PinSnapshot::new(PinContent::Image(pixels.clone())).with_text("图片的文字备选"));
    let id = cx.update(|cx| {
        PinManager::open(
            image_snapshot.clone(),
            PinPlacement::Pointer((0, 0)),
            &owner,
            cx,
        )
        .unwrap()
        .id()
    });
    let (window, view) = cx.update(|cx| owner.read(cx).view_for_test(id));
    let view = view.upgrade().unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find((gpui_kit::ElementId::from("pin-image-preview"), "viewport"))
                .visible()
        );
        assert_eq!(
            view.read(cx)
                .image_pixels
                .as_ref()
                .unwrap()
                .sdr_rgba()
                .unwrap(),
            pixels.sdr_rgba().unwrap()
        );
        window.press("ctrl-c", cx);
        window.click(("pin-representation", 0usize), cx);
        window.press("ctrl-c", cx);
        assert!(!view.read(cx).image_mode);
        window.click("pin-representation-image", cx);
        window.click("pin-copy", cx);
    })
    .unwrap();
    assert!(
        matches!(commands.try_recv().unwrap(), PinCommand::Copy { target: CopyTarget::Pixels(image), .. } if image.sdr_rgba().unwrap() == pixels.sdr_rgba().unwrap())
    );
    assert!(
        matches!(commands.try_recv().unwrap(), PinCommand::Copy { target: CopyTarget::Text(text), .. } if text == "图片的文字备选")
    );
    assert!(
        matches!(commands.try_recv().unwrap(), PinCommand::Copy { target: CopyTarget::Content(snapshot), .. } if Arc::ptr_eq(&snapshot, &image_snapshot))
    );
    // Exercise retry with decoded pixels already retained, as happens after a
    // render/upload failure. This checks host recovery, not a real GPU failure.
    cx.update_window(window.into(), |_, window, cx| {
        view.update(cx, |view, cx| {
            view.submit_image(
                yue_component::ImagePreviewLoad::Failed("渲染资源暂时不可用".into()),
                window,
                cx,
            )
        });
        window.render_frame(cx);
        assert!(view.read(cx).image_pixels.is_some());
        window.click(
            (gpui_kit::ElementId::from("pin-image-preview"), "retry"),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            view.read(cx)
                .image_preview
                .read(cx)
                .snapshot()
                .reference_size(),
            Some([2, 1])
        );
        assert_eq!(
            view.read(cx)
                .image_pixels
                .as_ref()
                .unwrap()
                .sdr_rgba()
                .unwrap(),
            pixels.sdr_rgba().unwrap()
        );
    })
    .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.click(
            (
                gpui_kit::ElementId::from("pin-image-preview"),
                "more-trigger",
            ),
            cx,
        );
    })
    .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.press("escape", cx);
        assert!(
            owner.read(cx).is_open(id),
            "the preview's own menu dismisses before the pin"
        );
        window.double_click(
            (gpui_kit::ElementId::from("pin-image-preview"), "viewport"),
            cx,
        );
        assert!(
            owner.read(cx).is_open(id),
            "viewer double click fits, rather than closes"
        );
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert!(!owner.read(cx).is_open(id)));

    let id = cx.update(|cx| {
        PinManager::open(
            Arc::new(
                PinSnapshot::new(PinContent::Document(PinDocument::new(
                    "附带位图".into(),
                    DocumentMode::Text,
                )))
                .with_image(pixels.clone()),
            ),
            PinPlacement::Pointer((0, 0)),
            &owner,
            cx,
        )
        .unwrap()
        .id()
    });
    let (window, view) = cx.update(|cx| owner.read(cx).view_for_test(id));
    let view = view.upgrade().unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("pin-representation-image", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            view.read(cx)
                .image_preview
                .read(cx)
                .snapshot()
                .reference_size(),
            Some([2, 1])
        );
        window.press("ctrl-c", cx);
        window.click(("pin-representation", 0usize), cx);
        window.press("ctrl-c", cx);
        window.remove_window();
    })
    .unwrap();
    assert!(
        matches!(commands.try_recv().unwrap(), PinCommand::Copy { target: CopyTarget::Pixels(image), .. } if image.sdr_rgba().unwrap() == pixels.sdr_rgba().unwrap())
    );
    assert!(
        matches!(commands.try_recv().unwrap(), PinCommand::Copy { target: CopyTarget::Text(text), .. } if text == "附带位图")
    );
    cx.run_until_parked();
    // A missing local file remains a file object. The component's retry delegates
    // preparation back to the host and can recover after the real file appears.
    let id = cx.update(|cx| {
        PinManager::open(
            Arc::new(PinSnapshot::new(PinContent::Files(Arc::new(vec![
                crate::PinFile::new(
                    image_path.to_str().unwrap().into(),
                    "恢复后的图片.png".into(),
                    FileKind::Image,
                ),
            ])))),
            PinPlacement::Pointer((0, 0)),
            &owner,
            cx,
        )
        .unwrap()
        .id()
    });
    let (window, view) = cx.update(|cx| owner.read(cx).view_for_test(id));
    let view = view.upgrade().unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("pin-more-button", cx);
    })
    .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let item = window
            .find_by_label("作为图片贴图")
            .path()
            .last()
            .unwrap()
            .clone();
        window.click(item, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(view.read(cx).image_mode);
        assert!(view.read(cx).image_pixels.is_none());
        assert!(
            window
                .find((gpui_kit::ElementId::from("pin-image-preview"), "retry"))
                .visible()
        );
    })
    .unwrap();
    std::fs::write(
        &image_path,
        crate::image_preview::encode(&pixels, false).unwrap(),
    )
    .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.click(
            (gpui_kit::ElementId::from("pin-image-preview"), "retry"),
            cx,
        )
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            view.read(cx)
                .image_pixels
                .as_ref()
                .unwrap()
                .sdr_rgba()
                .unwrap(),
            pixels.sdr_rgba().unwrap()
        );
        assert!(
            window
                .try_find((gpui_kit::ElementId::from("pin-image-preview"), "retry"))
                .is_none()
        );
        window.remove_window();
    })
    .unwrap();
    cx.run_until_parked();
    std::fs::remove_file(&image_path).unwrap();
    for original in [
        "  first\tvalue\r\n\n最后😀  \n".to_string(),
        " \t\n\n".to_string(),
        "中文 English 😀  ".repeat(80),
    ] {
        let id = cx.update(|cx| {
            PinManager::open(
                Arc::new(PinSnapshot::new(PinContent::Document(PinDocument::new(
                    original.clone(),
                    DocumentMode::Text,
                )))),
                PinPlacement::Pointer((0, 0)),
                &owner,
                cx,
            )
            .unwrap()
            .id()
        });
        let (window, view) = cx.update(|cx| owner.read(cx).view_for_test(id));
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press("ctrl-a", cx);
            if original.contains('\t') {
                assert!(
                    view.upgrade().unwrap().read(cx).representations[0]
                        .state
                        .read(cx)
                        .selected_text()
                        .contains('\t'),
                    "rendered selection must retain a real Tab"
                );
            }
            window.press("ctrl-c", cx);
        })
        .unwrap();
        assert!(
            matches!(commands.try_recv().unwrap(),PinCommand::Copy{target:CopyTarget::Text(t),..} if t==original),
            "select-all copy must preserve the original bytes, including CRLF and trailing whitespace"
        );
        cx.update_window(window.into(), |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();
    }
    let undo_window = cx
        .windows()
        .into_iter()
        .find(|handle| {
            cx.update_window(*handle, |_, window, cx| {
                window.render_frame(cx);
                window.try_find("pin-undo-close").is_some()
            })
            .unwrap_or(false)
        })
        .expect("closing a pin offers an Undo button");
    cx.update_window(undo_window, |_, window, cx| {
        window.click("pin-undo-close", cx)
    })
    .unwrap();
    cx.run_until_parked();
    let restored = cx
        .windows()
        .into_iter()
        .find(|handle| {
            cx.update_window(*handle, |_, window, cx| {
                window.render_frame(cx);
                window.try_find("pin-surface").is_some()
            })
            .unwrap_or(false)
        })
        .expect("Undo must reopen the production document window");
    cx.update_window(restored, |_, window, cx| window.press("ctrl-c", cx))
        .unwrap();
    assert!(
        matches!(commands.try_recv().unwrap(),PinCommand::Copy{target:CopyTarget::Text(t),..} if t=="中文 English 😀  ".repeat(80))
    );
    cx.update_window(restored, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        for content in [
            PinContent::Files(Arc::new(vec![])),
            PinContent::Document(PinDocument::new(String::new(), DocumentMode::Text)),
        ] {
            assert!(
                PinManager::open(
                    Arc::new(PinSnapshot::new(content)),
                    PinPlacement::Pointer((0, 0)),
                    &owner,
                    cx
                )
                .is_err()
            );
        }
    });
}
