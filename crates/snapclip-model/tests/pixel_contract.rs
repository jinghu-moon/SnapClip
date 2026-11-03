use snapclip_model::PixelFormat;
use snapclip_model::pixels::{AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding};

fn hdr() -> PixelDescriptor {
    PixelDescriptor::new(
        PixelFormat::Rgba16Float,
        PixelEncoding::ScRgb,
        AlphaMode::Straight,
    )
    .unwrap()
}

#[test]
fn format_and_encoding_must_describe_the_same_storage() {
    assert!(
        PixelDescriptor::new(
            PixelFormat::Bgra8Unorm,
            PixelEncoding::ScRgb,
            AlphaMode::Straight
        )
        .is_err()
    );
    assert!(
        PixelDescriptor::new(
            PixelFormat::Rgba16Float,
            PixelEncoding::Srgb,
            AlphaMode::Straight
        )
        .is_err()
    );
    let json = serde_json::to_string(&hdr()).unwrap();
    assert_eq!(
        serde_json::from_str::<PixelDescriptor>(&json).unwrap(),
        hdr()
    );
    assert!(
        serde_json::from_str::<PixelDescriptor>(&json.replace("rgba16_float", "bgra8_unorm"))
            .is_err()
    );
}

#[test]
fn padded_fp16_rows_keep_highlights_negative_rgb_and_signed_zero_bit_exactly() {
    // -0.25, 12.5, -0.0, 0.5. No color conversion belongs in a byte contract.
    let pixel = [0x00, 0xb4, 0x40, 0x4a, 0x00, 0x80, 0x00, 0x38];
    let mut bytes = pixel.to_vec();
    bytes.extend([0xcc; 8]);
    bytes.extend(pixel);
    bytes.extend([0xdd; 8]);
    let buffer = PixelBuffer::from_bytes(1, 2, 16, hdr(), bytes).unwrap();
    assert_eq!(buffer.as_bytes(), pixel.repeat(2));
    assert_eq!(buffer.row_stride(), 8);
    assert_eq!(buffer.descriptor(), hdr());
    assert_eq!(buffer.size(), [1, 2]);
}

#[test]
fn rejects_bad_lengths_limits_nonfinite_and_out_of_range_alpha() {
    assert!(PixelBuffer::from_bytes(1, 1, 7, hdr(), vec![0; 8]).is_err());
    assert!(PixelBuffer::from_bytes(1, 2, 16, hdr(), vec![0; 24]).is_err());
    assert!(PixelBuffer::from_bytes(0, 1, 0, hdr(), vec![]).is_err());
    assert!(PixelBuffer::from_bytes(u32::MAX, u32::MAX, usize::MAX, hdr(), vec![]).is_err());
    for value in [0x7c00u16, 0xfc00, 0x7e00] {
        let mut bytes = vec![0; 8];
        bytes[..2].copy_from_slice(&value.to_le_bytes());
        assert!(PixelBuffer::from_bytes(1, 1, 8, hdr(), bytes).is_err());
    }
    for alpha in [0x3c01u16, 0xb800, 0x7c00] {
        let mut bytes = vec![0; 8];
        bytes[6..8].copy_from_slice(&alpha.to_le_bytes());
        assert!(PixelBuffer::from_bytes(1, 1, 8, hdr(), bytes).is_err());
    }
}

#[test]
fn tight_owned_input_moves_without_another_full_frame_allocation() {
    let bytes = vec![0; 8];
    let pointer = bytes.as_ptr();
    let buffer = PixelBuffer::from_bytes(1, 1, 8, hdr(), bytes).unwrap();
    assert_eq!(buffer.as_bytes().as_ptr(), pointer);
    let bytes = buffer.into_bytes();
    assert_eq!(bytes.as_ptr(), pointer);
}

#[test]
fn opaque_descriptor_requires_opaque_pixels_instead_of_silently_changing_them() {
    let opaque = PixelDescriptor::new(
        PixelFormat::Rgba16Float,
        PixelEncoding::ScRgb,
        AlphaMode::Opaque,
    )
    .unwrap();
    assert!(PixelBuffer::from_bytes(1, 1, 8, opaque, vec![0; 8]).is_err());
    let mut bytes = vec![0; 8];
    bytes[6..8].copy_from_slice(&0x3c00u16.to_le_bytes());
    assert!(PixelBuffer::from_bytes(1, 1, 8, opaque, bytes).is_ok());
}

#[test]
fn serialized_source_keeps_its_frozen_encoding_and_generation() {
    use snapclip_model::pixels::PixelSource;
    let source = PixelSource::new(hdr(), 41);
    let json = serde_json::to_string(&source).unwrap();
    assert!(json.contains("sc_rgb"));
    assert_eq!(serde_json::from_str::<PixelSource>(&json).unwrap(), source);
    let invalid = json.replace("rgba16_float", "bgra8_unorm");
    assert!(serde_json::from_str::<PixelSource>(&invalid).is_err());
    let invalid = json.replace(",\"generation\":41", "");
    assert!(
        serde_json::from_str::<PixelSource>(&invalid).is_err(),
        "a source generation cannot be silently invented"
    );
}
