//! `P4.02`'s L4 half: the export port streams, and the export path holds no image.
//!
//! `docs/31` P4.02 asks for two things and puts them on two layers. The L2 half — the artifact
//! decodes back to the pixels that went in — is `apps/snapclip/src/capture/row_band_png.rs`'s own
//! test module and runs in the ordinary gate. This file is the L4 half, and L4 means what §2.2 says
//! it means: **Release, a real 30,000 px encode, one process**.
//!
//! Why a separate binary rather than a `#[cfg(test)]` module in the library: the measurement needs
//! a `#[global_allocator]`, and a global allocator installed in the library's test binary would
//! instrument every other unit test in the crate to no purpose. The idiom is `P0.04`'s
//! (`tests/png_params_probe.rs:35-107`) and the two files are deliberately the same shape.
//!
//! # How to run
//!
//! ```text
//! cargo test --release -p snapclip-app --test png_row_band_memory -- --ignored --nocapture
//! ```
//!
//! `--release` is not a preference: a debug build of the encoder is roughly an order of magnitude
//! slower and the numbers next to `E-PERF-2`'s would not be comparable.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use snapclip_app::capture::row_band_png::PngRowBandSink;
use snapclip_capture::scroll::Axis;
use snapclip_capture::scroll::export::{ImageMeta, RowBandSink};

// ---------------------------------------------------------------------------
// The allocator. `peak - live` is what the export path added on top of the rows it was handed.
// ---------------------------------------------------------------------------

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// Wraps the system allocator and keeps a running live counter plus its high-water mark.
pub struct CountingAllocator;

impl CountingAllocator {
    fn added(size: usize) {
        let live = LIVE.fetch_add(size, Relaxed) + size;
        let _ = PEAK.fetch_max(live, Relaxed);
    }

    fn removed(size: usize) {
        LIVE.fetch_sub(size, Relaxed);
    }

    /// Bytes currently allocated by this test binary.
    pub fn live() -> usize {
        LIVE.load(Relaxed)
    }

    /// Highest live value seen since the last [`CountingAllocator::reset_peak`].
    pub fn peak() -> usize {
        PEAK.load(Relaxed)
    }

    /// Restart the high-water mark at the current live value.
    pub fn reset_peak() {
        PEAK.store(LIVE.load(Relaxed), Relaxed);
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
// The measurement.
// ---------------------------------------------------------------------------

const WIDTH: u32 = 1280;
/// `P0.04`'s reference length (`docs/30` §17.7.1), so the two sets of numbers line up.
const HEIGHT: u32 = 30_000;
/// One band, in rows. Small enough that a materializing implementation would have to allocate the
/// image and large enough that the per-band overhead does not dominate.
const BAND_ROWS: u32 = 256;

/// Fills one band of a page-like image: a light background with dark "text" runs, generated **in
/// place**.
///
/// Generating it is not a convenience. A test that built the image first and then measured would be
/// measuring its own fixture: the whole point is that neither the caller nor the encoder ever holds
/// more than a band.
fn fill_band(buf: &mut [u8], first_row: u64) {
    let width = WIDTH as usize;
    for (index, pixel) in buf.chunks_exact_mut(4).enumerate() {
        let x = index % width;
        let y = first_row as usize + index / width;
        let ink = (x / 7) % 5 == 0 && (y % 13) < 9;
        let value = if ink { 32 } else { 244 };
        pixel.copy_from_slice(&[value, value, value, 255]);
    }
}

#[test]
#[ignore = "L4: Release, one process, a real 30,000 px encode"]
fn the_sink_streams_without_materializing_the_image() {
    let row_bytes = WIDTH as usize * 4;
    let plan = ImageMeta {
        width: u64::from(WIDTH),
        height: u64::from(HEIGHT),
        length: u64::from(HEIGHT),
        axis: Axis::Vertical,
        dpr: 1,
    };

    // Both of these exist before the measurement starts, exactly as a session's would: what is
    // measured is what the *export path* adds on top of the rows it is handed.
    let mut band = vec![0u8; BAND_ROWS as usize * row_bytes];
    let mut sink = PngRowBandSink::new();

    let before = CountingAllocator::live();
    CountingAllocator::reset_peak();

    let mut writer = sink.begin(&plan).expect("begin");
    let mut row = 0u64;
    while row < u64::from(HEIGHT) {
        let rows = BAND_ROWS.min(HEIGHT - row as u32);
        let band = &mut band[..rows as usize * row_bytes];
        fill_band(band, row);
        writer.write_rows(row, band).expect("write_rows");
        row += u64::from(rows);
    }
    let artifact = writer.finish(None).expect("finish");
    let extra = CountingAllocator::peak() - before;

    let raw_len = WIDTH as usize * HEIGHT as usize * 4;
    println!(
        "[P4.02] {}x{} raw {} B ({} MiB), artifact {} B ({} KiB), export path peak-live {} B ({} KiB)",
        WIDTH,
        HEIGHT,
        raw_len,
        raw_len / (1024 * 1024),
        artifact.bytes.len(),
        artifact.bytes.len() / 1024,
        extra,
        extra / 1024,
    );

    assert_eq!(artifact.rows, u64::from(HEIGHT), "every row was written");
    assert!(!artifact.bytes.is_empty(), "the encoder produced no bytes");

    // The one thing this sink relies on that `P4.02`'s prescribed borrowing form could not deliver:
    // IEND is written as the `png` `Writer` drops (`png-0.18.1/src/encoder.rs:1115-1119`). Asserting
    // the trailer says *why* a truncated artifact would be wrong, where "it decodes" would only say
    // *that* it was. The decode itself is the L2 test's job.
    assert_eq!(
        &artifact.bytes[..8],
        &b"\x89PNG\r\n\x1a\n"[..],
        "PNG signature"
    );
    assert_eq!(
        &artifact.bytes[artifact.bytes.len() - 12..],
        &b"\x00\x00\x00\x00IEND\xae\x42\x60\x82"[..],
        "the file must end with IEND, not with the last IDAT chunk"
    );

    // The claim. Two parts, because one alone would be satisfiable by a lie:
    //
    // 1. What the export path holds is the artifact plus the encoder's own buffers. A `Vec` doubles
    //    as it grows, so during the last reallocation the artifact is transiently present twice;
    //    that is the `2 *`, and writing it down is what keeps the bound honest rather than loose.
    //    The encoder's own share is three rows (`StreamWriter`'s `prev_buf`/`curr_buf`/
    //    `filtered_buf`) plus a 4 KiB chunk buffer.
    // 2. The artifact is far smaller than the image it came from — for a page-like image, and that
    //    is the assumption the `raw_len / 4` margin encodes. A materializing implementation would
    //    have to hold `raw_len` and could not pass either assertion.
    let ceiling = 2 * artifact.bytes.len() + 16 * row_bytes + 256 * 1024;
    assert!(
        extra <= ceiling,
        "the export path held {extra} B over live, which is more than the {} B ceiling \
         (artifact {} B, row {row_bytes} B): something is materializing",
        ceiling,
        artifact.bytes.len(),
    );
    assert!(
        artifact.bytes.len() < raw_len / 4,
        "the artifact is {} B against a raw image of {raw_len} B: the fixture is not page-like \
         enough for the memory claim to mean anything",
        artifact.bytes.len(),
    );
}
