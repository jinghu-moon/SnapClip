//! Global screenshot hotkey (`F5`) via `RegisterHotKey` / `WM_HOTKEY`.
//!
//! The registration is owned by the overlay UI thread: `WM_HOTKEY` is posted to the
//! queue of the thread that registered it, and that same thread runs the message
//! loop which drains both the hotkey and the overlay's input. Nothing here touches
//! textures, encoding or storage — `WM_HOTKEY` only flips the session state.

use windows_sys::Win32::{
    Foundation::{GetLastError, HWND},
    UI::{
        Input::KeyboardAndMouse::{
            MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey, VK_ESCAPE, VK_F5, VK_RETURN,
        },
    },
};

/// Identifier for the capture hotkey. Unique within the thread.
pub const CAPTURE_HOTKEY_ID: i32 = 0x5343;

/// F5 without modifiers, with repeat suppressed.
pub const F5_MODIFIERS: u32 = MOD_NOREPEAT as u32;
pub const F5_VIRTUAL_KEY: u32 = VK_F5 as u32;

/// Esc / Enter are read from `WM_KEYDOWN` rather than registered as global
/// hotkeys: they must only be consumed while the overlay owns the session.
pub const ESCAPE_VIRTUAL_KEY: u32 = VK_ESCAPE as u32;
pub const RETURN_VIRTUAL_KEY: u32 = VK_RETURN as u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyError {
    /// Another application already owns the combination.
    Conflict,
    /// Registration failed for any other reason.
    Failed(u32),
}

impl HotkeyError {
    pub fn message(self) -> String {
        match self {
            Self::Conflict => {
                "F5 is already registered by another application (Win32 error 1409)".to_string()
            }
            Self::Failed(code) => {
                format!("RegisterHotKey(VK_F5) failed with Win32 error {code}")
            }
        }
    }

    pub fn is_conflict(self) -> bool {
        matches!(self, Self::Conflict)
    }
}

const ERROR_HOTKEY_ALREADY_REGISTERED: u32 = 1409;

/// Register `F5` for the calling thread's message queue.
pub fn register_capture_hotkey(window: HWND) -> Result<(), HotkeyError> {
    let result = unsafe { RegisterHotKey(window, CAPTURE_HOTKEY_ID, F5_MODIFIERS, F5_VIRTUAL_KEY) };
    if result != 0 {
        return Ok(());
    }
    let code = unsafe { GetLastError() };
    if code == ERROR_HOTKEY_ALREADY_REGISTERED || code == 0 {
        Err(HotkeyError::Conflict)
    } else {
        Err(HotkeyError::Failed(code))
    }
}

/// Release the hotkey. Must be called from the registering thread.
pub fn unregister_capture_hotkey(window: HWND) {
    unsafe {
        UnregisterHotKey(window, CAPTURE_HOTKEY_ID);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CAPTURE_HOTKEY_ID, ESCAPE_VIRTUAL_KEY, F5_MODIFIERS, F5_VIRTUAL_KEY, HotkeyError,
        RETURN_VIRTUAL_KEY,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        MOD_NOREPEAT, VK_ESCAPE, VK_F5, VK_RETURN,
    };

    #[test]
    fn f5_is_registered_without_modifiers_and_with_repeat_suppressed() {
        assert_eq!(F5_VIRTUAL_KEY, VK_F5 as u32);
        assert_eq!(F5_MODIFIERS, MOD_NOREPEAT);
        assert_eq!(CAPTURE_HOTKEY_ID, 0x5343);
    }

    #[test]
    fn escape_and_enter_are_not_global_hotkeys() {
        assert_eq!(ESCAPE_VIRTUAL_KEY, VK_ESCAPE as u32);
        assert_eq!(RETURN_VIRTUAL_KEY, VK_RETURN as u32);
    }

    #[test]
    fn conflict_errors_are_distinguishable_from_other_failures() {
        let conflict = HotkeyError::Conflict;
        assert!(conflict.is_conflict());
        assert!(conflict.message().contains("1409"));
        let failed = HotkeyError::Failed(5);
        assert!(!failed.is_conflict());
        assert!(failed.message().contains("error 5"));
    }
}





