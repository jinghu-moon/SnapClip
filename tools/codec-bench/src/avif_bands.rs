//! Qualification of the real row-source/color contract. The explicit product
//! AVIF feature uses the same shared codec; MIT Yue stays codec-free.
use almost_enough::StopToken;
#[cfg(test)]
use enough::Stop;
use enough::Unstoppable;
use rgb::Rgba;
use snapclip_capture::ports::BandSource;
use snapclip_history::{artifact_store::CaptureArtifactStore, pixel_master::PixelMaster};
use snapclip_model::{
    CaptureMetadata, PixelFormat,
    pixels::{AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding, PixelSource},
};

#[cfg(test)]
use super::avif_grid::MAX_PAYLOAD;

const READ_BYTES: usize = 1024 * 1024;

pub(super) fn encode(
    bands: &mut dyn BandSource,
    width: u32,
    stop: StopToken,
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    snapclip_avif::write_hdr(bands, width, &std::env::temp_dir(), &mut bytes, stop)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

struct MasterBands<'a>(&'a mut PixelMaster);
impl BandSource for MasterBands<'_> {
    fn source(&self) -> PixelSource {
        self.0.metadata().source
    }
    fn rows(&self) -> u32 {
        self.0.metadata().height
    }
    fn read_rows(&mut self, first: u32, rows: u32, out: &mut Vec<u8>) -> bool {
        match self.0.read_rows(first, rows) {
            Ok(pixels) => {
                *out = pixels.into_bytes();
                true
            }
            Err(_) => false,
        }
    }
}
fn encode_master(master: &mut PixelMaster, stop: StopToken) -> Result<Vec<u8>, String> {
    let width = master.metadata().width;
    encode(&mut MasterBands(master), width, stop)
}

// Original FP16 bits: black, 80/1000/10000 nit neutral, a representable wide-
// gamut negative scRGB red, and a bright color. No original is tone mapped.
const RGB_BITS: [[u16; 3]; 6] = [
    [0, 0, 0],
    [0x3c00; 3],
    [0x4a40; 3],
    [0x57d0; 3],
    [0xb000, 0x3c00, 0x3400],
    [0x4400, 0x4000, 0x3800],
];
const ALPHA_BITS: [u16; 4] = [0, 0x3400, 0x3800, 0x3c00];
fn fixture_key(x: usize, y: usize) -> usize {
    ((x / 16 + y / 16) % RGB_BITS.len()) * 4 + (x / 32 + y / 32) % 4
}
fn fixture_bits(key: usize) -> [u16; 4] {
    let [r, g, b] = RGB_BITS[key / 4];
    [r, g, b, ALPHA_BITS[key % 4]]
}
fn source() -> PixelSource {
    PixelSource::new(
        PixelDescriptor::new(
            PixelFormat::Rgba16Float,
            PixelEncoding::ScRgb,
            AlphaMode::Straight,
        )
        .unwrap(),
        73,
    )
}
struct SyntheticBands {
    width: u32,
    height: u32,
    reads: usize,
    largest_read: usize,
}
impl SyntheticBands {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            reads: 0,
            largest_read: 0,
        }
    }
}
impl BandSource for SyntheticBands {
    fn source(&self) -> PixelSource {
        source()
    }
    fn rows(&self) -> u32 {
        self.height
    }
    fn read_rows(&mut self, first: u32, rows: u32, out: &mut Vec<u8>) -> bool {
        if rows == 0 || first.checked_add(rows).is_none_or(|end| end > self.height) {
            return false;
        }
        self.reads += 1;
        out.clear();
        out.reserve(rows as usize * self.width as usize * 8);
        for y in first..first + rows {
            for x in 0..self.width {
                for bits in fixture_bits(fixture_key(x as usize, y as usize)) {
                    out.extend_from_slice(&bits.to_le_bytes());
                }
            }
        }
        self.largest_read = self.largest_read.max(out.len());
        true
    }
}
fn palette() -> Vec<Rgba<u16>> {
    let raw = (0..24)
        .flat_map(fixture_bits)
        .flat_map(u16::to_le_bytes)
        .collect();
    let pixels = PixelBuffer::from_tight(24, 1, source().descriptor(), raw).unwrap();
    snapclip_model::pixel_color::rec2020_pq16_pixels(&pixels)
        .unwrap()
        .map(|p| {
            let [r, g, b, a] = p.unwrap();
            Rgba::new(r, g, b, a)
        })
        .collect()
}
fn metadata(width: u32, height: u32) -> CaptureMetadata {
    CaptureMetadata {
        session_id: "avif-band-probe".into(),
        width,
        height,
        dpi: 96,
        source: source(),
        captured_at_unix_ms: 0,
        monitor_device_name: None,
    }
}
fn write_master(
    store: &CaptureArtifactStore,
    bands: &mut SyntheticBands,
) -> Result<snapclip_model::ArtifactRef, String> {
    let meta = metadata(bands.width, bands.height);
    store
        .write_pixel_master(&meta, |first, count| {
            let mut bytes = Vec::new();
            if !bands.read_rows(first, count, &mut bytes) {
                return Err(snapclip_history::StoreError::InvalidPublication(
                    "fixture rows refused".into(),
                ));
            }
            PixelBuffer::from_tight(meta.width, count, meta.source.descriptor(), bytes)
                .map_err(|e| snapclip_history::StoreError::InvalidPublication(e.into()))
        })
        .map_err(|e| e.to_string())
}

struct Directory(std::path::PathBuf);
impl Directory {
    fn new() -> Result<Self, String> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "snapclip-avif-bands-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).map_err(|e| e.to_string())?;
        Ok(Self(path))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) fn fixtures(directory: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let mut bands = SyntheticBands::new(133, 257);
    let bytes = encode(&mut bands, 133, StopToken::new(Unstoppable))?;
    std::fs::write(directory.join("bands-pq.avif"), &bytes).map_err(|e| e.to_string())?;
    let temporary = Directory::new()?;
    let reference = write_master(&CaptureArtifactStore::new(&temporary.0), &mut bands)?;
    let mut master = PixelMaster::open(&reference).map_err(|e| e.to_string())?;
    let master_bytes = encode_master(&mut master, StopToken::new(Unstoppable))?;
    if master_bytes != bytes {
        return Err("row source and master produced different AVIF bytes".into());
    }
    std::fs::write(directory.join("master-pq.avif"), master_bytes).map_err(|e| e.to_string())?;
    let mut original = Vec::new();
    bands.read_rows(0, 257, &mut original);
    std::fs::write(directory.join("source-rgba16f-le.bin"), original).map_err(|e| e.to_string())?;
    let palette = palette();
    let raw: Vec<_> = (0..257)
        .flat_map(|y| {
            let palette = &palette;
            (0..133).flat_map(move |x| {
                let p = palette[fixture_key(x, y)];
                [p.r, p.g, p.b, p.a].into_iter().flat_map(u16::to_le_bytes)
            })
        })
        .collect();
    std::fs::write(directory.join("expected-rgba64le.bin"), raw).map_err(|e| e.to_string())
}

fn measured<T>(name: &str, body: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let start = std::time::Instant::now();
    let (result, peak) = super::alloc_probe::peak_extra(body);
    let largest = super::alloc_probe::largest_allocation();
    println!(
        "resource_{name} elapsed_ms={:.3} heap_peak_delta={peak} largest_allocation={largest}",
        start.elapsed().as_secs_f64() * 1000.
    );
    if peak >= 32 * 1024 * 1024 || largest >= 16 * 1024 * 1024 {
        return Err(format!("{name} exceeded unchanged 32 MiB/16 MiB budget"));
    }
    result
}
pub(super) fn resources(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 64 * 1024 * 1024 {
        return Err("unsupported band resource geometry".into());
    }
    let temporary = Directory::new()?;
    let mut bands = SyntheticBands::new(width, height);
    let bytes = measured("encode", || {
        encode(&mut bands, width, StopToken::new(Unstoppable))
    })?;
    let largest_read = bands.largest_read;
    if largest_read > READ_BYTES {
        return Err("source read exceeded row scratch budget".into());
    }
    let reference = measured("master_write", || {
        write_master(&CaptureArtifactStore::new(&temporary.0), &mut bands)
    })?;
    let master_bytes = measured("master_encode", || {
        let mut master = PixelMaster::open(&reference).map_err(|e| e.to_string())?;
        encode_master(&mut master, StopToken::new(Unstoppable))
    })?;
    if master_bytes != bytes {
        return Err("source/master AVIF bytes differ".into());
    }
    let palette = palette();
    measured("decode", || {
        let mut checked = 0u64;
        let mut error = 0u16;
        let info = super::avif_grid::decode(&bytes, &Unstoppable, |left, top, pixels| {
            let image = pixels.try_as_imgref::<Rgba<u16>>().unwrap();
            for (ix, p) in image.pixels().enumerate() {
                let expected = palette[fixture_key(
                    left as usize + ix % image.width(),
                    top as usize + ix / image.width(),
                )];
                for (a, b) in [
                    (p.r, expected.r),
                    (p.g, expected.g),
                    (p.b, expected.b),
                    (p.a, expected.a),
                ] {
                    error = error.max(a.abs_diff(b));
                }
                checked += 1;
            }
            Ok(())
        })?;
        let code_error = f64::from(error) * 1023. / 65535.;
        println!(
            "band_roundtrip checked_pixels={checked} max_code_error={code_error:.6} largest_source_read={largest_read} encoded_bytes={}",
            bytes.len()
        );
        if checked != u64::from(width) * u64::from(height)
            || code_error > 2.
            || (info.width, info.height, info.bit_depth, info.has_alpha)
                != (width, height, 10, true)
        {
            return Err("band/master pixel gate failed".into());
        }
        Ok(())
    })?;
    let stop = super::TimedStop::new(std::time::Duration::from_millis(10));
    let mut source = SyntheticBands::new(width, height);
    let start = std::time::Instant::now();
    let cancelled = encode(&mut source, width, StopToken::new(stop.clone()))
        .is_err_and(|e| e.to_lowercase().contains("cancel"));
    let passed = stop.passed.load(std::sync::atomic::Ordering::Relaxed);
    println!(
        "resource_cancel operation=bands deadline_ms=10 elapsed_ms={:.3} successful_checks={passed} reads={} cancelled={cancelled}",
        start.elapsed().as_secs_f64() * 1000.,
        source.reads
    );
    if !cancelled || passed == 0 || source.reads == 0 {
        return Err("band cancellation was not exercised".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "avif_bands_tests.rs"]
mod tests;
