use crate::{Error, Result, Stop, StopToken, check};
use rgb::Rgba;
use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};
use zenavif_parse::{AV1Metadata, AvifParser};
use zenavif_serialize::{Av1CBox, ColrBox, constants::*, grid::GridImage};

pub const TILE_SIDE: usize = 128;
pub const MAX_SOURCE_READ_BYTES: usize = 1024 * 1024;
const MAX_TILES: usize = 4096;
const MAX_PAYLOAD: u64 = 512 * 1024 * 1024;

/// Implementation budget, not an AVIF specification limit. Matches the row
/// source and 128-row conversion cache before any pixel or disk allocation.
pub fn within_export_geometry(width: u64, height: u64) -> bool {
    width > 0
        && height > 0
        && width <= u32::MAX as u64
        && height <= u32::MAX as u64
        && width.div_ceil(TILE_SIDE as u64) <= 255
        && height.div_ceil(TILE_SIDE as u64) <= 255
        && width.div_ceil(TILE_SIDE as u64) * height.div_ceil(TILE_SIDE as u64) <= MAX_TILES as u64
        && width * 8 <= MAX_SOURCE_READ_BYTES as u64
        && width * 8 * height.min(TILE_SIDE as u64) < 16 * 1024 * 1024
}

fn av1_config(meta: &AV1Metadata) -> Av1CBox {
    let mut cfg = Av1CBox::default();
    cfg.seq_profile = meta.seq_profile;
    cfg.high_bitdepth = meta.bit_depth > 8;
    cfg.twelve_bit = meta.bit_depth == 12;
    cfg.monochrome = meta.monochrome;
    cfg.chroma_subsampling_x = meta.chroma_subsampling.horizontal;
    cfg.chroma_subsampling_y = meta.chroma_subsampling.vertical;
    cfg
}
fn config() -> zenavif::EncoderConfig {
    use zenavif::*;
    EncoderConfig::new()
        .quality(100.)
        .alpha_quality(100.)
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

/// Encode known full-range Rec.2020/PQ straight RGBA16 tiles. The provider fills
/// exactly 128×128 samples, repeating the final edge for padded pixels. Only
/// compressed data is spooled; temporary files close/delete on every exit.
/// The caller discards partial output on error and owns atomic publication.
pub fn write_pq_tiles(
    width: u32,
    height: u32,
    scratch: &Path,
    output: &mut impl Write,
    stop: StopToken,
    mut fill: impl FnMut(u32, u32, &mut Vec<Rgba<u16>>) -> Result<()>,
) -> Result<u64> {
    check(&stop)?;
    let columns = (width as usize).div_ceil(TILE_SIDE);
    let rows = (height as usize).div_ceil(TILE_SIDE);
    let count = rows.checked_mul(columns).ok_or("grid geometry overflow")?;
    if width == 0 || height == 0 || rows > 255 || columns > 255 || count > MAX_TILES {
        return Err("grid exceeds bounded geometry".into());
    }
    let mut colors = tempfile::tempfile_in(scratch)?;
    let mut alphas = tempfile::tempfile_in(scratch)?;
    let mut color_lengths = Vec::with_capacity(count);
    let mut alpha_lengths = Vec::with_capacity(count);
    let mut tile = Vec::with_capacity(TILE_SIDE * TILE_SIDE);
    let mut total = 0u64;
    let mut image = GridImage::new();
    let mut colr = ColrBox::default();
    colr.color_primaries = ColorPrimaries::Bt2020;
    colr.transfer_characteristics = TransferCharacteristics::Smpte2084;
    colr.matrix_coefficients = MatrixCoefficients::Rgb;
    image.set_colr(colr).set_depth_bits(10);
    for row in 0..rows {
        for column in 0..columns {
            check(&stop)?;
            tile.clear();
            fill(
                (column * TILE_SIDE) as u32,
                (row * TILE_SIDE) as u32,
                &mut tile,
            )?;
            if tile.len() != TILE_SIDE * TILE_SIDE {
                return Err("source returned an incomplete grid tile".into());
            }
            check(&stop)?;
            let encoded = zenavif::encode_rgba16(
                imgref::Img::new(tile.as_slice(), TILE_SIDE, TILE_SIDE),
                &config(),
                stop.clone(),
            )
            .map_err(|e| {
                if stop.check().is_err() {
                    Error::Cancelled
                } else {
                    Error::Invalid(e.to_string())
                }
            })?;
            let parser = AvifParser::from_bytes(&encoded.avif_file).map_err(|e| e.to_string())?;
            let color = parser.primary_data().map_err(|e| e.to_string())?;
            let alpha = parser
                .alpha_data()
                .ok_or("tile lost alpha")?
                .map_err(|e| e.to_string())?;
            total = total
                .checked_add(color.len() as u64)
                .and_then(|v| v.checked_add(alpha.len() as u64))
                .filter(|&v| v <= MAX_PAYLOAD)
                .ok_or("compressed AVIF scratch limit exceeded")?;
            if color_lengths.is_empty() {
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
            check(&stop)?;
            colors.write_all(&color)?;
            alphas.write_all(&alpha)?;
            color_lengths.push(u32::try_from(color.len()).map_err(|_| "color tile overflow")?);
            alpha_lengths.push(u32::try_from(alpha.len()).map_err(|_| "alpha tile overflow")?);
        }
    }
    check(&stop)?;
    let header = image.header(
        rows as u8,
        columns as u8,
        width,
        height,
        TILE_SIDE as u32,
        TILE_SIDE as u32,
        &color_lengths,
        Some(&alpha_lengths),
    )?;
    output.write_all(&header)?;
    let mut scratch = [0u8; 64 * 1024];
    for (file, lengths) in [(&mut colors, &color_lengths), (&mut alphas, &alpha_lengths)] {
        let mut remaining = lengths.iter().map(|&v| u64::from(v)).sum::<u64>();
        if file.metadata()?.len() != remaining {
            return Err("AVIF scratch length changed".into());
        }
        file.seek(SeekFrom::Start(0))?;
        while remaining > 0 {
            check(&stop)?;
            let count = remaining.min(scratch.len() as u64) as usize;
            file.read_exact(&mut scratch[..count])?;
            output.write_all(&scratch[..count])?;
            remaining -= count as u64;
        }
    }
    check(&stop)?;
    Ok(total + header.len() as u64)
}
