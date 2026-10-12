//! The shell's clipboard writes.
//!
//! Copying an entry back is a *shell* job, not a history one: whoever writes to the
//! clipboard also has to mark the write as SnapClip's own, or the clipboard monitor would
//! record our copy as a new entry and the history would grow on every copy. That marker
//! lives in `snapclip-history`'s Win32 adapter (`mark_clipboard_excluded`).
//!
//! Text goes through `arboard`. Images are written here, directly: `arboard::set_image` encodes a
//! PNG of its own, flips the rows into a second copy and swaps the channels in a third. For a
//! finished scroll capture (2148x42924 in the field) that was a second 1.2 s PNG encode of an image
//! already encoded, and about 1 GB of transient copies. Here the PNG the caller already has goes on
//! the clipboard as it is, and the bitmap is written once, straight into the clipboard's memory.

/// The byte order of a pixel in [`ClipboardImage::pixels`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channels {
    Bgra,
    Rgba,
}

/// An image to copy: top-down rows, 4 bytes a pixel, tightly packed.
#[derive(Debug, Clone, Copy)]
pub struct ClipboardImage<'a> {
    pub width: u32,
    pub height: u32,
    pub pixels: &'a [u8],
    pub channels: Channels,
    /// The same image as a PNG file's bytes, when the caller has one: offered as the clipboard's
    /// `PNG` format (what browsers and Office prefer) without encoding it again.
    pub png: Option<&'a [u8]>,
}

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

    /// Copy an image as a `CF_DIBV5` bitmap, plus its PNG when it has one.
    pub fn copy_image(&self, image: &ClipboardImage<'_>) -> Result<(), String> {
        let expected = image.width as usize * image.height as usize * 4;
        if image.width == 0 || image.height == 0 || image.pixels.len() != expected {
            return Err(format!(
                "image is {} bytes, expected {expected} for {}x{}",
                image.pixels.len(),
                image.width,
                image.height
            ));
        }
        write_image(image)?;
        mark_ours();
        Ok(())
    }
}

#[cfg(windows)]
use win::write_image;

#[cfg(not(windows))]
fn write_image(_image: &ClipboardImage<'_>) -> Result<(), String> {
    Err("image copy is only implemented on Windows".into())
}

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

/// Keep SnapClip's own copy out of its clipboard history.
fn mark_ours() {
    #[cfg(windows)]
    snapclip_history::windows::mark_clipboard_excluded();
}

#[cfg(windows)]
mod win {
    use std::ptr::null_mut;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::Graphics::Gdi::{BI_BITFIELDS, BITMAPV5HEADER, LCS_GM_IMAGES};
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
    };
    use windows_sys::Win32::System::Ole::CF_DIBV5;

    use super::{Channels, ClipboardImage};

    /// Another process may hold the clipboard for a moment (a clipboard manager reading our
    /// previous write, for one); try again for about a second before giving up.
    const OPEN_ATTEMPTS: u32 = 50;
    const OPEN_RETRY: Duration = Duration::from_millis(20);
    const HEADER: usize = size_of::<BITMAPV5HEADER>();
    /// `LCS_sRGB` ('sRGB'), missing from `windows-sys`.
    const LCS_SRGB: u32 = 0x7352_4742;

    pub(super) fn write_image(image: &ClipboardImage<'_>) -> Result<(), String> {
        let dib = dibv5_len(image)?;
        let _open = Open::new()?;
        if unsafe { EmptyClipboard() } == 0 {
            return Err(last_error("EmptyClipboard"));
        }
        set(u32::from(CF_DIBV5), dib, |memory| fill_dibv5(memory, image))?;
        if let Some(png) = image.png {
            let name: Vec<u16> = "PNG\0".encode_utf16().collect();
            let format = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
            if format == 0 {
                return Err(last_error("RegisterClipboardFormatW(PNG)"));
            }
            set(format, png.len(), |memory| memory.copy_from_slice(png))?;
        }
        Ok(())
    }

    /// The `CF_DIBV5` block's size: the header, then the pixels.
    pub(super) fn dibv5_len(image: &ClipboardImage<'_>) -> Result<usize, String> {
        let pixels = image.pixels.len();
        if u32::try_from(pixels).is_err() || i32::try_from(image.height).is_err() {
            return Err(format!(
                "a {}x{} bitmap does not fit a DIB header",
                image.width, image.height
            ));
        }
        Ok(HEADER + pixels)
    }

    /// Write the `CF_DIBV5` block: a `BITMAPV5HEADER`, then the rows **bottom-up** in B, G, R, A
    /// order. A negative height would let the rows stay top-down, but Word and WordPad refuse to
    /// paste such a bitmap (the reason `arboard` flips too), so the flip happens here, in the one
    /// copy that has to be made anyway.
    pub(super) fn fill_dibv5(memory: &mut [u8], image: &ClipboardImage<'_>) {
        let header = BITMAPV5HEADER {
            bV5Size: HEADER as u32,
            bV5Width: image.width as i32,
            bV5Height: image.height as i32,
            bV5Planes: 1,
            bV5BitCount: 32,
            bV5Compression: BI_BITFIELDS,
            bV5SizeImage: image.pixels.len() as u32,
            bV5XPelsPerMeter: 0,
            bV5YPelsPerMeter: 0,
            bV5ClrUsed: 0,
            bV5ClrImportant: 0,
            bV5RedMask: 0x00ff_0000,
            bV5GreenMask: 0x0000_ff00,
            bV5BlueMask: 0x0000_00ff,
            bV5AlphaMask: 0xff00_0000,
            bV5CSType: LCS_SRGB,
            // Ignored unless `bV5CSType` is `LCS_CALIBRATED_RGB`.
            bV5Endpoints: unsafe { std::mem::zeroed() },
            bV5GammaRed: 0,
            bV5GammaGreen: 0,
            bV5GammaBlue: 0,
            bV5Intent: LCS_GM_IMAGES as u32,
            bV5ProfileData: 0,
            bV5ProfileSize: 0,
            bV5Reserved: 0,
        };
        let (head, body) = memory.split_at_mut(HEADER);
        // SAFETY: `BITMAPV5HEADER` is plain `repr(C)` data, `HEADER` bytes long.
        head.copy_from_slice(unsafe {
            std::slice::from_raw_parts((&header as *const BITMAPV5HEADER).cast::<u8>(), HEADER)
        });
        let row = image.width as usize * 4;
        for (to, from) in body
            .chunks_exact_mut(row)
            .zip(image.pixels.chunks_exact(row).rev())
        {
            match image.channels {
                Channels::Bgra => to.copy_from_slice(from),
                Channels::Rgba => {
                    for (to, from) in to
                        .as_chunks_mut::<4>()
                        .0
                        .iter_mut()
                        .zip(from.as_chunks::<4>().0)
                    {
                        *to = [from[2], from[1], from[0], from[3]];
                    }
                }
            }
        }
    }

    /// Hand `len` bytes, filled by `fill`, to the open clipboard as `format`.
    fn set(format: u32, len: usize, fill: impl FnOnce(&mut [u8])) -> Result<(), String> {
        unsafe {
            let handle = GlobalAlloc(GMEM_MOVEABLE, len);
            if handle.is_null() {
                return Err(last_error(&format!("GlobalAlloc({len})")));
            }
            let memory = GlobalLock(handle);
            if memory.is_null() {
                let error = last_error("GlobalLock");
                GlobalFree(handle);
                return Err(error);
            }
            fill(std::slice::from_raw_parts_mut(memory.cast::<u8>(), len));
            GlobalUnlock(handle);
            if SetClipboardData(format, handle).is_null() {
                // The memory is still ours when the clipboard did not take it.
                let error = last_error(&format!("SetClipboardData({format})"));
                GlobalFree(handle);
                return Err(error);
            }
        }
        Ok(())
    }

    /// The clipboard, open for this thread until dropped.
    struct Open;

    impl Open {
        fn new() -> Result<Self, String> {
            for _ in 0..OPEN_ATTEMPTS {
                if unsafe { OpenClipboard(null_mut()) } != 0 {
                    return Ok(Self);
                }
                std::thread::sleep(OPEN_RETRY);
            }
            Err(last_error("OpenClipboard"))
        }
    }

    impl Drop for Open {
        fn drop(&mut self) {
            unsafe { CloseClipboard() };
        }
    }

    fn last_error(operation: &str) -> String {
        format!("{operation} failed: {}", std::io::Error::last_os_error())
    }

    #[cfg(test)]
    mod tests {
        use super::{HEADER, dibv5_len, fill_dibv5};
        use crate::clipboard::{Channels, ClipboardImage};

        /// 2x3 pixels, each one distinct: `[row, column, channel-marker, alpha]`.
        fn pixels(channels: Channels) -> Vec<u8> {
            let mut pixels = Vec::new();
            for row in 0..3u8 {
                for column in 0..2u8 {
                    let (first, third) = (10 * row + column, 100 + 10 * row + column);
                    pixels.extend_from_slice(&match channels {
                        // B, G, R, A
                        Channels::Bgra => [first, 50, third, 200 + row],
                        // R, G, B, A: the same colour
                        Channels::Rgba => [third, 50, first, 200 + row],
                    });
                }
            }
            pixels
        }

        fn dib(channels: Channels) -> Vec<u8> {
            let pixels = pixels(channels);
            let image = ClipboardImage {
                width: 2,
                height: 3,
                pixels: &pixels,
                channels,
                png: None,
            };
            let mut memory = vec![0xEE; dibv5_len(&image).unwrap()];
            fill_dibv5(&mut memory, &image);
            memory
        }

        /// The header says what the pixels are, and the rows are bottom-up, in B, G, R, A.
        #[test]
        fn a_bitmap_is_bottom_up_bgra_behind_a_v5_header() {
            let memory = dib(Channels::Bgra);
            let le = |at: usize| u32::from_le_bytes(memory[at..at + 4].try_into().unwrap());
            assert_eq!(HEADER, 124);
            assert_eq!(le(0), 124, "bV5Size");
            assert_eq!(le(4), 2, "width");
            assert_eq!(le(8), 3, "a positive height: bottom-up rows");
            assert_eq!(le(16), 3, "BI_BITFIELDS");
            assert_eq!(le(20), 24, "bV5SizeImage");
            assert_eq!(
                (le(40), le(44), le(48), le(52)),
                (0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0xff00_0000)
            );
            let source = pixels(Channels::Bgra);
            let body = &memory[HEADER..];
            assert_eq!(body.len(), 24);
            assert_eq!(&body[0..8], &source[16..24], "the bottom row comes first");
            assert_eq!(&body[8..16], &source[8..16]);
            assert_eq!(&body[16..24], &source[0..8], "the top row comes last");
        }

        /// RGBA rows (a history entry's decoded pixels) make the very same bitmap.
        #[test]
        fn rgba_rows_make_the_same_bitmap_as_bgra_rows() {
            assert_eq!(dib(Channels::Rgba), dib(Channels::Bgra));
        }

        /// What the open clipboard holds as `format`, copied out.
        fn read(format: u32) -> Option<Vec<u8>> {
            use windows_sys::Win32::System::DataExchange::GetClipboardData;
            use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
            unsafe {
                let handle = GetClipboardData(format);
                if handle.is_null() {
                    return None;
                }
                let memory = GlobalLock(handle);
                let bytes =
                    std::slice::from_raw_parts(memory.cast::<u8>(), GlobalSize(handle)).to_vec();
                GlobalUnlock(handle);
                Some(bytes)
            }
        }

        /// The real clipboard, written and read back: the bitmap and the PNG arrive byte for byte,
        /// and Windows offers the `CF_DIB` and `CF_BITMAP` it synthesises from a `CF_DIBV5`, which
        /// is what older programs paste. With `SNAPCLIP_CLIPBOARD_PNG` naming a PNG file (the
        /// field's long image), that image is copied instead and the write is timed.
        ///
        /// Ignored: it replaces whatever the user has on the clipboard.
        #[test]
        #[ignore = "L3: overwrites the system clipboard; run with --ignored --nocapture"]
        fn the_system_clipboard_gets_the_bitmap_and_the_png_as_written() {
            use windows_sys::Win32::System::DataExchange::{
                CloseClipboard, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
            };
            use windows_sys::Win32::System::Ole::{CF_BITMAP, CF_DIB, CF_DIBV5};

            let (width, height, pixels, png) = match std::env::var("SNAPCLIP_CLIPBOARD_PNG") {
                Ok(path) => {
                    let png = std::fs::read(path).unwrap();
                    let mut decoder = png::Decoder::new(std::io::Cursor::new(&png[..]));
                    decoder.set_transformations(png::Transformations::EXPAND);
                    let mut reader = decoder.read_info().unwrap();
                    let mut rgba = vec![0; reader.output_buffer_size().unwrap()];
                    let info = reader.next_frame(&mut rgba).unwrap();
                    assert_eq!(info.color_type, png::ColorType::Rgba);
                    for pixel in rgba.as_chunks_mut::<4>().0 {
                        pixel.swap(0, 2);
                    }
                    (info.width, info.height, rgba, png)
                }
                Err(_) => (2, 3, pixels(Channels::Bgra), b"\x89PNG not really".to_vec()),
            };
            let image = ClipboardImage {
                width,
                height,
                pixels: &pixels,
                channels: Channels::Bgra,
                png: Some(&png),
            };
            let started = std::time::Instant::now();
            crate::clipboard::SystemClipboard
                .copy_image(&image)
                .unwrap();
            println!(
                "{width}x{height} clipboard copy ms={}",
                started.elapsed().as_millis()
            );

            let mut expected = vec![0; dibv5_len(&image).unwrap()];
            fill_dibv5(&mut expected, &image);
            let name: Vec<u16> = "PNG\0".encode_utf16().collect();
            let png_format = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
            assert_ne!(unsafe { OpenClipboard(std::ptr::null_mut()) }, 0);
            let dib = read(u32::from(CF_DIBV5));
            let stored_png = read(png_format);
            let synthesised = unsafe {
                (
                    IsClipboardFormatAvailable(u32::from(CF_DIB)),
                    IsClipboardFormatAvailable(u32::from(CF_BITMAP)),
                )
            };
            unsafe { CloseClipboard() };
            // `GlobalSize` may round the block up; what was written is its prefix.
            let dib = dib.expect("CF_DIBV5 on the clipboard");
            assert!(
                dib.starts_with(&expected),
                "the bitmap reads back as written"
            );
            let stored_png = stored_png.expect("PNG on the clipboard");
            assert!(
                stored_png.starts_with(&png),
                "the PNG reads back as written"
            );
            assert!(synthesised.0 != 0 && synthesised.1 != 0, "{synthesised:?}");
        }
    }
}
