//! Bounded, explicit-color PNG derivatives of the immutable pixel master.

use snapclip_capture::ports::BandSource;
use snapclip_model::pixels::{PixelBuffer, PixelEncoding, PixelSource};
use std::io::Write;

/// Stream a checked historical original. scRGB masters become Rec.2020/PQ
/// PNG16; relative/SDR masters become explicitly SDR PNG8. Existing PNG8 sources
/// are copied losslessly with their encoded identity checked before commit.
/// The caller owns the atomic destination; an error must discard partial bytes.
pub fn write_capture_source_png(
    source: &snapclip_model::CaptureSource,
    mut output: impl Write,
) -> Result<(), String> {
    if source.artifact.dimensions.width != source.metadata.width
        || source.artifact.dimensions.height != source.metadata.height
    {
        return Err("history source geometry differs from its reference".into());
    }
    if source.artifact.mime == "image/png" {
        use std::io::Read as _;
        if source.metadata.source.descriptor() != snapclip_model::pixels::PixelDescriptor::bgra8() {
            return Err("history PNG is not the recorded SDR source".into());
        }
        let mut input =
            std::fs::File::open(&source.artifact.absolute_path).map_err(|e| e.to_string())?;
        if input.metadata().map_err(|e| e.to_string())?.len() != source.artifact.byte_len {
            return Err("history PNG length differs from its reference".into());
        }
        let mut hash = blake3::Hasher::new();
        let mut scratch = [0; 64 * 1024];
        loop {
            let n = input.read(&mut scratch).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            hash.update(&scratch[..n]);
            output.write_all(&scratch[..n]).map_err(|e| e.to_string())?;
        }
        if hash.finalize().to_hex().as_str() != source.artifact.content_fingerprint {
            return Err("history PNG fingerprint differs from its reference".into());
        }
        return Ok(());
    }
    let master = snapclip_history::pixel_master::PixelMaster::open(&source.artifact)
        .map_err(|e| e.to_string())?;
    if master.metadata() != &source.metadata {
        return Err("history master metadata differs from its reference".into());
    }
    let policy = if source.metadata.source.descriptor().encoding() == PixelEncoding::ScRgb {
        ColorExport::PreserveHdr
    } else {
        ColorExport::Sdr {
            source_peak_nits: 100.,
        }
    };
    write_png(
        &mut MasterBands(master),
        source.metadata.width,
        policy,
        output,
    )
}

struct MasterBands(snapclip_history::pixel_master::PixelMaster);
impl BandSource for MasterBands {
    fn source(&self) -> PixelSource {
        self.0.metadata().source
    }
    fn rows(&self) -> u32 {
        self.0.metadata().height
    }
    fn read_rows(&mut self, first: u32, count: u32, out: &mut Vec<u8>) -> bool {
        match self.0.read_rows(first, count) {
            Ok(pixels) => {
                *out = pixels.into_bytes();
                true
            }
            Err(_) => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorExport {
    PreserveHdr,
    /// The target is 100 nit. Peak is fixed for all bands in this export.
    Sdr {
        source_peak_nits: f64,
    },
}

pub fn write_png(
    bands: &mut dyn BandSource,
    width: u32,
    policy: ColorExport,
    output: impl Write,
) -> Result<(), String> {
    let source = bands.source();
    let height = bands.rows();
    let descriptor = source.descriptor();
    let row = descriptor.row_bytes(width).ok_or("pixel row overflow")?;
    if width == 0 || height == 0 || row > 64 * 1024 * 1024 {
        return Err("PNG dimensions exceed the row budget".into());
    }
    let hdr = match policy {
        ColorExport::PreserveHdr => {
            if descriptor.encoding() != PixelEncoding::ScRgb {
                return Err("preserve HDR requires an absolute scRGB source".into());
            }
            true
        }
        ColorExport::Sdr { source_peak_nits } => {
            if !source_peak_nits.is_finite() || source_peak_nits <= 0.0 {
                return Err("invalid SDR export peak".into());
            }
            false
        }
    };
    let mut info = png::Info::with_size(width, height);
    info.color_type = png::ColorType::Rgba;
    info.bit_depth = if hdr {
        png::BitDepth::Sixteen
    } else {
        png::BitDepth::Eight
    };
    if hdr {
        info.coding_independent_code_points = Some(png::CodingIndependentCodePoints {
            color_primaries: 9,
            transfer_function: 16,
            matrix_coefficients: 0,
            is_video_full_range_image: true,
        });
    }
    let mut encoder = png::Encoder::with_info(output, info).map_err(|e| e.to_string())?;
    if !hdr {
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    }
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    // png 0.18 reads cICP but its header encoder does not emit Info's cICP.
    // Write the standard chunk before IDAT; the library owns length and CRC.
    if hdr {
        writer
            .write_chunk(png::chunk::cICP, &[9, 16, 0, 1])
            .map_err(|e| e.to_string())?;
    }
    {
        let mut stream = writer.stream_writer().map_err(|e| e.to_string())?;
        let band_rows = ((4 * 1024 * 1024 / row).max(1) as u32).min(256);
        let mut first = 0;
        let mut bytes = Vec::new();
        while first < height {
            let count = band_rows.min(height - first);
            bytes.clear();
            if bands.source() != source
                || bands.rows() != height
                || !bands.read_rows(first, count, &mut bytes)
                || bands.source() != source
                || bands.rows() != height
            {
                return Err("pixel source changed or refused an export band".into());
            }
            let pixels =
                PixelBuffer::from_tight(width, count, descriptor, std::mem::take(&mut bytes))
                    .map_err(str::to_owned)?;
            let encoded = match policy {
                ColorExport::PreserveHdr => {
                    snapclip_model::pixel_color::to_rec2020_pq16(&pixels).map_err(str::to_owned)?
                }
                ColorExport::Sdr { source_peak_nits } => {
                    let mut bytes =
                        snapclip_model::pixel_color::to_sdr_bgra8(&pixels, source_peak_nits)
                            .map_err(str::to_owned)?
                            .into_bytes();
                    for pixel in bytes.chunks_exact_mut(4) {
                        pixel.swap(0, 2);
                    }
                    bytes
                }
            };
            stream.write_all(&encoded).map_err(|e| e.to_string())?;
            bytes = pixels.into_bytes();
            first += count;
        }
        stream.finish().map_err(|e| e.to_string())?;
    }
    writer.finish().map_err(|e| e.to_string())
}

/// Whole images use the same bounded encoder as scroll and durable masters.
pub fn encode_pixels(pixels: &PixelBuffer, policy: ColorExport) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    write_png(
        &mut PixelBands(pixels),
        pixels.size()[0],
        policy,
        &mut output,
    )?;
    Ok(output)
}

struct PixelBands<'a>(&'a PixelBuffer);
impl BandSource for PixelBands<'_> {
    fn source(&self) -> PixelSource {
        PixelSource::new(self.0.descriptor(), 0)
    }
    fn rows(&self) -> u32 {
        self.0.size()[1]
    }
    fn read_rows(&mut self, first: u32, count: u32, out: &mut Vec<u8>) -> bool {
        let row = self.0.row_stride();
        let Some(end) = first.checked_add(count).filter(|end| *end <= self.rows()) else {
            return false;
        };
        let Some(bytes) = self
            .0
            .as_bytes()
            .get(first as usize * row..end as usize * row)
        else {
            return false;
        };
        out.clear();
        out.extend_from_slice(bytes);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snapclip_model::pixels::{
        AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding, PixelSource,
    };
    use std::io::Cursor;

    struct Bands {
        pixels: PixelBuffer,
        source: PixelSource,
        changed: bool,
    }
    impl snapclip_capture::ports::BandSource for Bands {
        fn source(&self) -> PixelSource {
            PixelSource::new(
                self.source.descriptor(),
                self.source.generation() + u64::from(self.changed),
            )
        }
        fn rows(&self) -> u32 {
            self.pixels.size()[1]
        }
        fn read_rows(&mut self, first: u32, count: u32, out: &mut Vec<u8>) -> bool {
            let row = self.pixels.row_stride();
            let Some(bytes) = self
                .pixels
                .as_bytes()
                .get(first as usize * row..(first + count) as usize * row)
            else {
                return false;
            };
            out.clear();
            out.extend_from_slice(bytes);
            true
        }
    }
    fn bands() -> Bands {
        let descriptor = PixelDescriptor::new(
            snapclip_model::PixelFormat::Rgba16Float,
            PixelEncoding::ScRgb,
            AlphaMode::Straight,
        )
        .unwrap();
        let bytes = [1.25_f32, 1.25, 1.25, 0.5, 12.5, 12.5, 12.5, 1.0]
            .into_iter()
            .flat_map(|v| half::f16::from_f32(v).to_le_bytes())
            .collect();
        Bands {
            pixels: PixelBuffer::from_tight(1, 2, descriptor, bytes)
                .unwrap()
                .into(),
            source: PixelSource::new(descriptor, 77),
            changed: false,
        }
    }

    #[test]
    fn hdr_png_is_sixteen_bit_pq_and_roundtrips_through_the_real_yue_decoder() {
        let mut bands = bands();
        let original = bands.pixels.as_bytes().to_vec();
        let mut bytes = Vec::new();
        write_png(&mut bands, 1, ColorExport::PreserveHdr, &mut bytes).unwrap();
        let reader = png::Decoder::new(Cursor::new(&bytes)).read_info().unwrap();
        assert_eq!(reader.info().bit_depth, png::BitDepth::Sixteen);
        let cicp = reader.info().coding_independent_code_points.unwrap();
        assert_eq!(
            (
                cicp.color_primaries,
                cicp.transfer_function,
                cicp.matrix_coefficients,
                cicp.is_video_full_range_image
            ),
            (9, 16, 0, true)
        );
        let decoded = yue_component::ImagePreviewPixels::decode(
            &bytes,
            &yue_component::ImagePreviewLimits::default(),
        )
        .unwrap();
        assert_eq!(
            decoded.color_space(),
            yue_component::ImagePreviewColorSpace::ScRgb
        );
        let floats: Vec<f32> = decoded
            .as_bytes()
            .chunks_exact(2)
            .map(|b| half::f16::from_le_bytes([b[0], b[1]]).to_f32())
            .collect();
        for (actual, expected) in floats
            .into_iter()
            .zip([1.25, 1.25, 1.25, 0.5, 12.5, 12.5, 12.5, 1.0])
        {
            assert!((actual - expected).abs() < 0.01, "{actual} != {expected}");
        }
        assert_eq!(bands.pixels.as_bytes(), original);
    }

    #[test]
    fn explicit_sdr_png_matches_the_same_derivative_pixels() {
        let mut bands = bands();
        let expected = snapclip_model::pixel_color::to_sdr_bgra8(&bands.pixels, 1000.0).unwrap();
        let mut bytes = Vec::new();
        write_png(
            &mut bands,
            1,
            ColorExport::Sdr {
                source_peak_nits: 1000.0,
            },
            &mut bytes,
        )
        .unwrap();
        let decoded = image::load_from_memory(&bytes)
            .unwrap()
            .into_rgba8()
            .into_raw();
        for (actual, expected) in decoded
            .chunks_exact(4)
            .zip(expected.as_bytes().chunks_exact(4))
        {
            assert_eq!(actual, [expected[2], expected[1], expected[0], expected[3]]);
        }
    }
}
