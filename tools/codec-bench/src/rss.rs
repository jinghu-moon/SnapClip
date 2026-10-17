//! Process working set, so a measurement can say what the process really held.
//!
//! The counting allocator in [`crate::alloc_probe`] answers "how much did the code ask for"; this
//! answers "how much did the process hold", which also covers allocator retention and anything
//! outside Rust's allocator. Both are reported: they disagree when a freed buffer is not returned
//! to the OS, and that disagreement is itself a finding (docs/35 §7.1).

/// Working set now and the peak since the process started, in bytes.
#[cfg(windows)]
pub fn working_set() -> Option<(usize, usize)> {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY: the handle is the current process pseudo handle and the structure is sized by `cb`.
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    };
    (ok != 0).then(|| {
        (
            counters.WorkingSetSize as usize,
            counters.PeakWorkingSetSize as usize,
        )
    })
}

#[cfg(not(windows))]
pub fn working_set() -> Option<(usize, usize)> {
    None
}
