//! The file path's assembly cost, measured the way the exporter runs it (`docs/35 §15`).
//!
//! A scroll session hands the writer row bands rather than a picture. This mode walks a synthetic
//! long image through the same shape the app's `write_bands` uses — four megabyte bands into a PNG
//! stream writer, straight to a file — and reports the peak extra allocation, so "the assembly is
//! 145 MB" has a counterweight from the banded path rather than a claim.
//!
//! Both runs write to a file on purpose: a `Vec` target would make the output buffer part of the
//! measurement and hide the very difference the mode exists to show.

use crate::alloc_probe;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

const WIDTH: u32 = 1280;
const ROWS: u32 = 29_760;

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("codec-bench-bands-{name}-{}.png", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

fn encode_bands(page: &[u8]) -> u64 {
    let row_bytes = WIDTH as usize * 4;
    let band_rows = ((4u32 << 20) / row_bytes as u32).max(1);
    let path = scratch("stream");
    let file = File::create(&path).expect("create");
    {
        // `png` owns its writer, so the file is moved in rather than borrowed.
        let mut encoder = png::Encoder::new(file, WIDTH, ROWS);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let writer = encoder.write_header().expect("png header");
        let mut stream = writer.into_stream_writer().expect("png stream");
        let mut first = 0u32;
        let mut band = Vec::new();
        while first < ROWS {
            let count = (ROWS - first).min(band_rows);
            band.clear();
            band.extend_from_slice(
                &page[first as usize * row_bytes..(first + count) as usize * row_bytes],
            );
            stream.write_all(&band).expect("band write");
            first += count;
        }
        stream.finish().expect("png finish");
    }
    let len = std::fs::metadata(&path).expect("stat").len();
    let _ = std::fs::remove_file(&path);
    len
}

fn encode_whole(page: &[u8]) -> u64 {
    // Owning the picture is the point: `SelectionPixels` hands the writer a whole buffer.
    let owned = page.to_vec();
    let path = scratch("whole");
    let file = File::create(&path).expect("create");
    {
        // `png` owns its writer, so the file is moved in rather than borrowed.
        let mut encoder = png::Encoder::new(file, WIDTH, ROWS);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(&owned).expect("one write");
        writer.finish().expect("png finish");
    }
    let len = std::fs::metadata(&path).expect("stat").len();
    let _ = std::fs::remove_file(&path);
    len
}

/// Both paths measured on the same long page, in bytes and milliseconds.
pub struct Bands {
    pub banded_bytes: u64,
    pub banded_ms: f64,
    pub banded_peak: usize,
    pub whole_bytes: u64,
    pub whole_ms: f64,
    pub whole_peak: usize,
}

/// The banded writer must stay within this fraction of the whole-picture peak (docs/35 §16).
pub const BANDED_PEAK_RATIO: f64 = 0.10;
/// And within this absolute ceiling, so a regression in the *whole* path cannot hide the ratio.
pub const BANDED_PEAK_CEILING: usize = 16 << 20;

pub fn compare() -> Bands {
    let page: Vec<u8> = (0..WIDTH as usize * ROWS as usize * 4)
        .map(|i| (i % 251) as u8)
        .collect();

    // The peak counter is process-wide, so hold the lock: a parallel test's allocations must not
    // be mistaken for ours (the same reason `webp_is_refused_before_it_can_allocate_a_long_image`
    // takes it).
    let _serial = alloc_probe::test_guard();
    let started = Instant::now();
    let (banded_bytes, banded_peak) = alloc_probe::peak_extra(|| encode_bands(&page));
    let banded_ms = started.elapsed().as_secs_f64() * 1e3;

    let started = Instant::now();
    let (whole_bytes, whole_peak) = alloc_probe::peak_extra(|| encode_whole(&page));
    let whole_ms = started.elapsed().as_secs_f64() * 1e3;

    Bands {
        banded_bytes,
        banded_ms,
        banded_peak,
        whole_bytes,
        whole_ms,
        whole_peak,
    }
}

/// Whether the banded path is still doing what it exists for; `Err` carries the diagnosis.
pub fn check(bands: &Bands) -> Result<(), String> {
    let ratio = bands.banded_peak as f64 / bands.whole_peak.max(1) as f64;
    if ratio > BANDED_PEAK_RATIO {
        return Err(format!(
            "banded peak {:.1} MB is {:.1}% of the whole picture's {:.1} MB, over the {:.0}% ceiling",
            bands.banded_peak as f64 / 1e6,
            ratio * 100.0,
            bands.whole_peak as f64 / 1e6,
            BANDED_PEAK_RATIO * 100.0
        ));
    }
    if bands.banded_peak > BANDED_PEAK_CEILING {
        return Err(format!(
            "banded peak {:.1} MB is over the {:.0} MB absolute ceiling",
            bands.banded_peak as f64 / 1e6,
            BANDED_PEAK_CEILING as f64 / 1e6
        ));
    }
    Ok(())
}

pub fn report() -> String {
    let bands = compare();
    let verdict = match check(&bands) {
        Ok(()) => format!(
            "verdict: the banded writer peaks at {:.1} MB against {:.1} MB when the exporter owns the whole picture ({:.1}x less), inside the {:.0}% / {:.0} MB targets",
            bands.banded_peak as f64 / 1e6,
            bands.whole_peak as f64 / 1e6,
            bands.whole_peak as f64 / bands.banded_peak.max(1) as f64,
            BANDED_PEAK_RATIO * 100.0,
            BANDED_PEAK_CEILING as f64 / 1e6
        ),
        Err(diagnosis) => format!("TARGET MISSED: {diagnosis}"),
    };
    format!(
        "bands {WIDTH}x{ROWS}: output {:.2} MB, {:.0} ms, peak extra {:.1} MB
         whole {WIDTH}x{ROWS}: output {:.2} MB, {:.0} ms, peak extra {:.1} MB
         {verdict}",
        bands.banded_bytes as f64 / 1e6,
        bands.banded_ms,
        bands.banded_peak as f64 / 1e6,
        bands.whole_bytes as f64 / 1e6,
        bands.whole_ms,
        bands.whole_peak as f64 / 1e6,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stage 4 peak target, as a test.
    ///
    /// Ignored for the same reason as the scroll targets: two encodes of a 30 000-row page belong
    /// in a release run. `--assert` is the entry point that checks them together.
    #[test]
    #[ignore = "release speed: two 30 000-row encodes; `--assert` runs them in release"]
    fn the_banded_writer_stays_under_a_tenth_of_the_whole_picture() {
        let bands = compare();
        if let Err(diagnosis) = check(&bands) {
            panic!("{diagnosis}");
        }
    }
}
