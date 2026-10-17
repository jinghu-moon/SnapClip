//! Measures PNG, JPEG and WebP encoders on synthetic screenshot material (docs/34 §10, §11).
//!
//! Two modes:
//!
//! * `--report` answers the size/time/memory question and, with it, whether `zenjpeg`'s
//!   `BytesEncoder::push` really streams without buffering blocks — which decides whether long
//!   scroll captures keep the Huffman and trellis optimisations.
//! * `--quality` answers the quality question on the same material: how much 4:2:0 costs, whether
//!   q92 is enough, and whether turning the optimisations on changes the decoded pixels at all.
//!   It writes a zoomed crop of one text region so the numbers can be checked by eye.

mod alloc_probe;
mod codecs;
mod huffman_ab;
mod quality;
mod scroll;
mod synthetic;
mod transcode;

use std::time::Instant;

use codecs::{JpegOptions, Outcome};
use synthetic::TextRegion;

#[global_allocator]
static ALLOCATOR: alloc_probe::Counting = alloc_probe::Counting;

/// Rows per `push` for the quality run. Banding does not change the output (docs/34 §10), so this
/// is the streaming shape the product will actually use rather than one whole-image call.
const QUALITY_BAND_ROWS: usize = 64;

/// Bits of zoom in the text-crop dump: enough to see 4:2:0 smear a one pixel stroke.
const ZOOM: usize = 8;

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.iter().any(|argument| argument == "--scroll") {
        println!("{}", scroll::report());
        return;
    }
    if arguments.iter().any(|argument| argument == "--transcode") {
        println!("{}", transcode::report());
        return;
    }
    if arguments.iter().any(|argument| argument == "--huffman-ab") {
        println!("{}", huffman_ab::report());
        return;
    }
    if arguments.iter().any(|argument| argument == "--quality") {
        run_quality();
        return;
    }
    if !arguments.iter().any(|argument| argument == "--report") {
        println!("codec-bench: pass --report, --quality, --scroll, --transcode or --huffman-ab");
        return;
    }
    run_report();
}

fn run_report() {
    let mut rows: Vec<(&'static str, String, Outcome)> = Vec::new();
    let mut verdict: Vec<(String, Outcome)> = Vec::new();

    for asset in synthetic::corpus() {
        let iterations = iterations_for(asset.width * asset.height);
        let full = JpegOptions {
            quality: 92,
            subsample_444: true,
            trellis: true,
            optimize_huffman: true,
            band_rows: 0,
        };
        let banded = JpegOptions { band_rows: 64, ..full };
        let plain = JpegOptions { trellis: false, optimize_huffman: false, ..full };

        let (width, height, pixels) = (asset.width, asset.height, &asset.pixels);
        let candidates: Vec<(&str, Box<dyn Fn() -> Result<usize, String> + '_>)> = vec![
            (
                "png row-band",
                Box::new(move || codecs::png_row_band(pixels, width, height).map(|bytes| bytes.len())),
            ),
            (
                "zenjpeg 444 trellis+huffman",
                Box::new(move || codecs::zenjpeg(pixels, width, height, full).map(|bytes| bytes.len())),
            ),
            (
                "zenjpeg 444 trellis+huffman 64r",
                Box::new(move || codecs::zenjpeg(pixels, width, height, banded).map(|bytes| bytes.len())),
            ),
            (
                "zenjpeg 444 plain",
                Box::new(move || codecs::zenjpeg(pixels, width, height, plain).map(|bytes| bytes.len())),
            ),
            (
                "mozjpeg 444 trellis",
                Box::new(move || codecs::mozjpeg(pixels, width, height, full).map(|bytes| bytes.len())),
            ),
        ];
        // WebP is only legal up to 16383 per side, and the tall asset is far past it.
        let mut candidates = candidates;
        if width <= 16_383 && height <= 16_383 {
            candidates.push((
                "zenwebp lossless",
                Box::new(move || codecs::zenwebp_lossless(pixels, width, height).map(|bytes| bytes.len())),
            ));
        }

        for (label, encode) in candidates {
            match measure(iterations, || encode()) {
                Ok(outcome) => {
                    if asset.name == "scroll-tall" && label.starts_with("zenjpeg") {
                        verdict.push((label.to_string(), outcome.clone()));
                    }
                    rows.push((asset.name, label.to_string(), outcome));
                }
                Err(error) => {
                    eprintln!("codec-bench: {} / {label}: {error}", asset.name);
                    rows.push((
                        asset.name,
                        label.to_string(),
                        Outcome { bytes: 0, millis: f64::NAN, peak_extra: usize::MAX },
                    ));
                }
            }
        }
    }

    print_table(&rows);
    print_verdict(&verdict);
}

/// One JPEG variant of the quality matrix: everything that decides its pixels and size.
struct Variant {
    label: &'static str,
    quality: u8,
    subsample_444: bool,
    trellis: bool,
    optimize_huffman: bool,
}

fn variants() -> Vec<Variant> {
    vec![
        Variant { label: "q92 444 plain", quality: 92, subsample_444: true, trellis: false, optimize_huffman: false },
        Variant { label: "q95 444 plain", quality: 95, subsample_444: true, trellis: false, optimize_huffman: false },
        Variant { label: "q85 444 plain", quality: 85, subsample_444: true, trellis: false, optimize_huffman: false },
        Variant { label: "q92 420 plain", quality: 92, subsample_444: false, trellis: false, optimize_huffman: false },
        // Same quantisation tables as plain: only the entropy coder changes, so the decoded pixels
        // must be identical and only the size may move.
        Variant { label: "q92 444 huffman only", quality: 92, subsample_444: true, trellis: false, optimize_huffman: true },
        Variant { label: "q92 444 preset(trellis)", quality: 92, subsample_444: true, trellis: true, optimize_huffman: true },
    ]
}

/// A decoded variant, reused by the table and the crop dump.
struct Decoded {
    bytes: usize,
    pixels: Vec<u8>,
}

fn run_quality() {
    println!("\nquality — JPEG decoded and compared against the lossless PNG of the same asset");
    println!("text regions: {ZOOM}x crop of the first one is written to target/jpeg_text_zoom.png");
    println!(
        "\n{:<10} {:<24} {:>10} {:>9} {:>9} {:>10} {:>10}",
        "asset", "variant", "bytes", "ssim", "psnr", "text ssim", "text psnr"
    );
    println!("{}", "-".repeat(86));

    // The tall scroll strip is left out: its quality question is the same as ui-4k's, and a
    // 38-megapixel decode twice per variant would only slow the run down.
    for asset in synthetic::corpus().into_iter().filter(|asset| asset.height <= 4_000) {
        let png = match codecs::png_row_band(&asset.pixels, asset.width, asset.height) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("codec-bench: {} / png: {error}", asset.name);
                continue;
            }
        };
        let (width, height, reference) = match codecs::decode_png(&png) {
            Ok(decoded) => decoded,
            Err(error) => {
                eprintln!("codec-bench: {} / png decode: {error}", asset.name);
                continue;
            }
        };
        if (width, height) != (asset.width, asset.height) {
            eprintln!("codec-bench: {} / png size mismatch", asset.name);
            continue;
        }
        let mut plain_444: Option<Decoded> = None;

        for variant in variants() {
            let encoded = match codecs::zenjpeg_knobs(
                &asset.pixels,
                asset.width,
                asset.height,
                variant.quality,
                variant.subsample_444,
                variant.trellis,
                variant.optimize_huffman,
                QUALITY_BAND_ROWS,
            ) {
                Ok(bytes) => bytes,
                Err(error) => {
                    eprintln!("codec-bench: {} / {}: {error}", asset.name, variant.label);
                    continue;
                }
            };
            let (jpeg_width, jpeg_height, pixels) = match codecs::decode_jpeg(&encoded) {
                Ok(decoded) => decoded,
                Err(error) => {
                    eprintln!("codec-bench: {} / {} decode: {error}", asset.name, variant.label);
                    continue;
                }
            };
            if (jpeg_width, jpeg_height) != (width, height) {
                eprintln!("codec-bench: {} / {} decoded to {}x{}", asset.name, variant.label, jpeg_width, jpeg_height);
                continue;
            }
            let all = quality::whole(&reference, &pixels, width, height);
            let text = quality::in_regions(&reference, &pixels, width, height, &asset.text_regions);
            println!(
                "{:<10} {:<24} {:>10} {:>9.4} {:>9.2} {:>10.4} {:>10.2}",
                asset.name, variant.label, encoded.len(), all.ssim, all.psnr, text.ssim, text.psnr
            );
            let decoded = Decoded { bytes: encoded.len(), pixels };
            if variant.label == "q92 444 plain" {
                plain_444 = Some(decoded);
            }
        }

        // The clean A/B this section exists for: plain and huffman-only use the same quantisation
        // tables, so their decoded pixels must match exactly. Anything else means the entropy coder
        // changed the picture, which it must not.
        if let Some(plain) = plain_444.as_ref() {
            if let Some(huffman) = huffman_only(&asset, &reference) {
                let identical = plain.pixels == huffman.pixels;
                println!(
                    "  {:<8} plain vs huffman-only: {} bytes vs {} bytes, pixels {}",
                    asset.name,
                    plain.bytes,
                    huffman.bytes,
                    if identical { "IDENTICAL" } else { "DIFFER" }
                );
            }
        }
    }

    write_text_crop();
    print_quality_verdict();
}

/// Re-encodes one asset with huffman only, for the pixel-identity check printed above.
fn huffman_only(asset: &synthetic::Asset, reference: &[u8]) -> Option<Decoded> {
    let encoded = codecs::zenjpeg_knobs(
        &asset.pixels,
        asset.width,
        asset.height,
        92,
        true,
        false,
        true,
        QUALITY_BAND_ROWS,
    )
    .ok()?;
    let (_, _, pixels) = codecs::decode_jpeg(&encoded).ok()?;
    if pixels.len() != reference.len() {
        return None;
    }
    Some(Decoded { bytes: encoded.len(), pixels })
}

/// Writes PNG / q92 4:4:4 / q92 4:2:0 crops of the first text region of `ui-1080`, side by side at
/// `ZOOM`, so 4:2:0's smear can be seen instead of only scored.
fn write_text_crop() {
    let Some(asset) = synthetic::corpus().into_iter().find(|asset| asset.name == "ui-1080") else {
        return;
    };
    let Some(region) = asset.text_regions.first().copied() else {
        return;
    };
    // A line box spans the panel's whole width; the dump takes a readable slice of it at 8x.
    const SLICE: usize = 160;
    let region = TextRegion { x1: (region.x0 + SLICE).min(region.x1), ..region };
    let Some(png) = codecs::png_row_band(&asset.pixels, asset.width, asset.height).ok() else {
        return;
    };
    let Some((width, _height, reference)) = codecs::decode_png(&png).ok() else {
        return;
    };
    let panel = |subsample_444: bool| -> Option<Vec<u8>> {
        let encoded = codecs::zenjpeg_knobs(
            &asset.pixels,
            asset.width,
            asset.height,
            92,
            subsample_444,
            false,
            false,
            QUALITY_BAND_ROWS,
        )
        .ok()?;
        let (_, _, pixels) = codecs::decode_jpeg(&encoded).ok()?;
        Some(pixels)
    };
    let Some(full_444) = panel(true) else { return };
    let Some(quarter) = panel(false) else { return };

    let (truth, crop_width, crop_height) = quality::crop(&reference, width, region);
    if crop_width == 0 || crop_height == 0 {
        return;
    }
    let (with_444, _, _) = quality::crop(&full_444, width, region);
    let (with_420, _, _) = quality::crop(&quarter, width, region);

    const GAP: usize = 8;
    let panels: [&Vec<u8>; 3] = [&truth, &with_444, &with_420];
    let strip_width = (crop_width * ZOOM * 3) + GAP * 2;
    let strip_height = crop_height * ZOOM;
    let mut strip = vec![255u8; strip_width * strip_height * 4];
    for (panel_index, panel) in panels.iter().enumerate() {
        let offset_x = panel_index * (crop_width * ZOOM + GAP);
        for y in 0..crop_height * ZOOM {
            for x in 0..crop_width * ZOOM {
                let source_x = x / ZOOM;
                let source_y = y / ZOOM;
                let source = (source_y * crop_width + source_x) * 4;
                let target = (y * strip_width + offset_x + x) * 4;
                strip[target..target + 4].copy_from_slice(&panel[source..source + 4]);
            }
        }
    }

    let path = std::path::Path::new("target/jpeg_text_zoom.png");
    let result = (|| -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|error| format!("create: {error}"))?;
        let writer = std::io::BufWriter::new(file);
        let mut encoder = png::Encoder::new(writer, strip_width as u32, strip_height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|error| format!("header: {error}"))?;
        writer.write_image_data(&strip).map_err(|error| format!("data: {error}"))
    })();
    match result {
        Ok(()) => println!(
            "\nwrote {} — PNG | q92 4:4:4 | q92 4:2:0, {crop_width}x{crop_height} at {ZOOM}x \
             (text region at {},{} of ui-1080, {}x{} total)",
            path.display(),
            region.x0,
            region.y0,
            asset.width,
            asset.height
        ),
        Err(error) => eprintln!("codec-bench: could not write the crop: {error}"),
    }
}

fn print_quality_verdict() {
    println!(
        "\nread it like this:\n\
         * `text ssim` is the number that matters — whole-image SSIM stays high because a UI is\n\
           mostly flat surfaces that survive compression almost perfectly.\n\
         * 4:2:0 against 4:4:4 at the same quality shows what chroma subsampling costs on coloured\n\
           text and thin strokes.\n\
         * `huffman only` must decode to the same pixels as `plain`: the entropy coder changes\n\
           bytes, not the picture.
         * `preset(trellis)` is *not* a controlled A/B: the crate keeps the trellis knob private, so
           the only way in is the mozjpeg preset, which also swaps the quantisation tables."
    );
}

fn iterations_for(pixels: usize) -> usize {
    if pixels > 8_000_000 { 1 } else { 3 }
}

/// Runs `encode` a few times and reports the median time and the largest peak it saw.
fn measure(iterations: usize, mut encode: impl FnMut() -> Result<usize, String>) -> Result<Outcome, String> {
    let mut times = Vec::with_capacity(iterations);
    let mut peak = 0usize;
    let mut bytes = 0usize;
    for _ in 0..iterations {
        let started = Instant::now();
        // An encoder that panics must show up as a row, not take the whole run down: one of the
        // candidates (mozjpeg-rs 0.9.2) does exactly that on this material.
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            alloc_probe::peak_extra(|| encode())
        }));
        let elapsed = started.elapsed();
        let (result, extra) = caught.map_err(|_| "panicked".to_string())?;
        bytes = result?;
        peak = peak.max(extra);
        times.push(elapsed.as_secs_f64() * 1000.0);
    }
    times.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let middle = times.len() / 2;
    let millis = if times.len() > 1 && times.len() % 2 == 0 {
        (times[middle - 1] + times[middle]) / 2.0
    } else {
        times[middle]
    };
    Ok(Outcome { bytes, millis, peak_extra: peak })
}

fn megabytes(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn print_table(rows: &[(&'static str, String, Outcome)]) {
    println!(
        "\n{:<12} {:<32} {:>11} {:>10} {:>11}",
        "asset", "encoder", "bytes", "median ms", "peak +MB"
    );
    println!("{}", "-".repeat(80));
    for (asset, label, outcome) in rows {
        if outcome.peak_extra == usize::MAX {
            println!("{asset:<12} {label:<32} {:>11}", "failed");
            continue;
        }
        println!(
            "{:<12} {:<32} {:>11} {:>10.1} {:>11.1}",
            asset,
            label,
            outcome.bytes,
            outcome.millis,
            megabytes(outcome.peak_extra)
        );
    }
}

fn print_verdict(rows: &[(String, Outcome)]) {
    println!("\nverdict — scroll-tall (1280x30000), zenjpeg, quality 92, 4:4:4");
    println!("{}", "-".repeat(80));
    for (label, outcome) in rows {
        println!(
            "{:<40} {:>11} bytes  {:>7.0} ms  peak {:>6.1} MB",
            label,
            outcome.bytes,
            outcome.millis,
            megabytes(outcome.peak_extra)
        );
    }
    println!(
        "\n64 rows matching the whole-buffer size means banding does not change the output.\n\
         `plain` matching `trellis+huffman` means the streaming path silently drops them,\n\
         which is the answer this run exists to produce."
    );
}
