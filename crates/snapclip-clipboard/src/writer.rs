//! System clipboard writes shared by capture, history copy and pin commands.
//! Marker and payload are published under one lock. Fully rendered data has its own
//! message-only owner window; no clipboard-owned handle escapes this module.

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
        #[cfg(windows)]
        {
            win::write_text(text, &[])
        }
        #[cfg(not(windows))]
        {
            let mut clipboard = arboard::Clipboard::new()
                .map_err(|error| format!("clipboard unavailable: {error}"))?;
            clipboard
                .set_text(text.to_string())
                .map_err(|error| format!("copy failed: {error}"))?;
            Ok(())
        }
    }

    /// Copy an image as a `CF_DIBV5` bitmap, plus its PNG when it has one.
    pub fn copy_image(&self, image: &ClipboardImage<'_>) -> Result<(), String> {
        let expected = usize::try_from(image.width)
            .ok()
            .and_then(|w| w.checked_mul(image.height as usize))
            .and_then(|n| n.checked_mul(4))
            .ok_or("图片尺寸溢出")?;
        if image.width == 0 || image.height == 0 || image.pixels.len() != expected {
            return Err(format!(
                "image is {} bytes, expected {expected} for {}x{}",
                image.pixels.len(),
                image.width,
                image.height
            ));
        }
        write_image(image)?;
        Ok(())
    }

    pub fn copy_files(&self, paths: &[String]) -> Result<(), String> {
        #[cfg(windows)]
        {
            win::write_files(paths)
        }
        #[cfg(not(windows))]
        {
            let _ = paths;
            Err("file copy is only implemented on Windows".into())
        }
    }
    pub fn copy_pin(&self, pin: &snapclip_model::pin::PinSnapshot) -> Result<(), String> {
        use snapclip_model::pin::PinContent;
        match pin.content() {
            PinContent::Image(image) => self.copy_image(&ClipboardImage {
                width: image.width(),
                height: image.height(),
                pixels: image.rgba(),
                channels: Channels::Rgba,
                png: None,
            }),
            PinContent::Files(files) => self.copy_files(
                &files
                    .iter()
                    .map(|file| file.path().to_string())
                    .collect::<Vec<_>>(),
            ),
            PinContent::Document(doc) => {
                #[cfg(windows)]
                {
                    win::write_text(doc.text(), pin.sources())
                }
                #[cfg(not(windows))]
                {
                    self.copy_text(doc.text())
                }
            }
        }
    }
}

#[cfg(windows)]
use win::write_image;

#[cfg(not(windows))]
fn write_image(_image: &ClipboardImage<'_>) -> Result<(), String> {
    Err("image copy is only implemented on Windows".into())
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

    fn mark_open() -> Result<(), String> {
        let format = crate::windows::formats::formats().exclude;
        if format == 0 {
            return Err("clipboard marker registration failed".into());
        }
        set(format, 4, |bytes| {
            bytes.copy_from_slice(&1u32.to_le_bytes())
        })
    }
    pub(super) fn write_text(
        text: &str,
        sources: &[(String, std::sync::Arc<[u8]>)],
    ) -> Result<(), String> {
        if text.contains('\0') {
            return Err("文本含有无法复制的 NUL 字符".into());
        }
        let _open = Open::new()?;
        if unsafe { EmptyClipboard() } == 0 {
            return Err(last_error("EmptyClipboard"));
        }
        mark_open()?;
        let wide: Vec<u8> = text
            .encode_utf16()
            .chain(Some(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        set(
            u32::from(windows_sys::Win32::System::Ole::CF_UNICODETEXT),
            wide.len(),
            |out| out.copy_from_slice(&wide),
        )?;
        for (name, source) in sources {
            let format = match name.as_str() {
                "HTML" => crate::windows::formats::formats().html,
                "RTF" => crate::windows::formats::formats().rtf,
                _ => continue,
            };
            if format != 0 {
                set(format, source.len() + 1, |out| {
                    out[..source.len()].copy_from_slice(source);
                    out[source.len()] = 0;
                })?;
            }
        }
        Ok(())
    }
    fn file_drop_bytes(paths: &[String]) -> Result<Vec<u8>, String> {
        if paths.is_empty()
            || paths.len() > crate::MAX_FILES
            || paths.iter().any(|path| {
                path.is_empty()
                    || path.contains('\0')
                    || !std::path::Path::new(path).is_absolute()
                    || path.encode_utf16().count() > 32767
            })
        {
            return Err("文件引用无效".into());
        }
        let units = paths
            .iter()
            .try_fold(1usize, |total, path| {
                total.checked_add(path.encode_utf16().count() + 1)
            })
            .ok_or("文件引用过大")?;
        let size = units
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(20))
            .filter(|size| *size <= crate::MAX_BYTES)
            .ok_or("文件引用过大")?;
        let names: Vec<u8> = paths
            .iter()
            .flat_map(|path| path.encode_utf16().chain(Some(0)))
            .chain(Some(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        let mut bytes = vec![0; size];
        bytes[..4].copy_from_slice(&20u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
        bytes[20..].copy_from_slice(&names);
        Ok(bytes)
    }
    pub(super) fn write_files(paths: &[String]) -> Result<(), String> {
        let bytes = file_drop_bytes(paths)?;
        let _open = Open::new()?;
        if unsafe { EmptyClipboard() } == 0 {
            return Err(last_error("EmptyClipboard"));
        }
        mark_open()?;
        set(
            u32::from(windows_sys::Win32::System::Ole::CF_HDROP),
            bytes.len(),
            |out| out.copy_from_slice(&bytes),
        )
    }

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
        // Marker and payload share one lock; the monitor cannot observe an unmarked intermediate write.
        if unsafe { EmptyClipboard() } == 0 {
            return Err(last_error("EmptyClipboard"));
        }
        mark_open()?;
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
        if u32::try_from(pixels).is_err()
            || i32::try_from(image.height).is_err()
            || i32::try_from(image.width).is_err()
        {
            return Err(format!(
                "a {}x{} bitmap does not fit a DIB header",
                image.width, image.height
            ));
        }
        HEADER
            .checked_add(pixels)
            .ok_or_else(|| "DIB size overflow".into())
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
    struct Owner(windows_sys::Win32::Foundation::HWND);
    impl Owner {
        fn new() -> Result<Self, String> {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            let hwnd = unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::CreateWindowExW(
                    0,
                    class.as_ptr(),
                    std::ptr::null(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    windows_sys::Win32::UI::WindowsAndMessaging::HWND_MESSAGE,
                    null_mut(),
                    windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()),
                    null_mut(),
                )
            };
            if hwnd.is_null() {
                Err(last_error("clipboard owner window"))
            } else {
                Ok(Self(hwnd))
            }
        }
    }
    impl Drop for Owner {
        fn drop(&mut self) {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.0);
            }
        }
    }
    struct Open {
        _owner: Owner,
    }

    impl Open {
        fn new() -> Result<Self, String> {
            let owner = Owner::new()?;
            for _ in 0..OPEN_ATTEMPTS {
                if unsafe { OpenClipboard(owner.0) } != 0 {
                    return Ok(Self { _owner: owner });
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
        use crate::writer::{Channels, ClipboardImage};

        #[test]
        fn file_references_encode_a_wide_double_terminated_dropfiles_block() {
            let paths = vec![
                r"C:\工作\示例😀.txt".to_string(),
                format!(r"C:\{}\file.txt", "a".repeat(9000)),
            ];
            let bytes = super::file_drop_bytes(&paths).unwrap();
            assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 20);
            assert_eq!(&bytes[4..16], &[0; 12]);
            assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 1);
            let actual = bytes[20..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect::<Vec<_>>();
            let expected = paths
                .iter()
                .flat_map(|path| path.encode_utf16().chain(Some(0)))
                .chain(Some(0))
                .collect::<Vec<_>>();
            assert_eq!(actual, expected);
            for bad in [
                vec![],
                vec![String::new()],
                vec!["relative.txt".into()],
                vec!["C:\\bad\0path".into()],
                vec![format!("C:\\{}", "a".repeat(32768))],
            ] {
                assert!(super::file_drop_bytes(&bad).is_err());
            }
        }

        #[test]
        #[ignore = "desktop-only: replaces the real clipboard with synthetic file references"]
        fn file_reference_write_roundtrips_as_files_and_keeps_the_self_marker() {
            let paths = vec![
                r"C:\SnapClip-fixture\中文😀.txt".to_string(),
                r"C:\SnapClip-fixture\folder".to_string(),
            ];
            super::write_files(&paths).unwrap();
            let snapshot =
                crate::windows::reader::read_snapshot(crate::ReadPurpose::ExplicitPin, None)
                    .unwrap();
            assert!(snapshot.is_excluded());
            assert_eq!(snapshot.files(), paths);
        }

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
            crate::writer::SystemClipboard.copy_image(&image).unwrap();
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
