//! The capture port adapter; system clipboard ownership lives in snapclip-clipboard.
pub use snapclip_clipboard::{Channels, ClipboardImage, SystemClipboard};

/// The same writer, seen through capture's port.
///
/// The overlay copies the colour under the magnifier (`C`) and a finished scroll capture's long
/// image. Capture declares the need; the shell owns the clipboard because whoever writes
/// to it also has to mark the write as ours — which is what `SystemClipboard` already does.
pub struct SystemClipboardWriter;

impl snapclip_capture::ports::ClipboardWriter for SystemClipboardWriter {
    fn copy_text(&self, text: &str) {
        // Failures are logged, never fatal: a colour the user could not copy must not take
        // the overlay down with it.
        if let Err(error) = SystemClipboard.copy_text(text) {
            eprintln!("[snapclip-app] overlay clipboard write failed: {error}");
        }
    }

    fn copy_image(
        &self,
        width: u32,
        height: u32,
        bgra: &[u8],
        png: Option<&std::path::Path>,
    ) -> Result<(), String> {
        // Without the file the bitmap alone still pastes; the PNG format is the extra.
        let png = png.and_then(|path| {
            std::fs::read(path)
                .inspect_err(|error| {
                    eprintln!(
                        "[snapclip-app] clipboard: {} unreadable, bitmap only: {error}",
                        path.display()
                    )
                })
                .ok()
        });
        SystemClipboard.copy_image(&ClipboardImage {
            width,
            height,
            pixels: bgra,
            channels: Channels::Bgra,
            png: png.as_deref(),
        })
    }
}
