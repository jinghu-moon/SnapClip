//! Saving from the PNG master, measured (`docs/34` §16).
//!
//! Capture writes one lossless PNG; every other format is a conversion of it at save time. The
//! question this answers is what that costs, and in particular whether a scroll capture's save
//! can avoid materialising the 153 MiB frame: the decoder is row based, and the JPEG sink streams
//! band by band above its budget, so the two arms differ by an order of magnitude in peak memory.
//!
//! The numbers are taken around the whole conversion — open the master, decode it, encode it,
//! write it — because that is what a save actually pays for.

use std::time::Instant;

use crate::alloc_probe;
use crate::codecs;
use crate::scroll;

/// The page a scroll capture ends up holding: the same width and row count `--scroll` stitches.
const PAGE_WIDTH: usize = 1280;
const LONG_ROWS: usize = 29_760;
/// A viewport, for the short-format measurements.
const VIEW_ROWS: usize = 960;

/// Rows per `push` when transcoding, the same band the product uses for a long save.
const TRANSCODE_BAND_ROWS: usize = 64;

/// JPEG settings `docs/34` §11 settled on.
const QUALITY: u8 = 92;

struct Measured {
    bytes: usize,
    millis: f64,
    peak: usize,
}

impl Measured {
    fn line(&self, label: &str) -> String {
        format!(
            "  {label:<34} {:>12} B  {:>9.1} ms  {:>9.1} MB peak",
            self.bytes,
            self.millis,
            self.peak as f64 / (1024.0 * 1024.0)
        )
    }
}

pub fn report() -> String {
    let mut out = String::new();
    out.push_str("── transcode (PNG master → save format, docs/34 §16) ────────────────\n");

    // The master the rest of the run converts, built before any measurement so the page itself
    // is not counted as a conversion cost.
    let page = scroll::page(PAGE_WIDTH, LONG_ROWS);
    let (master_bytes, master_peak) = alloc_probe::peak_extra(|| {
        codecs::png_row_band(&page, PAGE_WIDTH, LONG_ROWS).expect("the page encodes as PNG")
    });
    out.push_str(&format!(
        "master: {PAGE_WIDTH}x{LONG_ROWS} PNG, {} B, {} MB peak
",
        master_bytes.len(),
        master_peak / (1024 * 1024)
    ));

    // 2. PNG → PNG is a copy.
    let copy = measure(|| {
        let path = temp_path("copy-in");
        std::fs::write(&path, &master_bytes).expect("master");
        let destination = temp_path("copy-out");
        let bytes = std::fs::copy(&path, &destination).expect("copy");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&destination);
        Ok(bytes as usize)
    });
    out.push_str(&copy.line("save as PNG (file copy)"));

    // 3. The same master as JPEG, streamed band by band.
    let streamed = measure(|| Ok(transcode_to_jpeg(&master_bytes, true, false)?.len()));
    out.push_str(&streamed.line("save as JPG (row-by-row, streamed)"));

    // 4. And as JPEG from a whole frame, which is what decoding first would cost.
    let buffered = measure(|| Ok(transcode_to_jpeg(&master_bytes, false, true)?.len()));
    out.push_str(&buffered.line("save as JPG (whole frame, Huffman)"));

    // 5. The decode on its own, so the transcode cost can be attributed.
    let streamed_decode = measure(|| {
        let (width, height, rows) = decode_rows(&master_bytes, 1)?;
        let _ = (width, rows);
        Ok(height)
    });
    out.push_str(&streamed_decode.line("PNG decode only (row by row)"));
    let whole_decode = measure(|| {
        let (width, height, pixels) = codecs::decode_png(&master_bytes)?;
        let _ = width;
        let _ = pixels;
        Ok(height)
    });
    out.push_str(&whole_decode.line("PNG decode only (whole frame)"));

    // 6. A normal screenshot as lossless WebP, and the scroll master refused before decoding.
    let small = codecs::png_row_band(&scroll::page(PAGE_WIDTH, VIEW_ROWS), PAGE_WIDTH, VIEW_ROWS)
        .expect("a viewport master");
    let webp = measure(|| {
        let (width, height, rgba) = codecs::decode_png(&small)?;
        Ok(codecs::zenwebp_lossless(&rgba_to_bgra(&rgba), width, height)?.len())
    });
    out.push_str(&webp.line("save as WebP (lossless, viewport)"));

    let refused = measure(|| {
        // The same rule the product asks before it starts scrolling: the format cannot hold the
        // rows, so nothing is decoded at all.
        if !fits_webp(PAGE_WIDTH, LONG_ROWS) {
            return Ok(0);
        }
        Err("the limit did not apply".to_string())
    });
    out.push_str(&refused.line("save scroll master as WebP (refused)"));

    // 7. The verdict this mode exists for.
    let frame_bytes = PAGE_WIDTH * LONG_ROWS * 4;
    out.push_str(&format!(
        "verdict: {} MB frame decoded whole vs {:.1} MB streamed ({:.1}x less); save the scroll \
         capture by streaming it, never by decoding it first\n",
        frame_bytes / (1024 * 1024),
        streamed.peak as f64 / (1024.0 * 1024.0),
        frame_bytes as f64 / streamed.peak.max(1) as f64
    ));
    out
}

/// Decodes the master and encodes it as JPEG, with the rows either streamed or gathered first.
fn transcode_to_jpeg(master: &[u8], streamed: bool, huffman: bool) -> Result<Vec<u8>, String> {
    use zenjpeg::encoder::{
        ChromaSubsampling, EncoderConfig, PixelLayout, Unstoppable,
    };

    let mut decoder = png::Decoder::new(std::io::Cursor::new(master));
    decoder.set_transformations(png::Transformations::IDENTITY);
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("png header: {error}"))?;
    let (width, height) = (reader.info().width as usize, reader.info().height as usize);

    let config = EncoderConfig::ycbcr(f32::from(QUALITY), ChromaSubsampling::None)
        .force_baseline()
        .optimize_huffman(huffman);
    let mut encoder = config
        .encode_from_bytes(width as u32, height as u32, PixelLayout::Bgra8Srgb)
        .map_err(|error| format!("zenjpeg start: {error}"))?;

    let stride = width * 4;
    // Only the buffered arm holds the frame: allocating it unconditionally is what made the
    // streamed arm look like it needed 242 MB when the encoder itself needs 14.
    let mut gathered: Vec<u8> = if streamed {
        Vec::new()
    } else {
        Vec::with_capacity(stride * height)
    };
    let mut band: Vec<u8> = Vec::with_capacity(stride * TRANSCODE_BAND_ROWS);
    let mut rows = 0usize;
    loop {
        let next = reader
            .next_row()
            .map_err(|error| format!("png row: {error}"))?;
        let Some(source) = next else { break };
        let mut row = vec![0u8; stride];
        for (source, target) in source.data().chunks_exact(4).zip(row.chunks_exact_mut(4)) {
            target[0] = source[2];
            target[1] = source[1];
            target[2] = source[0];
            target[3] = source[3];
        }
        if streamed {
            band.extend_from_slice(&row);
            rows += 1;
            if rows % TRANSCODE_BAND_ROWS == 0 || rows == height {
                let count = band.len() / stride;
                encoder
                    .push(&band, count, stride, Unstoppable)
                    .map_err(|error| format!("zenjpeg push: {error}"))?;
                band.clear();
            }
        } else {
            gathered.extend_from_slice(&row);
        }
    }
    if !streamed {
        encoder
            .push(&gathered, height, stride, Unstoppable)
            .map_err(|error| format!("zenjpeg push: {error}"))?;
    }
    encoder
        .finish()
        .map_err(|error| format!("zenjpeg finish: {error}"))
}

/// Decodes the master a row at a time and returns how many rows it saw.
fn decode_rows(master: &[u8], step: usize) -> Result<(usize, usize, usize), String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(master));
    decoder.set_transformations(png::Transformations::IDENTITY);
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("png header: {error}"))?;
    let (width, height) = (reader.info().width as usize, reader.info().height as usize);
    let stride = width * 4;
    let mut buffer = vec![0u8; stride];
    let mut rows = 0usize;
    while let Some(row) = reader
        .next_row()
        .map_err(|error| format!("png row: {error}"))?
    {
        let take = step.min(row.data().len()).min(buffer.len());
        buffer[..take].copy_from_slice(&row.data()[..take]);
        rows += 1;
    }
    Ok((width, height, rows))
}

/// The format rule WebP has to obey, spelled out here so the bench does not import the shell.
fn fits_webp(width: usize, height: usize) -> bool {
    width <= 16_383 && height <= 16_383
}

fn rgba_to_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for pixel in rgba.chunks_exact(4) {
        out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
    }
    out
}

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("snapclip-transcode-{}-{name}.png", std::process::id()))
}

fn measure(body: impl FnOnce() -> Result<usize, String>) -> Measured {
    let (result, peak) = alloc_probe::peak_extra(|| {
        let started = Instant::now();
        let bytes = body().expect("the measurement body succeeds");
        (bytes, started.elapsed().as_secs_f64() * 1000.0)
    });
    Measured {
        bytes: result.0,
        millis: result.1,
        peak,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small master really converts, and the search for a reason it would not is one decode.
    #[test]
    fn a_small_master_transcodes_to_a_readable_jpeg() {
        let pixels = scroll::page(PAGE_WIDTH, 64);
        let master = codecs::png_row_band(&pixels, PAGE_WIDTH, 64).expect("master");
        for streamed in [true, false] {
            let jpeg = transcode_to_jpeg(&master, streamed, false).expect("jpeg");
            let (width, height, _) = codecs::decode_jpeg(&jpeg).expect("the jpeg decodes");
            assert_eq!((width, height), (PAGE_WIDTH, 64), "streamed = {streamed}");
        }
    }

    /// Which variable changes the pixels: the band shape, or `optimize_huffman`?
    #[test]
    fn the_huffman_switch_changes_the_picture_and_the_banding_does_not() {
        let pixels = scroll::page(PAGE_WIDTH, 64);
        let master = codecs::png_row_band(&pixels, PAGE_WIDTH, 64).expect("master");
        let decode = |bytes: &[u8]| codecs::decode_jpeg(bytes).expect("decodes").2;

        let banded = transcode_to_jpeg(&master, true, false).expect("banded");
        let whole = transcode_to_jpeg(&master, false, false).expect("whole");
        let banded_huffman = transcode_to_jpeg(&master, true, true).expect("banded huffman");

        let banded_pixels = decode(&banded);
        let whole_pixels = decode(&whole);
        let differing = banded_pixels
            .iter()
            .zip(&whole_pixels)
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(
            differing, 0,
            "band shape is not allowed to change the image (banded {} B vs whole {} B)",
            banded.len(),
            whole.len()
        );

        let huffman_pixels = decode(&banded_huffman);
        let differing = banded_pixels
            .iter()
            .zip(&huffman_pixels)
            .filter(|(a, b)| a != b)
            .count();
        // Measured, not assumed: in 0.8.4 this switch changes the decoded picture. Huffman
        // coding is entropy coding, so a pure implementation could not, which makes the flag a
        // quality knob in this version and not the "free 5 to 9 percent" the earlier run reported.
        // Recorded here so nobody re-enables it believing the pixels are untouched.
        println!(
            "note: optimize_huffman changes {differing} of {} RGBA bytes ({} B vs {} B)",
            banded_pixels.len(),
            banded.len(),
            banded_huffman.len()
        );
        assert!(
            differing > 0,
            "0.8.4 was measured to change pixels here; if that stops being true, the sink can              enable the optimiser again"
        );
    }

    /// The scroll master cannot be WebP, and the rule that says so is the one the product asks.
    #[test]
    fn the_scroll_master_cannot_be_webp() {
        assert!(!fits_webp(PAGE_WIDTH, 29_760));
        assert!(fits_webp(PAGE_WIDTH, 16_383));
    }
}
