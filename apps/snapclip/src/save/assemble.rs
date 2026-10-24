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
use super::scale::Frame;
use super::service::{ConflictPolicy, SaveError, SaveOutcome, SaveRequest};
use super::template::{self, TemplateError, TemplateValues};

/// The panel's answers, in the shape a save needs them.
#[derive(Debug, Clone, Copy)]
pub struct PanelValues<'a> {
    /// What the path field holds. Empty or relative is refused by `service`, not here.
    pub directory: &'a str,
    /// What the name field holds, exactly as typed: `SnapClip_[日期]`.
    pub name: &'a str,
    /// The format the panel is set to.
    pub format: SaveFormat,
    /// The encoder quality the panel's tier asks for. Only JPEG spends it: PNG is a copy and this
    /// project's WebP is lossless (`docs/42` D1, spec `:94`).
    pub quality: u8,
    /// What to do when the name is taken. The panel's default is to rename.
    pub conflict: ConflictPolicy,
    /// The colour a transparent pixel becomes when the format cannot carry alpha (`S.fill`, `:724`).
    ///
    /// Straight-order `[blue, green, red]`, matching the buffers, because the fill is composited onto
    /// them channel by channel. Only JPEG uses it; PNG and this project's WebP keep their alpha.
    pub fill: [u8; 3],
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

/// The template a shell starts with: the spec's default (`:68`), which is also the value the
/// mockup puts in the name field (`:556`). Written down once so the field's first text and the
/// remembered template cannot disagree about what "the default" means.
pub const DEFAULT_TEMPLATE: &str = "SnapClip_[日期]";

/// What a shell remembers from one panel to the next: the name template and the next number.
///
/// The spec puts them in one sentence — "模板与序号随输入实时记住" (`:201`) — and they outlive a
/// panel for the same reason: a panel is opened per capture, so anything that is meant to survive
/// one has to live outside it. One struct rather than two handles because they have one lifetime
/// and one owner; the write points stay separate, because the template is written while the user
/// types while the number only moves after a save succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remembered {
    /// The template the next panel opens with.
    pub template: String,
    /// The number the next save will use.
    pub sequence: Sequence,
}

impl Default for Remembered {
    fn default() -> Self {
        Self {
            template: DEFAULT_TEMPLATE.to_owned(),
            sequence: Sequence::FIRST,
        }
    }
}

impl Remembered {
    /// Take an edited template, reporting whether anything changed.
    ///
    /// A rule of its own rather than three lines inside the message handler, because the case that
    /// matters can only be asserted outside a window: setting a control's text fires the same change
    /// notification as typing does, so the panel's own initial text arrives here as an edit. A rule
    /// that wrote back unconditionally would look identical on screen and wake the shell for nothing.
    pub fn remember_template(&mut self, edited: &str) -> bool {
        if edited == self.template {
            return false;
        }
        self.template = edited.to_owned();
        true
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

/// The encoder quality each tier asks for, from `docs/42` D8: **92 / 75 / 50**.
///
/// The first is the project's measured default (`docs/34` §11). The other two are the numbers that
/// decision recorded, marked there as not yet measured; they are used as recorded rather than
/// invented here, and the gap stays visible in `docs/42` instead of being quietly filled by a
/// number nobody will find again.
pub const TIER_QUALITY: [u8; 3] = [92, 75, 50];

/// The quality a tier index asks for. An index outside the three tiers is the first tier, which is
/// what the panel opens on; there is no fourth answer to give.
pub fn quality_of_tier(tier: usize) -> u8 {
    TIER_QUALITY.get(tier).copied().unwrap_or(TIER_QUALITY[0])
}

/// Saves the selection the panel is showing, at the size the panel asks for.
///
/// The pixels arrive exactly as the capture handed them over: `width * height * 4` bytes,
/// top down, tightly packed BGRA, at [`Frame::source`]'s size. [`Frame::output`] is what the size
/// fields say, and the file is written at that size — the two are only allowed to differ because
/// the resampling happens here, once, before the encoder sees a row.
///
/// The name's `[分辨率]` uses the output size, because that is the resolution of the file the
/// user gets, not of the selection it came from.
pub fn save_panel(values: &PanelValues<'_>, frame: &Frame<'_>) -> Result<SaveOutcome, SaveError> {
    let output = frame.output;
    let request = request_for(values, u64::from(output.width), u64::from(output.height))?;
    let scaled = super::scale::scaled(frame);
    // JPEG has no alpha, so a transparent pixel has to be composited over the fill the panel shows
    // before anything is encoded. Doing it here rather than inside the transcoder keeps the master a
    // faithful copy of the panel's picture for the formats that do carry alpha.
    let scaled = if request.format == SaveFormat::Jpeg {
        super::scale::over_fill(&scaled, values.fill)
    } else {
        scaled
    };
    let mut bands = WholeImage {
        bgra: &scaled,
        width: output.width,
        height: output.height,
    };
    if request.format == SaveFormat::Png {
        return bands::save_bands(&mut bands, output.width, &request);
    }
    // Every other format is a transcode of the same PNG master, so the encoder that exists writes
    // the master and the transcoder spends the quality exactly once. The format's size limits are
    // asked here because the banded path asks them for its own (PNG) format.
    super::caps::check(request.format, u64::from(output.width), u64::from(output.height))
        .map_err(SaveError::Capability)?;
    let master = bands::encode_png_bands(&mut bands, output.width)?;
    let encoded = super::transcode::to_bytes(&master, request.format, values.quality)?;
    super::service::commit_bytes(&request, &encoded)
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
    use crate::save::scale::{output_size, Size, SizeMode};

    fn values() -> PanelValues<'static> {
        PanelValues {
            directory: "C:\\shots",
            name: "SnapClip_[日期]",
            format: SaveFormat::Png,
            quality: TIER_QUALITY[0],
            conflict: ConflictPolicy::AutoRename,
            fill: [255, 255, 255],
            date: "2026-10-23",
            time: "1042",
            sequence: 7,
            app: "SnapClip",
        }
    }

    /// A directory of its own per test, removed by the caller when it is done with it.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "snapclip-assemble-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("a clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        dir
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
        let dir = scratch("assemble");

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
        let frame = Frame {
            bgra: &bgra,
            source: Size::new(width, height),
            output: Size::new(width, height),
        };
        let outcome = save_panel(&v, &frame).expect("a save on a writable directory");

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

    /// The size fields decide the file, not the selection: a capture that arrives 3x2 and a panel
    /// that asks for 6x4 must write a 6x4 PNG, and the name's resolution must be the written size
    /// rather than the one the pixels arrived with.
    #[test]
    fn the_panels_size_is_the_size_of_the_file() {
        let dir = scratch("assemble-scale");
        let source = Size::new(3, 2);
        let output = Size::new(6, 4);
        let mut bgra = Vec::new();
        for y in 0..source.height {
            for x in 0..source.width {
                bgra.extend_from_slice(&[(x as u8) * 40, (y as u8) * 80, 200, 255]);
            }
        }
        let mut v = values();
        v.directory = dir.to_str().expect("a utf-8 temporary path");
        v.name = "shot_[分辨率]";
        let frame = Frame {
            bgra: &bgra,
            source,
            output,
        };
        let outcome = save_panel(&v, &frame).expect("a save on a writable directory");

        let written = std::fs::read(&outcome.path).expect("the saved file");
        let png_width = u32::from_be_bytes([written[16], written[17], written[18], written[19]]);
        let png_height = u32::from_be_bytes([written[20], written[21], written[22], written[23]]);
        assert_eq!(
            (png_width, png_height),
            (output.width, output.height),
            "the file is the size the panel asked for"
        );
        assert_eq!(
            outcome.path.file_name().unwrap(),
            "shot_6x4.png",
            "and the name reports the written size, not the one the pixels arrived with"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The chosen format is the file's format: a JPEG request writes a file that starts with the
    /// JPEG marker and carries the extension the format owns.
    #[test]
    fn the_chosen_format_is_the_file_that_lands() {
        let dir = scratch("assemble-format");
        let size = Size::new(4, 3);
        let mut bgra = Vec::new();
        for y in 0..size.height {
            for x in 0..size.width {
                bgra.extend_from_slice(&[(x as u8) * 50, (y as u8) * 70, 180, 255]);
            }
        }
        let mut v = values();
        v.directory = dir.to_str().expect("a utf-8 temporary path");
        v.format = SaveFormat::Jpeg;
        v.quality = TIER_QUALITY[0];
        let frame = Frame {
            bgra: &bgra,
            source: size,
            output: size,
        };
        let outcome = save_panel(&v, &frame).expect("a jpeg save");

        assert_eq!(outcome.format, SaveFormat::Jpeg);
        assert_eq!(outcome.path.file_name().unwrap(), "SnapClip_2026-10-23.jpg");
        let written = std::fs::read(&outcome.path).expect("the saved file");
        assert_eq!(&written[..2], &[0xFF, 0xD8], "the body is a JPEG, not a renamed PNG");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The tiers are the numbers the decision recorded (`docs/42` D8), not numbers invented here.
    #[test]
    fn the_quality_tiers_are_the_recorded_numbers() {
        assert_eq!(quality_of_tier(0), 92, "the project's measured default");
        assert_eq!(quality_of_tier(1), 75);
        assert_eq!(quality_of_tier(2), 50);
        assert_eq!(quality_of_tier(9), 92, "an index outside the tiers is the first tier");
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

    /// A first run opens with the spec's template, which is also the mockup's input value.
    #[test]
    fn a_first_run_starts_with_the_specs_template_and_the_specs_number() {
        let fresh = Remembered::default();
        assert_eq!(fresh.template, "SnapClip_[日期]", "the spec's default (:68)");
        assert_eq!(fresh.sequence.next(), 1, "the spec's next sequence is one");
    }

    /// The template is written as it is typed, and only when it is different.
    ///
    /// The second half is the half that cannot be seen: setting a control's text fires the same
    /// change notification as typing, so the panel's own initial text arrives here too. A rule that
    /// wrote back unconditionally would look identical on screen.
    #[test]
    fn an_edited_template_is_taken_and_an_unchanged_one_is_not() {
        let mut remembered = Remembered::default();

        assert!(
            remembered.remember_template("我的_[序号]"),
            "a different template is a change"
        );
        assert_eq!(remembered.template, "我的_[序号]");

        assert!(
            !remembered.remember_template("我的_[序号]"),
            "the same text again is not a change"
        );
        assert_eq!(remembered.template, "我的_[序号]");

        assert!(
            remembered.remember_template(""),
            "an emptied box is still the user's choice, and the save decides whether it is legal"
        );
        assert_eq!(remembered.template, "");
    }

    /// The template and the number are one record with one owner, and each is written by its own
    /// event: typing writes the template, a successful save writes the number. Moving the number
    /// must not put the template back to what it was when the save started.
    #[test]
    fn moving_the_number_leaves_the_template_alone() {
        let mut remembered = Remembered::default();
        remembered.remember_template("我的_[序号]");

        let moved = remembered.sequence.after(&Ok::<(), SaveError>(()), "我的_[序号]");
        remembered.sequence = moved;

        assert_eq!(remembered.sequence.next(), 2, "the save consumed a number");
        assert_eq!(
            remembered.template, "我的_[序号]",
            "the template is the user's, not the save's"
        );
    }

    /// A percentage pair reaches the file: what the mode computes is the size in the png header.
    ///
    /// This is the chain the panel walks, minus the window: the two fields are read in percent, the
    /// mode turns them into a size, and the size is what lands. Asserting the intermediate `Size`
    /// alone would not say the file followed it.
    #[test]
    fn a_percentage_size_reaches_the_file() {
        let dir = scratch("assemble-percent");
        let source = Size::new(100, 60);
        let mut bgra = Vec::new();
        for y in 0..source.height {
            for x in 0..source.width {
                bgra.extend_from_slice(&[(x as u8).wrapping_mul(3), (y as u8).wrapping_mul(7), 90, 255]);
            }
        }
        let output = output_size(SizeMode::Percent, "50", "25", source);
        assert_eq!(
            output,
            Size::new(50, 15),
            "fifty percent wide and twenty five tall"
        );
        let frame = Frame {
            bgra: &bgra,
            source,
            output,
        };
        let mut v = values();
        v.directory = dir.to_str().expect("a utf-8 temporary path");
        v.name = "percent";
        let outcome = save_panel(&v, &frame).expect("a save on a writable directory");

        let written = std::fs::read(&outcome.path).expect("the saved file");
        let png_width = u32::from_be_bytes([written[16], written[17], written[18], written[19]]);
        let png_height = u32::from_be_bytes([written[20], written[21], written[22], written[23]]);
        assert_eq!(
            (png_width, png_height),
            (output.width, output.height),
            "the file is the size the percentage asked for, not the capture's size"
        );
    }
}
