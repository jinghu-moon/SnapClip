//! The save panel's numeric typeface: the embedded subset and the private registration that makes
//! it resolvable by name.
//!
//! The size cells show a number and a unit, and the mockup gives them a monospaced face
//! (`.dim-input` / `.dim-unit`, `docs/design/save-panel-demo-v3.html:242`, `:244`) so that digits
//! line up between the two cells and a changing value does not shuffle the text sideways.
//!
//! The face is Fira Code, reduced by `subfont/build_number_font.py` to the thirteen characters the
//! row can show: three kilobytes instead of the two hundred and eighty three the source font
//! carries. It is embedded verbatim and registered from memory, exactly like the UI subset in
//! `snapclip-capture`'s `overlay_font`, so no machine needs it installed.
//!
//! This lives in the ui crate rather than beside the UI font because both consumers are here: the
//! panel draws the unit, and the application sets the face on the size cells' edit controls. The
//! ui crate is the only one both of them depend on.

use windows::Win32::Graphics::Gdi::AddFontMemResourceEx;

/// The family the numeric subset registers under. The subset's name table carries this one English
/// entry, and `subfont/build_number_font.py` refuses to install a build that says anything else.
pub const NUMBER_FAMILY: &str = "Fira Code";

/// Every character the numeric face may be asked to draw, read from the one file that also feeds the
/// subsetting script.
///
/// The set used to exist twice — once here and once inside `subfont/build_number_font.py` — and two
/// copies of a rule are two rules. It now lives in `subfont/number-chars.txt`, which the script
/// reads and this constant embeds, so a character added for one cannot be missing from the other.
///
/// The set is: the digits, the `px` unit, the percent sign for the percentage mode, the `KB` and
/// decimal point the estimate row's unit uses (`docs/design/save-panel-demo-v3.html:242`, `:244`,
/// `:648`), and the letters of the format control's `.PNG` / `.JPG` / `.WebP` (`:717`), which the
/// design also sets in this face (`:173`, `:182`).
pub const NUMBER_CHARACTERS: &str = include_str!("../../../../subfont/number-chars.txt");

/// The set with the file's incidental whitespace removed. Every reader goes through this, so a
/// trailing newline added by an editor is not read as a character the font has to carry.
pub fn number_characters() -> impl Iterator<Item = char> {
    NUMBER_CHARACTERS.trim().chars()
}

/// The numeric subset, read straight out of the binary by `include_bytes!`.
pub const NUMBER_SUBSET: &[u8] = include_bytes!("../../assets/fira-code-numbers-subset.ttf");

/// The stock monospaced families to fall through to when [`NUMBER_FAMILY`] cannot be resolved.
/// `Consolas` ships with Windows and is metrically different, which is why it is a fallback and
/// not the choice: the panel asks for the embedded face first.
pub const NUMBER_FALLBACKS: [&str; 2] = ["Consolas", "Courier New"];

/// Register the numeric subset privately for this process, once.
///
/// A failure is not fatal and has no one to report to: a run falls back through
/// [`NUMBER_FALLBACKS`] when the embedded family cannot be resolved.
pub fn register_number_font() {
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| unsafe {
        let mut installed = 0u32;
        let _ = AddFontMemResourceEx(
            NUMBER_SUBSET.as_ptr().cast(),
            NUMBER_SUBSET.len() as u32,
            None,
            &mut installed,
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The name the registration uses and the name the format asks for have to be the same string,
    /// and the artifact has to be the subset rather than the source font.
    #[test]
    fn the_numeric_face_is_the_subset_and_not_the_source_font() {
        assert_eq!(NUMBER_FAMILY, "Fira Code");
        assert!(
            NUMBER_SUBSET.len() < 16 * 1024,
            "the embedded numeric face is {} bytes, which is the source font rather than a subset",
            NUMBER_SUBSET.len()
        );
    }
}
