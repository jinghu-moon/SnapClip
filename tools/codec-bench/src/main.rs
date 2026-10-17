//! Measures PNG, JPEG and WebP encoders on synthetic screenshot material (docs/34 §10).
//!
//! Run with `cargo run -p codec-bench --release -- --report`. The first question it answers is the
//! one the documentation could not: whether `zenjpeg`'s `BytesEncoder::push` really streams without
//! buffering blocks, which decides whether long scroll captures keep the Huffman and trellis
//! optimisations. The `verdict` section prints the three sizes that answer it — same image, same
//! quality, only the band size and the optimisation switches differ.

mod alloc_probe;
mod codecs;
mod synthetic;

use std::time::Instant;

use codecs::{JpegOptions, Outcome};

#[global_allocator]
static ALLOCATOR: alloc_probe::Counting = alloc_probe::Counting;

fn main() {
    if !std::env::args().any(|argument| argument == "--report") {
        println!("codec-bench: pass --report to print the table");
        return;
    }
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
