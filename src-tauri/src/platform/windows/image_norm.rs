//! Normalize clipboard image bytes to PNG before storage/OCR.
//!
//! Only this module understands raw DIB/DIBV5. Store and OCR consume PNG only.

use std::io::Cursor;

/// Max long-side after optional downscale (configurable later; initial value).
const MAX_OCR_SIDE: u32 = 1920;
/// Hard cap on decoded pixels to bound memory (~24MP ≈ 8K×3K).
const MAX_DECODE_PIXELS: u64 = 24_000_000;

/// Decode raw CF_DIB / CF_DIBV5 clipboard bytes to PNG (downscaled if huge).
pub fn dib_to_png(dib: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let bmp = dib_to_bmp(dib)?;
    let reader = Cursor::new(bmp.as_slice());
    let mut img = image::ImageReader::new(reader)
        .with_guessed_format()
        .map_err(|e| format!("guess image format: {e}"))?
        .decode()
        .map_err(|e| format!("decode dib as bmp: {e}"))?;
    let (width, height) = (img.width(), img.height());
    if width == 0 || height == 0 {
        return Err("dib has zero dimensions".into());
    }
    if u64::from(width) * u64::from(height) > MAX_DECODE_PIXELS {
        return Err("image exceeds max decode pixels".into());
    }
    img = downscale(img);
    let width = img.width();
    let height = img.height();
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("encode png: {e}"))?;
    Ok((png, width, height))
}

/// Cheap PNG IHDR parse — avoids a full decode just for dimensions.
pub fn png_dimensions(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 {
        return None;
    }
    // PNG signature
    if png[..8] != [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        return None;
    }
    // IHDR is always first chunk at offset 8: len(4) type(4) width(4) height(4)
    if &png[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(png[20..24].try_into().ok()?);
    (width > 0 && height > 0).then_some((width, height))
}

fn downscale(img: image::DynamicImage) -> image::DynamicImage {
    let (w, h) = (img.width(), img.height());
    let long = w.max(h);
    if long <= MAX_OCR_SIDE {
        return img;
    }
    let scale = MAX_OCR_SIDE as f32 / long as f32;
    let nw = ((w as f32 * scale).round() as u32).max(1);
    let nh = ((h as f32 * scale).round() as u32).max(1);
    img.resize_exact(nw, nh, image::imageops::FilterType::Triangle)
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
    let _ = bit_count;

    // Pixel offset = file header (14) + DIB header.
    // BI_BITFIELDS on BITMAPINFOHEADER (40) has 3 masks AFTER the header (+12).
    // BITMAPV4 (108) / V5 (124) already contain masks inside the header.
    let mut offset = 14 + header_size;
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
    fn png_dimensions_reads_ihdr() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&32u32.to_be_bytes());
        png.extend_from_slice(&48u32.to_be_bytes());
        png.extend_from_slice(&[0; 5]);
        assert_eq!(png_dimensions(&png), Some((32, 48)));
    }
}
