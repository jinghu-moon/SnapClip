//! The act of writing: validate the destination, encode or copy, commit atomically.
//!
//! This batch covers **PNG only** (`docs/41` S1, first slice). Other formats arrive with the
//! encoder service in S2; the signature does not change when they do.

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
}

/// Why a save did not happen.
#[derive(Debug)]
pub enum SaveError {
    PathEmpty,
    PathNotAbsolute,
    InvalidName(String),
    /// This batch has no encoder for the format yet (`docs/41` S2 adds the rest).
    UnsupportedFormat(SaveFormat),
    NotEnoughSpace(String),
    Io(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::PathEmpty => write!(f, "the target directory is empty"),
            SaveError::PathNotAbsolute => write!(f, "the target directory is not absolute"),
            SaveError::InvalidName(why) => write!(f, "the file name is not usable: {why}"),
            SaveError::UnsupportedFormat(format) => {
                write!(f, "this build cannot save as {format:?} yet")
            }
            SaveError::NotEnoughSpace(why) => write!(f, "not enough space: {why}"),
            SaveError::Io(why) => write!(f, "the file cannot be written: {why}"),
        }
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
    if request.directory.as_os_str().is_empty() {
        return Err(SaveError::PathEmpty);
    }
    if !request.directory.is_absolute() {
        return Err(SaveError::PathNotAbsolute);
    }
    // This slice has no encoder for anything but PNG; S2 adds the rest behind the same signature
    // rather than a second entry point.
    if request.format != SaveFormat::Png {
        return Err(SaveError::UnsupportedFormat(request.format));
    }
    validate_stem(&request.stem)?;

    std::fs::create_dir_all(&request.directory).map_err(io_error)?;

    let target = resolve(&request.directory, &request.stem, "png", request.conflict);
    let partial = partial_path(&target);

    let written = (|| -> std::io::Result<u64> {
        let mut source = std::fs::File::open(master)?;
        let mut sink = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)?;
        std::io::copy(&mut source, &mut sink)?;
        // Flush to the device before the rename: otherwise a crash can commit a directory entry
        // that points at zero-filled blocks.
        sink.sync_all()?;
        drop(sink);
        std::fs::rename(&partial, &target)?;
        Ok(std::fs::metadata(&target)?.len())
    })();

    match written {
        Ok(bytes) => Ok(SaveOutcome {
            path: target,
            bytes,
            format: request.format,
        }),
        Err(error) => {
            // The final file is the only thing a user can see, so a failure must not leave one.
            // The rename is the commit point: before it there is nothing but the `.partial`.
            let _ = std::fs::remove_file(&partial);
            Err(io_error(error))
        }
    }
}

/// Rejects names the rest of the system cannot round-trip.
///
/// Deliberately small for this slice: S1's later batch owns the full list (reserved device names,
/// length, the eleven validation messages). What is here is what a save cannot survive without —
/// an empty name, or one Windows silently trims into a different file.
fn validate_stem(stem: &str) -> Result<(), SaveError> {
    if stem.trim().is_empty() {
        return Err(SaveError::InvalidName("the name is empty".to_owned()));
    }
    if stem.ends_with(' ') || stem.ends_with('.') {
        return Err(SaveError::InvalidName(
            "Windows drops a trailing space or dot, so the file would not be found by that name"
                .to_owned(),
        ));
    }
    if let Some(illegal) = stem.chars().find(|c| {
        matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control()
    }) {
        return Err(SaveError::InvalidName(format!(
            "{illegal:?} cannot appear in a file name"
        )));
    }
    Ok(())
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
}
