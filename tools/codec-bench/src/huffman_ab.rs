//! A minimal, decidable A/B for `optimize_huffman` (docs/34 §14).
//!
//! The question is narrow on purpose. Two encodes differ in **one** flag:
//! `EncoderConfig::optimize_huffman(true)` versus `(false)`, with the same quality, the same
//! 4:4:4 subsampling, the same baseline mode, the same pixel layout and the same band size.
//! `optimize_huffman` is documented as `HuffmanStrategy::Optimize`: two passes over the
//! coefficients to build optimal Huffman tables. Entropy coding cannot change a coefficient, so
//! the decoded picture **must** be identical. If it is not, the flag does more than its name or
//! the encoder is wrong.
//!
//! Two traps this module is built to avoid:
//!
//! * Decoding with the encoding library lets a bug mirror itself, so the pixels are decoded with
//!   `image` as well as with `zenjpeg`. The two decoders do not agree to the last bit (different
//!   YCbCr rounding), so the **plain versus plain cross-decoder** difference is reported as the
//!   noise floor that the plain versus huffman difference is judged against.
//! * A size difference proves nothing about pixels, so the segment headers are dumped: if the DQT
//!   payloads differ the arms are not comparable, and if the DHT payloads are identical the flag
//!   never reached the tables.

use std::fmt::Write as _;

use crate::codecs;
use crate::synthetic;

/// Rows per `push`; banding does not change the output (docs/34 §10).
const BAND_ROWS: usize = 64;

/// One JPEG marker segment with its payload, enough to compare two arms structurally.
struct Segment {
    marker: u8,
    payload: Vec<u8>,
}

fn segments(bytes: &[u8]) -> Vec<Segment> {
    let mut out = Vec::new();
    if bytes.len() < 2 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return out;
    }
    let mut index = 2;
    while index + 4 <= bytes.len() {
        if bytes[index] != 0xFF {
            break;
        }
        // Fill bytes (`FF FF …`) are legal before a marker.
        let mut marker_index = index + 1;
        while marker_index < bytes.len() && bytes[marker_index] == 0xFF {
            marker_index += 1;
        }
        if marker_index + 1 >= bytes.len() {
            break;
        }
        let marker = bytes[marker_index];
        let length_index = marker_index + 1;
        let length = usize::from(bytes[length_index]) << 8 | usize::from(bytes[length_index + 1]);
        let payload_start = length_index + 2;
        let payload_end = (length_index + length).min(bytes.len());
        out.push(Segment { marker, payload: bytes[payload_start..payload_end].to_vec() });
        // Start of scan: entropy-coded data follows, so stop before trying to walk it.
        if marker == 0xDA {
            break;
        }
        index = payload_end;
    }
    out
}

fn marker_name(marker: u8) -> String {
    match marker {
        0xC0 | 0xC1 | 0xC2 => format!("SOF{}({:02X})", marker - 0xC0, marker),
        0xDB => format!("DQT({marker:02X})"),
        0xC4 => format!("DHT({marker:02X})"),
        0xDA => format!("SOS({marker:02X})"),
        0xFE => format!("COM({marker:02X})"),
        0xE0..=0xEF => format!("APP{}({marker:02X})", marker - 0xE0),
        _ => format!("M{marker:02X}"),
    }
}

/// Which of the two arms' segments differ, in order, with payload lengths.
fn segment_diff(left: &[Segment], right: &[Segment]) -> String {
    let mut text = String::new();
    let count = left.len().max(right.len());
    for index in 0..count {
        let name = match (left.get(index), right.get(index)) {
            (Some(a), Some(b)) if a.marker == b.marker => marker_name(a.marker),
            (Some(a), b) => format!("{} vs {}", marker_name(a.marker), b.map_or("-".to_string(), |s| marker_name(s.marker))),
            (None, Some(b)) => format!("- vs {}", marker_name(b.marker)),
            (None, None) => unreachable!(),
        };
        let a = left.get(index).map(|s| s.payload.as_slice());
        let b = right.get(index).map(|s| s.payload.as_slice());
        let same = a == b;
        let describe = |payload: Option<&[u8]>| match payload {
            Some(payload) => format!("{} B", payload.len()),
            None => "absent".to_string(),
        };
        let _ = writeln!(
            text,
            "    {name:<14} plain {} · huffman {} · {}",
            describe(a),
            describe(b),
            if same { "identical" } else { "DIFFERENT" }
        );
    }
    text
}

/// Decoded pixels plus the mean and maximum absolute difference to a reference.
fn decode_with_image(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>), String> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
        .map_err(|error| format!("image decode: {error}"))?;
    let rgb = image.to_rgb8();
    Ok((rgb.width() as usize, rgb.height() as usize, rgb.into_raw()))
}

fn decode_with_zenjpeg(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>), String> {
    let (width, height, rgba) = codecs::decode_jpeg(bytes)?;
    let mut rgb = Vec::with_capacity(width * height * 3);
    for pixel in rgba.chunks_exact(4) {
        rgb.extend_from_slice(&pixel[..3]);
    }
    Ok((width, height, rgb))
}

/// Mean and maximum absolute difference over every channel, plus the share of equal bytes.
fn difference(left: &[u8], right: &[u8]) -> (f64, u8, f64) {
    if left.len() != right.len() || left.is_empty() {
        return (f64::NAN, 255, 0.0);
    }
    let mut total = 0u64;
    let mut max = 0u8;
    let mut equal = 0usize;
    for (a, b) in left.iter().zip(right) {
        let delta = a.abs_diff(*b);
        total += u64::from(delta);
        max = max.max(delta);
        if delta == 0 {
            equal += 1;
        }
    }
    (total as f64 / left.len() as f64, max, equal as f64 / left.len() as f64)
}

fn encode(asset: &synthetic::Asset, optimize_huffman: bool) -> Result<Vec<u8>, String> {
    codecs::zenjpeg_knobs(
        &asset.pixels,
        asset.width,
        asset.height,
        92,
        true,
        false,
        optimize_huffman,
        BAND_ROWS,
    )
}

/// The optimiser with the whole frame handed over in **one** push.
///
/// This exists to rule out the last remaining variable between the two arms the sink compares: the
/// sink buffers a short frame and encodes it once, while its plain arm pushes all rows in a single
/// call too, but the two are built through different code paths. If whole-frame and 64-row banding
/// agree here, then the `mean |Δ| = 75` once seen between the sink's arms cannot be blamed on the
/// optimiser *or* on banding (`docs/34` §14).
fn encode_whole_frame(asset: &synthetic::Asset, optimize_huffman: bool) -> Result<Vec<u8>, String> {
    codecs::zenjpeg_knobs(
        &asset.pixels,
        asset.width,
        asset.height,
        92,
        true,
        false,
        optimize_huffman,
        0,
    )
}

/// The whole report: per asset, the two arms, their segments and four pixel comparisons.
pub fn report() -> String {
    let mut text = String::new();
    let _ = writeln!(
        text,
        "optimize_huffman A/B (only change: optimize_huffman true vs false; q92, 4:4:4, baseline, BGRA, {BAND_ROWS}-row bands)\n"
    );

    for asset in synthetic::corpus().into_iter().filter(|a| a.name == "ui-1080" || a.name == "scroll-tall") {
        let _ = writeln!(text, "{} ({}x{})", asset.name, asset.width, asset.height);
        let plain = match encode(&asset, false) {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = writeln!(text, "  plain encode failed: {error}\n");
                continue;
            }
        };
        let huffman = match encode(&asset, true) {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = writeln!(text, "  huffman encode failed: {error}\n");
                continue;
            }
        };
        let reduction = 100.0 * (1.0 - huffman.len() as f64 / plain.len() as f64);
        let _ = writeln!(
            text,
            "  bytes: plain {} · huffman {} ({reduction:+.1}%)",
            plain.len(),
            huffman.len()
        );
        let _ = writeln!(text, "  segments:");
        let _ = write!(text, "{}", segment_diff(&segments(&plain), &segments(&huffman)));

        let plain_image = decode_with_image(&plain);
        let huffman_image = decode_with_image(&huffman);
        let plain_zen = decode_with_zenjpeg(&plain);
        let huffman_zen = decode_with_zenjpeg(&huffman);
        match (plain_image, huffman_image, plain_zen, huffman_zen) {
            (Ok((pw, ph, p_img)), Ok((hw, hh, h_img)), Ok((pzw, pzh, p_zen)), Ok((hzw, hzh, h_zen))) => {
                let (mean, max, equal) = difference(&p_img, &h_img);
                let _ = writeln!(
                    text,
                    "  pixels, image decoder: plain vs huffman → mean |Δ| {mean:.4} · max {max} · equal {:.4}% ({pw}x{ph} vs {hw}x{hh})",
                    equal * 100.0
                );
                let (mean, max, _) = difference(&p_zen, &h_zen);
                let _ = writeln!(
                    text,
                    "  pixels, zenjpeg decoder: plain vs huffman → mean |Δ| {mean:.4} · max {max}"
                );
                // Noise floor: the same bytes through two decoders. Any plain-vs-huffman difference
                // has to be judged against this, not against zero.
                let (mean, max, _) = difference(&p_img, &p_zen);
                let _ = writeln!(
                    text,
                    "  noise floor, plain arm, image vs zenjpeg: mean |Δ| {mean:.4} · max {max} ({pzw}x{pzh})"
                );
                let (mean, max, _) = difference(&h_img, &h_zen);
                let _ = writeln!(
                    text,
                    "  noise floor, huffman arm, image vs zenjpeg: mean |Δ| {mean:.4} · max {max} ({hzw}x{hzh})"
                );
                // Banded versus whole-frame, both with the optimiser on: the variable the sink's
                // own comparison had left open.
                match encode_whole_frame(&asset, true).and_then(|b| decode_with_image(&b)) {
                    Ok((ww, wh, whole)) => {
                        let (mean, max, equal) = difference(&whole, &h_img);
                        let _ = writeln!(
                            text,
                            "  banding only, huffman arm: whole-frame {} B vs {BAND_ROWS}-row bands · pixels mean |Δ| {mean:.4} · max {max} · equal {:.4}% ({ww}x{wh})",
                            encode_whole_frame(&asset, true).map(|b| b.len()).unwrap_or(0),
                            equal * 100.0
                        );
                    }
                    Err(error) => {
                        let _ = writeln!(text, "  banding check failed: {error}");
                    }
                }
            }
            (plain_image, huffman_image, plain_zen, huffman_zen) => {
                let _ = writeln!(
                    text,
                    "  decode failed: image plain {:?} / image huffman {:?} / zenjpeg plain {:?} / zenjpeg huffman {:?}",
                    plain_image.err(),
                    huffman_image.err(),
                    plain_zen.err(),
                    huffman_zen.err()
                );
            }
        }
        let _ = writeln!(text);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_segment_walk_finds_the_headers_of_a_real_jpeg() {
        let asset = synthetic::corpus().into_iter().find(|a| a.name == "ui-1080").unwrap();
        let bytes = encode(&asset, false).unwrap();
        let found = segments(&bytes);
        assert!(found.iter().any(|s| matches!(s.marker, 0xC0 | 0xC1 | 0xC2)), "SOF");
        assert!(found.iter().any(|s| s.marker == 0xDB), "DQT");
        assert!(found.iter().any(|s| s.marker == 0xC4), "DHT");
        assert_eq!(found.last().map(|s| s.marker), Some(0xDA), "SOS last");
    }

    #[test]
    fn the_difference_metric_is_zero_for_equal_bytes() {
        let (mean, max, equal) = difference(&[7, 7, 7], &[7, 7, 7]);
        assert_eq!((mean, max, equal), (0.0, 0, 1.0));
    }

    #[test]
    fn the_difference_metric_reports_asymmetric_changes() {
        let (mean, max, equal) = difference(&[0, 10, 20], &[255, 10, 21]);
        assert!((mean - (255.0 + 0.0 + 1.0) / 3.0).abs() < 1e-9);
        assert_eq!(max, 255);
        assert!((equal - 1.0 / 3.0).abs() < 1e-9);
    }
}
