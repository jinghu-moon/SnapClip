//! The act of writing: validate the destination, encode or copy, commit atomically.
//!
//! This batch covers **PNG only** (`docs/41` S1, first slice). Other formats arrive with the
//! encoder service in S2; the signature does not change when they do.

use std::io::Write as _;
use std::path::Path;

use snapclip_capture::save_format::SaveFormat;

/// What to do when the target name is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    /// Add `_2`, `_3`, … until the name is free. The default: a screenshot that quietly
    /// overwrites last night's screenshot is a data-loss bug, not a convenience.
    AutoRename,
    /// Replace the existing file, atomically: the old bytes survive until the rename.
    Overwrite,
}

/// Where to put the result, and under what name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveRequest {
    /// Absolute path. Empty or relative is an error rather than a guess.
    pub directory: std::path::PathBuf,
    /// File name without extension; the extension comes from `format`.
    pub stem: String,
    pub format: SaveFormat,
    pub conflict: ConflictPolicy,
}

/// What a save produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveOutcome {
    pub path: std::path::PathBuf,
    pub bytes: u64,
    pub format: SaveFormat,
    /// Warnings the save survived, so the panel can show the same sentence when it does not
    /// (a trailing dot is removed, not refused, and the user is still told).
    pub warnings: Vec<super::copy::ValidationCopy>,
}

/// Why a save did not happen.
#[derive(Debug)]
pub enum SaveError {
    /// A rule from the spec's validation table. The **key** travels, not a sentence: the panel
    /// renders it from `super::copy`, so one mistake cannot be described two ways.
    Validation {
        copy: super::copy::ValidationCopy,
        /// The filled-in sentence for the messages that have a placeholder, already formatted by
        /// the formatters in `super::copy`.
        detail: Option<String>,
    },
    /// The format cannot hold an image of this size (`super::caps`). Asked before any byte is
    /// written, and before "this build has no encoder yet", because the first is about the request
    /// and the second is about the build.
    Capability(super::caps::CapabilityError),
    /// This batch has no encoder for the format yet (`docs/41` S2 adds the rest).
    UnsupportedFormat(SaveFormat),
    NotEnoughSpace(String),
    Io(String),
}

impl SaveError {
    /// The sentence a person should read, from the one place that owns the wording.
    pub fn message(&self) -> String {
        match self {
            SaveError::Validation { copy, detail } => {
                detail.clone().unwrap_or_else(|| copy.message().to_owned())
            }
            SaveError::Capability(why) => format!("{why:?}"),
            SaveError::UnsupportedFormat(format) => {
                format!("this build cannot save as {format:?} yet")
            }
            SaveError::NotEnoughSpace(why) => format!("not enough space: {why}"),
            SaveError::Io(why) => format!("the file cannot be written: {why}"),
        }
    }
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for SaveError {}

/// Saves the PNG master at `master` according to `request`.
///
/// The master is the contract (`docs/34` §16): a PNG save is its bytes, so quality can never be
/// spent twice. The write is **atomic**: the body goes to `<target>.partial`, is flushed to the
/// device, and only then renamed onto the target, so an interrupted save cannot leave a
/// plausible-looking truncated file (and a failed one leaves the directory as it found it).
pub fn save(master: &Path, request: &SaveRequest) -> Result<SaveOutcome, SaveError> {
    let (stem, warnings) = prepare(request)?;
    commit(request, &stem, |sink| {
        let mut source = std::fs::File::open(master)?;
        std::io::copy(&mut source, sink)?;
        Ok(())
    })
    .map(|(path, bytes)| SaveOutcome {
        path,
        bytes,
        format: request.format,
        warnings,
    })
}

/// Saves bytes that were produced elsewhere (the banded encoder, and later the transcoder).
pub fn commit_bytes(request: &SaveRequest, bytes: &[u8]) -> Result<SaveOutcome, SaveError> {
    let (stem, warnings) = prepare(request)?;
    commit(request, &stem, |sink| sink.write_all(bytes)).map(|(path, written)| SaveOutcome {
        path,
        bytes: written,
        format: request.format,
        warnings,
    })
}

/// Validates the request and returns the stem the files should be named after, plus the warnings
/// that did not stop it. Every refusal carries a [`super::copy::ValidationCopy`] key.
fn prepare(request: &SaveRequest) -> Result<(String, Vec<super::copy::ValidationCopy>), SaveError> {
    use super::copy::{self, ValidationCopy};

    if request.directory.as_os_str().is_empty() {
        return Err(SaveError::Validation {
            copy: ValidationCopy::PathEmpty,
            detail: None,
        });
    }
    if !request.directory.is_absolute() {
        return Err(SaveError::Validation {
            copy: ValidationCopy::PathNotAbsolute,
            detail: None,
        });
    }
    let directory = request.directory.to_string_lossy();
    // A drive letter's colon is structure; any other colon (or any of the reserved set) is not.
    let tail = directory.get(2..).unwrap_or("");
    if let Some(bad) = tail
        .chars()
        .find(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') || c.is_control())
    {
        let _ = bad;
        return Err(SaveError::Validation {
            copy: ValidationCopy::PathIllegalChars,
            detail: None,
        });
    }

    let mut warnings = Vec::new();
    let (stem, trailing) = trim_trailing(&request.stem);
    if trailing {
        // The spec makes this a warning with a repair, so the save goes ahead with the name the
        // filesystem would have produced anyway, and the panel still says so.
        warnings.push(ValidationCopy::StemTrailingDotOrSpace);
    }
    if stem.trim().is_empty() {
        return Err(SaveError::Validation {
            copy: ValidationCopy::StemEmpty,
            detail: None,
        });
    }
    if let Some(list) = illegal_stem_chars(&stem) {
        return Err(SaveError::Validation {
            copy: ValidationCopy::StemIllegalChars,
            detail: Some(copy::illegal_chars(&list)),
        });
    }
    if is_reserved(&stem) {
        return Err(SaveError::Validation {
            copy: ValidationCopy::StemReservedName,
            detail: Some(copy::reserved_name(&stem)),
        });
    }
    Ok((stem, warnings))
}

/// Drops the dots and spaces Windows would drop, reporting whether anything was removed.
fn trim_trailing(stem: &str) -> (String, bool) {
    let trimmed = stem.trim_end_matches([' ', '.']);
    (trimmed.to_owned(), trimmed.len() != stem.len())
}

/// The distinct illegal characters of a stem, in the order they first appear.
fn illegal_stem_chars(stem: &str) -> Option<String> {
    let mut seen = Vec::new();
    for c in stem.chars() {
        if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
            if !seen.contains(&c) {
                seen.push(c);
            }
        }
    }
    if seen.is_empty() {
        None
    } else {
        Some(seen.into_iter().collect())
    }
}

/// `CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`, with or without an extension: a file
/// with one of those names opens the device instead.
fn is_reserved(stem: &str) -> bool {
    let base = stem.split('.').next().unwrap_or(stem).to_ascii_uppercase();
    if matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    let mut chars = base.chars();
    let (Some(prefix), Some(digit), None) = (chars.next(), chars.next(), chars.next()) else {
        return false;
    };
    matches!(prefix, 'C' | 'L') && matches!(digit, '1'..='9') && base.len() == 4
        && (base.starts_with("COM") || base.starts_with("LPT"))
}

/// The one place a file is committed: body into `.partial`, flush to the device, rename.
fn commit(
    request: &SaveRequest,
    stem: &str,
    write_body: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> Result<(std::path::PathBuf, u64), SaveError> {
    let extension = extension_of(request.format);
    let target = resolve(&request.directory, stem, extension, request.conflict);
    if let Some(too_long) = too_long(&target) {
        return Err(SaveError::Validation {
            copy: super::copy::ValidationCopy::PathTooLong,
            detail: Some(format!(
                "{} is {too_long} characters; the limit is 260",
                target.display()
            )),
        });
    }
    std::fs::create_dir_all(&request.directory).map_err(io_error)?;

    let partial = partial_path(&target);
    let written = (|| -> std::io::Result<u64> {
        let mut sink = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)?;
        write_body(&mut sink)?;
        // Flush to the device before the rename: otherwise a crash can commit a directory entry
        // that points at zero-filled blocks.
        sink.sync_all()?;
        drop(sink);
        std::fs::rename(&partial, &target)?;
        Ok(std::fs::metadata(&target)?.len())
    })();

    match written {
        Ok(bytes) => Ok((target, bytes)),
        Err(error) => {
            // The final file is the only thing a user can see, so a failure must not leave one.
            // The rename is the commit point: before it there is nothing but the `.partial`.
            let _ = std::fs::remove_file(&partial);
            Err(io_error(error))
        }
    }
}

/// Windows refuses paths past 260 characters unless long paths are enabled, and the spec's rule is
/// written for the common case.
fn too_long(path: &Path) -> Option<usize> {
    let length = path.as_os_str().to_string_lossy().chars().count();
    (length > 260).then_some(length)
}

/// Picks the target path the policy asks for.
fn resolve(
    directory: &Path,
    stem: &str,
    extension: &str,
    conflict: ConflictPolicy,
) -> std::path::PathBuf {
    let plain = directory.join(format!("{stem}.{extension}"));
    match conflict {
        ConflictPolicy::Overwrite => plain,
        ConflictPolicy::AutoRename => {
            // `exists()` counts a directory as taken, which is what we want: renaming onto one
            // fails at the commit, and "shot.png is a folder" is not a free name.
            if !plain.exists() {
                return plain;
            }
            for index in 2u32.. {
                let candidate = directory.join(format!("{stem}_{index}.{extension}"));
                if !candidate.exists() {
                    return candidate;
                }
            }
            unreachable!("u32 cannot run out of names")
        }
    }
}

/// The extension the file gets. Kept beside the commit rather than on `SaveFormat`, because the
/// format crate knows what bytes are legal and the save step knows what a user expects to see.
fn extension_of(format: SaveFormat) -> &'static str {
    match format {
        SaveFormat::Png => "png",
        SaveFormat::Jpeg => "jpg",
        SaveFormat::Webp => "webp",
        SaveFormat::Bmp => "bmp",
    }
}

fn partial_path(target: &Path) -> std::path::PathBuf {
    let mut name = target.as_os_str().to_owned();
    name.push(".partial");
    std::path::PathBuf::from(name)
}

/// Disk-full is a distinct answer, because "the disk is full" and "this folder is read-only" lead
/// to different advice.
fn io_error(error: std::io::Error) -> SaveError {
    match error.raw_os_error() {
        // ERROR_DISK_FULL / ENOSPC.
        Some(112) | Some(39) => SaveError::NotEnoughSpace(error.to_string()),
        _ => SaveError::Io(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// A directory this test owns outright.
    ///
    /// The name carries the pid, and the pid can be reused across runs: a stale directory from an
    /// earlier process would then be mistaken for this test's work (a leftover `shot.png` makes
    /// the rename test look like it did not rename). So removal is checked rather than hoped for,
    /// and a directory that will not go away fails loudly instead of skewing the assertions.
    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "snapclip-save-{}-{label}",
            std::process::id()
        ));
        match fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("the scratch directory {} cannot be cleared: {error}", dir.display()),
        }
        fs::create_dir_all(&dir).expect("scratch directory");
        dir
    }

    /// A real 2x2 PNG, so "byte for byte" means the bytes of a file a decoder accepts.
    fn master(dir: &Path) -> PathBuf {
        let path = dir.join("master.png");
        let file = fs::File::create(&path).expect("master file");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), 2, 2);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer
            .write_image_data(&[
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ])
            .expect("png data");
        drop(writer);
        path
    }

    fn request(directory: &Path, conflict: ConflictPolicy) -> SaveRequest {
        SaveRequest {
            directory: directory.to_path_buf(),
            stem: "shot".to_owned(),
            format: SaveFormat::Png,
            conflict,
        }
    }

    fn partial_files(directory: &Path) -> Vec<PathBuf> {
        fs::read_dir(directory)
            .expect("read directory")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".partial"))
            })
            .collect()
    }

    /// A PNG request must be the master's bytes, not a re-encode: decoding and re-encoding a
    /// lossless file spends seconds to produce a different file with the same pixels.
    #[test]
    fn a_png_request_copies_the_master_byte_for_byte() {
        let dir = scratch("png-copy");
        let master = master(&dir);
        let out = dir.join("out");

        let outcome = save(&master, &request(&out, ConflictPolicy::AutoRename))
            .expect("a PNG request should succeed");

        assert_eq!(outcome.path, out.join("shot.png"), "the name comes from the request");
        assert_eq!(outcome.format, SaveFormat::Png);
        assert_eq!(
            outcome.bytes,
            fs::metadata(&master).expect("master metadata").len(),
            "the outcome reports the bytes actually written"
        );
        assert_eq!(
            fs::read(&outcome.path).expect("written file"),
            fs::read(&master).expect("master"),
            "a PNG save is a byte copy of the master"
        );
    }

    /// Two screenshots must not become one file by accident.
    #[test]
    fn a_second_png_save_renames_instead_of_overwriting() {
        let dir = scratch("auto-rename");
        let master = master(&dir);
        let out = dir.join("out");

        let first = save(&master, &request(&out, ConflictPolicy::AutoRename)).expect("first save");
        let second = save(&master, &request(&out, ConflictPolicy::AutoRename)).expect("second save");

        assert_eq!(first.path, out.join("shot.png"));
        assert_eq!(second.path, out.join("shot_2.png"), "the second save steps to _2");
        assert!(first.path.exists(), "the first file is still there");
        assert_eq!(
            fs::read(&first.path).expect("first"),
            fs::read(&second.path).expect("second"),
            "both are copies of the same master"
        );
    }

    /// The other policy replaces, and replacement must not leave a half-written body behind.
    #[test]
    fn overwriting_a_name_replaces_it_and_no_partial_survives() {
        let dir = scratch("overwrite");
        let master = master(&dir);
        let out = dir.join("out");

        let first = save(&master, &request(&out, ConflictPolicy::Overwrite)).expect("first save");
        fs::write(&first.path, b"stale bytes").expect("age the first file");
        let second = save(&master, &request(&out, ConflictPolicy::Overwrite)).expect("second save");

        assert_eq!(second.path, first.path, "overwrite keeps the name");
        assert_eq!(
            fs::read(&second.path).expect("replaced"),
            fs::read(&master).expect("master"),
            "the old bytes are gone"
        );
        assert!(
            partial_files(&out).is_empty(),
            "committing must leave no .partial behind: {:?}",
            partial_files(&out)
        );
    }

    /// A save that cannot commit must leave the directory exactly as it found it: no final
    /// file, no `.partial`, and no half-updated original.
    #[test]
    fn a_failed_write_leaves_no_final_file_and_no_partial() {
        let dir = scratch("failed-write");
        let master = master(&dir);
        let out = dir.join("out");
        fs::create_dir_all(&out).expect("out");
        // The target name is occupied by a directory, so the final rename cannot succeed.
        fs::create_dir(out.join("shot.png")).expect("blocking directory");

        let error = save(&master, &request(&out, ConflictPolicy::Overwrite))
            .expect_err("a rename onto a directory cannot succeed");

        assert!(
            matches!(error, SaveError::Io(_)),
            "the failure is an I/O error, got {error:?}"
        );
        assert!(
            partial_files(&out).is_empty(),
            "a failed save cleans its .partial: {:?}",
            partial_files(&out)
        );
        assert!(
            out.join("shot.png").is_dir(),
            "the obstruction is untouched: the save wrote nothing at that name"
        );
    }

    fn request_with_stem(dir: &Path, stem: &str) -> SaveRequest {
        SaveRequest {
            directory: dir.to_path_buf(),
            stem: stem.to_owned(),
            format: SaveFormat::Png,
            conflict: ConflictPolicy::AutoRename,
        }
    }

    /// An empty name is refused with the spec's **key**, not with a sentence of our own: the panel
    /// renders `文件名不能为空` from one place, so a second wording cannot drift in.
    #[test]
    fn an_empty_stem_is_refused_with_the_copy_key() {
        let dir = scratch("empty-stem");
        let master = master(&dir);
        let out = dir.join("out");
        let error = save(&master, &request_with_stem(&out, "  ")).expect_err("an empty name");
        assert!(matches!(
            error,
            SaveError::Validation { copy: super::super::copy::ValidationCopy::StemEmpty, .. }
        ));
        assert!(
            fs::read_dir(&out).map(|mut d| d.next().is_none()).unwrap_or(true),
            "nothing was written for a refused name"
        );
    }

    /// The illegal characters are listed once each, in the order they appear, inside the spec's
    /// sentence — the formatter owns the wording, the caller owns the list.
    #[test]
    fn illegal_characters_are_listed_once_each() {
        let dir = scratch("illegal");
        let master = master(&dir);
        let out = dir.join("out");
        let error = save(&master, &request_with_stem(&out, "a<>a<")).expect_err("illegal name");
        match error {
            SaveError::Validation { copy, detail } => {
                assert_eq!(copy, super::super::copy::ValidationCopy::StemIllegalChars);
                assert_eq!(detail.as_deref(), Some("含非法字符 <>"));
            }
            other => panic!("expected the illegal-character key, got {other:?}"),
        }
    }

    /// A device name opens the device, not a file, so it is refused with the name quoted in the
    /// spec's full-width quotes.
    #[test]
    fn a_reserved_name_is_refused_with_its_own_sentence() {
        let dir = scratch("reserved");
        let master = master(&dir);
        let out = dir.join("out");
        let error = save(&master, &request_with_stem(&out, "con")).expect_err("a device name");
        match error {
            SaveError::Validation { copy, detail } => {
                assert_eq!(copy, super::super::copy::ValidationCopy::StemReservedName);
                assert_eq!(detail.as_deref(), Some("“con”是系统保留名"));
            }
            other => panic!("expected the reserved-name key, got {other:?}"),
        }
    }

    /// A trailing dot is a **warning** with a repair, so the save proceeds under the name Windows
    /// would have produced anyway — and the outcome still carries the notice.
    #[test]
    fn a_trailing_dot_is_removed_with_a_warning() {
        let dir = scratch("trailing-dot");
        let master = master(&dir);
        let out = dir.join("out");
        let outcome = save(&master, &request_with_stem(&out, "shot.")).expect("a trailing dot saves");
        assert_eq!(outcome.path, out.join("shot.png"), "the dot does not survive");
        assert_eq!(
            outcome.warnings,
            vec![super::super::copy::ValidationCopy::StemTrailingDotOrSpace]
        );
    }

    /// The length rule is about the **full path**, so it is checked after the name is joined, and
    /// before the directory is even created.
    #[test]
    fn a_path_past_the_limit_is_refused_before_writing() {
        let dir = scratch("too-long");
        let master = master(&dir);
        let out = dir.join("out");
        let stem = "x".repeat(300);
        let error = save(&master, &request_with_stem(&out, &stem)).expect_err("too long");
        assert!(matches!(
            error,
            SaveError::Validation { copy: super::super::copy::ValidationCopy::PathTooLong, .. }
        ));
        assert!(!out.exists(), "a refused path creates nothing");
        assert!(
            partial_files(&dir).is_empty(),
            "and leaves nothing behind in the parent"
        );
    }
}
