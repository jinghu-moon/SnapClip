//! Thin wrappers over the Windows graphics APIs used by the capture overlay.
//!
//! Each submodule owns one API surface and nothing else:
//! * [`d3d11`] — device, composition swap chain, texture helpers, readback
//! * [`wgc`] — Windows Graphics Capture provider
//! * [`bitblt`] — GDI BitBlt fallback provider
//! * [`d2d`] — Direct2D/DirectWrite resources and the L0/L1/L2 draw calls
//! * [`window`] — top-level window enumeration, attributes and DWM frame bounds

pub mod bitblt;
pub mod d2d;
pub mod d3d11;
pub mod wgc;
pub mod window;

/// Wrap a Windows error into a message that keeps the HRESULT for diagnostics.
pub fn hresult(context: &str, error: &::windows::core::Error) -> String {
    let code = error.code().0;
    format!("{context} failed ({error}) #code={code}")
}

/// Work out a Win32 error message from the thread's last-error value.
///
/// Kept as a helper so provider code never has to remember which binding exposes
/// `GetLastError`.
pub fn win32_error(context: &str) -> String {
    let code = unsafe { ::windows::Win32::Foundation::GetLastError().0 };
    format!("{context} failed with Win32 error {code} #code={code}")
}
