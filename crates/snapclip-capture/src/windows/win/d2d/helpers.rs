//! Free helpers shared by the render passes (docs/23 T1.8).

use super::*;

/// Register the embedded info font privately for this process, once.
///
/// `AddFontMemResourceEx` loads the subset straight from the bytes baked into the
/// binary — no temp file, no system install, no admin rights. `OverlayRenderer::new`
/// follows with `IDWriteFactory::ReloadSystemFonts` so the shared DWrite factory
/// re-enumerates and can resolve `"HarmonyOS Sans SC"` by name. The embedded slice
/// is `'static`, so the registration stays valid for the whole process.
pub(super) fn register_info_font_once() {
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| unsafe {
        let mut installed = 0u32;
        // A failure here is non-fatal: every text format falls back to a stock
        // family when the embedded one cannot be resolved.
        let _ = AddFontMemResourceEx(
            INFO_EMBEDDED_FONT.as_ptr().cast(),
            INFO_EMBEDDED_FONT.len() as u32,
            None,
            &mut installed,
        );
    });
}

pub(super) fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

/// How much wider than the ring the underlay is drawn, in logical pixels.
pub(super) const CHAIN_RING_SHADOW_WIDTH: f32 = 2.0;

/// The ring brush is mutable (`SetColor` per ring) because each level carries its own opacity.
pub(super) fn chain_ring_color(alpha: f32) -> D2D1_COLOR_F {
    color_of(CHAIN_RING_RGB, alpha.clamp(0.0, 1.0))
}

/// The capture box's outline colour: brand blue at rest, the capture green while walking (A3).
///
/// `appear` is the box's own fade-in — 0 the frame it appears, 1 from 140 ms later (docs/21 §5.24,
/// ②). It rides on the same brush because the two are the same statement: how strongly this outline
/// is on screen.
pub(super) fn capture_mix_color(walking: f32, appear: f32) -> D2D1_COLOR_F {
    color_of(
        mix(ACCENT_RGB, CAPTURE_RGB, walking.clamp(0.0, 1.0)),
        appear.clamp(0.0, 1.0),
    )
}

/// The capture box's wash: nothing at rest, the capture green at half strength while walking.
pub(super) fn capture_wash_color(walking: f32, appear: f32) -> D2D1_COLOR_F {
    color_of(
        CAPTURE_RGB,
        PREVIEW_WASH_ALPHA * PREVIEW_WASH_WALK_SCALE * walking.clamp(0.0, 1.0) * appear.clamp(0.0, 1.0),
    )
}

/// A `D2D1_COLOR_F` from the shared palette (docs/21 §5.24.11).
pub(super) fn color_of(rgb: Rgb, alpha: f32) -> D2D1_COLOR_F {
    let (r, g, b) = rgb.channels_f32();
    color(r, g, b, alpha)
}

/// Build the point type Direct2D expects.
pub(super) fn vector2(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

pub(super) fn to_d2d(rect: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: rect.left as f32,
        top: rect.top as f32,
        right: rect.right as f32,
        bottom: rect.bottom as f32,
    }
}

/// Fill `frame` with `brush`, leaving `hole` at whatever was underneath it.
///
/// Four bands instead of re-drawing the frame inside the hole: the pixels inside the hole keep their
/// original brightness, and a drag only touches the bands that actually changed. The hole is what
/// would be **captured** (docs/21 §5.24, A2), which is why the mask, the theme tint *and* the hover
/// wash all go through here — a veil painted over the box would undo the point of the hole.
pub(super) fn fill_outside(
    context: &ID2D1DeviceContext,
    frame: Rect,
    hole: Rect,
    brush: &ID2D1SolidColorBrush,
) {
    unsafe {
        if hole.is_empty() {
            context.FillRectangle(&to_d2d(frame), brush);
            return;
        }
        let hole = hole.intersect(frame);
        if hole.is_empty() {
            context.FillRectangle(&to_d2d(frame), brush);
            return;
        }
        for band in frame.surround(hole) {
            if band.is_empty() {
                continue;
            }
            context.FillRectangle(&to_d2d(band), brush);
        }
    }
}

pub(super) fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
