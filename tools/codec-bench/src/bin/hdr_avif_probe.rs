//! CPU-only qualification of the pinned candidate, never product AVIF support.
//! Synthetic fixtures are generated here; no user's image or clipboard is read.
use almost_enough::StopToken;
use enough::{Stop, StopReason, Unstoppable};
use rgb::Rgba;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use zenavif::{
    DecoderConfig, EncodeAlphaMode, EncodeBitDepth, EncodeColorModel, EncodePixelRange,
    EncoderConfig, ManagedAvifDecoder,
};

#[derive(Clone)]
struct CancelAfter {
    calls: Arc<AtomicUsize>,
    limit: usize,
}
impl CancelAfter {
    fn new(limit: usize) -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            limit,
        }
    }
}
impl Stop for CancelAfter {
    fn check(&self) -> Result<(), StopReason> {
        if self.calls.fetch_add(1, Ordering::Relaxed) >= self.limit {
            Err(StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}

fn config() -> EncoderConfig {
    EncoderConfig::new()
        .quality(100.0)
        .alpha_quality(100.0)
        .speed(10)
        .bit_depth(EncodeBitDepth::Ten)
        .color_model(EncodeColorModel::Rgb)
        .alpha_color_mode(EncodeAlphaMode::UnassociatedDirty)
        .color_primaries(9)
        .transfer_characteristics(16)
        .matrix_coefficients(0)
        .pixel_range(EncodePixelRange::Full)
        .threads(Some(1))
}
fn decode_config() -> DecoderConfig {
    DecoderConfig::new()
        .threads(1)
        .frame_size_limit(1024 * 1024)
        .prefer_8bit(false)
}
fn sample(code: u16) -> u16 {
    (code << 6) | (code >> 4)
}
fn fixture() -> Vec<Rgba<u16>> {
    (0..16)
        .flat_map(|_| {
            (0..32).map(|x| {
                let level = (x / 4) as u16 * 128;
                Rgba::new(sample(level), sample(level), sample(level), sample(512))
            })
        })
        .collect()
}
fn encode(pixels: &[Rgba<u16>], config: &EncoderConfig, stop: StopToken) -> Vec<u8> {
    zenavif::encode_rgba16(imgref::Img::new(pixels, 32, 16), config, stop)
        .expect("valid synthetic input must encode")
        .avif_file
}
// ST 2084 EOTF with independent constants; output is cd/m², not scRGB.
fn nits(code: f64) -> f64 {
    let v = code.powf(32.0 / 2523.0);
    let ratio = ((v - 3424.0 / 4096.0).max(0.0) / (2413.0 / 128.0 - 2392.0 / 128.0 * v)).max(0.0);
    10_000.0 * ratio.powf(16384.0 / 2610.0)
}

// Inspect only our generated fixture's ISO-BMFF box tree; this is not a product
// AVIF parser. Bounds errors are failures, never a substring search for metadata.
type FixtureBoxes<'a> = Vec<(&'a [u8], &'a [u8])>;
fn boxes(bytes: &[u8]) -> Result<FixtureBoxes<'_>, String> {
    let mut boxes = Vec::new();
    let mut remaining = bytes;
    while !remaining.is_empty() {
        let header = remaining.get(..8).ok_or("truncated fixture box header")?;
        let size = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        if size < 8 || size > remaining.len() {
            return Err("unsupported or invalid fixture box size".into());
        }
        boxes.push((&header[4..8], &remaining[8..size]));
        remaining = &remaining[size..];
    }
    Ok(boxes)
}
fn child<'a>(bytes: &'a [u8], name: &[u8]) -> Result<&'a [u8], String> {
    boxes(bytes)?
        .into_iter()
        .find_map(|(kind, data)| (kind == name).then_some(data))
        .ok_or_else(|| format!("missing fixture box {}", String::from_utf8_lossy(name)))
}
fn color_properties(bytes: &[u8]) -> Result<Vec<&[u8]>, String> {
    let meta = child(bytes, b"meta")?;
    let meta = meta.get(4..).ok_or("missing meta version flags")?;
    let iprp = child(meta, b"iprp")?;
    let ipco = child(iprp, b"ipco")?;
    Ok(boxes(ipco)?
        .into_iter()
        .filter_map(|(kind, data)| (kind == b"colr").then_some(data))
        .collect())
}

fn write_fixtures(
    directory: &std::path::Path,
    pixels: &[Rgba<u16>],
    bytes: &[u8],
) -> Result<(), String> {
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    std::fs::write(directory.join("pq10-rgb-alpha.avif"), bytes).map_err(|e| e.to_string())?;
    let expected: Vec<u8> = pixels
        .iter()
        .flat_map(|p| [p.r, p.g, p.b, p.a])
        .flat_map(u16::to_le_bytes)
        .collect();
    std::fs::write(directory.join("expected-rgba64le.bin"), expected).map_err(|e| e.to_string())
}

fn main() -> Result<(), String> {
    let pixels = fixture();
    let original = pixels.clone();
    let bytes = encode(&pixels, &config(), StopToken::new(Unstoppable));
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.as_slice() {
        [] => {}
        [flag, directory] if flag == "--fixture-dir" => {
            write_fixtures(std::path::Path::new(directory), &pixels, &bytes)?
        }
        _ => return Err("usage: hdr-avif-probe [--fixture-dir DIRECTORY]".into()),
    }
    let mut decoder =
        ManagedAvifDecoder::new(&bytes, &decode_config()).map_err(|e| e.to_string())?;
    let (decoded, info) = decoder
        .decode_full(&Unstoppable)
        .map_err(|e| e.to_string())?;
    let rgba = decoded
        .try_as_imgref::<Rgba<u16>>()
        .ok_or("candidate returned a layout other than native RGBA16")?;
    let mut max_code_error = 0.0_f64;
    let mut max_nit_error = 0.0_f64;
    let mut failures = Vec::new();
    let mut luminance_failed = false;
    for (index, actual) in rgba.pixels().take(32).enumerate().step_by(4) {
        println!(
            "sample x={index} expected={:?} decoded={actual:?}",
            pixels[index]
        );
    }
    for (expected, actual) in pixels.iter().zip(rgba.pixels()) {
        for (a, b) in [
            (expected.r, actual.r),
            (expected.g, actual.g),
            (expected.b, actual.b),
            (expected.a, actual.a),
        ] {
            max_code_error = max_code_error.max(f64::from(a.abs_diff(b)) * 1023.0 / 65535.0);
        }
        let expected_nits = nits(f64::from(expected.r) / 65535.0);
        let actual_nits = nits(f64::from(actual.r) / 65535.0);
        let error = (expected_nits - actual_nits).abs();
        max_nit_error = max_nit_error.max(error);
        if !luminance_failed && error > (expected_nits * 0.03).max(0.02) {
            failures.push(format!(
                "PQ luminance {actual_nits:.6} differs from {expected_nits:.6} nit"
            ));
            luminance_failed = true;
        }
    }
    let tags_ok = info.width == 32
        && info.height == 16
        && info.bit_depth == 10
        && info.color_primaries.0 == 9
        && info.transfer_characteristics.0 == 16
        && info.matrix_coefficients.0 == 0
        && info.color_range == zenavif::ColorRange::Full
        && info.has_alpha
        && !info.premultiplied_alpha;
    if !tags_ok {
        failures.push(format!("unexpected HDR/Alpha tags: {info:?}"));
    }
    if max_code_error > 2.0 {
        failures.push(format!("code error {max_code_error} exceeds 2/1023"));
    }
    if pixels != original {
        failures.push("encoder changed its input".into());
    }
    println!(
        "pq10_rgb_alpha bytes={} tags_ok={tags_ok} max_code_error={max_code_error:.6} max_nit_error={max_nit_error:.6}",
        bytes.len()
    );

    for limit in [0, 2] {
        let stop = CancelAfter::new(limit);
        let result = zenavif::encode_rgba16(
            imgref::Img::new(pixels.as_slice(), 32, 16),
            &config(),
            StopToken::new(stop.clone()),
        );
        let cancelled = result
            .as_ref()
            .is_err_and(|e| e.to_string().to_lowercase().contains("cancel"));
        println!(
            "encode_cancel limit={limit} checks={} cancelled={cancelled}",
            stop.calls.load(Ordering::Relaxed)
        );
        if !cancelled {
            failures.push(format!(
                "encoder did not honour cancellation after {limit} checks"
            ));
        }
        let stop = CancelAfter::new(limit);
        let result = zenavif::decode_with(&bytes, &decode_config(), &stop);
        let cancelled = result
            .as_ref()
            .is_err_and(|e| e.to_string().to_lowercase().contains("cancel"));
        println!(
            "decode_cancel limit={limit} checks={} cancelled={cancelled}",
            stop.calls.load(Ordering::Relaxed)
        );
        if !cancelled {
            failures.push(format!(
                "decoder did not honour cancellation after {limit} checks"
            ));
        }
    }
    let malformed_rejected =
        zenavif::decode_with(b"not an AVIF", &decode_config(), &Unstoppable).is_err();
    let truncated_rejected =
        zenavif::decode_with(&bytes[..bytes.len() / 2], &decode_config(), &Unstoppable).is_err();
    let budget_rejected = ManagedAvifDecoder::new(
        &bytes,
        &DecoderConfig::new().threads(1).frame_size_limit(511),
    )
    .is_err();
    println!(
        "malformed_rejected={malformed_rejected} truncated_rejected={truncated_rejected} pixel_budget_rejected={budget_rejected}"
    );
    if !(malformed_rejected && truncated_rejected && budget_rejected) {
        failures.push("candidate accepted malformed/truncated/over-budget input".into());
    }
    let profile = moxcms::ColorProfile::new_srgb()
        .encode()
        .map_err(|e| e.to_string())?;
    let conflict = encode(
        &pixels,
        &config().icc_profile(profile.clone()),
        StopToken::new(Unstoppable),
    );
    let properties = color_properties(&conflict)?;
    let container_icc = properties
        .iter()
        .any(|p| p.get(..4) == Some(b"prof") && p.get(4..) == Some(profile.as_slice()));
    let container_pq = properties
        .iter()
        .any(|p| p.get(..4) == Some(b"nclx") && p.get(4..8) == Some(&[0, 9, 0, 16]));
    let mut decoder =
        ManagedAvifDecoder::new(&conflict, &decode_config()).map_err(|e| e.to_string())?;
    let (_, metadata) = decoder
        .decode_full(&Unstoppable)
        .map_err(|e| e.to_string())?;
    let icc_exposed = metadata.icc_profile.as_ref() == Some(&profile);
    let pq_exposed = metadata.color_primaries.0 == 9 && metadata.transfer_characteristics.0 == 16;
    println!(
        "icc_nclx container_icc={container_icc} container_pq={container_pq} icc_exposed={icc_exposed} pq_exposed={pq_exposed}"
    );
    // This serializer deliberately replaces container nclx with ICC. Test what
    // it actually emits; this fixture does not qualify dual-colr precedence.
    if !container_icc || !icc_exposed || (container_pq && !pq_exposed) {
        failures.push("candidate failed to preserve its encoded color metadata".into());
    }
    if let [_, directory] = arguments.as_slice() {
        std::fs::write(
            std::path::Path::new(directory).join("icc-nclx-conflict.avif"),
            conflict,
        )
        .map_err(|e| e.to_string())?;
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_pq_reference_has_known_endpoints_and_preserves_alpha_codes() {
        assert!(nits(0.0).abs() < 1e-9);
        assert!((nits(1.0) - 10_000.0).abs() < 1e-6);
        assert!((nits(0.751827096247041) - 1000.0).abs() < 1e-6);
        assert_eq!(sample(0), 0);
        assert_eq!(sample(1023), 65535);
        assert_eq!(fixture().len(), 512);
    }
    #[test]
    fn fixture_box_reader_rejects_truncation_overflow_and_unsupported_extended_sizes() {
        for invalid in [&b"123"[..], &b"\0\0\0\x01meta"[..], &b"\0\0\0\x10meta"[..]] {
            assert!(boxes(invalid).is_err());
        }
        assert_eq!(child(b"\0\0\0\x0ctestdata", b"test").unwrap(), b"data");
        assert!(child(b"\0\0\0\x0ctestdata", b"absent").is_err());
    }
}
