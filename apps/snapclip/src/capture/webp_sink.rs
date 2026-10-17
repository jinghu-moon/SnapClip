//! `WebpSink` — lossless WebP export (`docs/34` §9.1, §13).
//!
//! # Why lossless, and why not for scroll captures
//!
//! Lossy WebP is Y'CbCr 4:2:0 by the format's own definition; that is the same colour subsampling
//! `docs/34` §11 measured as costing 2.25 dB of text PSNR in JPEG, so it is the one thing this
//! product will not do to a screenshot. Lossless WebP is a real win — §10 measured about a quarter
//! of PNG's bytes on UI material — so the sink offers that, and only that.
//!
//! The container caps a side at 16383, which is why a scroll capture never reaches this sink: the
//! capture stops at 30000 rows, and `SaveFormat::Webp::can_encode` says so before any encoder opens.
//!
//! # Why there is no row-band form
//!
//! `zenwebp 0.4.4` has no streaming encoder: `EncodeRequest` takes the whole image. The sink
//! therefore accumulates rows and encodes once at `finish`, and it says so in its type name rather
//! than pretending to stream. [`MAX_PIXELS`] keeps that accumulation from becoming unbounded.

use zenwebp::{EncodeRequest, LosslessConfig, PixelLayout};

use super::row_band_png::{Artifact, ExportError, ImageMeta};
use super::save_format::SaveFormat;

/// Lossless pre-processing is off at 100; below it, libwebp is allowed to *improve* pixels.
pub const NEAR_LOSSLESS_OFF: u8 = 100;

/// Quality/effort for the lossless compressor, matching `docs/34` §10's measurement run.
pub const WEBP_METHOD: u8 = 4;

/// Lossless WebP costs roughly 30 bytes per pixel *inside* the encoder on top of the rows
/// (`docs/34` §10: a 4K frame peaks 257 MB), so this guard is about memory, not the format.
///
/// 16 MP is the largest a screenshot of a 5K display needs; past it the encode would reserve half a
/// gigabyte, which is not something a save button should do silently.
pub const MAX_PIXELS: u64 = 16_000_000;

/// Rows in, one lossless WebP out.
pub struct WebpSink {
    method: u8,
}

impl Default for WebpSink {
    fn default() -> Self {
        Self {
            method: WEBP_METHOD,
        }
    }
}

impl WebpSink {
    /// The sink `docs/34` §9.1 selected: lossless, exact, near-lossless off.
    pub fn new() -> Self {
        Self::default()
    }

    /// The compressor effort handed to the encoder.
    pub fn method(&self) -> u8 {
        self.method
    }

    /// Opens an encoder for `meta`. Both the format's side limit and the memory guard are checked
    /// here, before a single row is held.
    pub fn begin(&mut self, meta: &ImageMeta) -> Result<WebpWriter, ExportError> {
        SaveFormat::Webp
            .can_encode(meta.width, meta.height)
            .map_err(|_| ExportError::TooLarge {
                width: meta.width,
                height: meta.height,
            })?;
        if meta.width == 0 || meta.height == 0 {
            return Err(ExportError::Sink("a 0-sided image has no WebP encoding".into()));
        }
        let pixels = meta.width.saturating_mul(meta.height);
        if pixels > MAX_PIXELS {
            return Err(ExportError::TooLarge {
                width: meta.width,
                height: meta.height,
            });
        }
        Ok(WebpWriter {
            meta: *meta,
            method: self.method,
            rows: Vec::with_capacity(usize::try_from(pixels.saturating_mul(4)).unwrap_or(0)),
            next_row: 0,
        })
    }
}

/// Rows in, in order; `finish` encodes once, because `zenwebp` has no streaming form.
#[derive(Debug)]
pub struct WebpWriter {
    meta: ImageMeta,
    method: u8,
    rows: Vec<u8>,
    next_row: u64,
}

impl WebpWriter {
    pub fn write_rows(&mut self, first_row: u64, rows: &[u8]) -> Result<(), ExportError> {
        let row_bytes = self.meta.width * 4;
        let got = rows.len() as u64;
        if row_bytes == 0 || got % row_bytes != 0 {
            return Err(ExportError::RowLength {
                expected: row_bytes,
                got,
            });
        }
        if first_row != self.next_row {
            return Err(ExportError::OutOfOrder {
                first_row,
                expected: self.next_row,
            });
        }
        let count = got / row_bytes;
        if first_row + count > self.meta.height {
            return Err(ExportError::BeyondHeight {
                first_row,
                height: self.meta.height,
            });
        }
        self.rows.extend_from_slice(rows);
        self.next_row += count;
        Ok(())
    }

    pub fn finish(self) -> Result<Artifact, ExportError> {
        if self.next_row != self.meta.height {
            return Err(ExportError::Sink(format!(
                "{} of {} rows were written, and WebP is encoded from the whole image",
                self.next_row, self.meta.height
            )));
        }

        // `near_lossless(NEAR_LOSSLESS_OFF)` off and `exact(true)` on are what make "lossless"
        // mean byte-identical: without `exact`, libwebp is free to rewrite the RGB values under
        // fully transparent pixels, which changes the file's pixels even though nothing visible did.
        let config = LosslessConfig::new()
            .with_method(self.method)
            .with_near_lossless(NEAR_LOSSLESS_OFF)
            .with_exact(true)
            .with_max_dimensions(16_383, 16_383);

        let mut bytes = Vec::new();
        // BGRA straight in: `PixelLayout::Bgra8` is why the sink needs no conversion pass.
        let request = EncodeRequest::lossless(
            &config,
            &self.rows,
            PixelLayout::Bgra8,
            u32::try_from(self.meta.width).map_err(|_| ExportError::TooLarge {
                width: self.meta.width,
                height: self.meta.height,
            })?,
            u32::try_from(self.meta.height).map_err(|_| ExportError::TooLarge {
                width: self.meta.width,
                height: self.meta.height,
            })?,
        );
        request.encode_to(&mut bytes).map_err(sink_error)?;

        Ok(Artifact {
            bytes,
            rows: self.next_row,
        })
    }
}

fn sink_error(error: impl std::fmt::Display) -> ExportError {
    ExportError::Sink(error.to_string())
}

/// Encodes one whole BGRA image.
pub fn encode_bgra(bgra: &[u8], meta: &ImageMeta) -> Result<Artifact, ExportError> {
    let mut sink = WebpSink::new();
    let mut writer = sink.begin(meta)?;
    writer.write_rows(0, bgra)?;
    writer.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(width: u64, height: u64) -> ImageMeta {
        ImageMeta { width, height }
    }

    /// Rows where every fourth pixel is fully transparent but keeps a loud colour underneath.
    /// That combination is what `exact(true)` exists for: without it libwebp may rewrite the RGB
    /// under alpha 0, and the file stops being a byte-for-byte copy.
    fn alpha_frame(width: u64, height: u64) -> Vec<u8> {
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let clear = (x + y) % 4 == 0;
                out.extend_from_slice(&[
                    250,          // B, deliberately loud under the transparent pixels
                    30 + (y % 7) as u8, // G
                    200,
                    if clear { 0 } else { 255 },
                ]);
            }
        }
        out
    }

    /// The source rows are BGRA; `image` decodes to RGBA. Same pixels, two orders.
    fn rgba_of(bgra: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bgra.len());
        for pixel in bgra.chunks_exact(4) {
            out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
        out
    }

    /// Decodes with the `image` crate rather than `zenwebp`: a different implementation, so an
    /// encoder/decoder pair that shares a bug cannot cancel itself out.
    fn decode(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
        let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::WebP)
            .expect("image decodes our webp")
            .to_rgba8();
        let (width, height) = decoded.dimensions();
        (width, height, decoded.into_raw())
    }

    #[test]
    fn a_lossless_webp_is_the_same_pixels() {
        let frame = alpha_frame(64, 48);
        let artifact = encode_bgra(&frame, &meta(64, 48)).expect("encode");
        assert!(!artifact.bytes.is_empty());
        // RIFF container.
        assert_eq!(&artifact.bytes[0..4], b"RIFF");
        assert_eq!(&artifact.bytes[8..12], b"WEBP");

        let (width, height, pixels) = decode(&artifact.bytes);
        assert_eq!((width, height), (64, 48));
        assert_eq!(pixels.len(), 64 * 48 * 4);
        // The source rows are BGRA (the layout handed to the encoder) and the decoder answers in
        // RGBA, so the comparison swaps the two colour channels rather than the encoder.
        assert_eq!(pixels, rgba_of(&frame), "lossless means the same pixels");
    }

    #[test]
    fn the_formats_side_limit_is_refused_before_allocating() {
        let mut sink = WebpSink::new();
        let error = sink.begin(&meta(16_384, 10)).unwrap_err();
        assert!(matches!(error, ExportError::TooLarge { .. }), "{error}");
        assert!(SaveFormat::Webp.can_encode(16_384, 10).is_err());
    }

    #[test]
    fn a_scroll_capture_never_reaches_this_sink() {
        // The product's tallest capture: 30000 rows is inside JPEG and PNG, and outside WebP.
        assert!(SaveFormat::Webp.can_encode(1280, 30_000).is_err());
        assert_eq!(SaveFormat::Jpeg.can_encode(1280, 30_000), Ok(()));
        assert_eq!(SaveFormat::Png.can_encode(1280, 30_000), Ok(()));
        let mut sink = WebpSink::new();
        assert!(matches!(
            sink.begin(&meta(1280, 30_000)),
            Err(ExportError::TooLarge { .. })
        ));
    }

    #[test]
    fn the_memory_guard_refuses_what_would_reserve_half_a_gigabyte() {
        // 4000x4000 = 16 MP is the ceiling; one row more is refused.
        let mut sink = WebpSink::new();
        assert!(sink.begin(&meta(4_000, 4_000)).is_ok());
        assert!(matches!(
            sink.begin(&meta(4_000, 4_001)),
            Err(ExportError::TooLarge { .. })
        ));
    }

    #[test]
    fn rows_must_arrive_in_order_and_in_whole_rows() {
        let mut sink = WebpSink::new();
        let mut writer = sink.begin(&meta(4, 4)).expect("begin");
        assert!(matches!(
            writer.write_rows(1, &[0u8; 16]),
            Err(ExportError::OutOfOrder { .. })
        ));
        assert!(matches!(
            writer.write_rows(0, &[0u8; 15]),
            Err(ExportError::RowLength { .. })
        ));
        assert!(matches!(
            writer.write_rows(0, &[0u8; 16 * 5]),
            Err(ExportError::BeyondHeight { .. })
        ));
    }

    #[test]
    fn a_short_write_cannot_become_a_webp() {
        let mut sink = WebpSink::new();
        let mut writer = sink.begin(&meta(4, 4)).expect("begin");
        writer.write_rows(0, &[0u8; 16]).expect("one row");
        let error = writer.finish().unwrap_err();
        assert!(error.to_string().contains("whole image"), "{error}");
    }
}
