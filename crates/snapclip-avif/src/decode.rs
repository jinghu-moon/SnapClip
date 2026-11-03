use crate::{Error, Result, Stop, check};
use moxcms::{ColorProfile, Layout, ToneReprCurve, TransformF32Executor, TransformOptions};
use rgb::{Rgb, Rgba};
use snapclip_model::{
    PixelFormat,
    pixels::{AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding},
};
use std::sync::Arc;

pub const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PREVIEW_PIXELS: u32 = 24_000_000;

pub fn is_avif(bytes: &[u8]) -> bool {
    let Some(header) = bytes.get(..16) else {
        return false;
    };
    let size = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
    header.get(4..8) == Some(b"ftyp")
        && size >= 16
        && bytes.get(..size).is_some_and(|ftyp| {
            [&ftyp[8..12], &ftyp[16..]].into_iter().any(|brands| {
                brands
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|v| v == b"avif" || v == b"avis")
            })
        })
}
fn external(error: impl std::fmt::Display, stop: &impl Stop) -> Error {
    if stop.check().is_err() {
        Error::Cancelled
    } else {
        Error::Invalid(error.to_string())
    }
}
fn u32_at(bytes: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(
        bytes
            .get(at..at + 4)
            .ok_or("truncated metadata")?
            .try_into()
            .unwrap(),
    ))
}
fn boxes(mut bytes: &[u8], depth: usize, count: &mut usize, stop: &impl Stop) -> Result<()> {
    if depth > 8 {
        return Err("metadata nesting limit exceeded".into());
    }
    while !bytes.is_empty() {
        check(stop)?;
        *count += 1;
        if *count > 20_000 {
            return Err("metadata box count limit exceeded".into());
        }
        let size = u32_at(bytes, 0)? as usize;
        if size < 8 || size > bytes.len() {
            return Err("unsupported or invalid box length".into());
        }
        let kind = &bytes[4..8];
        let body = &bytes[8..size];
        if kind == b"meta" && body.len() > 1024 * 1024 {
            return Err("metadata byte limit exceeded".into());
        }
        match kind {
            b"meta" | b"iref" => boxes(
                body.get(4..).ok_or("truncated full box")?,
                depth + 1,
                count,
                stop,
            )?,
            b"iinf" => {
                let skip = if body.first() == Some(&0) { 6 } else { 8 };
                let entries = if skip == 6 {
                    u16::from_be_bytes(body.get(4..6).ok_or("truncated iinf")?.try_into().unwrap())
                        as u32
                } else {
                    u32_at(body, 4)?
                };
                if entries > 8194 {
                    return Err("metadata item count limit exceeded".into());
                }
                boxes(
                    body.get(skip..).ok_or("truncated iinf")?,
                    depth + 1,
                    count,
                    stop,
                )?;
            }
            b"iprp" | b"ipco" => boxes(body, depth + 1, count, stop)?,
            b"colr" if body.starts_with(b"prof") || body.starts_with(b"rICC") => {
                if body.len() > 65540 {
                    return Err("ICC byte limit exceeded".into());
                }
                validate_icc(&body[4..])?;
            }
            b"irot" | b"imir" | b"clap" | b"moov" => {
                return Err("transformed or animated AVIF requires explicit support".into());
            }
            _ => (),
        }
        bytes = &bytes[size..];
    }
    Ok(())
}
fn validate_icc(bytes: &[u8]) -> Result<()> {
    if bytes.len() < 132 || bytes.len() > 64 * 1024 || u32_at(bytes, 0)? as usize != bytes.len() {
        return Err("invalid bounded ICC length".into());
    }
    let count = u32_at(bytes, 128)? as usize;
    if count > 128 || 132 + count * 12 > bytes.len() {
        return Err("ICC tag count limit exceeded".into());
    }
    for ix in 0..count {
        let at = 132 + ix * 12;
        let offset = u32_at(bytes, at + 4)? as usize;
        let length = u32_at(bytes, at + 8)? as usize;
        let data = bytes
            .get(offset..offset.checked_add(length).ok_or("ICC tag overflow")?)
            .ok_or("ICC tag outside profile")?;
        match data.get(..4) {
            Some(b"curv") if u32_at(data, 8)? > 4096 => {
                return Err("ICC curve count limit exceeded".into());
            }
            Some(b"mluc") if u32_at(data, 8)? > 128 => {
                return Err("ICC text count limit exceeded".into());
            }
            Some(b"mft1" | b"mft2" | b"mAB " | b"mBA ") => {
                return Err("ICC LUT profiles require separate validation".into());
            }
            _ => (),
        }
    }
    Ok(())
}
enum Color {
    Srgb,
    Pq,
    Icc(Arc<TransformF32Executor>),
}
impl Color {
    fn from_info(info: &zenavif::ImageInfo) -> Result<Self> {
        if let Some(icc) = &info.icc_profile {
            validate_icc(icc)?;
            let profile = ColorProfile::new_from_slice(icc).map_err(|e| e.to_string())?;
            if profile.color_space != moxcms::DataColorSpace::Rgb {
                return Err("only RGB ICC profiles are supported".into());
            }
            if let Some(cicp) = profile.cicp {
                if cicp.transfer_characteristics as u8 == 16 {
                    if (
                        cicp.color_primaries as u8,
                        cicp.matrix_coefficients as u8,
                        cicp.full_range,
                    ) == (9, 0, true)
                    {
                        return Ok(Self::Pq);
                    }
                    return Err("unsupported HDR ICC definition".into());
                }
                if !matches!(
                    cicp.transfer_characteristics as u8,
                    1 | 4 | 5 | 6 | 7 | 8 | 13
                ) {
                    return Err("unsupported ICC transfer".into());
                }
            }
            if profile.color_space != moxcms::DataColorSpace::Rgb
                || profile.red_trc.is_none()
                || profile.green_trc.is_none()
                || profile.blue_trc.is_none()
                || profile.lut_a_to_b_perceptual.is_some()
                || profile.lut_a_to_b_colorimetric.is_some()
                || profile.lut_a_to_b_saturation.is_some()
            {
                return Err("only RGB matrix/TRC ICC profiles are supported".into());
            }
            let mut linear = ColorProfile::new_srgb();
            let curve = Some(ToneReprCurve::Parametric(vec![1.]));
            linear.red_trc = curve.clone();
            linear.green_trc = curve.clone();
            linear.blue_trc = curve;
            linear.cicp = None;
            let options = TransformOptions {
                prefer_fixed_point: false,
                allow_extended_range_rgb_xyz: true,
                ..TransformOptions::default()
            };
            return profile
                .create_transform_f32(Layout::Rgba, &linear, Layout::Rgba, options)
                .map(Self::Icc)
                .map_err(|e| e.to_string().into());
        }
        match (info.color_primaries.0, info.transfer_characteristics.0) {
            (9, 16) => Ok(Self::Pq),
            (1, 13) => Ok(Self::Srgb),
            _ => Err("unsupported or untagged AVIF color definition".into()),
        }
    }
    fn encoding(&self) -> PixelEncoding {
        if matches!(self, Self::Pq) {
            PixelEncoding::ScRgb
        } else {
            PixelEncoding::LinearSrgb
        }
    }
}

/// Decode a bounded static AVIF into straight FP16 pixels. PQ is absolute scRGB;
/// SDR ICC/sRGB stays explicitly relative linear Rec.709, never relabeled HDR.
/// Unsupported transforms, animation, LUT profiles and excessive inputs fail.
pub fn decode(bytes: &[u8], stop: &impl Stop) -> Result<PixelBuffer> {
    check(stop)?;
    if bytes.len() > MAX_INPUT_BYTES || !is_avif(bytes) {
        return Err("invalid AVIF or encoded input exceeds 64 MiB".into());
    }
    boxes(bytes, 0, &mut 0, stop)?;
    let parse_cfg = zenavif_parse::DecodeConfig::default()
        .with_peak_memory_limit(16 * 1024 * 1024)
        .with_total_megapixels_limit(24)
        .with_max_grid_tiles(4096)
        .with_max_animation_frames(1);
    let parser = zenavif_parse::AvifParser::from_bytes_with_config(bytes, &parse_cfg, stop)
        .map_err(|e| external(e, stop))?;
    if parser.animation_info().is_some()
        || parser.gain_map_metadata().is_some()
        || parser.has_depth_map()
        || parser.pixel_aspect_ratio().is_some()
    {
        return Err("unsupported animated, auxiliary or aspect-transformed AVIF".into());
    }
    if parser.color_info().is_none() {
        return Err("untagged AVIF requires an explicit color definition".into());
    }
    let grid = parser.grid_config().is_some();
    drop(parser);
    let cfg = zenavif::DecoderConfig::new()
        .threads(1)
        .prefer_8bit(false)
        .frame_size_limit(MAX_PREVIEW_PIXELS)
        .grid_tile_limit(4096)
        .parser_memory_limit(16 * 1024 * 1024);
    let mut decoder = zenavif::ManagedAvifDecoder::new_with_stop(bytes, &cfg, stop)
        .map_err(|e| external(e, stop))?;
    let info = decoder.probe_info().map_err(|e| external(e, stop))?;
    let count = u64::from(info.width) * u64::from(info.height);
    if count == 0 || count > u64::from(MAX_PREVIEW_PIXELS) {
        return Err("AVIF preview pixel budget exceeded".into());
    }
    let color = Color::from_info(&info)?;
    check(stop)?;
    let mut out = Vec::new();
    out.try_reserve_exact(count as usize * 8)
        .map_err(|e| e.to_string())?;
    out.resize(count as usize * 8, 0);
    if grid {
        let mut failure = None;
        let result =
            decoder.decode_grid_tiles(stop, 1024 * 1024, |left, top, pixels| {
                match convert(left, top, pixels, &info, &color, &mut out, stop) {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        failure = Some(error);
                        Err(zenavif::Error::Unsupported("AVIF output conversion failed").into())
                    }
                }
            });
        if let Some(error) = failure {
            return Err(error);
        }
        let actual = result.map_err(|e| external(e, stop))?;
        if (
            actual.width,
            actual.height,
            actual.bit_depth,
            actual.color_primaries,
            actual.transfer_characteristics,
        ) != (
            info.width,
            info.height,
            info.bit_depth,
            info.color_primaries,
            info.transfer_characteristics,
        ) {
            return Err("AVIF metadata changed on grid decode".into());
        }
    } else {
        let (pixels, actual) = decoder.decode_full(stop).map_err(|e| external(e, stop))?;
        if (actual.width, actual.height, actual.bit_depth)
            != (info.width, info.height, info.bit_depth)
        {
            return Err("AVIF metadata changed on decode".into());
        }
        convert(0, 0, &pixels, &info, &color, &mut out, stop)?;
    }
    check(stop)?;
    let descriptor = PixelDescriptor::new(
        PixelFormat::Rgba16Float,
        color.encoding(),
        AlphaMode::Straight,
    )?;
    PixelBuffer::from_tight(info.width, info.height, descriptor, out).map_err(Into::into)
}
fn convert(
    left: u32,
    top: u32,
    pixels: &zenavif::PixelBuffer,
    info: &zenavif::ImageInfo,
    color: &Color,
    out: &mut [u8],
    stop: &impl Stop,
) -> Result<()> {
    let width = pixels.width() as usize;
    let mut row = vec![0f32; width * 4];
    let mut converted = vec![0f32; width * 4];
    for y in 0..pixels.height() as usize {
        check(stop)?;
        for x in 0..width {
            let p = if let Some(img) = pixels.try_as_imgref::<Rgba<u16>>() {
                let v = img[(x, y)];
                [
                    v.r as f32 / 65535.,
                    v.g as f32 / 65535.,
                    v.b as f32 / 65535.,
                    v.a as f32 / 65535.,
                ]
            } else if let Some(img) = pixels.try_as_imgref::<Rgb<u16>>() {
                let v = img[(x, y)];
                [
                    v.r as f32 / 65535.,
                    v.g as f32 / 65535.,
                    v.b as f32 / 65535.,
                    1.,
                ]
            } else if let Some(img) = pixels.try_as_imgref::<Rgba<u8>>() {
                let v = img[(x, y)];
                [
                    v.r as f32 / 255.,
                    v.g as f32 / 255.,
                    v.b as f32 / 255.,
                    v.a as f32 / 255.,
                ]
            } else if let Some(img) = pixels.try_as_imgref::<Rgb<u8>>() {
                let v = img[(x, y)];
                [v.r as f32 / 255., v.g as f32 / 255., v.b as f32 / 255., 1.]
            } else {
                return Err("unsupported AVIF decoded layout".into());
            };
            // ManagedAvifDecoder already outputs straight RGB, even when its
            // source metadata says premultiplied. Do not unassociate twice.
            row[x * 4..x * 4 + 4].copy_from_slice(&p);
        }
        match color {
            Color::Icc(transform) => transform
                .transform(&row, &mut converted)
                .map_err(|e| e.to_string())?,
            _ => {
                converted.copy_from_slice(&row);
                for p in converted.as_chunks_mut::<4>().0 {
                    let rgb = if matches!(color, Color::Pq) {
                        yue_color::rec2020_to_rec709(
                            [p[0], p[1], p[2]].map(|v| yue_color::pq_to_nits(v as f64) / 80.),
                        )
                    } else {
                        [p[0], p[1], p[2]].map(|v| yue_color::srgb_to_linear(v as f64))
                    };
                    for (v, value) in p[..3].iter_mut().zip(rgb) {
                        *v = value as f32;
                    }
                }
            }
        }
        for x in 0..width {
            converted[x * 4 + 3] = row[x * 4 + 3];
            let offset = ((top as usize + y) * info.width as usize + left as usize + x) * 8;
            let target = out
                .get_mut(offset..offset + 8)
                .ok_or("AVIF tile outside output")?;
            for (channel, value) in converted[x * 4..x * 4 + 4].iter().enumerate() {
                let value = half::f16::from_f32(*value);
                if !value.is_finite() {
                    return Err("AVIF conversion cannot be represented by finite FP16".into());
                }
                target[channel * 2..channel * 2 + 2].copy_from_slice(&value.to_le_bytes());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Unstoppable;
    fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut bytes = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(body);
        bytes
    }
    #[test]
    fn metadata_preflight_rejects_oversized_lengths_counts_transforms_and_nesting() {
        let check_boxes = |v: &[u8]| boxes(v, 0, &mut 0, &Unstoppable);
        for kind in [b"irot", b"imir", b"clap", b"moov"] {
            assert!(check_boxes(&boxed(kind, &[])).is_err());
        }
        let mut iinf = vec![1, 0, 0, 0];
        iinf.extend_from_slice(&8195_u32.to_be_bytes());
        assert!(
            check_boxes(&boxed(b"iinf", &iinf))
                .unwrap_err()
                .to_string()
                .contains("item count")
        );
        let mut nested = boxed(b"free", &[]);
        for _ in 0..9 {
            nested = boxed(b"ipco", &nested);
        }
        assert!(
            check_boxes(&nested)
                .unwrap_err()
                .to_string()
                .contains("nesting")
        );
        assert!(check_boxes(&boxed(b"meta", &vec![0; 1024 * 1024 + 1])).is_err());
        for length in [0_u32, 1, 7, u32::MAX] {
            let mut v = length.to_be_bytes().to_vec();
            v.extend_from_slice(b"free");
            assert!(check_boxes(&v).is_err());
        }
        let mut count = 20_000;
        assert!(boxes(&boxed(b"free", &[]), 0, &mut count, &Unstoppable).is_err());
    }
    #[test]
    fn hostile_icc_tables_curves_luts_and_offsets_fail_before_cms() {
        let valid = ColorProfile::new_srgb().encode().unwrap();
        validate_icc(&valid).unwrap();
        let mut profile = valid.clone();
        profile[128..132].copy_from_slice(&129_u32.to_be_bytes());
        assert!(validate_icc(&profile).is_err());
        profile = valid.clone();
        profile[136..140].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(validate_icc(&profile).is_err());
        for (kind, count) in [
            (b"curv", 4097_u32),
            (b"mluc", 129),
            (b"mft1", 0),
            (b"mAB ", 0),
        ] {
            let mut profile = vec![0; 156];
            profile[..4].copy_from_slice(&156_u32.to_be_bytes());
            profile[128..132].copy_from_slice(&1_u32.to_be_bytes());
            profile[136..140].copy_from_slice(&144_u32.to_be_bytes());
            profile[140..144].copy_from_slice(&12_u32.to_be_bytes());
            profile[144..148].copy_from_slice(kind);
            profile[152..156].copy_from_slice(&count.to_be_bytes());
            assert!(validate_icc(&profile).is_err());
        }
        for length in 0..132 {
            assert!(validate_icc(&valid[..length]).is_err());
        }
    }
    #[test]
    fn sniff_reads_only_real_ftyp_brands_and_never_minor_version_or_next_box() {
        let mut body = b"mif1avif".to_vec();
        body.extend_from_slice(b"mif1");
        let mut v = boxed(b"ftyp", &body);
        assert!(!is_avif(&v));
        v.extend_from_slice(&boxed(b"avif", b"1234"));
        assert!(!is_avif(&v));
        assert!(is_avif(&boxed(b"ftyp", b"avif\0\0\0\0")));
        assert!(is_avif(&boxed(b"ftyp", b"mif1\0\0\0\0avif")));
    }
}
