//! Saving from the PNG master, in the format the user asked for (`docs/34` §16).
//!
//! Capture has exactly one output: the lossless PNG artifact in `<app local data>/artifacts`.
//! Every other format is derived from it at **save** time, so capture never has to know which
//! format the user wants, and a conversion can never cost quality twice — the master is the
//! contract.
//!
//! The decode is row by row and the sinks decide what to do with those rows: the JPEG sink
//! buffers the frame when it is small enough for the Huffman optimiser to see all of it, and
//! streams band by band when it is not, so a 1280x30000 scroll save never materialises the
//! 153 MiB frame (`docs/34` §10-11). The PNG path does not decode at all.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use super::row_band_jpeg::JpgRowBandSink;
use super::row_band_png::{ExportError, ImageMeta};
use super::save_format::{within_avif_frame_budget, SaveFormat};
use super::webp_sink::WebpSink;

use almost_enough::{StopToken, Unstoppable};
use rgb::Rgba;
use zenavif::{encode_with, EncoderConfig, PixelBuffer};

/// What a conversion did, for the caller's log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversion {
    pub format: SaveFormat,
    pub width: u64,
    pub height: u64,
    pub bytes: u64,
}

/// Converts the PNG master at `master` into `format`, writing the result to `out`.
///
/// `quality` only reaches JPEG; PNG is a byte copy and WebP is lossless, so both ignore it.
/// Every failure is an [`ExportError`]: a truncated or malformed master is `Sink`, a format
/// that cannot hold the master is `TooLarge`, and neither panics.
pub fn convert(
    master: &Path,
    format: SaveFormat,
    quality: u8,
    mut out: impl Write,
) -> Result<Conversion, ExportError> {
    let file = File::open(master).map_err(|error| {
        ExportError::Sink(format!("the PNG master cannot be opened: {error}"))
    })?;

    // PNG to PNG is a copy: decoding and re-encoding a lossless file would spend seconds to
    // produce a different file with the same pixels.
    if format == SaveFormat::Png {
        let mut reader = BufReader::new(file);
        let mut buffer = Vec::new();
        reader
            .read_to_end(&mut buffer)
            .map_err(|error| ExportError::Sink(format!("the PNG master cannot be read: {error}")))?;
        out.write_all(&buffer)
            .map_err(|error| ExportError::Sink(format!("the copy cannot be written: {error}")))?;
        out.flush()
            .map_err(|error| ExportError::Sink(format!("the copy cannot be flushed: {error}")))?;
        let (width, height) = png_size(master)?;
        return Ok(Conversion {
            format,
            width,
            height,
            bytes: buffer.len() as u64,
        });
    }

    let mut decoder = png::Decoder::new(BufReader::new(file));
    // No transformations: the master is written by our own sink as straight RGBA8, and a
    // silent palette or 16-bit expansion would change what the encoder sees.
    decoder.set_transformations(png::Transformations::IDENTITY);
    let mut reader = decoder
        .read_info()
        .map_err(|error| ExportError::Sink(format!("the PNG master has no readable header: {error}")))?;
    // Copies, not a borrow: `next_row` needs the reader mutably once the loop starts.
    let (png_width, png_height, color, depth) = {
        let info = reader.info();
        (info.width, info.height, info.color_type, info.bit_depth)
    };
    let meta = ImageMeta {
        width: u64::from(png_width),
        height: u64::from(png_height),
    };
    if meta.width == 0 || meta.height == 0 {
        return Err(ExportError::Sink("the PNG master is 0 pixels wide or high".into()));
    }
    // The formats' own limits are checked before anything is allocated (`can_encode` is the
    // same rule the scroll session asks before it starts scrolling).
    format.can_encode(meta.width, meta.height).map_err(|_| ExportError::TooLarge {
        width: meta.width,
        height: meta.height,
    })?;

    let mut writer = SinkWriter::begin(format, quality, &meta)?;
    let mut row = vec![0u8; row_bytes(png_width, color, depth)?];
    let mut rows = 0u64;
    loop {
        let next = reader
            .next_row()
            .map_err(|error| ExportError::Sink(format!("the PNG master is corrupt: {error}")))?;
        let Some(source) = next else { break };
        if source.data().len() != row.len() {
            return Err(ExportError::Sink(format!(
                "row {rows} of the PNG master is {} bytes and not {}",
                source.data().len(),
                row.len()
            )));
        }
        // PNG has no BGRA; the sinks take BGRA because that is what Windows hands capture.
        // The swap is a row at a time, so it never costs a second copy of the frame.
        swap_to_bgra(source.data(), &mut row, color);
        writer.write_rows(rows, &row)?;
        rows += 1;
    }
    if rows != meta.height {
        return Err(ExportError::Sink(format!(
            "the PNG master ends after {rows} of {} rows",
            meta.height
        )));
    }
    let bytes = writer.finish(&mut out)?;
    out.flush()
        .map_err(|error| ExportError::Sink(format!("the artifact cannot be flushed: {error}")))?;
    Ok(Conversion {
        format,
        width: meta.width,
        height: meta.height,
        bytes,
    })
}

/// Converts the PNG master at `master` and writes the result beside it, replacing `out`.
///
/// The file is written through a temporary name first, so a failed conversion cannot leave a
/// half-written image where a previous good one was.
pub fn convert_to_file(
    master: &Path,
    out: &Path,
    format: SaveFormat,
    quality: u8,
) -> Result<Conversion, ExportError> {
    let temporary = out.with_extension("partial");
    let file = File::create(&temporary)
        .map_err(|error| ExportError::Sink(format!("the destination cannot be created: {error}")))?;
    let conversion = convert(master, format, quality, BufWriter::new(file));
    match conversion {
        Ok(conversion) => {
            std::fs::rename(&temporary, out).map_err(|error| {
                ExportError::Sink(format!("the artifact cannot be moved into place: {error}"))
            })?;
            Ok(conversion)
        }
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            Err(error)
        }
    }
}

fn png_size(master: &Path) -> Result<(u64, u64), ExportError> {
    let file = File::open(master)
        .map_err(|error| ExportError::Sink(format!("the PNG master cannot be opened: {error}")))?;
    let reader = png::Decoder::new(BufReader::new(file))
        .read_info()
        .map_err(|error| ExportError::Sink(format!("the PNG master has no readable header: {error}")))?;
    let info = reader.info();
    Ok((u64::from(info.width), u64::from(info.height)))
}

/// The bytes one row of the master occupies, in the layout the sinks want.
fn row_bytes(
    width: u32,
    color: png::ColorType,
    depth: png::BitDepth,
) -> Result<usize, ExportError> {
    let channels = match color {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        other => {
            return Err(ExportError::Sink(format!(
                "the PNG master is {other:?}, and every artifact we write is RGB or RGBA"
            )))
        }
    };
    if depth != png::BitDepth::Eight {
        return Err(ExportError::Sink(format!(
            "the PNG master is {depth:?}, and every artifact we write is 8 bits per channel"
        )));
    }
    usize::try_from(u64::from(width) * channels)
        .map_err(|_| ExportError::Sink("the row is wider than this machine can address".into()))
}

/// Rewrites one row of PNG pixels into the BGRA the sinks read.
fn swap_to_bgra(source: &[u8], target: &mut [u8], color: png::ColorType) {
    match color {
        png::ColorType::Rgba => {
            for (source, target) in source.chunks_exact(4).zip(target.chunks_exact_mut(4)) {
                target[0] = source[2];
                target[1] = source[1];
                target[2] = source[0];
                target[3] = source[3];
            }
        }
        // The sinks take four bytes per pixel, so an RGB master is widened with an opaque
        // alpha rather than silently losing its shape.
        png::ColorType::Rgb => {
            let mut source = source.chunks_exact(3);
            for target in target.chunks_exact_mut(4) {
                let Some(pixel) = source.next() else { break };
                target[0] = pixel[2];
                target[1] = pixel[1];
                target[2] = pixel[0];
                target[3] = 255;
            }
        }
        _ => {}
    }
}

/// The AVIF arm: the one format with no row interface. The encoder takes a whole frame, so this
/// holds one, and `SaveFormat::can_encode` is what keeps that bounded before a single row arrives.
struct WholeAvif {
    quality: u8,
    width: u32,
    height: u32,
    pixels: Vec<Rgba<u8>>,
}

impl WholeAvif {
    fn begin(quality: u8, meta: &ImageMeta) -> Self {
        // Reserved once, from a size the caller already approved, so the frame is one allocation
        // rather than a sequence of reallocations.
        let expected = usize::try_from(meta.width.saturating_mul(meta.height)).unwrap_or(0);
        Self {
            quality,
            width: meta.width as u32,
            height: meta.height as u32,
            pixels: Vec::with_capacity(expected),
        }
    }

    /// Rows arrive as BGRA and AVIF wants RGBA. The swap happens per pixel here, because the two
    /// layouts are compatible: handing the buffer over unconverted would encode with red and blue
    /// exchanged and report success (`docs/49`).
    fn write_rows(&mut self, first_row: u64, rows: &[u8]) -> Result<(), ExportError> {
        let written = self.pixels.len() as u64;
        let expected = written / u64::from(self.width);
        if first_row != expected {
            return Err(ExportError::Sink(format!(
                "AVIF received row {first_row} when the frame expects row {expected}"
            )));
        }
        for pixel in rows.chunks_exact(4) {
            self.pixels.push(Rgba {
                r: pixel[2],
                g: pixel[1],
                b: pixel[0],
                a: pixel[3],
            });
        }
        Ok(())
    }

    fn finish(self) -> Result<Vec<u8>, ExportError> {
        let typed = PixelBuffer::<Rgba<u8>>::from_pixels(self.pixels, self.width, self.height)
            .map_err(|error| ExportError::Sink(format!("the AVIF frame is not usable: {error}")))?;
        let erased: PixelBuffer = PixelBuffer::from(typed);
        // Speed 10 is the fastest setting the dependency has, and in release it is about 0.37 s per
        // megapixel (3.08 s for a 4K frame); the earlier 0.7 MP/s figure in this comment was a
        // debug build's number and is not a property of the encoder.
        // (`docs/49`): the budget, the worker and the cancellation are all sized around that.
        let config = EncoderConfig::default()
            .quality(f32::from(self.quality).clamp(1.0, 100.0))
            .speed(10);
        let encoded = encode_with(&erased, &config, StopToken::new(Unstoppable))
            .map_err(|error| ExportError::Sink(format!("the AVIF frame cannot be encoded: {error}")))?;
        Ok(encoded.avif_file)
    }
}

/// One of the row sinks, behind one interface for the loop above.
enum SinkWriter {
    Jpeg(super::row_band_jpeg::JpgRowBandWriter),
    Webp(super::webp_sink::WebpWriter),
    Avif(WholeAvif),
}

impl SinkWriter {
    fn begin(format: SaveFormat, quality: u8, meta: &ImageMeta) -> Result<Self, ExportError> {
        match format {
            SaveFormat::Jpeg => Ok(Self::Jpeg(
                JpgRowBandSink::with_quality(quality).begin(meta)?,
            )),
            SaveFormat::Webp => Ok(Self::Webp(WebpSink::new().begin(meta)?)),
            SaveFormat::Png => Err(ExportError::Sink(
                "PNG is a copy, so it never reaches a row sink".into(),
            )),
            SaveFormat::Avif => {
                // The budget is a save path policy rather than a format limit, so it is checked
                // here, in front of the frame buffer the encoder would need (`docs/49`).
                if !within_avif_frame_budget(meta.width, meta.height) {
                    return Err(ExportError::TooLarge {
                        width: meta.width,
                        height: meta.height,
                    });
                }
                Ok(Self::Avif(WholeAvif::begin(quality, meta)))
            }

        }
    }

    fn write_rows(&mut self, first_row: u64, rows: &[u8]) -> Result<(), ExportError> {
        match self {
            Self::Jpeg(writer) => writer.write_rows(first_row, rows),
            Self::Webp(writer) => writer.write_rows(first_row, rows),
            Self::Avif(writer) => writer.write_rows(first_row, rows),
        }
    }

    fn finish(self, out: &mut impl Write) -> Result<u64, ExportError> {
        // AVIF returns its bytes directly because there is no shared artifact type to wrap them in;
        // the two row sinks keep the length so the copy below sees one shape.
        let bytes = match self {
            Self::Jpeg(writer) => writer.finish()?.bytes,
            Self::Webp(writer) => writer.finish()?.bytes,
            Self::Avif(writer) => writer.finish()?,
        };
        out.write_all(&bytes)
            .map_err(|error| ExportError::Sink(format!("the artifact cannot be written: {error}")))?;
        Ok(bytes.len() as u64)
    }
}

/// Decodes an AVIF artifact back to BGRA rows for the preview.
///
/// The preview has to show the encoded file decoded, not the source pixels, or it lies about what
/// saving produced. An opaque image comes back as `Rgb8` because the encoder drops an alpha channel
/// nothing needs, so the channel count is read from the descriptor rather than assumed (`docs/49`).
pub fn decode_avif(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), ExportError> {
    let image = zenavif::decode(bytes)
        .map_err(|error| ExportError::Sink(format!("the AVIF artifact cannot be decoded: {error}")))?;
    let width = image.width();
    let height = image.height();
    let source = image
        .as_contiguous_bytes()
        .ok_or_else(|| ExportError::Sink("the decoded AVIF frame is not contiguous".into()))?;
    let channels = match image.descriptor().format {
        zenpixels::PixelFormat::Rgb8 => 3,
        zenpixels::PixelFormat::Rgba8 => 4,
        other => {
            return Err(ExportError::Sink(format!(
                "the decoded AVIF frame is {other:?}, and the preview reads RGB or RGBA"
            )))
        }
    };
    let mut bgra = Vec::with_capacity(source.len() / channels * 4);
    for pixel in source.chunks_exact(channels) {
        let alpha = if channels == 4 { pixel[3] } else { 255 };
        bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], alpha]);
    }
    Ok((width, height, bgra))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes an RGBA8 PNG the way the capture sink does, and returns its bytes.
    fn master(width: u32, height: u32, bgra: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("header");
            let mut rgba = Vec::with_capacity(bgra.len());
            for pixel in bgra.chunks_exact(4) {
                rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            }
            writer.write_image_data(&rgba).expect("rows");
        }
        out
    }

    fn ui_frame(width: u32, height: u32) -> Vec<u8> {
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

    fn written(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "snapclip-convert-{}-{name}.png",
            std::process::id()
        ));
        std::fs::write(&path, bytes).expect("the master is written");
        path
    }

    /// The PNG path is a copy: a lossless master saved as PNG must be the same file, not a
    /// re-encode that happens to look alike.
    #[test]
    fn saving_a_png_master_as_png_is_a_byte_copy() {
        let master_bytes = master(8, 5, &ui_frame(8, 5));
        let path = written("copy", &master_bytes);
        let mut out = Vec::new();
        let conversion = convert(&path, SaveFormat::Png, 92, &mut out).expect("a copy");
        assert_eq!(out, master_bytes, "PNG to PNG must not re-encode");
        assert_eq!(conversion.bytes, master_bytes.len() as u64);
        assert_eq!((conversion.width, conversion.height), (8, 5));
        let _ = std::fs::remove_file(&path);
    }

    /// JPEG is 4:4:4 at the requested quality and still decodes to the right shape.
    #[test]
    fn a_png_master_converts_to_a_readable_jpeg() {
        let path = written("jpeg", &master(32, 24, &ui_frame(32, 24)));
        let mut out = Vec::new();
        let conversion = convert(&path, SaveFormat::Jpeg, 92, &mut out).expect("a jpeg");
        assert!(conversion.bytes > 0);
        let decoded = ::image::load_from_memory_with_format(&out, ::image::ImageFormat::Jpeg)
            .expect("jpeg decodes");
        assert_eq!((decoded.width(), decoded.height()), (32, 24));
        assert_eq!(decoded.color(), ::image::ColorType::Rgb8, "JPEG has no alpha");
        let _ = std::fs::remove_file(&path);
    }

    /// The quality setting reaches the encoder instead of being accepted and ignored.
    #[test]
    fn the_quality_setting_changes_the_jpeg() {
        let path = written("quality", &master(64, 48, &ui_frame(64, 48)));
        let mut low = Vec::new();
        let mut high = Vec::new();
        convert(&path, SaveFormat::Jpeg, 30, &mut low).expect("q30");
        convert(&path, SaveFormat::Jpeg, 95, &mut high).expect("q95");
        assert!(
            low.len() < high.len(),
            "q30 ({} bytes) must be smaller than q95 ({} bytes)",
            low.len(),
            high.len()
        );
        let _ = std::fs::remove_file(&path);
    }

    /// A frame past the JPG save path's budget is refused before the master is read, because the
    /// only correct JPG arm holds the whole frame (`docs/34` §17–18).
    #[test]
    fn the_jpeg_frame_budget_is_refused_before_the_master_is_read() {
        let tall = ImageMeta {
            width: 1280,
            height: 30_000,
        };
        let refusal = SinkWriter::begin(SaveFormat::Jpeg, 92, &tall);
        assert!(
            matches!(refusal, Err(ExportError::JpegFrameBudget { .. })),
            "a 38 MP scroll capture must be refused, not encoded"
        );
        // A 4K frame is 8.3 MP, inside the budget, so the same call opens a writer.
        let four_k = ImageMeta {
            width: 3840,
            height: 2160,
        };
        assert!(SinkWriter::begin(SaveFormat::Jpeg, 92, &four_k).is_ok());
    }

    /// WebP's side limit is refused by the same rule the scroll session asks.
    #[test]
    fn webp_refuses_what_it_cannot_hold() {
        assert!(SaveFormat::Webp.can_encode(16_383, 1).is_ok());
        assert!(SaveFormat::Webp.can_encode(16_384, 1).is_err());
        let path = written("webp", &master(4, 4, &ui_frame(4, 4)));
        let mut out = Vec::new();
        let conversion = convert(&path, SaveFormat::Webp, 92, &mut out).expect("webp");
        assert!(conversion.bytes > 0);
        let _ = std::fs::remove_file(&path);
    }

    /// A truncated master is an error, not a panic and not a half-written image.
    #[test]
    fn a_corrupt_master_is_a_structured_error() {
        let mut bytes = master(16, 16, &ui_frame(16, 16));
        bytes.truncate(bytes.len() / 2);
        let path = written("corrupt", &bytes);
        let mut out = Vec::new();
        let error = convert(&path, SaveFormat::Jpeg, 92, &mut out);
        assert!(
            matches!(error, Err(ExportError::Sink(_))),
            "a corrupt master must come back as Sink, got {error:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// The file path writes through a temporary name, so a failed conversion leaves nothing.
    #[test]
    fn a_failed_conversion_leaves_no_file_behind() {
        let path = written("truncated", &{
            let mut bytes = master(8, 8, &ui_frame(8, 8));
            bytes.truncate(bytes.len() / 2);
            bytes
        });
        let destination = std::env::temp_dir().join(format!(
            "snapclip-convert-out-{}.jpg",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&destination);
        let result = convert_to_file(&path, &destination, SaveFormat::Jpeg, 92);
        assert!(result.is_err(), "a corrupt master cannot convert");
        assert!(!destination.exists(), "a failure must not leave an artifact");
        assert!(
            !destination.with_extension("partial").exists(),
            "a failure must not leave its temporary either"
        );
        let _ = std::fs::remove_file(&path);
    }
    /// AVIF is lossy, so the round trip is a tolerance rather than a byte comparison, and the
    /// tolerance has to be tight enough to catch a channel swap or a frame that came back the wrong
    /// size. The checker pattern is deliberate: a flat image would hide a swap.
    #[test]
    fn an_avif_request_round_trips_through_its_decoder() {
        let frame = ui_frame(64, 48);
        let path = written("avif-round-trip", &master(64, 48, &frame));
        let mut out = Vec::new();
        let conversion = convert(&path, SaveFormat::Avif, 80, &mut out).expect("an avif");
        assert_eq!((conversion.width, conversion.height), (64, 48));
        assert_eq!(conversion.bytes, out.len() as u64);

        let (width, height, decoded) = decode_avif(&out).expect("the artifact decodes");
        assert_eq!((width, height), (64, 48));
        assert_eq!(decoded.len(), (64 * 48 * 4) as usize);
        assert_ne!(decoded, frame, "a lossy round trip cannot be the source bytes");

        let mut worst = 0i32;
        for (source, got) in frame.chunks_exact(4).zip(decoded.chunks_exact(4)) {
            for channel in 0..3 {
                worst = worst.max((i32::from(source[channel]) - i32::from(got[channel])).abs());
            }
            assert_eq!(got[3], 255, "an opaque frame stays opaque");
        }
        // A hard checker is the worst case for a lossy codec, so the bound is loose. It still
        // catches what this test exists for: a red/blue channel swap on this pattern moves a
        // channel by at least 180.
        assert!(worst <= 60, "AVIF drifts by {worst} on a two colour pattern");
        let _ = std::fs::remove_file(&path);
    }

    /// The budget is checked in front of the frame buffer, so an over budget image is refused
    /// before anything is allocated rather than after. The sink is asked directly because building
    /// a master that large would cost 48 MB and a minute, and the guard is the same one `convert`
    /// reaches first.
    #[test]
    fn avif_refuses_a_frame_over_its_budget_before_allocating_it() {
        // 1250 x 9601 is 12 001 250 pixels: one row over the 12 MP budget.
        let over = ImageMeta {
            width: 1250,
            height: 9601,
        };
        assert!(matches!(
            SinkWriter::begin(SaveFormat::Avif, 80, &over),
            Err(ExportError::TooLarge { .. })
        ));
        // One row less fits, so the refusal is the budget and not something else.
        let fits = ImageMeta {
            width: 1250,
            height: 9600,
        };
        assert!(SinkWriter::begin(SaveFormat::Avif, 80, &fits).is_ok());
    }

    /// The budget allows a 4K capture (8.29 MP), and the encoder has to cope with that width. This
    /// strip is 3840 wide and 216 tall, so the width is real without paying for a full frame.
    #[test]
    fn the_avif_encoder_handles_a_four_k_width() {
        let frame = ui_frame(3840, 216);
        let path = written("avif-4k-wide", &master(3840, 216, &frame));
        let mut out = Vec::new();
        let conversion = convert(&path, SaveFormat::Avif, 80, &mut out).expect("a 4K wide avif");
        assert_eq!((conversion.width, conversion.height), (3840, 216));
        let (width, height, _) = decode_avif(&out).expect("it decodes back");
        assert_eq!((width, height), (3840, 216));
        let _ = std::fs::remove_file(&path);
    }
    /// Not a correctness test: this is the measurement `docs/49` is built on, kept so the numbers
    /// can be re-taken on another machine instead of trusted from this one. Run with
    /// `--ignored --nocapture`.
    #[test]
    #[ignore = "measurement: encodes a full 4K frame three times and prints the timings"]
    fn measure_a_four_k_save_in_each_format() {
        // Timing numbers are only meaningful in release. A debug build of the AVIF encoder is about
        // thirty times slower, which is how a four kilobyte per second reading became eighty seven
        // seconds for one frame and nearly led to the format being dropped.
        assert!(
            !cfg!(debug_assertions),
            "this measurement must run with --release: cargo test --release ... -- --ignored --nocapture"
        );
        use std::time::Instant;
        // Two contents, because the encoder is far more sensitive to that than to area: a checker
        // is the worst case, and text-like rows on white are what a screenshot actually is.
        let mut text_like = Vec::with_capacity((3840 * 2160 * 4) as usize);
        for y in 0..2160u32 {
            let ink = y % 16 == 0 || (y / 16) % 5 == 0;
            for x in 0..3840u32 {
                let stroke = ink && (x % 7 == 0 || (x / 40) % 3 == 0);
                let (b, g, r) = if stroke { (30, 30, 30) } else { (255, 255, 255) };
                text_like.extend_from_slice(&[b, g, r, 255]);
            }
        }
        let frame = text_like;
        let path = written("avif-4k-measure", &master(3840, 2160, &frame));
        for format in [SaveFormat::Avif, SaveFormat::Png, SaveFormat::Webp] {
            let mut out = Vec::new();
            let started = Instant::now();
            let conversion = convert(&path, format, 80, &mut out).expect("a conversion");
            println!(
                "{:?}: {} bytes in {} ms ({}x{})",
                format,
                conversion.bytes,
                started.elapsed().as_millis(),
                conversion.width,
                conversion.height
            );
        }
        let _ = std::fs::remove_file(&path);
    }

    /// Measures the real thing on a real file: nothing is synthesised. Point `SNAPCLIP_MEASURE_PNG`
    /// at a PNG and this transcodes it through the product's own `convert` into every implemented
    /// format and prints time against size. Synthetic content lies about this: the AVIF encoder is
    /// far more sensitive to sharp edges, which is what a screenshot of a user interface is made of.
    #[test]
    #[ignore = "measurement: transcodes one real PNG into every format and prints the table"]
    fn measure_formats_on_a_file() {
        assert!(
            !cfg!(debug_assertions),
            "this measurement must run with --release: cargo test --release ... -- --ignored --nocapture"
        );
        use std::time::Instant;
        let Ok(path) = std::env::var("SNAPCLIP_MEASURE_PNG") else {
            println!(
                "usage: SNAPCLIP_MEASURE_PNG=<file.png> cargo test --release -p snapclip-app --lib \
                 -- --ignored --nocapture measure_formats_on_a_file"
            );
            return;
        };
        // Reading the master is not what we are measuring, so its time is reported separately.
        let started = Instant::now();
        let info = png::Decoder::new(BufReader::new(
            std::fs::File::open(&path).unwrap_or_else(|error| panic!("{path}: {error}")),
        ))
        .read_info()
        .expect("a readable png")
        .info()
        .clone();
        println!(
            "source: {path}\n        {}x{} ({:.2} MP), header read in {} ms",
            info.width,
            info.height,
            f64::from(info.width) * f64::from(info.height) / 1e6,
            started.elapsed().as_millis()
        );
        let mut baseline = 0u64;
        for format in [SaveFormat::Png, SaveFormat::Webp, SaveFormat::Avif] {
            let mut out = Vec::new();
            let started = Instant::now();
            let result = convert(std::path::Path::new(&path), format, 80, &mut out);
            let elapsed = started.elapsed().as_millis();
            match result {
                Ok(conversion) => {
                    if format == SaveFormat::Png {
                        baseline = u64::from(conversion.bytes);
                    }
                    let ratio = if baseline > 0 {
                        format!("{:.2}x png", conversion.bytes as f64 / baseline as f64)
                    } else {
                        "-".to_owned()
                    };
                    println!(
                        "{format:?}: {} B in {elapsed} ms  ({ratio})",
                        conversion.bytes
                    );
                }
                Err(error) => println!("{format:?}: refused — {error}"),
            }
        }
        println!("Bmp: not implemented yet (docs/49)");
    }
}