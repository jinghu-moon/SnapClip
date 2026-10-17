//! SSIM and PSNR, for the whole image and for the boxes where text was drawn.
//!
//! JPEG is lossy, so its output has to be decoded and compared against the lossless PNG the same
//! asset produced. Whole-image numbers are not enough on screenshot material: a UI is mostly flat
//! surfaces, and those survive compression almost perfectly, so a whole-image SSIM of 0.99 can hide
//! text that has turned to mush. Every metric here therefore exists twice — whole image and
//! text-only — and the text-only one is the number this crate is built to report.

use crate::synthetic::TextRegion;

/// The two numbers the report prints for one encoding.
#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    /// Mean structural similarity, 1.0 for identical pixels.
    pub ssim: f64,
    /// Peak signal-to-noise ratio in dB, infinite for identical pixels.
    pub psnr: f64,
}

/// SSIM constants for 8-bit samples: C1 = (0.01·255)² and C2 = (0.03·255)².
const C1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
const C2: f64 = (0.03 * 255.0) * (0.03 * 255.0);

/// Side of the SSIM window. Eight is the smallest window that still has a stable variance and it
/// keeps a single line of text (10–13 pixels tall) inside one window row.
const BLOCK: usize = 8;

/// Luma of one RGBA pixel, BT.601 weights, which is what the eye's structural sensitivity follows.
fn luma(rgba: &[u8], width: usize, x: usize, y: usize) -> f64 {
    let index = (y * width + x) * 4;
    0.299 * f64::from(rgba[index]) + 0.587 * f64::from(rgba[index + 1]) + 0.114 * f64::from(rgba[index + 2])
}

/// Metrics over the whole frame.
pub fn whole(reference: &[u8], candidate: &[u8], width: usize, height: usize) -> Metrics {
    measure(reference, candidate, width, height, &[TextRegion { x0: 0, y0: 0, x1: width, y1: height }])
}

/// Metrics over the union of `regions`, which must not overlap.
pub fn in_regions(
    reference: &[u8],
    candidate: &[u8],
    width: usize,
    height: usize,
    regions: &[TextRegion],
) -> Metrics {
    measure(reference, candidate, width, height, regions)
}

fn measure(
    reference: &[u8],
    candidate: &[u8],
    width: usize,
    height: usize,
    regions: &[TextRegion],
) -> Metrics {
    assert_eq!(reference.len(), candidate.len(), "sizes must match");
    assert!(reference.len() >= width * height * 4, "buffers must cover the frame");
    Metrics {
        ssim: ssim(reference, candidate, width, height, regions),
        psnr: psnr(reference, candidate, width, height, regions),
    }
}

/// Peak signal-to-noise ratio over the three colour channels of every pixel in `regions`.
fn psnr(
    reference: &[u8],
    candidate: &[u8],
    width: usize,
    height: usize,
    regions: &[TextRegion],
) -> f64 {
    let mut squared = 0.0f64;
    let mut samples = 0u64;
    for region in regions {
        for y in region.y0..region.y1.min(height) {
            for x in region.x0..region.x1.min(width) {
                let index = (y * width + x) * 4;
                for channel in 0..3 {
                    let difference = f64::from(reference[index + channel]) - f64::from(candidate[index + channel]);
                    squared += difference * difference;
                    samples += 1;
                }
            }
        }
    }
    if samples == 0 {
        return f64::INFINITY;
    }
    let mse = squared / samples as f64;
    if mse == 0.0 { f64::INFINITY } else { 10.0 * (255.0 * 255.0 / mse).log10() }
}

/// Mean SSIM over every window that lies entirely inside one of `regions`.
///
/// A region smaller than one window is still measurable: the window shrinks to the region, which
/// keeps single text lines (ten pixels tall) out of the "no full window, no answer" corner.
fn ssim(
    reference: &[u8],
    candidate: &[u8],
    width: usize,
    height: usize,
    regions: &[TextRegion],
) -> f64 {
    let mut total = 0.0;
    let mut windows = 0u64;
    for region in regions {
        let right = region.x1.min(width);
        let bottom = region.y1.min(height);
        if region.x0 >= right || region.y0 >= bottom {
            continue;
        }
        let block_x = BLOCK.min(right - region.x0);
        let block_y = BLOCK.min(bottom - region.y0);
        let mut y = region.y0;
        while y + block_y <= bottom {
            let mut x = region.x0;
            while x + block_x <= right {
                total += window_ssim(reference, candidate, width, x, y, block_x, block_y);
                windows += 1;
                x += block_x;
            }
            y += block_y;
        }
    }
    if windows == 0 { 1.0 } else { total / windows as f64 }
}

fn window_ssim(
    reference: &[u8],
    candidate: &[u8],
    width: usize,
    x0: usize,
    y0: usize,
    block_width: usize,
    block_height: usize,
) -> f64 {
    let count = (block_width * block_height) as f64;
    let mut sum_reference = 0.0;
    let mut sum_candidate = 0.0;
    for y in y0..y0 + block_height {
        for x in x0..x0 + block_width {
            sum_reference += luma(reference, width, x, y);
            sum_candidate += luma(candidate, width, x, y);
        }
    }
    let mean_reference = sum_reference / count;
    let mean_candidate = sum_candidate / count;

    let mut variance_reference = 0.0;
    let mut variance_candidate = 0.0;
    let mut covariance = 0.0;
    for y in y0..y0 + block_height {
        for x in x0..x0 + block_width {
            let left = luma(reference, width, x, y) - mean_reference;
            let right = luma(candidate, width, x, y) - mean_candidate;
            variance_reference += left * left;
            variance_candidate += right * right;
            covariance += left * right;
        }
    }
    let variance_reference = variance_reference / count;
    let variance_candidate = variance_candidate / count;
    let covariance = covariance / count;

    let numerator = (2.0 * mean_reference * mean_candidate + C1) * (2.0 * covariance + C2);
    let denominator = (mean_reference * mean_reference + mean_candidate * mean_candidate + C1)
        * (variance_reference + variance_candidate + C2);
    if denominator == 0.0 { 1.0 } else { numerator / denominator }
}

/// Copies a rectangle out of an RGBA frame, for the zoomed side-by-side dump.
pub fn crop(rgba: &[u8], width: usize, region: TextRegion) -> (Vec<u8>, usize, usize) {
    let crop_width = region.width();
    let crop_height = region.height();
    let mut out = vec![0u8; crop_width * crop_height * 4];
    for row in 0..crop_height {
        let source = ((region.y0 + row) * width + region.x0) * 4;
        let destination = row * crop_width * 4;
        out[destination..destination + crop_width * 4]
            .copy_from_slice(&rgba[source..source + crop_width * 4]);
    }
    (out, crop_width, crop_height)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame with a busy left half and a flat right half, which makes region-restricted metrics
    /// easy to reason about.
    fn frame(width: usize, height: usize) -> Vec<u8> {
        let mut rgba = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                let index = (y * width + x) * 4;
                let value = if x < width / 2 { ((x * 37 + y * 11) % 256) as u8 } else { 128 };
                rgba[index] = value;
                rgba[index + 1] = value;
                rgba[index + 2] = value;
                rgba[index + 3] = 255;
            }
        }
        rgba
    }

    #[test]
    fn identical_frames_score_a_perfect_one_and_infinite_psnr() {
        let (width, height) = (64, 32);
        let reference = frame(width, height);
        let metrics = whole(&reference, &reference, width, height);
        assert!((metrics.ssim - 1.0).abs() < 1e-9, "ssim {}", metrics.ssim);
        assert!(metrics.psnr.is_infinite(), "psnr {}", metrics.psnr);
    }

    #[test]
    fn a_clearly_different_frame_scores_lower() {
        let (width, height) = (64, 32);
        let reference = frame(width, height);
        let mut candidate = reference.clone();
        for (index, byte) in candidate.iter_mut().enumerate() {
            if index % 4 != 3 {
                *byte = byte.wrapping_add(60);
            }
        }
        let metrics = whole(&reference, &candidate, width, height);
        assert!(metrics.ssim < 0.9, "ssim {}", metrics.ssim);
        assert!(metrics.psnr < 30.0, "psnr {}", metrics.psnr);
    }

    /// The point of the region index: damage outside the text must not move the text numbers.
    #[test]
    fn regions_see_only_their_own_pixels() {
        let (width, height) = (64, 32);
        let reference = frame(width, height);
        let mut candidate = reference.clone();
        // Damage the flat right half, which no region covers. All three channels, because luma is
        // a weighted sum: shifting only red moves it by 0.3x and the whole-image SSIM would stay
        // high enough to tell us nothing.
        for y in 0..height {
            for x in width / 2..width {
                let index = (y * width + x) * 4;
                for channel in 0..3 {
                    candidate[index + channel] = candidate[index + channel].wrapping_add(80);
                }
            }
        }
        let text = TextRegion { x0: 0, y0: 0, x1: width / 2, y1: height };
        let inside = in_regions(&reference, &candidate, width, height, &[text]);
        let outside = whole(&reference, &candidate, width, height);
        assert!((inside.ssim - 1.0).abs() < 1e-9, "region ssim {}", inside.ssim);
        assert!(outside.ssim < 0.99, "whole ssim {} should notice", outside.ssim);
    }

    #[test]
    fn a_region_smaller_than_one_window_is_still_measured() {
        let (width, height) = (64, 32);
        let reference = frame(width, height);
        let mut candidate = reference.clone();
        let tiny = TextRegion { x0: 4, y0: 4, x1: 14, y1: 9 };
        for y in tiny.y0..tiny.y1 {
            for x in tiny.x0..tiny.x1 {
                let index = (y * width + x) * 4;
                candidate[index] = candidate[index].wrapping_add(90);
            }
        }
        let metrics = in_regions(&reference, &candidate, width, height, &[tiny]);
        assert!(metrics.ssim < 0.9, "a 10x5 region must be measurable, got {}", metrics.ssim);
        assert!(metrics.psnr.is_finite());
    }

    #[test]
    fn cropping_takes_the_requested_rectangle() {
        let (width, height) = (8, 4);
        let mut rgba = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                let index = (y * width + x) * 4;
                rgba[index] = (y * 10) as u8;
            }
        }
        let (pixels, crop_width, crop_height) = crop(&rgba, width, TextRegion { x0: 2, y0: 1, x1: 6, y1: 3 });
        assert_eq!((crop_width, crop_height), (4, 2));
        assert_eq!(pixels.len(), 4 * 2 * 4);
        // First row of the crop is row 1 of the frame.
        assert_eq!(pixels[0], 10);
        assert_eq!(pixels[crop_width * 4], 20);
    }
}
