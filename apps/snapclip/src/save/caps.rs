//! Whether a format can hold an image of this size, asked **before** any byte is written.
//!
//! The side limits live in `snapclip_capture::save_format::SaveFormat::can_encode` and are not
//! repeated here. What this module adds is the two rules that are not about a single side: the JPG
//! path holds a whole frame to run its optimiser, and a BMP file is four bytes per pixel plus a
//! header, against a size field that is only 32 bits wide.
//!
//! Ordering matters and is deliberate: the size question is answered first, because it does not
//! depend on whether an encoder exists. A request that cannot be satisfied in its format is an
//! error about the **request**; "this build has no encoder yet" is an error about the **build**.

use snapclip_capture::save_format::{FormatLimit, SaveFormat, JPEG_FRAME_BUDGET_PIXELS};

/// The largest BMP file we will write.
///
/// `bfSize` is a `u32`, so four gibibytes is the format's own ceiling. Two gibibytes is the guard:
/// a screenshot that big is already a mistake, and refusing early is cheaper than filling a disk
/// to find out.
pub const BMP_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The bytes a BMP of this size occupies, header included (`BITMAPFILEHEADER` + `BITMAPV5HEADER`).
pub fn bmp_file_bytes(width: u64, height: u64) -> u64 {
    14 + 124 + width.saturating_mul(height).saturating_mul(4)
}

/// Why a format cannot hold this image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityError {
    /// One side past what the format can describe.
    TooWide { width: u64, max: u64 },
    TooHigh { height: u64, max: u64 },
    /// Too many pixels for the path that must hold a whole frame.
    FrameBudget {
        format: SaveFormat,
        pixels: u64,
        max: u64,
    },
    /// The file itself would be too large.
    FileSize { bytes: u64, max: u64 },
}

/// Checks the format's own limits and the budgets the save path imposes on top of them.
pub fn check(format: SaveFormat, width: u64, height: u64) -> Result<(), CapabilityError> {
    // The format's own header limits first: they come from the file format, not from us.
    match format.can_encode(width, height) {
        Ok(()) => {}
        Err(FormatLimit::TooWide { width, max }) => {
            return Err(CapabilityError::TooWide { width, max })
        }
        Err(FormatLimit::TooHigh { height, max }) => {
            return Err(CapabilityError::TooHigh { height, max })
        }
    }

    match format {
        SaveFormat::Jpeg => {
            let pixels = width.saturating_mul(height);
            if pixels > JPEG_FRAME_BUDGET_PIXELS {
                return Err(CapabilityError::FrameBudget {
                    format,
                    pixels,
                    max: JPEG_FRAME_BUDGET_PIXELS,
                });
            }
        }
        SaveFormat::Bmp => {
            let bytes = bmp_file_bytes(width, height);
            if bytes > BMP_MAX_FILE_BYTES {
                return Err(CapabilityError::FileSize {
                    bytes,
                    max: BMP_MAX_FILE_BYTES,
                });
            }
        }
        // PNG and WebP are streamed by their encoders, so the side limit is the whole story.
        SaveFormat::Png | SaveFormat::Webp => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PNG's header is `u32` per side, and nothing else stands in its way.
    #[test]
    fn png_accepts_a_width_only_a_u32_could_hold() {
        assert!(check(SaveFormat::Png, 100_000, 2).is_ok());
        assert!(matches!(
            check(SaveFormat::Png, u64::from(u32::MAX) + 1, 2),
            Err(CapabilityError::TooWide { .. })
        ));
    }

    /// WebP's own limit is the authority here: the limit passes, one past it does not.
    #[test]
    fn webp_refuses_one_pixel_past_its_side_limit() {
        let max = SaveFormat::Webp.max_side();
        assert!(check(SaveFormat::Webp, max, 10).is_ok(), "the limit itself is allowed");
        assert_eq!(
            check(SaveFormat::Webp, max + 1, 10),
            Err(CapabilityError::TooWide { width: max + 1, max })
        );
    }

    /// The JPG path holds the frame, so its budget binds before any side limit does.
    #[test]
    fn jpeg_refuses_one_pixel_past_its_frame_budget() {
        assert!(check(SaveFormat::Jpeg, 4_000, 4_000).is_ok(), "exactly 16 MP");
        assert_eq!(
            check(SaveFormat::Jpeg, 4_000, 4_001),
            Err(CapabilityError::FrameBudget {
                format: SaveFormat::Jpeg,
                pixels: 16_004_000,
                max: JPEG_FRAME_BUDGET_PIXELS
            })
        );
    }

    /// A BMP is uncompressed, so the file size is the real limit; the boundary is inclusive.
    #[test]
    fn bmp_refuses_a_file_over_two_gibibytes() {
        // 1024 × 524 287 × 4 + 138 = 2 147 479 690, under the guard.
        let height = 524_287;
        assert_eq!(bmp_file_bytes(1024, height), 2_147_479_690);
        assert!(check(SaveFormat::Bmp, 1024, height).is_ok(), "at the boundary");
        assert!(matches!(
            check(SaveFormat::Bmp, 1024, height + 1),
            Err(CapabilityError::FileSize { max, .. }) if max == BMP_MAX_FILE_BYTES
        ));
    }

    /// A 4K frame is well inside every limit, so the guard never gets in the way of a real save.
    #[test]
    fn a_four_k_frame_passes_every_format() {
        for format in [
            SaveFormat::Png,
            SaveFormat::Jpeg,
            SaveFormat::Webp,
            SaveFormat::Bmp,
        ] {
            assert!(check(format, 3_840, 2_160).is_ok(), "{format:?} can hold a 4K frame");
        }
    }
}
