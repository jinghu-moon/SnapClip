//! Turning what the panel holds into one save (`docs/41` S4).
//!
//! The panel owns values, not files: a directory, a name template, the chosen format, and the
//! pixels of the selection. This module is the one place that turns those into a
//! [`SaveRequest`] and hands them to the pipeline that already exists, so the panel never
//! learns how a file is written and the pipeline never learns what a panel is.
//!
//! Two decisions live here rather than in the window:
//!
//! * **The name is expanded at save time**, not while typing (`docs/42` D9): changing a name
//!   must never force a re-encode, and `[日期]` must mean the day the file was written.
//! * **A template that cannot be expanded is a validation failure**, carrying the same
//!   [`ValidationCopy`] keys the rest of the validation uses, with the specific complaint in
//!   `detail`. The detail is ascii on purpose: it is logged rather than drawn for now, and a
//!   drawn sentence would have to join the font subset.
//!
//! Nothing here reads a clock or a sequence counter. The caller passes both, which is what
//! makes "the name of a save" testable without waiting for a particular minute.

use std::path::PathBuf;

use snapclip_capture::ports::BandSource;
use snapclip_capture::save_format::SaveFormat;

use super::bands;
use super::copy::ValidationCopy;
use super::service::{ConflictPolicy, SaveError, SaveOutcome, SaveRequest};
use super::template::{self, TemplateError, TemplateValues};

/// The panel's answers, in the shape a save needs them.
#[derive(Debug, Clone, Copy)]
pub struct PanelValues<'a> {
    /// What the path field holds. Empty or relative is refused by `service`, not here.
    pub directory: &'a str,
    /// What the name field holds, exactly as typed: `SnapClip_[日期]`.
    pub name: &'a str,
    /// The format the panel is set to. Only PNG exists so far (`docs/41` S4).
    pub format: SaveFormat,
    /// What to do when the name is taken. The panel's default is to rename.
    pub conflict: ConflictPolicy,
    /// Values for the template's variables, all supplied by the caller.
    pub date: &'a str,
    pub time: &'a str,
    pub sequence: u32,
    pub app: &'a str,
}

/// The number the next save will use, and the rule that moves it.
///
/// The spec advances the counter only after a save succeeds, and only when the name actually asks
/// for it: a template without `[序号]` has no reason to consume a number. Cancelling and failing
/// leave it where it was, which is what makes the spec's "撤销保存时回退" true without an explicit
/// undo step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sequence(u32);

impl Sequence {
    /// The spec's "下一个序号" starts at one; nothing else in the project supplies a number.
    pub const FIRST: Self = Self(1);

    /// The number a save starting now would use.
    pub fn next(self) -> u32 {
        self.0
    }

    /// What the counter becomes after one attempt. Only success, and only a name that asks for
    /// the number, moves it.
    pub fn after<T>(self, result: &Result<T, SaveError>, template: &str) -> Self {
        match result {
            Ok(_) if template::uses_sequence(template) => Self(self.0.saturating_add(1)),
            _ => self,
        }
    }
}

/// The request one save will run with. Separate from [`save_panel`] so the name a save would
/// use can be asserted without touching a disk.
pub fn request_for(
    values: &PanelValues<'_>,
    width: u64,
    height: u64,
) -> Result<SaveRequest, SaveError> {
    let stem = template::expand(
        values.name,
        &TemplateValues {
            date: values.date,
            time: values.time,
            sequence: values.sequence,
            width,
            height,
            app: values.app,
        },
    )
    .map_err(template_failure)?;

    Ok(SaveRequest {
        directory: PathBuf::from(values.directory),
        stem,
        format: values.format,
        conflict: values.conflict,
    })
}

/// A template failure, expressed with the keys the rest of validation uses.
///
/// An empty template and a template that expands to nothing are both "there is no name"; a
/// stray bracket or a misspelled variable is a name with something in it that cannot be
/// allowed. `detail` says which, and stays ascii because it is logged rather than drawn.
fn template_failure(error: TemplateError) -> SaveError {
    let (copy, detail) = match error {
        TemplateError::Empty => (ValidationCopy::StemEmpty, "the name is empty".to_owned()),
        TemplateError::EmptyResult => (
            ValidationCopy::StemEmpty,
            "the name expands to nothing".to_owned(),
        ),
        TemplateError::Unclosed => (
            ValidationCopy::StemIllegalChars,
            "a '[' has no closing ']'".to_owned(),
        ),
        TemplateError::UnknownVariable(name) => (
            ValidationCopy::StemIllegalChars,
            format!("'{name}' is not one of the five variables"),
        ),
    };
    SaveError::Validation {
        copy,
        detail: Some(detail),
    }
}

/// Saves the selection the panel is showing.
///
/// The pixels arrive exactly as the capture handed them over: `width * height * 4` bytes,
/// top down, tightly packed BGRA. They are encoded as PNG and committed through the same
/// atomic path every other save uses, so an interrupted save cannot leave a truncated file
/// that looks whole.
pub fn save_panel(
    values: &PanelValues<'_>,
    width: u32,
    height: u32,
    bgra: &[u8],
) -> Result<SaveOutcome, SaveError> {
    let request = request_for(values, u64::from(width), u64::from(height))?;
    let mut bands = WholeImage {
        bgra,
        width,
        height,
    };
    bands::save_bands(&mut bands, width, &request)
}

/// The pixels the panel already holds, offered to the encoder a band at a time.
///
/// The banded path exists for captures that never exist whole (`docs/35`); a selection does
/// exist whole, so this source simply hands out the rows it was given. It is still a
/// [`BandSource`] rather than a second write path, because two ways to write a PNG is exactly
/// the kind of duplication the banded encoder replaced.
struct WholeImage<'a> {
    bgra: &'a [u8],
    width: u32,
    height: u32,
}

impl BandSource for WholeImage<'_> {
    fn rows(&self) -> u32 {
        self.height
    }

    fn read_rows(&mut self, first_row: u32, rows: u32, out: &mut Vec<u8>) -> bool {
        let stride = self.width as usize * 4;
        let Some(start) = (first_row as usize).checked_mul(stride) else {
            return false;
        };
        let Some(len) = (rows as usize).checked_mul(stride) else {
            return false;
        };
        let Some(end) = start.checked_add(len) else {
            return false;
        };
        if end > self.bgra.len() {
            return false;
        }
        out.clear();
        out.extend_from_slice(&self.bgra[start..end]);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values() -> PanelValues<'static> {
        PanelValues {
            directory: "C:\\shots",
            name: "SnapClip_[日期]",
            format: SaveFormat::Png,
            conflict: ConflictPolicy::AutoRename,
            date: "2026-10-23",
            time: "1042",
            sequence: 7,
            app: "SnapClip",
        }
    }

    /// The name a save would use is the template expanded with the selection's size, and it can
    /// be asked for without writing anything.
    #[test]
    fn the_request_expands_the_name_with_the_selections_size() {
        let mut v = values();
        v.name = "SnapClip_[日期]_[分辨率]";
        let request = request_for(&v, 1627, 1286).expect("a template of known variables");
        assert_eq!(request.stem, "SnapClip_2026-10-23_1627x1286");
        assert_eq!(request.directory, PathBuf::from("C:\\shots"));
        assert_eq!(request.format, SaveFormat::Png);
        assert_eq!(request.conflict, ConflictPolicy::AutoRename);
    }

    /// Width and height are two numbers of the same type, so a swap would look reasonable in a
    /// file name and only be wrong. The order is the assertion.
    #[test]
    fn the_width_comes_before_the_height() {
        let mut v = values();
        v.name = "[分辨率]";
        let request = request_for(&v, 1627, 1286).expect("the resolution variable");
        assert_eq!(request.stem, "1627x1286");
    }

    /// A template that cannot be expanded is refused before any file exists, with the key the
    /// rest of validation uses rather than a sentence of its own.
    #[test]
    fn a_broken_template_is_a_validation_failure() {
        let mut v = values();
        v.name = "";
        match request_for(&v, 10, 10) {
            Err(SaveError::Validation { copy, .. }) => assert_eq!(copy, ValidationCopy::StemEmpty),
            other => panic!("an empty name must be a validation failure, got {other:?}"),
        }

        v.name = "shot_[日期x]";
        match request_for(&v, 10, 10) {
            Err(SaveError::Validation { copy, detail }) => {
                assert_eq!(copy, ValidationCopy::StemIllegalChars);
                // The wording is ours and stays ascii, but the detail quotes the user's own word
                // back at them, so it carries whatever they typed. That is why this is logged
                // rather than drawn: a drawn sentence would pull their characters into the
                // font subset.
                let detail = detail.expect("a typo says which word was wrong");
                assert!(
                    detail.contains("日期x"),
                    "the detail names the word that was not a variable: {detail}"
                );
            }
            other => panic!("a misspelled variable must be a validation failure, got {other:?}"),
        }
    }

    /// The whole path, end to end, on a real file: a selection becomes a PNG whose header names
    /// the size it was handed, and the file is where the request said it would be.
    #[test]
    fn a_selection_becomes_a_png_of_the_size_it_was_given() {
        let dir = std::env::temp_dir().join(format!(
            "snapclip-assemble-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("a clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("a temporary directory");

        // 3x2, so a swap of the two numbers cannot pass: the PNG header records width then
        // height, and a square test image would hide exactly that mistake.
        let width = 3u32;
        let height = 2u32;
        let mut bgra = Vec::new();
        for y in 0..height {
            for x in 0..width {
                bgra.extend_from_slice(&[x as u8 * 40, y as u8 * 80, 200, 255]);
            }
        }

        let mut v = values();
        v.directory = dir.to_str().expect("a utf-8 temporary path");
        v.name = "shot_[分辨率]";
        let outcome = save_panel(&v, width, height, &bgra).expect("a save on a writable directory");

        assert_eq!(outcome.format, SaveFormat::Png);
        assert_eq!(outcome.path.file_name().unwrap(), "shot_3x2.png");
        let written = std::fs::read(&outcome.path).expect("the saved file");
        assert_eq!(u64::try_from(written.len()).unwrap(), outcome.bytes);
        assert_eq!(&written[1..4], b"PNG", "the file is a PNG, not a renamed body");
        // The IHDR payload starts at byte 16: width, then height, both big endian.
        let png_width = u32::from_be_bytes([written[16], written[17], written[18], written[19]]);
        let png_height = u32::from_be_bytes([written[20], written[21], written[22], written[23]]);
        assert_eq!(
            (png_width, png_height),
            (width, height),
            "the encoder is told width then height, and the header records the same order"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A band source that is asked for rows it does not have must refuse rather than hand back a
    /// short read, which would produce a file that looks whole.
    #[test]
    fn a_band_past_the_end_is_refused() {
        let bgra = vec![0u8; 4 * 4 * 4];
        let mut whole = WholeImage {
            bgra: &bgra,
            width: 4,
            height: 4,
        };
        let mut out = Vec::new();
        assert!(whole.read_rows(0, 4, &mut out), "all four rows are present");
        assert_eq!(out.len(), 4 * 4 * 4);
        assert!(!whole.read_rows(3, 2, &mut out), "rows 3..5 do not exist");
        assert!(!whole.read_rows(0, 5, &mut out), "five rows do not exist");
    }

    /// Success moves the number, and only when the name asks for it.
    #[test]
    fn a_successful_save_advances_the_sequence_when_the_name_asks_for_it() {
        let first = Sequence::FIRST;
        assert_eq!(first.next(), 1, "the spec's next sequence starts at one");

        let second = first.after(&Ok::<(), SaveError>(()), "SnapClip_[序号]");
        assert_eq!(second.next(), 2, "one save consumes one number");

        let third = second.after(&Ok::<(), SaveError>(()), "SnapClip_[序号]");
        assert_eq!(third.next(), 3, "and the next save the next one");
    }

    /// A template with no number in it has no reason to consume one.
    #[test]
    fn a_template_without_the_sequence_does_not_consume_a_number() {
        let first = Sequence::FIRST;
        let after = first.after(&Ok::<(), SaveError>(()), "SnapClip_[日期]");
        assert_eq!(after.next(), 1, "the name never asked for a number");
    }

    /// Neither cancelling nor failing uses a number up, which is the spec's "撤销保存时回退"
    /// without an explicit undo: the counter was never moved.
    #[test]
    fn neither_cancelling_nor_failing_uses_up_a_number() {
        let second = Sequence::FIRST.after(&Ok::<(), SaveError>(()), "SnapClip_[序号]");
        assert_eq!(second.next(), 2);

        let cancelled = second.after(&Err::<(), SaveError>(SaveError::Cancelled), "SnapClip_[序号]");
        assert_eq!(cancelled.next(), 2, "a cancelled save wrote no file");

        let failed: Result<(), SaveError> = Err(SaveError::Validation {
            copy: ValidationCopy::StemEmpty,
            detail: None,
        });
        let after_failure = second.after(&failed, "SnapClip_[序号]");
        assert_eq!(after_failure.next(), 2, "a failed save wrote no file either");
    }
}
