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
use crate::save::service::{ConflictPolicy, SaveError, SaveOutcome};
use std::sync::{Mutex, OnceLock};
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

    /// A picture whose middle is fully transparent, so a fill can be seen changing it.
    ///
    /// Test input, like [`Self::synthetic`]: the probe never shows it, and the only place it can be
    /// observed is the file a save writes.
    pub fn with_transparent_middle() -> Self {
        let mut bgra = checker_bgra(PROBE_SOURCE);
        let (w, _h) = (PROBE_SOURCE.width as usize, PROBE_SOURCE.height as usize);
        for y in 16..48 {
            for x in 16..48 {
                let at = (y * w + x) * 4;
                bgra[at..at + 4].copy_from_slice(&[0, 0, 0, 0]);
            }
        }
        Self {
            bgra,
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
    quality_tier: usize,
    fill: [u8; 3],
) -> PanelValues<'a> {
    values_with(
        directory,
        name,
        format,
        date,
        time,
        quality_tier,
        fill,
        next_sequence(),
        ConflictPolicy::AutoRename,
    )
}

/// The same, with the number and the conflict policy spelled out.
///
/// Two functions rather than one with defaults, because Rust has none: the short one exists so a
/// caller that has no opinion cannot accidentally pass the wrong number, and the long one exists for
/// the window, which has both.
#[allow(clippy::too_many_arguments)]
pub fn values_with<'a>(
    directory: &'a str,
    name: &'a str,
    format: SaveFormat,
    date: &'a str,
    time: &'a str,
    quality_tier: usize,
    fill: [u8; 3],
    sequence: u32,
    conflict: ConflictPolicy,
) -> PanelValues<'a> {
    PanelValues {
        directory,
        name,
        format,
        // The tier the view chose, turned into the recorded number in one place (`docs/42` D8), so
        // the probe cannot describe a quality the encoder would not receive.
        quality: assemble::quality_of_tier(quality_tier),
        // What the footer's link offered, so the file that lands obeys the choice the user saw.
        conflict,
        fill,
        date,
        time,
        // The number the shell's record holds, not a fresh one: `[序号]` in the name would otherwise
        // be the same number every time the probe is opened.
        sequence,
        app: crate::save::window::APP_NAME,
    }
}

/// What the shell remembers between panels: the template, the next number and the two switches.
///
/// The probe is one process, so one record is enough; it is a `static` rather than a field on the
/// view because the point of remembering is that a *second* view sees what the first one left.
fn remembered() -> &'static Mutex<assemble::Remembered> {
    static RECORD: OnceLock<Mutex<assemble::Remembered>> = OnceLock::new();
    RECORD.get_or_init(|| Mutex::new(assemble::Remembered::default()))
}

/// The template the next probe window opens with.
pub fn remembered_template() -> String {
    remembered()
        .lock()
        .map(|record| record.template.clone())
        .unwrap_or_else(|_| assemble::DEFAULT_TEMPLATE.to_owned())
}

/// The number the next save would use.
pub fn next_sequence() -> u32 {
    remembered()
        .lock()
        .map(|record| record.sequence.next())
        .unwrap_or(assemble::Sequence::FIRST.next())
}

/// The two after-save switches as last chosen.
pub fn remembered_after() -> [bool; 2] {
    remembered()
        .lock()
        .map(|record| record.after)
        .unwrap_or([false; 2])
}

/// Remember a typed template, reporting whether anything changed.
pub fn remember_template(edited: &str) -> bool {
    remembered()
        .lock()
        .map(|mut record| record.remember_template(edited))
        .unwrap_or(false)
}

/// Remember the two switches, reporting whether anything changed.
pub fn remember_after(after: [bool; 2]) -> bool {
    remembered()
        .lock()
        .map(|mut record| record.remember_after(after))
        .unwrap_or(false)
}

/// Move the number on after an attempt, and remember the template the attempt used.
///
/// The rule for when the number moves is [`assemble::Sequence::after`], which only advances on a
/// success whose template actually asks for the number; running it here keeps every caller — the
/// window and the tests — on the same rule.
pub fn note_attempt<T>(result: &Result<T, SaveError>, template: &str) {
    if let Ok(mut record) = remembered().lock() {
        record.sequence = record.sequence.after(result, template);
    }
}

/// The two switches' work after a successful save, in the panel's own order.
///
/// Passed to the panel's own implementation so the order and the shell calls exist once; the probe
/// only decides which of the two is on.
pub fn run_after_save(after: [bool; 2], path: &std::path::Path) {
    crate::save::window::run_after_save(after, path);
}

/// Whether the footer offers to change the conflict policy, and which sentence it shows.
///
/// The rule is the panel's: the link exists only while the plain name is taken (`:1149`), and the
/// word is the *action* rather than the state in force (`:1150`), because a state reads as a
/// statement instead of something to press.
pub fn conflict_link(
    values: &PanelValues<'_>,
    width: u32,
    height: u32,
    conflict: ConflictPolicy,
) -> Option<&'static str> {
    use crate::save::copy::ConflictLink;
    let request = assemble::request_for(values, width as u64, height as u64).ok()?;
    let plain = crate::save::service::plain_target(&request).ok()?;
    if !plain.exists() {
        return None;
    }
    Some(match conflict {
        ConflictPolicy::AutoRename => ConflictLink::Overwrite.text(),
        ConflictPolicy::Overwrite => ConflictLink::Rename.text(),
    })
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

/// The size that fits the limit, from a real encode at each candidate size.
///
/// The search is the panel's own (`estimate::fit_under`), so the probe cannot shrink differently
/// from the drawn panel. Returns nothing when no candidate fits, which is the panel's answer too:
/// it writes no file rather than one over the limit.
pub fn shrink_to_fit(
    picture: &Picture,
    values: &PanelValues<'_>,
    output: Size,
    limit: u64,
) -> Option<Size> {
    estimate::fit_under(output, limit, |size| {
        assemble::weigh(values, &picture.frame(size)).ok()
    })
    .map(|(size, _bytes)| size)
}

/// Write the file, shrinking it first when a limit is given.
///
/// A limit the picture already meets writes at its own size; one it does not writes at the size the
/// search found, so the file on disk is under the limit the footer promised. `None` means no size
/// fits, and nothing is written: the panel's own answer, recorded as a gap in `docs/41` because the
/// mockup has no sentence for it.
pub fn save_within(
    picture: &Picture,
    values: &PanelValues<'_>,
    output: Size,
    limit: Option<u64>,
) -> Option<Result<SaveOutcome, SaveError>> {
    let Some(limit) = limit else {
        return Some(save(picture, values, output));
    };
    shrink_to_fit(picture, values, output, limit).map(|size| save(picture, values, size))
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

    /// The ceiling is a real ceiling: what it lets through is under it, and what it cannot meet
    /// writes nothing at all.
    ///
    /// The synthetic picture is small and compresses to a few hundred bytes, so a ceiling above it
    /// must leave the size alone and one below it must refuse; both are read off the disk rather
    /// than from the return value, because "the limit was applied" is a fact about the file.
    #[gpui_kit::test]
    fn the_ceiling_decides_what_lands_and_writes_nothing_when_it_cannot_be_met() {
        let dir = temp_dir("ceiling");
        let picture = Picture::synthetic();
        let output = Size::new(96, 64);

        let roomy = values(
            dir.to_str().expect("utf-8 path"),
            "roomy",
            format_of(0),
            "2026-10-26",
            "0005",
            0,
            [0xff, 0xff, 0xff],
        );
        let written = save_within(&picture, &roomy, output, Some(1024 * 1024))
            .expect("a roomy ceiling still saves")
            .expect("and the save succeeds");
        let bytes = std::fs::read(&written.path).expect("the file is there");
        assert!(bytes.len() as u64 <= 1024 * 1024, "the file is under the ceiling");
        assert_eq!(png_size(&bytes), (96, 64), "a ceiling it already meets is not a reason to shrink");

        let cramped = values(
            dir.to_str().expect("utf-8 path"),
            "cramped",
            format_of(0),
            "2026-10-26",
            "0005",
            0,
            [0xff, 0xff, 0xff],
        );
        let nothing = save_within(&picture, &cramped, output, Some(16));
        assert!(nothing.is_none(), "sixteen bytes cannot hold a png, so nothing is written");
        let landed = std::fs::read_dir(&dir)
            .expect("the folder is readable")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("cramped"))
            .count();
        assert_eq!(landed, 0, "a ceiling nothing meets leaves no file behind");

        let off = save_within(&picture, &roomy, output, None).expect("no ceiling always attempts");
        assert_eq!(off.expect("and succeeds").bytes, written.bytes, "no ceiling is the plain save");
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
            0,
            [0xff, 0xff, 0xff],
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
            0,
            [0xff, 0xff, 0xff],
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

    /// The tier reaches the encoder: three tiers, three sizes, in order.
    ///
    /// The bytes are read from the files that landed, so a tier that is chosen but never used by
    /// the encoder fails here rather than passing on a value that looks right.
    #[test]
    fn a_lower_quality_tier_writes_fewer_bytes() {
        let dir = temp_dir("tiers");
        // Noise compresses badly at every tier, so the three sizes differ rather than all hitting
        // the same floor.
        let mut bgra = Vec::new();
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..(128 * 96) {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            bgra.push((seed & 0xff) as u8);
            bgra.push(((seed >> 8) & 0xff) as u8);
            bgra.push(((seed >> 16) & 0xff) as u8);
            bgra.push(0xff);
        }
        let picture = Picture {
            bgra,
            source: Size::new(128, 96),
        };
        let mut sizes = Vec::new();
        for tier in 0..3 {
            let name = format!("tier-{tier}");
            let values = values(
                dir.to_str().expect("utf-8 path"),
                &name,
                format_of(1),
                "2026-10-26",
                "0005",
                tier,
                [0xff, 0xff, 0xff],
            );
            let written = save(&picture, &values, Size::new(128, 96)).expect("the save succeeds");
            sizes.push(written.bytes);
        }
        assert!(
            sizes[0] > sizes[1] && sizes[1] > sizes[2],
            "each tier writes fewer bytes than the one above it: {sizes:?}"
        );
    }

    /// The fill reaches the encoder, for a format that fills and not for one that does not.
    #[test]
    fn the_chosen_fill_colours_a_jpegs_transparent_pixels() {
        let dir = temp_dir("fill");
        let picture = Picture::with_transparent_middle();
        let size = Size::new(64, 64);
        let middle = |bytes: &[u8]| {
            let image = image::load_from_memory(bytes).expect("the file decodes");
            image.to_rgba8().get_pixel(32, 32).0
        };

        let black = values(
            dir.to_str().expect("utf-8 path"),
            "fill-black",
            format_of(1),
            "2026-10-26",
            "0005",
            0,
            [0x00, 0x00, 0x00],
        );
        let written = save(&picture, &black, size).expect("the save succeeds");
        let bytes = std::fs::read(&written.path).expect("the file is there");
        let pixel = middle(&bytes);
        assert!(
            pixel[0] < 24 && pixel[1] < 24 && pixel[2] < 24,
            "the transparent middle took the black fill: {pixel:?}"
        );

        let white = values(
            dir.to_str().expect("utf-8 path"),
            "fill-white",
            format_of(1),
            "2026-10-26",
            "0005",
            0,
            [0xff, 0xff, 0xff],
        );
        let written = save(&picture, &white, size).expect("the save succeeds");
        let bytes = std::fs::read(&written.path).expect("the file is there");
        let pixel = middle(&bytes);
        assert!(
            pixel[0] > 180 && pixel[1] > 180 && pixel[2] > 180,
            "the same pixel takes the white fill instead, so the fill is read: {pixel:?}"
        );

        let transparent = values(
            dir.to_str().expect("utf-8 path"),
            "fill-png",
            format_of(0),
            "2026-10-26",
            "0005",
            0,
            [0x00, 0x00, 0x00],
        );
        let written = save(&picture, &transparent, size).expect("the save succeeds");
        let bytes = std::fs::read(&written.path).expect("the file is there");
        let image = image::load_from_memory(&bytes).expect("the file decodes");
        assert_eq!(
            image.to_rgba8().get_pixel(32, 32).0[3],
            0,
            "png keeps the transparency, so the fill is not applied to it"
        );
    }

    /// The number and the policy the view chose reach the values, and the link's word is the
    /// policy that is *not* in force.
    ///
    /// Three facts in one test because they are one decision: what the footer offers and what the
    /// save does have to be the same choice, and both come from these two fields.
    #[gpui_kit::test]
    fn the_chosen_number_and_policy_reach_the_save() {
        let dir = temp_dir("policy");
        let output = Size::new(96, 64);
        let plain = dir.join("shot.png");

        let chosen = values_with(
            dir.to_str().expect("utf-8 path"),
            "shot",
            SaveFormat::Png,
            "2026-10-26",
            "0005",
            0,
            [0xff, 0xff, 0xff],
            7,
            ConflictPolicy::Overwrite,
        );
        assert_eq!(chosen.sequence, 7, "the number the shell holds is the one the name gets");
        assert_eq!(chosen.conflict, ConflictPolicy::Overwrite, "the choice travels with the values");

        std::fs::write(&plain, b"last night").expect("the test can write a sentinel");

        // While the plain name is taken, the link offers the other policy, in words that are an
        // action rather than a state (`:1150`).
        let renaming = values_with(
            dir.to_str().expect("utf-8 path"),
            "shot",
            SaveFormat::Png,
            "2026-10-26",
            "0005",
            0,
            [0xff, 0xff, 0xff],
            1,
            ConflictPolicy::AutoRename,
        );
        assert_eq!(
            conflict_link(&renaming, output.width, output.height, ConflictPolicy::AutoRename),
            Some("改为覆盖"),
            "a taken name offers to overwrite"
        );
        assert_eq!(
            conflict_link(&renaming, output.width, output.height, ConflictPolicy::Overwrite),
            Some("改为重命名"),
            "an overwriting save offers to rename instead"
        );

        // The choice really decides what lands: renaming keeps both files and the sentinel intact.
        let kept = save(&Picture::synthetic(), &renaming, output).expect("the probe can save");
        assert_ne!(kept.path, plain, "a taken name is not reused while renaming");
        assert_eq!(
            std::fs::read(&plain).expect("the sentinel is still there"),
            b"last night",
            "renaming has to leave the existing file alone"
        );
        assert!(kept.path.exists(), "and put the new one beside it");

        // Overwriting keeps the name and replaces the bytes.
        let overwriting = values_with(
            dir.to_str().expect("utf-8 path"),
            "shot",
            SaveFormat::Png,
            "2026-10-26",
            "0005",
            0,
            [0xff, 0xff, 0xff],
            1,
            ConflictPolicy::Overwrite,
        );
        let replaced = save(&Picture::synthetic(), &overwriting, output).expect("the probe can save");
        assert_eq!(replaced.path, plain, "overwriting keeps the name");
        assert_ne!(
            std::fs::read(&plain).expect("the file is there"),
            b"last night",
            "overwriting replaces the bytes"
        );

        // With the name free again the link has nothing to offer.
        std::fs::remove_file(&plain).expect("the test can remove it");
        assert_eq!(
            conflict_link(&renaming, output.width, output.height, ConflictPolicy::AutoRename),
            None,
            "a free name has nothing to link to"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
