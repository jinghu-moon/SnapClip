//! CPU-only qualification of the pinned candidate, never product AVIF support.
//! Synthetic fixtures are generated here; no user's image or clipboard is read.
use almost_enough::StopToken;
use enough::{Stop, StopReason, Unstoppable};
use rgb::Rgb;
use rgb::Rgba;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use zenavif::{
    DecoderConfig, EncodeAlphaMode, EncodeBitDepth, EncodeColorModel, EncodePixelRange,
    EncoderConfig, ManagedAvifDecoder,
};

#[path = "../alloc_probe.rs"]
mod alloc_probe;
#[path = "../avif_bands.rs"]
mod avif_bands;
#[path = "../avif_grid.rs"]
mod avif_grid;
#[global_allocator]
static ALLOCATOR: alloc_probe::Counting = alloc_probe::Counting;

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
    // Windows' main thread has a 1 MiB stack. The safe AV1 decoder's debug
    // frames exceed that; use a bounded worker stack for this diagnostic CLI.
    std::thread::Builder::new()
        .name("hdr-avif-qualification".into())
        .stack_size(4 * 1024 * 1024)
        .spawn(run)
        .map_err(|e| e.to_string())?
        .join()
        .map_err(|_| "qualification worker panicked")?
}

fn run() -> Result<(), String> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, directory] = arguments.as_slice()
        && flag == "--band-fixture-dir"
    {
        return avif_bands::fixtures(std::path::Path::new(directory));
    }
    if let [flag, width, height] = arguments.as_slice()
        && flag == "--band-resources"
    {
        return avif_bands::resources(
            width.parse().map_err(|_| "invalid width")?,
            height.parse().map_err(|_| "invalid height")?,
        );
    }
    if let [flag, directory] = arguments.as_slice()
        && flag == "--grid-fixture-dir"
    {
        return avif_grid::fixtures(std::path::Path::new(directory));
    }
    if let [flag, width, height] = arguments.as_slice()
        && flag == "--grid-resources"
    {
        return avif_grid::resources(
            width.parse().map_err(|_| "invalid width")?,
            height.parse().map_err(|_| "invalid height")?,
        );
    }
    if let [flag, width, height] = arguments.as_slice()
        && flag == "--grid-noise-resources"
    {
        return avif_grid::noise_resources(
            width.parse().map_err(|_| "invalid width")?,
            height.parse().map_err(|_| "invalid height")?,
        );
    }
    if let [flag, width, height] = arguments.as_slice()
        && flag == "--resources"
    {
        return resources(
            width.parse().map_err(|_| "invalid width")?,
            height.parse().map_err(|_| "invalid height")?,
        );
    }
    if let [flag, input, output] = arguments.as_slice()
        && flag == "--decode"
    {
        return decode_file(std::path::Path::new(input), std::path::Path::new(output));
    }
    let pixels = fixture();
    let original = pixels.clone();
    let bytes = encode(&pixels, &config(), StopToken::new(Unstoppable));
    match arguments.as_slice() {
        [] => {}
        [flag, directory] if flag == "--fixture-dir" => {
            write_fixtures(std::path::Path::new(directory), &pixels, &bytes)?
        }
        _ => return Err("usage: hdr-avif-probe [--fixture-dir DIRECTORY | --grid-fixture-dir DIRECTORY | --decode INPUT OUTPUT | --resources WIDTH HEIGHT | --grid-resources WIDTH HEIGHT | --grid-noise-resources WIDTH HEIGHT]".into()),
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
    let directory = match arguments.as_slice() {
        [_, directory] => Some(std::path::Path::new(directory)),
        _ => None,
    };
    if let Err(error) = check_color_encoders(directory) {
        failures.push(error);
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

fn check_color_encoders(directory: Option<&std::path::Path>) -> Result<(), String> {
    let source: Vec<Rgba<u16>> = (0..512)
        .map(|index| {
            let level = ((index % 32) / 4) as u16 * 128;
            Rgba::new(
                sample(level),
                sample((level + 256) % 1024),
                sample((level + 512) % 1024),
                sample([0, 1, 512, 1023][(index / 4) % 4]),
            )
        })
        .collect();
    for alpha in [false, true] {
        let expected: Vec<Rgba<u16>> = source
            .iter()
            .map(|p| Rgba::new(p.r, p.g, p.b, if alpha { p.a } else { u16::MAX }))
            .collect();
        let bytes = if alpha {
            encode(&source, &config(), StopToken::new(Unstoppable))
        } else {
            let rgb: Vec<Rgb<u16>> = source.iter().map(|p| Rgb::new(p.r, p.g, p.b)).collect();
            zenavif::encode_rgb16(
                imgref::Img::new(rgb.as_slice(), 32, 16),
                &config(),
                StopToken::new(Unstoppable),
            )
            .map_err(|e| e.to_string())?
            .avif_file
        };
        let name = if alpha {
            "rgba10-colored"
        } else {
            "rgb10-colored"
        };
        // Write before the assertion so an independent decoder can determine
        // whether a failure belongs to the encoder or the candidate decoder.
        if let Some(directory) = directory {
            std::fs::write(directory.join(format!("{name}.avif")), &bytes)
                .map_err(|e| e.to_string())?;
            let expected_bytes: Vec<u8> = expected
                .iter()
                .flat_map(|p| [p.r, p.g, p.b, p.a])
                .flat_map(u16::to_le_bytes)
                .collect();
            std::fs::write(
                directory.join(format!("{name}-expected.bin")),
                expected_bytes,
            )
            .map_err(|e| e.to_string())?;
        }
        let mut decoder =
            ManagedAvifDecoder::new(&bytes, &decode_config()).map_err(|e| e.to_string())?;
        let (decoded, info) = decoder
            .decode_full(&Unstoppable)
            .map_err(|e| e.to_string())?;
        if info.bit_depth != 10
            || info.color_primaries.0 != 9
            || info.transfer_characteristics.0 != 16
            || info.matrix_coefficients.0 != 0
            || info.color_range != zenavif::ColorRange::Full
            || info.has_alpha != alpha
            || info.premultiplied_alpha
        {
            return Err(format!(
                "{name}: unexpected encoded color/Alpha metadata: {info:?}"
            ));
        }
        let pixels: Vec<Rgba<u16>> = if alpha {
            decoded
                .try_as_imgref::<Rgba<u16>>()
                .ok_or("expected RGBA16")?
                .pixels()
                .collect()
        } else {
            decoded
                .try_as_imgref::<Rgb<u16>>()
                .ok_or("expected RGB16")?
                .pixels()
                .map(|p| Rgba::new(p.r, p.g, p.b, u16::MAX))
                .collect()
        };
        let max_error = expected
            .iter()
            .zip(&pixels)
            .flat_map(|(a, b)| [(a.r, b.r), (a.g, b.g), (a.b, b.b), (a.a, b.a)])
            .map(|(a, b)| f64::from(a.abs_diff(b)) * 1023. / 65535.)
            .fold(0.0_f64, f64::max);
        println!(
            "{name} max_code_error={max_error:.6} expected_first={:?} actual_first={:?}",
            expected[0], pixels[0]
        );
        if max_error > 2. {
            return Err(format!("{name} exceeded unchanged 2/1023 budget"));
        }
    }
    Ok(())
}

fn resources(width: usize, height: usize) -> Result<(), String> {
    if !matches!((width, height), (3840, 2160) | (8256, 5504)) {
        return Err("resource qualification accepts only 4K or 45MP synthetic input".into());
    }
    let _measurement = alloc_probe::test_guard();
    let count = width.checked_mul(height).ok_or("pixel count overflow")?;
    let pixels: Vec<Rgba<u16>> = (0..height)
        .flat_map(|y| {
            (0..width).map(move |x| {
                let red = ((x / 128 + y / 128) % 8) as u16 * 128;
                Rgba::new(
                    sample(red),
                    sample(((red / 128 + 2) % 8) * 128),
                    sample(((red / 128 + 4) % 8) * 128),
                    sample([0, 256, 512, 1023][(x / 128) % 4]),
                )
            })
        })
        .collect();
    println!(
        "resource_input width={width} height={height} pixels={count} input_bytes={}",
        count * size_of::<Rgba<u16>>()
    );
    let begin = std::time::Instant::now();
    let (result, peak) = alloc_probe::peak_extra(|| {
        zenavif::encode_rgba16(
            imgref::Img::new(pixels.as_slice(), width, height),
            &config(),
            StopToken::new(Unstoppable),
        )
    });
    let bytes = result.map_err(|e| e.to_string())?.avif_file;
    println!(
        "resource_encode elapsed_ms={:.3} heap_peak_delta={peak} largest_allocation={} encoded_bytes={}",
        begin.elapsed().as_secs_f64() * 1000.,
        alloc_probe::largest_allocation(),
        bytes.len()
    );
    let decode_config = decode_config().frame_size_limit(count as u32);
    let begin = std::time::Instant::now();
    let (result, peak) = alloc_probe::peak_extra(|| {
        let mut decoder = ManagedAvifDecoder::new(&bytes, &decode_config)?;
        decoder.decode_full(&Unstoppable)
    });
    let (decoded, info) = result.map_err(|e| e.to_string())?;
    println!(
        "resource_decode elapsed_ms={:.3} heap_peak_delta={peak} largest_allocation={}",
        begin.elapsed().as_secs_f64() * 1000.,
        alloc_probe::largest_allocation()
    );
    if info.width as usize != width || info.height as usize != height || info.bit_depth != 10 {
        return Err("resource decode dimensions/depth changed".into());
    }
    let actual = decoded
        .try_as_imgref::<Rgba<u16>>()
        .ok_or("resource output is not RGBA16")?;
    let max_error = pixels
        .iter()
        .zip(actual.pixels())
        .flat_map(|(a, b)| [(a.r, b.r), (a.g, b.g), (a.b, b.b), (a.a, b.a)])
        .map(|(a, b)| f64::from(a.abs_diff(b)) * 1023. / 65535.)
        .fold(0.0_f64, f64::max);
    println!("resource_roundtrip max_code_error={max_error:.6}");
    if max_error > 2. {
        return Err("resource roundtrip exceeded 2/1023".into());
    }
    drop(decoded);
    for operation in ["encode", "decode"] {
        let stop = TimedStop::new(std::time::Duration::from_millis(10));
        let started = std::time::Instant::now();
        let cancelled = if operation == "encode" {
            zenavif::encode_rgba16(
                imgref::Img::new(pixels.as_slice(), width, height),
                &config(),
                StopToken::new(stop.clone()),
            )
            .is_err_and(|e| e.to_string().to_lowercase().contains("cancel"))
        } else {
            let mut decoder =
                ManagedAvifDecoder::new(&bytes, &decode_config).map_err(|e| e.to_string())?;
            decoder
                .decode_full(&stop)
                .is_err_and(|e| e.to_string().to_lowercase().contains("cancel"))
        };
        let elapsed = started.elapsed().as_secs_f64() * 1000.;
        println!(
            "resource_cancel operation={operation} deadline_ms=10 elapsed_ms={elapsed:.3} checks={} successful_checks={} cancelled={cancelled}",
            stop.calls.load(Ordering::Relaxed),
            stop.passed.load(Ordering::Relaxed)
        );
        if !cancelled || stop.passed.load(Ordering::Relaxed) == 0 {
            return Err(format!(
                "{operation}: did not exercise cancellation after work started"
            ));
        }
    }
    Ok(())
}

#[derive(Clone)]
struct TimedStop {
    deadline: std::time::Instant,
    calls: Arc<AtomicUsize>,
    passed: Arc<AtomicUsize>,
}
impl TimedStop {
    fn new(delay: std::time::Duration) -> Self {
        Self {
            deadline: std::time::Instant::now() + delay,
            calls: Arc::new(AtomicUsize::new(0)),
            passed: Arc::new(AtomicUsize::new(0)),
        }
    }
}
impl Stop for TimedStop {
    fn check(&self) -> Result<(), StopReason> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if std::time::Instant::now() >= self.deadline {
            Err(StopReason::Cancelled)
        } else {
            self.passed.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }
}

// Qualification interface for independently generated 8/10/12-bit fixtures.
// All outputs are straight, full-range RGBA64LE; no transfer function is applied.
fn decode_file(input: &std::path::Path, output: &std::path::Path) -> Result<(), String> {
    let bytes = std::fs::read(input).map_err(|e| e.to_string())?;
    let mut decoder =
        ManagedAvifDecoder::new(&bytes, &decode_config()).map_err(|e| e.to_string())?;
    let (pixels, info) = decoder
        .decode_full(&Unstoppable)
        .map_err(|e| e.to_string())?;
    let words: Vec<u16> = if let Some(image) = pixels.try_as_imgref::<Rgba<u16>>() {
        image.pixels().flat_map(|p| [p.r, p.g, p.b, p.a]).collect()
    } else if let Some(image) = pixels.try_as_imgref::<Rgb<u16>>() {
        image
            .pixels()
            .flat_map(|p| [p.r, p.g, p.b, u16::MAX])
            .collect()
    } else if let Some(image) = pixels.try_as_imgref::<Rgba<u8>>() {
        image
            .pixels()
            .flat_map(|p| [p.r, p.g, p.b, p.a].map(|v| u16::from(v) * 257))
            .collect()
    } else if let Some(image) = pixels.try_as_imgref::<Rgb<u8>>() {
        image
            .pixels()
            .flat_map(|p| {
                [
                    u16::from(p.r) * 257,
                    u16::from(p.g) * 257,
                    u16::from(p.b) * 257,
                    u16::MAX,
                ]
            })
            .collect()
    } else {
        return Err("unsupported qualification pixel layout".into());
    };
    let output_bytes: Vec<u8> = words.into_iter().flat_map(u16::to_le_bytes).collect();
    std::fs::write(output, output_bytes).map_err(|e| e.to_string())?;
    std::fs::write(
        output.with_extension("icc"),
        info.icc_profile.as_deref().unwrap_or_default(),
    )
    .map_err(|e| e.to_string())?;
    println!(
        "{{\"width\":{},\"height\":{},\"depth\":{},\"primaries\":{},\"transfer\":{},\"matrix\":{},\"full\":{},\"alpha\":{},\"icc_bytes\":{}}}",
        info.width,
        info.height,
        info.bit_depth,
        info.color_primaries.0,
        info.transfer_characteristics.0,
        info.matrix_coefficients.0,
        info.color_range == zenavif::ColorRange::Full,
        info.has_alpha,
        info.icc_profile.as_ref().map_or(0, Vec::len)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn colored_rgb16_and_rgba16_encoders_preserve_channels_and_alpha_boundaries() {
        check_color_encoders(None).unwrap();
    }
    #[test]
    fn rgb16_and_rgba16_encoders_honor_stride_without_reading_padding_or_mutating_input() {
        let expected = Rgba::new(sample(0), sample(256), sample(1023), sample(512));
        let mut source = vec![Rgba::new(u16::MAX, 0, 0, u16::MAX); 40 * 16];
        for row in source.as_chunks_mut::<40>().0 {
            row[..32].fill(expected);
        }
        let original = source.clone();
        let rgb: Vec<Rgb<u16>> = source.iter().map(|p| Rgb::new(p.r, p.g, p.b)).collect();
        for alpha in [false, true] {
            let bytes = if alpha {
                zenavif::encode_rgba16(
                    imgref::Img::new_stride(source.as_slice(), 32, 16, 40),
                    &config(),
                    StopToken::new(Unstoppable),
                )
                .unwrap()
                .avif_file
            } else {
                zenavif::encode_rgb16(
                    imgref::Img::new_stride(rgb.as_slice(), 32, 16, 40),
                    &config(),
                    StopToken::new(Unstoppable),
                )
                .unwrap()
                .avif_file
            };
            let mut decoder = ManagedAvifDecoder::new(&bytes, &decode_config()).unwrap();
            let (decoded, info) = decoder.decode_full(&Unstoppable).unwrap();
            assert_eq!((info.width, info.height), (32, 16));
            if alpha {
                for actual in decoded.try_as_imgref::<Rgba<u16>>().unwrap().pixels() {
                    assert_eq!(actual, expected);
                }
            } else {
                for actual in decoded.try_as_imgref::<Rgb<u16>>().unwrap().pixels() {
                    assert_eq!(actual, Rgb::new(expected.r, expected.g, expected.b));
                }
            }
        }
        assert_eq!(source, original);
    }
    #[test]
    fn identity_rgb_keeps_black_gray_and_alpha_in_native_10bit_codes() {
        let source = fixture();
        let bytes = encode(&source, &config(), StopToken::new(Unstoppable));
        let mut decoder = ManagedAvifDecoder::new(&bytes, &decode_config()).unwrap();
        let (pixels, _) = decoder.decode_full(&Unstoppable).unwrap();
        let image = pixels.try_as_imgref::<Rgba<u16>>().unwrap();
        for (expected, actual) in source.iter().zip(image.pixels()) {
            for (a, b) in [
                (expected.r, actual.r),
                (expected.g, actual.g),
                (expected.b, actual.b),
                (expected.a, actual.a),
            ] {
                assert!(
                    f64::from(a.abs_diff(b)) * 1023.0 / 65535.0 <= 2.0,
                    "expected={expected:?}, actual={actual:?}"
                );
            }
        }
    }
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
