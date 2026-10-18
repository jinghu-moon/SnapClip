//! The process's own working set, for measurement only.
//!
//! An external sampler sees one number for the whole process and cannot say which part grew. The
//! scroll capture shares that process with the overlay, the compositor and the capture session, so
//! a rise seen from outside has to be correlated with counters taken inside, in the same frame
//! (docs/39 §3). The FFI is declared here rather than by widening `windows-sys`'s feature set: this
//! file exists in measurement builds only, and a dependency feature is crate-wide.

use core::ffi::c_void;

#[repr(C)]
struct ProcessMemoryCounters {
    cb: u32,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn K32GetProcessMemoryInfo(
        process: *mut c_void,
        counters: *mut ProcessMemoryCounters,
        size: u32,
    ) -> i32;
}

/// This process's working set in bytes, or `None` if the query failed.
///
/// `-1` is the pseudo-handle for the current process, which is why nothing is opened or closed
/// here.
pub(crate) fn working_set_bytes() -> Option<usize> {
    let mut counters = ProcessMemoryCounters {
        cb: size_of::<ProcessMemoryCounters>() as u32,
        page_fault_count: 0,
        peak_working_set_size: 0,
        working_set_size: 0,
        quota_peak_paged_pool_usage: 0,
        quota_paged_pool_usage: 0,
        quota_peak_non_paged_pool_usage: 0,
        quota_non_paged_pool_usage: 0,
        pagefile_usage: 0,
        peak_pagefile_usage: 0,
    };
    // SAFETY: the pseudo-handle needs no cleanup, and the structure is `repr(C)` with the size and
    // layout the API requires, passed by pointer for the duration of the call.
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            (-1i64) as *mut c_void,
            &mut counters,
            size_of::<ProcessMemoryCounters>() as u32,
        )
    };
    (ok != 0).then_some(counters.working_set_size)
}

/// The working set formatted for a log line, in mebibytes.
pub(crate) fn working_set_mb() -> String {
    match working_set_bytes() {
        Some(bytes) => format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0)),
        None => "unavailable".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wrong struct layout would still return a number, so the shape is checked by asking for a
    /// plausible one: a running test process has megabytes of working set, not zero and not a
    /// number larger than any real machine's memory.
    #[test]
    fn the_query_returns_this_process_working_set() {
        let bytes = working_set_bytes().expect("the query must succeed for this process");
        assert!(bytes > 1024 * 1024, "working set {bytes} is implausibly small");
        assert!(
            bytes < 1024usize * 1024 * 1024 * 1024,
            "working set {bytes} is implausibly large"
        );
    }
}
