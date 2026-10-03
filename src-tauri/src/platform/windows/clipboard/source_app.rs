//! Clipboard source application detection.
//!
//! Strategy: clipboard owner (accurate for background writers such as screenshot
//! tools) → foreground window at event time (fallback for apps that leave the
//! clipboard owner unset). UWP apps hosted by ApplicationFrameHost are resolved
//! to the real package process.

use std::path::Path;

use windows_sys::Win32::{
    Foundation::{BOOL, CloseHandle, HWND, LPARAM},
    System::{
        DataExchange::GetClipboardOwner,
        Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW},
    },
    UI::WindowsAndMessaging::{EnumChildWindows, GetForegroundWindow, GetWindowThreadProcessId},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceAppInfo {
    pub display_name: String,
    pub exe_path: String,
}

/// HWND snapshot captured when the clipboard notification fires.
#[derive(Debug, Clone, Copy)]
pub struct SourceWindowSnapshot {
    pub owner: usize,
    pub foreground: usize,
}

impl SourceWindowSnapshot {
    pub fn capture() -> Self {
        unsafe {
            Self {
                owner: GetClipboardOwner() as usize,
                foreground: GetForegroundWindow() as usize,
            }
        }
    }
}

/// Resolve the source application from a clipboard-event window snapshot.
pub fn resolve_source(snapshot: SourceWindowSnapshot) -> Option<SourceAppInfo> {
    let self_pid = std::process::id();
    let owner = snapshot.owner as HWND;
    let foreground = snapshot.foreground as HWND;

    if let Some(info) = resolve_from_hwnd(owner, self_pid) {
        return Some(info);
    }
    resolve_from_hwnd(foreground, self_pid)
}

pub fn resolve_from_hwnd(hwnd: HWND, self_pid: u32) -> Option<SourceAppInfo> {
    if hwnd.is_null() {
        return None;
    }
    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == 0 || pid == self_pid {
        return None;
    }
    let exe_path = unsafe { exe_path_from_pid(pid)? };
    let exe_path = unsafe { resolve_uwp_exe(hwnd, &exe_path) }.unwrap_or(exe_path);
    let display_name = display_name_for_exe(&exe_path);
    Some(SourceAppInfo {
        display_name,
        exe_path,
    })
}

unsafe fn exe_path_from_pid(pid: u32) -> Option<String> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return None;
    }
    let mut path = vec![0u16; 32768];
    let mut length = path.len() as u32;
    let success = unsafe { QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length) };
    unsafe { CloseHandle(process) };
    if success == 0 || length == 0 {
        return None;
    }
    String::from_utf16(&path[..length as usize]).ok()
}

/// UWP windows are hosted by ApplicationFrameHost.exe; walk children for the real process.
unsafe fn resolve_uwp_exe(hwnd: HWND, exe_path: &str) -> Option<String> {
    let exe_name = Path::new(exe_path).file_name()?.to_str()?;
    if !exe_name.eq_ignore_ascii_case("ApplicationFrameHost.exe") {
        return None;
    }

    let mut host_pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut host_pid) };

    struct CallbackData {
        host_pid: u32,
        found_path: Option<String>,
    }

    unsafe extern "system" fn enum_callback(child: HWND, lparam: LPARAM) -> BOOL {
        let data = unsafe { &mut *(lparam as *mut CallbackData) };
        let mut child_pid: u32 = 0;
        unsafe { GetWindowThreadProcessId(child, &mut child_pid) };
        if child_pid != 0 && child_pid != data.host_pid {
            if let Some(path) = unsafe { exe_path_from_pid(child_pid) } {
                let name = Path::new(&path)
                    .file_name()
                    .and_then(|value| value.to_str());
                if !name.is_some_and(|value| value.eq_ignore_ascii_case("ApplicationFrameHost.exe"))
                {
                    data.found_path = Some(path);
                    return 0;
                }
            }
        }
        1
    }

    let mut data = CallbackData {
        host_pid,
        found_path: None,
    };
    unsafe {
        EnumChildWindows(
            hwnd,
            Some(enum_callback),
            &mut data as *mut CallbackData as LPARAM,
        )
    };
    data.found_path
}

/// Prefer the exe FileDescription; fall back to the file stem.
pub fn display_name_for_exe(exe_path: &str) -> String {
    file_description(exe_path)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback_display_name(exe_path))
}

pub fn fallback_display_name(exe_path: &str) -> String {
    Path::new(exe_path)
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("Unknown")
        .to_string()
}

fn file_description(exe_path: &str) -> Option<String> {
    use std::ffi::c_void;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };

    unsafe fn query_string(block: *const c_void, sub_block: &str) -> Option<String> {
        let wide: Vec<u16> = sub_block.encode_utf16().chain(std::iter::once(0)).collect();
        let mut buffer: *mut c_void = std::ptr::null_mut();
        let mut length: u32 = 0;
        let ok = unsafe { VerQueryValueW(block, wide.as_ptr(), &mut buffer, &mut length) };
        if ok == 0 || buffer.is_null() || length == 0 {
            return None;
        }
        let slice = unsafe { std::slice::from_raw_parts(buffer as *const u16, length as usize) };
        let end = slice.iter().position(|&c| c == 0).unwrap_or(slice.len());
        let value = String::from_utf16_lossy(&slice[..end]);
        if value.trim().is_empty() {
            None
        } else {
            Some(value)
        }
    }

    unsafe {
        let wide_path: Vec<u16> = exe_path.encode_utf16().chain(std::iter::once(0)).collect();
        let size = GetFileVersionInfoSizeW(wide_path.as_ptr(), std::ptr::null_mut());
        if size == 0 {
            return None;
        }
        let mut block = vec![0u8; size as usize];
        let ok = GetFileVersionInfoW(
            wide_path.as_ptr(),
            0,
            size,
            block.as_mut_ptr() as *mut c_void,
        );
        if ok == 0 {
            return None;
        }
        let block_ptr = block.as_ptr() as *const c_void;

        let trans_path: Vec<u16> = "\\VarFileInfo\\Translation\0".encode_utf16().collect();
        let mut trans_ptr: *mut c_void = std::ptr::null_mut();
        let mut trans_len: u32 = 0;
        if VerQueryValueW(
            block_ptr,
            trans_path.as_ptr(),
            &mut trans_ptr,
            &mut trans_len,
        ) != 0
            && !trans_ptr.is_null()
            && trans_len >= 4
        {
            let lang = *(trans_ptr as *const u16);
            let codepage = *((trans_ptr as *const u16).add(1));
            let sub = format!("\\StringFileInfo\\{lang:04x}{codepage:04x}\\FileDescription\0");
            if let Some(value) = query_string(block_ptr, &sub) {
                return Some(value);
            }
        }

        query_string(block_ptr, "\\StringFileInfo\\040904B0\\FileDescription\0")
    }
}

#[cfg(test)]
mod tests {
    use super::{SourceAppInfo, SourceWindowSnapshot, fallback_display_name};

    #[test]
    fn fallback_display_name_uses_file_stem() {
        assert_eq!(fallback_display_name(r"C:\Apps\PixPin.exe"), "PixPin");
        assert_eq!(fallback_display_name(r"C:\Apps\code.exe"), "code");
        assert_eq!(fallback_display_name(r"C:\"), "Unknown");
        assert_eq!(fallback_display_name(""), "Unknown");
    }

    #[test]
    fn snapshot_capture_does_not_panic() {
        let snapshot = SourceWindowSnapshot::capture();
        let _ = SourceAppInfo {
            display_name: "x".into(),
            exe_path: "y".into(),
        };
        // Values are environment-dependent; just ensure capture returns.
        let _ = snapshot;
    }

    #[test]
    fn display_name_prefers_file_description() {
        use super::display_name_for_exe;
        let path = r"C:\Windows\System32\notepad.exe";
        if !std::path::Path::new(path).exists() {
            return;
        }
        let name = display_name_for_exe(path);
        assert!(!name.is_empty());
        assert_ne!(name, "Unknown");
    }
}
