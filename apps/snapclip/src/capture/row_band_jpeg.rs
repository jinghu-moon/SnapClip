//! `JpgRowBandSink` — lossy export, one strategy only (`docs/34` §10–11, §17–18).
//!
//! # Why there is one arm and not two
//!
//! `docs/34` §10 measured that `zenjpeg`'s `BytesEncoder::push` streams truly while the Huffman
//! optimiser is off (1280x30000 in 393 ms, 13.9 MB peak) and buffers the whole frame once it is on
//! (2.95 s, 456.5 MB). §11 then measured the optimiser as free in pixels on UI material, so this
//! sink used to stream tall frames with the optimiser off to keep the peak low.
//!
//! §17 showed that arm is **wrong**: relative to the source, the optimiser-off encode lands at a
//! mean error of 81.8 on high-contrast content, against 10.4 with the optimiser on, while the
//! segments differ only in `DHT`. Two independent decoders reproduce it, and it follows the content
//! rather than the switch. So Huffman optimisation here is a **correctness requirement**, not a size
//! trick, and a streaming arm without it cannot be offered at any memory saving.
//!
//! That leaves one arm: buffer the frame, encode once with the optimiser on. The buffer is bounded
//! by [`JPEG_FRAME_BUDGET_PIXELS`], and an image past it is refused rather than encoded wrongly — a
//! 30000-row scroll capture stays PNG (`docs/34` §18).
//!
//! # What is fixed, and not configurable
//!
//! Baseline, 4:4:4, quality 92, and BGRA straight in. §11 is why:
//!
//! * 4:2:0 costs 2.25 dB of text PSNR while whole-image SSIM moves by 0.0003 — invisible unless the
//!   text is measured, and 4:4:4 is the only arm that keeps it.
//! * q95 buys 0.0018 of text SSIM for 22 % more bytes; q85 gives back 27 % of the bytes for 0.0068.
//! * `PixelLayout::Bgra8Srgb` means the rows the capture side hands out go in untouched, which is
//!   the whole point of a row-band sink.
//!
//! Trellis is deliberately absent: its only external entry point is `OptimizationPreset::MozjpegBaseline`,
//! which also swaps the quantisation tables, and §11 measured that preset as *worse* on text
//! (0.9729 versus 0.9968) and 13 % larger. The crate keeps `TrellisConfig` private, so a controlled
//! trellis arm cannot be built from outside; that is why this sink does not claim one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use zenjpeg::encoder::{ChromaSubsampling, EncoderConfig, PixelLayout, Stop as JpegStop};

use super::row_band_png::{Artifact, ExportError, ImageMeta};
use super::save_format::{within_jpeg_frame_budget, SaveFormat, JPEG_FRAME_BUDGET_PIXELS};

/// The quality `docs/34` §11 selected: q95 and q85 are both worse trades for UI material.
pub const JPEG_QUALITY: u8 = 92;

/// Rows in, a complete JPEG out. BGRA goes in as it arrives; nothing is converted first.
pub struct JpgRowBandSink {
    quality: u8,
}

impl Default for JpgRowBandSink {
    fn default() -> Self {
        Self {
            quality: JPEG_QUALITY,
        }
    }
}

impl JpgRowBandSink {
    /// The sink `docs/34` §11 selected: baseline, 4:4:4, q92.
    pub fn new() -> Self {
        Self::default()
    }

    /// The same sink at another quality, for the save path's own setting (`docs/34` §16).
    ///
    /// Clamped rather than refused: a hand-edited settings file must not fail an encode.
    pub fn with_quality(quality: u8) -> Self {
        Self {
            quality: quality.clamp(1, 100),
        }
    }

    /// The quality handed to the encoder, 1..=100.
    pub fn quality(&self) -> u8 {
        self.quality
    }

    /// Opens an encoder for `meta`. Every limit is checked here, before anything allocates.
    ///
    /// The frame budget is checked before the format's side limit is reported, because "this is too
    /// big for JPG" is more useful than "this is past 65535" for a scroll capture that is only
    /// 30000 rows tall: the first names the real reason, the second names a limit it never reached.
    pub fn begin(&mut self, meta: &ImageMeta) -> Result<JpgRowBandWriter, ExportError> {
        if meta.width == 0 || meta.height == 0 {
            return Err(ExportError::Sink("a 0-sided image has no JPEG encoding".into()));
        }
        SaveFormat::Jpeg
            .can_encode(meta.width, meta.height)
            .map_err(|_| ExportError::TooLarge {
                width: meta.width,
                height: meta.height,
            })?;
        if !within_jpeg_frame_budget(meta.width, meta.height) {
            return Err(ExportError::JpegFrameBudget {
                width: meta.width,
                height: meta.height,
                max: JPEG_FRAME_BUDGET_PIXELS,
            });
        }

        let encoder = config(self.quality)
            .encode_from_bytes(
                u32::try_from(meta.width).map_err(|_| ExportError::TooLarge {
                    width: meta.width,
                    height: meta.height,
                })?,
                u32::try_from(meta.height).map_err(|_| ExportError::TooLarge {
                    width: meta.width,
                    height: meta.height,
                })?,
                PixelLayout::Bgra8Srgb,
            )
            .map_err(sink_error)?;

        let cancelled = Arc::new(AtomicBool::new(false));
        Ok(JpgRowBandWriter {
            meta: *meta,
            encoder: Some(encoder),
            cancelled,
            buffered: Vec::with_capacity(
                usize::try_from(meta.width * meta.height * 4).unwrap_or(0),
            ),
            next_row: 0,
        })
    }
}

/// The one place the encoder configuration is built, so every path agrees on 4:4:4 and baseline.
///
/// The optimiser is on unconditionally: §17 measured the alternative as wrong on high-contrast
/// content, so it is not a knob.
fn config(quality: u8) -> EncoderConfig {
    EncoderConfig::ycbcr(f32::from(quality), ChromaSubsampling::None)
        .force_baseline()
        .optimize_huffman(true)
}

fn sink_error(error: impl std::fmt::Display) -> ExportError {
    ExportError::Sink(error.to_string())
}

/// The cancellation token `zenjpeg`'s `Stop` expects, backed by the shared flag `cancel` sets.
struct Cancel(Arc<AtomicBool>);

impl JpegStop for Cancel {
    fn check(&self) -> Result<(), enough::StopReason> {
        if self.0.load(Ordering::Relaxed) {
            Err(enough::StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Rows in, in order; `finish` encodes the frame and closes the JPEG.
pub struct JpgRowBandWriter {
    meta: ImageMeta,
    /// `None` only after `finish`, which is the only thing that can take it.
    encoder: Option<zenjpeg::encoder::BytesEncoder>,
    cancelled: Arc<AtomicBool>,
    /// Every row, in order: the optimiser needs the whole frame (`docs/34` §10), and it is on
    /// because the alternative is wrong (§17).
    buffered: Vec<u8>,
    next_row: u64,
}

impl JpgRowBandWriter {
    /// Stops the encoder at its next cancellation point. `finish` then fails instead of producing a
    /// file.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

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

        // Kept, not pushed: the optimiser has to see the whole frame before it writes anything
        // (`docs/34` §10), and it is the only correct arm (§17).
        self.buffered.extend_from_slice(rows);
        self.next_row += count;
        Ok(())
    }

    pub fn finish(mut self) -> Result<Artifact, ExportError> {
        if self.next_row != self.meta.height {
            return Err(ExportError::Sink(format!(
                "{} of {} rows were written, and a short JPEG is a corrupt one",
                self.next_row, self.meta.height
            )));
        }

        let mut encoder = self
            .encoder
            .take()
            .ok_or_else(|| ExportError::Sink("the artifact was already closed".into()))?;

        encoder
            .push(
                &self.buffered,
                usize::try_from(self.meta.height).unwrap_or(0),
                usize::try_from(self.meta.width * 4).unwrap_or(0),
                Cancel(Arc::clone(&self.cancelled)),
            )
            .map_err(sink_error)?;

        let bytes = encoder.finish().map_err(sink_error)?;
        Ok(Artifact {
            bytes,
            rows: self.next_row,
        })
    }
}

/// Encodes one whole BGRA image. The convenience path for callers that already hold the rows.
pub fn encode_bgra(bgra: &[u8], meta: &ImageMeta) -> Result<Artifact, ExportError> {
    let mut sink = JpgRowBandSink::new();
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

    /// A frame with text-like detail: sharp edges survive 4:4:4 and smear under 4:2:0.
    fn ui_frame(width: u64, height: u64) -> Vec<u8> {
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let ink = (x / 3 + y / 3) % 2 == 0;
                let (b, g, r) = if ink { (40, 40, 220) } else { (250, 250, 250) };
                out.extend_from_slice(&[b, g, r, 255]);
            }
        }
        out
    }

    /// Decodes with the `image` crate rather than `zenjpeg`: the encoder and the decoder then come
    /// from different implementations, so a bug they share cannot cancel itself out.
    fn decode(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
        let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
            .expect("image decodes our jpeg")
            .to_rgba8();
        let (width, height) = decoded.dimensions();
        (width, height, decoded.into_raw())
    }

    #[test]
    fn a_small_image_round_trips_and_is_baseline() {
        let frame = ui_frame(160, 120);
        let artifact = encode_bgra(&frame, &meta(160, 120)).expect("encode");
        assert!(!artifact.bytes.is_empty());
        let (width, height, pixels) = decode(&artifact.bytes);
        assert_eq!((width, height), (160, 120));
        assert_eq!(pixels.len(), 160 * 120 * 4);
        // Baseline SOF0, not progressive SOF2 (docs/34 §9: streaming is baseline only anyway).
        assert!(
            !artifact
                .bytes
                .windows(2)
                .any(|pair| pair == [0xFF, 0xC2]),
            "progressive markers must not appear"
        );
    }

    #[test]
    fn the_sink_encodes_with_the_optimiser_on() {
        // §17: the optimiser-off arm is wrong on high-contrast content, so the sink must not have a
        // switch. `EncoderConfig` keeps its fields private, so this compares bytes instead: the
        // sink's output must be the output of an encoder built with the optimiser explicitly on.
        let meta = meta(1920, 1080);
        let frame = ui_frame(1920, 1080);
        let from_sink = encode_bgra(&frame, &meta).expect("encode").bytes;

        let mut explicit = EncoderConfig::ycbcr(f32::from(JPEG_QUALITY), ChromaSubsampling::None)
            .force_baseline()
            .optimize_huffman(true)
            .encode_from_bytes(1920, 1080, PixelLayout::Bgra8Srgb)
            .expect("start");
        explicit
            .push(&frame, 1080, 1920 * 4, enough::Unstoppable)
            .expect("push");
        let explicit = explicit.finish().expect("finish");

        assert_eq!(
            from_sink, explicit,
            "the sink must build the optimised arm, byte for byte"
        );
    }

    #[test]
    fn a_4k_frame_fits_the_budget_and_a_30k_row_capture_does_not() {
        let mut sink = JpgRowBandSink::new();
        // 3840 x 2160 = 8.3 MP: a 4K screenshot is well inside the 16 MP frame budget.
        assert!(sink.begin(&meta(3840, 2160)).is_ok());
        // 1280 x 30000 = 38.4 MP: the scroll capture case is refused, not encoded wrongly.
        assert!(matches!(
            sink.begin(&meta(1280, 30_000)),
            Err(ExportError::JpegFrameBudget { .. })
        ));
        // The budget is a pixel count, so a frame just inside it is accepted and just past it is not.
        assert!(within_jpeg_frame_budget(4000, 4000));
        assert!(!within_jpeg_frame_budget(4001, 4000));
    }

    #[test]
    fn the_frame_budget_is_refused_before_the_rows_exist() {
        // The refusal has to come from `begin`, so a caller that streams a master in has not read a
        // single row before it learns the answer. `write_rows` is therefore never reachable here.
        let mut sink = JpgRowBandSink::new();
        // 2048 x 7813 = 16,001,024 pixels: one row past the 16,000,000 budget.
        assert!(sink.begin(&meta(2048, 7813)).is_err());
        // 2048 x 7812 = 15,998,976: the last row that fits.
        assert!(sink.begin(&meta(2048, 7812)).is_ok());
    }

    #[test]
    fn rows_must_arrive_in_order_and_in_whole_rows() {
        let mut sink = JpgRowBandSink::new();
        let meta = meta(4, 4);
        let mut writer = sink.begin(&meta).expect("begin");
        assert!(matches!(
            writer.write_rows(1, &[0u8; 16]),
            Err(ExportError::OutOfOrder { .. })
        ));
        assert!(matches!(
            writer.write_rows(0, &[0u8; 15]),
            Err(ExportError::RowLength { .. })
        ));
        assert!(matches!(
            writer.write_rows(0, &[0u8; 4 * 4 * 5]),
            Err(ExportError::BeyondHeight { .. })
        ));
    }

    #[test]
    fn a_short_write_cannot_become_a_jpeg() {
        let mut sink = JpgRowBandSink::new();
        let meta = meta(4, 4);
        let mut writer = sink.begin(&meta).expect("begin");
        writer.write_rows(0, &[0u8; 16]).expect("one row");
        let error = writer.finish().unwrap_err();
        assert!(error.to_string().contains("short JPEG"), "{error}");
    }

    #[test]
    fn an_image_past_the_formats_side_is_refused_before_allocating() {
        let mut sink = JpgRowBandSink::new();
        assert!(matches!(
            sink.begin(&meta(70_000, 10)),
            Err(ExportError::TooLarge { .. })
        ));
    }

    #[test]
    fn cancelling_stops_the_encode() {
        let mut sink = JpgRowBandSink::new();
        let meta = meta(64, 64);
        let mut writer = sink.begin(&meta).expect("begin");
        writer.write_rows(0, &vec![0x80u8; 64 * 4 * 64]).expect("rows");
        writer.cancel();
        let error = writer.finish().unwrap_err();
        assert!(
            !error.to_string().is_empty(),
            "a cancelled encode must say so"
        );
    }
}
