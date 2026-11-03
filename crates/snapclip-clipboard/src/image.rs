//! Bounded original-resolution decoding, shared DIB framing; no OCR downscale policy.
use crate::{ImageEncoding, MAX_BYTES, MAX_PIXELS, ReadError};
use image::ImageDecoder as _;
use snapclip_model::pin::PinImage;

pub fn decode(bytes: &[u8], encoding: ImageEncoding) -> Result<PinImage, ReadError> {
    if bytes.len() > MAX_BYTES {
        return Err(ReadError::TooLarge);
    }
    let bmp;
    let bytes = if encoding == ImageEncoding::Dib {
        bmp = dib_to_bmp(bytes).map_err(|_| ReadError::InvalidImage)?;
        &bmp[..]
    } else {
        bytes
    };
    decode_encoded(bytes)
}

pub fn decode_encoded(bytes: &[u8]) -> Result<PinImage, ReadError> {
    if bytes.len() > MAX_BYTES {
        return Err(ReadError::TooLarge);
    }
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| ReadError::InvalidImage)?;
    let mut decoder = reader.into_decoder().map_err(|_| ReadError::InvalidImage)?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(ReadError::TooLarge);
    }
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_PIXELS * 8);
    decoder
        .set_limits(limits)
        .map_err(|_| ReadError::TooLarge)?;
    let rgba = image::DynamicImage::from_decoder(decoder)
        .map_err(|_| ReadError::InvalidImage)?
        .into_rgba8();
    PinImage::new(width, height, rgba.into_raw()).ok_or(ReadError::InvalidImage)
}

/// Wrap raw DIB bytes in a BITMAPFILEHEADER so standard BMP decoders can read them.
pub fn dib_to_bmp(dib: &[u8]) -> Result<Vec<u8>, String> {
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
    #[test]
    fn top_down_dib_preserves_dimensions() {
        let mut dib = vec![0; 40];
        dib[..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1920i32.to_le_bytes());
        dib[8..12].copy_from_slice(&(-1080i32).to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        let bmp = super::dib_to_bmp(&dib).unwrap();
        let reader = image::ImageReader::new(std::io::Cursor::new(bmp))
            .with_guessed_format()
            .unwrap();
        assert_eq!(reader.into_dimensions().unwrap(), (1920, 1080));
    }
}
