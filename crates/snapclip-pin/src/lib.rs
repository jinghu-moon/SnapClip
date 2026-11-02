//! Windows pin windows: display supplied pixels above other windows without taking focus.
//!
//! [`PinManager`] owns one host thread and all of its windows. Clones share that lifetime;
//! dropping the last manager closes the windows, unregisters F6, and joins the thread.
//! [`PinPoster`] is a weak posting handle and cannot keep the host alive.
//!
//! Capture and clipboard sources belong to the application. F6 consumes a selection through
//! [`PinHome`]; prepared images can be submitted through [`PinManager::create_pin`]. Both paths
//! allocate handles from the same counter and report their results as [`PinEvent`].
//!
//! This crate does not capture screens, read the clipboard, encode images, or depend on GPUI.
//! See `docs/51-pin-crate.md` in the workspace for wiring and validation.
//!
//! ```no_run
//! use std::sync::Arc;
//! use snapclip_pin::{ImageSource, PinHome, PinManager, PinRequest, PinSelection};
//!
//! struct Source;
//! impl PinHome for Source {
//!     fn take_selection(&self) -> Option<PinSelection> { None }
//! }
//! # fn main() -> anyhow::Result<()> {
//! let pins = PinManager::start(Arc::new(Source))?;
//! let image = ImageSource::from_bgra(vec![0xFF4488AA; 32 * 32], 32, 32)?;
//! let handle = pins.create_pin(PinRequest::new(image, (100, 100)).radius(4))?;
//! // The application polls pins.try_recv_event() or pins.drain_events() in its event loop.
//! pins.close_pin(handle)?;
//! # Ok(())
//! # }
//! ```

#![cfg(windows)]

mod host;
mod source;

use std::sync::Arc;

use snapclip_model::geometry::Rect;

pub use host::{PinEvent, PinHandle, PinManager, PinPoster, PinReport, PinRequest};
pub use source::ImageSource;

/// The most recently confirmed screenshot, in virtual-desktop physical pixels.
///
/// The application supplies top-down BGRA bytes and the corner radius captured with the image.
/// The host validates dimensions and byte length before creating a window.
#[derive(Debug, Clone)]
pub struct PinSelection {
    region: Rect,
    pixels: Arc<Vec<u8>>,
    radius: i32,
}

impl PinSelection {
    /// Create a selection with square corners. Validation happens when it is consumed.
    pub fn new(region: Rect, pixels: Arc<Vec<u8>>) -> Self {
        Self {
            region,
            pixels,
            radius: 0,
        }
    }

    /// Set the captured corner radius in source-image physical pixels; negative values become 0.
    pub fn radius(mut self, radius: i32) -> Self {
        self.radius = radius.max(0);
        self
    }

    /// Position and bounds in virtual-desktop physical pixels (negative origins are valid).
    pub fn region(&self) -> Rect {
        self.region
    }

    /// Top-down BGRA bytes. The host forces screenshot alpha to opaque.
    pub fn bytes(&self) -> &[u8] {
        &self.pixels
    }

    /// Captured corner radius in source-image physical pixels.
    pub fn corner_radius(&self) -> i32 {
        self.radius
    }
}

/// Supplies the most recently confirmed screenshot when F6 or [`PinPoster::post`] is handled.
///
/// Called on the pin host thread. Implementations must return promptly and must not require a
/// synchronous callback to the UI thread. Return `None` when there is no image; this allocates
/// no handle. Taking a selection should consume it so repeated F6 cannot duplicate it.
pub trait PinHome: Send + Sync + 'static {
    fn take_selection(&self) -> Option<PinSelection>;
}
