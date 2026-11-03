//! Source identity and bounded row conversion shared by every AVIF caller.
use crate::{MAX_SOURCE_READ_BYTES, Result, StopToken, TILE_SIDE, check, write_pq_tiles};
use rgb::Rgba;
use snapclip_model::pixels::{BandSource, PixelBuffer, PixelEncoding};
use std::{io::Write, path::Path};
const READ_BYTES: usize = MAX_SOURCE_READ_BYTES;
const BAND_BYTES: usize = 16 * 1024 * 1024;

/// Stream an immutable absolute FP16/scRGB row source as Rec.2020/PQ AVIF.
/// Caller owns publication, scratch location, cancellation and background work.
pub fn write_hdr(
    bands: &mut dyn BandSource,
    width: u32,
    scratch: &Path,
    output: &mut impl Write,
    stop: StopToken,
) -> Result<u64> {
    check(&stop)?;
    let source = bands.source();
    let descriptor = source.descriptor();
    let height = bands.rows();
    if descriptor.encoding() != PixelEncoding::ScRgb {
        return Err("HDR grid requires an absolute scRGB source".into());
    }
    let row = descriptor.row_bytes(width).ok_or("source row overflow")?;
    let band_rows = (height as usize).min(TILE_SIDE);
    let band_bytes = row.checked_mul(band_rows).ok_or("source band overflow")?;
    if width == 0 || height == 0 || row > READ_BYTES || band_bytes >= BAND_BYTES {
        return Err("source exceeds the bounded row-band budget".into());
    }
    let mut cached = Vec::new();
    let mut cached_top = None;
    let mut raw = Vec::new();
    let stable = |bands: &dyn BandSource| bands.source() == source && bands.rows() == height;
    let bytes = write_pq_tiles(
        width,
        height,
        scratch,
        output,
        stop.clone(),
        |left, top, tile| {
            let (left, top) = (left as usize, top as usize);
            if !stable(bands) {
                return Err("pixel source changed during grid export".into());
            }
            let count = (height as usize - top).min(TILE_SIDE);
            if cached_top != Some(top) {
                cached.clear();
                cached
                    .try_reserve_exact(width as usize * count)
                    .map_err(|e| e.to_string())?;
                let end = top + count;
                let mut first = top;
                while first < end {
                    check(&stop)?;
                    let rows = (READ_BYTES / row).min(end - first);
                    raw.clear();
                    if !stable(bands)
                        || !bands.read_rows(first as u32, rows as u32, &mut raw)
                        || !stable(bands)
                    {
                        return Err("pixel source changed or refused a grid band".into());
                    }
                    check(&stop)?;
                    let pixels = PixelBuffer::from_tight(
                        width,
                        rows as u32,
                        descriptor,
                        std::mem::take(&mut raw),
                    )
                    .map_err(str::to_owned)?;
                    let converted = snapclip_model::pixel_color::rec2020_pq16_pixels(&pixels)
                        .map_err(str::to_owned)?;
                    for (ix, pixel) in converted.enumerate() {
                        if ix % width as usize == 0 {
                            check(&stop)?;
                        }
                        let [r, g, b, a] = pixel.map_err(str::to_owned)?;
                        cached.push(Rgba::new(r, g, b, a));
                    }
                    raw = pixels.into_bytes();
                    first += rows;
                }
                cached_top = Some(top);
            }
            for y in 0..TILE_SIDE {
                let row = y.min(count - 1) * width as usize;
                for x in 0..TILE_SIDE {
                    tile.push(cached[row + (left + x).min(width as usize - 1)]);
                }
            }
            Ok(())
        },
    )?;
    if !stable(bands) {
        return Err("pixel source changed before completing grid export".into());
    }
    check(&stop)?;
    Ok(bytes)
}
