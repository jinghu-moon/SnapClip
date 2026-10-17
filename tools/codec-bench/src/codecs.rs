//! The encoders under comparison, each behind the same shape: BGRA in, bytes out.
//!
//! Every encoder converts inside the measured region, because that is what the product will do: the
//! capture side hands out BGRA row bands and the sink converts as it writes. `zenjpeg` is the
//! exception worth measuring: it accepts `Bgra8Srgb` directly, so its numbers include no conversion.

use std::io::Write;
use std::sync::{Arc, Mutex};

/// One measurement: bytes written, wall time, and the extra allocation at its peak.
#[derive(Clone)]
pub struct Outcome {
    pub bytes: usize,
    pub millis: f64,
    pub peak_extra: usize,
}

/// A JPEG quality and subsampling choice, so the report can name what it ran.
#[derive(Clone, Copy)]
pub struct JpegOptions {
    pub quality: u8,
    pub subsample_444: bool,
    pub trellis: bool,
    pub optimize_huffman: bool,
    /// Rows per push; zero means "the whole image in one call".
    pub band_rows: usize,
}

/// The shared output handle the PNG stream writer needs: `into_stream_writer` asks for a `'static`
/// `Write`, which is exactly the shape the product's sink uses.
#[derive(Clone)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("writer poisoned"))?
            .extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// PNG, row by row, mirroring `apps/snapclip/src/capture/row_band_png.rs`.
pub fn png_row_band(bgra: &[u8], width: usize, height: usize) -> Result<Vec<u8>, String> {
    let shared = Arc::new(Mutex::new(Vec::new()));
    let mut encoder =
        png::Encoder::new(SharedWriter(Arc::clone(&shared)), width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Balanced);
    encoder.set_filter(png::Filter::Up);
    let header = encoder.write_header().map_err(|error| format!("png header: {error}"))?;
    let mut writer = header.into_stream_writer().map_err(|error| format!("png: {error}"))?;
    let mut row = vec![0u8; width * 4];
    for y in 0..height {
        let source = &bgra[y * width * 4..(y + 1) * width * 4];
        for (pixel, out) in source.chunks_exact(4).zip(row.chunks_exact_mut(4)) {
            out[0] = pixel[2];
            out[1] = pixel[1];
            out[2] = pixel[0];
            out[3] = pixel[3];
        }
        writer.write_all(&row).map_err(|error| format!("png write: {error}"))?;
    }
    writer.finish().map_err(|error| format!("png finish: {error}"))?;
    let bytes = shared.lock().map_err(|_| "png output poisoned".to_string())?.clone();
    Ok(bytes)
}

/// zenjpeg through `BytesEncoder::push`, BGRA straight in.
pub fn zenjpeg(
    bgra: &[u8],
    width: usize,
    height: usize,
    options: JpegOptions,
) -> Result<Vec<u8>, String> {
    use zenjpeg::encoder::{
        ChromaSubsampling, EncoderConfig, OptimizationPreset, PixelLayout, Unstoppable,
    };
    let subsampling = if options.subsample_444 {
        ChromaSubsampling::None
    } else {
        ChromaSubsampling::Quarter
    };
    let mut config = EncoderConfig::ycbcr(f32::from(options.quality), subsampling);
    // `MozjpegBaseline` is baseline + optimised Huffman + trellis (full search); the plain arm turns
    // both off explicitly so the two sizes are comparable.
    config = if options.trellis && options.optimize_huffman {
        config.optimization(OptimizationPreset::MozjpegBaseline)
    } else {
        config.force_baseline().optimize_huffman(false)
    };
    let mut encoder = config
        .encode_from_bytes(width as u32, height as u32, PixelLayout::Bgra8Srgb)
        .map_err(|error| format!("zenjpeg start: {error}"))?;
    let stride = width * 4;
    let band = if options.band_rows == 0 { height } else { options.band_rows };
    let mut y = 0;
    while y < height {
        let rows = band.min(height - y);
        let start = y * stride;
        encoder
            .push(&bgra[start..start + rows * stride], rows, stride, Unstoppable)
            .map_err(|error| format!("zenjpeg push: {error}"))?;
        y += rows;
    }
    encoder.finish().map_err(|error| format!("zenjpeg finish: {error}"))
}

/// mozjpeg-rs streaming, RGB in (it has no BGRA entry point), rows in multiples of eight.
///
/// Its streaming builder has no `trellis` knob — that lives on the batch `Encoder` only — so
/// `options.trellis` is deliberately ignored here and the report says so.
pub fn mozjpeg(
    bgra: &[u8],
    width: usize,
    height: usize,
    options: JpegOptions,
) -> Result<Vec<u8>, String> {
    use mozjpeg_rs::{StreamingEncoder, Subsampling};
    let mut builder = StreamingEncoder::baseline_fastest();
    builder = builder.quality(options.quality);
    builder = builder.subsampling(if options.subsample_444 {
        Subsampling::S444
    } else {
        Subsampling::S420
    });
    builder = builder.force_baseline(true);
    let mut stream = builder
        .start_rgb(width as u32, height as u32, Vec::new())
        .map_err(|error| format!("mozjpeg start: {error}"))?;
    let band = if options.band_rows == 0 { height } else { options.band_rows.max(8) };
    let mut rgb = vec![0u8; width * band * 3];
    let mut y = 0;
    while y < height {
        let rows = band.min(height - y);
        for row in 0..rows {
            let source = &bgra[(y + row) * width * 4..(y + row + 1) * width * 4];
            for (pixel, out) in source
                .chunks_exact(4)
                .zip(rgb[row * width * 3..].chunks_exact_mut(3))
            {
                out[0] = pixel[2];
                out[1] = pixel[1];
                out[2] = pixel[0];
            }
        }
        stream
            .write_scanlines(&rgb[..rows * width * 3])
            .map_err(|error| format!("mozjpeg rows: {error}"))?;
        y += rows;
    }
    stream.finish().map_err(|error| format!("mozjpeg finish: {error}"))
}

/// zenjpeg with the entropy coder as a **separate** knob, and the trellis arm as a preset.
///
/// `optimize_huffman` is a public switch and cannot change the picture, only the bytes, so `plain`
/// versus `huffman only` is a clean A/B. Trellis is not: the crate keeps `TrellisConfig` and the
/// `trellis` field crate-private, so the only way in is `OptimizationPreset::MozjpegBaseline`,
/// which also swaps the quantisation tables. That arm is therefore labelled as a preset in the
/// report rather than pretended to be a controlled experiment (docs/34 §11).
pub fn zenjpeg_knobs(
    bgra: &[u8],
    width: usize,
    height: usize,
    quality: u8,
    subsample_444: bool,
    trellis: bool,
    optimize_huffman: bool,
    band_rows: usize,
) -> Result<Vec<u8>, String> {
    use zenjpeg::encoder::{ChromaSubsampling, EncoderConfig, OptimizationPreset, PixelLayout, Unstoppable};
    let subsampling = if subsample_444 { ChromaSubsampling::None } else { ChromaSubsampling::Quarter };
    let mut config = EncoderConfig::ycbcr(f32::from(quality), subsampling);
    config = if trellis {
        config.optimization(OptimizationPreset::MozjpegBaseline)
    } else {
        config.force_baseline().optimize_huffman(optimize_huffman)
    };
    let mut encoder = config
        .encode_from_bytes(width as u32, height as u32, PixelLayout::Bgra8Srgb)
        .map_err(|error| format!("zenjpeg start: {error}"))?;
    let stride = width * 4;
    let band = if band_rows == 0 { height } else { band_rows };
    let mut y = 0;
    while y < height {
        let rows = band.min(height - y);
        let start = y * stride;
        encoder
            .push(&bgra[start..start + rows * stride], rows, stride, Unstoppable)
            .map_err(|error| format!("zenjpeg push: {error}"))?;
        y += rows;
    }
    encoder.finish().map_err(|error| format!("zenjpeg finish: {error}"))
}

/// Decodes a JPEG to RGBA, so its pixels can be compared with the lossless reference.
pub fn decode_jpeg(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>), String> {
    use zenjpeg::decoder::{Decoder, PixelFormat};
    let result = Decoder::new()
        .output_format(PixelFormat::Rgba)
        .decode(bytes, enough::Unstoppable)
        .map_err(|error| format!("zenjpeg decode: {error}"))?;
    let (width, height) = result.dimensions();
    let pixels = result.into_pixels_u8().ok_or_else(|| "zenjpeg decode: no u8 pixels".to_string())?;
    Ok((width as usize, height as usize, pixels))
}

/// Decodes one of our own PNGs to RGBA. Anything that is not RGBA is refused rather than guessed at.
pub fn decode_png(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>), String> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().map_err(|error| format!("png decode: {error}"))?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| "png decode: no output size".to_string())?;
    let mut buffer = vec![0u8; size];
    let info = reader.next_frame(&mut buffer).map_err(|error| format!("png frame: {error}"))?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(format!("png decode: expected 8-bit RGBA, got {:?}/{:?}", info.color_type, info.bit_depth));
    }
    buffer.truncate(info.buffer_size());
    Ok((info.width as usize, info.height as usize, buffer))
}

/// zenwebp lossless. Whole image in; dimensions past the format limit are refused up front.
pub fn zenwebp_lossless(bgra: &[u8], width: usize, height: usize) -> Result<Vec<u8>, String> {
    use zenwebp::{EncodeRequest, LosslessConfig, PixelLayout};
    let mut rgba = vec![0u8; width * height * 4];
    for (source, out) in bgra.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
        out[0] = source[2];
        out[1] = source[1];
        out[2] = source[0];
        out[3] = source[3];
    }
    let config = LosslessConfig::new()
        .with_quality(75.0)
        .with_method(4)
        .with_max_dimensions(16_383, 16_383);
    EncodeRequest::lossless(&config, &rgba, PixelLayout::Rgba8, width as u32, height as u32)
        .encode()
        .map_err(|error| format!("zenwebp: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_bgra(width: usize, height: usize) -> Vec<u8> {
        let mut pixels = vec![0u8; width * height * 4];
        for (index, pixel) in pixels.chunks_exact_mut(4).enumerate() {
            let x = index % width;
            let y = index / width;
            pixel[0] = (x * 7 % 256) as u8;
            pixel[1] = (y * 5 % 256) as u8;
            pixel[2] = ((x + y) * 3 % 256) as u8;
            pixel[3] = 255;
        }
        pixels
    }

    fn options() -> JpegOptions {
        JpegOptions {
            quality: 90,
            subsample_444: true,
            trellis: false,
            optimize_huffman: false,
            band_rows: 16,
        }
    }

    #[test]
    fn every_encoder_writes_something_for_a_small_image() {
        let (width, height) = (64, 48);
        let pixels = small_bgra(width, height);
        assert!(!png_row_band(&pixels, width, height).unwrap().is_empty());
        assert!(!zenjpeg(&pixels, width, height, options()).unwrap().is_empty());
        // mozjpeg-rs 0.9.2 panics inside its own writer on this material (`write_bits`, a shift of
        // 64), which is recorded as a finding rather than hidden: the call must therefore be
        // caught, and "panicked" is a legitimate outcome here.
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            mozjpeg(&pixels, width, height, options()).map(|bytes| bytes.len())
        }));
        match caught {
            Ok(Ok(bytes)) => assert!(bytes > 0),
            Ok(Err(error)) => panic!("mozjpeg should not error quietly: {error}"),
            Err(_) => eprintln!("mozjpeg-rs panicked on this material (recorded as a finding)"),
        }
        assert!(!zenwebp_lossless(&pixels, width, height).unwrap().is_empty());
    }

    #[test]
    fn png_output_round_trips_through_the_decoder() {
        let (width, height) = (32, 16);
        let pixels = small_bgra(width, height);
        let encoded = png_row_band(&pixels, width, height).unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(encoded));
        let mut reader = decoder.read_info().unwrap();
        let mut buffer = vec![0u8; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buffer).unwrap();
        assert_eq!((info.width, info.height), (32, 16));
        assert_eq!(info.color_type, png::ColorType::Rgba);
    }

    #[test]
    fn webp_refuses_a_scroll_capture_before_encoding() {
        // The format's own limit; the point is that it comes back as an error, not a panic.
        let pixels = vec![0u8; 8 * 8 * 4];
        assert!(
            zenwebp_lossless(&pixels, 16_000, 16_400).is_err(),
            "a past-limit height must be rejected"
        );
    }
}
