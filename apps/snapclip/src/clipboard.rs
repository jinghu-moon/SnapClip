//! The shell's clipboard writes.
//!
//! Copying an entry back is a *shell* job, not a history one: whoever writes to the
//! clipboard also has to mark the write as SnapClip's own, or the clipboard monitor would
//! record our copy as a new entry and the history would grow on every copy. That marker
//! lives in `snapclip-history`'s Win32 adapter (`mark_clipboard_excluded`), which is why
//! this is a thin adapter over `arboard` plus that one call — the same shape the Tauri host
//! used in `src-tauri/src/app/clipboard_writer.rs`.

use std::borrow::Cow;

/// Writes to the system clipboard and marks the write as ours.
pub struct SystemClipboard;

impl SystemClipboard {
    /// Copy plain text.
    pub fn copy_text(&self, text: &str) -> Result<(), String> {
        let mut clipboard =
            arboard::Clipboard::new().map_err(|error| format!("clipboard unavailable: {error}"))?;
        clipboard
            .set_text(text.to_string())
            .map_err(|error| format!("copy failed: {error}"))?;
        mark_ours();
        Ok(())
    }

    /// Copy an image, given as tightly packed RGBA rows.
    pub fn copy_image(&self, width: u32, height: u32, rgba: Vec<u8>) -> Result<(), String> {
        let expected = width as usize * height as usize * 4;
        if rgba.len() != expected {
            return Err(format!(
                "image is {} bytes, expected {expected} for {width}x{height}",
                rgba.len()
            ));
        }
        let mut clipboard =
            arboard::Clipboard::new().map_err(|error| format!("clipboard unavailable: {error}"))?;
        clipboard
            .set_image(arboard::ImageData {
                width: width as usize,
                height: height as usize,
                bytes: Cow::Owned(rgba),
            })
            .map_err(|error| format!("copy failed: {error}"))?;
        mark_ours();
        Ok(())
    }
}

/// Keep SnapClip's own copy out of its clipboard history.
fn mark_ours() {
    #[cfg(windows)]
    snapclip_history::windows::mark_clipboard_excluded();
}
