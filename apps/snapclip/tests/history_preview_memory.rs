//! `P6.03`'s L4 half: the row preview decodes **a window**, not the image.
//!
//! `docs/30 §19.2` and `ADR-5` say a long capture's preview comes from a window of the image.
//! `apps/snapclip/src/history/view.rs::load_thumbnail` used to hand the payload straight to the
//! renderer, which decoded all of it: a 1058 x 502649 scroll capture is 2028 MiB of RGBA, decoded
//! to fill a 32 px box. `history/preview.rs` replaced that with a streaming window, and this file
//! is the measurement that decides whether it did:
//!
//! * `the_row_preview_never_decodes_the_whole_image` bounds the **peak** live bytes, and
//! * counts the **number of allocations at least as large as the decoded image** — which is the
//!   mechanical form of "the preview is not the artifact". A peak bound alone can be satisfied by
//!   one big allocation that is released early; the count alone can be satisfied by many small
//!   ones. `§19.2` claims the window is all there is, so the test asserts zero copies.
//!
//! The two claims are the same two claims `tests/export_path_copies.rs` makes about the export
//! path, and for the same reason: `P4.03` and `P6.03` are the two places where a whole image used
//! to be materialised, and both are now measured rather than asserted.
//!
//! Why a separate binary with its own `#[global_allocator]`: a binary can only have one, and a
//! peak number is only meaningful if the allocator that produced it is the one in the file that
//! quotes it. The allocator is repeated on purpose (the same choice `export_path_copies.rs`
//! made).
//!
//! # How to run
//!
//! ```text
//! cargo test --release -p snapclip-app --test history_preview_memory -- --ignored --nocapture
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use snapclip_app::history::preview::{PREVIEW_MAX_PX, row_preview_png};

// ---------------------------------------------------------------------------
// The allocator: a live counter, its high-water mark, and a counter for
// allocations that are at least one whole decoded copy of the image.
// ---------------------------------------------------------------------------

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
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

/// A tall page: dark text runs on a light background, 256 x 20,000.
///
/// The decoded image is 20,480,000 B, so "a full copy" is unambiguous. The *source* PNG is built
/// before the measurement and stays alive across it — that is the payload the store returned, and
/// a preview that needs it must be allowed to have it.
const WIDTH: u32 = 256;
const HEIGHT: u32 = 20_000;
const DECODED_BYTES: usize = WIDTH as usize * HEIGHT as usize * 4;

/// The bound on the peak. One output window is `128 * 128 * 4` = 65,536 B; the decoder holds a few
/// source rows of 1 KiB and the encoder a few output rows. 4 MiB is 64x the window and a fifth of
/// one decoded copy: it is a bound on the *shape* of the work (a window, not an image), not a
/// benchmark, and it would fail by 5x if the window ever became the whole image.
const PEAK_BUDGET: usize = 4 * 1024 * 1024;

fn tall_page_png() -> Vec<u8> {
    // The raw rows are dropped before this returns, so the measurement starts from the payload
    // alone.
    let mut rows = vec![0u8; DECODED_BYTES];
    for y in 0..HEIGHT as usize {
        for x in 0..WIDTH as usize {
            let value = if (x / 3 + y / 500) % 2 == 0 { 0x20 } else { 0xF0 };
            let base = (y * WIDTH as usize + x) * 4;
            rows[base] = value;
            rows[base + 1] = value;
            rows[base + 2] = value;
            rows[base + 3] = 0xFF;
        }
    }
    let mut encoded = Vec::new();
    let mut encoder = png::Encoder::new(&mut encoded, WIDTH, HEIGHT);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("a png header")
        .write_image_data(&rows)
        .expect("the rows");
    encoded
}

#[test]
#[ignore = "a measurement: run with --release and --ignored"]
fn the_row_preview_never_decodes_the_whole_image() {
    let source = tall_page_png();

    CountingAllocator::arm(DECODED_BYTES);
    let preview = row_preview_png(&source).expect("a preview");
    let peak = CountingAllocator::peak();
    let full_copies = CountingAllocator::disarm();
    let live_after = CountingAllocator::live();

    println!(
        "[P6.03] {WIDTH}x{HEIGHT} source {} B, preview {} B, peak-live {} B ({} KiB), \
         full-image allocations {full_copies} (threshold {DECODED_BYTES} B), \
         live after {live_after} B, budget {PEAK_BUDGET} B, target {PREVIEW_MAX_PX} px",
        source.len(),
        preview.len(),
        peak,
        peak / 1024,
    );

    assert_eq!(
        full_copies, 0,
        "the row preview allocated {full_copies} buffer(s) at least as large as the {} B image: \
         it is materialising it",
        DECODED_BYTES
    );
    assert!(
        peak <= PEAK_BUDGET,
        "the row preview held {peak} B over live for a {} B image: the window is {PREVIEW_MAX_PX} \
         px, so this is the whole image being decoded",
        DECODED_BYTES
    );
}
