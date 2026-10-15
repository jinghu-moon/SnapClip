//! The overlay's typeface: the embedded subset, the family stack that goes with it, and the
//! private registration that makes the family resolvable by name.
//!
//! Two renderers draw the same labels — the D2D overlay through DirectWrite, the pin window
//! through GDI — so the family names live here instead of inside one of them. Both ask for
//! [`FAMILY`] first and fall through [`FALLBACKS`] in order (docs/34 §4.2.4).
//!
//! The subset is embedded verbatim and registered from memory: `AddFontMemResourceEx` loads it
//! straight from the bytes baked into the binary — no temp file, no system install, no admin
//! rights. The slice is `'static`, so the registration stays valid for the whole process.

use ::windows::Win32::Graphics::Gdi::AddFontMemResourceEx;

/// The overlay's typeface: a subsetted HarmonyOS Sans SC carrying exactly the characters the
/// overlay can draw — every label, hint, guide row and key name, under any binding
/// (`overlay_drawn_strings` in the tests, built by `subfont/subset.ps1`).
pub const SUBSET: &[u8] = include_bytes!("../assets/harmonyos-sans-sc-subset.ttf");

/// Family name the embedded subset registers under (its name table was reduced to this single
/// English entry during subsetting).
pub const FAMILY: &str = "HarmonyOS Sans SC";

/// The stock UI families to fall through to, in order, when [`FAMILY`] cannot be resolved.
pub const FALLBACKS: [&str; 2] = ["Segoe UI Variable Display", "Segoe UI"];

/// Every family a text run may end up in, in priority order.
pub const FAMILIES: [&str; 3] = [FAMILY, FALLBACKS[0], FALLBACKS[1]];

/// Shared metrics for the zoom badge drawn by DirectWrite and the pin window's GDI fallback.
///
/// Keeping these values beside the shared family stack prevents the two renderers from silently
/// drifting apart again. `700` is DirectWrite's BOLD weight and GDI's `FW_BOLD` weight.
pub const ZOOM_BADGE_FONT_SIZE_DIP: f32 = 14.0;
pub const ZOOM_BADGE_FONT_WEIGHT: i32 = 700;

/// Register the embedded subset privately for this process, once.
///
/// A failure is not fatal and has no one to report to: every text run falls back through
/// [`FAMILIES`] when the embedded family cannot be resolved.
///
/// The DirectWrite path follows this with an *isolated* `IDWriteFactory` so that its first
/// enumeration already contains [`FAMILY`] — windows-rs does not bind
/// `IDWriteFactory::ReloadSystemFonts`, and a shared factory may have been cached earlier by
/// another subsystem.
pub fn register() {
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| unsafe {
        let mut installed = 0u32;
        let _ = AddFontMemResourceEx(
            SUBSET.as_ptr().cast(),
            SUBSET.len() as u32,
            None,
            &mut installed,
        );
    });
}
