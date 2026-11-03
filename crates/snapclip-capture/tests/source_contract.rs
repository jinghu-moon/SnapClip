use snapclip_capture::artifact::{CaptureService, PixelSliceSource};
use snapclip_capture::session::CapturedFrame;
use snapclip_capture::{CaptureResult, Rect};
use snapclip_model::PixelFormat;
use snapclip_model::pixels::{AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding, PixelSource};

struct Source {
    source: PixelSource,
    returned: PixelDescriptor,
}
impl PixelSliceSource for Source {
    fn source(&self) -> PixelSource {
        self.source
    }
    fn read_pixels(&self, region: Rect) -> CaptureResult<PixelBuffer> {
        let pixel = [0x00, 0xbc, 0x00, 0x49, 0x00, 0x00, 0x00, 0x3c]; // -1, 10, 0, 1
        let bytes = if self.returned.format() == PixelFormat::Rgba16Float {
            pixel.repeat(region.area() as usize)
        } else {
            [0, 0, 0, 255].repeat(region.area() as usize)
        };
        Ok(PixelBuffer::from_bytes(
            region.width() as u32,
            region.height() as u32,
            self.returned.row_bytes(region.width() as u32).unwrap(),
            self.returned,
            bytes,
        )
        .unwrap())
    }
}
fn descriptor() -> PixelDescriptor {
    PixelDescriptor::new(
        PixelFormat::Rgba16Float,
        PixelEncoding::ScRgb,
        AlphaMode::Straight,
    )
    .unwrap()
}
fn frame(source: PixelSource) -> CapturedFrame {
    CapturedFrame {
        width: 4,
        height: 3,
        source,
        captured_at_unix_ms: 1,
        provider: "test",
        origin: None,
    }
}
#[test]
fn selection_preserves_fp16_bits_encoding_and_generation() {
    let source = PixelSource::new(descriptor(), 17);
    let pixels = Source {
        source,
        returned: descriptor(),
    };
    let result = CaptureService::new()
        .prepare_selection(&frame(source), Rect::new(1, 1, 3, 3), &pixels)
        .unwrap();
    assert_eq!(result.frame.source, source);
    assert_eq!(result.pixels.descriptor(), descriptor());
    assert_eq!(result.pixels.size(), [2, 2]);
    assert_eq!(
        result.pixels.as_bytes(),
        [0, 0xbc, 0, 0x49, 0, 0, 0, 0x3c].repeat(4)
    );
    assert!(
        result.sdr_bgra().is_err(),
        "HDR must not enter a BGRA8 consumer"
    );
}
#[test]
fn equal_layout_with_a_different_generation_is_rejected() {
    let pixels = Source {
        source: PixelSource::new(descriptor(), 18),
        returned: descriptor(),
    };
    assert!(
        CaptureService::new()
            .prepare_selection(
                &frame(PixelSource::new(descriptor(), 17)),
                Rect::new(0, 0, 2, 2),
                &pixels
            )
            .is_err()
    );
}
#[test]
fn readback_must_match_the_frozen_color_definition() {
    let source = PixelSource::new(descriptor(), 17);
    let pixels = Source {
        source,
        returned: PixelDescriptor::bgra8(),
    };
    assert!(
        CaptureService::new()
            .prepare_selection(&frame(source), Rect::new(0, 0, 2, 2), &pixels)
            .is_err()
    );
    let opaque = PixelDescriptor::new(
        PixelFormat::Rgba16Float,
        PixelEncoding::ScRgb,
        AlphaMode::Opaque,
    )
    .unwrap();
    let source = PixelSource::new(opaque, 17);
    assert!(
        CaptureService::new()
            .prepare_selection(
                &frame(source),
                Rect::new(0, 0, 2, 2),
                &Source {
                    source,
                    returned: descriptor()
                }
            )
            .is_err(),
        "only a derived mask can change alpha; raw readback must match exactly"
    );
}

#[test]
fn a_source_change_during_readback_is_rejected() {
    struct Changing(std::cell::Cell<u64>);
    impl PixelSliceSource for Changing {
        fn source(&self) -> PixelSource {
            PixelSource::new(descriptor(), self.0.get())
        }
        fn read_pixels(&self, region: Rect) -> CaptureResult<PixelBuffer> {
            self.0.set(18);
            Source {
                source: self.source(),
                returned: descriptor(),
            }
            .read_pixels(region)
        }
    }
    assert!(
        CaptureService::new()
            .prepare_selection(
                &frame(PixelSource::new(descriptor(), 17)),
                Rect::new(0, 0, 2, 2),
                &Changing(std::cell::Cell::new(17))
            )
            .is_err()
    );
}

#[test]
fn band_materialization_preserves_fp16_and_refuses_mixed_generations() {
    use snapclip_capture::ports::{BandSource, SelectionSpec, whole_from_bands};
    struct Bands {
        source: PixelSource,
        change: bool,
    }
    impl BandSource for Bands {
        fn source(&self) -> PixelSource {
            self.source
        }
        fn rows(&self) -> u32 {
            3
        }
        fn read_rows(&mut self, _: u32, rows: u32, out: &mut Vec<u8>) -> bool {
            *out = [0, 0xbc, 0, 0x49, 0, 0, 0, 0x3c].repeat(2 * rows as usize);
            if self.change {
                self.source = PixelSource::new(descriptor(), 18);
            }
            true
        }
    }
    let source = PixelSource::new(descriptor(), 17);
    let spec = SelectionSpec {
        region: Rect::new(0, 0, 2, 3),
        source,
        captured_at_unix_ms: 1,
        origin: None,
        corner_radius: 0,
    };
    let result = whole_from_bands(
        &spec,
        &mut Bands {
            source,
            change: false,
        },
    )
    .unwrap();
    assert_eq!(result.frame.source, source);
    assert_eq!(
        result.pixels.as_bytes(),
        [0, 0xbc, 0, 0x49, 0, 0, 0, 0x3c].repeat(6)
    );
    assert!(
        whole_from_bands(
            &spec,
            &mut Bands {
                source,
                change: true
            }
        )
        .is_err()
    );
}

#[test]
fn rounded_fp16_keeps_hdr_rgb_and_multiplies_associated_color() {
    use snapclip_capture::artifact::round_pixel_corners;
    let sample = [-1., 10., 0.5, 1.]
        .map(half::f16::from_f32)
        .into_iter()
        .flat_map(|v| v.to_le_bytes())
        .collect::<Vec<_>>();
    for mode in [
        AlphaMode::Straight,
        AlphaMode::Premultiplied,
        AlphaMode::Opaque,
    ] {
        let desc =
            PixelDescriptor::new(PixelFormat::Rgba16Float, PixelEncoding::ScRgb, mode).unwrap();
        let buffer = PixelBuffer::from_tight(20, 20, desc, sample.repeat(400)).unwrap();
        let result = round_pixel_corners(buffer, 6).unwrap();
        assert_eq!(&result.as_bytes()[(10 * 20 + 10) * 8..][..8], sample);
        assert_eq!(
            result.descriptor().alpha(),
            if mode == AlphaMode::Opaque {
                AlphaMode::Straight
            } else {
                mode
            }
        );
        let mut partial = 0;
        for p in result.as_bytes().as_chunks::<8>().0 {
            let values: [f32; 4] = std::array::from_fn(|c| {
                half::f16::from_le_bytes([p[c * 2], p[c * 2 + 1]]).to_f32()
            });
            if mode == AlphaMode::Premultiplied {
                assert!((values[0] + values[3]).abs() < 0.001);
                assert!((values[1] - 10. * values[3]).abs() < 0.01);
            } else {
                assert_eq!(&p[..6], &sample[..6]);
            }
            if values[3] > 0. && values[3] < 1. {
                partial += 1;
            }
        }
        assert!(partial > 0);
    }
}
