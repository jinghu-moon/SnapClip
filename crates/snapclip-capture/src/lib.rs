//! Native screenshot capture: hotkey, overlay window, frame providers, window
//! detection and the selection state machine.
//!
//! # Independent by construction
//!
//! This crate is deliberately free of everything that is not capture: no clipboard,
//! no SQLite, no OCR, no Tauri, no GPUI. It produces exactly one domain result —
//! [`CaptureArtifact`] — and talks to the rest of the process through the traits in
//! [`ports`], which the composition root implements. `cargo tree -p snapclip-capture`
//! must never show `tauri`, `wry` or `gpui-kit`; T1.9 turns that into a gate.
//!
//! # Seam (what is public API)
//!
//! Public and stable (once the move lands): the error type (`CaptureError` /
//! `CaptureResult`), the domain values (`CaptureArtifact`, `CapturePayload`,
//! `CaptureState`, `PixelFormat`, plus the geometry value objects re-exported from
//! `snapclip-model`), the ports in `ports`, `CaptureRuntime`, and the artifact service
//! in `artifact`.
//!
//! **Not** public API: painter internals (render views, frame state, ring options),
//! the window-decision state machine's internals, and anything under `windows` other
//! than the concrete overlay the composition root constructs. Keep it that way: every
//! type that crosses the seam has to be justified, and `pub` fields are not allowed on
//! seam types (docs/22 §10.1).
//!
#![cfg(windows)]

pub mod annotation;
pub mod artifact;
pub mod diagnostics;
pub mod error;
pub mod geometry;
pub mod monitor_cache;
pub mod ports;
pub mod ring_contrast;
pub mod runtime;
pub mod sampler;
// The scroll-capture subsystem (`docs/30` §28). `P1.01` created the directory, so the module is
// a normal one from here on (`DEV-8`); its production items arrive with `P1.02`+. Nothing under
// `scroll/` may reference `windows` (§28.4).
pub mod scroll;
pub mod session;
pub mod window_detection;
pub mod windows;

pub use artifact::{
    CaptureService, PixelSliceSource, SelectionPixels,
};
pub use error::{CaptureError, CaptureResult};
pub use ports::{ArtifactWriter, CaptureEventSink, ClipboardWriter, OverlayPlatform};
pub use runtime::CaptureRuntime;

pub use snapclip_model::capture::{CaptureArtifact, CapturePayload, CaptureState, PixelFormat};
pub use snapclip_model::error::ErrorCode;
pub use snapclip_model::geometry::{ImageDimensions, Point, Rect};
