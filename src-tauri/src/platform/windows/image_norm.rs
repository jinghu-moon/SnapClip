//! Normalize clipboard image bytes to PNG before storage/OCR.
//!
//! Only this module understands raw DIB/DIBV5. Store and OCR consume PNG only.

use std::io::Cursor;

/// Decode raw CF_DIB / CF_DIBV5 clipboard bytes to PNG.
/// Returns Err when the DIB is unusable (caller drops the image payload).
pub fn dib_to_png(dib: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let bmp = dib_to_bmp(dib)?;
    let reader = Cursor::new(bmp.as_slice());
    let img = image::ImageReader::new(reader)
        .with_guessed_format()
        .map_err(|e| format!("guess image format: {e}"))?
        .decode()
        .map_err(|e| format!("decode dib as bmp: {e}"))?;
    let width = img.width();
    let height = img.height();
    if width == 0 || height == 0 {
        return Err("dib has zero dimensions".into());
    }
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("encode png: {e}"))?;
    Ok((png, width, height))
}

/// Dimensions from a PNG payload (image_dimensions may have been empty).
pub fn png_dimensions(png: &[u8]) -> Option<(u32, u32)> {
    let reader = Cursor::new(png);
    let img = image::ImageReader::new(reader)
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let (w, h) = (img.width(), img.height());
    (w > 0 && h > 0).then_some((w, h))
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
    // Height may be negative (top-down); absolute value is the pixel height.
    if height == 0 {
        return Err("invalid DIB height".into());
    }

    let bit_count = u16::from_le_bytes(dib[14..16].try_into().unwrap());
    let compression = u32::from_le_bytes(dib[16..20].try_into().unwrap());
    // BI_RGB=0, BI_BITFIELDS=3 are common for clipboard DIBs.
    if compression != 0 && compression != 3 {
        return Err(format!("unsupported DIB compression: {compression}"));
    }

    let mut out = Vec::with_capacity(14 + dib.len());
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    // Pixel data offset: file header (14) + DIB header + optional masks.
    let mut offset = 14 + header_size;
    if compression == 3 {
        // BI_BITFIELDS: 3 (or 4) DWORD color masks follow the header.
        offset += 12;
    }
    let _ = bit_count;
    out.extend_from_slice(&(offset as u32).to_le_bytes());
    out.extend_from_slice(dib);
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn rejects_short_dib() {
        assert!(super::dib_to_png(&[0u8; 10]).is_err());
    }

    #[test]
    fn rejects_zero_width() {
        let mut dib = vec![0u8; 40];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        assert!(super::dib_to_png(&dib).is_err());
    }
}
