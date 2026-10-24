//! Where the panel points before the user says otherwise.
//!
//! The mockup opens its path field with `C:\Users\阿伟\Desktop` (`:544`). That is one person's
//! desktop, not a default: copying it would make the panel wrong on every other machine. The
//! shell knows where the real one is, so the value comes from there, and an empty string is the
//! honest answer when it will not say — the field then shows the empty value the path
//! validation exists to describe.

use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{FOLDERID_Desktop, SHGetKnownFolderPath};

/// The user's desktop as an absolute path with no trailing separator, or an empty string.
///
/// No trailing separator because every place that joins a file name to this path adds its own,
/// and two separators in a row is the kind of detail that only shows up in someone's file dialog.
pub fn desktop() -> String {
    // SAFETY: `SHGetKnownFolderPath` writes one pointer and returns a failure code we check
    // before reading it. On success the buffer is allocated with `CoTaskMemAlloc`, which is why
    // it is released with `CoTaskMemFree` exactly once, after the text has been copied out.
    unsafe {
        let mut path: *mut u16 = std::ptr::null_mut();
        let result = SHGetKnownFolderPath(
            &FOLDERID_Desktop,
            0,
            std::ptr::null_mut(),
            &mut path,
        );
        if result < 0 || path.is_null() {
            return String::new();
        }
        let mut len = 0usize;
        while *path.add(len) != 0 {
            len += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(path, len));
        CoTaskMemFree(path as *const core::ffi::c_void);
        text
    }
}

#[cfg(test)]
mod tests {
    use super::desktop;

    /// A path the panel can join a name to, or nothing at all.
    ///
    /// This cannot assert a particular directory without binding the test to one machine, so it
    /// asserts the two properties the join depends on: an absolute path, and no trailing
    /// separator. An empty string is allowed and means the shell declined — which is checked as a
    /// fact rather than skipped, because a test that returns early on some inputs decides at run
    /// time that it has nothing to assert (`docs/31` P6.05).
    #[test]
    fn the_desktop_is_absolute_or_empty_and_never_ends_with_a_separator() {
        let path = desktop();
        let declined = path.is_empty();
        assert!(
            declined || path.contains(":\\") || path.starts_with("\\\\"),
            "the desktop must be absolute, or the shell declined and said so by returning nothing; \
             this is {path:?}"
        );
        assert!(
            declined || !path.ends_with('\\'),
            "the desktop must not end with a separator, and this is {path:?}"
        );
        assert!(
            declined || path.len() >= 3,
            "a desktop that is not declined has to be more than a drive letter, and this is {path:?}"
        );
    }
}
