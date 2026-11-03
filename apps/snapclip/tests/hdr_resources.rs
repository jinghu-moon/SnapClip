//! Opt-in Release measurements of the production archive -> history -> PNG path.
//! Synthetic row bands deliberately never allocate a whole-image fixture.
//! Run each ignored test in its own process with --exact --nocapture.
#![cfg(windows)]

use std::{
    alloc::{GlobalAlloc, Layout, System},
    io::BufReader,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use snapclip_app::{
    capture::{HistoryArtifactWriter, pixel_export::write_capture_source_png},
    save::service::{ConflictPolicy, NoCancel, SaveRequest, commit_stream},
};
use snapclip_capture::ports::{ArtifactWriter, BandSource, SelectionSpec};
use snapclip_history::{pixel_master::PixelMaster, store::Store};
use snapclip_model::{
    PixelFormat, Rect,
    pixels::{AlphaMode, PixelDescriptor, PixelEncoding, PixelSource},
};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static LARGEST: AtomicUsize = AtomicUsize::new(0);
static IMAGE_BYTES: AtomicUsize = AtomicUsize::new(usize::MAX);
static FULL_COPIES: AtomicUsize = AtomicUsize::new(0);
struct Allocator;
impl Allocator {
    fn add(bytes: usize) {
        PEAK.fetch_max(LIVE.fetch_add(bytes, Relaxed) + bytes, Relaxed);
        LARGEST.fetch_max(bytes, Relaxed);
        if bytes >= IMAGE_BYTES.load(Relaxed) {
            FULL_COPIES.fetch_add(1, Relaxed);
        }
    }
}
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            Self::add(layout.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            Self::add(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
        LIVE.fetch_sub(layout.size(), Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(p, layout, size) };
        if !p.is_null() {
            LIVE.fetch_sub(layout.size(), Relaxed);
            Self::add(size);
        }
        p
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

#[repr(C)]
#[derive(Default)]
struct Memory {
    cb: u32,
    page_faults: u32,
    peak_working_set: usize,
    working_set: usize,
    quota_peak_paged: usize,
    quota_paged: usize,
    quota_peak_nonpaged: usize,
    quota_nonpaged: usize,
    pagefile: usize,
    peak_pagefile: usize,
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn K32GetProcessMemoryInfo(
        process: *mut core::ffi::c_void,
        counters: *mut Memory,
        size: u32,
    ) -> i32;
}
fn working_set() -> usize {
    let mut memory = Memory {
        cb: size_of::<Memory>() as u32,
        ..Memory::default()
    };
    // SAFETY: current-process pseudo-handle and an initialized repr(C) output.
    assert_ne!(
        unsafe {
            K32GetProcessMemoryInfo(-1isize as *mut _, &mut memory, size_of::<Memory>() as u32)
        },
        0
    );
    memory.working_set
}

fn measured<T>(stage: &str, raw_bytes: usize, f: impl FnOnce() -> T) -> T {
    let rss_start = working_set();
    let running = Arc::new(AtomicBool::new(true));
    let peak_rss = Arc::new(AtomicUsize::new(rss_start));
    let worker = {
        let running = running.clone();
        let peak = peak_rss.clone();
        std::thread::spawn(move || {
            while running.load(Relaxed) {
                peak.fetch_max(working_set(), Relaxed);
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    };
    let baseline = LIVE.load(Relaxed);
    PEAK.store(baseline, Relaxed);
    LARGEST.store(0, Relaxed);
    FULL_COPIES.store(0, Relaxed);
    IMAGE_BYTES.store(raw_bytes, Relaxed);
    let begin = Instant::now();
    let result = f();
    let elapsed = begin.elapsed();
    IMAGE_BYTES.store(usize::MAX, Relaxed);
    let peak_heap = PEAK.load(Relaxed).saturating_sub(baseline);
    let largest = LARGEST.load(Relaxed);
    let copies = FULL_COPIES.load(Relaxed);
    peak_rss.fetch_max(working_set(), Relaxed);
    running.store(false, Relaxed);
    worker.join().unwrap();
    eprintln!(
        "{stage}: raw={raw_bytes} elapsed_ms={:.3} rust_heap_peak_delta={peak_heap} largest_allocation={largest} full_image_allocations={copies} sampled_working_set_peak={} sampled_working_set_delta={}",
        elapsed.as_secs_f64() * 1000.,
        peak_rss.load(Relaxed),
        peak_rss.load(Relaxed).saturating_sub(rss_start)
    );
    assert_eq!(copies, 0, "{stage} allocated a complete FP16 image");
    // Four MiB bands, decoder tiles and PNG working buffers fit in 32 MiB;
    // neither bound grows with image height, and both reject a full 4K image.
    assert!(
        peak_heap < 32 * 1024 * 1024,
        "{stage}: {peak_heap} exceeds bounded working storage"
    );
    assert!(
        largest < 16 * 1024 * 1024,
        "{stage}: oversized allocation {largest}"
    );
    result
}

struct Rows {
    width: u32,
    height: u32,
    source: PixelSource,
}
impl Rows {
    fn new(width: u32, height: u32) -> Self {
        let descriptor = PixelDescriptor::new(
            PixelFormat::Rgba16Float,
            PixelEncoding::ScRgb,
            AlphaMode::Straight,
        )
        .unwrap();
        Self {
            width,
            height,
            source: PixelSource::new(descriptor, 41),
        }
    }
}
impl BandSource for Rows {
    fn source(&self) -> PixelSource {
        self.source
    }
    fn rows(&self) -> u32 {
        self.height
    }
    fn read_rows(&mut self, first: u32, count: u32, bytes: &mut Vec<u8>) -> bool {
        if first.checked_add(count).is_none_or(|end| end > self.height) {
            return false;
        }
        bytes.clear();
        bytes.reserve(self.width as usize * count as usize * 8);
        for y in first..first + count {
            for x in 0..self.width {
                // Contains negative scRGB, >1 highlights, color and transparency.
                // Negative scRGB is chosen inside Rec.2020's representable
                // gamut. Truly unrepresentable colors have a refusal test.
                // Structured gradients are intentional; this is not photo throughput.
                for v in [
                    if x % 257 == 0 {
                        -0.125
                    } else {
                        (x % 256) as f32 / 32.
                    },
                    if x % 257 == 0 {
                        4.
                    } else {
                        (y % 128) as f32 / 16.
                    },
                    if x % 257 == 0 {
                        1.
                    } else {
                        ((x / 64 + y / 64) % 16) as f32 / 8.
                    },
                    if y % 13 == 0 { 0.5 } else { 1. },
                ] {
                    bytes.extend_from_slice(&half::f16::from_f32(v).to_bits().to_le_bytes());
                }
            }
        }
        true
    }
}

struct Scratch(std::path::PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn archive_and_export(width: u32, height: u32) {
    assert!(
        !cfg!(debug_assertions),
        "run this measurement with --release"
    );
    let root = Scratch(std::env::temp_dir().join(format!(
        "snapclip-hdr-resources-{}-{width}-{height}-{}",
        std::process::id(),
        snapclip_model::unix_time_ms()
    )));
    std::fs::create_dir(&root.0).unwrap();
    let store = Store::open(&root.0).unwrap();
    let mut rows = Rows::new(width, height);
    let spec = SelectionSpec {
        region: Rect::new(0, 0, width as i32, height as i32),
        source: rows.source,
        captured_at_unix_ms: 1,
        origin: None,
        corner_radius: 0,
    };
    let writer =
        HistoryArtifactWriter::new(root.0.join("artifacts/capture")).with_history(store.clone());
    let raw_bytes = width as usize * height as usize * 8;
    let artifact = measured("archive+thumbnail+database", raw_bytes, || {
        writer
            .write_bands("hdr-resource-probe", &spec, &mut rows, 0, None)
            .unwrap()
    });
    let page = store.history_page(None, None).unwrap();
    assert_eq!(page.items.len(), 1);
    let source = page.items[0].capture_source.as_ref().unwrap();
    let request = SaveRequest {
        directory: root.0.clone(),
        stem: "export".into(),
        format: snapclip_capture::save_format::SaveFormat::Png,
        conflict: ConflictPolicy::Overwrite,
    };
    let saved = measured("verified-master+PQ-PNG16+atomic-commit", raw_bytes, || {
        commit_stream(&request, &mut NoCancel, |file| {
            write_capture_source_png(source, file).map_err(std::io::Error::other)
        })
        .unwrap()
    });
    eprintln!(
        "{width}x{height}: master_bytes={} png_bytes={}",
        source.artifact.byte_len, saved.bytes
    );
    let mut master = PixelMaster::open(&source.artifact).unwrap();
    let mut expected = Vec::new();
    assert!(rows.read_rows(height - 1, 1, &mut expected));
    assert_eq!(
        master.read_rows(height - 1, 1).unwrap().as_bytes(),
        expected
    );
    assert_eq!(master.metadata().source.generation(), 41);
    drop(master);
    let info = png::Decoder::new(BufReader::new(std::fs::File::open(&saved.path).unwrap()))
        .read_info()
        .unwrap();
    assert_eq!((info.info().width, info.info().height), (width, height));
    assert_eq!(info.info().bit_depth, png::BitDepth::Sixteen);
    let cicp = info.info().coding_independent_code_points.unwrap();
    assert_eq!(
        (
            cicp.color_primaries,
            cicp.transfer_function,
            cicp.matrix_coefficients
        ),
        (9, 16, 0)
    );
    drop(info);
    drop(artifact);
    assert!(store.delete_publication(page.items[0].id.clone()).unwrap());
    assert!(!source.artifact.absolute_path.exists());
}

#[test]
#[ignore = "Release resource measurement, explicit process, no desktop capture"]
fn hdr_4k_archive_and_export_are_bounded() {
    archive_and_export(3840, 2160);
}

#[test]
#[ignore = "Release 45MP resource measurement, explicit process, no desktop capture"]
fn hdr_45mp_archive_and_export_are_bounded() {
    archive_and_export(8256, 5504);
}
