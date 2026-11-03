use snapclip_history::{
    StoreError, artifact_store::CaptureArtifactStore, pixel_master::PixelMaster,
};
use snapclip_model::pixels::{AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding, PixelSource};
use snapclip_model::{CaptureMetadata, PixelFormat};

fn metadata() -> CaptureMetadata {
    CaptureMetadata {
        session_id: "master-test".into(),
        width: 1024,
        height: 1200,
        dpi: 144,
        source: PixelSource::new(
            PixelDescriptor::new(
                PixelFormat::Rgba16Float,
                PixelEncoding::ScRgb,
                AlphaMode::Straight,
            )
            .unwrap(),
            41,
        ),
        captured_at_unix_ms: 7,
        monitor_device_name: Some("test-display".into()),
    }
}
fn rows(meta: &CaptureMetadata, first: u32, count: u32) -> PixelBuffer {
    let mut bytes = Vec::new();
    for y in first..first + count {
        for x in 0..meta.width {
            for bits in [
                0xbc00 | (x as u16 & 0x1ff),
                0x4900 | (y as u16 & 0xff),
                0x8000,
                0x3800,
            ] {
                bytes.extend_from_slice(&bits.to_le_bytes());
            }
        }
    }
    PixelBuffer::from_tight(meta.width, count, meta.source.descriptor(), bytes).unwrap()
}
struct Directory(std::path::PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self(std::env::temp_dir().join(format!(
            "snapclip-master-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn compressed_master_preserves_fp16_bits_metadata_and_cross_tile_rows() {
    let dir = Directory::new();
    let store = CaptureArtifactStore::new(&dir.0);
    let meta = metadata();
    let mut largest = 0;
    let started = std::time::Instant::now();
    let reference = store
        .write_pixel_master(&meta, |first, count| {
            largest = largest.max(count as usize * meta.width as usize * 8);
            Ok(rows(&meta, first, count))
        })
        .unwrap();
    let write_ms = started.elapsed().as_secs_f64() * 1000.;
    assert!(largest <= 4 * 1024 * 1024);
    assert!(reference.byte_len < u64::from(meta.width) * u64::from(meta.height) * 8);
    let started = std::time::Instant::now();
    let mut master = PixelMaster::open(&reference).unwrap();
    let open_ms = started.elapsed().as_secs_f64() * 1000.;
    assert_eq!(master.metadata(), &meta);
    for (first, count) in [(0, 2), (510, 5), (1198, 2)] {
        let decoded = master.read_rows(first, count).unwrap();
        assert_eq!(decoded.descriptor(), meta.source.descriptor());
        assert_eq!(decoded.as_bytes(), rows(&meta, first, count).as_bytes());
    }
    assert!(master.read_rows(1200, 1).is_err());
    assert!(master.read_rows(0, u32::MAX).is_err());
    println!(
        "synthetic_master raw_bytes={} encoded_bytes={} largest_raw_tile={} write_with_fixture_ms={write_ms:.3} open_verify_ms={open_ms:.3}",
        u64::from(meta.width) * u64::from(meta.height) * 8,
        reference.byte_len,
        largest
    );
}
#[test]
fn failed_source_does_not_publish_a_partial_master() {
    let dir = Directory::new();
    let store = CaptureArtifactStore::new(&dir.0);
    let meta = metadata();
    assert!(
        store
            .write_pixel_master(&meta, |first, count| {
                if first > 0 {
                    Err(StoreError::Internal("injected source failure".into()))
                } else {
                    Ok(rows(&meta, first, count))
                }
            })
            .is_err()
    );
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
}
#[test]
fn damaged_compressed_pixels_are_rejected() {
    use std::io::{Seek, SeekFrom, Write};
    let dir = Directory::new();
    let store = CaptureArtifactStore::new(&dir.0);
    let meta = metadata();
    let mut reference = store
        .write_pixel_master(&meta, |first, count| Ok(rows(&meta, first, count)))
        .unwrap();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(&reference.absolute_path)
        .unwrap();
    file.seek(SeekFrom::End(-4)).unwrap();
    file.write_all(&[0; 4]).unwrap();
    drop(file);
    assert!(
        PixelMaster::open(&reference).is_err(),
        "encoded identity must catch replacement"
    );
    // A forged/refreshed reference still cannot bypass tile/deflate validation.
    reference.content_fingerprint = blake3::hash(&std::fs::read(&reference.absolute_path).unwrap())
        .to_hex()
        .to_string();
    let mut master = PixelMaster::open(&reference).unwrap();
    assert!(master.read_rows(1198, 2).is_err());
}

#[test]
fn invalid_tiles_and_old_versions_are_rejected_without_partial_publication() {
    use std::io::{Seek, SeekFrom, Write};
    let dir = Directory::new();
    let store = CaptureArtifactStore::new(&dir.0);
    let meta = metadata();
    assert!(
        store
            .write_pixel_master(&meta, |_, _| Ok(PixelBuffer::from_tight(
                1,
                1,
                PixelDescriptor::bgra8(),
                vec![0; 4]
            )
            .unwrap()))
            .is_err()
    );
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
    let mut reference = store
        .write_pixel_master(&meta, |first, count| Ok(rows(&meta, first, count)))
        .unwrap();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(&reference.absolute_path)
        .unwrap();
    file.seek(SeekFrom::Start(4)).unwrap();
    file.write_all(&[0]).unwrap();
    drop(file);
    reference.content_fingerprint = blake3::hash(&std::fs::read(&reference.absolute_path).unwrap())
        .to_hex()
        .to_string();
    assert!(
        PixelMaster::open(&reference).is_err(),
        "old development layouts must not be guessed"
    );
}

#[test]
fn sdr_and_relative_linear_masters_keep_their_original_definition() {
    let dir = Directory::new();
    let store = CaptureArtifactStore::new(&dir.0);
    for descriptor in [
        PixelDescriptor::bgra8(),
        PixelDescriptor::new(
            PixelFormat::Rgba16Float,
            PixelEncoding::LinearSrgb,
            AlphaMode::Premultiplied,
        )
        .unwrap(),
    ] {
        let mut meta = metadata();
        meta.width = 2;
        meta.height = 3;
        meta.source = PixelSource::new(descriptor, 52);
        let sample = if descriptor.format() == PixelFormat::Bgra8Unorm {
            vec![2, 3, 4, 5]
        } else {
            vec![0, 0xb8, 0, 0x44, 0, 0x80, 0, 0x38]
        };
        let bytes = sample.repeat(6);
        let reference = store
            .write_pixel_master(&meta, |_, count| {
                Ok(PixelBuffer::from_tight(2, count, descriptor, bytes.clone()).unwrap())
            })
            .unwrap();
        let mut master = PixelMaster::open(&reference).unwrap();
        assert_eq!(master.metadata(), &meta);
        assert_eq!(master.read_rows(0, 3).unwrap().as_bytes(), bytes);
    }
}
