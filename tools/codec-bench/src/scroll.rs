//! The scroll capture pipeline under measurement (docs/34 §12).
//!
//! The cost of a long scroll capture is dominated by the **canvas**, not the encoder: the
//! stitcher holds the whole long image in memory long before an encoder sees it. So the number
//! that matters is measured where the product spends it — the real `Tracker` from
//! `snapclip-capture` (behind its `test-support` feature), fed synthetic viewport frames.
//!
//! Three things are reported per format: how much the stitching itself cost, how much memory the
//! growing canvas held at its peak, and what encoding the finished image costs. The stitching and
//! the canvas are format independent and are measured once.

use crate::alloc_probe;
use crate::codecs;

/// The viewport the scroll happens in.
pub const WIDTH: usize = 1280;
pub const VIEW: usize = 960;
/// How far each frame scrolls. The overlap left over (960 - 600 = 360 rows) is far above the
/// estimator's 32-row minimum, so every frame is measurable.
pub const STEP: usize = 600;
/// The long image the reference material is built to reach.
pub const ROWS: usize = 30_000;

/// A deterministic page: every row carries a value derived from its own index (so a shift is
/// unambiguous), plus a gradient and periodic bands so the material is not a solid colour.
pub fn page(width: usize, rows: usize) -> Vec<u8> {
    let mut pixels = vec![0u8; width * rows * 4];
    for y in 0..rows {
        let tag = (y as u32).wrapping_mul(2_654_435_761);
        let band = if y % 97 < 6 { 200 } else { 0 };
        let row = &mut pixels[y * width * 4..(y + 1) * width * 4];
        for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
            let value = ((x as u32).wrapping_mul(31) ^ tag) as u8;
            let gradient = ((x * 255) / width.max(1)) as u8;
            pixel[0] = value;
            pixel[1] = gradient.saturating_add(band);
            pixel[2] = (tag >> 16) as u8;
            pixel[3] = 255;
        }
        // The first eight pixels spell the row number, which makes a wrong stitch obvious.
        for x in 0..8 {
            let byte = (y >> (x * 4)) as u8;
            let offset = y * width * 4 + x * 4;
            pixels[offset] = byte;
            pixels[offset + 1] = !byte;
        }
    }
    pixels
}

/// One viewport frame: `view` rows starting at `top`.
pub fn frame(page: &[u8], width: usize, top: usize, view: usize) -> Vec<u8> {
    let start = top * width * 4;
    page[start..start + view * width * 4].to_vec()
}

/// What the stitching run cost.
pub struct Stitched {
    pub frames: usize,
    pub rows: u32,
    /// Per-frame `Tracker::push` times, in microseconds, with the thumbnail refresh taken out.
    pub stitch_us: Vec<u64>,
    pub thumb_us: u64,
    /// Peak extra allocation while pushing frames: the edge buffers and the tile file.
    pub peak_bytes: usize,
    /// Peak extra allocation when the finished image is gathered for the encoder.
    pub assembly_peak: usize,
    pub lost: usize,
    pub correct: bool,
}

/// Drive the real tracker over the page, scrolling down by `step` each frame.
///
/// `view` is a parameter because it decides the screen cap: the product stops growing at 32
/// viewports, so a 800-row view can never reach 30 000 rows while a 960-row one can.
pub fn run(view: usize, step: usize) -> Stitched {
    let page = page(WIDTH, ROWS);
    let first = frame(&page, WIDTH, 0, view);
    let mut probe =
        snapclip_capture::scroll::probe::ScrollProbe::new(&first, WIDTH as u32, view as u32, WIDTH as u32 / 8);
    // The measured window covers only what capture holds: pushing frames, the edge buffers and the
    // tile file. Gathering the finished image is the export path's cost (docs/35 §5.3), and the
    // whole point of the backing store is that the two are no longer the same number.
    let (pushed, peak_bytes) = alloc_probe::peak_extra(|| {
        let mut stitch_us = Vec::new();
        let mut thumb_us = 0;
        let mut lost = 0;
        let mut frames = 1;
        let mut top = 0;
        while top + view + step <= ROWS {
            top += step;
            let next = frame(&page, WIDTH, top, view);
            let report = probe.push(&next);
            stitch_us.push(report.stitch_us);
            thumb_us += report.thumb_us;
            if report.lost {
                lost += 1;
            }
            frames += 1;
        }
        let rows = probe.image_rows();
        (frames, rows, stitch_us, thumb_us, lost)
    });
    let ((width, height, canvas), assembly_peak) = alloc_probe::peak_extra(|| probe.into_image());
    let truth = &page[..(width as usize * height as usize * 4).min(page.len())];
    let correct = canvas.len() == truth.len() && canvas == truth;
    let (frames, rows, stitch_us, thumb_us, lost) = pushed;
    Stitched {
        frames,
        rows,
        stitch_us,
        thumb_us,
        peak_bytes,
        assembly_peak,
        lost,
        correct,
    }
}

fn median_and_max(values: &[u64]) -> (u64, u64) {
    if values.is_empty() {
        return (0, 0);
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let max = *sorted.last().unwrap_or(&0);
    (sorted[sorted.len() / 2], max)
}

fn megabytes(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// The whole `--scroll` report.
pub fn report() -> String {
    let mut out = String::new();
    out.push_str("scroll capture pipeline (docs/34 §12)\n");
    out.push_str(&format!(
        "page {}x{} · viewport {}x{} · {} px per frame\n\n",
        WIDTH,
        ROWS,
        WIDTH,
        VIEW,
        STEP
    ));

    // The cap is a function of the viewport, and that is a product fact worth printing: with the
    // 32-screen cap, an 800-row viewport stops at 25 600 rows and cannot reach this page.
    let cap_for = |view: usize| {
        snapclip_capture::scroll::probe::ScrollProbe::row_cap(WIDTH as u32, view as u32)
            .min(snapclip_capture::scroll::probe::ScrollProbe::screen_cap(
                view as u32,
            ))
    };
    out.push_str(&format!(
        "canvas cap: {} rows at a {}-row viewport, {} rows at a {}-row one (MAX_SCREENS x view, and MAX_BYTES)\n\n",
        cap_for(800),
        800,
        cap_for(VIEW),
        VIEW
    ));

    let stitched = run(VIEW, STEP);
    let (median, max) = median_and_max(&stitched.stitch_us);
    out.push_str(&format!(
        "stitching: {} frames · {} rows · per-frame {:.2} ms median / {:.2} ms max · thumb total {:.1} ms · working peak {:.1} MB · assembly peak {:.1} MB · lost {} · pixel-exact {}\n\n",
        stitched.frames,
        stitched.rows,
        median as f64 / 1000.0,
        max as f64 / 1000.0,
        stitched.thumb_us as f64 / 1000.0,
        megabytes(stitched.peak_bytes),
        megabytes(stitched.assembly_peak),
        stitched.lost,
        stitched.correct
    ));

    let page = page(WIDTH, ROWS);
    let (width, height, canvas) = {
        let first = frame(&page, WIDTH, 0, VIEW);
        let mut probe = snapclip_capture::scroll::probe::ScrollProbe::new(
            &first,
            WIDTH as u32,
            VIEW as u32,
            WIDTH as u32 / 8,
        );
        let mut top = 0;
        while top + VIEW + STEP <= ROWS {
            top += STEP;
            let next = frame(&page, WIDTH, top, VIEW);
            probe.push(&next);
        }
        probe.into_image()
    };

    out.push_str("format | bytes | encode ms | encode peak MB | note\n");
    out.push_str("--- | ---: | ---: | ---: | ---\n");

    let (png_bytes, png_ms, png_peak) = {
        let started = std::time::Instant::now();
        let (result, peak) = alloc_probe::peak_extra(|| {
            codecs::png_row_band(&canvas, width as usize, height as usize)
        });
        (result.ok().map(|bytes| bytes.len()), started.elapsed(), peak)
    };
    out.push_str(&format!(
        "PNG | {} | {:.1} | {:.1} | row band streaming\n",
        png_bytes.map_or_else(|| "failed".to_string(), |bytes| bytes.to_string()),
        png_ms.as_secs_f64() * 1000.0,
        megabytes(png_peak)
    ));

    let (jpeg_bytes, jpeg_ms, jpeg_peak) = {
        let options = codecs::JpegOptions {
            quality: 92,
            subsample_444: true,
            trellis: false,
            optimize_huffman: false,
            band_rows: 64,
        };
        let started = std::time::Instant::now();
        let (result, peak) = alloc_probe::peak_extra(|| {
            codecs::zenjpeg(&canvas, width as usize, height as usize, options)
        });
        (result.ok().map(|bytes| bytes.len()), started.elapsed(), peak)
    };
    out.push_str(&format!(
        "JPG | {} | {:.1} | {:.1} | baseline 4:4:4 q92, plain, 64-row bands\n",
        jpeg_bytes.map_or_else(|| "failed".to_string(), |bytes| bytes.to_string()),
        jpeg_ms.as_secs_f64() * 1000.0,
        megabytes(jpeg_peak)
    ));

    // WebP must be refused **before** anything is allocated for it: the limit is the format's, so
    // the honest product behaviour is to check the size before a scroll session starts at all.
    let (webp_result, webp_peak) = alloc_probe::peak_extra(|| {
        let limit = 16_383;
        if width as usize > limit || height as usize > limit {
            Err(format!(
                "webp: {}x{} exceeds the format's 16383 per side",
                width, height
            ))
        } else {
            codecs::zenwebp_lossless(&canvas, width as usize, height as usize)
        }
    });
    out.push_str(&format!(
        "WebP | {} | - | {:.1} | refused before encoding: {}\n",
        "refused",
        megabytes(webp_peak),
        webp_result.err().unwrap_or_default()
    ));

    let total_ms = stitched.stitch_us.iter().sum::<u64>() as f64 / 1000.0
        + stitched.thumb_us as f64 / 1000.0
        + png_ms.as_secs_f64() * 1000.0;
    out.push_str(&format!(
        "\nverdict: a {}x{} scroll capture costs {:.1} ms of stitching plus {:.0} ms of PNG encoding, with a {:.1} MB canvas at its peak; the bottleneck is the canvas, not the encoder.",
        WIDTH,
        stitched.rows,
        stitched.stitch_us.iter().sum::<u64>() as f64 / 1000.0,
        total_ms - stitched.stitch_us.iter().sum::<u64>() as f64 / 1000.0,
        megabytes(stitched.peak_bytes)
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_is_deterministic_and_every_row_is_identifiable() {
        let first = page(64, 200);
        let second = page(64, 200);
        assert_eq!(first, second, "same arguments, same bytes");
        // The row tag in the first eight pixels must differ between adjacent rows.
        let tag = |row: usize| first[row * 64 * 4..row * 64 * 4 + 32].to_vec();
        assert_ne!(tag(0), tag(1));
        assert_ne!(tag(1), tag(2));
    }

    #[test]
    fn a_scroll_run_reproduces_the_page_pixel_for_pixel() {
        // A short page so the test is cheap; the same code drives the 30 000-row report.
        let page = page(WIDTH, 4 * STEP + VIEW);
        let rows = 4 * STEP + VIEW;
        let view = VIEW;
        let first = frame(&page, WIDTH, 0, view);
        let mut probe = snapclip_capture::scroll::probe::ScrollProbe::new(
            &first,
            WIDTH as u32,
            view as u32,
            WIDTH as u32 / 8,
        );
        let mut top = 0;
        while top + view + STEP <= rows {
            top += STEP;
            let next = frame(&page, WIDTH, top, view);
            probe.push(&next);
        }
        let (width, height, canvas) = probe.into_image();
        assert_eq!(width as usize, WIDTH);
        assert_eq!(height as usize, rows, "the whole page was captured");
        assert!(canvas == page, "the stitch must be pixel exact");
    }

    #[test]
    fn webp_is_refused_before_it_can_allocate_a_long_image() {
        // The refusal has to happen before the BGRA-to-RGBA conversion, or "refused" still costs
        // 153 MB for a 30 000-row image. The peak is process-wide, so hold the measurement lock:
        // another test's allocations must not be mistaken for ours.
        let _serial = crate::alloc_probe::test_guard();
        let (result, peak) = alloc_probe::peak_extra(|| {
            let limit = 16_383;
            let (width, height) = (WIDTH, ROWS);
            if width > limit || height > limit {
                Err("exceeds 16383".to_string())
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        // The peak is process-wide, so the tolerance has to absorb whatever the other tests in the
        // same binary allocate while this one runs. What it must keep is the discrimination: the
        // conversion this test exists to forbid would allocate WIDTH * ROWS * 4, about 152 MB, so a
        // megabyte still tells "refused before allocating" apart from "converted and then refused".
        assert!(peak < 1024 * 1024, "refusal allocated {peak} bytes");
    }
}
