//! The row preview: what one history row hands the renderer for an image payload.
//!
//! `docs/30 §19.2` and `ADR-5` say a long capture's preview has to come from a **window** of the
//! image rather than from the whole thing. The artifact a scroll capture produces is
//! 1058 x 502649 — about 2 GiB of RGBA — and the box a row draws it in is `THUMBNAIL_PX` (32 px)
//! wide, so "hand the payload to the renderer and let the renderer scale it" decodes two
//! gigabytes to fill one row of a list.
//!
//! This module is the seam that makes the claim a fact about a function rather than a fact about a
//! `gpui` render context: [`row_preview_png`] takes the payload bytes the store returned and gives
//! back the bytes the row renders. `apps/snapclip/tests/history_preview.rs` measures the size of
//! the second, and `apps/snapclip/tests/history_preview_memory.rs` measures what producing it
//! costs.
//!
//! # RED (2026-10-09)
//!
//! The seam was extracted **with today's behaviour** — the payload, verbatim
//! (`history/view.rs::load_thumbnail` handed `image_bytes` straight to `Image::from_bytes`) — so
//! that the claim fails before it is implemented:
//! `the_row_preview_is_a_window_not_the_artifact` reports `256x20000` against a 128 px budget.
//! The extraction is what makes the failure observable without a render context; it is not the
//! fix. `docs/31` records it that way.

use std::fmt;
use std::io::Cursor;

/// Longest side of a row preview, in pixels.
///
/// The row draws it in a 32 px box, so 128 px is already 4x oversampled on a 150%-DPI display. It
/// is the same target the scroll preview uses (`docs/30 §19.3`), on purpose: one number for "a
/// preview is this big", not two.
pub const PREVIEW_MAX_PX: u32 = 128;

/// Why a preview could not be produced.
#[derive(Debug)]
pub enum PreviewError {
    /// The source bytes are not a PNG the decoder can read.
    Decode(String),
    /// The image has no pixels, so there is no window to take.
    Empty,
    /// A single row of the source image is too long to address.
    TooLarge,
    /// The window could not be encoded back to PNG.
    Encode(String),
}

impl fmt::Display for PreviewError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(message) => write!(formatter, "the preview source is not a readable png: {message}"),
            Self::Empty => formatter.write_str("the image has no pixels"),
            Self::TooLarge => formatter.write_str("one row of the image is too long to address"),
            Self::Encode(message) => write!(formatter, "the preview could not be encoded: {message}"),
        }
    }
}

impl std::error::Error for PreviewError {}

/// The bytes one history row hands the renderer for an image payload.
///
/// The window is `PREVIEW_MAX_PX` **output** rows, which is `PREVIEW_MAX_PX * scale` source rows:
/// for an image shorter than that the window is the whole image (the ordinary screenshot), and for
/// a tall capture it is the top of it. Reading stops there — the decoder never walks the other
/// 500,000 rows, and nothing larger than one output row is ever held.
///
/// It is the **top** and not a centred window on purpose: the first rows of a page capture are the
/// part a user recognises in a list, and centring would make the preview depend on a height the
/// row cannot show anyway.
pub fn row_preview_png(source: &[u8]) -> Result<Vec<u8>, PreviewError> {
    let mut decoder = png::Decoder::new(Cursor::new(source));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|error| PreviewError::Decode(error.to_string()))?;
    let (width, height, color_type) = {
        let info = reader.info();
        (info.width, info.height, info.color_type)
    };
    if width == 0 || height == 0 {
        return Err(PreviewError::Empty);
    }
    let channels = match color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        // `normalize_to_color8` expands a palette into RGB, so this is a contradiction rather
        // than a case to handle.
        png::ColorType::Indexed => return Err(PreviewError::Decode("a palette survived the transform".into())),
    };
    let row_bytes = reader
        .output_line_size(width)
        .ok_or(PreviewError::TooLarge)?;

    // An integer scale, so every output pixel averages a whole block of source pixels: nearest
    // neighbour would turn a page of text into noise at 128 px, and a fractional scale would make
    // the block sizes drift along the row.
    let scale = width.div_ceil(PREVIEW_MAX_PX).max(1);
    let out_width = width.div_ceil(scale) as usize;
    let source_rows = PREVIEW_MAX_PX.saturating_mul(scale).min(height);
    let out_height = source_rows.div_ceil(scale) as usize;
    // How many source columns each output column covers: `scale` for all but the last, which is
    // short whenever the width is not a multiple of `scale`.
    let column_counts: Vec<u32> = (0..out_width)
        .map(|index| (width - index as u32 * scale).min(scale))
        .collect();

    let mut out = vec![0u8; out_width * out_height * 4];
    let mut sum = vec![0u32; out_width * 4];
    let mut covered = 0u32;
    let mut out_row = 0usize;
    for y in 0..source_rows {
        let Some(row) = reader
            .next_row()
            .map_err(|error| PreviewError::Decode(error.to_string()))?
        else {
            break;
        };
        let data = row.data();
        if data.len() < row_bytes {
            return Err(PreviewError::Decode(format!(
                "a row of {width} pixels came back as {} bytes, expected {row_bytes}",
                data.len()
            )));
        }
        for x in 0..width as usize {
            let pixel = &data[x * channels..x * channels + channels];
            let (red, green, blue, alpha) = match color_type {
                png::ColorType::Grayscale => (pixel[0], pixel[0], pixel[0], 0xFF),
                png::ColorType::GrayscaleAlpha => (pixel[0], pixel[0], pixel[0], pixel[1]),
                png::ColorType::Rgb => (pixel[0], pixel[1], pixel[2], 0xFF),
                _ => (pixel[0], pixel[1], pixel[2], pixel[3]),
            };
            let base = (x / scale as usize) * 4;
            sum[base] += u32::from(red);
            sum[base + 1] += u32::from(green);
            sum[base + 2] += u32::from(blue);
            sum[base + 3] += u32::from(alpha);
        }
        covered += 1;
        if (y + 1) % scale != 0 && y + 1 != source_rows {
            continue;
        }
        let target = &mut out[out_row * out_width * 4..(out_row + 1) * out_width * 4];
        for block in 0..out_width {
            let divisor = covered * column_counts[block];
            for channel in 0..4 {
                let index = block * 4 + channel;
                target[index] = (sum[index] / divisor) as u8;
                sum[index] = 0;
            }
        }
        covered = 0;
        out_row += 1;
    }

    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, out_width as u32, out_height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Balanced);
        // The same row filter as the export path (`capture/row_band_png.rs`): the preview is also
        // a screenshot, and `Up` beats the adaptive default on one.
        encoder.set_filter(png::Filter::Up);
        let mut writer = encoder
            .write_header()
            .map_err(|error| PreviewError::Encode(error.to_string()))?;
        writer
            .write_image_data(&out)
            .map_err(|error| PreviewError::Encode(error.to_string()))?;
    }
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A tall page: dark text runs on a light background.
    fn tall_page_png(width: u32, height: u32) -> Vec<u8> {
        let mut rows = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for y in 0..height {
            for x in 0..width {
                let value = if (x / 3 + y / 500) % 2 == 0 { 0x20 } else { 0xF0 };
                rows.extend_from_slice(&[value, value, value, 0xFF]);
            }
        }
        let mut encoded = Vec::new();
        let mut encoder = png::Encoder::new(&mut encoded, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .expect("a png header")
            .write_image_data(&rows)
            .expect("the rows");
        encoded
    }

    /// The declared size of a PNG, read from its header alone — no pixels are decoded.
    fn declared_size(png: &[u8]) -> (u32, u32) {
        let reader = png::Decoder::new(Cursor::new(png))
            .read_info()
            .expect("a readable png");
        let info = reader.info();
        (info.width, info.height)
    }

    /// The pixels of a PNG, as RGBA8.
    fn decode_rgba(png: &[u8]) -> (u32, u32, Vec<u8>) {
        let mut reader = png::Decoder::new(Cursor::new(png))
            .read_info()
            .expect("a readable png");
        let mut buffer = vec![0u8; reader.output_buffer_size().expect("an addressable image")];
        let info = reader.next_frame(&mut buffer).expect("the pixels");
        buffer.truncate(info.buffer_size());
        (info.width, info.height, buffer)
    }

    /// A `width` x `height` image with two horizontal bands: `top_rows` rows of `top`, the rest
    /// `bottom`. (A band boundary in the middle is what tells "the window" from "the whole image"
    /// apart.)
    fn banded_png(width: u32, height: u32, top_rows: u32, top: [u8; 3], bottom: [u8; 3]) -> Vec<u8> {
        let mut rows = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for y in 0..height {
            let colour = if y < top_rows { top } else { bottom };
            for _ in 0..width {
                rows.extend_from_slice(&[colour[0], colour[1], colour[2], 0xFF]);
            }
        }
        let mut encoded = Vec::new();
        let mut encoder = png::Encoder::new(&mut encoded, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .expect("a png header")
            .write_image_data(&rows)
            .expect("the rows");
        encoded
    }

    #[test]
    fn the_row_preview_is_a_window_not_the_artifact() {
        const WIDTH: u32 = 256;
        const HEIGHT: u32 = 20_000;

        let source = tall_page_png(WIDTH, HEIGHT);
        let preview = row_preview_png(&source).expect("a preview");
        let (width, height) = declared_size(&preview);

        assert!(
            width <= PREVIEW_MAX_PX && height <= PREVIEW_MAX_PX,
            "the row hands the renderer a {width}x{height} image, and the row draws it in a \
             {PREVIEW_MAX_PX} px box: the artifact is {WIDTH}x{HEIGHT} ({decoded} MiB of RGBA \
             once the renderer decodes it), and a scroll capture is 1058x502649 ({artifact} MiB)",
            decoded = (u64::from(WIDTH) * u64::from(HEIGHT) * 4) / (1024 * 1024),
            artifact = (1058u64 * 502_649 * 4) / (1024 * 1024),
        );
    }

    /// The size claim alone could be satisfied by a blank image, so this one asks for the pixels:
    /// a quarter-coloured image has to come back with that quarter in the right place.
    #[test]
    fn the_preview_carries_the_pixels_of_the_window() {
        let mut rows = Vec::new();
        for y in 0..256u32 {
            for x in 0..256u32 {
                let (red, green): (u8, u8) = if x < 128 && y < 128 { (0xF0, 0x20) } else { (0x20, 0xF0) };
                rows.extend_from_slice(&[red, green, 0x40, 0xFF]);
            }
        }
        let mut source = Vec::new();
        let mut encoder = png::Encoder::new(&mut source, 256, 256);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .expect("a png header")
            .write_image_data(&rows)
            .expect("the rows");

        let preview = row_preview_png(&source).expect("a preview");
        let (width, height, pixels) = decode_rgba(&preview);
        assert_eq!((width, height), (128, 128), "a 256x256 image at a 128 px target");

        let pixel = |x: u32, y: u32| {
            let base = ((y * width + x) * 4) as usize;
            (pixels[base], pixels[base + 1], pixels[base + 2])
        };
        // Box-averaged over a 2x2 block, so the values are the source values.
        assert_eq!(pixel(0, 0), (0xF0, 0x20, 0x40), "the top-left quadrant");
        assert_eq!(pixel(width - 1, 0), (0x20, 0xF0, 0x40), "the top-right quadrant");
        assert_eq!(pixel(0, height - 1), (0x20, 0xF0, 0x40), "the bottom-left quadrant");
        assert_eq!(pixel(width - 1, height - 1), (0x20, 0xF0, 0x40), "the bottom-right");
    }

    /// And this is the claim the module doc makes in words: the window is the **top** of a tall
    /// page, so a page whose first rows are white comes back white — if the whole image had been
    /// averaged, 256 of 2000 rows of white would be a dark grey.
    #[test]
    fn the_window_is_the_top_of_a_tall_page() {
        let source = banded_png(256, 2_000, 256, [0xFF; 3], [0x00; 3]);
        let preview = row_preview_png(&source).expect("a preview");
        let (width, height, pixels) = decode_rgba(&preview);
        assert_eq!((width, height), (128, 128), "the window is 128 output rows tall");

        let mean = pixels
            .chunks_exact(4)
            .map(|pixel| u32::from(pixel[0]))
            .sum::<u32>()
            / (pixels.len() as u32 / 4);
        assert_eq!(mean, 0xFF, "the window is the top 256 rows, which are uniformly white");
    }
}
