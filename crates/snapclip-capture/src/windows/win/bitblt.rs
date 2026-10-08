//! Compatibility provider: GDI `BitBlt` from the desktop DC.
//!
//! Used when Windows Graphics Capture is unavailable (older builds, remote
//! sessions, some virtual machines). It is a CPU → GPU path: the desktop is copied
//! into a DIB once per session and then uploaded. It is never used per mouse move.

use std::ffi::c_void;
use std::ptr::null_mut;

use ::windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BITSPIXEL, BitBlt, CAPTUREBLT, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDeviceCaps, HBITMAP, HDC,
    HGDIOBJ, PLANES, SRCCOPY, SelectObject,
};

use super::super::monitor::CapturedMonitor;
use crate::geometry::{Point, Rect};

/// Desktop pixels captured through GDI.
pub struct CapturedBitmap {
    pub width: u32,
    pub height: u32,
    /// Tightly packed BGRA8, top-down.
    pub pixels: Vec<u8>,
    /// Desktop bit depth as reported by the DC, for diagnostics.
    #[allow(dead_code)]
    pub bits_per_pixel: i32,
}

/// Copy the monitor rectangle out of the desktop DC.
pub fn capture_monitor(layout: &CapturedMonitor) -> Result<CapturedBitmap, String> {
    let origin = layout.origin();
    capture_rect(Rect::new(
        origin.x,
        origin.y,
        origin.x + layout.width() as i32,
        origin.y + layout.height() as i32,
    ))
}

/// Copy an arbitrary desktop rectangle out of the desktop DC.
///
/// The rectangle is in physical desktop pixels, so the caller is responsible
/// for per-monitor DPI awareness (`monitor::set_per_monitor_v2_awareness`).
/// A zero-sized rectangle is rejected rather than captured as an empty image.
pub fn capture_rect(rect: Rect) -> Result<CapturedBitmap, String> {
    if rect.width() == 0 || rect.height() == 0 {
        return Err("capture rectangle has zero size".into());
    }

    unsafe {
        let screen_dc = GetDC(None);
        if screen_dc.is_invalid() {
            return Err(super::win32_error("GetDC"));
        }
        let result = capture_with_dc(
            screen_dc,
            Point::new(rect.left, rect.top),
            rect.width() as u32,
            rect.height() as u32,
        );
        ::windows::Win32::Graphics::Gdi::ReleaseDC(None, screen_dc);
        result
    }
}

unsafe fn capture_with_dc(
    screen_dc: HDC,
    origin: Point,
    width: u32,
    height: u32,
) -> Result<CapturedBitmap, String> {
    let bits_per_pixel = unsafe { GetDeviceCaps(Some(screen_dc), BITSPIXEL) };
    let planes = unsafe { GetDeviceCaps(Some(screen_dc), PLANES) };

    let memory_dc = unsafe { CreateCompatibleDC(Some(screen_dc)) };
    if memory_dc.is_invalid() {
        return Err(super::win32_error("CreateCompatibleDC"));
    }

    let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
    info.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width as i32,
        // Negative height requests a top-down DIB, matching every other provider.
        biHeight: -(height as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        biSizeImage: width * height * 4,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };
    let mut bits: *mut c_void = null_mut();
    let bitmap: HBITMAP = unsafe {
        CreateDIBSection(Some(screen_dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
    }
    .map_err(|error| super::hresult("CreateDIBSection", &error))?;
    if bits.is_null() {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(memory_dc);
        }
        return Err("CreateDIBSection returned no pixel buffer".into());
    }

    let previous = unsafe { SelectObject(memory_dc, HGDIOBJ(bitmap.0)) };
    let copied = unsafe {
        BitBlt(
            memory_dc,
            0,
            0,
            width as i32,
            height as i32,
            Some(screen_dc),
            origin.x,
            origin.y,
            // CAPTUREBLT includes layered windows such as the mouse cursor
            // overlay; the cursor itself is excluded because the overlay draws it.
            SRCCOPY | CAPTUREBLT,
        )
    };
    let pixels = unsafe {
        std::slice::from_raw_parts(bits as *const u8, (width * height * 4) as usize).to_vec()
    };

    unsafe {
        if !previous.is_invalid() {
            SelectObject(memory_dc, previous);
        }
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(memory_dc);
    }

    copied.map_err(|error| super::hresult("BitBlt", &error))?;

    Ok(CapturedBitmap {
        width,
        height,
        pixels,
        bits_per_pixel: if planes > 0 { bits_per_pixel * planes } else { bits_per_pixel },
    })
}

#[cfg(test)]
mod tests {
    use super::capture_monitor;
    use crate::windows::monitor;

    #[test]
    fn bitblt_captures_a_real_monitor() {
        let _ = monitor::set_per_monitor_v2_awareness();
        let Ok(target) = monitor::captured_monitor_at_cursor() else {
            return;
        };
        match capture_monitor(&target) {
            Ok(captured) => {
                assert_eq!(captured.width, target.width());
                assert_eq!(captured.height, target.height());
                assert_eq!(
                    captured.pixels.len(),
                    (captured.width * captured.height * 4) as usize
                );
                // A real desktop is not uniformly transparent.
                assert!(captured.pixels.chunks_exact(4).any(|pixel| pixel[3] != 0));
            }
            // Locked or disconnected desktops may legitimately refuse BitBlt.
            Err(error) => eprintln!("BitBlt capture unavailable in this session: {error}"),
        }
    }
}






