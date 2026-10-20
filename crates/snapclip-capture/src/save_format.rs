//! `SaveFormat` — what each output format can carry, as pure data (`docs/34` §13, §15).
//!
//! The *encoders* live in the shell (`docs/30` §17.7) because they are heavy dependencies, but the
//! *capability* has to live here: the scroll session runs in this crate and must refuse to start a
//! capture its format could never hold. So this module is data and rules only — no codec, no
//! allocation, no filesystem — and the shell re-exports it for its sinks.
//!
//! The three limits are the formats', not the crates':
//!
//! * PNG carries the height in a `u32` IHDR field, so `u32::MAX` is the real ceiling.
//! * JPEG is specified with 65535 per side; `zenjpeg` refuses past it rather than truncating.
//! * WebP's container caps a side at 16383 (Google's own FAQ), which is why a 30000-row scroll
//!   capture can never be a single WebP file no matter which encoder is used.
//!
//! There is deliberately no *format* pixel-count limit: none of the three specifies one, and
//! inventing a fourth limit here would be a product decision wearing a format's clothes. The save
//! path does have one budget, and it is named as a budget rather than a limit:
//! [`JPEG_FRAME_BUDGET_PIXELS`]. It lives here because the scroll session has to answer the same
//! question as the sink — can this capture be saved as JPG at all — before it starts.

use serde::{Deserialize, Serialize};

/// The three formats the save path can produce.
///
/// `Png` is the master and the default; `Jpeg` is the lossy export; `Webp` is lossless only (see
/// [`SaveFormat::label`]) and only within its side limit.
///
/// Stored by name (`"png"` / `"jpeg"` / `"webp"`), so a settings file written by an older build
/// keeps loading: a missing field takes the default, which is `Png`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SaveFormat {
    Png,
    Jpeg,
    Webp,
    /// AVIF (`docs/49`). Save-time only: the encoder is whole-frame, so a capture never uses it.
    Avif,
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

/// The most pixels the JPG save path encodes in one frame.
///
/// Not a format limit ([`SaveFormat::max_side`] is): JPEG itself carries 65535 per side. This is the
/// budget of the only correct JPG arm, which holds the whole frame so the Huffman optimiser can run
/// (`docs/34` §10, §17). 16 MP covers 4K with room to spare and refuses a 30000-row scroll capture,
/// which stays PNG (`docs/34` §18).
pub const JPEG_FRAME_BUDGET_PIXELS: u64 = 16_000_000;

/// Whether an image of this size can be saved as JPG at all.
///
/// Both the scroll session and the sink ask this, so a capture the encoder would refuse is lowered
/// before it starts rather than after it is read.
pub fn within_jpeg_frame_budget(width: u64, height: u64) -> bool {
    width.saturating_mul(height) <= JPEG_FRAME_BUDGET_PIXELS
}

/// The most pixels the AVIF save path encodes in one frame.
///
/// AVIF has no streaming encoder in this dependency (`docs/49`): `encode` takes a whole frame, so
/// the save path has to hold it, and the same budget the JPG arm uses is the right shape. A long
/// scroll capture is refused by this before anything is allocated and stays PNG in the artifact,
/// then is transcoded only if it fits.
pub const AVIF_FRAME_BUDGET_PIXELS: u64 = 16_000_000;

/// Whether an image of this size can be saved as AVIF at all.
pub fn within_avif_frame_budget(width: u64, height: u64) -> bool {
    width.saturating_mul(height) <= AVIF_FRAME_BUDGET_PIXELS
}

/// Why a scroll capture will not be saved in the format the user chose.
///
/// Two different answers, kept apart because they mean different things: the format cannot express
/// the rows, or the format could but the save path for it cannot hold the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lowering {
    /// The format's own header cannot express this many rows.
    Format(FormatLimit),
    /// The format could, but the save path would have to hold the whole frame.
    FrameBudget { pixels: u64, max: u64 },
}

impl std::fmt::Display for Lowering {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Lowering::Format(limit) => write!(f, "{limit}"),
            Lowering::FrameBudget { pixels, max } => write!(
                f,
                "the save path holds the whole frame, and {pixels} pixels exceed its {max} budget"
            ),
        }
    }
}

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
            // Conservative: 16384 is the figure this repository could defend without a source, and
            // the real container limit has not been verified (`docs/49`). A test pins the figure so
            // a future correction changes one number and one expectation.
            SaveFormat::Avif => 16_384,
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
            SaveFormat::Avif => "AVIF",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avif_uses_the_conservative_side_limit() {
        assert_eq!(SaveFormat::Avif.max_side(), 16_384);
        assert_eq!(SaveFormat::Avif.can_encode(16_384, 16_384), Ok(()));
        assert!(matches!(
            SaveFormat::Avif.can_encode(16_385, 10),
            Err(FormatLimit::TooWide { .. })
        ));
        assert_eq!(SaveFormat::Avif.label(), "AVIF");
    }

    #[test]
    fn avif_refuses_a_frame_over_its_budget() {
        assert!(within_avif_frame_budget(4000, 4000));
        assert!(!within_avif_frame_budget(1250, 30_000));
        assert!(!within_avif_frame_budget(1250, 12_801));
        assert!(within_avif_frame_budget(1250, 12_800));
    }

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

/// What the save path remembers between captures.
///
/// Split from the overlay's presentation on purpose: a format is a decision about the output, and
/// the overlay never draws it. `jpeg_quality` is stored raw and clamped on read
/// ([`SavePreferences::quality`]) because a hand-edited file must not be able to make the encoder
/// fail — the three values the UI offers (85/92/95) are a UI choice, not a storage rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SavePreferences {
    pub format: SaveFormat,
    /// JPEG quality, 1–100. Only consulted when `format` is `Jpeg`.
    pub jpeg_quality: u8,
}

impl Default for SavePreferences {
    fn default() -> Self {
        Self {
            format: SaveFormat::Png,
            jpeg_quality: DEFAULT_JPEG_QUALITY,
        }
    }
}

impl SavePreferences {
    /// The quality the encoder should use, clamped into the range the format accepts.
    pub fn quality(self) -> u8 {
        self.jpeg_quality.clamp(MIN_JPEG_QUALITY, MAX_JPEG_QUALITY)
    }
}

/// The default JPEG quality: inside the 85–95 band the measurements picked (`docs/34` §11) and
/// above the point where text starts to suffer.
pub const DEFAULT_JPEG_QUALITY: u8 = 92;
pub const MIN_JPEG_QUALITY: u8 = 1;
pub const MAX_JPEG_QUALITY: u8 = 100;

#[cfg(test)]
mod preference_tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_products_current_behaviour() {
        let save = SavePreferences::default();
        assert_eq!(save.format, SaveFormat::Png);
        assert_eq!(save.jpeg_quality, DEFAULT_JPEG_QUALITY);
        assert_eq!(save.quality(), 92);
    }

    #[test]
    fn a_hand_edited_quality_is_clamped_not_rejected() {
        // A settings file is not a place to fail a capture: clamp and carry on.
        let low = SavePreferences { format: SaveFormat::Jpeg, jpeg_quality: 0 };
        assert_eq!(low.quality(), MIN_JPEG_QUALITY);
        let high = SavePreferences { format: SaveFormat::Jpeg, jpeg_quality: 255 };
        assert_eq!(high.quality(), MAX_JPEG_QUALITY);
        let fine = SavePreferences { format: SaveFormat::Jpeg, jpeg_quality: 85 };
        assert_eq!(fine.quality(), 85);
    }

    #[test]
    fn the_formats_round_trip_through_their_stored_names() {
        for (format, text) in [
            (SaveFormat::Png, "\"png\""),
            (SaveFormat::Jpeg, "\"jpeg\""),
            (SaveFormat::Webp, "\"webp\""),
        ] {
            assert_eq!(serde_json::to_string(&format).unwrap(), text);
            assert_eq!(serde_json::from_str::<SaveFormat>(text).unwrap(), format);
        }
    }

    #[test]
    fn an_older_settings_file_without_the_save_block_loads_as_the_default() {
        // The struct-level `#[serde(default)]` is what makes the older file legal.
        let save: SavePreferences = serde_json::from_str("{}").unwrap();
        assert_eq!(save, SavePreferences::default());
        let partial: SavePreferences = serde_json::from_str("{\"format\":\"webp\"}").unwrap();
        assert_eq!(partial.format, SaveFormat::Webp);
        assert_eq!(partial.jpeg_quality, DEFAULT_JPEG_QUALITY);
    }
}
