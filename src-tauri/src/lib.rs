//! SnapClip backend crate entry point.
//!
//! Responsibilities are split by layer, and the dependency direction is strictly
//! one way:
//!
//! ```text
//! commands/events (Tauri) -> application -> domain
//! platform adapters       -> application/domain contracts
//! infrastructure          -> domain
//! ```
//!
//! `lib.rs` only wires the layers together.

pub mod application;
pub mod capture;
pub mod domain;
pub mod infrastructure;

#[cfg(windows)]
mod icon;
mod ocr;
#[cfg(windows)]
mod platform;

mod app;
mod commands;
mod events;

pub use app::run;
