//! Clipboard format identifiers and helpers.
//!
//! The wide literals are kept as `&[u16]` so registration costs no allocation on
//! the hot path.

use std::sync::OnceLock;

/// `HTML Format` — the standard CF_HTML envelope.
pub const HTML_FORMAT_NAME: &[u16] = &[72, 84, 77, 76, 32, 70, 111, 114, 109, 97, 116, 0];
/// `Rich Text Format` — the standard CF_RTF payload.
pub const RTF_FORMAT_NAME: &[u16] = &[
    82, 105, 99, 104, 32, 84, 101, 120, 116, 32, 70, 111, 114, 109, 97, 116, 0,
];
/// `PNG` — the registered format most modern screenshot tools publish.
pub const PNG_FORMAT_NAME: &[u16] = &[80, 78, 71, 0];

/// SnapClip's own marker format: content carrying it is ignored by the monitor.
pub const EXCLUDE_FORMAT_NAME: &str = "ExcludeClipboardContentFromMonitorProcessing";

/// Maximum total number of bytes accepted from one clipboard snapshot.
pub const MAX_CLIPBOARD_BYTES: usize = 64 * 1024 * 1024;

/// Registered clipboard format ids, resolved once per process.
#[derive(Debug, Clone, Copy)]
pub struct ClipboardFormats {
    pub html: u32,
    pub rtf: u32,
    pub png: u32,
    pub exclude: u32,
}

impl ClipboardFormats {
    pub fn register() -> Self {
        use windows_sys::Win32::System::DataExchange::RegisterClipboardFormatW;

        let exclude_name = wide_nul(EXCLUDE_FORMAT_NAME);
        Self {
            html: unsafe { RegisterClipboardFormatW(HTML_FORMAT_NAME.as_ptr()) },
            rtf: unsafe { RegisterClipboardFormatW(RTF_FORMAT_NAME.as_ptr()) },
            png: unsafe { RegisterClipboardFormatW(PNG_FORMAT_NAME.as_ptr()) },
            exclude: unsafe { RegisterClipboardFormatW(exclude_name.as_ptr()) },
        }
    }
}

/// Process-wide format ids. Registration is idempotent, so caching is safe.
pub fn formats() -> ClipboardFormats {
    static FORMATS: OnceLock<ClipboardFormats> = OnceLock::new();
    *FORMATS.get_or_init(ClipboardFormats::register)
}

pub fn wide_nul(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::{HTML_FORMAT_NAME, PNG_FORMAT_NAME, RTF_FORMAT_NAME, wide_nul};

    #[test]
    fn wide_literals_match_their_names() {
        let decode = |value: &[u16]| {
            let end = value.iter().position(|c| *c == 0).unwrap_or(value.len());
            String::from_utf16_lossy(&value[..end])
        };
        assert_eq!(decode(HTML_FORMAT_NAME), "HTML Format");
        assert_eq!(decode(RTF_FORMAT_NAME), "Rich Text Format");
        assert_eq!(decode(PNG_FORMAT_NAME), "PNG");
    }

    #[test]
    fn wide_nul_appends_a_single_terminator() {
        assert_eq!(wide_nul("ab"), vec![97, 98, 0]);
        assert_eq!(wide_nul(""), vec![0]);
    }
}
