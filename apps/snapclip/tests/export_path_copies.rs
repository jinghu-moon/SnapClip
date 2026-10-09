//! `P4.03`'s L4 half: the export path holds **one band**, not the image.
//!
//! `docs/30 §22.1` counts four full-image copies in the export chain and `§22.4` concludes that all
//! four must go. This file is the measurement that decides whether they did:
//!
//! | # | 拷贝 | 位置 |
//! |---|---|---|
//! | 1 | `prepared.bgra.clone()` | `apps/snapclip/src/capture/artifact_writer.rs:44` |
//! | 2 | `bgra_to_rgba(...).to_vec()` | `crates/snapclip-history/src/image.rs:153` |
//! | 3 | `RgbaImage::from_raw(rgba.to_vec())` | `crates/snapclip-history/src/image.rs:66` |
//! | 4 | 编码器内部 | `image::DynamicImage::write_to` |
//!
//! Two claims, and they are different claims: `the_export_path_holds_at_most_one_row_band_at_a_time`
//! measures the **peak**, and `the_export_path_allocates_no_second_full_copy` counts the **number of
//! allocations at least as large as the image** — which is the mechanical form of `§30.7`'s
//! "导出拷贝数 ≤ 2 份（今天 4 份）" row. A peak bound alone could be satisfied by one big copy that
//! is released early; the count alone could be satisfied by many small ones. `§22.4` claims zero,
//! so the test asserts zero.
//!
//! Why a separate binary with its own `#[global_allocator]`: same reason as
//! `tests/png_row_band_memory.rs`, and the same shape as `tests/png_params_probe.rs:35-107`. The
//! allocator is repeated on purpose — each probe binary has to be readable on its own, and a shared
//! helper would put the measurement machinery in a third file that neither claim mentions.
//!
//! # How to run
//!
//! ```text
//! cargo test --release -p snapclip-app --test export_path_copies -- --ignored --nocapture
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use snapclip_app::capture::HistoryArtifactWriter;
use snapclip_capture::artifact::SelectionPixels;
use snapclip_capture::geometry::Rect;
use snapclip_capture::ports::ArtifactWriter;
use snapclip_capture::session::CapturedFrame;
use snapclip_model::PixelFormat;

// ---------------------------------------------------------------------------
// The allocator: a live counter, its high-water mark, and a counter for
// allocations that are at least one whole copy of the image.
// ---------------------------------------------------------------------------

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// An allocation of at least this many bytes is "a full copy of the image".
static FULL_COPY_THRESHOLD: AtomicUsize = AtomicUsize::new(usize::MAX);
static FULL_COPIES: AtomicUsize = AtomicUsize::new(0);

pub struct CountingAllocator;

impl CountingAllocator {
    fn added(size: usize) {
        let live = LIVE.fetch_add(size, Relaxed) + size;
        let _ = PEAK.fetch_max(live, Relaxed);
        if size >= FULL_COPY_THRESHOLD.load(Relaxed) {
            FULL_COPIES.fetch_add(1, Relaxed);
        }
    }

    fn removed(size: usize) {
        LIVE.fetch_sub(size, Relaxed);
    }

    pub fn live() -> usize {
        LIVE.load(Relaxed)
    }

    pub fn peak() -> usize {
        PEAK.load(Relaxed)
    }

    /// Starts a measurement: the peak restarts here and full-copy counting is armed.
    pub fn arm(full_copy_bytes: usize) {
        PEAK.store(LIVE.load(Relaxed), Relaxed);
        FULL_COPIES.store(0, Relaxed);
        FULL_COPY_THRESHOLD.store(full_copy_bytes, Relaxed);
    }

    /// Ends a measurement and returns how many full copies were allocated since `arm`.
    pub fn disarm() -> usize {
        FULL_COPY_THRESHOLD.store(usize::MAX, Relaxed);
        FULL_COPIES.load(Relaxed)
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            CountingAllocator::added(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            CountingAllocator::added(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        CountingAllocator::removed(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            CountingAllocator::removed(layout.size());
            CountingAllocator::added(new_size);
        }
        new
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

// ---------------------------------------------------------------------------
// The fixture.
// ---------------------------------------------------------------------------

/// A page-like selection: a light background with dark "text" runs.
///
/// This allocation is the **caller's** and it exists before the measurement — it is the one copy
/// `§22.4` calls unavoidable (the GPU→CPU read), and counting it against the export path would make
/// the test measure its own fixture.
fn selection(width: u32, height: u32) -> SelectionPixels {
    let mut bgra = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let ink = (x / 7) % 5 == 0 && (y % 13) < 9;
            let value = if ink { 32 } else { 244 };
            bgra.extend_from_slice(&[value, value, value, 255]);
        }
    }
    SelectionPixels {
        frame: CapturedFrame {
            width,
            height,
            pixel_format: PixelFormat::Bgra8Unorm,
            captured_at_unix_ms: 1_700_000_000_000,
            provider: "p4-03",
        },
        region: Rect::new(0, 0, width as i32, height as i32),
        bgra,
    }
}

fn root(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("snapclip-p403-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    path
}

/// The artifact's size on disk. `CaptureArtifact` carries the path, not the bytes (§5's split), so
/// the size is read back rather than reported by the writer.
fn artifact_len(artifact: &snapclip_model::CaptureArtifact) -> u64 {
    let path = artifact.png_path().expect("a PNG payload");
    std::fs::metadata(path).expect("the artifact exists").len()
}

/// `P4.03` RED: today this measures three full copies and a peak of about three times the image.
#[test]
#[ignore = "L4: Release, one process, a real 20,000 px export"]
fn the_export_path_holds_at_most_one_row_band_at_a_time() {
    let (width, height) = (1280u32, 20_000u32);
    let prepared = selection(width, height);
    let raw_len = prepared.bgra.len();
    let directory = root("peak");
    let writer = HistoryArtifactWriter::new(directory.clone());

    let before = CountingAllocator::live();
    CountingAllocator::arm(raw_len);
    let artifact = writer
        .write("capture-p403-peak", &prepared, 96, None)
        .expect("the export must succeed");
    let extra = CountingAllocator::peak() - before;
    let full_copies = CountingAllocator::disarm();
    let artifact_len = artifact_len(&artifact);

    println!(
        "[P4.03] {width}x{height} raw {raw_len} B ({} MiB), artifact {artifact_len} B ({} KiB), \
         export peak-live {extra} B ({} KiB), full-copy allocations {full_copies}",
        raw_len / (1024 * 1024),
        artifact_len / 1024,
        extra / 1024,
    );

    assert_eq!((artifact.width, artifact.height), (width, height));
    // The claim: what the export path holds is the compressed artifact plus the encoder's own
    // row-sized buffers, so it is *strictly smaller* than one more copy of the image. A `Vec` doubles
    // as it grows, so the artifact is transiently present twice; that is why the artifact is not
    // subtracted and the bound is `raw_len` rather than `artifact + slack`.
    assert!(
        extra < raw_len,
        "the export path held {extra} B over live for a {raw_len} B image: it is materializing",
    );
    let _ = std::fs::remove_dir_all(directory);
}

/// `P4.03`'s copy count: the mechanical form of `§30.7`'s "导出拷贝数 ≤ 2 份".
#[test]
#[ignore = "L4: Release, one process, a real 20,000 px export"]
fn the_export_path_allocates_no_second_full_copy() {
    let (width, height) = (1280u32, 20_000u32);
    let prepared = selection(width, height);
    let raw_len = prepared.bgra.len();
    let directory = root("copies");
    let writer = HistoryArtifactWriter::new(directory.clone());

    CountingAllocator::arm(raw_len);
    let artifact = writer
        .write("capture-p403-copies", &prepared, 96, None)
        .expect("the export must succeed");
    let full_copies = CountingAllocator::disarm();

    println!(
        "[P4.03] {width}x{height}: {full_copies} allocation(s) of at least {raw_len} B during the \
         export (artifact {} B)",
        artifact_len(&artifact),
    );

    // `§22.4`'s conclusion is not "fewer copies", it is that the four export copies are **all**
    // eliminated: the encoder is fed rows, so nothing ever needs a second whole image. `§30.7`'s
    // "≤ 2 份" is the design's ceiling under uncertainty; the measured answer is 0, and asserting 0
    // is what makes a regression visible.
    assert_eq!(
        full_copies, 0,
        "the export path allocated {full_copies} buffer(s) at least as large as the image",
    );
    let _ = std::fs::remove_dir_all(directory);
}
