//! Native screenshot capture: hotkey, overlay window, capture providers and the
//! selection state machine.
//!
//! This module is intentionally independent from the clipboard, the store, OCR and
//! Tauri. It produces one domain result — [`CaptureArtifact`] — and nothing else.
//!
//! Consumers outside the feature reach it through the narrow surface below:
//! `capture::application` (the overlay contract and runtime),
//! `capture::geometry` / `capture::session` (pure types used by the artifact
//! service), and the error type on the IPC boundary.

mod error;
pub mod annotation;
pub mod diagnostics;
pub mod geometry;
pub mod monitor_cache;
pub mod ring_contrast;
pub mod sampler;
pub mod session;
pub mod window_detection;

pub mod application;

pub use error::{CaptureError, CaptureResult};

pub use crate::domain::{CaptureArtifact, CapturePayload, CaptureState, PixelFormat};
