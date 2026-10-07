//! Raw clipboard reading: formats → payload bytes.
//!
//! Only this module (and [`super::image_norm`]) understands Windows clipboard
//! formats. Everything below returns platform-neutral byte payloads so the
//! application layer never sees a HGLOBAL, DIB or CF_* constant.

use std::{
    ffi::c_void,
    mem::zeroed,
    ptr::null_mut,
    thread,
    time::Duration,
};

use windows_sys::Win32::{
    Foundation::GetLastError,
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
    UI::{
        Shell::DragQueryFileW,
        WindowsAndMessaging::GetForegroundWindow,
    },
};

use snapclip_model::{ImageDimensions, PayloadData, PayloadKind, PayloadRef};

use super::{
    formats::{self, MAX_CLIPBOARD_BYTES},
    image_norm,
};

/// One clipboard observation, already normalised to payload bytes.
#[derive(Debug, Clone)]
pub struct ClipboardSnapshot {
    pub sequence: u32,
    pub owner: usize,
    pub foreground: usize,
    pub payloads: Vec<PayloadData>,
}

/// Open the clipboard with a short retry loop (other processes may hold it).
pub fn open_clipboard() -> Result<(), String> {
    for attempt in 0..5 {
        if unsafe { OpenClipboard(null_mut()) } != 0 {
            return Ok(());
        }
        if attempt < 4 {
            thread::sleep(Duration::from_millis(20));
        }
    }
    Err(last_error("OpenClipboard"))
}

/// Read the currently open clipboard into payload bytes.
///
/// The caller owns the clipboard lock; this function never opens or closes it.
///
/// # Safety
/// The clipboard must be open on the calling thread.
pub unsafe fn read_open_clipboard(ids: formats::ClipboardFormats) -> Result<Vec<PayloadData>, String> {
    if ids.exclude != 0 && unsafe { IsClipboardFormatAvailable(ids.exclude) } != 0 {
        // SnapClip wrote this content itself: never re-capture it.
        return Ok(Vec::new());
    }

    let mut captured = Vec::new();
    let mut total_size = 0usize;

    if let Some(bytes) =
        unsafe { read_global(CF_UNICODETEXT as u32, MAX_CLIPBOARD_BYTES - total_size)? }
    {
        let text = String::from_utf16_lossy(&bytes_to_utf16(&bytes));
        let text = text.trim_end_matches('\0');
        if !text.trim().is_empty() {
            push_payload(
                &mut captured,
                &mut total_size,
                PayloadKind::Text,
                None,
                text.as_bytes().to_vec(),
            )?;
        }
    }

    for (format, kind) in [
        (ids.html, PayloadKind::Html),
        (ids.rtf, PayloadKind::Rtf),
    ] {
        if format != 0 {
            if let Some(bytes) = unsafe { read_global(format, MAX_CLIPBOARD_BYTES - total_size)? } {
                if !bytes.is_empty() {
                    push_payload(&mut captured, &mut total_size, kind, None, bytes)?;
                }
            }
        }
    }

    if unsafe { IsClipboardFormatAvailable(CF_HDROP as u32) } != 0 {
        let paths = unsafe { read_file_paths()? };
        if !paths.is_empty() {
            let bytes = serde_json::to_vec(&paths).map_err(|error| error.to_string())?;
            push_payload(
                &mut captured,
                &mut total_size,
                PayloadKind::Files,
                None,
                bytes,
            )?;
        }
    }

    // Prefer PNG; otherwise normalise DIB/DIBV5/CF_BITMAP → PNG so the store and
    // OCR never see raw Windows bitmap formats.
    let mut image_png: Option<(Option<ImageDimensions>, Vec<u8>)> = None;
    for format in [ids.png, CF_DIBV5 as u32, CF_DIB as u32] {
        if format == 0 || unsafe { IsClipboardFormatAvailable(format) } == 0 {
            continue;
        }
        let Some(bytes) = (unsafe { read_global(format, MAX_CLIPBOARD_BYTES - total_size) })?
        else {
            continue;
        };
        if bytes.is_empty() {
            continue;
        }
        let normalized = if format == ids.png {
            image_norm::normalize_png(&bytes)
        } else {
            image_norm::dib_to_png(&bytes)
        };
        match normalized {
            Ok((png, width, height)) => {
                image_png = Some((Some(ImageDimensions { width, height }), png));
                break;
            }
            Err(error) => {
                eprintln!("[snapclip][clipboard] image normalize failed: {error}");
            }
        }
    }

    if image_png.is_none() && unsafe { IsClipboardFormatAvailable(CF_BITMAP as u32) } != 0 {
        match unsafe { read_bitmap_dib() } {
            Ok(Some(bytes)) => match image_norm::dib_to_png(&bytes) {
                Ok((png, width, height)) => {
                    image_png = Some((Some(ImageDimensions { width, height }), png));
                }
                Err(error) => {
                    eprintln!("[snapclip][clipboard] CF_BITMAP normalize failed: {error}");
                }
            },
            Ok(None) => {}
            Err(error) => {
                eprintln!("[snapclip][clipboard] CF_BITMAP read failed: {error}");
            }
        }
    }

    if let Some((dimensions, bytes)) = image_png {
        push_payload(
            &mut captured,
            &mut total_size,
            PayloadKind::Image,
            dimensions,
            bytes,
        )?;
    }

    if captured.is_empty() && unsafe { EnumClipboardFormats(0) } != 0 {
        // The clipboard has formats but none of them were usable.
        unsafe { log_clipboard_formats() };
    }

    Ok(captured)
}

/// Read the clipboard end to end: open, read, close.
pub fn read_snapshot(ids: formats::ClipboardFormats) -> Result<ClipboardSnapshot, String> {
    open_clipboard()?;
    let sequence = unsafe { GetClipboardSequenceNumber() };
    let owner = unsafe { GetClipboardOwner() };
    let foreground = unsafe { GetForegroundWindow() };
    let result = unsafe { read_open_clipboard(ids) };
    unsafe { CloseClipboard() };
    let payloads = result?;
    Ok(ClipboardSnapshot {
        sequence,
        owner: owner as usize,
        foreground: foreground as usize,
        payloads,
    })
}

/// Convert a clipboard-owned HBITMAP (CF_BITMAP) into a raw 32-bit DIB.
/// The returned bytes use the same layout accepted by `image_norm::dib_to_png`.
///
/// # Safety
/// The clipboard must be open on the calling thread.
unsafe fn read_bitmap_dib() -> Result<Option<Vec<u8>>, String> {
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
        return Err("invalid CF_BITMAP dimensions".into());
    }
    let width = u32::try_from(bitmap.bmWidth).map_err(|_| "invalid CF_BITMAP width")?;
    let height = u32::try_from(bitmap.bmHeight).map_err(|_| "invalid CF_BITMAP height")?;
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| "CF_BITMAP dimensions overflow".to_string())?;
    if pixels > image_norm::MAX_DECODE_PIXELS {
        return Err("image exceeds max decode pixels".into());
    }
    let image_size = pixels
        .checked_mul(4)
        .and_then(|size| usize::try_from(size).ok())
        .ok_or_else(|| "CF_BITMAP image size overflow".to_string())?;
    if image_size > MAX_CLIPBOARD_BYTES {
        return Err("CF_BITMAP exceeds clipboard size limit".into());
    }

    let mut header: BITMAPINFO = unsafe { zeroed() };
    header.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: bitmap.bmWidth,
        biHeight: bitmap.bmHeight,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        biSizeImage: u32::try_from(image_size).map_err(|_| "CF_BITMAP image too large")?,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };
    let mut pixels_bgra = vec![0u8; image_size];
    let screen_dc = unsafe { GetDC(null_mut()) };
    if screen_dc.is_null() {
        return Err(last_error("GetDC"));
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
        return Err(last_error("GetDIBits"));
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
    if size == 0 || size > max_size {
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
unsafe fn read_file_paths() -> Result<Vec<String>, String> {
    let handle = unsafe { GetClipboardData(CF_HDROP as u32) };
    if handle.is_null() {
        return Ok(Vec::new());
    }
    let drop_handle = handle as *mut c_void;
    let count = unsafe { DragQueryFileW(drop_handle, u32::MAX, null_mut(), 0) };
    let mut paths = Vec::with_capacity(count.min(1024) as usize);
    for index in 0..count.min(1024) {
        let length = unsafe { DragQueryFileW(drop_handle, index, null_mut(), 0) } as usize;
        if length == 0 || length > 8192 {
            continue;
        }
        let mut wide = vec![0u16; length + 1];
        let written =
            unsafe { DragQueryFileW(drop_handle, index, wide.as_mut_ptr(), wide.len() as u32) }
                as usize;
        if written > 0 && written <= length {
            paths.push(String::from_utf16_lossy(&wide[..written]));
        }
    }
    Ok(paths)
}

fn bytes_to_utf16(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|value| *value != 0)
        .collect()
}

fn push_payload(
    captured: &mut Vec<PayloadData>,
    total_size: &mut usize,
    kind: PayloadKind,
    dimensions: Option<ImageDimensions>,
    bytes: Vec<u8>,
) -> Result<(), String> {
    *total_size = total_size
        .checked_add(bytes.len())
        .ok_or_else(|| "clipboard payload size overflow".to_string())?;
    if *total_size > MAX_CLIPBOARD_BYTES {
        return Err("clipboard snapshot exceeds the size limit".into());
    }
    // Sequence numbers change between reads, so identity is derived from the index
    // and content. The application layer assigns the final publication id.
    let index = captured.len();
    let payload = PayloadRef {
        payload_id: format!("payload-{index}-{}", blake3::hash(&bytes).to_hex()),
        content_hash: blake3::hash(&bytes).to_hex().to_string(),
        kind: kind.clone(),
        size_bytes: bytes.len() as u64,
        mime_type: Some(kind.default_mime_type().to_string()),
        image_dimensions: dimensions,
    };
    captured.push(PayloadData::new(payload, bytes));
    Ok(())
}

#[cfg(test)]
pub(crate) fn dib_dimensions(bytes: &[u8]) -> Option<ImageDimensions> {
    if bytes.len() < 12 {
        return None;
    }
    let width = i32::from_le_bytes(bytes[4..8].try_into().ok()?).unsigned_abs();
    let height = i32::from_le_bytes(bytes[8..12].try_into().ok()?).unsigned_abs();
    if width == 0 || height == 0 {
        return None;
    }
    Some(ImageDimensions { width, height })
}

pub fn last_error(operation: &str) -> String {
    format!("{operation} failed with Win32 error {}", unsafe {
        GetLastError()
    })
}

#[cfg(test)]
mod tests {
    use super::{bytes_to_utf16, dib_dimensions};
    use snapclip_model::{PayloadData, PayloadKind};

    #[test]
    fn decodes_null_terminated_utf16_clipboard_data() {
        let bytes = [b'h', 0, b'i', 0, 0, 0, 1];
        assert_eq!(String::from_utf16_lossy(&bytes_to_utf16(&bytes)), "hi");
    }

    #[test]
    fn reads_dib_dimensions_from_bitmap_info_header() {
        let mut dib = vec![0; 40];
        dib[..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1920i32.to_le_bytes());
        dib[8..12].copy_from_slice(&(-1080i32).to_le_bytes());
        assert_eq!(dib_dimensions(&dib).unwrap().width, 1920);
        assert_eq!(dib_dimensions(&dib).unwrap().height, 1080);
    }

    fn payload(kind: PayloadKind, bytes: &[u8]) -> PayloadData {
        PayloadData::new(
            snapclip_model::PayloadRef {
                payload_id: "p".into(),
                content_hash: "h".into(),
                kind,
                size_bytes: bytes.len() as u64,
                mime_type: None,
                image_dimensions: None,
            },
            bytes.to_vec(),
        )
    }

    #[test]
    fn snapshot_keeps_the_event_window_handles() {
        // The owner/foreground snapshot is taken at event time, so it must survive
        // into the snapshot the application layer reads.
        let snapshot = super::ClipboardSnapshot {
            sequence: 7,
            owner: 0x1234,
            foreground: 0x5678,
            payloads: vec![payload(PayloadKind::Image, b"i")],
        };
        assert_eq!(snapshot.sequence, 7);
        assert_eq!(snapshot.owner, 0x1234);
        assert_eq!(snapshot.foreground, 0x5678);
        assert_eq!(snapshot.payloads.len(), 1);
    }
}

