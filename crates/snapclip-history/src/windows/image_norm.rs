//! Clipboard-only image normalisation.
//!
//! Windows publishes bitmaps as CF_DIB / CF_DIBV5 / CF_BITMAP. Only this module
//! understands those raw layouts; it converts them to PNG so the store, history and
//! OCR never see a Windows bitmap format. The generic codec work is delegated to
//! [`crate::image`].

use crate::image;

/// Max long side after optional downscale. Bounds what OCR has to chew on.
pub const MAX_OCR_SIDE: u32 = 1920;
/// Hard cap on decoded pixels to bound memory (~24MP ≈ 8K×3K).
pub const MAX_DECODE_PIXELS: u64 = image::MAX_DECODE_PIXELS;

/// Decode raw CF_DIB / CF_DIBV5 clipboard bytes to PNG (downscaled if huge).
pub fn dib_to_png(dib: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let (width, height) = dib_header_dimensions(dib)?;
    if u64::from(width) * u64::from(height) > MAX_DECODE_PIXELS {
        return Err("image exceeds max decode pixels".into());
    }
    let bmp = dib_to_bmp(dib)?;
    let rgba = image::decode_to_rgba8(&bmp).map_err(|error| format!("decode dib: {error}"))?;
    let (width, height) = rgba.dimensions();
    let (pixels, width, height) = image::downscale_rgba_to_max_side(
        rgba.into_raw(),
        width,
        height,
        MAX_OCR_SIDE,
    )?;
    let png = image::encode_rgba_png(&pixels, width, height)?;
    Ok((png, width, height))
}

/// Normalise a PNG payload: enforce the pixel cap and downscale the long side.
/// Dimensions come from the IHDR chunk, so small images are never decoded.
pub fn normalize_png(png: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let (width, height) = png_dimensions(png).ok_or_else(|| "invalid png header".to_string())?;
    if u64::from(width) * u64::from(height) > MAX_DECODE_PIXELS {
        return Err("image exceeds max decode pixels".into());
    }
    if width.max(height) <= MAX_OCR_SIDE {
        return Ok((png.to_vec(), width, height));
    }
    let rgba = image::decode_to_rgba8(png)?;
    let (width, height) = rgba.dimensions();
    let (pixels, width, height) = image::downscale_rgba_to_max_side(
        rgba.into_raw(),
        width,
        height,
        MAX_OCR_SIDE,
    )?;
    let png = image::encode_rgba_png(&pixels, width, height)?;
    Ok((png, width, height))
}

/// Cheap PNG IHDR parse — avoids a full decode just for dimensions.
pub fn png_dimensions(png: &[u8]) -> Option<(u32, u32)> {
    image::png_dimensions(png)
}

/// Width/height from BITMAPINFOHEADER without decoding pixels.
fn dib_header_dimensions(dib: &[u8]) -> Result<(u32, u32), String> {
    if dib.len() < 12 {
        return Err("dib too short".into());
    }
    let width = i32::from_le_bytes(dib[4..8].try_into().unwrap());
    let height = i32::from_le_bytes(dib[8..12].try_into().unwrap());
    if width <= 0 || height == 0 {
        return Err("invalid dib dimensions".into());
    }
    Ok((width as u32, height.unsigned_abs()))
}

/// Wrap raw DIB bytes in a BITMAPFILEHEADER so standard BMP decoders can read them.
fn dib_to_bmp(dib: &[u8]) -> Result<Vec<u8>, String> {
    if dib.len() < 40 {
        return Err("dib too short for BITMAPINFOHEADER".into());
    }
    let header_size = u32::from_le_bytes(dib[0..4].try_into().unwrap()) as usize;
    if header_size < 40 || header_size > dib.len() {
        return Err("invalid DIB header size".into());
    }
    let width = i32::from_le_bytes(dib[4..8].try_into().unwrap());
    let height = i32::from_le_bytes(dib[8..12].try_into().unwrap());
    if width <= 0 {
        return Err("invalid DIB width".into());
    }
    if height == 0 {
        return Err("invalid DIB height".into());
    }

    let bit_count = u16::from_le_bytes(dib[14..16].try_into().unwrap());
    let compression = u32::from_le_bytes(dib[16..20].try_into().unwrap());
    if compression != 0 && compression != 3 {
        return Err(format!("unsupported DIB compression: {compression}"));
    }

    let colors_used = u32::from_le_bytes(dib[32..36].try_into().unwrap()) as usize;
    let color_table_entries = if bit_count <= 8 {
        if colors_used > 0 {
            colors_used
        } else {
            1usize << bit_count
        }
    } else if header_size == 40 && colors_used > 0 {
        colors_used
    } else {
        0
    };

    // Pixel offset = file header (14) + DIB header + palette.
    // BI_BITFIELDS on BITMAPINFOHEADER (40) has 3 masks AFTER the header (+12).
    // BITMAPV4 (108) / V5 (124) already contain masks inside the header.
    let mut offset = 14usize
        .checked_add(header_size)
        .and_then(|value| value.checked_add(color_table_entries.checked_mul(4)?))
        .ok_or_else(|| "DIB pixel offset overflow".to_string())?;
    if compression == 3 && header_size == 40 {
        offset += 12;
    }

    let mut out = Vec::with_capacity(14 + dib.len());
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(offset as u32).to_le_bytes());
    out.extend_from_slice(dib);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_short_dib() {
        assert!(dib_to_png(&[0u8; 10]).is_err());
    }

    #[test]
    fn rejects_zero_width() {
        let mut dib = vec![0u8; 40];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        assert!(dib_to_png(&dib).is_err());
    }

    #[test]
    fn rejects_unsupported_compression() {
        let mut dib = vec![0u8; 40];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1i32.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        dib[16..20].copy_from_slice(&9u32.to_le_bytes());
        assert!(dib_to_png(&dib).unwrap_err().contains("compression"));
    }

    #[test]
    fn bitfields_info_header_adds_masks_offset() {
        let mut dib = vec![0u8; 52]; // 40 + 12 masks
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1i32.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        dib[16..20].copy_from_slice(&3u32.to_le_bytes());
        let bmp = dib_to_bmp(&dib).unwrap();
        let offset = u32::from_le_bytes(bmp[10..14].try_into().unwrap());
        assert_eq!(offset, 14 + 40 + 12);
    }

    #[test]
    fn bitfields_v5_does_not_add_masks_offset() {
        let mut dib = vec![0u8; 124];
        dib[0..4].copy_from_slice(&124u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1i32.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        dib[16..20].copy_from_slice(&3u32.to_le_bytes());
        let bmp = dib_to_bmp(&dib).unwrap();
        let offset = u32::from_le_bytes(bmp[10..14].try_into().unwrap());
        assert_eq!(offset, 14 + 124);
    }

    #[test]
    fn paletted_dib_includes_color_table_in_pixel_offset() {
        let mut dib = vec![0u8; 40 + 4 * 16];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&4u16.to_le_bytes());
        let bmp = dib_to_bmp(&dib).unwrap();
        let offset = u32::from_le_bytes(bmp[10..14].try_into().unwrap());
        assert_eq!(offset, 14 + 40 + 16 * 4);
    }

    #[test]
    fn png_dimensions_reads_ihdr() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&32u32.to_be_bytes());
        png.extend_from_slice(&48u32.to_be_bytes());
        png.extend_from_slice(&[0; 5]);
        assert_eq!(png_dimensions(&png), Some((32, 48)));
    }

    #[test]
    fn normalize_png_rejects_huge_ihdr_before_decode() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        // ~400MP > 24MP cap — must fail from the header alone.
        png.extend_from_slice(&20_000u32.to_be_bytes());
        png.extend_from_slice(&20_000u32.to_be_bytes());
        png.extend_from_slice(&[0; 5]);
        assert!(
            normalize_png(&png)
                .unwrap_err()
                .contains("max decode pixels")
        );
    }

    #[test]
    fn normalize_png_keeps_small_png_without_decoding() {
        // Valid IHDR beyond the pixel fields is not needed: the small-image path
        // returns the input untouched.
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&16u32.to_be_bytes());
        png.extend_from_slice(&16u32.to_be_bytes());
        png.extend_from_slice(&[0; 5]);
        let (out, w, h) = normalize_png(&png).unwrap();
        assert_eq!((w, h), (16, 16));
        assert_eq!(out, png);
    }

    #[test]
    fn dib_header_rejects_oversize_before_decode() {
        let mut dib = vec![0u8; 40];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&20_000i32.to_le_bytes());
        dib[8..12].copy_from_slice(&20_000i32.to_le_bytes());
        assert!(dib_to_png(&dib).unwrap_err().contains("max decode pixels"));
    }

    #[test]
    fn dib_round_trip_produces_a_readable_png() {
        // 2x1 32bpp BI_RGB DIB: bottom-up rows, BGRA pixels.
        let mut dib = vec![0u8; 40];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&2i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&[0x10, 0x20, 0x30, 0xFF, 0x40, 0x50, 0x60, 0xFF]);
        let (png, width, height) = dib_to_png(&dib).unwrap();
        assert_eq!((width, height), (2, 1));
        assert_eq!(png_dimensions(&png), Some((2, 1)));
        let decoded = crate::image::decode_to_bgra8(&png).unwrap();
        assert_eq!(decoded.bytes(), &[0x10, 0x20, 0x30, 0xFF, 0x40, 0x50, 0x60, 0xFF]);
    }
}
