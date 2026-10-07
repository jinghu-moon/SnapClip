//! The notification-area icon (docs/23 T4.6).
//!
//! There is no tray in this project before this file, and no UI toolkit in the stack provides
//! one (`gpui-kit` 0.7.1 has no tray component — checked before writing this), so this is the
//! real Win32 thing: a hidden window of our own, a message loop on its own thread, and
//! `Shell_NotifyIconW`.
//!
//! Two rules shape it:
//!
//! * **The message loop owns the icon.** `Shell_NotifyIcon` needs a window that pumps
//!   messages, and the menu has to be tracked on that same thread; tracking it on GPUI's
//!   thread would block rendering while the user hovers a menu.
//! * **It reaches the shell through a channel.** The thread's only output is a
//!   [`TrayCommand`]; nothing here knows what a window or a `gpui` entity is, and the capture
//!   hot path cannot be delayed by tray work because it never runs on this thread.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Shell::{
    ExtractIconExW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GWLP_USERDATA, GetCursorPos, GetMessageW, HICON, IDI_APPLICATION, LoadIconW,
    MF_STRING, MSG, PostQuitMessage, PostThreadMessageW, RegisterClassW, SetForegroundWindow,
    SetWindowLongPtrW, TPM_NONOTIFY, TPM_RETURNCMD, TrackPopupMenuEx, TranslateMessage, WM_APP,
    WM_DESTROY, WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSW, WS_POPUP,
};

/// Menu item identity. `TrackPopupMenuEx` answers with these, and nothing else.
const MENU_TOGGLE: u32 = 1;
const MENU_QUIT: u32 = 2;
/// The icon's callback message, and the message that ends the loop.
const TRAY_CALLBACK_MESSAGE: u32 = WM_APP + 1;
const WM_STOP_TRAY: u32 = WM_APP + 2;
const TRAY_ICON_ID: u32 = 1;

/// What the tray asks the shell to do. Deliberately two verbs, not a menu model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    /// Put the window away if it is in front, bring it back if it is not.
    ToggleWindow,
    /// Leave the application.
    Quit,
}

/// Menu item id → command. Split out so the mapping is testable without a message loop.
fn command_for_menu_id(id: u32) -> Option<TrayCommand> {
    match id {
        MENU_TOGGLE => Some(TrayCommand::ToggleWindow),
        MENU_QUIT => Some(TrayCommand::Quit),
        _ => None,
    }
}

/// What the window procedure needs to reach the shell.
struct WndContext {
    commands: async_channel::Sender<TrayCommand>,
}

/// A running tray icon and the commands it produces.
pub struct Tray {
    commands: async_channel::Receiver<TrayCommand>,
    thread_id: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Tray {
    /// Create the icon. Fails when Windows refuses the window or the icon slot.
    pub fn start() -> Result<Self, String> {
        let (commands_tx, commands_rx) = async_channel::unbounded();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_id = Arc::new(AtomicU32::new(0));
        let thread = {
            let stop = Arc::clone(&stop);
            let thread_id = Arc::clone(&thread_id);
            thread::Builder::new()
                .name("snapclip-tray".into())
                .spawn(move || run_message_loop(commands_tx, ready_tx, thread_id, stop))
                .map_err(|error| format!("could not start the tray thread: {error}"))?
        };
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                commands: commands_rx,
                thread_id,
                stop,
                thread: Some(thread),
            }),
            Ok(Err(message)) => {
                let _ = thread.join();
                Err(message)
            }
            Err(_) => {
                let _ = thread.join();
                Err("the tray thread stopped before it reported readiness".into())
            }
        }
    }

    /// The commands the icon produced; the shell drains this on its own thread.
    pub fn commands(&self) -> async_channel::Receiver<TrayCommand> {
        self.commands.clone()
    }

    /// Remove the icon and join the message loop. Safe to call more than once.
    pub fn shutdown(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.stop.store(true, Ordering::SeqCst);
            let id = self.thread_id.load(Ordering::SeqCst);
            if id != 0 {
                // SAFETY: a thread message takes no pointers and may be posted from any thread.
                unsafe {
                    PostThreadMessageW(id, WM_STOP_TRAY, 0, 0);
                }
            }
            let _ = thread.join();
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The window procedure for the tray's hidden window.
extern "system" fn tray_wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_DESTROY => {
            // The box was put here by `run_message_loop`; free it exactly once.
            let context = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *mut WndContext;
            if !context.is_null() {
                // SAFETY: this pointer came from `Box::into_raw` and is freed only here.
                drop(unsafe { Box::from_raw(context) });
            }
            unsafe { PostQuitMessage(0) };
            0
        }
        TRAY_CALLBACK_MESSAGE => {
            // The low word of lparam is the mouse message the user caused.
            let event = (lparam as u32) & 0xffff;
            if event == WM_RBUTTONUP || event == WM_LBUTTONUP {
                let context =
                    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *const WndContext;
                if !context.is_null() {
                    // SAFETY: put straight back, and the context lives until WM_DESTROY.
                    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, context as isize) };
                    if let Some(command) = unsafe { show_menu(hwnd) } {
                        let _ = unsafe { &*context }.commands.try_send(command);
                    }
                }
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

/// Track the popup menu and report the chosen item.
unsafe fn show_menu(hwnd: HWND) -> Option<TrayCommand> {
    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() {
        return None;
    }
    for (id, label) in [
        (MENU_TOGGLE, wide("显示 / 隐藏窗口")),
        (MENU_QUIT, wide("退出 SnapClip")),
    ] {
        // SAFETY: `menu` is alive for this function; the label lives until the call returns.
        unsafe {
            AppendMenuW(menu, MF_STRING, id as usize, label.as_ptr());
        }
    }
    let mut cursor = POINT { x: 0, y: 0 };
    // SAFETY: `cursor` is a valid out-pointer; the window is ours.
    unsafe {
        GetCursorPos(&mut cursor);
        // Without foreground the menu would not dismiss when the user clicks elsewhere.
        SetForegroundWindow(hwnd);
    }
    // `TPM_RETURNCMD` makes the return value the chosen id rather than a success flag, which is
    // why this reads a command instead of a BOOL. 0 means "dismissed", and maps to no command.
    // SAFETY: `menu` is valid, `hwnd` owns the tracking, and no RECT is passed.
    let chosen = unsafe {
        TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY,
            cursor.x,
            cursor.y,
            hwnd,
            std::ptr::null(),
        )
    };
    // SAFETY: the menu is ours and is no longer being tracked.
    unsafe {
        DestroyMenu(menu);
    }
    command_for_menu_id(chosen as u32)
}

/// Create the hidden window, add the icon, and pump messages until asked to stop.
fn run_message_loop(
    commands: async_channel::Sender<TrayCommand>,
    ready: mpsc::SyncSender<Result<(), String>>,
    thread_id: Arc<AtomicU32>,
    _stop: Arc<AtomicBool>,
) {
    // SAFETY: every call below is a documented Win32 entry point used with valid arguments.
    // The class is registered once per thread; re-registering an existing class is a benign
    // failure that is deliberately ignored.
    let hwnd = unsafe {
        let class_name = wide("SnapClipTrayWindow");
        let window_name = wide("SnapClip");
        let instance = GetModuleHandleW(std::ptr::null());
        let class = WNDCLASSW {
            lpfnWndProc: Some(tray_wndproc),
            hInstance: instance,
            lpszClassName: class_name.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&class);
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            window_name.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        let _ = ready.send(Err("could not create the tray's window".into()));
        return;
    }
    // SAFETY: the pointer is handed to the window and freed in `WM_DESTROY`.
    let context = Box::into_raw(Box::new(WndContext { commands }));
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, context as isize);
    }

    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = TRAY_ICON_ID;
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = TRAY_CALLBACK_MESSAGE;
    // SAFETY: the icon is extracted from this executable, or a stock one is used.
    data.hIcon = unsafe { tray_icon() };
    write_wide_buffer(&mut data.szTip, "SnapClip");
    // SAFETY: `data` is a fully initialised `NOTIFYICONDATAW` for `NIM_ADD`.
    if unsafe { Shell_NotifyIconW(NIM_ADD, &data) } == 0 {
        // No icon slot: undo the window rather than leaving an invisible one behind.
        unsafe {
            DestroyWindow(hwnd);
        }
        let _ = ready.send(Err("Windows refused the notification-area icon".into()));
        return;
    }

    // Publish the thread id *before* readiness, so `shutdown` can never find a zero id.
    // SAFETY: asking for the current thread id takes no arguments.
    thread_id.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
    let _ = ready.send(Ok(()));

    let mut message: MSG = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: the out-pointer is valid and every window on this thread is ours.
        let result = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
        if result <= 0 {
            // 0 is WM_QUIT, -1 is an error; either way the loop is over.
            break;
        }
        if message.message == WM_STOP_TRAY {
            break;
        }
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    unsafe {
        Shell_NotifyIconW(NIM_DELETE, &data);
        // The window owns the context box, so destroying it is also what frees the sender.
        DestroyWindow(hwnd);
    }
}

/// The application's own icon, falling back to the generic one.
unsafe fn tray_icon() -> HICON {
    if let Ok(exe) = std::env::current_exe() {
        let path = wide(&exe.to_string_lossy());
        let mut large: HICON = std::ptr::null_mut();
        let mut small: HICON = std::ptr::null_mut();
        // SAFETY: `path` is NUL-terminated and both out-pointers are valid.
        let extracted = unsafe { ExtractIconExW(path.as_ptr(), 0, &mut large, &mut small, 1) };
        if extracted > 0 && !small.is_null() {
            return small;
        }
    }
    // SAFETY: a stock icon takes no pointers; a null instance means "system stock".
    unsafe { LoadIconW(std::ptr::null_mut::<std::ffi::c_void>() as HINSTANCE, IDI_APPLICATION) }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn write_wide_buffer(buffer: &mut [u16], value: &str) {
    let encoded: Vec<u16> = value
        .encode_utf16()
        .take(buffer.len().saturating_sub(1))
        .collect();
    buffer[..encoded.len()].copy_from_slice(&encoded);
    buffer[encoded.len()] = 0;
}

#[cfg(test)]
mod tests {
    use super::{TrayCommand, command_for_menu_id, wide, write_wide_buffer};

    #[test]
    fn the_menu_ids_are_the_only_way_a_command_is_made() {
        assert_eq!(command_for_menu_id(1), Some(TrayCommand::ToggleWindow));
        assert_eq!(command_for_menu_id(2), Some(TrayCommand::Quit));
        // TrackPopupMenuEx answers 0 when the menu is dismissed; that is not a command.
        assert_eq!(command_for_menu_id(0), None);
        assert_eq!(command_for_menu_id(99), None);
    }

    #[test]
    fn the_tooltip_buffer_is_always_terminated() {
        let mut buffer = [0xffffu16; 8];
        write_wide_buffer(&mut buffer, "SnapClip");
        assert_eq!(buffer[0], 'S' as u16);
        assert_eq!(buffer[7], 0, "the last slot stays the terminator");

        // A value longer than the buffer is truncated, not overflowed.
        let mut small = [0xffffu16; 4];
        write_wide_buffer(&mut small, "a very long tooltip");
        assert_eq!(small[3], 0);
        assert_eq!(small[..3], ['a' as u16, ' ' as u16, 'v' as u16]);
    }

    #[test]
    fn wide_strings_are_nul_terminated_utf16() {
        assert_eq!(wide("A中"), vec![0x41, 0x4e2d, 0]);
    }

    /// A real window, a real icon slot, a real thread — and a real shutdown.
    ///
    /// Ignored by default because it puts an icon in the notification area while it runs; run
    /// it with `cargo test -p snapclip-app --lib -- --ignored` on a desktop session. It is
    /// here because "the tray exists" cannot be proven by the mapping tests above.
    #[test]
    #[ignore = "shows a real tray icon; run on a desktop session"]
    fn the_icon_can_be_created_and_taken_away() {
        let mut tray = super::Tray::start().expect("the tray should start on a desktop session");
        assert!(tray.thread_id.load(std::sync::atomic::Ordering::SeqCst) != 0);
        // Two shutdowns: the second must be a no-op rather than a double join or double
        // `NIM_DELETE`.
        tray.shutdown();
        tray.shutdown();
    }
}
