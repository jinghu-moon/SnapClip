//! Explicit color derivatives. Source buffers are immutable; conversion never
//! changes the original encoding or acquisition generation.

use crate::pixels::{AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding};

/// Iterate validated pixels as straight, linear Rec.709 RGBA. The RGB scale
/// remains absolute for scRGB and relative for SDR; alpha is never gamma coded.
pub fn linear_rgba(source: &PixelBuffer) -> impl Iterator<Item = [f64; 4]> + '_ {
    source
        .as_bytes()
        .chunks_exact(source.descriptor().bytes_per_pixel())
        .map(|pixel| straight_linear(source.descriptor(), pixel))
}

/// Generate the explicit, straight-alpha SDR compatibility image at 100 nit.
/// `source_peak_nits` is fixed for an analysis session/export, never adapted per
/// band. The same BT.2390 implementation is used by Yue's display derivative.
pub fn to_sdr_bgra8(
    source: &PixelBuffer,
    source_peak_nits: f64,
) -> Result<PixelBuffer, &'static str> {
    if !source_peak_nits.is_finite() || source_peak_nits <= 0.0 {
        return Err("SDR mapping requires a finite positive source peak");
    }
    let descriptor = source.descriptor();
    if descriptor == PixelDescriptor::bgra8() {
        return Ok(source.clone());
    }
    let [width, height] = source.size();
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(width as usize * height as usize * 4)
        .map_err(|_| "cannot allocate SDR derivative")?;
    for pixel in source.as_bytes().chunks_exact(descriptor.bytes_per_pixel()) {
        let [r, g, b, alpha] = straight_linear(descriptor, pixel);
        let rgb = if descriptor.encoding() == PixelEncoding::ScRgb {
            yue_color::scrgb_to_sdr([r, g, b], source_peak_nits)
        } else {
            [r, g, b]
        };
        let [r, g, b] =
            rgb.map(|v| (yue_color::linear_to_srgb(v).clamp(0.0, 1.0) * 255.0).round() as u8);
        bytes.extend_from_slice(&[b, g, r, (alpha * 255.0).round() as u8]);
    }
    PixelBuffer::from_tight(width, height, PixelDescriptor::bgra8(), bytes)
}

/// Full-range Rec.2020/PQ RGBA16 in PNG network byte order. This is a quantized
/// export, not a replacement for the lossless FP16 master. Nonrepresentable
/// negative/out-of-gamut or >10000 nit colors are errors, not silent clipping.
pub fn to_rec2020_pq16(source: &PixelBuffer) -> Result<Vec<u8>, &'static str> {
    let pixels = rec2020_pq16_pixels(source)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(source.as_bytes().len())
        .map_err(|_| "cannot allocate PQ export band")?;
    for pixel in pixels {
        for channel in pixel? {
            output.extend_from_slice(&channel.to_be_bytes());
        }
    }
    Ok(output)
}

/// Iterate full-range, straight-alpha Rec.2020/PQ RGBA16 without allocating an
/// output band. The caller must discard partial output on any pixel error.
/// Absolute scRGB is required; alpha remains linear coverage. The same conversion
/// backs PNG16 and bounded tile exports, so neither invents a second color path.
pub fn rec2020_pq16_pixels(
    source: &PixelBuffer,
) -> Result<impl Iterator<Item = Result<[u16; 4], &'static str>> + '_, &'static str> {
    if source.descriptor().encoding() != PixelEncoding::ScRgb {
        return Err("HDR PQ export requires an absolute scRGB source");
    }
    Ok(linear_rgba(source).map(|[r, g, b, alpha]| {
        let rgb = yue_color::rec709_to_rec2020([r, g, b]);
        let mut rgba = [0; 4];
        for (output, channel) in rgba[..3].iter_mut().zip(rgb) {
            let nits = channel * 80.0;
            // Only accommodate matrix roundoff at the exact representable ends.
            if !nits.is_finite() || !(-1e-8..=10000.0 + 1e-8).contains(&nits) {
                return Err(
                    "source color is outside Rec.2020/PQ range; preserve the FP16 master or explicitly export SDR",
                );
            }
            *output = (yue_color::nits_to_pq(nits.clamp(0.0, 10000.0)) * 65535.0).round() as u16;
        }
        rgba[3] = (alpha * 65535.0).round() as u16;
        Ok(rgba)
    }))
}

fn straight_linear(descriptor: PixelDescriptor, pixel: &[u8]) -> [f64; 4] {
    let mut rgba = match descriptor.format() {
        crate::PixelFormat::Bgra8Unorm => [
            f64::from(pixel[2]) / 255.0,
            f64::from(pixel[1]) / 255.0,
            f64::from(pixel[0]) / 255.0,
            f64::from(pixel[3]) / 255.0,
        ],
        crate::PixelFormat::Rgba16Float => std::array::from_fn(|c| {
            half::f16::from_le_bytes([pixel[c * 2], pixel[c * 2 + 1]]).to_f64()
        }),
    };
    if descriptor.alpha() == AlphaMode::Premultiplied {
        let alpha = rgba[3];
        for channel in &mut rgba[..3] {
            *channel = if alpha > 0.0 { *channel / alpha } else { 0.0 };
        }
    }
    if descriptor.encoding() == PixelEncoding::Srgb {
        for channel in &mut rgba[..3] {
            *channel = yue_color::srgb_to_linear(*channel);
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixels::{AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding};

    fn floats(values: &[[f32; 4]], encoding: PixelEncoding, alpha: AlphaMode) -> PixelBuffer {
        let bytes = values
            .iter()
            .flatten()
            .flat_map(|v| half::f16::from_f32(*v).to_le_bytes())
            .collect();
        PixelBuffer::from_tight(
            values.len() as u32,
            1,
            PixelDescriptor::new(crate::PixelFormat::Rgba16Float, encoding, alpha).unwrap(),
            bytes,
        )
        .unwrap()
    }

    #[test]
    fn sdr_derivative_preserves_source_and_alpha_and_matches_yue_mapping() {
        let source = floats(
            &[[2.5, 2.5, 2.5, 0.5], [-0.25, 0.4, 1.5, 1.0]],
            PixelEncoding::ScRgb,
            AlphaMode::Straight,
        );
        let original = source.as_bytes().to_vec();
        let mapped = to_sdr_bgra8(&source, 1000.0).unwrap();
        assert_eq!(source.as_bytes(), original);
        assert_eq!(mapped.descriptor(), PixelDescriptor::bgra8());
        assert_eq!(mapped.as_bytes()[3], 128);
        for (pixel, input) in mapped
            .as_bytes()
            .as_chunks::<4>()
            .0
            .iter()
            .zip([[2.5; 3], [-0.25, half::f16::from_f32(0.4).to_f64(), 1.5]])
        {
            let rgb = yue_color::scrgb_to_sdr(input, 1000.0)
                .map(|v| (yue_color::linear_to_srgb(v) * 255.0).round() as u8);
            assert_eq!(pixel[..3], [rgb[2], rgb[1], rgb[0]]);
        }
        assert!(to_sdr_bgra8(&source, f64::NAN).is_err());
        assert!(to_sdr_bgra8(&source, 0.0).is_err());
    }

    #[test]
    fn pq_export_has_known_codes_and_linear_alpha() {
        let source = floats(
            &[[1.25, 1.25, 1.25, 0.5], [12.5, 12.5, 12.5, 1.0]],
            PixelEncoding::ScRgb,
            AlphaMode::Straight,
        );
        let encoded = to_rec2020_pq16(&source).unwrap();
        let codes: Vec<u16> = encoded
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(&codes[..4], &[33297, 33297, 33297, 32768]);
        assert_eq!(&codes[4..], &[49271, 49271, 49271, 65535]);
        assert_eq!(
            rec2020_pq16_pixels(&source)
                .unwrap()
                .flat_map(|p| p.unwrap().into_iter().flat_map(u16::to_be_bytes))
                .collect::<Vec<_>>(),
            encoded
        );
    }

    #[test]
    fn pq_iterator_reports_invalid_later_pixels_and_rejects_relative_sources() {
        let source = floats(
            &[[1.25, 1.25, 1.25, 0.5], [-1., 0., 0., 1.]],
            PixelEncoding::ScRgb,
            AlphaMode::Straight,
        );
        let original = source.as_bytes().to_vec();
        let mut pixels = rec2020_pq16_pixels(&source).unwrap();
        assert_eq!(
            pixels.next().unwrap().unwrap(),
            [33297, 33297, 33297, 32768]
        );
        assert!(pixels.next().unwrap().is_err());
        assert!(pixels.next().is_none());
        assert!(to_rec2020_pq16(&source).is_err());
        assert_eq!(source.as_bytes(), original);
        let relative = floats(&[[1.; 4]], PixelEncoding::LinearSrgb, AlphaMode::Straight);
        assert!(rec2020_pq16_pixels(&relative).is_err());
    }

    #[test]
    fn pq_rejects_out_of_gamut_and_over_ten_thousand_nits() {
        for value in [[-1.0, 0.0, 0.0, 1.0], [126.0, 126.0, 126.0, 1.0]] {
            let source = floats(&[value], PixelEncoding::ScRgb, AlphaMode::Straight);
            assert!(to_rec2020_pq16(&source).is_err());
        }
    }

    #[test]
    fn transparent_premultiplied_pixels_do_not_divide_by_zero() {
        let source = floats(
            &[[0.0; 4], [1.25, 1.25, 1.25, 0.5]],
            PixelEncoding::ScRgb,
            AlphaMode::Premultiplied,
        );
        let straight = floats(
            &[[0.0; 4], [2.5, 2.5, 2.5, 0.5]],
            PixelEncoding::ScRgb,
            AlphaMode::Straight,
        );
        assert_eq!(
            to_sdr_bgra8(&source, 1000.0).unwrap().as_bytes(),
            to_sdr_bgra8(&straight, 1000.0).unwrap().as_bytes()
        );
        assert_eq!(
            to_rec2020_pq16(&source).unwrap(),
            to_rec2020_pq16(&straight).unwrap()
        );
    }
}
