//! Prepared-content pins: Yue-backed GPUI viewers and an original-position native F6 host.
#![cfg(windows)]
mod feedback;
mod host;
mod image_preview;
mod manager;
mod platform;
mod source;
mod table;
mod view;
pub use host::{PinEvent, PinHandle, PinReport, PinRequest};
pub use manager::{CopyTarget, PinCommand, PinManager, PinPlacement};
use snapclip_model::geometry::Rect;
pub use snapclip_model::pin::*;
pub use source::ImageSource;
use std::sync::Arc;
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

/// Bind pin-window commands once after GPUI/Yue initialization.
pub fn init(cx: &mut gpui_kit::App) {
    view::init(cx);
}

/// A transient reading result near the command's origin; never enters the live pin registry.
pub fn show_feedback(message: impl Into<String>, position: (i32, i32), cx: &mut gpui_kit::App) {
    feedback::show(
        message.into(),
        position,
        None,
        std::time::Duration::from_secs(3),
        cx,
    );
}
