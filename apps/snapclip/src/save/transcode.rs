//! Turning the PNG master into the format the panel chose (`docs/34` §16, `docs/41` S4).
//!
//! The banded encoder writes one lossless master; every other format is a transcode of it, so
//! quality is spent at most once and never on a file that has already lost it. That is also why
//! this module exists at all rather than teaching the encoder about four formats: one master, one
//! transcoder, and the same atomic commit for both.
//!
//! The transcoder in `capture::convert` reads its master from a path, so the bytes are parked in a
//! temporary file for the length of the call and removed before returning, whatever the outcome.
//! A failed transcode leaves no master behind to be mistaken for a result.

use snapclip_capture::save_format::SaveFormat;

use super::service::SaveError;

/// `png` transcoded to `format` at `quality`, in memory.
///
/// `quality` only reaches JPEG: PNG is a byte copy and this project's WebP is lossless, which is
/// why the panel has no tier for either of them (`docs/42` D1, spec `:94`).
pub fn to_bytes(png: &[u8], format: SaveFormat, quality: u8) -> Result<Vec<u8>, SaveError> {
    let master = master_path();
    std::fs::write(&master, png)
        .map_err(|error| SaveError::Io(format!("the master cannot be parked: {error}")))?;
    let mut bytes = Vec::new();
    let outcome = crate::capture::convert::convert(&master, format, quality, &mut bytes)
        .map_err(|error| SaveError::Io(format!("the transcode to {format:?}: {error}")));
    // Removed on both paths: a leftover master is a mystery file in the user's temporary folder.
    let _ = std::fs::remove_file(&master);
    outcome.map(|_| bytes)
}

/// A name nothing else in this process is likely to pick: the process id, the clock, and the
/// format's extension so a victim of a crash can be recognised by hand.
fn master_path() -> std::path::PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "snapclip-master-{}-{stamp}.png",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-pixel PNG, encoded by the same encoder the save path uses.
    fn master() -> Vec<u8> {
        use crate::capture::row_band_png::{ImageMeta, PngRowBandSink};

        let mut sink = PngRowBandSink::new();
        let mut writer = sink
            .begin(&ImageMeta {
                width: 2,
                height: 1,
            })
            .expect("a png header");
        writer
            .write_rows(0, &[0, 0, 0, 255, 255, 0, 0, 255])
            .expect("two pixels");
        writer.finish().expect("the png body").bytes
    }

    /// A JPEG is a JPEG: the file starts with the marker rather than with the master's signature.
    #[test]
    fn a_transcode_is_the_format_it_was_asked_for() {
        let jpeg = to_bytes(&master(), SaveFormat::Jpeg, 92).expect("a jpeg");
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "a JPEG begins with SOI");
        assert_ne!(&jpeg[..4], b"\x89PNG", "and not with the master's signature");
    }

    /// The quality setting reaches the encoder instead of being accepted and ignored: two tiers
    /// produce different files for the same picture.
    #[test]
    fn the_quality_reaches_the_encoder() {
        let high = to_bytes(&master(), SaveFormat::Jpeg, 92).expect("a high quality jpeg");
        let low = to_bytes(&master(), SaveFormat::Jpeg, 50).expect("a low quality jpeg");
        assert_ne!(
            high, low,
            "two qualities must not produce the same bytes; if they do, the number is being dropped"
        );
    }

    /// WebP in this project is lossless, so the tier cannot change its bytes. Recorded here as an
    /// assertion rather than a comment, because "quality has no effect" is a claim worth holding.
    #[test]
    fn the_quality_is_ignored_by_lossless_webp() {
        let high = to_bytes(&master(), SaveFormat::Webp, 92).expect("a webp");
        let low = to_bytes(&master(), SaveFormat::Webp, 50).expect("a webp");
        assert_eq!(high, low, "lossless WebP has no quality to spend");
    }

    /// No master is left behind, whether the transcode worked or not.
    #[test]
    fn no_master_is_left_in_the_temporary_folder() {
        let before = count_masters();
        let _ = to_bytes(&master(), SaveFormat::Jpeg, 92);
        assert_eq!(count_masters(), before, "the parked master must be removed");
    }

    fn count_masters() -> usize {
        std::fs::read_dir(std::env::temp_dir())
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with("snapclip-master-")
                    })
                    .count()
            })
            .unwrap_or(0)
    }
}
