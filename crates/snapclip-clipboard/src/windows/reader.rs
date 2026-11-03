//! Copy system data while holding one bounded clipboard lock. No decoding or filesystem I/O.
use std::{mem::zeroed, ptr::null_mut, thread, time::Duration};

use windows_sys::Win32::{
    Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, GetDC, GetDIBits, GetObjectW,
        ReleaseDC,
    },
    System::{
        DataExchange::{
            CloseClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardFormatNameW,
            GetClipboardOwner, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
            OpenClipboard,
        },
        Memory::{GlobalLock, GlobalSize, GlobalUnlock},
        Ole::{CF_BITMAP, CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT},
    },
    UI::{Shell::DragQueryFileW, WindowsAndMessaging::GetForegroundWindow},
};

use super::formats::{self, MAX_CLIPBOARD_BYTES};
use crate::{ImageEncoding, ReadError, ReadPurpose, Snapshot};

struct Open;
impl Drop for Open {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}

pub fn sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

pub fn read_snapshot(purpose: ReadPurpose, expected: Option<u32>) -> Result<Snapshot, ReadError> {
    let mut opened = false;
    for attempt in 0..5 {
        if unsafe { OpenClipboard(null_mut()) } != 0 {
            opened = true;
            break;
        }
        if attempt < 4 {
            thread::sleep(Duration::from_millis(20));
        }
    }
    if !opened {
        return Err(ReadError::Busy);
    }
    let _lock = Open;
    let at_lock = sequence();
    if expected.is_some_and(|expected| expected != at_lock) {
        return Err(ReadError::Changed);
    }
    let ids = formats::formats();
    let mut out = Snapshot::new(at_lock);
    out.owner = unsafe { GetClipboardOwner() } as usize;
    out.foreground = unsafe { GetForegroundWindow() } as usize;
    out.has_formats = unsafe { EnumClipboardFormats(0) } != 0;
    out.excluded = ids.exclude != 0 && unsafe { IsClipboardFormatAvailable(ids.exclude) } != 0;
    if purpose == ReadPurpose::Monitor && out.excluded {
        return Ok(out);
    }
    if unsafe { IsClipboardFormatAvailable(CF_HDROP as u32) } != 0 {
        out.files = unsafe { read_file_paths() }?;
        if !out.files.is_empty() {
            // The copied object is the file reference list. Incidental text and
            // bitmaps must not consume its budget or trigger delayed decoding.
            out.completed_sequence = sequence();
            if out.byte_len() > MAX_CLIPBOARD_BYTES {
                return Err(ReadError::TooLarge);
            }
            return Ok(out);
        }
    }
    let mut remaining = MAX_CLIPBOARD_BYTES;
    let mut read = |format| -> Result<Option<Vec<u8>>, ReadError> {
        if format == 0 {
            return Ok(None);
        }
        let bytes = unsafe { read_global(format, remaining) }.map_err(|e| {
            if e.contains("limit") {
                ReadError::TooLarge
            } else {
                ReadError::Platform(e)
            }
        })?;
        if let Some(bytes) = &bytes {
            remaining -= bytes.len();
        }
        Ok(bytes)
    };
    if let Some(bytes) = read(CF_UNICODETEXT as u32)? {
        let text = String::from_utf16_lossy(&bytes_to_utf16(&bytes));
        if purpose == ReadPurpose::ExplicitPin || !text.trim().is_empty() {
            out.text = Some(text);
        }
    }
    out.html = read(ids.html)?;
    out.rtf = read(ids.rtf)?;
    // Keep candidates in preference order; a corrupt PNG may still have a valid DIB fallback.
    for (format, encoding) in [
        (ids.png, ImageEncoding::Png),
        (CF_DIBV5 as u32, ImageEncoding::Dib),
        (CF_DIB as u32, ImageEncoding::Dib),
    ] {
        match read(format) {
            Ok(Some(bytes)) => out.images.push((encoding, bytes)),
            Err(ReadError::TooLarge) => out.oversized_image = true,
            Err(ReadError::InvalidImage) => {}
            Err(error) => return Err(error),
            Ok(None) => {}
        }
    }
    if out.images.is_empty() && unsafe { IsClipboardFormatAvailable(CF_BITMAP as u32) } != 0 {
        match unsafe { read_bitmap_dib(remaining) } {
            Ok(Some(bytes)) => out.images.push((ImageEncoding::Dib, bytes)),
            Ok(None) => {}
            Err(ReadError::TooLarge) => out.oversized_image = true,
            Err(error) => return Err(error),
        }
    }
    for name in [
        "FileGroupDescriptorW",
        "FileGroupDescriptor",
        "FileContents",
    ] {
        let wide = formats::wide_nul(name);
        let format = unsafe {
            windows_sys::Win32::System::DataExchange::RegisterClipboardFormatW(wide.as_ptr())
        };
        out.virtual_files |= format != 0 && unsafe { IsClipboardFormatAvailable(format) } != 0;
    }
    out.completed_sequence = sequence(); // delayed materialization under this lock is allowed
    if out.byte_len() > MAX_CLIPBOARD_BYTES {
        return Err(ReadError::TooLarge);
    }
    Ok(out)
}
/// Convert a clipboard-owned HBITMAP (CF_BITMAP) into a raw 32-bit DIB.
/// The returned bytes use the same layout accepted by `image_norm::dib_to_png`.
///
/// # Safety
/// The clipboard must be open on the calling thread.
unsafe fn read_bitmap_dib(remaining: usize) -> Result<Option<Vec<u8>>, ReadError> {
    let handle = unsafe { GetClipboardData(CF_BITMAP as u32) };
    if handle.is_null() {
        return Ok(None);
    }

    let mut bitmap: BITMAP = unsafe { zeroed() };
    let copied = unsafe {
        GetObjectW(
            handle,
            std::mem::size_of::<BITMAP>() as i32,
            (&mut bitmap as *mut BITMAP).cast(),
        )
    };
    if copied == 0 || bitmap.bmWidth <= 0 || bitmap.bmHeight <= 0 {
        return Err(ReadError::InvalidImage);
    }
    let width = u32::try_from(bitmap.bmWidth).map_err(|_| ReadError::InvalidImage)?;
    let height = u32::try_from(bitmap.bmHeight).map_err(|_| ReadError::InvalidImage)?;
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(ReadError::TooLarge)?;
    if pixels > crate::MAX_PIXELS {
        return Err(ReadError::TooLarge);
    }
    let image_size = pixels
        .checked_mul(4)
        .and_then(|size| usize::try_from(size).ok())
        .ok_or(ReadError::TooLarge)?;
    if image_size
        .checked_add(std::mem::size_of::<BITMAPINFOHEADER>())
        .is_none_or(|size| size > remaining)
    {
        return Err(ReadError::TooLarge);
    }

    let mut header: BITMAPINFO = unsafe { zeroed() };
    header.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: bitmap.bmWidth,
        biHeight: bitmap.bmHeight,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        biSizeImage: u32::try_from(image_size).map_err(|_| ReadError::TooLarge)?,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };
    let mut pixels_bgra = vec![0u8; image_size];
    let screen_dc = unsafe { GetDC(null_mut()) };
    if screen_dc.is_null() {
        return Err(ReadError::Platform(last_error("GetDC")));
    }
    let copied_lines = unsafe {
        GetDIBits(
            screen_dc,
            handle,
            0,
            height,
            pixels_bgra.as_mut_ptr().cast(),
            &mut header as *mut BITMAPINFO,
            DIB_RGB_COLORS,
        )
    };
    unsafe { ReleaseDC(null_mut(), screen_dc) };
    if copied_lines <= 0 {
        return Err(ReadError::Platform(last_error("GetDIBits")));
    }

    let mut dib = Vec::with_capacity(std::mem::size_of::<BITMAPINFOHEADER>() + image_size);
    let header_bytes = unsafe {
        std::slice::from_raw_parts(
            (&header.bmiHeader as *const BITMAPINFOHEADER).cast::<u8>(),
            std::mem::size_of::<BITMAPINFOHEADER>(),
        )
    };
    dib.extend_from_slice(header_bytes);
    dib.extend_from_slice(&pixels_bgra);
    Ok(Some(dib))
}

/// Log every advertised clipboard format. Used only to diagnose "no image" cases.
///
/// # Safety
/// The clipboard must be open on the calling thread.
pub unsafe fn log_clipboard_formats() {
    let mut formats = Vec::new();
    let mut previous = 0u32;
    for _ in 0..64 {
        let format = unsafe { EnumClipboardFormats(previous) };
        if format == 0 {
            break;
        }
        let mut name = [0u16; 128];
        let name_len =
            unsafe { GetClipboardFormatNameW(format, name.as_mut_ptr(), name.len() as i32) };
        let label = if name_len > 0 {
            String::from_utf16_lossy(&name[..name_len as usize])
        } else {
            match format {
                2 => "CF_BITMAP".into(),
                8 => "CF_DIB".into(),
                13 => "CF_UNICODETEXT".into(),
                15 => "CF_HDROP".into(),
                17 => "CF_DIBV5".into(),
                _ => "standard/unnamed".into(),
            }
        };
        formats.push(format!("{format}:{label}"));
        previous = format;
    }
    if !formats.is_empty() {
        eprintln!("[snapclip][clipboard] no supported payload; formats={formats:?}");
    }
}

/// # Safety
/// The clipboard must be open on the calling thread.
unsafe fn read_global(format: u32, max_size: usize) -> Result<Option<Vec<u8>>, String> {
    let handle = unsafe { GetClipboardData(format) };
    if handle.is_null() {
        return Ok(None);
    }
    let size = unsafe { GlobalSize(handle) };
    if size > max_size {
        return Err("clipboard size limit".into());
    }
    if size == 0 {
        return Ok(None);
    }
    let source = unsafe { GlobalLock(handle) } as *const u8;
    if source.is_null() {
        return Err(last_error("GlobalLock"));
    }
    let bytes = unsafe { std::slice::from_raw_parts(source, size) }.to_vec();
    unsafe { GlobalUnlock(handle) };
    Ok(Some(bytes))
}

/// # Safety
/// The clipboard must be open on the calling thread.
unsafe fn read_file_paths() -> Result<Vec<String>, ReadError> {
    let handle = unsafe { GetClipboardData(CF_HDROP as u32) };
    if handle.is_null() {
        return Ok(Vec::new());
    }
    let drop_handle = handle;
    let count = unsafe { DragQueryFileW(drop_handle, u32::MAX, null_mut(), 0) };
    let mut paths = Vec::with_capacity(count.min(1024) as usize);
    if count > crate::MAX_FILES as u32 {
        return Err(ReadError::TooLarge);
    }
    for index in 0..count {
        let length = unsafe { DragQueryFileW(drop_handle, index, null_mut(), 0) } as usize;
        if length == 0 {
            return Err(ReadError::Platform("文件引用包含空路径".into()));
        }
        if length > 32767 {
            return Err(ReadError::TooLarge);
        }
        let mut wide = vec![0u16; length + 1];
        let written =
            unsafe { DragQueryFileW(drop_handle, index, wide.as_mut_ptr(), wide.len() as u32) }
                as usize;
        if written != length {
            return Err(ReadError::Platform("文件路径未完整读取".into()));
        }
        paths.push(
            String::from_utf16(&wide[..written])
                .map_err(|_| ReadError::Platform("文件路径无法表示为 Unicode 文本".into()))?,
        );
    }
    Ok(paths)
}

fn bytes_to_utf16(bytes: &[u8]) -> Vec<u16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|value| *value != 0)
        .collect()
}

pub fn last_error(operation: &str) -> String {
    format!("{operation} failed: {}", std::io::Error::last_os_error())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf16_ends_at_first_nul() {
        assert_eq!(
            String::from_utf16_lossy(&bytes_to_utf16(&[65, 0, 0, 0, 66, 0])),
            "A"
        );
    }
    #[test]
    fn snapshot_keeps_sequence_and_event_window_handles() {
        let mut snapshot = Snapshot::new(7).with_text("payload");
        snapshot.owner = 0x1234;
        snapshot.foreground = 0x5678;
        assert_eq!(snapshot.sequence(), 7);
        assert_eq!(snapshot.owner(), 0x1234);
        assert_eq!(snapshot.foreground(), 0x5678);
        assert_eq!(snapshot.text(), Some("payload"));
    }
}
