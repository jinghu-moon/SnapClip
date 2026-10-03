//! Windows capture platform adapter.
//!
//! Re-exports the concrete overlay controller so the composition root can build it
//! without naming `platform::windows` directly.

pub use crate::platform::windows::capture::overlay::WindowsOverlay;