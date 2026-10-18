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

pub fn report() -> String {
    let page: Vec<u8> = (0..WIDTH as usize * ROWS as usize * 4)
        .map(|i| (i % 251) as u8)
        .collect();

    let started = Instant::now();
    let (banded_bytes, banded_peak) = alloc_probe::peak_extra(|| encode_bands(&page));
    let banded_ms = started.elapsed().as_secs_f64() * 1e3;

    let started = Instant::now();
    let (whole_bytes, whole_peak) = alloc_probe::peak_extra(|| encode_whole(&page));
    let whole_ms = started.elapsed().as_secs_f64() * 1e3;

    format!(
        "bands {WIDTH}x{ROWS}: output {:.2} MB, {:.0} ms, peak extra {:.1} MB\n\
         whole {WIDTH}x{ROWS}: output {:.2} MB, {:.0} ms, peak extra {:.1} MB\n\
         verdict: the banded writer peaks at {:.1} MB against {:.1} MB when the exporter owns the whole picture ({:.1}x less)",
        banded_bytes as f64 / 1e6,
        banded_ms,
        banded_peak as f64 / 1e6,
        whole_bytes as f64 / 1e6,
        whole_ms,
        whole_peak as f64 / 1e6,
        banded_peak as f64 / 1e6,
        whole_peak as f64 / 1e6,
        whole_peak as f64 / banded_peak.max(1) as f64,
    )
}
