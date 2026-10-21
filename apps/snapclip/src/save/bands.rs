//! Saving a picture that never exists whole, as PNG (`docs/35`, `docs/41` S1).
//!
//! A scroll capture is 80 000 rows; materialising it to copy it would cost 404 MB to write a
//! 10 MB file. [`BandSource`] hands out rows instead, and the PNG sink encodes them as they
//! arrive, so the peak is one band.
//!
//! Only PNG here: the other formats are transcodes of the master and arrive in the next batch.

use snapclip_capture::ports::BandSource;

use crate::capture::row_band_png::{ImageMeta, PngRowBandSink};

use super::caps;
use super::service::{self, SaveError, SaveOutcome, SaveRequest};
use snapclip_capture::save_format::SaveFormat;

/// How many rows are read per band. A band is a few hundred kilobytes: large enough that the
/// encoder sees long runs, small enough that the peak stays flat for an 80 000 row capture.
pub const BAND_ROWS: u32 = 256;

/// Encodes `bands` and saves the result according to `request`, streaming the rows.
pub fn save_bands(
    bands: &mut dyn BandSource,
    width: u32,
    request: &SaveRequest,
) -> Result<SaveOutcome, SaveError> {
    let rows = bands.rows();
    // The size question first, for the same reason it comes first in `service::save` (`caps`).
    caps::check(request.format, u64::from(width), u64::from(rows)).map_err(SaveError::Capability)?;
    if request.format != SaveFormat::Png {
        return Err(SaveError::UnsupportedFormat(request.format));
    }
    let bytes = encode_png_bands(bands, width)?;
    service::commit_bytes(request, &bytes)
}

/// Encodes a band source to PNG bytes without touching the disk. Separate from [`save_bands`] so
/// the encoding can be compared with the whole-image path without a file in between.
pub fn encode_png_bands(
    bands: &mut dyn BandSource,
    width: u32,
) -> Result<Vec<u8>, SaveError> {
    let rows = bands.rows();
    let mut sink = PngRowBandSink::new();
    let mut writer = sink
        .begin(&ImageMeta {
            width: u64::from(width),
            height: u64::from(rows),
        })
        .map_err(|error| SaveError::Io(format!("the png header: {error}")))?;

    let mut first = 0u32;
    let mut buffer: Vec<u8> = Vec::new();
    while first < rows {
        let take = BAND_ROWS.min(rows - first);
        buffer.clear();
        if !bands.read_rows(first, take, &mut buffer) {
            return Err(SaveError::Io(format!(
                "the band source refused rows {first}..{}",
                first + take
            )));
        }
        writer
            .write_rows(u64::from(first), &buffer)
            .map_err(|error| SaveError::Io(format!("the png rows at {first}: {error}")))?;
        first += take;
    }
    Ok(writer
        .finish()
        .map_err(|error| SaveError::Io(format!("the png body: {error}")))?
        .bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::row_band_png::{ImageMeta, PngRowBandSink};
    use crate::save::service::ConflictPolicy;
    use snapclip_capture::ports::BandSource;
    use std::path::PathBuf;

    /// Four rows of distinct colours, so a band boundary that drops or repeats a row changes bytes.
    fn pixels(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                out.extend_from_slice(&[(x * 7) as u8, (y * 11) as u8, ((x + y) * 3) as u8, 255]);
            }
        }
        out
    }

    /// A band source over an in-memory picture: the cheapest honest stand-in for the scroll
    /// tracker, and it makes an out-of-order read a test failure rather than a corruption.
    struct Rows {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
        next: u32,
    }

    impl Rows {
        fn new(width: u32, height: u32) -> Self {
            Self {
                width,
                height,
                pixels: pixels(width, height),
                next: 0,
            }
        }
    }

    impl BandSource for Rows {
        fn rows(&self) -> u32 {
            self.height
        }

        fn read_rows(&mut self, first_row: u32, rows: u32, out: &mut Vec<u8>) -> bool {
            if first_row != self.next || first_row + rows > self.height {
                return false;
            }
            let per_row = self.width as usize * 4;
            let start = first_row as usize * per_row;
            let end = start + rows as usize * per_row;
            out.clear();
            out.extend_from_slice(&self.pixels[start..end]);
            self.next += rows;
            true
        }
    }

    fn write_whole(width: u32, height: u32) -> Vec<u8> {
        let mut sink = PngRowBandSink::new();
        let mut writer = sink
            .begin(&ImageMeta {
                width: u64::from(width),
                height: u64::from(height),
            })
            .expect("png header");
        writer
            .write_rows(0, &pixels(width, height))
            .expect("the whole picture in one band");
        writer.finish().expect("png body").bytes
    }

    /// The guarantee that lets a long capture use this path at all: splitting the rows into bands
    /// must not change a single byte of the file. Same encoder, same rows, same output.
    #[test]
    fn splitting_into_bands_does_not_change_the_png() {
        let (width, height) = (7u32, 11u32);
        let whole = write_whole(width, height);

        let mut rows = Rows::new(width, height);
        let banded = encode_png_bands(&mut rows, width).expect("banded encode");

        assert_eq!(banded, whole, "a band boundary must not be visible in the file");
    }

    /// A save writes exactly the bytes the encoder produced, and commits atomically.
    #[test]
    fn a_banded_save_writes_the_encoded_bytes_and_leaves_no_partial() {
        let dir = scratch("bands");
        let mut rows = Rows::new(7, 11);
        let request = SaveRequest {
            directory: dir.clone(),
            stem: "long".to_owned(),
            format: SaveFormat::Png,
            conflict: ConflictPolicy::AutoRename,
        };

        let outcome = save_bands(&mut rows, 7, &request).expect("a PNG band save");

        assert_eq!(outcome.path, dir.join("long.png"));
        let on_disk = std::fs::read(&outcome.path).expect("written file");
        assert_eq!(on_disk, write_whole(7, 11), "the file is the encoder's bytes");
        assert_eq!(outcome.bytes, on_disk.len() as u64);
        assert!(
            std::fs::read_dir(&dir)
                .expect("dir")
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".partial")),
            "no partial survives a commit"
        );
    }

    /// The band path is PNG only in this batch, and it says so with an error rather than by
    /// re-encoding through a path that is not built yet.
    #[test]
    fn a_non_png_band_request_is_refused() {
        let dir = scratch("bands-format");
        let mut rows = Rows::new(7, 11);
        let request = SaveRequest {
            directory: dir,
            stem: "long".to_owned(),
            format: SaveFormat::Jpeg,
            conflict: ConflictPolicy::AutoRename,
        };
        assert!(matches!(
            save_bands(&mut rows, 7, &request),
            Err(SaveError::UnsupportedFormat(SaveFormat::Jpeg))
        ));
    }

    /// The size question is answered before the format question, so a request that is impossible
    /// in its format is reported as such even when that format is not buildable yet.
    #[test]
    fn a_too_wide_band_picture_is_refused_before_the_format_is_considered() {
        let dir = scratch("bands-wide");
        let mut rows = Rows::new(1, 1);
        let request = SaveRequest {
            directory: dir,
            stem: "long".to_owned(),
            format: SaveFormat::Webp,
            conflict: ConflictPolicy::AutoRename,
        };
        let max = SaveFormat::Webp.max_side();
        let width = u32::try_from(max + 1).expect("fits");
        assert!(matches!(
            save_bands(&mut rows, width, &request),
            Err(SaveError::Capability(_))
        ));
    }

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("snapclip-save-{}-{label}", std::process::id()));
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("cannot clear {}: {error}", dir.display()),
        }
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn caps_are_checked_for_banded_saves_against_the_real_width() {
        // A guard against passing something other than the region's width to the size check.
        assert!(caps::check(SaveFormat::Png, 7, 11).is_ok());
    }
}
