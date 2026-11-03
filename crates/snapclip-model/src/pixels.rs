//! Framework-free capture/storage pixel contract. No color conversion, GPU handle
//! or monitor lookup occurs here. Byte layout alone never establishes luminance.
use crate::PixelFormat;
use serde::{Deserialize, Serialize};

/// Immutable source rows requested by codecs, capture and durable storage.
/// A source may read from pixels, a file, or bounded storage. Returned rows are
/// top-down, tightly packed in `source()`'s format. Failure supplies no artifact.
pub trait BandSource: Send {
    fn source(&self) -> PixelSource;
    fn rows(&self) -> u32;
    fn read_rows(&mut self, first_row: u32, rows: u32, out: &mut Vec<u8>) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelEncoding {
    /// Encoded sRGB, relative SDR white.
    Srgb,
    /// Linear Rec.709, relative SDR white (not absolute HDR).
    LinearSrgb,
    /// Linear Rec.709/scRGB; 1.0 = 80 nit. Negative RGB is valid.
    ScRgb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlphaMode {
    Straight,
    Premultiplied,
    /// Every pixel must carry alpha one; unknown source alpha is not opaque.
    Opaque,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "DescriptorRecord", into = "DescriptorRecord")]
pub struct PixelDescriptor {
    format: PixelFormat,
    encoding: PixelEncoding,
    alpha: AlphaMode,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DescriptorRecord {
    format: PixelFormat,
    encoding: PixelEncoding,
    alpha: AlphaMode,
}
impl TryFrom<DescriptorRecord> for PixelDescriptor {
    type Error = &'static str;
    fn try_from(record: DescriptorRecord) -> Result<Self, Self::Error> {
        Self::new(record.format, record.encoding, record.alpha)
    }
}
impl From<PixelDescriptor> for DescriptorRecord {
    fn from(value: PixelDescriptor) -> Self {
        Self {
            format: value.format,
            encoding: value.encoding,
            alpha: value.alpha,
        }
    }
}

impl PixelDescriptor {
    /// The existing SDR capture contract: encoded sRGB, straight BGRA8.
    pub const fn bgra8() -> Self {
        Self {
            format: PixelFormat::Bgra8Unorm,
            encoding: PixelEncoding::Srgb,
            alpha: AlphaMode::Straight,
        }
    }
    pub fn new(
        format: PixelFormat,
        encoding: PixelEncoding,
        alpha: AlphaMode,
    ) -> Result<Self, &'static str> {
        if !matches!(
            (format, encoding),
            (PixelFormat::Bgra8Unorm, PixelEncoding::Srgb)
                | (
                    PixelFormat::Rgba16Float,
                    PixelEncoding::LinearSrgb | PixelEncoding::ScRgb
                )
        ) {
            return Err("pixel layout and color encoding do not match");
        }
        Ok(Self {
            format,
            encoding,
            alpha,
        })
    }
    pub fn format(self) -> PixelFormat {
        self.format
    }
    pub fn encoding(self) -> PixelEncoding {
        self.encoding
    }
    pub fn alpha(self) -> AlphaMode {
        self.alpha
    }
    pub fn bytes_per_pixel(self) -> usize {
        self.format.bytes_per_pixel() as usize
    }
    pub fn row_bytes(self, width: u32) -> Option<usize> {
        (width as usize).checked_mul(self.bytes_per_pixel())
    }
}

/// Frozen source definition. A generation belongs to one acquisition/session;
/// its owner changes it whenever the source color interpretation changes.
/// It is not an export-job cancellation counter or a display-output revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PixelSource {
    descriptor: PixelDescriptor,
    generation: u64,
}
impl PixelSource {
    pub const fn new(descriptor: PixelDescriptor, generation: u64) -> Self {
        Self {
            descriptor,
            generation,
        }
    }
    pub fn descriptor(self) -> PixelDescriptor {
        self.descriptor
    }
    pub fn generation(self) -> u64 {
        self.generation
    }
}

/// Validated tightly packed top-down source pixels. Tight owned buffers move
/// without copying; padding is removed once. All float values must be finite,
/// alpha is [0,1], but RGB is neither clamped nor quantized.
#[derive(Debug, Clone)]
pub struct PixelBuffer {
    size: [u32; 2],
    descriptor: PixelDescriptor,
    bytes: Vec<u8>,
}
impl PixelBuffer {
    pub fn from_tight(
        width: u32,
        height: u32,
        descriptor: PixelDescriptor,
        bytes: Vec<u8>,
    ) -> Result<Self, &'static str> {
        let stride = descriptor
            .row_bytes(width)
            .ok_or("pixel row size overflow")?;
        Self::from_bytes(width, height, stride, descriptor, bytes)
    }
    /// A padded input includes all declared rows including final-row padding.
    pub fn from_bytes(
        width: u32,
        height: u32,
        row_stride: usize,
        descriptor: PixelDescriptor,
        bytes: Vec<u8>,
    ) -> Result<Self, &'static str> {
        let row = descriptor
            .row_bytes(width)
            .ok_or("pixel row size overflow")?;
        let size = row_stride
            .checked_mul(height as usize)
            .ok_or("pixel buffer size overflow")?;
        if width == 0 || height == 0 || row_stride < row || bytes.len() != size {
            return Err("invalid pixel dimensions, row stride or buffer length");
        }
        for data in bytes.chunks_exact(row_stride) {
            let data = &data[..row];
            if descriptor.format == PixelFormat::Rgba16Float {
                for pixel in data.as_chunks::<8>().0 {
                    let channels: [u16; 4] = std::array::from_fn(|c| {
                        u16::from_le_bytes([pixel[c * 2], pixel[c * 2 + 1]])
                    });
                    if channels.iter().any(|c| c & 0x7c00 == 0x7c00) {
                        return Err("non-finite FP16 pixel channel");
                    }
                    let alpha = channels[3];
                    if (alpha & 0x7fff) > 0x3c00 || (alpha & 0x8000 != 0 && alpha & 0x7fff != 0) {
                        return Err("FP16 alpha is outside [0,1]");
                    }
                    if descriptor.alpha == AlphaMode::Opaque && alpha != 0x3c00 {
                        return Err("opaque FP16 pixels require alpha one");
                    }
                }
            } else if descriptor.alpha == AlphaMode::Opaque
                && data.as_chunks::<4>().0.iter().any(|p| p[3] != 255)
            {
                return Err("opaque BGRA pixels require alpha 255");
            }
        }
        let bytes = if row == row_stride {
            bytes
        } else {
            let mut tight = Vec::new();
            let size = row
                .checked_mul(height as usize)
                .ok_or("tight pixel size overflow")?;
            tight
                .try_reserve_exact(size)
                .map_err(|_| "cannot allocate tight pixel buffer")?;
            for data in bytes.chunks_exact(row_stride) {
                tight.extend_from_slice(&data[..row]);
            }
            tight
        };
        Ok(Self {
            size: [width, height],
            descriptor,
            bytes,
        })
    }
    pub fn size(&self) -> [u32; 2] {
        self.size
    }
    pub fn descriptor(&self) -> PixelDescriptor {
        self.descriptor
    }
    pub fn row_stride(&self) -> usize {
        self.size[0] as usize * self.descriptor.bytes_per_pixel()
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}
