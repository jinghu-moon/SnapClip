use super::*;
use rgb::Rgba;
use std::{
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use zenavif::{EncodeAlphaMode, EncodeBitDepth, EncodeColorModel, EncodePixelRange, EncoderConfig};
use zenavif_parse::AvifParser;
use zenavif_serialize::{Av1CBox, ColrBox, constants::*, grid::GridImage};

#[derive(Clone, Default)]
struct Flag(Arc<AtomicBool>);
impl Stop for Flag {
    fn check(&self) -> std::result::Result<(), enough::StopReason> {
        if self.0.load(Ordering::Relaxed) {
            Err(enough::StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}
fn config(depth: EncodeBitDepth) -> EncoderConfig {
    EncoderConfig::new()
        .quality(100.)
        .alpha_quality(100.)
        .speed(10)
        .bit_depth(depth)
        .color_model(EncodeColorModel::Rgb)
        .alpha_color_mode(EncodeAlphaMode::UnassociatedDirty)
        .color_primaries(1)
        .transfer_characteristics(13)
        .matrix_coefficients(0)
        .pixel_range(EncodePixelRange::Full)
        .threads(Some(1))
}
fn single(depth: EncodeBitDepth, pixel: Rgba<u16>) -> Vec<u8> {
    let data = vec![pixel; 64 * 64];
    zenavif::encode_rgba16(
        imgref::Img::new(data.as_slice(), 64, 64),
        &config(depth),
        StopToken::new(Unstoppable),
    )
    .unwrap()
    .avif_file
}
fn av1_config(meta: &zenavif_parse::AV1Metadata) -> Av1CBox {
    let mut cfg = Av1CBox::default();
    cfg.seq_profile = meta.seq_profile;
    cfg.high_bitdepth = meta.bit_depth > 8;
    cfg.twelve_bit = meta.bit_depth == 12;
    cfg.monochrome = meta.monochrome;
    cfg.chroma_subsampling_x = meta.chroma_subsampling.horizontal;
    cfg.chroma_subsampling_y = meta.chroma_subsampling.vertical;
    cfg
}
fn grid(depth: EncodeBitDepth, alpha: bool, premult: bool, icc: Option<Vec<u8>>) -> Vec<u8> {
    let tile = single(depth, Rgba::new(16384, 8192, 4096, 32768));
    let parser = AvifParser::from_bytes(&tile).unwrap();
    let c = parser.primary_data().unwrap();
    let a = parser.alpha_data().unwrap().unwrap();
    let mut image = GridImage::new();
    let meta = parser.primary_metadata().unwrap();
    image
        .set_depth_bits(meta.bit_depth)
        .set_color_config(av1_config(&meta))
        .set_alpha_config(av1_config(&parser.alpha_metadata().unwrap().unwrap()))
        .set_premultiplied_alpha(premult);
    let mut colr = ColrBox::default();
    colr.color_primaries = ColorPrimaries::Bt2020;
    colr.transfer_characteristics = TransferCharacteristics::Smpte2084;
    colr.matrix_coefficients = MatrixCoefficients::Rgb;
    image.set_colr(colr);
    if let Some(icc) = icc {
        image.set_icc_profile(icc);
    }
    let colors = [c.as_ref(); 6];
    let alphas = [a.as_ref(); 6];
    image
        .serialize(2, 3, 133, 71, 64, 64, &colors, alpha.then_some(&alphas))
        .unwrap()
}
fn channel(pixels: &snapclip_model::pixels::PixelBuffer, at: usize) -> f32 {
    half::f16::from_le_bytes(pixels.as_bytes()[at * 2..at * 2 + 2].try_into().unwrap()).to_f32()
}

#[test]
fn premultiplied_grid_is_unassociated_exactly_once() {
    let bytes = grid(
        EncodeBitDepth::Ten,
        true,
        true,
        Some(moxcms::ColorProfile::new_srgb().encode().unwrap()),
    );
    let pixels = decode(&bytes, &Unstoppable).unwrap();
    // Native codec has already divided encoded RGB by coverage. The shell
    // must not divide a second time before the transfer function.
    let expected = yue_color::srgb_to_linear(0.5) as f32;
    assert!(
        (channel(&pixels, 0) - expected).abs() < 0.003,
        "{} vs {expected}",
        channel(&pixels, 0)
    );
    assert!((channel(&pixels, 3) - 0.5).abs() < 0.002);
}

#[test]
fn grid_sdr_icc_overrides_pq_nclx_and_preserves_edges_and_alpha() {
    for (depth, tolerance) in [(EncodeBitDepth::Eight, 0.005), (EncodeBitDepth::Ten, 0.002)] {
        for alpha in [false, true] {
            let bytes = grid(
                depth,
                alpha,
                false,
                Some(moxcms::ColorProfile::new_srgb().encode().unwrap()),
            );
            let pixels = decode(&bytes, &Unstoppable).unwrap();
            assert_eq!(pixels.size(), [133, 71]);
            assert_eq!(
                pixels.descriptor().encoding(),
                snapclip_model::pixels::PixelEncoding::LinearSrgb
            );
            for at in [0, 132, 133 * 70 + 132] {
                assert!(
                    (channel(&pixels, at * 4) - yue_color::srgb_to_linear(0.25) as f32).abs()
                        < tolerance
                );
                assert!(
                    (channel(&pixels, at * 4 + 3) - if alpha { 0.5 } else { 1. }).abs() < tolerance
                );
            }
        }
    }
}

#[test]
fn matrix_icc_converts_display_p3_to_relative_linear_rec709() {
    let bytes = grid(
        EncodeBitDepth::Ten,
        true,
        false,
        Some(moxcms::ColorProfile::new_display_p3().encode().unwrap()),
    );
    let pixels = decode(&bytes, &Unstoppable).unwrap();
    let p3 = [0.25_f64, 0.125, 0.0625].map(yue_color::srgb_to_linear);
    // Independent D65 Display-P3 -> Rec.709 matrix; the profile has D50 PCS
    // and CMS must handle the chromatic adaptation on both sides.
    let matrix = [
        [1.224745, -0.224904, 0.],
        [-0.042058, 1.042081, 0.],
        [-0.019642, -0.078655, 1.098537],
    ];
    for (channel_ix, row) in matrix.iter().enumerate() {
        let expected = row.iter().zip(p3).map(|(a, b)| a * b).sum::<f64>() as f32;
        assert!((channel(&pixels, channel_ix) - expected).abs() < 0.001);
    }
    assert_eq!(
        pixels.descriptor().encoding(),
        snapclip_model::pixels::PixelEncoding::LinearSrgb
    );
}

#[test]
fn icc_pq_is_absolute_but_hlg_and_other_hdr_gamuts_are_explicitly_rejected() {
    let mut profile = moxcms::ColorProfile::new_bt2020_pq();
    // CMS's convenience constructor leaves video-range/matrix defaults. An
    // RGB ICC fixture must explicitly declare Identity/full-range; the
    // ambiguous constructor output remains rejected, rather than relaxing it.
    let ambiguous = grid(
        EncodeBitDepth::Ten,
        true,
        false,
        Some(profile.encode().unwrap()),
    );
    assert!(decode(&ambiguous, &Unstoppable).is_err());
    let cicp = profile.cicp.as_mut().unwrap();
    cicp.matrix_coefficients = moxcms::MatrixCoefficients::Identity;
    cicp.full_range = true;
    let bytes = grid(
        EncodeBitDepth::Ten,
        true,
        false,
        Some(profile.encode().unwrap()),
    );
    let pixels = decode(&bytes, &Unstoppable).unwrap();
    assert_eq!(
        pixels.descriptor().encoding(),
        snapclip_model::pixels::PixelEncoding::ScRgb
    );
    let pq = [0.25, 0.125, 0.0625].map(|v| yue_color::pq_to_nits(v) / 80.);
    let expected = yue_color::rec2020_to_rec709(pq);
    for (ix, value) in expected.into_iter().enumerate() {
        assert!((channel(&pixels, ix) - value as f32).abs() < 0.003);
    }
    for profile in [
        moxcms::ColorProfile::new_bt2020_hlg(),
        moxcms::ColorProfile::new_display_p3_pq(),
    ] {
        let bytes = grid(
            EncodeBitDepth::Ten,
            true,
            false,
            Some(profile.encode().unwrap()),
        );
        assert!(decode(&bytes, &Unstoppable).is_err());
    }
}

#[test]
fn export_geometry_budget_matches_row_band_storage_and_4096_tiles() {
    assert!(within_export_geometry(8256, 5504));
    for (width, height) in [
        (0, 1),
        (1, 0),
        (16384, 128),
        (8192, 8193),
        (1, 32641),
        (u64::MAX, 1),
    ] {
        assert!(!within_export_geometry(width, height), "{width}x{height}");
    }
    assert!(within_export_geometry(16383, 128));
}

#[test]
fn single_srgb_stays_relative_and_pq_spool_is_absolute() {
    for depth in [EncodeBitDepth::Eight, EncodeBitDepth::Ten] {
        let bytes = single(depth, Rgba::new(32768, 16384, 8192, 32768));
        let p = decode(&bytes, &Unstoppable).unwrap();
        assert_eq!(
            p.descriptor().encoding(),
            snapclip_model::pixels::PixelEncoding::LinearSrgb
        );
        assert!((channel(&p, 0) - yue_color::srgb_to_linear(0.5) as f32).abs() < 0.004);
    }
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = Vec::new();
    let len = write_pq_tiles(
        133,
        71,
        dir.path(),
        &mut bytes,
        StopToken::new(Unstoppable),
        |_, _, tile| {
            tile.resize(TILE_SIDE * TILE_SIDE, Rgba::new(49271, 49271, 49271, 32768));
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(len as usize, bytes.len());
    let p = decode(&bytes, &Unstoppable).unwrap();
    assert_eq!(
        p.descriptor().encoding(),
        snapclip_model::pixels::PixelEncoding::ScRgb
    );
    assert!((channel(&p, 0) - 12.5).abs() < 0.08);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn spool_cleans_up_on_sink_source_and_cancellation_errors() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::StorageFull, "disk full"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let fill = |_: u32, _: u32, tile: &mut Vec<Rgba<u16>>| {
        tile.resize(TILE_SIDE * TILE_SIDE, Rgba::new(0, 0, 0, 65535));
        Ok(())
    };
    let err = write_pq_tiles(
        1,
        1,
        dir.path(),
        &mut Broken,
        StopToken::new(Unstoppable),
        fill,
    )
    .unwrap_err();
    assert_eq!(err.into_io().kind(), io::ErrorKind::StorageFull);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    let mut output = Vec::new();
    let mut calls = 0;
    let err = write_pq_tiles(
        129,
        1,
        dir.path(),
        &mut output,
        StopToken::new(Unstoppable),
        |x, y, tile| {
            calls += 1;
            if calls == 2 {
                return Err("source lost".into());
            }
            fill(x, y, tile)
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("source lost"));
    assert!(output.is_empty());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    let stop = Flag::default();
    let err = write_pq_tiles(
        1,
        1,
        dir.path(),
        &mut output,
        StopToken::new(stop.clone()),
        |x, y, tile| {
            fill(x, y, tile)?;
            stop.0.store(true, Ordering::Relaxed);
            Ok(())
        },
    )
    .unwrap_err();
    assert!(matches!(err, Error::Cancelled));
    assert!(output.is_empty());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    assert!(
        write_pq_tiles(
            1,
            1,
            &dir.path().join("missing"),
            &mut output,
            StopToken::new(Unstoppable),
            fill
        )
        .is_err()
    );
}

#[test]
fn streamed_header_matches_complete_serializer_and_rejects_offset_overflow() {
    for alpha in [false, true] {
        let mut image = GridImage::new();
        image.set_alpha_config(Av1CBox::default());
        let colors = [b"1234".as_slice(), b"567890".as_slice()];
        let alphas = [b"ab".as_slice(), b"cde".as_slice()];
        let all = image
            .serialize(1, 2, 129, 128, 128, 128, &colors, alpha.then_some(&alphas))
            .unwrap();
        let mut stream = image
            .header(1, 2, 129, 128, 128, 128, &[4, 6], alpha.then_some(&[2, 3]))
            .unwrap();
        for bytes in colors
            .into_iter()
            .chain(alpha.then_some(alphas).into_iter().flatten())
        {
            stream.extend_from_slice(bytes);
        }
        assert_eq!(stream, all);
        assert!(
            image
                .header(1, 2, 129, 128, 128, 128, &[u32::MAX, 1], None)
                .is_err()
        );
        assert!(image.header(0, 2, 129, 128, 128, 128, &[], None).is_err());
    }
}

#[test]
fn decode_keeps_cancellation_typed_before_and_during_parsing_or_tiles() {
    use std::sync::atomic::AtomicUsize;
    struct After(AtomicUsize, usize);
    impl Stop for After {
        fn check(&self) -> std::result::Result<(), enough::StopReason> {
            if self.0.fetch_add(1, Ordering::Relaxed) >= self.1 {
                Err(enough::StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }
    let bytes = grid(EncodeBitDepth::Ten, true, false, None);
    for after in [0, 5, 35, 70, 200] {
        let stop = After(AtomicUsize::new(0), after);
        assert!(
            matches!(decode(&bytes, &stop), Err(Error::Cancelled)),
            "after {after}"
        );
    }
    for end in [0, 4, 8, 16, bytes.len() / 2, bytes.len() - 1] {
        assert!(decode(&bytes[..end], &Unstoppable).is_err());
    }
}
