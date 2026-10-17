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
use super::save_format::SaveFormat;
use super::webp_sink::WebpSink;

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

/// One of the two row sinks, behind one interface for the loop above.
enum SinkWriter {
    Jpeg(super::row_band_jpeg::JpgRowBandWriter),
    Webp(super::webp_sink::WebpWriter),
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
        }
    }

    fn write_rows(&mut self, first_row: u64, rows: &[u8]) -> Result<(), ExportError> {
        match self {
            Self::Jpeg(writer) => writer.write_rows(first_row, rows),
            Self::Webp(writer) => writer.write_rows(first_row, rows),
        }
    }

    fn finish(self, out: &mut impl Write) -> Result<u64, ExportError> {
        let artifact = match self {
            Self::Jpeg(writer) => writer.finish()?,
            Self::Webp(writer) => writer.finish()?,
        };
        out.write_all(&artifact.bytes)
            .map_err(|error| ExportError::Sink(format!("the artifact cannot be written: {error}")))?;
        Ok(artifact.bytes.len() as u64)
    }
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
}
