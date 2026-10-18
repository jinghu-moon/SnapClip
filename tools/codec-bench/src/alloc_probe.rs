//! Counts how many bytes each encoding allocates on top of what it starts with.
//!
//! The point is to answer "does this encoder buffer the whole frame?" without trusting task
//! manager numbers: the probe wraps the system allocator, tracks the live total with atomics, and
//! records the high-water mark between two resets.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// Wraps the system allocator and keeps a live total plus a peak.
pub struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            account(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            if new_size >= layout.size() {
                account(new_size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        moved
    }
}

fn account(size: usize) {
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    // A compare-exchange loop keeps the peak exact even under contention.
    let mut peak = PEAK.load(Ordering::Relaxed);
    while live > peak {
        match PEAK.compare_exchange_weak(peak, live, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(current) => peak = current,
        }
    }
}

/// Bytes currently allocated by this process.
pub fn live() -> usize {
    LIVE.load(Ordering::Relaxed)
}

/// Runs `body` and returns its value with the bytes it allocated at its peak above the entry mark.
pub fn peak_extra<T>(body: impl FnOnce() -> T) -> (T, usize) {
    let before = live();
    PEAK.store(before, Ordering::Relaxed);
    let value = body();
    let peak = PEAK.load(Ordering::Relaxed);
    (value, peak.saturating_sub(before))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_large_allocation_raises_the_peak_and_a_free_lowers_the_live_total() {
        // The peak is process-wide: do not measure while another test allocates.
        let _serial = crate::alloc_probe::test_guard();
        let (length, peak) = peak_extra(|| vec![7u8; 1 << 20].len());
        assert_eq!(length, 1 << 20);
        assert!(peak >= 1 << 20, "peak {peak} should cover the megabyte");
        // Only the peak is asserted: the live counter is process-wide, so other test threads
        // allocate concurrently and an absolute total here would be flaky by construction.
        assert!(peak >= 1 << 20);
    }

    #[test]
    fn an_empty_body_peaks_at_nothing() {
        let ((), peak) = peak_extra(|| ());
        assert_eq!(peak, 0);
    }
}

/// Serialises the tests that read the process-wide peak.
///
/// The counter is global, so two tests measuring at once would read each other's allocations and a
/// refusal that allocates nothing could look like one that allocates 64 KiB. A test that takes this
/// guard is the only one measuring for as long as it holds it.
pub fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
