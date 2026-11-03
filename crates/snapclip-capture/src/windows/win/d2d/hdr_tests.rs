//! Opt-in numeric checks through the actual D2D render/export path. No screen
//! capture, ShowWindow, clipboard or Windows HDR setting is involved.
use super::*;
use snapclip_model::{
    PixelFormat,
    pixels::{AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding},
};

fn sample(pixels: &PixelBuffer, x: u32, y: u32) -> [f32; 4] {
    let at = (y as usize * pixels.size()[0] as usize + x as usize) * 8;
    std::array::from_fn(|c| {
        half::f16::from_le_bytes([
            pixels.as_bytes()[at + c * 2],
            pixels.as_bytes()[at + c * 2 + 1],
        ])
        .to_f32()
    })
}
fn close(actual: [f32; 4], expected: [f32; 4]) {
    for (actual, expected) in actual.into_iter().zip(expected) {
        assert!(
            (actual - expected).abs() <= 0.003 + expected.abs() * 0.002,
            "actual={actual}, expected={expected}"
        );
    }
}

#[test]
#[ignore = "L4: real GPU/D2D FP16 rendering; no screen capture or system-setting changes"]
fn hdr_overlay_keeps_source_scale_masks_linearly_and_exports_fp16_annotations() {
    let device = Arc::new(GraphicsDevice::create().unwrap());
    let descriptor = PixelDescriptor::new(
        PixelFormat::Rgba16Float,
        PixelEncoding::ScRgb,
        AlphaMode::Straight,
    )
    .unwrap();
    let source_pixel = [-0.25, 12.5, 0.5, 1.0];
    let bytes = source_pixel
        .into_iter()
        .flat_map(|v| half::f16::from_f32(v).to_le_bytes())
        .collect::<Vec<_>>()
        .repeat(64 * 48);
    let pixels = PixelBuffer::from_tight(64, 48, descriptor, bytes).unwrap();
    let mut renderer = OverlayRenderer::new(device.clone(), 96).unwrap();
    renderer
        .configure_color(color::OverlayColor::sc_rgb(160.).unwrap())
        .unwrap();
    renderer.ensure_back_buffer(64, 48).unwrap();
    let target = device
        .create_render_target_texture(64, 48, PixelFormat::Rgba16Float)
        .unwrap();
    let bitmap = super::super::d3d11::create_bitmap_from_texture(
        &renderer.d2d,
        &target.texture,
        D2D1_BITMAP_OPTIONS_TARGET,
        D2D1_ALPHA_MODE_PREMULTIPLIED,
    )
    .unwrap();
    let mut view = RenderView::new(Rect::new(0, 0, 64, 48));
    view.selection = Rect::new(16, 12, 48, 36);
    view.show_chrome = false;
    view.cursor_visible = false;
    for gpu_source in [false, true] {
        if gpu_source {
            let frame = device.create_pixel_texture(&pixels).unwrap();
            renderer
                .set_frame_texture(64, 48, &frame.texture, descriptor)
                .unwrap();
            assert!(
                renderer
                    .set_frame_texture(63, 48, &frame.texture, descriptor)
                    .is_err()
            );
            assert!(
                renderer
                    .set_frame_texture(64, 48, &frame.texture, PixelDescriptor::bgra8())
                    .is_err()
            );
            let mut sampler = super::super::d3d11::AsyncSampleBuffer::new(&device).unwrap();
            assert!(
                sampler.submit(&frame.texture, 0, 0, 1).is_err(),
                "SDR sampler must reject FP16 before copying"
            );
        } else {
            renderer.update_pixels(&pixels).unwrap();
        }
        for blur in [false, true] {
            view.mask_blur = blur;
            renderer.draw_to(&bitmap, &view).unwrap();
            let actual = device
                .read_back_pixels(&target.texture, [0, 0, 64, 48], descriptor)
                .unwrap();
            close(sample(&actual, 32, 24), source_pixel);
            // Independent sRGB EOTF reference for the theme tint (#1f75db).
            let tint = [0.013702083, 0.17788842, 0.7083758];
            let expected = std::array::from_fn(|c| {
                if c == 3 {
                    1.
                } else {
                    source_pixel[c] * (1. - MASK_ALPHA) * (1. - MASK_TINT_ALPHA)
                        + tint[c] * 2. * MASK_TINT_ALPHA
                }
            });
            close(sample(&actual, 3, 3), expected);
        }
    }
    view.mask_blur = false;
    view.annotation_items.push(AnnotationItem {
        id: 1,
        kind: AnnotationKind::Rectangle,
        geometry: AnnotationGeometry::Rect {
            bounds: Rect::new(20, 16, 40, 30),
        },
        style: crate::annotation::AnnotationStyle {
            stroke_color: [0.5, 0.5, 0.5, 1.0],
            fill_color: Some([0.5, 0.5, 0.5, 1.0]),
            ..Default::default()
        },
    });
    let exported = renderer.render_export(&view).unwrap();
    assert_eq!(exported.descriptor(), descriptor);
    assert_eq!(exported.size(), [32, 24]);
    close(
        sample(&exported, 14, 11),
        [0.4280823, 0.4280823, 0.4280823, 1.],
    );
    close(sample(&exported, 28, 20), source_pixel);
    assert!(
        renderer.countdown_dim_source(4).is_err(),
        "the old layered SDR countdown must not silently truncate HDR"
    );
}

#[test]
#[ignore = "L4: real GPU/D2D FP16 rendering; no screen capture or system-setting changes"]
fn hdr_ui_icons_alpha_and_white_changes_keep_one_color_interpretation() {
    use ::windows::Win32::Graphics::Direct2D::D2D1_BUFFER_PRECISION_16BPC_FLOAT;
    use snapclip_ui::panel::ICON_BAKED_PX;
    let device = Arc::new(GraphicsDevice::create().unwrap());
    let mut renderer = OverlayRenderer::new(device.clone(), 96).unwrap();
    let edge = ICON_BAKED_PX;
    let descriptor = PixelDescriptor::new(
        PixelFormat::Rgba16Float,
        PixelEncoding::ScRgb,
        AlphaMode::Premultiplied,
    )
    .unwrap();
    for white in [160., 240., 80.] {
        renderer
            .configure_color(color::OverlayColor::sc_rgb(white).unwrap())
            .unwrap();
        assert!(
            renderer.frame_bitmap.is_none(),
            "a color change invalidates the old source interpretation"
        );
        renderer.ensure_back_buffer(edge, edge).unwrap();
        assert_eq!(
            unsafe { renderer.d2d.GetRenderingControls() }.bufferPrecision,
            D2D1_BUFFER_PRECISION_16BPC_FLOAT
        );
        renderer.prepare_panel_icons().unwrap();
        let target = device
            .create_render_target_texture(edge, edge, PixelFormat::Rgba16Float)
            .unwrap();
        let bitmap = super::super::d3d11::create_bitmap_from_texture(
            &renderer.d2d,
            &target.texture,
            D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
        .unwrap();
        let tint = unsafe { renderer.label_text_brush.as_ref().unwrap().GetColor() };
        close(
            [tint.r, tint.g, tint.b, tint.a],
            [white / 80., white / 80., white / 80., 1.],
        );
        for glyph in Glyph::ALL {
            unsafe {
                renderer.d2d.SetTarget(&bitmap);
                renderer.d2d.BeginDraw();
                renderer.d2d.Clear(None);
            }
            renderer
                .panel_icons
                .draw(
                    &renderer.d2d,
                    glyph,
                    Rect::new(0, 0, edge as i32, edge as i32),
                    tint,
                    false,
                )
                .unwrap();
            unsafe {
                renderer.d2d.EndDraw(None, None).unwrap();
            }
            let pixels = device
                .read_back_pixels(&target.texture, [0, 0, edge, edge], descriptor)
                .unwrap();
            let mut covered = 0;
            for y in 0..edge {
                for x in 0..edge {
                    let p = sample(&pixels, x, y);
                    if p[3] > 0.01 {
                        covered += 1;
                        close(
                            p,
                            [
                                white / 80. * p[3],
                                white / 80. * p[3],
                                white / 80. * p[3],
                                p[3],
                            ],
                        );
                    }
                }
            }
            assert!(covered > 20, "{} must actually render", glyph.name());
        }
        // Straight authored alpha is multiplied once by source-over, after EOTF.
        let brush = renderer.create_brush(&color(0.5, 0.5, 0.5, 0.25)).unwrap();
        unsafe {
            renderer.d2d.BeginDraw();
            renderer.d2d.Clear(Some(&color(0., 0., 0., 1.)));
            renderer
                .d2d
                .FillRectangle(&to_d2d(Rect::new(0, 0, edge as i32, edge as i32)), &brush);
            renderer.d2d.EndDraw(None, None).unwrap();
        }
        let pixels = device
            .read_back_pixels(&target.texture, [0, 0, edge, edge], descriptor)
            .unwrap();
        let expected = 0.21404114 * white / 80. * 0.25;
        close(sample(&pixels, 40, 40), [expected, expected, expected, 1.]);
    }
    // Return to the production SDR contract; no stale HDR brushes or glyphs survive.
    renderer
        .configure_color(color::OverlayColor::sdr())
        .unwrap();
    assert!(renderer.frame_bitmap.is_none());
    renderer.prepare_panel_icons().unwrap();
    let gray = [128, 128, 128, 255].repeat((edge * edge) as usize);
    renderer.update_frame(edge, edge, &gray).unwrap();
    let mut view = RenderView::new(Rect::new(0, 0, edge as i32, edge as i32));
    view.selection = view.frame;
    view.show_chrome = false;
    view.cursor_visible = false;
    let exported = renderer.render_export(&view).unwrap();
    assert_eq!(exported.descriptor(), PixelDescriptor::bgra8());
    assert_eq!(exported.as_bytes(), gray);
}

#[test]
#[ignore = "L4: real GPU/D2D FP16 rendering; no screen capture or system-setting changes"]
fn hdr_loupe_center_uses_the_source_not_the_sdr_sampler_or_crosshair() {
    let device = Arc::new(GraphicsDevice::create().unwrap());
    let mut renderer = OverlayRenderer::new(device.clone(), 96).unwrap();
    let descriptor = color::OverlayColor::sc_rgb(160.).unwrap().descriptor();
    renderer.configure_source(descriptor, Some(160.)).unwrap();
    let (width, height) = (640, 480);
    renderer.ensure_back_buffer(width, height).unwrap();
    let source = [0.25, 4., 1., 1.];
    let bytes = source
        .into_iter()
        .flat_map(|v| half::f16::from_f32(v).to_le_bytes())
        .collect::<Vec<_>>()
        .repeat((width * height) as usize);
    renderer
        .update_pixels(&PixelBuffer::from_tight(width, height, descriptor, bytes).unwrap())
        .unwrap();
    let target = device
        .create_render_target_texture(width, height, PixelFormat::Rgba16Float)
        .unwrap();
    let bitmap = super::super::d3d11::create_bitmap_from_texture(
        &renderer.d2d,
        &target.texture,
        D2D1_BITMAP_OPTIONS_TARGET,
        D2D1_ALPHA_MODE_PREMULTIPLIED,
    )
    .unwrap();
    let mut view = RenderView::new(Rect::new(0, 0, width as i32, height as i32));
    view.selection = view.frame;
    view.cursor = Point::new(120, 120);
    view.cursor_visible = true;
    view.show_chrome = true;
    view.work_area = view.frame;
    view.show_magnifier = true;
    view.magnifier_rgb = Some((8, 9, 10)); // A stale SDR sample must not overwrite HDR.
    let config = MagnifierConfig::with_zoom(view.magnifier_zoom).scaled(96);
    let geometry = magnifier_geometry(view.cursor, config, view.frame, view.work_area);
    let x = geometry.panel.left
        + ((geometry.source.width() / 2) as f32 * geometry.zoom).round() as i32
        + (geometry.zoom / 2.) as i32;
    let y = geometry.panel.top
        + ((geometry.source.height() / 2) as f32 * geometry.zoom).round() as i32
        + (geometry.zoom / 2.) as i32;
    renderer.draw_to(&bitmap, &view).unwrap();
    let actual = device
        .read_back_pixels(&target.texture, [0, 0, width, height], descriptor)
        .unwrap();
    close(sample(&actual, x as u32, y as u32), source);
    // The info swatch is deliberately SDR; unlike the source pixel it goes
    // through UI EOTF/output-white mapping. Test the actual D2D fill, not a flag.
    let swatch_x = geometry.info_panel.left as f32 + INFO_PADDING_H_DIP + INFO_SWATCH_SIZE_DIP / 2.;
    let swatch_y = geometry.info_panel.top as f32 + INFO_PADDING_V_DIP + INFO_SWATCH_SIZE_DIP / 2.;
    // All three sRGB values fall on EOTF's linear segment; white is 160/80.
    let expected = [8., 9., 10.].map(|v| v / 255. / 12.92 * 2.);
    close(
        sample(&actual, swatch_x as u32, swatch_y as u32),
        [expected[0], expected[1], expected[2], 1.],
    );
}
