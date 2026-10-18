//! Sweep the tile files a crashed scroll session left behind (docs/36 §12.2).
//!
//! The policy is pure and lives in `scroll::backed`; only the liveness probe needs the platform,
//! which is why this lives here and not next to the store.

use std::time::SystemTime;

use crate::scroll::backed::{tile_is_orphan, tile_owner};

/// Remove `.tile` files that a previous run left behind.
///
/// Called when a scroll session starts, before its store exists: a file that already carries this
/// pid is therefore provably an orphan (no store of ours exists yet, so an earlier process had the
/// same pid). Files of other pids go only when that process is gone **and** the file is an hour old.
pub(crate) fn sweep() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
            return;
        };
        let me = std::process::id();
        let now = SystemTime::now();
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(pid) = tile_owner(name) else { continue };
            let age = meta
                .modified()
                .ok()
                .and_then(|then| now.duration_since(then).ok())
                .map_or(u64::MAX, |age| age.as_secs());
            if tile_is_orphan(pid, me, age, is_running) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    });
}

/// Whether another process still exists. Access denied counts as alive: a handle we may not open
/// must never be a reason to delete its working file.
fn is_running(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_INVALID_PARAMETER};
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
    // SAFETY: opening a synchronisation handle for a liveness query; closed immediately.
    match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
        Ok(handle) => {
            // SAFETY: the handle came from OpenProcess and is closed exactly once.
            unsafe {
                let _ = CloseHandle(handle);
            }
            true
        }
        Err(_) => {
            // SAFETY: no preconditions; reports the error of the failed call above.
            unsafe { GetLastError() != ERROR_INVALID_PARAMETER }
        }
    }
}
