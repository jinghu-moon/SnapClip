//! Contracts replacing the removed GDI source conversion/resampling tests.
//! Exercise the production PinImage -> Yue resource path, without a window.
use super::*;

fn float_image(
    values: &[f32],
    width: u32,
    height: u32,
    encoding: PixelEncoding,
    alpha: AlphaMode,
) -> PinImage {
    let descriptor =
        PixelDescriptor::new(snapclip_model::PixelFormat::Rgba16Float, encoding, alpha).unwrap();
    let bytes = values
        .iter()
        .flat_map(|v| half::f16::from_f32(*v).to_le_bytes())
        .collect();
    PinImage::from_pixels(
        PixelSource::new(descriptor, 71),
        Arc::new(PixelBuffer::from_tight(width, height, descriptor, bytes).unwrap()),
    )
    .unwrap()
}
fn channels(preview: &ImagePreviewImage) -> Vec<f32> {
    preview
        .image()
        .as_bytes(0)
        .unwrap()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| half::f16::from_le_bytes(*b).to_f32())
        .collect()
}

#[test]
fn bgra_rows_keep_their_original_top_down_order() {
    let input = [1, 2, 3, 255, 4, 5, 6, 128, 7, 8, 9, 64, 10, 11, 12, 0];
    let image = PinImage::new(2, 2, input.to_vec()).unwrap();
    assert_eq!(
        render(&image).unwrap().image().as_bytes(0).unwrap(),
        [3, 2, 1, 255, 6, 5, 4, 128, 9, 8, 7, 64, 12, 11, 10, 0]
    );
    assert_eq!(image.sdr_rgba().unwrap(), input);
}
#[test]
fn padded_bgra_source_never_leaks_padding_into_the_display() {
    let descriptor = PixelDescriptor::bgra8();
    let pixels = PixelBuffer::from_bytes(
        1,
        2,
        8,
        descriptor,
        vec![1, 2, 3, 255, 99, 99, 99, 99, 4, 5, 6, 128, 88, 88, 88, 88],
    )
    .unwrap();
    let image = PinImage::from_pixels(PixelSource::new(descriptor, 2), Arc::new(pixels)).unwrap();
    assert_eq!(
        render(&image).unwrap().image().as_bytes(0).unwrap(),
        [1, 2, 3, 255, 4, 5, 6, 128]
    );
}
#[test]
fn padded_fp16_rows_reach_the_same_hdr_display_resource() {
    let descriptor = PixelDescriptor::new(
        snapclip_model::PixelFormat::Rgba16Float,
        PixelEncoding::ScRgb,
        AlphaMode::Straight,
    )
    .unwrap();
    let row: Vec<u8> = [-0.25_f32, 12.5, 0.5, 0.5]
        .into_iter()
        .flat_map(|v| half::f16::from_f32(v).to_le_bytes())
        .collect();
    let mut bytes = row.clone();
    bytes.extend_from_slice(&[99; 8]);
    bytes.extend_from_slice(&row);
    bytes.extend_from_slice(&[88; 8]);
    let pixels = PixelBuffer::from_bytes(1, 2, 16, descriptor, bytes).unwrap();
    let image = PinImage::from_pixels(PixelSource::new(descriptor, 2), Arc::new(pixels)).unwrap();
    let preview = render(&image).unwrap();
    assert_eq!(
        channels(&preview),
        [-0.25, 12.5, 0.5, 0.5, -0.25, 12.5, 0.5, 0.5]
    );
    assert_eq!(preview.color_space(), ImagePreviewColorSpace::ScRgb);
}
#[test]
fn zero_alpha_associated_bgra_is_normalized_without_a_divide_by_zero() {
    let descriptor = PixelDescriptor::new(
        snapclip_model::PixelFormat::Bgra8Unorm,
        PixelEncoding::Srgb,
        AlphaMode::Premultiplied,
    )
    .unwrap();
    let image = PinImage::from_pixels(
        PixelSource::new(descriptor, 4),
        Arc::new(PixelBuffer::from_tight(1, 1, descriptor, vec![64, 32, 16, 0]).unwrap()),
    )
    .unwrap();
    assert_eq!(
        render(&image).unwrap().image().as_bytes(0).unwrap(),
        [0, 0, 0, 0]
    );
    assert_eq!(image.pixels().as_bytes(), [64, 32, 16, 0]);
}
#[test]
fn associated_bgra_is_unassociated_exactly_once_before_display() {
    let descriptor = PixelDescriptor::new(
        snapclip_model::PixelFormat::Bgra8Unorm,
        PixelEncoding::Srgb,
        AlphaMode::Premultiplied,
    )
    .unwrap();
    let pixels =
        Arc::new(PixelBuffer::from_tight(1, 1, descriptor, vec![64, 32, 16, 128]).unwrap());
    let image = PinImage::from_pixels(PixelSource::new(descriptor, 4), pixels.clone()).unwrap();
    let preview = render(&image).unwrap();
    assert_eq!(
        preview.image().as_bytes(0).unwrap(),
        snapclip_model::pixel_color::to_sdr_bgra8(&pixels, 100.)
            .unwrap()
            .as_bytes()
    );
    assert_eq!(pixels.as_bytes(), [64, 32, 16, 128]);
}
#[test]
fn relative_linear_sdr_is_not_relabelled_as_absolute_scrgb() {
    let image = float_image(
        &[0.25, 0.5, 1., 0.5],
        1,
        1,
        PixelEncoding::LinearSrgb,
        AlphaMode::Straight,
    );
    let preview = render(&image).unwrap();
    assert!(!image.is_hdr());
    assert_eq!(preview.color_space(), ImagePreviewColorSpace::LinearSrgb);
    assert_eq!(channels(&preview), [0.25, 0.5, 1., 0.5]);
}
#[test]
fn associated_linear_sdr_is_unassociated_without_changing_white_definition() {
    let image = float_image(
        &[0.125, 0.25, 0.5, 0.5],
        1,
        1,
        PixelEncoding::LinearSrgb,
        AlphaMode::Premultiplied,
    );
    let preview = render(&image).unwrap();
    assert_eq!(preview.color_space(), ImagePreviewColorSpace::LinearSrgb);
    assert_eq!(channels(&preview), [0.25, 0.5, 1., 0.5]);
}
#[test]
fn transparent_straight_hdr_keeps_hidden_rgb_in_the_immutable_source() {
    let image = float_image(
        &[-0.5, 12.5, 3., 0.],
        1,
        1,
        PixelEncoding::ScRgb,
        AlphaMode::Straight,
    );
    let original = image.pixels().as_bytes().to_vec();
    assert_eq!(channels(&render(&image).unwrap()), [-0.5, 12.5, 3., 0.]);
    assert_eq!(image.pixels().as_bytes(), original);
}
#[test]
fn hdr_display_does_not_clip_finite_highlights_to_eight_bit_white() {
    let image = float_image(
        &[125., 12.5, -0.5, 1.],
        1,
        1,
        PixelEncoding::ScRgb,
        AlphaMode::Straight,
    );
    let preview = render(&image).unwrap();
    assert_eq!(channels(&preview), [125., 12.5, -0.5, 1.]);
    assert_eq!(image.pixel_source().generation(), 71);
}
#[test]
fn the_sdr_png_copy_round_trips_every_byte_code_and_alpha() {
    let rgba: Vec<u8> = (0..=255_u8).flat_map(|c| [c, 255 - c, c / 2, c]).collect();
    let image = PinImage::new(256, 1, rgba.clone()).unwrap();
    let encoded = encode(&image, false).unwrap();
    assert_eq!(decode_image(&encoded).unwrap().sdr_rgba().unwrap(), rgba);
    assert!(!image.is_hdr());
}
#[test]
fn sixteen_bit_sdr_alpha_is_retained_above_eight_bit_precision() {
    let pixel = image::ImageBuffer::from_pixel(1, 1, image::Rgba([16384_u16, 32768, 49152, 32768]));
    let mut encoded = std::io::Cursor::new(vec![]);
    image::DynamicImage::ImageRgba16(pixel)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let image = decode_image(encoded.get_ref()).unwrap();
    let alpha = channels(&render(&image).unwrap())[3];
    assert!((alpha - 32768. / 65535.).abs() < 0.0002);
    assert!((alpha - (alpha * 255.).round() / 255.).abs() > 0.0003);
}
#[test]
fn texture_edge_budget_uses_an_exact_source_and_then_an_explicit_proxy() {
    for width in [8192_u32, 8193] {
        let image = PinImage::new(width, 1, [180, 70, 17, 127].repeat(width as usize)).unwrap();
        let preview = render(&image).unwrap();
        assert_eq!(preview.pixel_size(), [width, 1]);
        assert_eq!(preview.texture_size(), [8192, 1]);
        assert_eq!(preview.is_proxy(), width > 8192);
        assert_eq!(image.width(), width);
    }
}
#[test]
fn independent_display_preparations_never_replace_shared_source_identity() {
    let image = PinImage::new(1, 1, vec![1, 2, 3, 127]).unwrap();
    let clone = image.clone();
    let first = render(&image).unwrap();
    let second = render(&clone).unwrap();
    assert_ne!(first.image().id, second.image().id);
    assert!(Arc::ptr_eq(image.pixels(), clone.pixels()));
    assert_eq!(first.image().as_bytes(0), second.image().as_bytes(0));
}
#[test]
fn source_edge_limit_rejects_preparation_instead_of_silently_truncating_pixels() {
    let image = PinImage::new(32769, 1, [1, 2, 3, 255].repeat(32769)).unwrap();
    assert!(render(&image).is_err());
    assert_eq!(image.width(), 32769);
    assert_eq!(image.pixels().as_bytes().len(), 32769 * 4);
}
