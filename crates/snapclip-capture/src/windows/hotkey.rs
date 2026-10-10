//! Global hotkeys (`F5` capture, `F7` scroll capture) via `RegisterHotKey` / `WM_HOTKEY`.
//!
//! The registration is owned by the overlay UI thread: `WM_HOTKEY` is posted to the
//! queue of the thread that registered it, and that same thread runs the message
//! loop which drains both the hotkey and the overlay's input. Nothing here touches
//! textures, encoding or storage — `WM_HOTKEY` only flips the session state.
//!
//! The keys are rows of [`HOTKEYS`], not one function copied per key: an id, a combination,
//! a user-visible label and the entry point they name travel together, so a third key cannot
//! be added by copying the first and forgetting half of it (docs/32 §4.1). Two things are why
//! that matters here and not somewhere else:
//!
//! * the id is the **only** thing `WM_HOTKEY` carries about which key fired, so the table is
//!   also the dispatch table (`from_id`), and ids must stay distinct per thread;
//! * a conflict is a run-time fact about a *specific* key, so the error carries the label —
//!   "F5 is already registered" is actionable, "a hotkey is already registered" is not.

use windows_sys::Win32::{
    Foundation::{GetLastError, HWND},
    UI::Input::KeyboardAndMouse::{
        MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey, VK_ESCAPE, VK_F5, VK_F7, VK_RETURN,
    },
};

/// Which entry point a hotkey names.
///
/// The dispatch in `overlay::window_host` matches on this, so adding a key means adding a row
/// to [`HOTKEYS`] and one arm to that match — the compiler names the arm that is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyKind {
    /// Take a screenshot.
    Capture,
    /// Start a scroll capture (docs/32 `P7.04` onwards).
    Scroll,
}

/// One global hotkey: identity, combination, and what it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    /// Unique within the registering thread; carried by `WM_HOTKEY.wParam`.
    pub id: i32,
    pub modifiers: u32,
    pub virtual_key: u32,
    /// How this key is named to the user, in logs and in conflict messages.
    pub label: &'static str,
    pub kind: HotkeyKind,
}

/// Identifier for the capture hotkey. Unique within the thread.
pub const CAPTURE_HOTKEY_ID: i32 = 0x5343;

/// Identifier for the scroll-capture hotkey. Unique within the thread, and distinct from
/// [`CAPTURE_HOTKEY_ID`] because the id is all `WM_HOTKEY` says about which key fired.
pub const SCROLL_HOTKEY_ID: i32 = 0x5344;

/// `F5` without modifiers, with repeat suppressed.
pub const CAPTURE_HOTKEY: Hotkey = Hotkey {
    id: CAPTURE_HOTKEY_ID,
    modifiers: MOD_NOREPEAT as u32,
    virtual_key: VK_F5 as u32,
    label: "F5",
    kind: HotkeyKind::Capture,
};

/// `F7` without modifiers, with repeat suppressed (docs/32 §4.1).
pub const SCROLL_HOTKEY: Hotkey = Hotkey {
    id: SCROLL_HOTKEY_ID,
    modifiers: MOD_NOREPEAT as u32,
    virtual_key: VK_F7 as u32,
    label: "F7",
    kind: HotkeyKind::Scroll,
};

/// Every global hotkey this crate registers, in registration order.
pub const HOTKEYS: [Hotkey; 2] = [CAPTURE_HOTKEY, SCROLL_HOTKEY];

/// Esc / Enter are read from `WM_KEYDOWN` rather than registered as global
/// hotkeys: they must only be consumed while the overlay owns the session.
pub const ESCAPE_VIRTUAL_KEY: u32 = VK_ESCAPE as u32;
pub const RETURN_VIRTUAL_KEY: u32 = VK_RETURN as u32;

/// The row `WM_HOTKEY` reported, if this crate registered it.
pub fn from_id(id: i32) -> Option<Hotkey> {
    HOTKEYS.iter().copied().find(|hotkey| hotkey.id == id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyError {
    /// Another application already owns the combination.
    Conflict { label: &'static str },
    /// Registration failed for any other reason.
    Failed { label: &'static str, code: u32 },
}

impl HotkeyError {
    pub fn message(self) -> String {
        match self {
            Self::Conflict { label } => {
                format!("{label} is already registered by another application (Win32 error 1409)")
            }
            Self::Failed { label, code } => {
                format!("RegisterHotKey({label}) failed with Win32 error {code}")
            }
        }
    }

    pub fn is_conflict(self) -> bool {
        matches!(self, Self::Conflict { .. })
    }
}

const ERROR_HOTKEY_ALREADY_REGISTERED: u32 = 1409;

/// Register every row of [`HOTKEYS`] for the calling thread's message queue.
///
/// Stops at the first failure and names that key. A partially registered table is not unwound
/// here: the only caller creates the window that owns these registrations in the same breath,
/// and destroying a window releases its hotkeys.
pub fn register(window: HWND) -> Result<(), HotkeyError> {
    for hotkey in HOTKEYS {
        let result = unsafe {
            RegisterHotKey(window, hotkey.id, hotkey.modifiers, hotkey.virtual_key)
        };
        if result != 0 {
            continue;
        }
        let code = unsafe { GetLastError() };
        return Err(if code == ERROR_HOTKEY_ALREADY_REGISTERED || code == 0 {
            HotkeyError::Conflict {
                label: hotkey.label,
            }
        } else {
            HotkeyError::Failed {
                label: hotkey.label,
                code,
            }
        });
    }
    Ok(())
}

/// Release every registered hotkey. Must be called from the registering thread.
pub fn unregister(window: HWND) {
    for hotkey in HOTKEYS {
        unsafe {
            UnregisterHotKey(window, hotkey.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CAPTURE_HOTKEY, CAPTURE_HOTKEY_ID, ESCAPE_VIRTUAL_KEY, HOTKEYS, Hotkey, HotkeyError,
        HotkeyKind, RETURN_VIRTUAL_KEY, SCROLL_HOTKEY, SCROLL_HOTKEY_ID, from_id,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        MOD_NOREPEAT, VK_ESCAPE, VK_F5, VK_F7, VK_RETURN,
    };

    #[test]
    fn the_second_hotkey_is_registered_with_the_same_discipline() {
        assert_eq!(HOTKEYS.len(), 2);
        assert_eq!(CAPTURE_HOTKEY_ID, 0x5343);
        assert_eq!(
            CAPTURE_HOTKEY,
            Hotkey {
                id: CAPTURE_HOTKEY_ID,
                modifiers: MOD_NOREPEAT,
                virtual_key: VK_F5 as u32,
                label: "F5",
                kind: HotkeyKind::Capture,
            }
        );
        assert_eq!(SCROLL_HOTKEY.id, SCROLL_HOTKEY_ID);
        assert_eq!(SCROLL_HOTKEY.modifiers, MOD_NOREPEAT);
        assert_eq!(SCROLL_HOTKEY.virtual_key, VK_F7 as u32);
        assert_eq!(SCROLL_HOTKEY.kind, HotkeyKind::Scroll);
        assert_eq!(from_id(CAPTURE_HOTKEY_ID), Some(CAPTURE_HOTKEY));
        assert_eq!(from_id(SCROLL_HOTKEY_ID), Some(SCROLL_HOTKEY));
    }

    /// Two rows sharing an id would make `WM_HOTKEY` ambiguous: the id is the only thing the
    /// message carries about which key fired.
    ///
    /// The second assertion is a literal table, the way `overlay/tests.rs:57-77` records the
    /// message ids — a third key added without touching this line fails here.
    #[test]
    fn hotkey_ids_are_distinct_and_pinned_to_a_table() {
        let mut ids: Vec<i32> = HOTKEYS.iter().map(|hotkey| hotkey.id).collect();
        let rows = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), rows, "two hotkey rows share an id");
        assert_eq!(HOTKEYS.map(|hotkey| hotkey.id), [0x5343, 0x5344]);
    }

    #[test]
    fn each_conflict_message_names_its_own_key() {
        for hotkey in HOTKEYS {
            let conflict = HotkeyError::Conflict {
                label: hotkey.label,
            };
            assert!(conflict.is_conflict());
            assert!(
                conflict.message().contains(hotkey.label),
                "{} does not name the key it is about",
                conflict.message()
            );
            assert!(conflict.message().contains("1409"));
            let failed = HotkeyError::Failed {
                label: hotkey.label,
                code: 5,
            };
            assert!(!failed.is_conflict());
            assert!(failed.message().contains(hotkey.label));
            assert!(failed.message().contains("error 5"));
        }
        assert!(
            HotkeyError::Conflict {
                label: SCROLL_HOTKEY.label
            }
            .message()
            .contains("F7")
        );
        assert!(
            HotkeyError::Conflict {
                label: CAPTURE_HOTKEY.label
            }
            .message()
            .contains("F5")
        );
    }

    /// The gate the pump asks before it does anything: an id nobody registered must reach no
    /// entry point (and must not fall through to `DefWindowProcW` either).
    #[test]
    fn an_unknown_hotkey_id_is_swallowed() {
        assert_eq!(from_id(0x9999), None);
        assert_eq!(from_id(0), None);
        assert_eq!(from_id(0x9999).map(|hotkey| hotkey.kind), None);
        for hotkey in HOTKEYS {
            assert!(matches!(
                hotkey.kind,
                HotkeyKind::Capture | HotkeyKind::Scroll
            ));
            assert_eq!(from_id(hotkey.id).map(|row| row.kind), Some(hotkey.kind));
        }
    }

    #[test]
    fn escape_and_enter_are_not_global_hotkeys() {
        assert_eq!(ESCAPE_VIRTUAL_KEY, VK_ESCAPE as u32);
        assert_eq!(RETURN_VIRTUAL_KEY, VK_RETURN as u32);
        assert!(
            !HOTKEYS.iter().any(|hotkey| hotkey.virtual_key == VK_ESCAPE as u32
                || hotkey.virtual_key == VK_RETURN as u32),
            "Esc/Enter must stay session-scoped `WM_KEYDOWN`, not global hotkeys"
        );
    }

    /// L1: the table is what `register` walks, so the registration order is the table order.
    #[test]
    fn the_registration_order_is_the_table_order() {
        assert_eq!(HOTKEYS, [CAPTURE_HOTKEY, SCROLL_HOTKEY]);
    }

    /// L3 (`docs/32` §7, row 79): both keys really register on this desktop, and releasing them
    /// really releases them — a conflict is a run-time fact (`Win32 error 1409`) that no
    /// compile-time assertion can see.
    ///
    /// Scope, stated honestly: this covers the OS half (register ×2, release ×2, re-register).
    /// The other half — a `WM_HOTKEY` arriving with a given id and reaching the right entry
    /// point — is `from_id` at L1 plus the pump's `if let` arm. A *synthesized* key press is
    /// deliberately not used: `SendInput` would deliver F5/F7 to every application on this
    /// desktop, including the harness running the tests. The physical press is covered end to
    /// end by `P7.13` (row 114).
    #[test]
    #[ignore = "registers and releases real global hotkeys on this desktop"]
    fn both_hotkeys_register_and_release_on_a_real_thread() {
        // A null window is the documented "post `WM_HOTKEY` to this thread's queue" form of
        // `RegisterHotKey`, which is the same syscall the overlay makes with its own HWND.
        let thread_window = std::ptr::null_mut();
        super::register(thread_window).unwrap_or_else(|error| {
            panic!(
                "this desktop already owns a key in the table ({}); docs/32 R-31",
                error.message()
            )
        });
        super::unregister(thread_window);
        // Released, so the same ids must be claimable again: without this the test would pass
        // even if `register` had silently done nothing.
        super::register(thread_window).expect("the ids were not released by `unregister`");
        super::unregister(thread_window);
    }
}
