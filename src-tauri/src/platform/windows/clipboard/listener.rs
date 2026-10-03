//! `WM_CLIPBOARDUPDATE` listener window and its dedicated message loop.
//!
//! The listener only *reports* that the clipboard changed. Reading, retrying,
//! deduplicating and persisting happen in the application layer, so a slow write to
//! the database can never stall clipboard notifications.

use std::{
    mem::zeroed,
    ptr::{null, null_mut},
    sync::mpsc::{self, SyncSender},
    thread::{self, JoinHandle},
};

use windows_sys::Win32::{
    Foundation::{GetLastError, GlobalFree, HWND, LPARAM, LRESULT, WPARAM},
    System::{
        DataExchange::{
            CloseClipboard, GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW,
            SetClipboardData,
        },
        LibraryLoader::GetModuleHandleW,
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
        Threading::GetCurrentThreadId,
    },
    UI::WindowsAndMessaging::{
        CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
        DispatchMessageW, GetMessageW, MSG, PeekMessageW, PostThreadMessageW, RegisterClassW,
        TranslateMessage, UnregisterClassW, WM_CLIPBOARDUPDATE, WM_QUIT, WNDCLASSW,
    },
};

use super::{formats, source_app};

const LISTENER_CLASS: &[u16] = &[
    83, 110, 97, 112, 67, 108, 105, 112, 67, 108, 105, 112, 98, 111, 97, 114, 100, 0,
];
const LISTENER_TITLE: &[u16] = &[83, 110, 97, 112, 67, 108, 105, 112, 0];

/// A clipboard change notification with the window handles resolved *at event time*.
///
/// The owner/foreground snapshot must be taken synchronously: by the time a worker
/// reads the clipboard the user may already have switched windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipboardEvent {
    pub sequence: u32,
    pub owner: usize,
    pub foreground: usize,
}

/// Owns the listener thread. Dropping it posts `WM_QUIT` and joins the thread.
pub struct ClipboardUpdateListener {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl ClipboardUpdateListener {
    /// Spawn the listener and block until its window and message queue exist.
    pub fn start(
        on_event: impl Fn(ClipboardEvent) + Send + 'static,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("snapclip-clipboard-listener".into())
            .spawn(move || listener_thread(on_event, ready_tx))?;
        let thread_id = ready_rx.recv()??;
        Ok(Self {
            thread_id,
            thread: Some(thread),
        })
    }
}

impl Drop for ClipboardUpdateListener {
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
    on_event: impl Fn(ClipboardEvent) + Send + 'static,
    ready: SyncSender<Result<u32, String>>,
) {
    let thread_id = unsafe { GetCurrentThreadId() };
    // Force the thread message queue into existence before publishing the id, so
    // `PostThreadMessageW` cannot race with queue creation.
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

    if unsafe { AddClipboardFormatListener(window) } == 0 {
        unsafe {
            DestroyWindow(window);
            UnregisterClassW(LISTENER_CLASS.as_ptr(), instance);
        }
        let _ = ready.send(Err(last_error("AddClipboardFormatListener")));
        return;
    }

    if ready.send(Ok(thread_id)).is_err() {
        unsafe {
            RemoveClipboardFormatListener(window);
            DestroyWindow(window);
            UnregisterClassW(LISTENER_CLASS.as_ptr(), instance);
        }
        return;
    }

    let mut message: MSG = unsafe { zeroed() };
    loop {
        let result = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
        if result <= 0 {
            break;
        }
        if message.message == WM_CLIPBOARDUPDATE {
            let snapshot = source_app::SourceWindowSnapshot::capture();
            on_event(ClipboardEvent {
                sequence: unsafe { GetClipboardSequenceNumber() },
                owner: snapshot.owner,
                foreground: snapshot.foreground,
            });
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
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// Marks clipboard content written by SnapClip so the monitor does not capture it
/// again. The marker is a private one-byte format; other applications ignore it.
pub fn mark_clipboard_excluded() {
    let _ = formats::formats();
    let format_name = formats::wide_nul(formats::EXCLUDE_FORMAT_NAME);
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
                    // Ownership stays with us when SetClipboardData fails.
                    GlobalFree(handle);
                }
            } else {
                GlobalFree(handle);
            }
        }
        CloseClipboard();
    }
}

/// Current clipboard owner plus foreground window, for callers that need a fresh
/// snapshot outside the listener callback.
#[allow(dead_code)]
pub fn current_source_snapshot() -> source_app::SourceWindowSnapshot {
    source_app::SourceWindowSnapshot::capture()
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





