//! The only GPUI/Win32 boundary for pin chrome, placement and explicit file operations.
use gpui_kit::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::{
    Foundation::{HWND, POINT},
    Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint},
    UI::{
        HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
        WindowsAndMessaging::*,
    },
};

pub(crate) fn hwnd(window: &Window) -> Option<HWND> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
        _ => None,
    }
}
pub(crate) fn cursor_position() -> (i32, i32) {
    let mut cursor = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut cursor);
    }
    (cursor.x, cursor.y)
}
pub(crate) fn start_move(window: &mut Window) {
    // A headless window has no native surface to move. Keep the pointer routing
    // contract testable without invoking the test backend's unsupported syscall.
    if hwnd(window).is_some() {
        window.start_window_move();
    }
}
pub(crate) fn work_area(point: (i32, i32)) -> ((i32, i32, i32, i32), f32) {
    unsafe {
        let monitor = MonitorFromPoint(
            POINT {
                x: point.0,
                y: point.1,
            },
            MONITOR_DEFAULTTONEAREST,
        );
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let mut x = 96;
        let mut y = 96;
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y);
        if GetMonitorInfoW(monitor, &mut info) != 0 {
            let r = info.rcWork;
            ((r.left, r.top, r.right, r.bottom), x as f32 / 96.)
        } else {
            ((0, 0, 1920, 1080), 1.)
        }
    }
}
pub(crate) fn clamp_position(
    cursor: (i32, i32),
    size: (i32, i32),
    area: (i32, i32, i32, i32),
) -> (i32, i32) {
    let (l, t, r, b) = area;
    let (w, h) = size;
    let x = if cursor.0.saturating_add(16).saturating_add(w) <= r {
        cursor.0.saturating_add(16)
    } else {
        cursor.0.saturating_sub(w).saturating_sub(16)
    };
    let y = if cursor.1.saturating_add(16).saturating_add(h) <= b {
        cursor.1.saturating_add(16)
    } else {
        cursor.1.saturating_sub(h).saturating_sub(16)
    };
    (
        x.clamp(l, r.saturating_sub(w).max(l)),
        y.clamp(t, b.saturating_sub(h).max(t)),
    )
}
pub(crate) fn configure(window: &Window, cursor: (i32, i32), logical: (f32, f32), topmost: bool) {
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    let (area, scale) = work_area(cursor);
    let w = (logical.0 * scale)
        .round()
        .min((area.2 - area.0) as f32)
        .max(1.) as i32;
    let h = (logical.1 * scale)
        .round()
        .min((area.3 - area.1) as f32)
        .max(1.) as i32;
    let position = clamp_position(cursor, (w, h), area);
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            (ex | WS_EX_TOOLWINDOW as isize) & !(WS_EX_APPWINDOW as isize),
        );
        SetWindowPos(
            hwnd,
            if topmost {
                HWND_TOPMOST
            } else {
                HWND_NOTOPMOST
            },
            position.0,
            position.1,
            w,
            h,
            SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_FRAMECHANGED,
        );
        let corner: u32 = 2;
        windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            33,
            (&corner as *const u32).cast(),
            4,
        );
    }
}
pub(crate) fn topmost(window: &Window, on: bool) {
    if let Some(hwnd) = hwnd(window) {
        unsafe {
            SetWindowPos(
                hwnd,
                if on { HWND_TOPMOST } else { HWND_NOTOPMOST },
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            );
        }
    }
}
pub(crate) fn visible(window: &Window, on: bool) {
    if let Some(hwnd) = hwnd(window) {
        unsafe {
            ShowWindow(hwnd, if on { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
    }
}
pub(crate) fn raise(window: &Window) {
    if let Some(hwnd) = hwnd(window) {
        unsafe {
            SetWindowPos(
                hwnd,
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            );
        }
    }
}
pub(crate) fn open(path: &str, reveal: bool) -> Result<(), String> {
    let wide = |text: &str| text.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let verb = wide("open");
    let (file, args) = if reveal {
        (wide("explorer.exe"), wide(&format!("/select,\"{}\"", path)))
    } else {
        (wide(path), vec![0])
    };
    let result = unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            args.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if result > 32 {
        Ok(())
    } else {
        Err("无法打开该项目，请检查文件或系统关联程序".into())
    }
}

#[cfg(test)]
mod tests {
    use super::clamp_position;
    #[test]
    fn placement_flips_and_clamps_on_negative_monitors() {
        assert_eq!(
            clamp_position((-20, -20), (400, 300), (-1920, -1080, 0, 0)),
            (-436, -336)
        );
        assert_eq!(
            clamp_position((-1919, -1079), (400, 300), (-1920, -1080, 0, 0)),
            (-1903, -1063)
        );
        assert_eq!(
            clamp_position((10, 10), (4000, 3000), (-1920, -1080, 0, 0)),
            (-1920, -1080)
        );
    }
}
