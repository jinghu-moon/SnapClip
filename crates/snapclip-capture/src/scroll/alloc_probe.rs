//! The capture test binary's one global allocator, and the four numbers it keeps.
//!
//! A binary can have exactly one `#[global_allocator]`, so every measurement device that wants
//! allocator numbers has to share this one. It started life inside `perf_probe` (`P0.03`, for
//! `E-PERF-1`); `P4.07` (`E-MEM-1`) needed a counter `perf_probe` did not have, and two
//! allocators were not an option, so the accounting moved here and both probes read it.
//!
//! # The four numbers, and why they are four
//!
//! `docs/30` §22.6 quotes the benchmark methodology this project borrowed from
//! `snow-crates/benchmark-support`:
//!
//! > **heap traffic cannot prove a drop in space (when the storage moved to an OS mapping)**
//!
//! * **`live`** — bytes currently held. The steady state: what the design is still paying for when
//!   the work is done.
//! * **`peak`** — the high-water mark of `live`. Process-global, and therefore only meaningful for
//!   a workload that is the only thing running in its process; that is why every probe here runs
//!   one scenario per process.
//! * **`allocated`** — cumulative bytes ever handed out. **Traffic, not space.** A streaming design
//!   has a large `allocated` and a small `peak`, and reporting either alone would let the other's
//!   failure hide: a design that materializes the whole image shows up in `peak`, and a design that
//!   copies a band per step shows up in `allocated`.
//! * **`allocations`** — how many times the allocator was called. The coarsest of the four, and the
//!   one that catches "the traffic is fine but it happens ten million times".
//!
//! `allocated` is a *lower* bound on traffic in the sense that only the allocator's own view is
//! counted: memory the OS maps outside `GlobalAlloc` — a file mapping, a D3D staging texture — is
//! invisible here, which is exactly the trap the quoted sentence names. That is why `E-MEM-1` also
//! records the spill file size, and why the two are reported as separate columns rather than
//! combined into one "footprint".

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

/// Bytes currently held by this process.
pub(crate) fn live_bytes() -> usize {
    LIVE_BYTES.load(Ordering::Relaxed)
}

/// Highest `live` value seen since the last [`reset_peak`].
pub(crate) fn peak_bytes() -> usize {
    PEAK_BYTES.load(Ordering::Relaxed)
}

/// Cumulative bytes handed out since the last [`reset_traffic`].
pub(crate) fn allocated_bytes() -> usize {
    ALLOCATED_BYTES.load(Ordering::Relaxed)
}

/// Allocator calls since the last [`reset_traffic`].
pub(crate) fn allocations() -> usize {
    ALLOCATIONS.load(Ordering::Relaxed)
}

/// Moves the high-water mark to the current live total, so a later `peak - live` measures what the
/// region under measurement itself added.
pub(crate) fn reset_peak() {
    PEAK_BYTES.store(live_bytes(), Ordering::Relaxed);
}

/// Restarts the traffic counters at zero.
///
/// Separate from [`reset_peak`] on purpose: a workload's *space* is interesting from the moment the
/// region starts, while its *traffic* is interesting from the moment the work starts, and the two
/// moments are not the same one (building the fixture is not the measurement).
pub(crate) fn reset_traffic() {
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    ALLOCATIONS.store(0, Ordering::Relaxed);
}

/// The four numbers read together, so a report cannot quote three of them from one instant and the
/// fourth from another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AllocSnapshot {
    pub(crate) live: usize,
    pub(crate) peak: usize,
    pub(crate) allocated: usize,
    pub(crate) allocations: usize,
}

pub(crate) fn snapshot() -> AllocSnapshot {
    AllocSnapshot {
        live: live_bytes(),
        peak: peak_bytes(),
        allocated: allocated_bytes(),
        allocations: allocations(),
    }
}

struct CountingAllocator;

impl CountingAllocator {
    fn record_growth(bytes: usize) {
        let live = LIVE_BYTES.fetch_add(bytes, Ordering::Relaxed) + bytes;
        PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(bytes, Ordering::Relaxed);
    }

    fn record_shrink(bytes: usize) {
        LIVE_BYTES.fetch_sub(bytes, Ordering::Relaxed);
    }

    fn record_call() {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    }
}

// SAFETY: every method forwards to `System` unchanged; the counters are the only added work and
// they use relaxed atomics, which cannot allocate.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::record_call();
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            Self::record_growth(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Self::record_call();
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            Self::record_growth(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        Self::record_shrink(layout.size());
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        Self::record_call();
        let new_pointer = unsafe { System.realloc(pointer, layout, new_size) };
        if !new_pointer.is_null() {
            if new_size >= layout.size() {
                Self::record_growth(new_size - layout.size());
            } else {
                Self::record_shrink(layout.size() - new_size);
            }
        }
        new_pointer
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
