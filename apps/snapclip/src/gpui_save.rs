//! What the GPUI probe saves.
//!
//! The probe has no capture behind it, so the picture it writes is a synthetic one: a checker of
//! two colours at a size this module names. That is a **test input**, not a number chosen to make
//! the panel look complete — the panel shows no size for it anywhere, and the only thing the user
//! sees about it is the file that lands on disk.
//!
//! Everything else here is the panel's own: the values come from the same [`PanelValues`] the
//! direct-drawn panel builds, the weight is the same [`weigh`] that panel uses, the frame is the
//! same [`Frame`], and the sentence for a failure is [`SaveError::message`], which already prefers
//! the filled-in detail over the generic rule text. Nothing in this file decides anything twice.

use crate::save::assemble::{self, PanelValues};
use crate::save::estimate;
use crate::save::scale::{Frame, Size};
use crate::save::service::{SaveError, SaveOutcome};
use snapclip_capture::save_format::SaveFormat;

/// The size of the synthetic picture. Small on purpose: its pixels are only ever written to a file
/// in a test, and a smaller source keeps the real-encode tests quick.
pub const PROBE_SOURCE: Size = Size::new(64, 64);

/// The two colours of the synthetic checker, `[blue, green, red, alpha]` like every buffer here.
const CHECKER: [[u8; 4]; 2] = [[0x3e, 0x36, 0x32, 0xff], [0x2a, 0x24, 0x21, 0xff]];

/// A picture the probe can save, and its size.
///
/// A struct rather than a bare `Vec<u8>` because [`Frame`] borrows the pixels: a function returning
/// a `Frame` could not own them, and a caller assembling the two by hand could pass a buffer of one
/// size with a caption of another.
pub struct Picture {
    bgra: Vec<u8>,
    source: Size,
}

impl Picture {
    /// The synthetic picture, `PROBE_SOURCE` in size.
    pub fn synthetic() -> Self {
        Self {
            bgra: checker_bgra(PROBE_SOURCE),
            source: PROBE_SOURCE,
        }
    }

    /// The pixels as the save chain wants them.
    pub fn frame(&self, output: Size) -> Frame<'_> {
        Frame {
            bgra: &self.bgra,
            source: self.source,
            output,
        }
    }

    /// The same pixels for the clipboard, which takes straight rows of four bytes.
    pub fn pixels(&self) -> (&[u8], Size) {
        (&self.bgra, self.source)
    }
}

/// A checker of [`CHECKER`], top down, four bytes a pixel, tightly packed.
pub fn checker_bgra(size: Size) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(size.width as usize * size.height as usize * 4);
    for y in 0..size.height {
        for x in 0..size.width {
            let tile = ((x / 8) + (y / 8)) % 2;
            pixels.extend_from_slice(&CHECKER[tile as usize]);
        }
    }
    pixels
}

/// The format a menu index stands for.
///
/// The panel already owns this mapping ([`crate::save::window::format_of_index`]); it is called
/// rather than repeated, because the index and the format are both small integers and a swap would
/// write a file of the wrong kind without anything looking wrong.
pub fn format_of(index: usize) -> SaveFormat {
    crate::save::window::format_of_index(index)
}

/// The panel's answers as a save wants them.
///
/// Named fields, and a function rather than a call site, for the same reason the panel has one: its
/// two text fields are both strings, so a swap would save something entirely reasonable to the
/// wrong place.
pub fn values<'a>(
    directory: &'a str,
    name: &'a str,
    format: SaveFormat,
    date: &'a str,
    time: &'a str,
) -> PanelValues<'a> {
    PanelValues {
        directory,
        name,
        format,
        // The probe shows the tiers but cannot pick one yet; the first tier is what the panel
        // starts on, and only JPEG spends it.
        quality: assemble::quality_of_tier(0),
        // A name that is taken is renamed rather than replaced, which is the panel's default.
        conflict: crate::save::service::ConflictPolicy::AutoRename,
        fill: [0xff, 0xff, 0xff],
        date,
        time,
        sequence: 1,
        app: crate::save::window::APP_NAME,
    }
}

/// The date and time the naming template asks for, from the one place that reads the clock.
pub fn now() -> (String, String) {
    crate::save::window::local_date_and_time()
}

/// How heavy the file would be, as the panel writes it (`227.31 KB`, `:668`).
///
/// One real encode of the picture the save would write, not a model: the panel's own weighing
/// function is used, so this number and the file cannot disagree.
pub fn weight(picture: &Picture, values: &PanelValues<'_>, output: Size) -> Option<String> {
    let frame = picture.frame(output);
    assemble::weigh(values, &frame)
        .ok()
        .map(estimate::format_bytes)
}

/// Write the file, and say what happened.
pub fn save(
    picture: &Picture,
    values: &PanelValues<'_>,
    output: Size,
) -> Result<SaveOutcome, SaveError> {
    assemble::save_panel(values, &picture.frame(output))
}

/// The sentence the footer shows, or nothing.
///
/// A save that succeeded explains nothing, and a cancelled one is not a failure: the user changed
/// their mind, and the panel's rule (`docs/41`) is that only something to explain is drawn.
pub fn status_of(result: &Result<SaveOutcome, SaveError>) -> Option<String> {
    match result {
        Ok(_) => None,
        Err(SaveError::Cancelled) => None,
        Err(other) => Some(other.message()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save::copy::ValidationCopy;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("snapclip-gpui-probe-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the test can make a folder");
        dir
    }

    /// The file that landed is the size the panel asked for: a PNG's first fourteen bytes are its
    /// signature and its header, and the width and height are the two words after that.
    fn png_size(bytes: &[u8]) -> (u32, u32) {
        let word = |at: usize| {
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        (word(16), word(20))
    }

    #[gpui_kit::test]
    fn the_probe_writes_the_size_and_the_format_it_was_given() {
        let dir = temp_dir("size");
        let picture = Picture::synthetic();
        for (index, suffix, magic) in [
            (0usize, "png", &[0x89u8, b'P'][..]),
            (1, "jpg", &[0xffu8, 0xd8][..]),
        ] {
            let format = format_of(index);
            let values = values(
                dir.to_str().expect("utf-8 path"),
                "probe_[日期]",
                format,
                "2026-10-26",
                "0005",
            );
            let output = Size::new(96, 64);
            let outcome = save(&picture, &values, output).expect("the probe can save");
            let bytes = std::fs::read(&outcome.path).expect("the file is there");
            assert_eq!(outcome.path.extension().and_then(|e| e.to_str()), Some(suffix));
            assert_eq!(&bytes[..2], magic, "the file starts like a {suffix}");
            assert_eq!(bytes.len() as u64, outcome.bytes, "the reported size is the file");
            if suffix == "png" {
                assert_eq!(png_size(&bytes), (96, 64), "the file is the size the panel asked");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The weight the note shows is the length of the file that would land.
    #[gpui_kit::test]
    fn the_weight_is_the_file_that_would_be_written() {
        let dir = temp_dir("weight");
        let picture = Picture::synthetic();
        let values = values(
            dir.to_str().expect("utf-8 path"),
            "probe_[日期]",
            SaveFormat::Png,
            "2026-10-26",
            "0005",
        );
        let output = Size::new(64, 64);
        let note = weight(&picture, &values, output).expect("a small picture weighs something");
        let outcome = save(&picture, &values, output).expect("and it saves");
        assert_eq!(note, estimate::format_bytes(outcome.bytes));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failure_is_explained_and_a_success_and_a_cancel_are_not() {
        let ok = Ok(SaveOutcome {
            path: std::path::PathBuf::from("x.png"),
            bytes: 1,
            format: SaveFormat::Png,
            warnings: Vec::new(),
        });
        assert_eq!(status_of(&ok), None, "a save that worked explains nothing");
        assert_eq!(
            status_of(&Err(SaveError::Cancelled)),
            None,
            "changing your mind is not a failure"
        );
        let generic = SaveError::Validation {
            copy: ValidationCopy::StemEmpty,
            detail: None,
        };
        assert_eq!(
            status_of(&Err(generic)),
            Some(ValidationCopy::StemEmpty.message().to_owned())
        );
        let filled = SaveError::Validation {
            copy: ValidationCopy::StemEmpty,
            detail: Some("文件名不能为空".to_owned()),
        };
        assert_eq!(
            status_of(&Err(filled)),
            Some("文件名不能为空".to_owned()),
            "a filled-in sentence wins over the generic one"
        );
    }

    #[test]
    fn the_synthetic_picture_is_the_size_it_claims_and_is_opaque() {
        let picture = Picture::synthetic();
        let (pixels, size) = picture.pixels();
        assert_eq!(size, PROBE_SOURCE);
        assert_eq!(pixels.len(), 64 * 64 * 4);
        assert!(
            pixels.chunks_exact(4).all(|pixel| pixel[3] == 0xff),
            "every pixel is opaque, so a jpeg has nothing to fill"
        );
    }

    #[test]
    fn the_menu_order_is_the_panels_own() {
        let words = snapclip_ui::panel::FMT_OPTIONS;
        for (index, word) in words.iter().enumerate() {
            let format = format_of(index);
            let expected = match *word {
                ".PNG" => SaveFormat::Png,
                ".JPG" => SaveFormat::Jpeg,
                ".WebP" => SaveFormat::Webp,
                other => panic!("a word the probe cannot map: {other}"),
            };
            assert_eq!(format, expected, "{word} is the format at index {index}");
        }
    }
}
