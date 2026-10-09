//! Render-level tests (docs/23 T1.8).
//!
//! They live outside the parent so the render code is not read through 1600 lines of
//! test scaffolding.

    use super::{
        ChainRingView, MASK_ALPHA, MASK_TINT_ALPHA, OverlayRenderer, PREVIEW_WASH_ALPHA,
        PREVIEW_WASH_WALK_SCALE, RenderMetrics, RenderView, capture_mix_color, capture_wash_color,
        to_d2d,
    };
    use crate::geometry::LevelReach;
    use crate::ring_contrast::{ACCENT_RGB, CAPTURE_RGB, Rgb};
    use crate::annotation::{
        AnnotationGeometry, AnnotationItem, AnnotationKind, AnnotationStyle,
    };
    use ::windows::Win32::Graphics::Direct2D::Common::D2D1_ALPHA_MODE_PREMULTIPLIED;
    use ::windows::Win32::Graphics::Direct2D::D2D1_BITMAP_OPTIONS_TARGET;
    use crate::geometry::{
        Handle, LabelWhere, Point, Rect, SizeLabelPlacement, size_label_placement,
    };

    /// A flat-coloured BGRA frame.
    fn solid_bgra(width: u32, height: u32, pixel: [u8; 4]) -> Vec<u8> {
        pixel
            .iter()
            .copied()
            .cycle()
            .take((width * height * 4) as usize)
            .collect()
    }

    fn pixel_at(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let offset = ((y * width + x) * 4) as usize;
        [
            pixels[offset],
            pixels[offset + 1],
            pixels[offset + 2],
            pixels[offset + 3],
        ]
    }

    /// The exact pixel the L1 mask produces for an opaque source pixel.
    ///
    /// Derived from the palette constants rather than hard-coded, so adding a layer to the mask (the
    /// theme tint, docs/21 §5.22) moves every assertion with it instead of breaking a dozen tests.
    /// Straight (non-premultiplied) blends; the readback is BGRA, so channel 0 is blue.
    fn masked(pixel: [u8; 4]) -> [u8; 4] {
        // The neutral mask is black at MASK_ALPHA…
        let masked = [0, 1, 2].map(|channel| pixel[channel] as f32 * (1.0 - MASK_ALPHA));
        // …and the theme tint is `#1f75db` at MASK_TINT_ALPHA, written in BGRA like a readback.
        const ACCENT_BGRA: [f32; 3] = [219.0, 117.0, 31.0];
        let out = [0, 1, 2].map(|channel| {
            masked[channel] * (1.0 - MASK_TINT_ALPHA) + ACCENT_BGRA[channel] * MASK_TINT_ALPHA
        });
        [
            out[0].round() as u8,
            out[1].round() as u8,
            out[2].round() as u8,
            255,
        ]
    }

    /// Compare a readback pixel with the palette's expected value, tolerating one unit per channel.
    ///
    /// The mask's layers are composited by Direct2D in premultiplied space, rounding at each stage,
    /// while `masked()` is a straight-alpha calculation — the two can disagree by one. Anything
    /// beyond that is a real difference and still fails.
    fn assert_pixel_close(actual: [u8; 4], expected: [u8; 4], what: &str) {
        for channel in 0..3 {
            assert!(
                (actual[channel] as i32 - expected[channel] as i32).abs() <= 1,
                "{what}: got {actual:?}, expected {expected:?}",
            );
        }
    }

    /// Visual regression for the L0/L1/L2 composition.
    ///
    /// Renders the exact composition the overlay presents into an offscreen target and
    /// reads it back, so the mask, the selection cut-out and the chrome are all
    /// verified against real GPU output rather than a mock.
    #[test]
    fn composed_frame_masks_outside_the_selection_and_keeps_it_clear() {
        let Ok(device) = super::GraphicsDevice::create() else {
            eprintln!("no D3D11 device in this session; skipping the composition check");
            return;
        };
        let width = 64u32;
        let height = 48u32;
        let background = [64u8, 96, 128, 255];
        let Ok(frame) = device.create_bgra_texture(width, height, &solid_bgra(width, height, background))
        else {
            return;
        };
        let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
            return;
        };
        renderer
            .update_frame(width, height, &solid_bgra(width, height, background))
            .unwrap();
        // The overlay creates its swap chain before the first draw; this also builds
        // the L1/L2 brushes the composition needs.
        renderer.ensure_back_buffer(width, height).unwrap();

        // A render target the test can read back.
        let target = renderer
            .device()
            .create_render_target_texture(width, height)
            .unwrap();
        let d2d_context = renderer.device().create_d2d_context().unwrap();
        let target_bitmap = super::super::d3d11::create_bitmap_from_texture(
            &d2d_context,
            &target.texture,
            D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
        .unwrap();

        let selection = Rect::new(16, 12, 48, 36);
        let mut view = RenderView::new(Rect::from_origin_size(
            Point::new(0, 0),
            width as i32,
            height as i32,
        ));
        view.show_chrome = false;
        view.cursor_visible = false;
        renderer.draw_to(&target_bitmap, &view).unwrap();
        let unselected = renderer.device().read_back_bgra(&target.texture).unwrap();
        // Before the first drag every pixel is masked; the frame shows through at the
        // mask's complement.
        assert_pixel_close(
            pixel_at(&unselected, width, 32, 24),
            masked(background),
            "an unselected frame must be uniformly masked",
        );

        view.selection = selection;
        view.cursor = Point::new(24, 18);
        // The magnifier draws cursor crosshairs across the whole frame, so it stays off
        // while the L0/L1 layers are checked.
        view.cursor_visible = false;
        view.show_chrome = true;

        renderer.draw_to(&target_bitmap, &view).unwrap();
        let composed = renderer.device().read_back_bgra(&target.texture).unwrap();

        // Inside the selection the L0 pixels survive untouched.
        let inside = pixel_at(&composed, width, 32, 24);
        assert_eq!(
            inside, background,
            "selected pixels must keep the raw back-buffer brightness"
        );

        // Outside the selection the mask darkens the frame by exactly MASK_ALPHA.
        let outside = pixel_at(&composed, width, 4, 4);
        assert!(
            outside[0] < background[0] && outside[1] < background[1] && outside[2] < background[2],
            "masked pixel {outside:?} must be darker than the frame {background:?}"
        );
        assert_pixel_close(
            outside,
            masked(background),
            "mask alpha must match the configured value",
        );

        let _ = frame;
    }

    /// The annotated export must be byte-identical to the crop of the exact
    /// composition the preview presents, with annotations baked in and the mask,
    /// selection chrome and control handles excluded (docs/11 §8.2/§8.3).
    ///
    /// `render_export` runs the same [`OverlayRenderer::draw_to`] layer path into a
    /// frame-sized offscreen target, then reads `selection` back. This renders the
    /// identical view through the swap-chain-style path (`draw_to` into a fresh
    /// render-target bitmap) and asserts the returned crop equals that full frame's
    /// selection region pixel-for-pixel, so a readback offset/pitch/format bug cannot
    /// pass. It then spot-checks that a red fill landed inside the artifact while a
    /// selected-but-unannotated pixel stayed at the raw frame brightness (no mask).
    #[test]
    fn export_crop_matches_preview_and_bakes_annotations() {
        let Ok(device) = super::GraphicsDevice::create() else {
            eprintln!("no D3D11 device in this session; skipping the export check");
            return;
        };
        let width = 64u32;
        let height = 48u32;
        let background = [64u8, 96, 128, 255];
        let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
            return;
        };
        renderer
            .update_frame(width, height, &solid_bgra(width, height, background))
            .unwrap();
        renderer.ensure_back_buffer(width, height).unwrap();

        // A preview target we can read the whole frame back from.
        let target = renderer
            .device()
            .create_render_target_texture(width, height)
            .unwrap();
        let d2d_context = renderer.device().create_d2d_context().unwrap();
        let target_bitmap = super::super::d3d11::create_bitmap_from_texture(
            &d2d_context,
            &target.texture,
            D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
        .unwrap();

        let frame = Rect::from_origin_size(Point::new(0, 0), width as i32, height as i32);
        let selection = Rect::new(16, 12, 48, 36);
        // A solid red filled rectangle sitting wholly inside the selection.
        let item = AnnotationItem {
            id: 1,
            kind: AnnotationKind::Rectangle,
            geometry: AnnotationGeometry::Rect {
                bounds: Rect::new(20, 16, 40, 30),
            },
            style: AnnotationStyle {
                stroke_color: [1.0, 0.0, 0.0, 1.0],
                fill_color: Some([1.0, 0.0, 0.0, 1.0]),
                ..AnnotationStyle::default()
            },
        };

        // The view the export path uses: chrome/cursor/selection-box/draft all off.
        let mut view = RenderView::new(frame);
        view.selection = selection;
        view.show_chrome = false;
        view.cursor_visible = false;
        view.annotation_items = vec![item];
        view.annotation_selected_id = None;
        view.annotation_draft = None;

        // Reference: present the same view and read the whole frame back.
        renderer.draw_to(&target_bitmap, &view).unwrap();
        let full = renderer.device().read_back_bgra(&target.texture).unwrap();

        // The artifact crop.
        let crop_w = selection.width() as u32;
        let crop_h = selection.height() as u32;
        let export = renderer.render_export(&view).unwrap();
        assert_eq!(
            export.len(),
            (crop_w * crop_h * 4) as usize,
            "export must be exactly the selection region, tightly packed"
        );

        // Every exported pixel equals the preview's same location: same code path,
        // correct row pitch and offset.
        for y in 0..crop_h {
            for x in 0..crop_w {
                let preview = pixel_at(&full, width, selection.left as u32 + x, selection.top as u32 + y);
                let artifact = pixel_at(&export, crop_w, x, y);
                assert_eq!(
                    artifact, preview,
                    "export pixel ({x},{y}) must match the presented preview crop"
                );
            }
        }

        // The annotation is baked into the artifact (not just the preview):
        // a fill-interior pixel is red, not the blue frame.
        let red = pixel_at(&export, crop_w, (30 - selection.left) as u32, (23 - selection.top) as u32);
        assert!(
            red[2] > 200 && red[0] < 60 && red[1] < 60,
            "annotation fill must appear in the export, got {red:?}"
        );

        // Selected-but-unannotated pixels keep the raw frame brightness: the L1 mask
        // and the L3 selection chrome never enter the artifact.
        let clear = pixel_at(&export, crop_w, 1, 1);
        assert_eq!(
            clear, background,
            "selected, unannotated pixels must stay raw (no mask/chrome)"
        );
    }

    /// The chrome is preview-only and must not touch the L0 pixels inside the
    /// selection; the border itself is drawn on the selection edge.
    #[test]
    fn chrome_stays_out_of_the_exported_pixels() {
        let Ok(device) = super::GraphicsDevice::create() else {
            return;
        };
        let width = 64u32;
        let height = 48u32;
        let background = [200u8, 180, 160, 255];
        let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
            return;
        };
        renderer
            .update_frame(width, height, &solid_bgra(width, height, background))
            .unwrap();
        renderer.ensure_back_buffer(width, height).unwrap();
        let target = renderer
            .device()
            .create_render_target_texture(width, height)
            .unwrap();
        let d2d_context = renderer.device().create_d2d_context().unwrap();
        let bitmap = super::super::d3d11::create_bitmap_from_texture(
            &d2d_context,
            &target.texture,
            D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
        .unwrap();

        let mut view = RenderView::new(Rect::from_origin_size(
            Point::new(0, 0),
            width as i32,
            height as i32,
        ));
        view.selection = Rect::new(16, 12, 48, 36);
        // The label prefers to sit below the selection, so chrome draws over some
        // selected pixels; the check below samples one it cannot cover.
        view.show_chrome = true;
        view.cursor_visible = false;

        renderer.draw_to(&bitmap, &view).unwrap();
        let composed = renderer.device().read_back_bgra(&target.texture).unwrap();

        // Inside the selection, at least one row stays untouched by the border and the
        // grips. The label cannot fit inside a frame this small, so it must be dropped
        // rather than painted over the selection.
        let preserved = (13..35).any(|y| pixel_at(&composed, width, 32, y) == background);
        assert!(
            preserved,
            "selected pixels must reach the output unmodified somewhere in the selection"
        );
    }

    /// Hover and preview are painted as their own layers, and neither is baked into the
    /// exported pixels (docs/14 §8).
    #[test]
    fn window_snap_hints_are_painted_but_never_exported() {
        let Ok(device) = super::GraphicsDevice::create() else {
            return;
        };
        let width = 64u32;
        let height = 48u32;
        let background = [200u8, 180, 160, 255];
        let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
            return;
        };
        renderer
            .update_frame(width, height, &solid_bgra(width, height, background))
            .unwrap();
        renderer.ensure_back_buffer(width, height).unwrap();
        let target = renderer
            .device()
            .create_render_target_texture(width, height)
            .unwrap();
        let d2d_context = renderer.device().create_d2d_context().unwrap();
        let bitmap = super::super::d3d11::create_bitmap_from_texture(
            &d2d_context,
            &target.texture,
            D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
        .unwrap();

        let mut view = RenderView::new(Rect::from_origin_size(
            Point::new(0, 0),
            width as i32,
            height as i32,
        ));
        view.cursor_visible = false;
        let window = Rect::new(8, 8, 40, 32);
        let sample = |pixels: &[u8], x: u32, y: u32| pixel_at(pixels, width, x, y);

        // Baseline: no hint at all, so the whole frame is masked down.
        renderer.draw_to(&bitmap, &view).unwrap();
        let bare = renderer.device().read_back_bgra(&target.texture).unwrap();
        let masked = sample(&bare, 20, 20);

        // Hover: the window is lifted out of the mask, everything else is untouched.
        view.hover_bounds = Some(window);
        renderer.draw_to(&bitmap, &view).unwrap();
        let hovered = renderer.device().read_back_bgra(&target.texture).unwrap();
        let hover_pixel = sample(&hovered, 20, 20);
        assert_ne!(hover_pixel, masked, "the hover wash must lighten the window");
        assert_eq!(
            sample(&hovered, 60, 44),
            masked,
            "the mask outside the hovered window must not change"
        );

        // Preview: its own accent wash, distinct from both the mask and the hover wash.
        view.hover_bounds = None;
        view.preview_bounds = Some(window);
        renderer.draw_to(&bitmap, &view).unwrap();
        let previewed = renderer.device().read_back_bgra(&target.texture).unwrap();
        let preview_pixel = sample(&previewed, 20, 20);
        assert_ne!(preview_pixel, masked, "the preview must be visible");
        assert_ne!(
            preview_pixel, hover_pixel,
            "the preview must be distinguishable from the hover hint"
        );

        // Deep-selection ancestors are outlined so the nesting is visible; the level that is
        // also the emphasised rectangle is skipped instead of being stroked twice.
        let ancestor = Rect::new(4, 4, 44, 36);
        view.hover_bounds = None;
        view.preview_bounds = Some(window);
        view.chain_rings = vec![
            ChainRingView { rect: ancestor, inner: false, alpha: 0.6 },
            ChainRingView { rect: window, inner: false, alpha: 0.6 },
        ];
        renderer.draw_to(&bitmap, &view).unwrap();
        let with_path = renderer.device().read_back_bgra(&target.texture).unwrap();
        assert_ne!(
            sample(&with_path, 24, 4),
            masked,
            "an ancestor level must be outlined"
        );

        // The ring is painted here; *how legible* it is belongs to
        // `chain_rings_carry_luminance_on_light_and_dark_content`, which scans the whole stroke —
        // a ring is a bright core plus a dark underlay, so one arbitrary pixel can legitimately sit
        // on either side of the background.
        let stroke: Vec<[u8; 4]> = (0..8).map(|y| sample(&with_path, 24, y)).collect();
        assert!(
            stroke.iter().any(|pixel| *pixel != masked),
            "an ancestor level must actually be painted: {stroke:?}",
        );

        // Inner rings are painted *above* the preview wash — the whole point of the two-pass order,
        // and what the prototype got wrong first (an accent line under an accent wash is invisible).
        //
        // The measurable consequence of the order: with the wash on, the ring's own pixels stay
        // (nearly) unchanged, because nothing is painted over them. Painted underneath, the wash
        // would tint every ring pixel. So draw the same inner ring twice — wash off, wash on — and
        // compare the ring's own brightest pixel. Everything else is held constant, so the wash is
        // the only difference between the two frames (docs/21 §5.24, A3: it exists only while the
        // walk is live, which is exactly what `capture_green` carries).
        let inner = Rect::new(12, 12, 36, 28);
        view.hover_bounds = None;
        view.chain_rings = vec![ChainRingView {
            rect: inner,
            inner: true,
            alpha: 0.9,
        }];
        let brightest = |pixels: &[u8]| {
            (6..22)
                .map(|y| sample(pixels, 24, y))
                .max_by_key(|pixel| pixel[0] as i32 + pixel[1] as i32 + pixel[2] as i32)
                .unwrap()
        };
        view.preview_bounds = Some(window);
        view.capture_green = 0.0;
        renderer.draw_to(&bitmap, &view).unwrap();
        let bare = renderer.device().read_back_bgra(&target.texture).unwrap();
        let bare_ring = brightest(&bare);
        view.capture_green = 1.0;
        renderer.draw_to(&bitmap, &view).unwrap();
        let with_wash = renderer.device().read_back_bgra(&target.texture).unwrap();
        let washed_ring = brightest(&with_wash);
        assert_ne!(
            sample(&with_wash, 24, 22),
            sample(&bare, 24, 22),
            "the preview wash has to be painted for this comparison to mean anything",
        );
        let drift: i32 = (0..3)
            .map(|channel| (bare_ring[channel] as i32 - washed_ring[channel] as i32).abs())
            .sum();
        assert!(
            drift <= 30,
            "the inner ring moved by {drift} under the wash ({bare_ring:?} vs {washed_ring:?}): \
             it is being painted underneath it",
        );

        // Exporting the previewed rectangle must produce raw frozen pixels: a hint is a
        // hint, and it must never reach the artifact. The preview here is deliberately *not* the
        // exported rectangle, so the mask hole (docs/21 §5.24, A2) cannot be what saves it — the
        // export path has to clear the hint, and with it the hole.
        view.hover_bounds = Some(window);
        view.preview_bounds = Some(Rect::new(4, 4, 44, 36));
        view.capture_green = 1.0;
        view.selection = window;
        let exported = renderer.render_export(&view).unwrap();
        assert_eq!(exported.len(), (window.width() * window.height() * 4) as usize);
        assert!(
            exported
                .chunks_exact(4)
                .all(|pixel| pixel == background.as_slice()),
            "hover/preview hints must never be exported"
        );
    }

    /// A2 (docs/21 §5.24): the box that would be taken shows its own pixels — no mask, no tint —
    /// and the whole-window fallback does not, because its "box" is the screen.
    #[test]
    fn an_element_preview_keeps_its_own_pixels_and_the_window_fallback_does_not() {
        let Ok(device) = super::GraphicsDevice::create() else {
            return;
        };
        let width = 96u32;
        let height = 64u32;
        let background = [210u8, 200, 190, 255];
        let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
            return;
        };
        renderer
            .update_frame(width, height, &solid_bgra(width, height, background))
            .unwrap();
        renderer.ensure_back_buffer(width, height).unwrap();
        let target = renderer
            .device()
            .create_render_target_texture(width, height)
            .unwrap();
        let d2d_context = renderer.device().create_d2d_context().unwrap();
        let bitmap = super::super::d3d11::create_bitmap_from_texture(
            &d2d_context,
            &target.texture,
            D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
        .unwrap();

        let mut view = RenderView::new(Rect::from_origin_size(
            Point::new(0, 0),
            width as i32,
            height as i32,
        ));
        view.cursor_visible = false;
        let element = Rect::new(20, 16, 70, 48);

        // Baseline: no hint at all, so the whole frame is masked down.
        renderer.draw_to(&bitmap, &view).unwrap();
        let bare = renderer.device().read_back_bgra(&target.texture).unwrap();
        let masked = pixel_at(&bare, width, 40, 30);
        let masked_corner = pixel_at(&bare, width, 8, 8);

        // An element preview: the content inside it is the frozen frame's own pixel, and the pixels
        // outside it are exactly the ones the bare frame had — the hole moves nothing else.
        view.preview_bounds = Some(element);
        renderer.draw_to(&bitmap, &view).unwrap();
        let previewed = renderer.device().read_back_bgra(&target.texture).unwrap();
        assert_eq!(
            pixel_at(&previewed, width, 40, 30),
            background,
            "an element preview must show its own pixels"
        );
        assert_eq!(
            pixel_at(&previewed, width, 8, 8),
            masked_corner,
            "the mask outside the box must be untouched"
        );
        assert_ne!(
            pixel_at(&previewed, width, 40, 30),
            masked,
            "which is the whole point: the box being chosen is not a darkened picture of itself"
        );

        // …and a box inside a hovered window keeps its own pixels too: the hover wash is cut around
        // the same hole. It lifts the hovered window out of the mask; it must not veil the pixels
        // that are about to be taken, which is the whole reason the hole exists.
        let hover = Rect::new(4, 4, 88, 60);
        view.hover_bounds = Some(hover);
        renderer.draw_to(&bitmap, &view).unwrap();
        let hovered = renderer.device().read_back_bgra(&target.texture).unwrap();
        assert_eq!(
            pixel_at(&hovered, width, 40, 30),
            background,
            "the hole has to survive the hover wash"
        );
        assert_ne!(
            pixel_at(&hovered, width, 8, 8),
            masked_corner,
            "while the rest of the hovered window is still washed"
        );

        // The whole-window fallback is the neutral answer: same rectangle, no hole, and it reads as
        // the hover wash with a thin outline. A hole the size of the window would erase the mask,
        // and with it the reading that capture mode is on.
        view.preview_is_window = true;
        renderer.draw_to(&bitmap, &view).unwrap();
        let fallback = renderer.device().read_back_bgra(&target.texture).unwrap();
        let fallback_pixel = pixel_at(&fallback, width, 40, 30);
        assert_ne!(
            fallback_pixel, background,
            "the whole-window fallback must not punch a hole in the mask"
        );
        // …and it is a *wash over the mask* rather than the raw pixels — the neutral answer lights
        // the box the way the hovered window is lit (one wash stronger, since the hover fills the
        // same rectangle).
        assert!(
            (0..3).all(|c| fallback_pixel[c] > masked[c]),
            "the fallback has to read as a wash over the mask: {fallback_pixel:?} vs {masked:?}"
        );

        // …and with no preview at all the hole is the settled selection, exactly as before.
        view.preview_is_window = false;
        view.hover_bounds = None;
        view.preview_bounds = None;
        view.selection = element;
        renderer.draw_to(&bitmap, &view).unwrap();
        let selected = renderer.device().read_back_bgra(&target.texture).unwrap();
        assert_eq!(pixel_at(&selected, width, 40, 30), background);
    }

    /// A3 (docs/21 §5.24): the capture box is brand blue with no wash at rest, and the capture green
    /// with a wash while the level walk is live — the colour says "you moved", not "this is the one".
    #[test]
    fn the_capture_box_is_blue_at_rest_and_green_while_walking() {
        // The two ends are the palette entries themselves, so the box's colour cannot drift away
        // from the brushes the rest of the overlay uses.
        let rest = capture_mix_color(0.0, 1.0);
        let walking = capture_mix_color(1.0, 1.0);
        let (accent_r, _, accent_b) = ACCENT_RGB.channels_f32();
        let (capture_r, capture_g, _) = CAPTURE_RGB.channels_f32();
        assert!(
            (rest.r - accent_r).abs() < 1e-6 && (rest.b - accent_b).abs() < 1e-6,
            "at rest the box is the brand blue"
        );
        assert!(
            (walking.r - capture_r).abs() < 1e-6 && (walking.g - capture_g).abs() < 1e-6,
            "walking the box is the capture green"
        );
        // Nothing is washed at rest: the hole already shows the content at its own brightness, and a
        // wash on top of it would only tint the pixels the user is trying to judge.
        assert_eq!(capture_wash_color(0.0, 1.0).a, 0.0);
        assert!(
            (capture_wash_color(1.0, 1.0).a - PREVIEW_WASH_ALPHA * PREVIEW_WASH_WALK_SCALE).abs()
                < 1e-6
        );
        // ②: the box's own fade-in rides on the same two colours, so "how strongly is this outline
        // on screen" is one number rather than a second brush.
        assert!((capture_mix_color(1.0, 0.0).a).abs() < 1e-6);
        assert!(
            (capture_wash_color(1.0, 0.5).a
                - PREVIEW_WASH_ALPHA * PREVIEW_WASH_WALK_SCALE * 0.5)
                .abs()
                < 1e-6
        );

        let Ok(device) = super::GraphicsDevice::create() else {
            return;
        };
        let width = 96u32;
        let height = 64u32;
        let background = [128u8, 128, 128, 255];
        let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
            return;
        };
        renderer
            .update_frame(width, height, &solid_bgra(width, height, background))
            .unwrap();
        renderer.ensure_back_buffer(width, height).unwrap();
        let target = renderer
            .device()
            .create_render_target_texture(width, height)
            .unwrap();
        let d2d_context = renderer.device().create_d2d_context().unwrap();
        let bitmap = super::super::d3d11::create_bitmap_from_texture(
            &d2d_context,
            &target.texture,
            D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
        .unwrap();

        let mut view = RenderView::new(Rect::from_origin_size(
            Point::new(0, 0),
            width as i32,
            height as i32,
        ));
        view.cursor_visible = false;
        view.hover_bounds = None;
        view.preview_bounds = Some(Rect::new(20, 16, 70, 48));

        // The stroke's own pixel: the column crosses the box's top edge, and the most covered pixel
        // is the one furthest from the content behind it. Anti-aliasing means no pixel is exactly
        // the brush colour, so the assertion is which of the two ends it is nearer to.
        let stroke_sample = |pixels: &[u8]| {
            (14..24)
                .map(|y| pixel_at(pixels, width, 48, y))
                .max_by_key(|pixel| {
                    (0..3)
                        .map(|c| (pixel[c] as i32 - background[c] as i32).abs())
                        .sum::<i32>()
                })
                .unwrap()
        };
        let distance_to = |pixel: [u8; 4], rgb: Rgb| {
            // Readback is BGRA; the palette is RGB.
            let want = [rgb.b as i32, rgb.g as i32, rgb.r as i32];
            (0..3)
                .map(|c| (pixel[c] as i32 - want[c]).abs())
                .sum::<i32>()
        };

        view.capture_green = 0.0;
        renderer.draw_to(&bitmap, &view).unwrap();
        let at_rest = renderer.device().read_back_bgra(&target.texture).unwrap();
        let rest_stroke = stroke_sample(&at_rest);
        view.capture_green = 1.0;
        renderer.draw_to(&bitmap, &view).unwrap();
        let walking_frame = renderer.device().read_back_bgra(&target.texture).unwrap();
        let walking_stroke = stroke_sample(&walking_frame);

        assert_ne!(rest_stroke, walking_stroke, "the walk must change the box");
        assert!(
            distance_to(rest_stroke, ACCENT_RGB) < distance_to(rest_stroke, CAPTURE_RGB),
            "at rest the box is the brand blue: {rest_stroke:?}"
        );
        assert!(
            distance_to(walking_stroke, CAPTURE_RGB) < distance_to(walking_stroke, ACCENT_RGB),
            "walking the box is the capture green: {walking_stroke:?}"
        );
    }

    /// A3's cost, measured rather than argued (docs/21 §5.24): at rest the capture box is the brand
    /// blue, so on light and dark content the box reads by *luminance* and on a mid grey only by hue.
    ///
    /// The hole is the primary cue either way — the whole box is 45% brighter than the mask around it
    /// — but the stroke has to be a line, not a rumour. Light and dark are gated; the mid case is
    /// printed, because no single blue can be a luminance step against a background of its own
    /// brightness and pretending otherwise would just move the failure somewhere else.
    #[test]
    fn the_resting_capture_box_is_a_luminance_step_on_light_and_dark_content() {
        let relative_luminance = |pixel: [u8; 4]| -> f32 {
            // Readback is BGRA.
            let channel = |value: u8| {
                let value = value as f32 / 255.0;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(pixel[2]) + 0.7152 * channel(pixel[1]) + 0.0722 * channel(pixel[0])
        };
        let ratio = |a: [u8; 4], b: [u8; 4]| {
            let (high, low) = {
                let (a, b) = (relative_luminance(a), relative_luminance(b));
                if a > b { (a, b) } else { (b, a) }
            };
            (high + 0.05) / (low + 0.05)
        };

        for (name, background, gated) in [
            ("light", [250u8, 250, 250, 255], true),
            ("mid", [128, 128, 128, 255], false),
            ("dark", [24, 24, 28, 255], true),
        ] {
            let Ok(device) = super::GraphicsDevice::create() else {
                return;
            };
            let width = 96u32;
            let height = 64u32;
            let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
                return;
            };
            renderer
                .update_frame(width, height, &solid_bgra(width, height, background))
                .unwrap();
            renderer.ensure_back_buffer(width, height).unwrap();
            let target = renderer
                .device()
                .create_render_target_texture(width, height)
                .unwrap();
            let d2d_context = renderer.device().create_d2d_context().unwrap();
            let bitmap = super::super::d3d11::create_bitmap_from_texture(
                &d2d_context,
                &target.texture,
                D2D1_BITMAP_OPTIONS_TARGET,
                D2D1_ALPHA_MODE_PREMULTIPLIED,
            )
            .unwrap();

            let mut view = RenderView::new(Rect::from_origin_size(
                Point::new(0, 0),
                width as i32,
                height as i32,
            ));
            view.cursor_visible = false;
            // The real situation: the box is inside a hovered window, so the pixels *outside* the
            // hole are masked and washed while the pixels inside it are the frame's own.
            view.hover_bounds = Some(Rect::new(4, 4, 88, 60));
            view.preview_bounds = Some(Rect::new(20, 16, 70, 48));
            view.capture_green = 0.0;
            renderer.draw_to(&bitmap, &view).unwrap();
            let pixels = renderer.device().read_back_bgra(&target.texture).unwrap();

            let inside = pixel_at(&pixels, width, 48, 30);
            let outside = pixel_at(&pixels, width, 48, 8);
            // The column crosses the box's top edge; the stroke is anti-aliased across a couple of
            // rows, and what the eye uses is whichever side of it is stronger.
            let best = (12..24)
                .map(|y| {
                    let pixel = pixel_at(&pixels, width, 48, y);
                    ratio(pixel, inside).max(ratio(pixel, outside))
                })
                .fold(0.0, f32::max);
            eprintln!(
                "[capture box] {name}: stroke peaks at {best:.2}:1 (content {inside:?} / mask \
                 {outside:?})"
            );
            if gated {
                assert!(
                    best >= 2.5,
                    "at rest the capture box peaks at {best:.2}:1 on {name} content",
                );
            }
        }
    }

    /// The level badge reads as two directions, and the one with nothing left is dimmed (v3 A1,
    /// docs/21 §5.24).
    ///
    /// Scans the chip's centre row and classifies every pixel: `G` = the capture green (a live
    /// arrow), `W` = the label white (its count), `d` = the dimmed colour, `.` = the panel and its
    /// hairlines. What has to hold: each live direction contributes a green arrow and a white count,
    /// they sit on their own side of the divider, and a pinned direction contributes neither.
    #[test]
    fn the_level_badge_reads_as_two_directions_with_the_pinned_one_dimmed() {
        let Ok(device) = super::GraphicsDevice::create() else {
            return;
        };
        let width = 240u32;
        let height = 120u32;
        let background = [40u8, 44, 52, 255];
        let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
            return;
        };
        renderer
            .update_frame(width, height, &solid_bgra(width, height, background))
            .unwrap();
        renderer.ensure_back_buffer(width, height).unwrap();
        let target = renderer
            .device()
            .create_render_target_texture(width, height)
            .unwrap();
        let context = renderer.device().create_d2d_context().unwrap();
        let bitmap = super::super::d3d11::create_bitmap_from_texture(
            &context,
            &target.texture,
            D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
        .unwrap();

        let mut view = RenderView::new(Rect::from_origin_size(
            Point::new(0, 0),
            width as i32,
            height as i32,
        ));
        view.cursor_visible = false;
        view.show_chrome = false;
        // A box big enough for the badge to sit inside it, selected at level 6 of 9.
        view.preview_bounds = Some(Rect::new(20, 20, 220, 100));

        // The chip sits in the box's top-right corner, 6 px in.
        let (badge_w, badge_h) = crate::geometry::LEVEL_BADGE_SIZE;
        let badge = Rect::new(220 - 6 - badge_w, 20 + 6, 220 - 6, 20 + 6 + badge_h);
        let y = (badge.top + badge.height() / 2) as u32;
        // Classify the chip's centre row, one entry per pixel: `G` = capture green, `W` = label
        // white, `d` = the dimmed colour, `.` = the panel and its hairlines. White text sums to
        // ~765, the dimmed colour to ~230 and the panel to ~75, so the two thresholds sit between
        // them; green is tested first because its sum lands in the middle of that range.
        let scan = |pixels: &[u8]| -> Vec<char> {
            (badge.left..badge.right)
                .map(|x| {
                    let pixel = pixel_at(pixels, width, x as u32, y);
                    let sum: i32 = (0..3).map(|channel| pixel[channel] as i32).sum();
                    let green = pixel[1] as i32 > pixel[0] as i32 + 20 && pixel[1] > pixel[2];
                    if green {
                        'G'
                    } else if sum > 450 {
                        'W'
                    } else if sum > 200 {
                        'd'
                    } else {
                        '.'
                    }
                })
                .collect()
        };
        // One glyph can be several runs of pixels on a single row (an arrow's head is two arms and a
        // shaft), so "how many arrows" is a question about *clusters*: pixels within 4 px of each
        // other belong to the same glyph.
        let clusters = |kinds: &[char], want: char| -> Vec<usize> {
            let mut starts = Vec::new();
            let mut last: Option<usize> = None;
            for (index, kind) in kinds.iter().enumerate() {
                if *kind != want {
                    continue;
                }
                if last.is_none_or(|at| index - at > 4) {
                    starts.push(index);
                }
                last = Some(index);
            }
            starts
        };
        let half = (badge.width() / 2) as usize;
        let pixels_of = |kinds: &[char], want: char, left: bool| -> usize {
            kinds
                .iter()
                .enumerate()
                .filter(|(index, kind)| **kind == want && (*index < half) == left)
                .count()
        };

        // Both directions live: a green arrow and a white count on each side of the divider.
        view.level_badge = Some(LevelReach { up: 3, down: 5 });
        renderer.draw_to(&bitmap, &view).unwrap();
        let pixels = renderer.device().read_back_bgra(&target.texture).unwrap();
        let kinds = scan(&pixels);
        let arrows = clusters(&kinds, 'G');
        assert_eq!(arrows.len(), 2, "two live directions: {kinds:?}");
        assert!(
            arrows[0] < half && arrows[1] > half,
            "one arrow per half: {kinds:?}"
        );
        assert!(
            pixels_of(&kinds, 'W', true) > 0 && pixels_of(&kinds, 'W', false) > 0,
            "each live direction carries its count: {kinds:?}"
        );

        // Already against the window frame (`↑0`): the left half is dim, the right half still runs.
        view.level_badge = Some(LevelReach { up: 0, down: 5 });
        renderer.draw_to(&bitmap, &view).unwrap();
        let pinned = renderer.device().read_back_bgra(&target.texture).unwrap();
        let kinds = scan(&pinned);
        let arrows = clusters(&kinds, 'G');
        assert_eq!(
            arrows.len(),
            1,
            "only the live direction keeps the capture green: {kinds:?}"
        );
        assert!(
            arrows[0] > half,
            "and it is the right-hand direction: {kinds:?}"
        );
        assert!(
            pixels_of(&kinds, 'W', true) == 0,
            "the pinned direction must not keep the label white: {kinds:?}"
        );
        assert!(
            pixels_of(&kinds, 'd', true) > 0,
            "the pinned direction still has to say `0`, dimmed: {kinds:?}"
        );
    }

    /// The regression the user found on a real screen: a mid-tone blue ring on a **masked light**
    /// page. The mask (45% black) lands a white page at ~140 grey, whose luminance is nearly that of
    /// a mid blue — so the first ring colour differed in hue and not in brightness and could not be
    /// seen. This asserts the ring's *luminance* against the masked background, not just that it
    /// changed, and does it on light and dark content alike.
    #[test]
    fn chain_rings_carry_luminance_on_light_and_dark_content() {
        use crate::geometry::Point as GPoint;

        let relative_luminance = |pixel: [u8; 4]| -> f32 {
            // Readback is BGRA; the eye weighs green most.
            let channel = |value: u8| {
                let value = value as f32 / 255.0;
                if value <= 0.03928 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(pixel[2]) + 0.7152 * channel(pixel[1]) + 0.0722 * channel(pixel[0])
        };
        let ratio = |a: [u8; 4], b: [u8; 4]| {
            let (high, low) = {
                let (a, b) = (relative_luminance(a), relative_luminance(b));
                if a > b { (a, b) } else { (b, a) }
            };
            (high + 0.05) / (low + 0.05)
        };

        for (name, background, inner) in [
            ("light", [250u8, 250, 250, 255], false),
            ("mid", [128, 128, 128, 255], false),
            ("dark", [24, 24, 28, 255], false),
            // The inner rings' actual situation: inside the capture wash, on light content — the
            // combination the user is looking at when the wheel has walked up a level.
            ("light + capture wash", [250, 250, 250, 255], true),
        ] {
            let Ok(device) = super::GraphicsDevice::create() else {
                return;
            };
            let width = 96u32;
            let height = 64u32;
            // The user's monitor: at 144 DPI the ring is 1.5 physical px, which is the case that has
            // to read. (The first version drew a raw physical pixel — thinner here than in any
            // prototype, which ran at 96 DPI.)
            let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 144) else {
                return;
            };
            renderer
                .update_frame(width, height, &solid_bgra(width, height, background))
                .unwrap();
            renderer.ensure_back_buffer(width, height).unwrap();
            let target = renderer
                .device()
                .create_render_target_texture(width, height)
                .unwrap();
            let context = renderer.device().create_d2d_context().unwrap();
            let bitmap = super::super::d3d11::create_bitmap_from_texture(
                &context,
                &target.texture,
                D2D1_BITMAP_OPTIONS_TARGET,
                D2D1_ALPHA_MODE_PREMULTIPLIED,
            )
            .unwrap();

            let mut view = RenderView::new(Rect::from_origin_size(
                GPoint::new(0, 0),
                width as i32,
                height as i32,
            ));
            view.cursor_visible = false;
            view.show_chrome = false;
            // The ring sits well inside the frame; the sample below is 6 px below it, i.e. masked
            // content with nothing else painted on it.
            let ring = Rect::new(20, 20, 76, 48);
            if inner {
                view.preview_bounds = Some(Rect::new(12, 12, 84, 56));
            }
            view.chain_rings = vec![ChainRingView {
                rect: ring,
                inner,
                alpha: 1.0, // the floor of the ramp, i.e. the faintest ring the plan can produce
            }];
            renderer.draw_to(&bitmap, &view).unwrap();
            let pixels = renderer.device().read_back_bgra(&target.texture).unwrap();
            // Scan the column through the ring's top edge: a 1.5 px line plus its 2.5 px underlay
            // are anti-aliased across a few rows, and what the eye uses is the *pair* — a bright
            // core with a dark edge. Sampling one arbitrary row measures neither.
            let column: Vec<[u8; 4]> = (16..26)
                .map(|y| pixel_at(&pixels, width, 48, y))
                .collect();
            let plain = pixel_at(&pixels, width, 48, 40);
            let brightest = *column
                .iter()
                .max_by(|a, b| relative_luminance(**a).total_cmp(&relative_luminance(**b)))
                .unwrap();
            let darkest = *column
                .iter()
                .min_by(|a, b| relative_luminance(**a).total_cmp(&relative_luminance(**b)))
                .unwrap();
            eprintln!(
                "[ring] {name}: brightest {brightest:?} {:.2}:1 · darkest {darkest:?} {:.2}:1 · \
                 background {plain:?}",
                ratio(brightest, plain),
                ratio(darkest, plain),
            );
            // The pair is what makes it legible, so the assertion is on the *better* side: on light
            // content the dark edge carries it (3.4:1 there, 1.5:1 for the core), on mid and dark
            // content the light core does (3.6:1 and 7.0:1). Requiring both sides would demand a
            // colour that no screenshot can ever match.
            let best = ratio(brightest, plain).max(ratio(darkest, plain));
            assert!(
                best >= 2.5,
                "on {name} content the ring peaks at {best:.2}:1 ({brightest:?} core / {darkest:?} \
                 edge against {plain:?})",
            );
        }
    }

    /// The size label is painted with an opaque panel above the selection when there is room.
    #[test]
    fn size_label_panel_is_painted_at_the_selection_top_left() {
        let Ok(device) = super::GraphicsDevice::create() else {
            return;
        };
        let width = 640u32;
        let height = 480u32;
        let background = [200u8, 180, 160, 255];
        let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
            return;
        };
        renderer
            .update_frame(width, height, &solid_bgra(width, height, background))
            .unwrap();
        renderer.ensure_back_buffer(width, height).unwrap();
        let target = renderer
            .device()
            .create_render_target_texture(width, height)
            .unwrap();
        let d2d_context = renderer.device().create_d2d_context().unwrap();
        let bitmap = super::super::d3d11::create_bitmap_from_texture(
            &d2d_context,
            &target.texture,
            D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
        .unwrap();

        let mut view = RenderView::new(Rect::from_origin_size(
            Point::new(0, 0),
            width as i32,
            height as i32,
        ));
        let selection = Rect::new(200, 200, 400, 280);
        view.selection = selection;
        view.show_chrome = true;
        view.cursor_visible = false;

        renderer.draw_to(&bitmap, &view).unwrap();
        let composed = renderer.device().read_back_bgra(&target.texture).unwrap();

        // Above the selection, aligned with its left edge, the label panel replaces
        // the masked frame with an opaque, high-contrast surface.
        let label_row = selection.top - 12;
        let panel = pixel_at(
            &composed,
            width,
            (selection.left + 12) as u32,
            label_row as u32,
        );
        assert_ne!(
            panel,
            masked(background),
            "the label panel must be painted at the selection top-left"
        );
        assert_eq!(panel[3], 255, "the label panel must be fully opaque");
    }

    /// Regression for the drag ghost: every present repaints the whole surface, so a
    /// moving selection cannot leave the previous frame's chrome behind. The size label
    /// and grips are painted partly *outside* the selection rect, so a partial repaint
    /// that only covered the raw selection would strand the old label — the exact ghost
    /// this guards against.
    #[test]
    fn moving_the_selection_erases_the_previous_chrome() {
        let Ok(device) = super::GraphicsDevice::create() else {
            return;
        };
        let width = 640u32;
        let height = 480u32;
        let background = [200u8, 180, 160, 255];
        let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
            return;
        };
        let frame = Rect::from_origin_size(Point::new(0, 0), width as i32, height as i32);
        renderer
            .update_frame(width, height, &solid_bgra(width, height, background))
            .unwrap();
        renderer.ensure_back_buffer(width, height).unwrap();
        let target = renderer
            .device()
            .create_render_target_texture(width, height)
            .unwrap();
        let d2d_context = renderer.device().create_d2d_context().unwrap();
        let bitmap = super::super::d3d11::create_bitmap_from_texture(
            &d2d_context,
            &target.texture,
            D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
        .unwrap();

        let selection_a = Rect::new(120, 120, 240, 200);
        let selection_b = Rect::new(360, 360, 480, 440);

        // Frame A: selection at A, chrome drawn.
        let mut view_a = RenderView::new(frame);
        view_a.selection = selection_a;
        view_a.show_chrome = true;
        view_a.cursor_visible = false;
        renderer.draw_to(&bitmap, &view_a).unwrap();
        let after_a = renderer.device().read_back_bgra(&target.texture).unwrap();

        // A probe on A's size label, above the top-left — outside the raw selection.
        let probe = Point::new(selection_a.left + 12, selection_a.top - 12);
        assert_ne!(
            pixel_at(&after_a, width, probe.x as u32, probe.y as u32),
            masked(background),
            "frame A must paint the size label above the selection"
        );

        // Frame B: move the selection far away. Because the whole surface is repainted,
        // the previous chrome is gone — a stale back buffer (the flip-model ghost) would
        // leave the label behind.
        let mut view_b = RenderView::new(frame);
        view_b.selection = selection_b;
        view_b.show_chrome = true;
        view_b.cursor_visible = false;
        renderer.draw_to(&bitmap, &view_b).unwrap();
        let after_b = renderer.device().read_back_bgra(&target.texture).unwrap();

        assert_pixel_close(
            pixel_at(&after_b, width, probe.x as u32, probe.y as u32),
            masked(background),
            "moving the selection must erase the previous chrome (ghost regression)",
        );
    }

    #[test]
    fn metrics_scale_with_dpi() {
        let at_100 = RenderMetrics::for_dpi(96);
        let at_200 = RenderMetrics::for_dpi(192);
        assert!((at_200.border_width - at_100.border_width * 2.0).abs() < 0.01);
        assert!((at_200.handle_size - at_100.handle_size * 2.0).abs() < 0.01);
        assert!((at_200.label_font_size - at_100.label_font_size * 2.0).abs() < 0.01);
        // Border stays visible even at 100%: never below one physical pixel.
        assert!(at_100.border_width >= 1.0);
    }

    #[test]
    fn label_text_reports_physical_pixels() {
        assert_eq!(
            RenderMetrics::label_text_at(Rect::new(0, 0, 1920, 1080), Point::new(0, 0)),
            "0,0 1920×1080 px"
        );
        assert_eq!(
            RenderMetrics::label_text_at(Rect::new(10, 10, 11, 11), Point::new(-1920, 200)),
            "-1910,210 1×1 px"
        );
    }

    #[test]
    fn label_size_grows_with_digits_and_dpi() {
        let metrics = RenderMetrics::for_dpi(96);
        let origin = Point::new(0, 0);
        let small = metrics.label_size_at(Rect::new(0, 0, 9, 9), origin);
        let large = metrics.label_size_at(Rect::new(0, 0, 1920, 1080), origin);
        assert!(large.0 > small.0);
        assert!(small.0 > 0 && small.1 > 0);
        let scaled = RenderMetrics::for_dpi(192);
        assert!(scaled.label_size_at(Rect::new(0, 0, 9, 9), origin).0 > small.0);
    }

    #[test]
    fn mask_alpha_is_translucent_but_dark() {
        assert!(MASK_ALPHA > 0.0 && MASK_ALPHA < 1.0);
    }

    #[test]
    fn geometry_helpers_match_the_shared_selection_maths() {
        // The renderer must not re-derive label placement: it delegates to the
        // tested geometry module.
        let placement = size_label_placement(
            Rect::new(0, 0, 100, 50),
            (80, 24),
            Rect::new(0, 0, 400, 400),
            8,
            None,
        );
        assert_eq!(
            placement,
            Some(SizeLabelPlacement {
                rect: Rect::new(0, 58, 80, 82),
                where_: LabelWhere::Below,
            })
        );
        for handle in Handle::ALL {
            let anchor = handle.anchor(Rect::new(0, 0, 100, 50));
            assert!(anchor.x >= 0 && anchor.x <= 100);
        }
        let _ = Point::new(0, 0);
        let _ = to_d2d(Rect::new(1, 2, 3, 4));
    }

    /// The info row must fit the **widest** colour string the sampler can emit
    /// (`hsl(360,100%,100%)`) alongside a representative position, so no colour
    /// value can ever wrap or clip. Widths are measured with the real embedded
    /// font, not an estimate — this is the correct way to prove the layout.
    #[test]
    fn widest_colour_value_fits_the_info_row() {
        let Ok(device) = super::GraphicsDevice::create() else {
            eprintln!("no D3D11 device; skipping the info-row width check");
            return;
        };
        let Ok(mut renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
            return;
        };

        let colour_fmt = renderer
            .info_text_format_mut(
                true,
                super::INFO_PRIMARY_COLOR_FONT_DIP,
                super::DWRITE_FONT_WEIGHT_SEMI_BOLD,
                false,
            )
            .unwrap();
        let widest_colour = renderer
            .measure_text_width_in("hsl(360,100%,100%)", &colour_fmt)
            .unwrap();

        let pos_fmt = renderer
            .info_text_format_mut(
                true,
                super::INFO_COORD_VALUE_FONT_DIP,
                super::DWRITE_FONT_WEIGHT_NORMAL,
                false,
            )
            .unwrap();
        let widest_pos = renderer
            .measure_text_width_in("(2560, 1440)", &pos_fmt)
            .unwrap();

        // Horizontal budget at 96 DPI (scale = 1): the loupe is a fixed 340-
        // physical-px box. The colour value (left) and the coordinates (right)
        // share row 1, so both must fit in the space left after the swatch and
        // the gap between them.
        let content = crate::geometry::MagnifierConfig::PANEL_WIDTH_PHYSICAL as f32
            - 2.0 * super::INFO_PADDING_H_DIP;
        let swatch_block = super::INFO_SWATCH_SIZE_DIP + super::INFO_SWATCH_GAP_DIP;
        let available = content - swatch_block - super::INFO_COORD_BLOCK_GAP_DIP;

        assert!(
            widest_colour + widest_pos <= available,
            "widest colour ({widest_colour}px) + coordinates ({widest_pos}px) do \
             not share row 1 (available {available}px = content {content} - swatch \
             {swatch_block} - gap {})",
            super::INFO_COORD_BLOCK_GAP_DIP
        );
    }

    /// Codepoints a TrueType font answers with a real glyph (`cmap` subtable formats 0/4/6/12).
    ///
    /// Deliberately small: this exists to check one subsetted font from a test, not to become a
    /// font library. Unknown subtable formats are skipped rather than guessed at.
    fn cmap_codepoints(bytes: &[u8]) -> std::collections::HashSet<u32> {
        use std::collections::HashSet;

        let u16_at = |at: usize| u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
        let u32_at = |at: usize| {
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize
        };
        let mut covered = HashSet::new();
        let mut cmap = None;
        for record in 0..u16_at(4) {
            let at = 12 + record * 16;
            if &bytes[at..at + 4] == b"cmap" {
                cmap = Some(u32_at(at + 8));
            }
        }
        let Some(cmap) = cmap else {
            return covered;
        };
        for subtable in 0..u16_at(cmap + 2) {
            let record = cmap + 4 + subtable * 8;
            let offset = cmap + u32_at(record + 4);
            match u16_at(offset) {
                // Byte encoding: a 256-entry glyph index array.
                0 => {
                    for code in 0..256 {
                        if bytes[offset + 6 + code] != 0 {
                            covered.insert(code as u32);
                        }
                    }
                }
                // Segment mapping: the one Windows uses for BMP text.
                4 => {
                    let segments = u16_at(offset + 6) / 2;
                    let ends = offset + 14;
                    let starts = ends + segments * 2 + 2;
                    let deltas = starts + segments * 2;
                    let ranges = deltas + segments * 2;
                    for segment in 0..segments {
                        let end = u16_at(ends + segment * 2);
                        let start = u16_at(starts + segment * 2);
                        let delta = u16_at(deltas + segment * 2);
                        let range = u16_at(ranges + segment * 2);
                        for code in start..=end {
                            if code == 0xFFFF {
                                continue;
                            }
                            let glyph = if range == 0 {
                                (code + delta) & 0xFFFF
                            } else {
                                // idRangeOffset is relative to its own slot.
                                let at = ranges + segment * 2 + range + (code - start) * 2;
                                if at + 2 > bytes.len() {
                                    continue;
                                }
                                let glyph = u16_at(at);
                                if glyph == 0 { 0 } else { (glyph + delta) & 0xFFFF }
                            };
                            if glyph != 0 {
                                covered.insert(code as u32);
                            }
                        }
                    }
                }
                // Trimmed mapping: first code + glyph index array.
                6 => {
                    let first = u16_at(offset + 6);
                    for i in 0..u16_at(offset + 8) {
                        if u16_at(offset + 10 + i * 2) != 0 {
                            covered.insert((first + i) as u32);
                        }
                    }
                }
                // Segmented coverage: (start, end, start glyph) triples, full Unicode range.
                12 => {
                    for group in 0..u32_at(offset + 12) {
                        let at = offset + 16 + group * 12;
                        for code in u32_at(at)..=u32_at(at + 4) {
                            covered.insert(code as u32);
                        }
                    }
                }
                _ => {}
            }
        }
        covered
    }

    /// Every string the overlay draws with the embedded family (docs/21 §5.21/§5.23).
    ///
    /// This is the one list, and it lives in Rust next to the drawing code. Wherever the text has
    /// a producer, the producer is *called* rather than copied, so the list cannot drift from what
    /// is painted. It feeds two things:
    ///
    /// * the coverage gate below — the embedded subset has to render every character of these;
    /// * `subfont/drawn-text.txt`, which `subfont/build_subset.py` turns into the subset's
    ///   codepoint list — so the font carries exactly the characters the overlay uses and nothing
    ///   that merely happens to appear in some other literal in the crate.
    ///
    /// Adding UI text means extending this list (or the producer it comes from). A string drawn
    /// inline in this file without being listed here is caught by the builder's guard, which scans
    /// the text-drawing files and fails on any character this list does not account for.
    fn overlay_drawn_strings() -> Vec<String> {
        use crate::windows::overlay::{LEVEL_HINT, level_hint, preview_label};
        use crate::window_detection::LevelKind;

        let mut drawn = vec![
            // Size label, zoom badge, hex colour, and the two other colour formats.
            "-1910,210 1×1 px".to_owned(),
            "1.5×".to_owned(),
            "#ABCDEF".to_owned(),
            "rgb(255,0,0)".to_owned(),
            "hsl(359,100%,100%)".to_owned(),
        ];
        for (key, description) in super::INFO_HINTS {
            drawn.push(key.to_owned());
            drawn.push(description.to_owned());
        }
        // Every state the preview label has: element, walked-to container, whole window, degraded.
        drawn.push(preview_label(Rect::new(0, 0, 341, 55), false, None, false, false));
        drawn.push(preview_label(Rect::new(0, 0, 689, 55), false, None, true, true));
        drawn.push(preview_label(
            Rect::new(0, 0, 3840, 2088),
            true,
            None,
            false,
            false,
        ));
        // …and the nouns of B6 (docs/21 §5.24): the label prints whatever the transport named the
        // box, so the font subset needs the vocabulary itself, not a sample sentence.
        drawn.extend(LevelKind::label_nouns().map(str::to_owned));
        // …and the two one-shot hints. The level badge now draws `↑` `↓` and digits (docs/21 §5.24,
        // A1), so it is covered through the sentence below rather than by a string of its own: the
        // chip's characters are a subset of these.
        drawn.push(LEVEL_HINT.to_owned());
        drawn.push(level_hint(LevelReach { up: 8, down: 1 }));
        // …and the scroll panel (docs/30 §19.7). Its list comes from the panel itself, which is the
        // producer of every word it draws: the constants, a computed amount line, the eleven ended
        // reasons and the thirteen diagnostic codes. Asking it here instead of copying the strings is
        // what makes a new `StopReason` unable to leave the font behind.
        drawn.extend(crate::scroll::panel::drawn_strings());
        drawn
    }

    /// Generate `subfont/drawn-text.txt` for the font builder; `subfont/subset.ps1` runs this.
    ///
    /// Ignored so the ordinary suite never writes files. The builder refuses to run on a file older
    /// than the sources, so a stale list cannot silently produce a font that is missing glyphs.
    #[test]
    #[ignore = "codegen for subfont/build_subset.py; run through subfont/subset.ps1"]
    fn write_drawn_text_for_the_font_subset() {
        // `CARGO_MANIFEST_DIR` is this crate's directory; the tooling lives at the
        // repository root. (It was `../subfont/…` while this file lived in src-tauri.)
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../subfont/drawn-text.txt");
        let mut body = String::from(
            "// Generated by `cargo test --lib write_drawn_text_for_the_font_subset -- --ignored`\n\
             // from `overlay_drawn_strings()` in win/d2d.rs. Every line is one string the overlay\n\
             // draws; subfont/build_subset.py turns their characters into the font subset.\n\
             // Do not edit: extend `overlay_drawn_strings()` instead.\n",
        );
        for text in overlay_drawn_strings() {
            body.push_str(&text);
            body.push('\n');
        }
        std::fs::write(&path, body).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
        eprintln!("wrote {}", path.display());
    }

    /// The gate the hand-written `--unicodes` list kept failing to be (docs/21 §5.21).
    ///
    /// The embedded subset is a hard allowlist: a drawn string with a character that is not in it
    /// does not fail, log, or throw — DirectWrite falls back per glyph, so the text quietly renders
    /// half in another font (or as tofu where no fallback exists).
    #[test]
    fn the_embedded_subset_covers_the_strings_the_overlay_draws() {
        let covered = cmap_codepoints(super::INFO_EMBEDDED_FONT);
        let drawn = overlay_drawn_strings();

        for text in &drawn {
            for character in text.chars() {
                assert!(
                    covered.contains(&(character as u32)),
                    "the embedded subset has no glyph for {character:?} (U+{:04X}), drawn by \
                     {text:?}: rebuild it with subfont/subset.ps1, which regenerates the\n\
                     codepoint list from `overlay_drawn_strings()`",
                    character as u32,
                );
            }
        }
    }

    /// The panel's palette is a copy of the interface's, and this is where the copy is checked.
    ///
    /// `crate::scroll::panel` may not name `crate::windows` (docs/30 §28.4: the model side of the
    /// crate is platform-free), so it cannot import `ACCENT_RGB` and has to restate the number. That
    /// is the kind of restatement that silently drifts, so the one module that can see both compares
    /// them — and, since the point of §19.4 is that the unadopted state is *not* a warning colour, it
    /// checks the two colours a user could confuse it with.
    #[test]
    fn the_panel_palette_matches_the_interface_palette() {
        use crate::scroll::panel::{ADOPTED_RGB, UNADOPTED_RGB};
        use crate::ring_contrast::{ACCENT_RGB, CAPTURE_RGB};

        assert_eq!(
            (ADOPTED_RGB.r, ADOPTED_RGB.g, ADOPTED_RGB.b),
            (ACCENT_RGB.r, ACCENT_RGB.g, ACCENT_RGB.b),
            "the adopted viewport box is the interface accent; if that changed, change the panel too"
        );

        // Neither red nor the capture green, and neutral: this is what "the colour must match the
        // action the user has to take" (docs/30 §19.4) comes down to when it is checked mechanically.
        assert_ne!(
            (UNADOPTED_RGB.r, UNADOPTED_RGB.g, UNADOPTED_RGB.b),
            (CAPTURE_RGB.r, CAPTURE_RGB.g, CAPTURE_RGB.b),
            "the unadopted box is not the capture green: nothing is being captured this step"
        );
        assert!(
            UNADOPTED_RGB.r <= UNADOPTED_RGB.g && UNADOPTED_RGB.r <= UNADOPTED_RGB.b,
            "the unadopted box is neutral, not red"
        );
    }

    /// Glyph count from the font's `maxp` table — the ceiling a `cmap` reader can be held to.
    fn font_glyph_count(bytes: &[u8]) -> usize {
        let u16_at = |at: usize| u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
        let u32_at = |at: usize| {
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize
        };
        for record in 0..u16_at(4) {
            let at = 12 + record * 16;
            if &bytes[at..at + 4] == b"maxp" {
                return u16_at(u32_at(at + 8) + 4);
            }
        }
        0
    }

    /// The gate is only as good as its instrument, so the reader above is checked too: it has to
    /// Measurement, not a gate: what the embedded subset costs at runtime (docs/21 §5.23).
    ///
    /// The file is 14 KB, which is a *binary size* item. The runtime items are the one-time
    /// registration and the per-draw text layout, and neither depends on how many glyphs the file
    /// carries — only on the glyphs actually shaped. This prints both, next to two stock families
    /// (one of them a ~10 MB installed CJK font) so the comparison is visible rather than argued.
    #[test]
    #[ignore = "measurement probe; run with --ignored --nocapture"]
    fn font_cost_probe() {
        use ::windows::Win32::Graphics::DirectWrite::{
            DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_TEXT_METRICS,
        };
        use ::windows::Win32::Graphics::Gdi::RemoveFontMemResourceEx;
        use ::windows::core::PCWSTR;
        use std::time::Instant;

        let bytes = super::INFO_EMBEDDED_FONT;
        let samples = 200;
        let strings = ["3840×2088 px  容器 8/9", "#A1B2C3", "hsl(359,100%,100%)"];

        println!(
            "embedded subset: {} B, {} codepoints, {} glyphs",
            bytes.len(),
            cmap_codepoints(bytes).len(),
            font_glyph_count(bytes)
        );

        // 1) The one-time cost, measured on the same bytes the process registers at start-up.
        let mut installed = 0u32;
        let started = Instant::now();
        let handle = unsafe {
            ::windows::Win32::Graphics::Gdi::AddFontMemResourceEx(
                bytes.as_ptr().cast(),
                bytes.len() as u32,
                None,
                &mut installed,
            )
        };
        println!(
            "AddFontMemResourceEx      : {:.0} us (once per process; {installed} face(s))",
            started.elapsed().as_secs_f64() * 1e6
        );
        if !handle.0.is_null() {
            let _ = unsafe { RemoveFontMemResourceEx(handle) };
        }

        // 2) Per-draw layout: the overlay builds a text layout for the size label and the preview
        //    label on every present (`measure_label`), so this is the number that shows up in a
        //    frame — and it is about shaping, not about the size of the font file.
        let Ok(device) = super::GraphicsDevice::create() else {
            eprintln!("no D3D11 device; skipping the layout half of the probe");
            return;
        };
        let Ok(renderer) = OverlayRenderer::new(std::sync::Arc::new(device), 96) else {
            eprintln!("no renderer; skipping the layout half of the probe");
            return;
        };
        for family in ["HarmonyOS Sans SC", "Microsoft YaHei UI", "Segoe UI"] {
            let wide = super::to_wide(family);
            let locale = super::to_wide("en-us");
            let format = unsafe {
                renderer.dwrite.CreateTextFormat(
                    PCWSTR(wide.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    12.0,
                    PCWSTR(locale.as_ptr()),
                )
            };
            let Ok(format) = format else {
                println!("{family:26}: unavailable");
                continue;
            };
            let mut worst = 0.0_f64;
            let mut total = 0.0_f64;
            for _ in 0..samples {
                for text in strings {
                    let wide = super::to_wide(text);
                    let started = Instant::now();
                    let Ok(layout) = (unsafe {
                        renderer.dwrite.CreateTextLayout(
                            &wide,
                            &format,
                            f32::INFINITY,
                            f32::INFINITY,
                        )
                    }) else {
                        continue;
                    };
                    let mut metrics = DWRITE_TEXT_METRICS::default();
                    let _ = unsafe { layout.GetMetrics(&mut metrics) };
                    let elapsed = started.elapsed().as_secs_f64() * 1e6;
                    worst = worst.max(elapsed);
                    total += elapsed;
                }
            }
            println!(
                "{family:26}: layout+metrics avg {:.1} us, worst {:.1} us  ({} samples)",
                total / (samples * strings.len()) as f64,
                worst,
                samples * strings.len()
            );
        }
    }

    /// The gate is only as good as its instrument, so the reader above is checked too: it has to
    /// find the glyphs the subset does carry and not invent ranges it never read.
    #[test]
    fn the_cmap_reader_finds_glyphs_and_does_not_invent_them() {
        let covered = cmap_codepoints(super::INFO_EMBEDDED_FONT);
        for character in ['×', '0', 'p', 'x', '色'] {
            assert!(
                covered.contains(&(character as u32)),
                "{character:?} is in the subset and the reader missed it"
            );
        }
        // A `cmap` answers codepoints with *glyphs*, and the font only has so many: a reader that
        // mis-reads an offset and walks a whole plane reports far more codepoints than that, which
        // is the failure mode that would make the gate above pass on a deficient font.
        let glyphs = font_glyph_count(super::INFO_EMBEDDED_FONT);
        assert!(glyphs > 50, "the subset should carry the chrome's glyphs ({glyphs})");
        assert!(
            covered.len() <= glyphs,
            "the reader reports {} codepoints for a font with {glyphs} glyphs",
            covered.len()
        );
        // The noncharacters are the one range a font is guaranteed never to map.
        assert!(
            !covered.contains(&0xFDD0),
            "U+FDD0 is a noncharacter and cannot be in the font"
        );
    }

    /// The preview panel must not cover anything the user still has to be able to grab (docs/30
    /// §19.7; §30.6's "布局不遮挡操作").
    ///
    /// The same composition is rendered twice into the same target — once without the panel, once
    /// with it — and both are read back. Pixels, not the model, because there are three claims here
    /// and only the readback can settle any of them:
    ///
    /// * the panel paints something at all, otherwise "it covers nothing" is vacuously true;
    /// * it paints **only inside its own rectangle** — no stroke, no shadow, no stray mark reaches
    ///   outside, so the rectangle the painter reports is the region that can hide things;
    /// * no pixel it paints is a point the overlay calls a grip or a grabbable border. That question
    ///   is asked of `SelectionSnapshot::hit_test`, the same call the overlay's pointer routing
    ///   makes, so this test cannot drift away from the hit testing it exists to protect.
    ///
    /// The selection is the frame itself, which is the geometry §19.7 anchors the panel for — a scroll
    /// capture of a window that fills the work area. The right grip is then centred exactly on the
    /// edge the panel hangs off, which is why the DPI sweep is not decoration: the DIP margin and the
    /// grip's reach are rounded independently, and 100% is one of the DPIs where they happen to agree.
    #[test]
    #[ignore = "renders through the real D3D11/D2D path; the L3 gate runs it with --ignored"]
    fn the_preview_panel_does_not_cover_the_toolbar_hit_regions() {
        use crate::geometry::{SelectionGeometry, SelectionSnapshot};
        use crate::scroll::displacement::Status;
        use crate::scroll::panel::ScrollPanel;
        use crate::scroll::preview::PreviewUpdate;

        // A maximised window on a 1280x900 work area at 100%, scaled by the DPI under test.
        const FRAME_DIP: (i32, i32) = (1280, 900);
        const DPIS: [u32; 5] = [96, 120, 144, 168, 192];

        for dpi in DPIS {
            let scale = dpi as f32 / 96.0;
            let width = (FRAME_DIP.0 as f32 * scale).round() as u32;
            let height = (FRAME_DIP.1 as f32 * scale).round() as u32;

            let device = super::GraphicsDevice::create().expect(
                "this probe renders for real, which is why it is #[ignore]d: without a D3D11 device \
                 its question cannot be answered, and passing quietly is the one thing it must not do",
            );
            let mut renderer = OverlayRenderer::new(std::sync::Arc::new(device), dpi)
                .expect("the overlay renders at every DPI the overlay runs at");
            let frame_pixels = solid_bgra(width, height, [0x20, 0x28, 0x30, 0xFF]);
            renderer
                .update_frame(width, height, &frame_pixels)
                .expect("a full-frame BGRA buffer is a frame");
            renderer
                .ensure_back_buffer(width, height)
                .expect("the back buffer matches the frame");
            let target = renderer
                .device()
                .create_render_target_texture(width, height)
                .expect("an off-screen target");
            let context = renderer
                .device()
                .create_d2d_context()
                .expect("a D2D context over our own device");
            let target_bitmap = super::super::d3d11::create_bitmap_from_texture(
                &context,
                &target.texture,
                D2D1_BITMAP_OPTIONS_TARGET,
                D2D1_ALPHA_MODE_PREMULTIPLIED,
            )
            .expect("a D2D bitmap over the target");

            let frame = Rect::from_origin_size(Point::new(0, 0), width as i32, height as i32);
            let mut view = RenderView::new(frame);
            // The chrome is what has to survive the panel, so it has to be on: the grips under test
            // are drawn chrome, not model state.
            view.show_chrome = true;
            view.cursor_visible = false;
            view.selection = frame;

            renderer
                .draw_to(&target_bitmap, &view)
                .expect("the composition without the panel");
            let without = renderer
                .device()
                .read_back_bgra(&target.texture)
                .expect("a CPU readback of the composed frame");

            let mut panel = ScrollPanel::new(u64::from(width), u64::from(height));
            panel.on_update(PreviewUpdate::Span {
                primary_len: 4_000,
                steps: 42,
                discarded: 3,
            });
            panel.on_update(PreviewUpdate::Viewport {
                band: 2_000,
                status: Status::Confirmed { d: 120 },
            });
            view.scroll_panel = Some(panel);

            renderer
                .draw_to(&target_bitmap, &view)
                .expect("the composition with the panel");
            let with = renderer
                .device()
                .read_back_bgra(&target.texture)
                .expect("a CPU readback of the same frame, with the panel");

            let (panel_rect, _) = super::scroll_panel_placement(&view, renderer.metrics())
                .expect("the panel was handed to the renderer a moment ago");
            let snapshot = SelectionSnapshot::new(view.selection, dpi);

            let mut painted = 0u32;
            for y in panel_rect.top..panel_rect.bottom {
                for x in panel_rect.left..panel_rect.right {
                    let before = pixel_at(&without, width, x as u32, y as u32);
                    let after = pixel_at(&with, width, x as u32, y as u32);
                    if before == after {
                        continue;
                    }
                    painted += 1;
                    let geometry = snapshot.hit_test(Point::new(x, y), dpi);
                    assert!(
                        !matches!(geometry, SelectionGeometry::Resize(_)),
                        "at {dpi} DPI the preview panel painted ({x}, {y}), which the overlay's hit \
                         test calls {geometry:?}: the panel covers a grip the user has to grab"
                    );
                }
            }
            assert!(
                painted > 0,
                "at {dpi} DPI the panel changed nothing over a {width}x{height} frame, so this test \
                 would have passed for a panel that was never drawn"
            );

            // And nothing outside the rectangle it reports moved: the claim is about a region the
            // user can see, not about that region plus whatever a stroke or a shadow reaches.
            for y in 0..height as i32 {
                for x in 0..width as i32 {
                    if panel_rect.contains(Point::new(x, y)) {
                        continue;
                    }
                    assert_eq!(
                        pixel_at(&with, width, x as u32, y as u32),
                        pixel_at(&without, width, x as u32, y as u32),
                        "at {dpi} DPI the panel changed ({x}, {y}), outside the rectangle it reports \
                         ({panel_rect:?})"
                    );
                }
            }
        }
    }

    /// "Follows the finger" has to be a number, and the number that matters is how long the
    /// *composing* thread — the overlay's, and therefore the user's — is blocked per preview update
    /// (docs/30 §23.2's `UI 主线程最大同步工作`, §23.3's `E-PERF-4`).
    ///
    /// The seam is `PreviewUpdate` → pixels: fold the update into the panel's view model
    /// (`ScrollPanel::on_update`) and compose the whole frame into the target the overlay presents
    /// (`OverlayRenderer::draw_to`, the same layer code `render` calls, and §5 established that it
    /// repaints every pixel). Two things it does *not* include, and both are named rather than
    /// quietly assumed:
    ///
    /// * the swap chain's `Present`, which belongs to the window host on the far side of the
    ///   assembly root (`P6`);
    /// * the wait in front of the fold — `RENDER_TICK_MS` of timer, which is a wait and not work.
    ///   That wait is the reason §23.3's `Preview 更新延迟` cannot be measured from here at all.
    ///
    /// Four rates, because §23.1 asks whether 10 Hz is the right cap. The per-update cost does not
    /// depend on the rate; the **duty cycle** does, and that is what the cap is chosen against. The
    /// naps between updates keep the wall clock honest to each rate and are excluded from the
    /// measurement — a thread that is asleep is exactly a thread that is not blocking.
    ///
    /// §23.3 carries two numbers: 8 ms is the pass threshold and 4 ms is the target the threshold is
    /// meant to leave room for. Both are reported, because a measurement between them is not a pass.
    #[test]
    #[ignore = "composes through the real D3D11/D2D path; the L4 gate runs it with --ignored"]
    fn the_overlay_thread_never_blocks_longer_than_eight_ms() {
        use std::time::{Duration, Instant};

        use crate::scroll::displacement::Status;
        use crate::scroll::latency_probe::percentiles;
        use crate::scroll::panel::ScrollPanel;
        use crate::scroll::preview::PreviewUpdate;

        /// §23.3: the pass threshold, and the target it exists to leave room for.
        const THRESHOLD_NS: u64 = 8_000_000;
        const TARGET_NS: u64 = 4_000_000;
        /// A 1500x900 window at 125%: §22.6's content width, and a physical surface large enough that
        /// the frame repaint — not the panel — is the bulk of the work.
        const DIP: (i32, i32) = (1500, 900);
        const DPI: u32 = 120;
        const HZ: [u32; 4] = [5, 10, 20, 30];
        const UPDATES: u32 = 30;

        let scale = DPI as f32 / 96.0;
        let width = (DIP.0 as f32 * scale).round() as u32;
        let height = (DIP.1 as f32 * scale).round() as u32;

        let device = super::GraphicsDevice::create().expect(
            "this probe composes for real, which is why it is #[ignore]d: without a D3D11 device its \
             question cannot be answered, and passing quietly is the one thing it must not do",
        );
        let mut renderer = OverlayRenderer::new(std::sync::Arc::new(device), DPI)
            .expect("the overlay renders at every DPI the overlay runs at");
        let frame_pixels = solid_bgra(width, height, [0x20, 0x28, 0x30, 0xFF]);
        renderer
            .update_frame(width, height, &frame_pixels)
            .expect("a full-frame BGRA buffer is a frame");
        renderer
            .ensure_back_buffer(width, height)
            .expect("the back buffer matches the frame");
        let target = renderer
            .device()
            .create_render_target_texture(width, height)
            .expect("an off-screen target");
        let context = renderer
            .device()
            .create_d2d_context()
            .expect("a D2D context over our own device");
        let target_bitmap = super::super::d3d11::create_bitmap_from_texture(
            &context,
            &target.texture,
            D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_ALPHA_MODE_PREMULTIPLIED,
        )
        .expect("a D2D bitmap over the target");

        let frame = Rect::from_origin_size(Point::new(0, 0), width as i32, height as i32);
        let mut view = RenderView::new(frame);
        view.show_chrome = true;
        view.cursor_visible = false;
        view.selection = frame;

        // Warm-up. The first composition through a freshly created context and target builds the
        // resources every later one reuses, and it costs about as much as a whole frame budget. The
        // overlay has composed thousands of frames before a scroll session begins, so charging that
        // one-time cost against a per-update budget would be measuring the wrong thing — but it is
        // reported rather than dropped, because it is real, it is bounded, and the reader of the
        // numbers below deserves to know it exists.
        let panel = ScrollPanel::new(1500, 900);
        view.scroll_panel = Some(panel.clone());
        let warm_start = Instant::now();
        renderer
            .draw_to(&target_bitmap, &view)
            .expect("the warm-up composition");
        println!(
            "[P5.06] the first composition through a fresh target: {:.2} ms (one-time, not counted \
             below)",
            warm_start.elapsed().as_secs_f64() * 1e3
        );
        for _ in 0..2 {
            renderer
                .draw_to(&target_bitmap, &view)
                .expect("a warm-up composition");
        }

        for hz in HZ {
            let interval = Duration::from_nanos(1_000_000_000 / u64::from(hz));
            let mut panel = ScrollPanel::new(1500, 900);
            let mut work: Vec<u64> = Vec::with_capacity(UPDATES as usize);
            for tick in 0..UPDATES {
                std::thread::sleep(interval);
                // The three updates a scrolling producer actually sends, folded the way the overlay
                // folds them: rows announced, how many steps and how many dropped, where the viewport
                // now is.
                let at = Instant::now();
                panel.on_update(PreviewUpdate::Bands {
                    first_row: u64::from(tick) * 540,
                    rows: 540,
                    scale: 1,
                });
                panel.on_update(PreviewUpdate::Span {
                    primary_len: u64::from(tick) * 540 + 900,
                    steps: tick,
                    discarded: tick / 7,
                });
                panel.on_update(PreviewUpdate::Viewport {
                    band: u64::from(tick) * 540,
                    status: Status::Confirmed { d: 540 },
                });
                view.scroll_panel = Some(panel.clone());
                renderer
                    .draw_to(&target_bitmap, &view)
                    .expect("the composition the overlay presents");
                work.push(at.elapsed().as_nanos() as u64);
            }
            let (p50, p95, max) = percentiles(&mut work);
            // The overlay thread has 1/hz seconds per update; work × rate is the fraction of it spent
            // blocked. `u64::from(hz) * 100` keeps the percentage in integers.
            let duty_tenths = (max as u128 * u128::from(hz) * 1000 / 1_000_000_000) as u64;
            let duty = format!("{}.{}", duty_tenths / 10, duty_tenths % 10);
            println!(
                "[P5.06] overlay @{hz} Hz: fold+compose p50 {:.2} ms / p95 {:.2} ms / max {:.2} ms \
                 over {UPDATES} updates; worst-case duty {duty}%",
                p50 as f64 / 1e6,
                p95 as f64 / 1e6,
                max as f64 / 1e6,
            );
            assert!(
                max <= THRESHOLD_NS,
                "at {hz} Hz the composing thread was blocked for {} ms, and §23.3's pass threshold \
                 is {} ms (p50 {} ms, p95 {} ms)",
                max as f64 / 1e6,
                THRESHOLD_NS as f64 / 1e6,
                p50 as f64 / 1e6,
                p95 as f64 / 1e6,
            );
            if max > TARGET_NS {
                println!(
                    "[P5.06] ...past §23.3's {} ms target, inside the {} ms threshold",
                    TARGET_NS as f64 / 1e6,
                    THRESHOLD_NS as f64 / 1e6
                );
            }
        }
    }
