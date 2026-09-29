use std::{
    ffi::c_void,
    mem::zeroed,
    ptr::{null, null_mut},
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const CLIPBOARD_UPDATED_EVENT: &str = "clipboard://updated.v1";

use windows_sys::Win32::{
    Foundation::{GetLastError, GlobalFree, HWND, LPARAM, LRESULT, WPARAM},
    Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, GetDC, GetDIBits, GetObjectW,
        ReleaseDC,
    },
    System::{
        DataExchange::{
            CloseClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardFormatNameW,
            GetClipboardOwner, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
            OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
        },
        LibraryLoader::GetModuleHandleW,
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
        Ole::{CF_BITMAP, CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT},
        Threading::GetCurrentThreadId,
    },
    UI::{
        Shell::DragQueryFileW,
        WindowsAndMessaging::{
            CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
            DispatchMessageW, GetMessageW, MSG, PeekMessageW, PostThreadMessageW, RegisterClassW,
            TranslateMessage, UnregisterClassW, WM_CLIPBOARDUPDATE, WM_QUIT, WNDCLASSW,
        },
    },
};

use crate::{
    domain::{ClipboardPublication, ImageDimensions, PayloadKind, PayloadRef},
    store::{PayloadData, QueueDecision, Store},
};

use super::{
    image_norm,
    source_app::{self, SourceWindowSnapshot},
};

const LISTENER_CLASS: &[u16] = &[
    83, 110, 97, 112, 67, 108, 105, 112, 67, 108, 105, 112, 98, 111, 97, 114, 100, 0,
];
const LISTENER_TITLE: &[u16] = &[83, 110, 97, 112, 67, 108, 105, 112, 0];
const HTML_FORMAT: &[u16] = &[72, 84, 77, 76, 32, 70, 111, 114, 109, 97, 116, 0];
const RTF_FORMAT: &[u16] = &[
    82, 105, 99, 104, 32, 84, 101, 120, 116, 32, 70, 111, 114, 109, 97, 116, 0,
];
const PNG_FORMAT: &[u16] = &[80, 78, 71, 0];
const EXCLUDE_FORMAT_NAME: &str = "ExcludeClipboardContentFromMonitorProcessing";
const MAX_CLIPBOARD_BYTES: usize = 64 * 1024 * 1024;
const EVENT_QUEUE_CAPACITY: usize = 32;

/// Marks clipboard content written by SnapClip so the monitor does not capture it again.
/// The marker is a private one-byte format; consumer applications ignore it.
pub fn mark_clipboard_excluded() {
    let format_name = wide_nul(EXCLUDE_FORMAT_NAME);
    let format = unsafe { RegisterClipboardFormatW(format_name.as_ptr()) };
    if format == 0 || unsafe { OpenClipboard(null_mut()) } == 0 {
        return;
    }
    unsafe {
        let handle = GlobalAlloc(GMEM_MOVEABLE, 1);
        if !handle.is_null() {
            let ptr = GlobalLock(handle);
            if !ptr.is_null() {
                *(ptr as *mut u8) = 1;
                GlobalUnlock(handle);
                if SetClipboardData(format, handle).is_null() {
                    // Ownership remains with us when SetClipboardData fails.
                    GlobalFree(handle);
                }
            } else {
                GlobalFree(handle);
            }
        }
        CloseClipboard();
    }
}

pub struct ClipboardMonitor {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl ClipboardMonitor {
    pub fn start(
        store: Store,
        ocr: Option<crate::ocr::OcrEnqueuer>,
        app: tauri::AppHandle,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("snapclip-clipboard-listener".into())
            .spawn(move || listener_thread(store, ocr, app, ready_tx))?;
        let thread_id = ready_rx.recv()??;
        Ok(Self {
            thread_id,
            thread: Some(thread),
        })
    }
}

impl Drop for ClipboardMonitor {
    fn drop(&mut self) {
        unsafe {
            PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn listener_thread(
    store: Store,
    ocr: Option<crate::ocr::OcrEnqueuer>,
    app: tauri::AppHandle,
    ready: SyncSender<Result<u32, String>>,
) {
    let thread_id = unsafe { GetCurrentThreadId() };
    let mut initial_message: MSG = unsafe { zeroed() };
    unsafe { PeekMessageW(&mut initial_message, null_mut(), 0, 0, 0) };
    let instance = unsafe { GetModuleHandleW(null()) };
    if instance.is_null() {
        let _ = ready.send(Err(last_error("GetModuleHandleW")));
        return;
    }

    let window_class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: null_mut(),
        hCursor: null_mut(),
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: LISTENER_CLASS.as_ptr(),
    };
    if unsafe { RegisterClassW(&window_class) } == 0 {
        let _ = ready.send(Err(last_error("RegisterClassW")));
        return;
    }

    let window = unsafe {
        CreateWindowExW(
            0,
            LISTENER_CLASS.as_ptr(),
            LISTENER_TITLE.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null(),
        )
    };
    if window.is_null() {
        unsafe { UnregisterClassW(LISTENER_CLASS.as_ptr(), instance) };
        let _ = ready.send(Err(last_error("CreateWindowExW")));
        return;
    }

    let (event_tx, event_rx) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
    let worker = thread::Builder::new()
        .name("snapclip-clipboard-worker".into())
        .spawn(move || clipboard_worker(event_rx, store, ocr, app));
    let worker = match worker {
        Ok(worker) => worker,
        Err(error) => {
            unsafe {
                DestroyWindow(window);
                UnregisterClassW(LISTENER_CLASS.as_ptr(), instance);
            }
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };

    if unsafe { AddClipboardFormatListener(window) } == 0 {
        unsafe {
            DestroyWindow(window);
            UnregisterClassW(LISTENER_CLASS.as_ptr(), instance);
        }
        drop(event_tx);
        let _ = worker.join();
        let _ = ready.send(Err(last_error("AddClipboardFormatListener")));
        return;
    }

    if ready.send(Ok(thread_id)).is_err() {
        unsafe {
            RemoveClipboardFormatListener(window);
            DestroyWindow(window);
            UnregisterClassW(LISTENER_CLASS.as_ptr(), instance);
        }
        drop(event_tx);
        let _ = worker.join();
        return;
    }

    let mut message: MSG = unsafe { zeroed() };
    loop {
        let result = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
        if result <= 0 {
            break;
        }
        if message.message == WM_CLIPBOARDUPDATE {
            // Snapshot owner/foreground immediately: the worker may run later,
            // after the user has already switched windows.
            let snapshot = SourceWindowSnapshot::capture();
            let sequence = unsafe { GetClipboardSequenceNumber() };
            let event = ClipboardEvent {
                sequence,
                owner: snapshot.owner,
                foreground: snapshot.foreground,
            };
            match event_tx.try_send(event) {
                Ok(()) | Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => break,
            }
        }
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    unsafe {
        RemoveClipboardFormatListener(window);
        DestroyWindow(window);
        UnregisterClassW(LISTENER_CLASS.as_ptr(), instance);
    }
    drop(event_tx);
    let _ = worker.join();
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

fn clipboard_worker(
    events: Receiver<ClipboardEvent>,
    store: Store,
    ocr: Option<crate::ocr::OcrEnqueuer>,
    app: tauri::AppHandle,
) {
    let html_format = unsafe { RegisterClipboardFormatW(HTML_FORMAT.as_ptr()) };
    let rtf_format = unsafe { RegisterClipboardFormatW(RTF_FORMAT.as_ptr()) };
    let png_format = unsafe { RegisterClipboardFormatW(PNG_FORMAT.as_ptr()) };
    let exclude_format_name = wide_nul(EXCLUDE_FORMAT_NAME);
    let exclude_format = unsafe { RegisterClipboardFormatW(exclude_format_name.as_ptr()) };
    let mut last_sequence = None;

    for event in events {
        if last_sequence == Some(event.sequence) {
            continue;
        }
        let captured_at_unix_ms = unix_time_ms();
        let mut clipboard_result = None;
        for attempt in 0..5 {
            let Ok(result) = read_clipboard(html_format, rtf_format, png_format, exclude_format)
            else {
                if attempt < 4 {
                    thread::sleep(Duration::from_millis(40));
                }
                continue;
            };
            let has_image = result
                .2
                .iter()
                .any(|payload| payload.0 == PayloadKind::Image);
            let has_rich_text = result
                .2
                .iter()
                .any(|payload| matches!(payload.0, PayloadKind::Html | PayloadKind::Rtf));
            if has_image || (!has_rich_text && !result.2.is_empty()) {
                clipboard_result = Some(result);
                break;
            }
            // Word and some screenshot tools publish delayed clipboard formats.
            // Give them a short window before treating the event as text-only.
            clipboard_result = Some(result);
            if attempt < 4 {
                thread::sleep(Duration::from_millis(40));
            }
        }
        let Some((current_sequence, owner_now, mut captured)) = clipboard_result else {
            continue;
        };
        if last_sequence == Some(current_sequence) {
            continue;
        }
        if captured.is_empty() {
            last_sequence = Some(current_sequence);
            continue;
        }

        let source = source_app::resolve_source(SourceWindowSnapshot {
            owner: if event.owner != 0 {
                event.owner
            } else {
                owner_now as usize
            },
            foreground: event.foreground,
        });

        let publication_id = format!("clipboard-{captured_at_unix_ms}-{current_sequence}");
        let payloads = captured
            .drain(..)
            .enumerate()
            .map(|(index, (kind, mime_type, image_dimensions, bytes))| {
                let payload = PayloadRef {
                    payload_id: format!("{publication_id}-{index}"),
                    content_hash: blake3::hash(&bytes).to_hex().to_string(),
                    kind,
                    size_bytes: bytes.len() as u64,
                    mime_type: Some(mime_type),
                    image_dimensions,
                };
                PayloadData { payload, bytes }
            })
            .collect::<Vec<_>>();
        let publication = ClipboardPublication {
            publication_id,
            captured_at_unix_ms,
            source_app: source.as_ref().map(|info| info.display_name.clone()),
            source_exe_path: source.as_ref().map(|info| info.exe_path.clone()),
            payloads: payloads
                .iter()
                .map(|payload| payload.payload.clone())
                .collect(),
        };
        if store
            .save_publication(publication.clone(), payloads)
            .is_ok()
        {
            last_sequence = Some(current_sequence);
            let _ = tauri::Emitter::emit(&app, CLIPBOARD_UPDATED_EVENT, ());
            if let Some(image) = publication
                .payloads
                .iter()
                .find(|payload| payload.kind == PayloadKind::Image)
            {
                let clip_id = publication.publication_id.clone();
                let hash = image.content_hash.clone();
                match store.enqueue_ocr(clip_id.clone(), hash.clone()) {
                    Ok(QueueDecision::Enqueued { attempt }) => {
                        let queued = ocr
                            .as_ref()
                            .map(|queue| queue.try_enqueue(&clip_id, &hash))
                            .unwrap_or(true);
                        if !queued {
                            let _ = store.release_queued(clip_id, attempt);
                        }
                    }
                    Ok(_) => {}
                    Err(_) => {}
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ClipboardEvent {
    sequence: u32,
    owner: usize,
    foreground: usize,
}

type CapturedPayload = (PayloadKind, String, Option<ImageDimensions>, Vec<u8>);

fn read_clipboard(
    html_format: u32,
    rtf_format: u32,
    png_format: u32,
    exclude_format: u32,
) -> Result<(u32, HWND, Vec<CapturedPayload>), String> {
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
        return Err(last_error("OpenClipboard"));
    }
    let sequence = unsafe { GetClipboardSequenceNumber() };
    let owner = unsafe { GetClipboardOwner() };
    let result =
        unsafe { read_open_clipboard(html_format, rtf_format, png_format, exclude_format) }
            .map(|payloads| (sequence, owner, payloads));
    unsafe { CloseClipboard() };
    result
}

unsafe fn read_open_clipboard(
    html_format: u32,
    rtf_format: u32,
    png_format: u32,
    exclude_format: u32,
) -> Result<Vec<CapturedPayload>, String> {
    if exclude_format != 0 && unsafe { IsClipboardFormatAvailable(exclude_format) } != 0 {
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
                "text/plain;charset=utf-8",
                None,
                text.as_bytes().to_vec(),
            )?;
        }
    }

    for (format, kind, mime_type) in [
        (html_format, PayloadKind::Html, "text/html"),
        (rtf_format, PayloadKind::Rtf, "text/rtf"),
    ] {
        if format != 0 {
            if let Some(bytes) = unsafe { read_global(format, MAX_CLIPBOARD_BYTES - total_size)? } {
                if !bytes.is_empty() {
                    push_payload(&mut captured, &mut total_size, kind, mime_type, None, bytes)?;
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
                "application/json",
                None,
                bytes,
            )?;
        }
    }

    // Prefer PNG; otherwise normalize DIB/DIBV5/CF_BITMAP → PNG so Store/OCR
    // never see raw Windows bitmap formats.
    let mut image_png: Option<(Option<ImageDimensions>, Vec<u8>)> = None;
    for format in [png_format, CF_DIBV5 as u32, CF_DIB as u32] {
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
        if format == png_format {
            match image_norm::normalize_png(&bytes) {
                Ok((png, width, height)) => {
                    image_png = Some((Some(ImageDimensions { width, height }), png));
                    break;
                }
                Err(error) => {
                    eprintln!("[snapclip][clipboard] png normalize failed: {error}");
                    continue;
                }
            }
        }
        match image_norm::dib_to_png(&bytes) {
            Ok((png, width, height)) => {
                image_png = Some((Some(ImageDimensions { width, height }), png));
                break;
            }
            Err(error) => {
                eprintln!("[snapclip][clipboard] dib normalize failed: {error}");
                continue;
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
    if image_png.is_none() {
        unsafe { log_clipboard_formats() };
    }
    if let Some((dimensions, bytes)) = image_png {
        push_payload(
            &mut captured,
            &mut total_size,
            PayloadKind::Image,
            "image/png",
            dimensions,
            bytes,
        )?;
    }

    Ok(captured)
}

/// Convert a clipboard-owned HBITMAP (CF_BITMAP) into a raw 32-bit DIB.
/// The returned bytes intentionally use the same format accepted by `dib_to_png`.
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
    if pixels > 24_000_000 {
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

unsafe fn log_clipboard_formats() {
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
        eprintln!(
            "[snapclip][clipboard] no supported image payload; clipboard formats={formats:?}"
        );
    }
}

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

#[cfg(test)]
fn dib_dimensions(bytes: &[u8]) -> Option<ImageDimensions> {
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

fn push_payload(
    captured: &mut Vec<CapturedPayload>,
    total_size: &mut usize,
    kind: PayloadKind,
    mime_type: &str,
    dimensions: Option<ImageDimensions>,
    bytes: Vec<u8>,
) -> Result<(), String> {
    *total_size = total_size
        .checked_add(bytes.len())
        .ok_or_else(|| "clipboard payload size overflow".to_string())?;
    if *total_size > MAX_CLIPBOARD_BYTES {
        return Err("clipboard snapshot exceeds the size limit".into());
    }
    captured.push((kind, mime_type.into(), dimensions, bytes));
    Ok(())
}

fn unix_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as i64
}

fn wide_nul(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_error(operation: &str) -> String {
    format!("{operation} failed with Win32 error {}", unsafe {
        GetLastError()
    })
}

#[link(name = "user32")]
unsafe extern "system" {
    fn AddClipboardFormatListener(hwnd: HWND) -> i32;
    fn RemoveClipboardFormatListener(hwnd: HWND) -> i32;
}

#[cfg(test)]
mod tests {
    use super::{bytes_to_utf16, dib_dimensions};

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
}
