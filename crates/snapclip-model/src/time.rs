//! Wall-clock helpers shared by the capability crates.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch.
///
/// One definition, because this was previously copy-pasted into several modules and the
/// capture crate could only reach it by depending on the clipboard module — exactly the
/// kind of accidental edge the seam is supposed to prevent.
///
/// Clamped to `0` when the system clock sits before the epoch, so a misconfigured
/// machine produces a wrong timestamp instead of a panic.
pub fn unix_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as i64
}
