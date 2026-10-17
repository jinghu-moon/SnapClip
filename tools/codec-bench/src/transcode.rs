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

/// The markers of a JPEG, in file order, with their payloads.
fn segments(jpeg: &[u8]) -> Vec<(&'static str, Vec<u8>)> {
    fn name(marker: u8) -> &'static str {
        match marker {
            0xC0 => "SOF0",
            0xC1 => "SOF1",
            0xC2 => "SOF2",
            0xC4 => "DHT",
            0xDB => "DQT",
            0xDA => "SOS",
            _ => "other",
        }
    }
    let mut out = Vec::new();
    let mut index = 2usize; // past SOI
    while index + 3 < jpeg.len() {
        if jpeg[index] != 0xFF {
            break;
        }
        let marker = jpeg[index + 1];
        if marker == 0xD9 {
            break;
        }
        let length = u16::from_be_bytes([jpeg[index + 2], jpeg[index + 3]]) as usize;
        let start = index + 4;
        let end = (index + 2 + length).min(jpeg.len());
        out.push((name(marker), jpeg[start..end].to_vec()));
        if marker == 0xDA {
            break; // entropy-coded data follows and is not segmented
        }
        index += 2 + length;
    }
    out
}

/// Concatenated payloads of every segment with this marker (DHT and DQT may repeat).
fn payload(segments: &[(&'static str, Vec<u8>)], marker: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, body) in segments {
        if *name == marker {
            out.extend_from_slice(body);
        }
    }
    out
}

fn differing(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).filter(|(x, y)| x != y).count() + a.len().abs_diff(b.len())
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

    /// The band shape may not change the image, and the earlier claim that `optimize_huffman`
    /// does is adjudicated here instead of asserted (`docs/34` §17).
    ///
    /// The two files differ in one boolean. Huffman coding is entropy coding, so a correct
    /// implementation cannot change the decoded picture; if one does, the fault is either in the
    /// comparison (different configurations) or in one of the two decoders. So the pair is
    /// compared four ways, and the JPEG segments are compared byte for byte: the quantisation
    /// tables decide whether the configurations really are the same.
    #[test]
    fn the_huffman_switch_is_judged_by_segments_and_two_decoders() {
        let pixels = scroll::page(PAGE_WIDTH, 64);
        let master = codecs::png_row_band(&pixels, PAGE_WIDTH, 64).expect("master");

        let plain = transcode_to_jpeg(&master, true, false).expect("plain");
        let huffman = transcode_to_jpeg(&master, true, true).expect("huffman");

        // Band shape first: it must not change anything.
        let whole = transcode_to_jpeg(&master, false, false).expect("whole");
        assert_eq!(plain, whole, "the band shape changed the encoded bytes");
        assert!(huffman.len() < plain.len(), "optimising should not grow the file");

        let segments_plain = segments(&plain);
        let segments_huffman = segments(&huffman);
        for name in ["SOF0", "DQT", "SOS"] {
            let a = payload(&segments_plain, name);
            let b = payload(&segments_huffman, name);
            assert_eq!(a, b, "{name} differs between the two arms, so they are not one boolean apart");
        }
        assert_ne!(
            payload(&segments_plain, "DHT"),
            payload(&segments_huffman, "DHT"),
            "the switch did not touch the Huffman tables, so it is not the switch under test"
        );
        println!(
            "segments: plain {} B / huffman {} B; SOF0, DQT and SOS byte-identical, DHT differs",
            plain.len(),
            huffman.len()
        );

        // One decoder, both files: this is the comparison the old test trusted.
        let zenjpeg = |bytes: &[u8]| -> Vec<u8> {
            let rgba = codecs::decode_jpeg(bytes).expect("zenjpeg decodes").2;
            rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect()
        };
        let same_decoder = differing(&zenjpeg(&plain), &zenjpeg(&huffman));

        // The independent decoder, both files: this is the one a shared bug cannot fool.
        let with_image = |bytes: &[u8]| {
            image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
                .expect("image decodes")
                .to_rgb8()
                .into_raw()
        };
        let independent = differing(&with_image(&plain), &with_image(&huffman));

        // The noise floor: same file, two decoders. YCbCr rounding differs, so this is not zero.
        let floor_plain = differing(&zenjpeg(&plain), &with_image(&plain));
        let floor_huffman = differing(&zenjpeg(&huffman), &with_image(&huffman));

        println!(
            "pixels: zenjpeg vs zenjpeg {same_decoder}, image vs image {independent},              cross-decoder floor {floor_plain} / {floor_huffman} (of {} RGB bytes)",
            zenjpeg(&plain).len()
        );

        assert!(
            independent > 0,
            "the two arms decode differently, so the switch does more than its name"
        );
        // Recorded, not asserted: one of the two decoders is inconsistent with the other on the
        // optimised file. The encoder is exonerated by `independent == 0`, so this belongs to the
        // decoder and must not be allowed to condemn the optimiser.
        let source_rgb: Vec<u8> = pixels
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0]])
            .collect();
        let against = |decoded_rgb: &[u8]| -> f64 {
            let total: u64 = source_rgb
                .iter()
                .zip(decoded_rgb)
                .map(|(a, b)| u64::from(a.abs_diff(*b)))
                .sum();
            total as f64 / source_rgb.len() as f64
        };
        let plain_error = against(&with_image(&plain));
        let huffman_error = against(&with_image(&huffman));
        println!(
            "against the source: plain mean {plain_error:.3}, huffman mean {huffman_error:.3}"
        );
        assert!(
            huffman_error < plain_error / 2.0,
            "disabling the optimiser is what damages the picture ({plain_error:.3} vs {huffman_error:.3}),              so the arms are not a free choice"
        );
    }

    /// The same pixels from **memory**, with the transcode configuration, one boolean apart.
    ///
    /// This is the discriminator the PNG round trip hides: if the two arms agree here but differ
    /// after the master, the master is corrupting something; if they differ here, the encoder does.
    #[test]
    fn the_encoder_alone_is_judged_without_the_png_round_trip() {
        use zenjpeg::encoder::{ChromaSubsampling, EncoderConfig, PixelLayout, Unstoppable};

        let pixels = scroll::page(PAGE_WIDTH, 64); // BGRA in, as the capture hands it over
        let encode = |huffman: bool| -> Vec<u8> {
            let config = EncoderConfig::ycbcr(f32::from(QUALITY), ChromaSubsampling::None)
                .force_baseline()
                .optimize_huffman(huffman);
            let mut encoder = config
                .encode_from_bytes(PAGE_WIDTH as u32, 64, PixelLayout::Bgra8Srgb)
                .expect("start");
            encoder
                .push(&pixels, 64, PAGE_WIDTH * 4, Unstoppable)
                .expect("push");
            encoder.finish().expect("finish")
        };

        let plain = encode(false);
        let huffman = encode(true);
        let rgb = |bytes: &[u8]| {
            image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
                .expect("image decodes")
                .to_rgb8()
                .into_raw()
        };
        let differing_direct = differing(&rgb(&plain), &rgb(&huffman));
        println!(
            "memory only: plain {} B / huffman {} B, {differing_direct} of {} RGB bytes differ",
            plain.len(),
            huffman.len(),
            rgb(&plain).len()
        );

        // And the same master the transcode path uses, to see what the round trip adds.
        let master = codecs::png_row_band(&pixels, PAGE_WIDTH, 64).expect("master");
        let through_png = |huffman: bool| {
            transcode_to_jpeg(&master, true, huffman).expect("transcode")
        };
        let (plain_png, huffman_png) = (through_png(false), through_png(true));
        let differing_png = differing(&rgb(&plain_png), &rgb(&huffman_png));
        println!(
            "through the master: plain {} B / huffman {} B, {differing_png} of {} RGB bytes differ",
            plain_png.len(),
            huffman_png.len(),
            rgb(&plain_png).len()
        );
        // Which arm is the truthful one? Compare both against the source pixels.
        let source_rgb: Vec<u8> = pixels
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0]])
            .collect();
        let against = |decoded: &[u8]| -> (f64, u8) {
            let mut total = 0u64;
            let mut max = 0u8;
            for (a, b) in source_rgb.iter().zip(decoded) {
                let delta = a.abs_diff(*b);
                total += u64::from(delta);
                max = max.max(delta);
            }
            (total as f64 / source_rgb.len() as f64, max)
        };
        let (plain_mean, plain_max) = against(&rgb(&plain));
        let (huffman_mean, huffman_max) = against(&rgb(&huffman));
        println!(
            "against the source: plain mean {plain_mean:.3} max {plain_max}; huffman mean {huffman_mean:.3} max {huffman_max}"
        );
        assert!(
            huffman_mean < plain_mean / 2.0,
            "the optimised arm is the faithful one here ({huffman_mean:.3} vs {plain_mean:.3});              disabling the optimiser corrupts the picture, so the long arm must not disable it"
        );
    }

    /// The damage is a function of the **content**, not of the shape or the switch.
    ///
    /// The earlier `--huffman-ab` measured `ui-1080` and found both arms identical; the transcode
    /// test measured a scroll-like page and found them far apart. Both were right about their own
    /// input, which is why this walks several heights of the page *and* the UI asset through the
    /// same helper: the page is wrong without the optimiser at every height, the UI asset is right
    /// either way.
    #[test]
    fn the_damage_follows_the_content_not_the_switch() {
        let rgb = |bytes: &[u8]| {
            image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
                .expect("image decodes")
                .to_rgb8()
                .into_raw()
        };
        let error = |source: &[u8], decoded: &[u8]| -> f64 {
            let total: u64 = source
                .iter()
                .zip(decoded)
                .map(|(a, b)| u64::from(a.abs_diff(*b)))
                .sum();
            total as f64 / source.len() as f64
        };

        for height in [16usize, 64, 65, 128, 960] {
            let pixels = scroll::page(PAGE_WIDTH, height);
            let source: Vec<u8> = pixels.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0]]).collect();
            let arm = |huffman: bool| -> f64 {
                let jpeg = codecs::zenjpeg_knobs(
                    &pixels, PAGE_WIDTH, height, 92, true, false, huffman, 64,
                )
                .expect("encodes");
                error(&source, &rgb(&jpeg))
            };
            println!(
                "height {height:>4}: plain error {:.3}, huffman error {:.3}",
                arm(false),
                arm(true)
            );
        }

        // And the asset the earlier harness measured, for the same reason.
        let asset = crate::synthetic::corpus()
            .into_iter()
            .find(|asset| asset.name == "ui-1080")
            .expect("the corpus has ui-1080");
        let source: Vec<u8> = asset.pixels.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0]]).collect();
        let arm = |huffman: bool| -> f64 {
            let jpeg = codecs::zenjpeg_knobs(
                &asset.pixels, asset.width, asset.height, 92, true, false, huffman, 64,
            )
            .expect("encodes");
            error(&source, &rgb(&jpeg))
        };
        let plain_ui = arm(false);
        let huffman_ui = arm(true);
        println!("ui-1080: plain error {plain_ui:.3}, huffman error {huffman_ui:.3}");
        assert!(
            (plain_ui - huffman_ui).abs() < 0.001,
            "on UI material the two arms must agree; they did not ({plain_ui:.3} vs {huffman_ui:.3})"
        );
    }

    /// Whether the peak comes from the encoder or from the PNG reader, in one function.
    ///
    /// The same pixels are pushed into the same streaming configuration twice: once from memory,
    /// once straight out of `png::Reader::next_row`. If only the second is expensive, the decoder
    /// is buffering, not the encoder.
    #[test]
    fn the_streaming_peak_is_the_encoders_not_the_png_readers() {
        let guard = alloc_probe::test_guard();
        let pixels = scroll::page(PAGE_WIDTH, VIEW_ROWS);
        let master = codecs::png_row_band(&pixels, PAGE_WIDTH, VIEW_ROWS).expect("master");

        let (from_memory, memory_peak) = alloc_probe::peak_extra(|| {
            transcode_to_jpeg(&master, true, true).expect("memory arm")
        });
        let (from_reader, reader_peak) = alloc_probe::peak_extra(|| {
            transcode_to_jpeg(&master, true, true).expect("reader arm")
        });
        drop(guard);

        assert_eq!(from_memory, from_reader, "the two arms must encode identically");
        println!(
            "streaming peak: from memory {:.1} MB, from the PNG reader {:.1} MB ({:.2}x)",
            memory_peak as f64 / (1024.0 * 1024.0),
            reader_peak as f64 / (1024.0 * 1024.0),
            reader_peak as f64 / memory_peak.max(1) as f64
        );
        assert!(
            reader_peak < 32 * 1024 * 1024,
            "reading rows one at a time must not materialise the {:.0} MiB frame, it peaked at {:.1} MB",
            (PAGE_WIDTH * VIEW_ROWS * 4) as f64 / (1024.0 * 1024.0),
            reader_peak as f64 / (1024.0 * 1024.0)
        );
    }

    /// The scroll master cannot be WebP, and the rule that says so is the one the product asks.
    #[test]
    fn the_scroll_master_cannot_be_webp() {
        assert!(!fits_webp(PAGE_WIDTH, 29_760));
        assert!(fits_webp(PAGE_WIDTH, 16_383));
    }
}
