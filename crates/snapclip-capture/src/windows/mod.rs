//! Win32/D3D11 capture platform adapter.
//!
//! Layout follows the responsibility split in `docs/08-screenshot-mvp-tasklist.md`
//! §2.4, extended by the scroll work (`docs/30`):
//! * [`hotkey`] — `RegisterHotKey` / `WM_HOTKEY`
//! * [`monitor`] — monitor enumeration and physical/DPI geometry
//! * [`overlay`] — overlay HWND, message loop, input and session lifecycle
//! * [`renderer`] — the D3D11 swap chain and the Direct2D target the overlay presents
//! * [`providers`] — DXGI/WGC/BitBlt frame acquisition and region readback
//! * [`capture_worker`] — persistent capture thread with a capacity-1 mailbox
//! * [`detection_worker`] / [`refinement_worker`] — window and sub-element detection
//!   off the message thread
//! * [`export_worker`] — persistent export thread: encode and atomic write off the UI
//! * [`top_level_provider`], [`uia_provider`], [`msaa_provider`] — the three target
//!   sources behind the detection workers
//! * [`scroll_source`] / [`scroll_actuator`] — window-level frame stream and the wheel
//!   injection the scroll loop drives
//! * [`win`] — thin GPU/WinRT wrappers (D3D11, DXGI, WGC, D2D)
//!
//! The adapter produces pixels through [`crate::artifact::CaptureService`] and hands
//! them to the composition root; it never links against the clipboard module, the
//! store, OCR or a UI framework.

pub mod capture_worker;
pub mod detection_worker;
pub mod export_worker;
pub mod hotkey;
pub mod monitor;
pub mod msaa_provider;
pub mod overlay;
pub mod providers;
pub mod refinement_worker;
pub mod renderer;
pub mod scroll_actuator;
/// Measure-only probe for the injection decision (`docs/30` §24.6, `E-INJECT-1`).
/// It exists to answer "can we actually drive this window's scroll position",
/// never to ship: it is test-only and nothing in the capture path calls it.
#[cfg(test)]
pub mod scroll_probe;
pub mod scroll_source;
pub mod timed_call;
pub mod top_level_provider;
pub mod uia_provider;
pub mod win;
