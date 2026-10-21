//! Shared layout, theme and native painting primitives for SnapClip's Win32 UI.
//!
//! Domain state stays with its owning feature. This crate owns reusable visual behavior and the
//! geometry contracts that must agree between painting and hit testing.

pub mod panel;

pub mod copy;
