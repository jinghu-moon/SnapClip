//! Image encoding and decoding helpers (docs/23 T2.4).
//!
//! Moved from `src-tauri/src/infrastructure/image/encode.rs`: both of its users belong to
//! this crate — the artifact store encodes capture pixels to PNG, and the clipboard
//! adapter normalises clipboard images. Capture no longer knows how bytes become a PNG.
//!
//! Pure functions over byte buffers: no Win32, no GPU, no filesystem.

use std::io::Cursor;

/// Hard cap on decoded pixels to bound memory (~24MP ≈ 8K×3K). Every producer of
/// image payloads applies it before decoding.
pub const MAX_DECODE_PIXELS: u64 = 24_000_000;

/// A tightly packed 8-bit BGRA buffer, row-major, top-down, no row padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bgra8Image {
    width: u32,
    height: u32,
    bytes: Vec<u8>,
}

impl Bgra8Image {
    /// Validate dimensions and buffer length, returning `None` on mismatch.
    pub fn new(width: u32, height: u32, bytes: Vec<u8>) -> Option<Self> {
        let expected = u64::from(width) * u64::from(height) * 4;
        if width == 0 || height == 0 || expected != bytes.len() as u64 {
            return None;
        }
        Some(Self {
            width,
            height,
            bytes,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Number of pixels, always consistent with `bytes().len()`.
    pub fn pixel_count(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }
}

/// Encode BGRA pixels to PNG.
pub fn encode_png(image: &Bgra8Image) -> Result<Vec<u8>, String> {
    encode_rgba_png(&bgra_to_rgba(image.bytes()), image.width(), image.height())
}

/// Encode tight RGBA8 pixels to PNG.
pub fn encode_rgba_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let buffer = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| "invalid rgba buffer for png encoding".to_string())?;
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(buffer)
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|error| format!("encode png: {error}"))?;
    Ok(png)
}

/// Downscale tight RGBA8 pixels so the long side is at most `max_side`.
///
/// Returns the pixels unchanged when they already fit.
pub fn downscale_rgba_to_max_side(
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    max_side: u32,
) -> Result<(Vec<u8>, u32, u32), String> {
    let long_side = width.max(height);
    if long_side <= max_side {
        return Ok((rgba, width, height));
    }
    let buffer = image::RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| "invalid rgba buffer for downscale".to_string())?;
    let scale = max_side as f32 / long_side as f32;
    let target_width = ((width as f32 * scale).round() as u32).max(1);
    let target_height = ((height as f32 * scale).round() as u32).max(1);
    let resized = image::imageops::resize(
        &buffer,
        target_width,
        target_height,
        image::imageops::FilterType::Triangle,
    );
    Ok((resized.into_raw(), target_width, target_height))
}

/// Decode arbitrary encoded image bytes (PNG/BMP) into BGRA8.
pub fn decode_to_bgra8(bytes: &[u8]) -> Result<Bgra8Image, String> {
    let rgba = decode_to_rgba8(bytes)?;
    let (width, height) = rgba.dimensions();
    let mut bgra = rgba.into_raw();
    rgba_to_bgra(&mut bgra);
    Bgra8Image::new(width, height, bgra).ok_or_else(|| "decoded image has invalid dimensions".into())
}

/// Decode arbitrary encoded image bytes (PNG/BMP) into RGBA8.
pub fn decode_to_rgba8(bytes: &[u8]) -> Result<image::RgbaImage, String> {
    // The pixel cap is enforced from the header before any allocation, so a hostile
    // or corrupt header can never make the decoder allocate the full image.
    if let Some((width, height)) = png_dimensions(bytes)
        && u64::from(width) * u64::from(height) > MAX_DECODE_PIXELS
    {
        return Err("image exceeds max decode pixels".into());
    }
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| format!("guess image format: {error}"))?;
    let decoded = reader
        .decode()
        .map_err(|error| format!("decode image: {error}"))?;
    let (width, height) = (decoded.width(), decoded.height());
    if width == 0 || height == 0 {
        return Err("image has zero dimensions".into());
    }
    if u64::from(width) * u64::from(height) > MAX_DECODE_PIXELS {
        return Err("image exceeds max decode pixels".into());
    }
    Ok(decoded.to_rgba8())
}

/// Read width/height from a PNG IHDR chunk without decoding the image.
pub fn png_dimensions(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 {
        return None;
    }
    if png[..8] != [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        return None;
    }
    // IHDR is always the first chunk at offset 8: len(4) type(4) width(4) height(4)
    if &png[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(png[20..24].try_into().ok()?);
    (width > 0 && height > 0).then_some((width, height))
}

fn bgra_to_rgba(bgra: &[u8]) -> Vec<u8> {
    let mut rgba = bgra.to_vec();
    rgba_to_bgra(&mut rgba);
    rgba
}

/// Swaps the B and R lanes in place. The operation is its own inverse, which is why
/// one helper serves both directions.
fn rgba_to_bgra(bytes: &mut [u8]) {
    for pixel in bytes.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
}

#[cfg(test)]
mod tests {
    use super::{Bgra8Image, decode_to_bgra8, encode_png};

    fn solid_bgra(width: u32, height: u32, b: u8, g: u8, r: u8, a: u8) -> Bgra8Image {
        let mut bytes = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            bytes.extend_from_slice(&[b, g, r, a]);
        }
        Bgra8Image::new(width, height, bytes).unwrap()
    }

    #[test]
    fn rejects_buffers_that_do_not_match_the_dimensions() {
        assert!(Bgra8Image::new(2, 2, vec![0; 15]).is_none());
        assert!(Bgra8Image::new(0, 2, vec![]).is_none());
        assert!(Bgra8Image::new(2, 2, vec![0; 16]).is_some());
    }

    #[test]
    fn png_round_trip_preserves_pixels_and_channel_order() {
        let image = solid_bgra(3, 2, 0x10, 0x20, 0x30, 0xFF);
        let png = encode_png(&image).unwrap();
        let decoded = decode_to_bgra8(&png).unwrap();
        assert_eq!(decoded.width(), 3);
        assert_eq!(decoded.height(), 2);
        assert_eq!(decoded.bytes(), image.bytes());
    }

    #[test]
    fn decode_rejects_oversized_headers_before_allocating() {
        // Cheap PNG IHDR claiming 20000x20000 (~400MP > 24MP cap).
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&20_000u32.to_be_bytes());
        png.extend_from_slice(&20_000u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        let error = decode_to_bgra8(&png).unwrap_err();
        assert!(error.contains("max decode pixels"), "{error}");
    }
}
