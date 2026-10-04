//! Win32/D3D11 capture platform adapter.
//!
//! Layout follows the responsibility split in `docs/08-screenshot-mvp-tasklist.md`
//! §2.4:
//! * [`hotkey`] — `RegisterHotKey` / `WM_HOTKEY`
//! * [`monitor`] — monitor enumeration and physical/DPI geometry
//! * [`renderer`] — overlay HWND, message loop, input and session lifecycle
//! * [`providers`] — DXGI/WGC/BitBlt frame acquisition and region readback
//! * [`capture_worker`] — persistent capture thread with a capacity-1 mailbox
//! * [`export_worker`] — persistent export thread: encode and atomic write off the UI
//! * [`win`] — thin GPU/WinRT wrappers (D3D11, DXGI, WGC, D2D)
//!
//! The adapter produces artifacts through [`crate::application::capture_service`];
//! it never links against the clipboard module, the store, OCR or Tauri.

pub mod capture_worker;
pub mod export_worker;
pub mod hotkey;
pub mod monitor;
pub mod overlay;
pub mod providers;
pub mod renderer;
pub mod win;
