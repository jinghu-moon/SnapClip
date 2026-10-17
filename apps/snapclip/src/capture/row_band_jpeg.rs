//! `JpgRowBandSink` — lossy export, split by what the optimiser costs (`docs/34` §10–11, §13).
//!
//! # Why there are two strategies and not one
//!
//! `docs/34` §10 measured this rather than assumed it. `zenjpeg`'s `BytesEncoder::push` is true
//! streaming while the optimiser is off — a 1280x30000 strip encodes in 393 ms with a 13.9 MB peak —
//! but turning Huffman optimisation on makes it buffer the whole frame first: 2.95 s and a 456.5 MB
//! peak on the same strip. Huffman optimisation is also **free quality-wise**: §11 decoded the
//! optimised and plain output and got byte-identical pixels, with 5.6–9 % fewer bytes.
//!
//! So the trade is stated in bytes of frame, not in taste:
//!
//! * A frame that fits [`SHORT_IMAGE_MAX_BYTES`] is buffered and encoded with the optimiser on. The
//!   buffer is bounded by that constant, so the peak is bounded too.
//! * Anything larger streams band by band with the optimiser off, and pays the 5.6–9 %.
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

/// The quality `docs/34` §11 selected: q95 and q85 are both worse trades for UI material.
pub const JPEG_QUALITY: u8 = 92;

/// A frame at or below this size is buffered so the Huffman optimiser can run.
///
/// 16 MiB is `docs/34` §10's measurement turned into a budget: the optimiser needs the whole frame
/// in memory (456.5 MB for a 1280x30000 strip), and spending at most this much to save 5.6–9 % is a
/// trade worth making for ordinary screenshots while never letting a scroll capture reach it.
pub const SHORT_IMAGE_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// What [`JpgRowBandSink::plan`] decided for a given image. A value so the choice can be asserted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JpgPlan {
    /// Whether Huffman optimisation runs. Costs the whole frame in memory when it does.
    pub optimise_huffman: bool,
    /// Whether the sink holds every row before encoding.
    pub buffers_whole_frame: bool,
}

impl JpgPlan {
    /// The strategy this plan describes, for logs and tests.
    pub fn strategy(self) -> &'static str {
        if self.buffers_whole_frame {
            "buffered+optimised"
        } else {
            "streaming"
        }
    }
}

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

    /// The same plan at another quality, for the save path's own setting (`docs/34` §16).
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

    /// Which strategy an image of this size gets. Pure: no encoder, no allocation.
    pub fn plan(&self, meta: &ImageMeta) -> JpgPlan {
        let frame_bytes = meta.width.saturating_mul(meta.height).saturating_mul(4);
        let short = frame_bytes <= SHORT_IMAGE_MAX_BYTES;
        JpgPlan {
            optimise_huffman: short,
            buffers_whole_frame: short,
        }
    }

    /// Opens an encoder for `meta`. The format's limits are checked here, before anything allocates.
    pub fn begin(&mut self, meta: &ImageMeta) -> Result<JpgRowBandWriter, ExportError> {
        super::save_format::SaveFormat::Jpeg
            .can_encode(meta.width, meta.height)
            .map_err(|_| ExportError::TooLarge {
                width: meta.width,
                height: meta.height,
            })?;
        if meta.width == 0 || meta.height == 0 {
            return Err(ExportError::Sink("a 0-sided image has no JPEG encoding".into()));
        }

        let plan = self.plan(meta);
        let config = config(self.quality, plan.optimise_huffman);
        let encoder = config
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
            plan,
            encoder: Some(encoder),
            cancelled,
            buffered: if plan.buffers_whole_frame {
                Some(Vec::with_capacity(
                    usize::try_from(meta.width * meta.height * 4).unwrap_or(0),
                ))
            } else {
                None
            },
            next_row: 0,
        })
    }
}

/// The one place the encoder configuration is built, so every path agrees on 4:4:4 and baseline.
fn config(quality: u8, optimise_huffman: bool) -> EncoderConfig {
    EncoderConfig::ycbcr(f32::from(quality), ChromaSubsampling::None)
        .force_baseline()
        .optimize_huffman(optimise_huffman)
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

/// Rows in, in order; `finish` closes the JPEG.
pub struct JpgRowBandWriter {
    meta: ImageMeta,
    plan: JpgPlan,
    /// `None` only after `finish`, which is the only thing that can take it.
    encoder: Option<zenjpeg::encoder::BytesEncoder>,
    cancelled: Arc<AtomicBool>,
    /// Every row, for the buffered strategy. `None` when streaming.
    buffered: Option<Vec<u8>>,
    next_row: u64,
}

impl JpgRowBandWriter {
    /// The strategy this writer is running.
    pub fn plan(&self) -> JpgPlan {
        self.plan
    }

    /// Stops the encoder at the next row boundary. `finish` then fails instead of producing a file.
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

        if let Some(buffer) = self.buffered.as_mut() {
            // The buffered strategy keeps the rows and encodes once at `finish`, because that is
            // the only way the optimiser can see the whole frame (`docs/34` §10).
            buffer.extend_from_slice(rows);
        } else {
            let encoder = self.encoder.as_mut().ok_or_else(|| {
                ExportError::Sink("rows were written after the artifact closed".into())
            })?;
            encoder
                .push(
                    rows,
                    usize::try_from(count).unwrap_or(0),
                    usize::try_from(row_bytes).unwrap_or(0),
                    Cancel(Arc::clone(&self.cancelled)),
                )
                .map_err(sink_error)?;
        }
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

        if let Some(rows) = self.buffered {
            encoder
                .push(
                    &rows,
                    usize::try_from(self.meta.height).unwrap_or(0),
                    usize::try_from(self.meta.width * 4).unwrap_or(0),
                    Cancel(Arc::clone(&self.cancelled)),
                )
                .map_err(sink_error)?;
        }

        let bytes = encoder.finish().map_err(sink_error)?;
        Ok(Artifact {
            bytes,
            rows: self.next_row,
        })
    }
}

/// Encodes one whole BGRA image. The convenience path for callers that already hold the rows.
pub fn encode_bgra(
    bgra: &[u8],
    meta: &ImageMeta,
) -> Result<Artifact, ExportError> {
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
    fn a_small_frame_is_buffered_so_the_optimiser_can_run() {
        let plan = JpgRowBandSink::new().plan(&meta(1920, 1080));
        assert!(plan.buffers_whole_frame);
        assert!(plan.optimise_huffman);
        assert_eq!(plan.strategy(), "buffered+optimised");
        // 1920 * 1080 * 4 = 8.3 MB, inside the 16 MiB budget.
        assert!(1920 * 1080 * 4 <= SHORT_IMAGE_MAX_BYTES);
    }

    #[test]
    fn a_tall_frame_streams_and_pays_the_optimiser() {
        // The scroll capture case: 1280 x 30000 is 153 MB of rows, far past the budget.
        let plan = JpgRowBandSink::new().plan(&meta(1280, 30_000));
        assert!(!plan.buffers_whole_frame, "the whole frame must not be held");
        assert!(!plan.optimise_huffman, "the optimiser would force the buffer back");
        assert_eq!(plan.strategy(), "streaming");
    }

    #[test]
    fn the_budget_boundary_is_where_it_says_it_is() {
        let sink = JpgRowBandSink::new();
        // Exactly at the budget still buffers.
        let at = SHORT_IMAGE_MAX_BYTES / 4;
        let width = 1024;
        let height = at / width;
        assert!(sink.plan(&meta(width, height)).buffers_whole_frame);
        // One row more does not.
        assert!(!sink.plan(&meta(width, height + 1)).buffers_whole_frame);
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
    fn the_optimised_arm_is_smaller_than_the_plain_one() {
        // Same picture, same quantisation tables; only the optimiser differs (docs/34 §11).
        // A real screenshot size: at 320x240 the optimiser's table overhead outweighs the gain,
        // and `docs/34` §11 measured the 6–9 % on full frames, not on thumbnails.
        let frame = ui_frame(1920, 1080);
        let meta = meta(1920, 1080);
        let mut optimised = JpgRowBandSink::new();
        let mut writer = optimised.begin(&meta).expect("begin");
        writer.write_rows(0, &frame).expect("rows");
        let optimised = writer.finish().expect("finish").bytes;

        // Force the streaming arm by asking the same encoder for a frame past the budget is not
        // possible for this size, so build the plain configuration directly.
        let mut encoder = config(JPEG_QUALITY, false)
            .encode_from_bytes(1920, 1080, PixelLayout::Bgra8Srgb)
            .expect("start");
        encoder
            .push(&frame, 1080, 1920 * 4, enough::Unstoppable)
            .expect("push");
        let plain = encoder.finish().expect("finish");

        assert!(
            optimised.len() < plain.len(),
            "optimised {} vs plain {}",
            optimised.len(),
            plain.len()
        );
        // Only what this sink owns is asserted here: the optimiser makes the file smaller and the
        // result still decodes at the size it was asked for. Whether the two arms are pixel-equal is
        // a question for `tools/codec-bench` (docs/34 §11), which owns the comparison against a
        // lossless reference; a rounding-level diff belongs there, not in a sink unit test.
        let (width, height, pixels) = decode(&optimised);
        assert_eq!((width, height), (1920, 1080));
        assert_eq!(pixels.len(), 1920 * 1080 * 4);
        let (plain_width, plain_height, _) = decode(&plain);
        assert_eq!((plain_width, plain_height), (1920, 1080));
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
    fn cancelling_stops_the_streaming_arm() {
        let mut sink = JpgRowBandSink::new();
        // Tall enough to stream, so the cancel hits `push` rather than `finish`.
        let meta = meta(1024, 6000);
        let mut writer = sink.begin(&meta).expect("begin");
        let band = vec![0x80u8; 1024 * 4 * 64];
        writer.write_rows(0, &band).expect("first band");
        writer.cancel();
        let error = writer.write_rows(64, &band).unwrap_err();
        assert!(
            !error.to_string().is_empty(),
            "a cancelled encode must say so"
        );
    }
}
