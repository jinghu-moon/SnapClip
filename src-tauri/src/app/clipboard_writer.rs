//! The shell's implementation of the capture crate's `ClipboardWriter` port.
//!
//! The overlay can copy the sampled colour with `C`, but it must not own clipboard
//! concerns: whoever writes to the clipboard also has to mark the write so SnapClip's
//! own clip monitor ignores it, and that marker lives in the clipboard platform module
//! (`platform::windows::clipboard::mark_clipboard_excluded`). So capture declares the
//! need (`snapclip_capture::ports::ClipboardWriter`) and this adapter satisfies it.

use snapclip_capture::ports::ClipboardWriter;

/// Copies text with `arboard` and marks the write as SnapClip's own.
pub struct SystemClipboardWriter;

impl ClipboardWriter for SystemClipboardWriter {
    fn copy_text(&self, text: &str) {
        match arboard::Clipboard::new() {
            Ok(mut clipboard) => {
                if let Err(error) = clipboard.set_text(text.to_string()) {
                    eprintln!("[snapclip][capture] colour copy failed: {error}");
                } else {
                    crate::platform::windows::clipboard::mark_clipboard_excluded();
                }
            }
            Err(error) => eprintln!("[snapclip][capture] clipboard unavailable: {error}"),
        }
    }
}
