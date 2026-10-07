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
//! **Status (T1.1)**: the crate exists so the workspace, the dependency direction and
//! the target gating are settled before any code moves. The modules below arrive with
//! T1.4 (platform-independent capture) and T1.5 (the Windows implementation).

#![cfg(windows)]
