//! Bounded, local image-file preparation. Called only from background tasks.
use crate::PinImage;
use std::io::Read as _;
use yue_component::{
    ImagePreviewColorSpace, ImagePreviewImage, ImagePreviewLimits, ImagePreviewPixels,
};

/// GPUI consumes BGRA; keep the immutable RGBA source for clipboard copying.
/// This runs on the worker, without a PNG encode/decode round trip.
pub(crate) fn render(image: &PinImage) -> Result<ImagePreviewImage, String> {
    ImagePreviewImage::from_rgba8(image.width(), image.height(), image.rgba(), "贴图图片")
        .map_err(|error| error.to_string())
}

pub(crate) fn load(path: &str) -> Result<PinImage, String> {
    if path.starts_with("\\\\") {
        return Err("网络图片请先保存到本地再预览".into());
    }
    let mut bytes = vec![];
    std::fs::File::open(path)
        .map_err(|_| "无法读取图片文件")?
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取图片文件")?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err("图片文件超过 64 MiB".into());
    }
    decode(&bytes)
}
fn decode(bytes: &[u8]) -> Result<PinImage, String> {
    let limits = ImagePreviewLimits::default()
        .max_pixels(24_000_000)
        .max_decode_bytes(24_000_000 * 8);
    let decoded = ImagePreviewPixels::decode(bytes, &limits).map_err(|e| e.to_string())?;
    let [w, h] = decoded.pixel_size();
    if decoded.color_space() == ImagePreviewColorSpace::ScRgb {
        return Err("当前贴图像素模型仅支持 SDR；HDR 原图未被转换或截断".into());
    }
    // The current pin model is an SDR copy source. Converting high-precision
    // SDR is explicit; the file identity still references the original file.
    PinImage::new(w, h, decoded.into_sdr_rgba8()).ok_or_else(|| "图片为空".into())
}

pub(crate) fn encode(image: &PinImage, thumbnail: bool) -> Result<Vec<u8>, String> {
    let rgba = image::RgbaImage::from_raw(image.width(), image.height(), image.rgba().to_vec())
        .ok_or("图片尺寸错误")?;
    let image = image::DynamicImage::ImageRgba8(rgba);
    let image = if thumbnail {
        image.thumbnail(768, 384)
    } else {
        image
    };
    let mut out = std::io::Cursor::new(vec![]);
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|_| "无法准备图片预览")?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sixteen_bit_sdr_has_an_explicit_eight_bit_copy_derivative() {
        let pixels =
            image::ImageBuffer::from_pixel(1, 1, image::Rgba([0x4000_u16, 0x8080, 0xc000, 0xffff]));
        let mut encoded = std::io::Cursor::new(vec![]);
        image::DynamicImage::ImageRgba16(pixels)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let image = decode(encoded.get_ref()).unwrap();
        assert_eq!(image.rgba(), [64, 128, 191, 255]);
    }
    #[test]
    fn render_converts_channels_once_without_changing_source_or_alpha() {
        let rgba = vec![180, 70, 17, 0, 45, 21, 93, 127];
        let source = PinImage::new(2, 1, rgba.clone()).unwrap();
        let rendered = render(&source).unwrap();
        assert_eq!(rendered.image().frame_count(), 1);
        assert_eq!(
            rendered.image().as_bytes(0).unwrap(),
            [17, 70, 180, 0, 93, 21, 45, 127]
        );
        assert_eq!(source.rgba(), &rgba);
    }
    #[test]
    fn local_file_preview_keeps_original_pixels_and_alpha() {
        let original = PinImage::new(2, 1, vec![255, 0, 0, 0, 0, 255, 0, 127]).unwrap();
        let encoded = encode(&original, false).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.rgba(), original.rgba());
        let file =
            std::env::temp_dir().join(format!("snapclip-pin-preview-{}.png", std::process::id()));
        std::fs::write(&file, &encoded).unwrap();
        let loaded = load(file.to_str().unwrap()).unwrap();
        std::fs::remove_file(file).unwrap();
        assert_eq!(loaded.rgba(), original.rgba());
        assert_eq!((loaded.width(), loaded.height()), (2, 1));
        assert!(decode(b"not an image").is_err());
        assert!(
            load(r"\\never-contact-this-server\share\image.png")
                .unwrap_err()
                .contains("网络")
        );
    }
    #[test]
    fn thumbnail_is_derived_and_does_not_replace_original_pixels() {
        let original = PinImage::new(1600, 900, vec![128; 1600 * 900 * 4]).unwrap();
        let thumb = decode(&encode(&original, true).unwrap()).unwrap();
        assert!(thumb.width() <= 768 && thumb.height() <= 384);
        assert_eq!((original.width(), original.height()), (1600, 900));
    }
    #[test]
    fn dimensions_are_rejected_before_decoding_pixel_data() {
        let mut header = vec![0u8; 54];
        header[..2].copy_from_slice(b"BM");
        header[2..6].copy_from_slice(&54u32.to_le_bytes());
        header[10..14].copy_from_slice(&54u32.to_le_bytes());
        header[14..18].copy_from_slice(&40u32.to_le_bytes());
        header[18..22].copy_from_slice(&6000i32.to_le_bytes());
        header[22..26].copy_from_slice(&4001i32.to_le_bytes());
        header[26..28].copy_from_slice(&1u16.to_le_bytes());
        header[28..30].copy_from_slice(&24u16.to_le_bytes());
        assert!(decode(&header).unwrap_err().contains("2400"));
    }
    #[test]
    fn long_image_proxy_keeps_copy_pixels_and_original_dimensions() {
        let pixels = [180, 70, 17, 127].repeat(10000);
        let image = PinImage::new(10000, 1, pixels.clone()).unwrap();
        let preview = render(&image).unwrap();
        assert_eq!(preview.pixel_size(), [10000, 1]);
        assert_eq!(preview.texture_size(), [8192, 1]);
        assert!(preview.is_proxy());
        assert_eq!(image.rgba(), pixels);
    }
}
