//! `SaveFormat` — what each output format can carry, as pure data (`docs/34` §13).
//!
//! This lives in the shell for the same reason the encoders do (`docs/30` §17.7): a format is a
//! decision about the **output**, not a fact about capturing, so `snapclip-capture` must never learn
//! that formats exist. The scroll session will ask this type whether the format it would save with
//! can hold the image before it starts scrolling — which is why the answer is a value and not a
//! string, and why nothing here allocates or touches the filesystem.
//!
//! The three limits are the formats', not the crates':
//!
//! * PNG carries the height in a `u32` IHDR field, so `u32::MAX` is the real ceiling.
//! * JPEG is specified with 65535 per side; `zenjpeg` refuses past it rather than truncating.
//! * WebP's container caps a side at 16383 (Google's own FAQ), which is why a 30000-row scroll
//!   capture can never be a single WebP file no matter which encoder is used.
//!
//! There is deliberately no pixel-count limit: none of the three formats specifies one, and
//! inventing a fourth limit here would be a product decision wearing a format's clothes. The WebP
//! sink *does* carry a memory guard, and it says so where it is defined.

/// The three formats the save path can produce.
///
/// `Png` is the master and the default; `Jpeg` is the lossy export; `Webp` is lossless only (see
/// [`SaveFormat::label`]) and only within its side limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveFormat {
    Png,
    Jpeg,
    Webp,
}

/// Why a format cannot hold an image. Structured rather than a message: the caller decides how to
/// say it, and the scroll session needs to reason about the pair without parsing English.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatLimit {
    /// The width is past what the format's header can express.
    TooWide { width: u64, max: u64 },
    /// The height is past it.
    TooHigh { height: u64, max: u64 },
}

impl FormatLimit {
    /// The largest side the format allows, whichever one was broken.
    pub fn max(self) -> u64 {
        match self {
            FormatLimit::TooWide { max, .. } | FormatLimit::TooHigh { max, .. } => max,
        }
    }
}

impl std::fmt::Display for FormatLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormatLimit::TooWide { width, max } => {
                write!(f, "宽度 {width} 超过该格式的上限 {max}")
            }
            FormatLimit::TooHigh { height, max } => {
                write!(f, "高度 {height} 超过该格式的上限 {max}")
            }
        }
    }
}

impl std::error::Error for FormatLimit {}

impl SaveFormat {
    /// The largest side the format can hold, in pixels.
    ///
    /// PNG reports `u32::MAX` because that is its header field, not because an image that large is
    /// practical; the memory consequences of one are the caller's business.
    pub fn max_side(self) -> u64 {
        match self {
            SaveFormat::Png => u64::from(u32::MAX),
            SaveFormat::Jpeg => 65_535,
            SaveFormat::Webp => 16_383,
        }
    }

    /// Whether an image of this size can be encoded in this format.
    ///
    /// Checked before any encoder is opened: a scroll capture must learn that its format is too
    /// small *before* it scrolls for thirty seconds, not after.
    pub fn can_encode(self, width: u64, height: u64) -> Result<(), FormatLimit> {
        let max = self.max_side();
        if width > max {
            return Err(FormatLimit::TooWide { width, max });
        }
        if height > max {
            return Err(FormatLimit::TooHigh { height, max });
        }
        Ok(())
    }

    /// The name to put in front of a person.
    ///
    /// `WebP` says **lossless** because that is the only mode this product offers: lossy WebP is
    /// 4:2:0 by the format's own definition, which is the colour subsampling that damages coloured
    /// text and thin UI edges (`docs/34` §9.1, §11).
    pub fn label(self) -> &'static str {
        match self {
            SaveFormat::Png => "PNG",
            SaveFormat::Jpeg => "JPG",
            SaveFormat::Webp => "WebP（无损）",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webp_stops_at_the_container_limit() {
        assert_eq!(SaveFormat::Webp.max_side(), 16_383);
        assert_eq!(SaveFormat::Webp.can_encode(16_383, 16_383), Ok(()));
        assert_eq!(
            SaveFormat::Webp.can_encode(16_384, 10),
            Err(FormatLimit::TooWide {
                width: 16_384,
                max: 16_383
            })
        );
        assert_eq!(
            SaveFormat::Webp.can_encode(10, 16_384),
            Err(FormatLimit::TooHigh {
                height: 16_384,
                max: 16_383
            })
        );
        // The scroll capture case: 30000 rows can never be one WebP.
        assert!(SaveFormat::Webp.can_encode(1280, 30_000).is_err());
    }

    #[test]
    fn jpeg_stops_at_its_specified_side() {
        assert_eq!(SaveFormat::Jpeg.max_side(), 65_535);
        assert_eq!(SaveFormat::Jpeg.can_encode(65_535, 65_535), Ok(()));
        assert_eq!(
            SaveFormat::Jpeg.can_encode(65_536, 1),
            Err(FormatLimit::TooWide {
                width: 65_536,
                max: 65_535
            })
        );
        // The tallest scroll capture the product supports is inside JPEG.
        assert_eq!(SaveFormat::Jpeg.can_encode(3840, 30_000), Ok(()));
    }

    #[test]
    fn png_can_hold_anything_its_header_can() {
        assert_eq!(SaveFormat::Png.max_side(), u64::from(u32::MAX));
        assert_eq!(SaveFormat::Png.can_encode(1, 1), Ok(()));
        assert_eq!(SaveFormat::Png.can_encode(3840, 30_000), Ok(()));
        assert_eq!(SaveFormat::Png.can_encode(u64::from(u32::MAX), 1), Ok(()));
        assert_eq!(
            SaveFormat::Png.can_encode(u64::from(u32::MAX) + 1, 1),
            Err(FormatLimit::TooWide {
                width: u64::from(u32::MAX) + 1,
                max: u64::from(u32::MAX)
            })
        );
    }

    #[test]
    fn the_labels_say_what_the_formats_are() {
        assert_eq!(SaveFormat::Png.label(), "PNG");
        assert_eq!(SaveFormat::Jpeg.label(), "JPG");
        // Lossless is the whole story for WebP here, so the name has to carry it.
        assert!(SaveFormat::Webp.label().contains("无损"));
    }

    #[test]
    fn the_limit_says_the_number_it_was_broken_by() {
        let limit = SaveFormat::Webp.can_encode(20_000, 10).unwrap_err();
        assert_eq!(limit.max(), 16_383);
        let message = limit.to_string();
        assert!(message.contains("20000"), "{message}");
        assert!(message.contains("16383"), "{message}");
    }
}
