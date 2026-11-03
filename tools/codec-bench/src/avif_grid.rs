//! Qualification-only tiled AVIF. No application or UI crate enables this module.
use almost_enough::StopToken;
use enough::Stop;
use rgb::Rgba;
use zenavif_parse::{AV1Metadata, AvifParser};
use zenavif_serialize::{Av1CBox, ColrBox, constants::*, grid::GridImage};

// Both compressed tiles and their serialized container are bounded. This is a
// qualification contract, not a general-purpose large-file AVIF writer.
pub(super) const MAX_PAYLOAD: usize = 4 * 1024 * 1024;
const MAX_FILE: usize = MAX_PAYLOAD + 1024 * 1024;
const MAX_TILES: usize = 4096;
pub(super) const TILE_SIDE: usize = 128;

fn av1_config(meta: &AV1Metadata) -> Av1CBox {
    let mut config = Av1CBox::default();
    config.seq_profile = meta.seq_profile;
    config.high_bitdepth = meta.bit_depth > 8;
    config.twelve_bit = meta.bit_depth == 12;
    config.monochrome = meta.monochrome;
    config.chroma_subsampling_x = meta.chroma_subsampling.horizontal;
    config.chroma_subsampling_y = meta.chroma_subsampling.vertical;
    config
}

pub(super) fn encode(
    width: usize,
    height: usize,
    side: usize,
    pixel: impl Fn(usize, usize) -> Rgba<u16>,
    icc: Option<&[u8]>,
    stop: StopToken,
) -> Result<Vec<u8>, String> {
    encode_with_payload_limit(width, height, side, pixel, icc, stop, MAX_PAYLOAD)
}

fn encode_with_payload_limit(
    width: usize,
    height: usize,
    side: usize,
    pixel: impl Fn(usize, usize) -> Rgba<u16>,
    icc: Option<&[u8]>,
    stop: StopToken,
    payload_limit: usize,
) -> Result<Vec<u8>, String> {
    encode_tiles(
        width,
        height,
        side,
        icc,
        stop,
        payload_limit,
        |left, top, tile| {
            tile.extend((0..side).flat_map(|y| {
                let pixel = &pixel;
                (0..side).map(move |x| pixel((left + x).min(width - 1), (top + y).min(height - 1)))
            }));
            Ok(())
        },
    )
}

// The fallible tile provider lets real row sources reject a changed generation,
// invalid color or failed read before any partial compressed result is returned.
pub(super) fn encode_tiles(
    width: usize,
    height: usize,
    side: usize,
    icc: Option<&[u8]>,
    stop: StopToken,
    payload_limit: usize,
    mut fill: impl FnMut(usize, usize, &mut Vec<Rgba<u16>>) -> Result<(), String>,
) -> Result<Vec<u8>, String> {
    stop.check().map_err(|e| e.to_string())?;
    if width == 0 || height == 0 || !matches!(side, 64 | 128) {
        return Err("unsupported grid geometry".into());
    }
    let columns = width.div_ceil(side);
    let rows = height.div_ceil(side);
    let count = rows.checked_mul(columns).ok_or("tile count overflow")?;
    if count > MAX_TILES || rows > 255 || columns > 255 {
        return Err("grid exceeds tile limit".into());
    }
    let mut image = GridImage::new();
    if let Some(icc) = icc {
        if icc.len() > 64 * 1024 {
            return Err("grid ICC profile limit exceeded".into());
        }
        image.set_icc_profile(icc.to_vec());
    }
    let mut color_info = ColrBox::default();
    color_info.color_primaries = ColorPrimaries::Bt2020;
    color_info.transfer_characteristics = TransferCharacteristics::Smpte2084;
    color_info.matrix_coefficients = MatrixCoefficients::Rgb;
    image.set_depth_bits(10).set_colr(color_info);
    let mut colors = Vec::with_capacity(count);
    let mut alphas = Vec::with_capacity(count);
    let mut payload = 0usize;
    let mut tile = Vec::with_capacity(side * side);
    for row in 0..rows {
        for column in 0..columns {
            stop.check().map_err(|e| e.to_string())?;
            tile.clear();
            fill(column * side, row * side, &mut tile)?;
            if tile.len() != side * side {
                return Err("source returned an incomplete grid tile".into());
            }
            stop.check().map_err(|e| e.to_string())?;
            let encoded = zenavif::encode_rgba16(
                imgref::Img::new(tile.as_slice(), side, side),
                &super::config(),
                stop.clone(),
            )
            .map_err(|e| e.to_string())?;
            let parser = AvifParser::from_bytes(&encoded.avif_file).map_err(|e| e.to_string())?;
            let color = parser.primary_data().map_err(|e| e.to_string())?;
            let alpha = parser
                .alpha_data()
                .ok_or("tile lost alpha")?
                .map_err(|e| e.to_string())?;
            payload = payload
                .checked_add(color.len())
                .and_then(|n| n.checked_add(alpha.len()))
                .filter(|&n| n <= payload_limit.min(MAX_PAYLOAD))
                .ok_or("compressed grid payload limit exceeded")?;
            if colors.is_empty() {
                image.set_color_config(av1_config(
                    &parser.primary_metadata().map_err(|e| e.to_string())?,
                ));
                image.set_alpha_config(av1_config(
                    &parser
                        .alpha_metadata()
                        .ok_or("missing alpha metadata")?
                        .map_err(|e| e.to_string())?,
                ));
            }
            colors.push(color.into_owned());
            alphas.push(alpha.into_owned());
        }
    }
    stop.check().map_err(|e| e.to_string())?;
    let colors: Vec<&[u8]> = colors.iter().map(Vec::as_slice).collect();
    let alphas: Vec<&[u8]> = alphas.iter().map(Vec::as_slice).collect();
    let bytes = image
        .serialize(
            rows as u8,
            columns as u8,
            width as u32,
            height as u32,
            side as u32,
            side as u32,
            &colors,
            Some(&alphas),
        )
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FILE {
        return Err("serialized grid file limit exceeded".into());
    }
    stop.check().map_err(|e| e.to_string())?;
    Ok(bytes)
}

pub(super) fn decode(
    bytes: &[u8],
    stop: &impl Stop,
    consume: impl FnMut(u32, u32, &zenavif::PixelBuffer) -> zenavif::Result<()>,
) -> Result<zenavif::ImageInfo, String> {
    stop.check().map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FILE {
        return Err("compressed grid file limit exceeded".into());
    }
    let cfg = super::decode_config()
        .frame_size_limit(64 * 1024 * 1024)
        .grid_tile_limit(MAX_TILES as u32);
    let mut decoder =
        zenavif::ManagedAvifDecoder::new_with_stop(bytes, &cfg, stop).map_err(|e| e.to_string())?;
    decoder
        .decode_grid_tiles(stop, (TILE_SIDE * TILE_SIDE) as u32, consume)
        .map_err(|e| e.to_string())
}

pub(super) fn fixtures(directory: &std::path::Path) -> Result<(), String> {
    use enough::Unstoppable;
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let profile = moxcms::ColorProfile::new_srgb()
        .encode()
        .map_err(|e| e.to_string())?;
    for (name, icc) in [("grid-pq", None), ("grid-icc", Some(profile.as_slice()))] {
        let bytes = encode(133, 71, 64, fixture_pixel, icc, StopToken::new(Unstoppable))?;
        std::fs::write(directory.join(format!("{name}.avif")), bytes).map_err(|e| e.to_string())?;
    }
    let raw: Vec<u8> = (0..71)
        .flat_map(|y| {
            (0..133).flat_map(move |x| {
                let p = fixture_pixel(x, y);
                [p.r, p.g, p.b, p.a].into_iter().flat_map(u16::to_le_bytes)
            })
        })
        .collect();
    std::fs::write(directory.join("expected-rgba64le.bin"), raw).map_err(|e| e.to_string())?;
    std::fs::write(directory.join("expected.icc"), profile).map_err(|e| e.to_string())?;
    Ok(())
}

fn fixture_pixel(x: usize, y: usize) -> Rgba<u16> {
    Rgba::new(
        super::sample((x % 1024) as u16),
        super::sample((y % 1024) as u16),
        super::sample(((x + y) % 1024) as u16),
        super::sample([0, 256, 512, 1023][x % 4]),
    )
}

fn resource_pixel(x: usize, y: usize) -> Rgba<u16> {
    let red = ((x / 128 + y / 128) % 8) as u16 * 128;
    Rgba::new(
        super::sample(red),
        super::sample(((red / 128 + 2) % 8) * 128),
        super::sample(((red / 128 + 4) % 8) * 128),
        super::sample([0, 256, 512, 1023][(x / 128) % 4]),
    )
}

pub(super) fn noise_resources(width: usize, height: usize) -> Result<(), String> {
    use enough::Unstoppable;
    if !matches!((width, height), (3840, 2160) | (8256, 5504)) {
        return Err("noise qualification accepts only 4K or 45MP".into());
    }
    let _guard = super::alloc_probe::test_guard();
    let start = std::time::Instant::now();
    let mut read_pixels = 0usize;
    let (result, peak) = super::alloc_probe::peak_extra(|| {
        let count = std::cell::Cell::new(0);
        let result = encode(
            width,
            height,
            TILE_SIDE,
            |x, y| {
                count.set(count.get() + 1);
                let mut value = (x as u64).wrapping_mul(0x9e3779b97f4a7c15)
                    ^ (y as u64).wrapping_mul(0xbf58476d1ce4e5b9);
                value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
                value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
                value ^= value >> 31;
                Rgba::new(
                    super::sample((value & 1023) as u16),
                    super::sample(((value >> 10) & 1023) as u16),
                    super::sample(((value >> 20) & 1023) as u16),
                    super::sample(((value >> 30) & 1023) as u16),
                )
            },
            None,
            StopToken::new(Unstoppable),
        );
        read_pixels = count.get();
        result
    });
    let largest = super::alloc_probe::largest_allocation();
    let rejected = result.is_err_and(|e| e.contains("payload limit"));
    println!(
        "grid_noise width={width} height={height} read_pixels={read_pixels} heap_peak_delta={peak} largest_allocation={largest} payload_rejected={rejected} elapsed_ms={:.3}",
        start.elapsed().as_secs_f64() * 1000.
    );
    if !rejected
        || peak >= 32 * 1024 * 1024
        || largest >= 16 * 1024 * 1024
        || read_pixels >= width * height
    {
        return Err("noise did not stop within the existing streaming budget".into());
    }
    Ok(())
}

pub(super) fn resources(width: usize, height: usize) -> Result<(), String> {
    use enough::Unstoppable;
    if !matches!((width, height), (3840, 2160) | (8256, 5504)) {
        return Err("resource qualification accepts only 4K or 45MP synthetic input".into());
    }
    let _measurement = super::alloc_probe::test_guard();
    println!(
        "resource_input width={width} height={height} full_frame_input_bytes=0 tile_side={TILE_SIDE} compressed_payload_limit={MAX_PAYLOAD} container_limit={MAX_FILE}"
    );
    let start = std::time::Instant::now();
    let (result, peak) = super::alloc_probe::peak_extra(|| {
        encode(
            width,
            height,
            TILE_SIDE,
            resource_pixel,
            None,
            StopToken::new(Unstoppable),
        )
    });
    let bytes = result?;
    println!(
        "resource_encode elapsed_ms={:.3} heap_peak_delta={peak} largest_allocation={} encoded_bytes={}",
        start.elapsed().as_secs_f64() * 1000.,
        super::alloc_probe::largest_allocation(),
        bytes.len()
    );
    let cfg = super::decode_config()
        .frame_size_limit((width * height) as u32)
        .grid_tile_limit(MAX_TILES as u32);
    let start = std::time::Instant::now();
    let (result, peak) = super::alloc_probe::peak_extra(|| {
        let mut checked = 0usize;
        let mut max_error = 0u16;
        let info = decode(&bytes, &Unstoppable, |left, top, pixels| {
            let image = pixels.try_as_imgref::<Rgba<u16>>().expect("RGBA16 tiles");
            for (index, actual) in image.pixels().enumerate() {
                let expected = resource_pixel(
                    left as usize + index % image.width(),
                    top as usize + index / image.width(),
                );
                for (a, b) in [
                    (expected.r, actual.r),
                    (expected.g, actual.g),
                    (expected.b, actual.b),
                    (expected.a, actual.a),
                ] {
                    max_error = max_error.max(a.abs_diff(b));
                }
                checked += 1;
            }
            Ok(())
        })?;
        Ok::<_, String>((info, checked, max_error))
    });
    println!(
        "resource_decode elapsed_ms={:.3} heap_peak_delta={peak} largest_allocation={}",
        start.elapsed().as_secs_f64() * 1000.,
        super::alloc_probe::largest_allocation()
    );
    let (info, checked, max_error) = result?;
    let error = f64::from(max_error) * 1023. / 65535.;
    println!("resource_roundtrip max_code_error={error:.6} checked_pixels={checked}");
    if checked != width * height
        || (
            info.width as usize,
            info.height as usize,
            info.bit_depth,
            info.has_alpha,
        ) != (width, height, 10, true)
        || error > 2.
    {
        return Err("grid resource pixel gate failed".into());
    }
    for operation in ["encode", "decode", "decode_tiles"] {
        // Also measure tile execution after indexing, so a cancellation during
        // indexing cannot stand in for actually interrupting the AV1 work.
        let mut prepared = if operation == "decode_tiles" {
            Some(zenavif::ManagedAvifDecoder::new(&bytes, &cfg).map_err(|e| e.to_string())?)
        } else {
            None
        };
        let stop = super::TimedStop::new(std::time::Duration::from_millis(10));
        let start = std::time::Instant::now();
        let cancelled = if operation == "encode" {
            encode(
                width,
                height,
                TILE_SIDE,
                resource_pixel,
                None,
                StopToken::new(stop.clone()),
            )
            .is_err_and(|e| e.to_lowercase().contains("cancel"))
        } else {
            let result = if let Some(decoder) = prepared.as_mut() {
                decoder.decode_grid_tiles(&stop, (TILE_SIDE * TILE_SIDE) as u32, |_, _, _| Ok(()))
            } else {
                zenavif::ManagedAvifDecoder::new_with_stop(&bytes, &cfg, &stop).and_then(
                    |mut decoder| {
                        decoder.decode_grid_tiles(
                            &stop,
                            (TILE_SIDE * TILE_SIDE) as u32,
                            |_, _, _| Ok(()),
                        )
                    },
                )
            };
            result.is_err_and(|e| e.to_string().to_lowercase().contains("cancel"))
        };
        let elapsed = start.elapsed().as_secs_f64() * 1000.;
        let calls = stop.calls.load(std::sync::atomic::Ordering::Relaxed);
        let passed = stop.passed.load(std::sync::atomic::Ordering::Relaxed);
        println!(
            "resource_cancel operation={operation} deadline_ms=10 elapsed_ms={elapsed:.3} checks={calls} successful_checks={passed} cancelled={cancelled}"
        );
        if !cancelled || passed == 0 {
            return Err(format!(
                "{operation}: cancellation was not exercised during work"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use enough::Unstoppable;

    fn pixel(x: usize, y: usize) -> Rgba<u16> {
        fixture_pixel(x, y)
    }

    #[test]
    fn cropped_grid_reads_descriptor_and_merges_alpha_in_row_major_order() {
        let bytes = encode(133, 71, 64, pixel, None, StopToken::new(Unstoppable)).unwrap();
        let parser = AvifParser::from_bytes(&bytes).unwrap();
        let grid = parser.grid_config().unwrap();
        assert_eq!(
            (
                grid.rows,
                grid.columns,
                grid.output_width,
                grid.output_height
            ),
            (2, 3, 133, 71)
        );
        let mut decoder =
            zenavif::ManagedAvifDecoder::new(&bytes, &super::super::decode_config()).unwrap();
        assert_eq!(decoder.probe_info().unwrap().bit_depth, 10);
        assert_eq!(
            decoder.probe_info().unwrap().chroma_sampling,
            zenavif::ChromaSampling::Cs444
        );
        let (actual, info) = decoder.decode_full(&Unstoppable).unwrap();
        assert_eq!(
            (info.width, info.height, info.bit_depth, info.has_alpha),
            (133, 71, 10, true)
        );
        let rgba = actual.try_as_imgref::<Rgba<u16>>().unwrap();
        // Exact equality to independently decoded standalone tiles separates
        // stitching errors from AV1's per-tile quantization. Source fidelity
        // retains the pre-existing <=2 native-code-value qualification gate.
        let mut max_error = 0u16;
        for index in 0..6 {
            let color = parser.tile_data(index).unwrap();
            let alpha = parser.alpha_tile_data(index).unwrap().unwrap();
            let mut mux = zenavif_serialize::Aviffy::new();
            mux.set_width(64)
                .set_height(64)
                .set_bit_depth(10)
                .set_chroma_subsampling((false, false));
            let mut single = Vec::new();
            mux.write_slice(&mut single, &color, Some(&alpha)).unwrap();
            let mut decoder =
                zenavif::ManagedAvifDecoder::new(&single, &super::super::decode_config()).unwrap();
            let (tile, _) = decoder.decode_full(&Unstoppable).unwrap();
            let reference = tile.try_as_imgref::<Rgba<u16>>().unwrap();
            for y in 0..64.min(71 - index / 3 * 64) {
                for x in 0..64.min(133 - index % 3 * 64) {
                    let (gx, gy) = (index % 3 * 64 + x, index / 3 * 64 + y);
                    let value = rgba[(gx, gy)];
                    assert_eq!(value, reference[(x, y)], "tile {index} ({x},{y})");
                    let expected = pixel(gx, gy);
                    assert_eq!(value.a, expected.a);
                    for (a, b) in [
                        (value.r, expected.r),
                        (value.g, expected.g),
                        (value.b, expected.b),
                    ] {
                        max_error = max_error.max(a.abs_diff(b));
                    }
                }
            }
        }
        assert!(f64::from(max_error) * 1023. / 65535. <= 2.);
    }

    #[test]
    fn tile_sink_covers_each_output_pixel_once_and_keeps_icc() {
        let profile = moxcms::ColorProfile::new_srgb().encode().unwrap();
        let bytes = encode(
            133,
            71,
            64,
            pixel,
            Some(&profile),
            StopToken::new(Unstoppable),
        )
        .unwrap();
        let mut covered = vec![false; 133 * 71];
        let mut tiles = 0;
        let info = decode(&bytes, &Unstoppable, |x, y, pixels| {
            tiles += 1;
            assert!(pixels.width() <= 64 && pixels.height() <= 64);
            for py in 0..pixels.height() {
                for px in 0..pixels.width() {
                    let offset = (y + py) as usize * 133 + (x + px) as usize;
                    assert!(!covered[offset]);
                    covered[offset] = true;
                }
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(tiles, 6);
        assert!(covered.into_iter().all(|p| p));
        assert_eq!(info.icc_profile, Some(profile));
        assert_eq!(
            (info.width, info.height, info.bit_depth, info.has_alpha),
            (133, 71, 10, true)
        );
    }

    #[test]
    fn sink_error_and_cancellation_stop_before_delivering_the_next_tile() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        #[derive(Clone)]
        struct Flag(Arc<AtomicBool>);
        impl Stop for Flag {
            fn check(&self) -> Result<(), enough::StopReason> {
                if self.0.load(Ordering::Relaxed) {
                    Err(enough::StopReason::Cancelled)
                } else {
                    Ok(())
                }
            }
        }
        let bytes = encode(133, 71, 64, pixel, None, StopToken::new(Unstoppable)).unwrap();
        let mut delivered = 0;
        let error = decode(&bytes, &Unstoppable, |_, _, _| {
            delivered += 1;
            Err(zenavif::Error::Unsupported("fixture sink failure").into())
        })
        .unwrap_err();
        assert!(error.contains("fixture sink failure"));
        assert_eq!(delivered, 1);
        delivered = 0;
        let stop = Flag(Arc::new(AtomicBool::new(false)));
        let error = decode(&bytes, &stop, |_, _, _| {
            delivered += 1;
            stop.0.store(true, Ordering::Relaxed);
            Ok(())
        })
        .unwrap_err();
        assert!(error.to_lowercase().contains("cancel"));
        assert_eq!(delivered, 1);
    }

    #[test]
    fn tile_pixel_limit_is_checked_before_delivering_or_allocating_a_frame() {
        let bytes = encode(64, 64, 64, pixel, None, StopToken::new(Unstoppable)).unwrap();
        let mut decoder =
            zenavif::ManagedAvifDecoder::new(&bytes, &super::super::decode_config()).unwrap();
        let mut delivered = 0;
        let _guard = super::super::alloc_probe::test_guard();
        let (result, peak) = super::super::alloc_probe::peak_extra(|| {
            decoder.decode_grid_tiles(&Unstoppable, 4095, |_, _, _| {
                delivered += 1;
                Ok(())
            })
        });
        assert!(result.unwrap_err().to_string().contains("tile pixel limit"));
        assert_eq!(delivered, 0);
        assert!(peak < 64 * 1024, "rejection allocated {peak}");
    }

    #[test]
    fn grid_tile_count_limit_is_explicit_and_preserves_the_parser_default() {
        let bytes = encode(133, 71, 64, pixel, None, StopToken::new(Unstoppable)).unwrap();
        assert_eq!(
            zenavif_parse::DecodeConfig::default().max_grid_tiles,
            Some(1000)
        );
        let error = zenavif::ManagedAvifDecoder::new(
            &bytes,
            &super::super::decode_config().grid_tile_limit(5),
        )
        .err()
        .expect("six tiles must exceed a five-tile limit");
        assert!(error.to_string().contains("tile count limit"));
        assert!(
            zenavif::ManagedAvifDecoder::new(
                &bytes,
                &super::super::decode_config().grid_tile_limit(6)
            )
            .is_ok()
        );
    }

    #[test]
    fn constructor_honors_cancellation_before_copy_and_during_grid_indexing() {
        let bytes = encode(133, 71, 64, pixel, None, StopToken::new(Unstoppable)).unwrap();
        for limit in [0, 5] {
            let stop = super::super::CancelAfter::new(limit);
            let cfg = super::super::decode_config();
            let error = zenavif::ManagedAvifDecoder::new_with_stop(&bytes, &cfg, &stop)
                .err()
                .expect("constructor must honor the passed stop token");
            assert!(error.to_string().to_lowercase().contains("cancel"));
            assert_eq!(
                stop.calls.load(std::sync::atomic::Ordering::Relaxed),
                limit + 1
            );
        }
    }

    #[test]
    fn corrupted_descriptor_alpha_and_truncated_file_fail_explicitly() {
        let bytes = encode(133, 71, 64, pixel, None, StopToken::new(Unstoppable)).unwrap();
        let parser = AvifParser::from_bytes(&bytes).unwrap();
        let descriptor = parser.primary_data().unwrap();
        let offset = descriptor.as_ptr() as usize - bytes.as_ptr() as usize;
        for (field, value) in [(0, 1), (1, 2), (2, 2), (3, 0), (4, 255), (7, 0)] {
            let mut invalid = bytes.clone();
            invalid[offset + field] = value;
            assert!(
                AvifParser::from_bytes(&invalid).is_err(),
                "descriptor field {field}"
            );
        }
        let alpha = parser.alpha_tile_data(0).unwrap().unwrap();
        let offset = alpha.as_ptr() as usize - bytes.as_ptr() as usize;
        let mut invalid = bytes.clone();
        invalid[offset] = 255;
        let mut delivered = 0;
        assert!(
            decode(&invalid, &Unstoppable, |_, _, _| {
                delivered += 1;
                Ok(())
            })
            .is_err()
        );
        assert_eq!(delivered, 0);
        assert!(AvifParser::from_bytes(&bytes[..bytes.len() / 2]).is_err());
    }

    #[test]
    fn compressed_and_geometry_limits_refuse_work_without_a_full_frame_fallback() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        let result = encode_with_payload_limit(
            133,
            71,
            64,
            |x, y| {
                calls.set(calls.get() + 1);
                pixel(x, y)
            },
            None,
            StopToken::new(Unstoppable),
            0,
        );
        assert!(result.unwrap_err().contains("payload limit"));
        assert_eq!(calls.get(), 64 * 64);
        for (width, height, side) in [
            (0, 71, 64),
            (133, 0, 64),
            (133, 71, 512),
            (usize::MAX, 71, 64),
        ] {
            assert!(
                encode(
                    width,
                    height,
                    side,
                    |_, _| panic!("invalid geometry read pixels"),
                    None,
                    StopToken::new(Unstoppable)
                )
                .is_err()
            );
        }
        let too_large = vec![0; MAX_FILE + 1];
        assert!(
            decode(&too_large, &Unstoppable, |_, _, _| panic!(
                "oversized input delivered pixels"
            ))
            .unwrap_err()
            .contains("file limit")
        );
    }

    #[test]
    fn serializer_patches_only_iloc_fields_and_validates_grid_geometry() {
        let marker = b"\xba\xad\xf0\x0d\xff\xff\xff\xff";
        let mut image = GridImage::new();
        image.set_icc_profile(marker.to_vec());
        let bytes = image
            .serialize(1, 1, 64, 64, 64, 64, &[marker], None)
            .unwrap();
        let parser = AvifParser::from_bytes(&bytes).unwrap();
        assert_eq!(&*parser.tile_data(0).unwrap(), marker);
        assert_eq!(
            parser.color_info(),
            Some(&zenavif_parse::ColorInformation::IccProfile(
                marker.to_vec()
            ))
        );
        assert!(image.serialize(0, 1, 64, 64, 64, 64, &[], None).is_err());
        assert!(
            image
                .serialize(1, 1, 65, 64, 64, 64, &[marker], None)
                .is_err()
        );
        assert!(
            image
                .serialize(1, 1, 64, 64, 64, 64, &[marker], Some(&[marker]))
                .is_err()
        );
    }

    #[test]
    fn grid_without_alpha_keeps_rgb16_in_full_and_tile_apis() {
        let source = encode(64, 64, 64, pixel, None, StopToken::new(Unstoppable)).unwrap();
        let parser = AvifParser::from_bytes(&source).unwrap();
        let color = parser.tile_data(0).unwrap();
        let mut image = GridImage::new();
        image.set_depth_bits(10).set_color_config(av1_config(
            &AV1Metadata::parse_av1_bitstream(&color).unwrap(),
        ));
        let bytes = image
            .serialize(1, 1, 64, 64, 64, 64, &[&color], None)
            .unwrap();
        let mut delivered = 0;
        let info = decode(&bytes, &Unstoppable, |x, y, pixels| {
            assert_eq!((x, y), (0, 0));
            assert!(pixels.try_as_imgref::<rgb::Rgb<u16>>().is_some());
            delivered += 1;
            Ok(())
        })
        .unwrap();
        assert!(!info.has_alpha);
        assert_eq!(info.bit_depth, 10);
        assert_eq!(delivered, 1);
        let mut decoder =
            zenavif::ManagedAvifDecoder::new(&bytes, &super::super::decode_config()).unwrap();
        let (pixels, info) = decoder.decode_full(&Unstoppable).unwrap();
        assert!(pixels.try_as_imgref::<rgb::Rgb<u16>>().is_some());
        assert_eq!((info.width, info.height, info.has_alpha), (64, 64, false));
    }
}
