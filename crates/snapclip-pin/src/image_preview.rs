//! Bounded, local image-file preparation. Called only from background tasks.
use crate::PinImage;
use snapclip_model::pixels::{AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding, PixelSource};
use std::io::Read as _;
use std::{borrow::Cow, sync::Arc};
use yue_component::{
    ImagePreviewColorSpace, ImagePreviewImage, ImagePreviewLimits, ImagePreviewPixels,
};

/// Prepare a display derivative on the worker. The shared immutable source
/// retains its precision, absolute luminance, alpha and acquisition generation.
pub(crate) fn render(image: &PinImage) -> Result<ImagePreviewImage, String> {
    let source = image.pixels();
    let descriptor = source.descriptor();
    let prepared = if descriptor.format() == snapclip_model::PixelFormat::Bgra8Unorm {
        let bytes = if descriptor.alpha() == AlphaMode::Premultiplied {
            Cow::Owned(
                snapclip_model::pixel_color::to_sdr_bgra8(source, 100.0)
                    .map_err(str::to_owned)?
                    .into_bytes(),
            )
        } else {
            Cow::Borrowed(source.as_bytes())
        };
        ImagePreviewPixels::from_bgra8(image.width(), image.height(), bytes)
    } else {
        let bytes = if descriptor.alpha() == AlphaMode::Premultiplied {
            Cow::Owned(
                snapclip_model::pixel_color::linear_rgba(source)
                    .flatten()
                    .flat_map(|v| half::f16::from_f64(v).to_le_bytes())
                    .collect::<Vec<u8>>(),
            )
        } else {
            Cow::Borrowed(source.as_bytes())
        };
        let encoding = if image.is_hdr() {
            ImagePreviewColorSpace::ScRgb
        } else {
            ImagePreviewColorSpace::LinearSrgb
        };
        ImagePreviewPixels::from_rgba16f(
            image.width(),
            image.height(),
            source.row_stride(),
            bytes,
            encoding,
        )
    };
    prepared
        .and_then(|pixels| pixels.into_preview(&ImagePreviewLimits::default(), "贴图图片"))
        .map_err(|e| e.to_string())
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
    decode_image(&bytes)
}
pub fn decode_image(bytes: &[u8]) -> Result<PinImage, String> {
    #[cfg(feature = "avif")]
    if snapclip_avif::is_avif(bytes) {
        let pixels =
            snapclip_avif::decode(bytes, &snapclip_avif::Unstoppable).map_err(|e| e.to_string())?;
        return PinImage::from_pixels(PixelSource::new(pixels.descriptor(), 0), Arc::new(pixels))
            .ok_or_else(|| "图片为空".into());
    }
    let limits = ImagePreviewLimits::default()
        .max_pixels(24_000_000)
        .max_decode_bytes(24_000_000 * 8);
    let decoded = ImagePreviewPixels::decode(bytes, &limits).map_err(|e| e.to_string())?;
    let [w, h] = decoded.pixel_size();
    let descriptor = match decoded.color_space() {
        ImagePreviewColorSpace::Srgb => PixelDescriptor::bgra8(),
        encoding => PixelDescriptor::new(
            snapclip_model::PixelFormat::Rgba16Float,
            if encoding == ImagePreviewColorSpace::ScRgb {
                PixelEncoding::ScRgb
            } else {
                PixelEncoding::LinearSrgb
            },
            AlphaMode::Straight,
        )
        .map_err(str::to_owned)?,
    };
    let swap = descriptor == PixelDescriptor::bgra8() && !decoded.is_bgra8();
    let mut bytes = decoded.into_bytes();
    if swap {
        for pixel in bytes.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
    }
    let pixels = PixelBuffer::from_tight(w, h, descriptor, bytes).map_err(str::to_owned)?;
    PinImage::from_pixels(PixelSource::new(descriptor, 0), Arc::new(pixels))
        .ok_or_else(|| "图片为空".into())
}

pub(crate) fn encode(image: &PinImage, thumbnail: bool) -> Result<Vec<u8>, String> {
    let rgba = image::RgbaImage::from_raw(
        image.width(),
        image.height(),
        image.sdr_rgba().map_err(str::to_owned)?,
    )
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
#[path = "image_preview_contract_tests.rs"]
mod contract_tests;

#[cfg(test)]
mod tests {
    use super::decode_image as decode;
    use super::*;
    #[cfg(feature = "avif")]
    #[test]
    fn avif_uses_the_shared_hdr_decoder_and_existing_preview_resource() {
        let mut bytes = Vec::new();
        snapclip_avif::write_pq_tiles(
            1,
            1,
            &std::env::temp_dir(),
            &mut bytes,
            snapclip_avif::StopToken::new(snapclip_avif::Unstoppable),
            |_, _, tile| {
                tile.resize(128 * 128, [49271_u16, 49271, 49271, 32768].into());
                Ok(())
            },
        )
        .unwrap();
        let pin = decode(&bytes).unwrap();
        assert!(pin.is_hdr());
        let source = pin.pixels().as_bytes().to_vec();
        let display = render(&pin).unwrap();
        assert_eq!(display.color_space(), ImagePreviewColorSpace::ScRgb);
        assert_eq!(display.image().as_bytes(0).unwrap(), source);
        let value = half::f16::from_le_bytes(source[..2].try_into().unwrap()).to_f32();
        assert!((value - 12.5).abs() < 0.08);
        assert_eq!(pin.pixels().as_bytes(), source);
    }
    #[test]
    fn sixteen_bit_sdr_has_an_explicit_eight_bit_copy_derivative() {
        let pixels =
            image::ImageBuffer::from_pixel(1, 1, image::Rgba([0x4000_u16, 0x8080, 0xc000, 0xffff]));
        let mut encoded = std::io::Cursor::new(vec![]);
        image::DynamicImage::ImageRgba16(pixels)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let image = decode(encoded.get_ref()).unwrap();
        assert_eq!(
            image.pixels().descriptor().encoding(),
            PixelEncoding::LinearSrgb
        );
        assert_eq!(image.pixels().as_bytes().len(), 8);
        assert_eq!(
            render(&image).unwrap().color_space(),
            ImagePreviewColorSpace::LinearSrgb
        );
        assert_eq!(image.sdr_rgba().unwrap(), [64, 128, 191, 255]);
    }
    #[test]
    fn pq_file_decodes_and_renders_hdr_without_an_sdr_intermediate() {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Sixteen);
        {
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_chunk(png::chunk::cICP, &[9, 16, 0, 1])
                .unwrap();
            let codes: Vec<u8> = [49271_u16, 49271, 49271, 32768]
                .into_iter()
                .flat_map(u16::to_be_bytes)
                .collect();
            writer.write_image_data(&codes).unwrap();
        }
        let source = decode(&bytes).unwrap();
        assert!(source.is_hdr());
        let original = source.pixels().as_bytes().to_vec();
        let preview = render(&source).unwrap();
        assert_eq!(preview.color_space(), ImagePreviewColorSpace::ScRgb);
        assert_eq!(preview.image().as_bytes(0).unwrap(), original);
        let r = half::f16::from_le_bytes(original[..2].try_into().unwrap()).to_f32();
        assert!((r - 12.5).abs() < 0.02);
        let sdr = decode(&encode(&source, false).unwrap()).unwrap();
        assert!(!sdr.is_hdr());
        assert_eq!(sdr.sdr_rgba().unwrap()[3], 128);
        assert_eq!(source.pixels().as_bytes(), original);
    }
    #[test]
    fn render_preserves_extended_rgb_and_normalizes_associated_alpha_once() {
        for alpha in [AlphaMode::Straight, AlphaMode::Premultiplied] {
            let descriptor = PixelDescriptor::new(
                snapclip_model::PixelFormat::Rgba16Float,
                PixelEncoding::ScRgb,
                alpha,
            )
            .unwrap();
            let values = if alpha == AlphaMode::Straight {
                [-0.25_f32, 2.5, 0.5, 0.5]
            } else {
                [-0.125, 1.25, 0.25, 0.5]
            };
            let bytes: Vec<u8> = values
                .into_iter()
                .flat_map(|v| half::f16::from_f32(v).to_le_bytes())
                .collect();
            let source =
                Arc::new(PixelBuffer::from_tight(1, 1, descriptor, bytes.clone()).unwrap());
            let image =
                PinImage::from_pixels(PixelSource::new(descriptor, 7), source.clone()).unwrap();
            let preview = render(&image).unwrap();
            let rendered: Vec<f32> = preview
                .image()
                .as_bytes(0)
                .unwrap()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| half::f16::from_le_bytes(*b).to_f32())
                .collect();
            assert_eq!(rendered, [-0.25, 2.5, 0.5, 0.5]);
            assert_eq!(source.as_bytes(), bytes);
            assert!(Arc::ptr_eq(image.pixels(), &source));
            assert_eq!(image.pixel_source().generation(), 7);
        }
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
        assert_eq!(source.sdr_rgba().unwrap(), rgba);
    }
    #[test]
    fn local_file_preview_keeps_original_pixels_and_alpha() {
        let original = PinImage::new(2, 1, vec![255, 0, 0, 0, 0, 255, 0, 127]).unwrap();
        let encoded = encode(&original, false).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.sdr_rgba().unwrap(), original.sdr_rgba().unwrap());
        let file =
            std::env::temp_dir().join(format!("snapclip-pin-preview-{}.png", std::process::id()));
        std::fs::write(&file, &encoded).unwrap();
        let loaded = load(file.to_str().unwrap()).unwrap();
        std::fs::remove_file(file).unwrap();
        assert_eq!(loaded.sdr_rgba().unwrap(), original.sdr_rgba().unwrap());
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
        assert_eq!(image.sdr_rgba().unwrap(), pixels);
    }
}
