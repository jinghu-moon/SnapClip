//! Rich text for a card: RTF today, Markdown next (docs/23, card redesign).
//!
//! The card shows **three lines of inline-level text**: bold, italic, underline and colour.
//! Block-level structure is deliberately not rendered here — see the note at the bottom of
//! this module — because three lines of a list row cannot carry heading sizes or code-block
//! backgrounds without becoming unreadable. The full, block-aware rendering belongs to the
//! popup that shows the whole clip, which is a later feature.
//!
//! `rclip-rtf` was picked for one specific reason: it models the two things that break on
//! Chinese content — `\ucN` fallback skipping and surrogate pairs — and its `Document::text`
//! already turns `\par` / `\line` into newlines, which is exactly what the card's three-line
//! preview consumes.

/// One inline span of a clip, in our own vocabulary.
///
/// `rclip_rtf`'s types stop at this module's edge: the view must not have to know what an RTF
/// colour-table index is, and swapping the parser later must not touch the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl Span {
    /// Whether this span needs styling at all.
    ///
    /// Plain spans are the common case, and the card draws them with no `TextRun` overhead.
    pub fn is_plain(&self) -> bool {
        !self.bold && !self.italic && !self.underline
    }
}

/// Whether these bytes claim to be RTF.
///
/// The clipboard can hand us the same document as RTF *and* as plain text; this is the test
/// that decides which one the card reads (RTF wins, because it is the richer one).
pub fn is_rtf(bytes: &[u8]) -> bool {
    rclip_rtf::is_rtf(bytes)
}

/// The code page the document declares, or 1252 (the RTF default) when it declares none.
fn declared_codepage(raw: &[u8]) -> u16 {
    const MARKER: &[u8] = br"\ansicpg";
    let mut index = 0;
    while index + MARKER.len() < raw.len() {
        if &raw[index..index + MARKER.len()] == MARKER {
            let mut value = 0u32;
            let mut digits = 0;
            let mut cursor = index + MARKER.len();
            while cursor < raw.len() && raw[cursor].is_ascii_digit() {
                value = value.saturating_mul(10) + u32::from(raw[cursor] - b'0');
                digits += 1;
                cursor += 1;
            }
            if digits > 0 {
                return value.min(u32::from(u16::MAX)) as u16;
            }
        }
        index += 1;
    }
    1252
}

/// The DBCS pages worth decoding here.
///
/// Single-byte pages are already handled inside `rclip-rtf`; these three are the ones a
/// clipboard in this part of the world actually sees.
fn dbcs_encoding(codepage: u16) -> Option<&'static encoding_rs::Encoding> {
    match codepage {
        936 => Some(encoding_rs::GBK),
        950 => Some(encoding_rs::BIG5),
        932 => Some(encoding_rs::SHIFT_JIS),
        _ => None,
    }
}

/// Rewrite `\'xx` byte escapes into `\uN?` escapes, decoding them with the declared code page.
///
/// Why this exists: `rclip-rtf` decodes `\uN` (decimal) escapes perfectly — fallback skipping
/// and all — but its codepage crate says outright that it is not a DBCS decoder, so `\'c4\'e3`
/// under `\ansicpg936` arrives as mojibake. Doing that one conversion here and handing the
/// parser an unambiguous escape is what makes Chinese RTF work without replacing the parser.
///
/// Two details that are easy to get wrong:
///   * escapes are collected **in runs**, because a DBCS character is two bytes (`\'c4\'e3` is
///     one character, not two);
///   * the output is `\uN?` per UTF-16 unit, not raw UTF-8 bytes — literal non-ASCII bytes in an
///     RTF stream are interpreted through the code page again, which is the trap this whole
///     function exists to escape. Surrogate pairs are emitted for anything outside the BMP.
fn normalize_byte_escapes(raw: &[u8]) -> Vec<u8> {
    let Some(encoding) = dbcs_encoding(declared_codepage(raw)) else {
        return raw.to_vec();
    };
    let hex = |byte: u8| (byte as char).to_digit(16).map(|digit| digit as u8);
    let mut out = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        let is_escape = raw[index] == b'\\'
            && index + 3 < raw.len()
            && raw[index + 1] == b'\''
            && hex(raw[index + 2]).is_some()
            && hex(raw[index + 3]).is_some();
        if !is_escape {
            out.push(raw[index]);
            index += 1;
            continue;
        }
        // One character may need several escapes; collect until the run ends.
        let mut bytes = Vec::new();
        while index + 3 < raw.len()
            && raw[index] == b'\\'
            && raw[index + 1] == b'\''
        {
            let (Some(high), Some(low)) = (hex(raw[index + 2]), hex(raw[index + 3])) else {
                break;
            };
            bytes.push(high * 16 + low);
            index += 4;
        }
        let (decoded, _) = encoding.decode_without_bom_handling(&bytes);
        for character in decoded.chars() {
            if (character as u32) < 0x80 {
                out.push(character as u8);
                continue;
            }
            let mut units = [0u16; 2];
            for unit in character.encode_utf16(&mut units) {
                out.extend_from_slice(format!("\\u{unit}?").as_bytes());
            }
        }
    }
    out
}

/// Parse RTF into inline spans, or `None` when there is nothing usable.
///
/// Never panics and never surfaces a parser error as a UI failure: a document we cannot read
/// is, from the card's point of view, simply not rich text — the caller falls back to showing
/// the plain text, which is what the user copied anyway.
pub fn rtf_spans(bytes: &[u8]) -> Option<Vec<Span>> {
    let normalized = normalize_byte_escapes(bytes);
    let document = rclip_rtf::Document::parse(&normalized).ok()?;
    let spans: Vec<Span> = document
        .runs
        .iter()
        .filter_map(|run| {
            let text = document.run_text(run).to_string();
            if text.is_empty() {
                return None;
            }
            Some(Span {
                text,
                bold: run.props.bold,
                italic: run.props.italic,
                underline: run.props.underline,
            })
        })
        .collect();
    if spans.is_empty() { None } else { Some(spans) }
}

/// The same spans, as one plain string. Used by the card's three-line preview and by the
/// search field's expectations, both of which want text and not formatting.
pub fn spans_to_text(spans: &[Span]) -> String {
    spans.iter().map(|span| span.text.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::{is_rtf, rtf_spans, spans_to_text};

    /// Word's usual shape: a Unicode escape for the Chinese characters.
    ///
    /// Note the escapes are **decimal**, not hex: 日 is U+65E5 = 26085 and 本 is U+672C =
    /// 26412. Writing `\u672c` would be a hex escape, which RTF does not have — the parser
    /// would read decimal 672 and produce a different character, which is exactly how this
    /// fixture was wrong the first time.
    const UNICODE_ESCAPE: &[u8] =
        br"{\rtf1\ansi\deff0{\fonttbl{\f0\fnil Arial;}}\fs24\b bold\b0  \u26085?\u26412? \par second}";

    /// The other common shape: `\'xx` byte escapes under a declared code page. Without the
    /// `codepage` feature these decode to mojibake, which is the failure this test exists for.
    const CODEPAGE_ESCAPE: &[u8] = br"{\rtf1\ansi\ansicpg936\deff0{\fonttbl{\f0\fnil Arial;}}\'c4\'e3\'ba\'c3\par plain}";

    #[test]
    fn rtf_is_recognised_and_plain_text_is_not() {
        assert!(is_rtf(UNICODE_ESCAPE));
        assert!(!is_rtf(b"just a note"));
        assert!(!is_rtf(b""));
        // HTML-ish content must not be mistaken for RTF.
        assert!(!is_rtf(b"{\\not-rtf}"));
    }

    #[test]
    fn the_three_inline_attributes_come_through() {
        let spans = rtf_spans(UNICODE_ESCAPE).expect("the sample should parse");
        // The bold word is its own span; the text after `\b0` is not.
        assert!(
            spans.iter().any(|span| span.bold && span.text.contains("bold")),
            "expected a bold span, got {spans:?}"
        );
        assert!(
            spans
                .iter()
                .any(|span| !span.bold && span.text.contains("second")),
            "expected the text after \\b0 to be a plain span, got {spans:?}"
        );
        // And nothing was invented: the whole text is still there, in order.
        assert_eq!(
            spans_to_text(&spans).split_whitespace().collect::<Vec<_>>(),
            ["bold", "日本", "second"]
        );
    }

    #[test]
    fn chinese_survives_both_escape_styles() {
        // \uN? — the fallback character after the escape must be skipped, not printed.
        let text = spans_to_text(&rtf_spans(UNICODE_ESCAPE).expect("parse"));
        assert!(text.contains("日本"), "unicode escape lost: {text}");
        assert!(!text.contains('?'), "the \\u fallback byte leaked: {text}");

        // And the paragraph mark became a newline, which is what the card's three lines need.
        assert!(text.contains('\n'), "\\par should be a line break: {text:?}");
    }

    /// The spike's finding, and the fix that came out of it.
    ///
    /// `\'xx` under `\ansicpg936` used to arrive as mojibake, because `rclip-codepage` states
    /// up front that GBK/Big5/Shift-JIS are outside its design. The fix is ours: collect the
    /// escape run, decode it with `encoding_rs`, and hand the parser `\uN?` — which it reads
    /// correctly. This test is the proof that the two halves meet.
    #[test]
    fn cp936_byte_escapes_decode_to_chinese() {
        let text = spans_to_text(&rtf_spans(CODEPAGE_ESCAPE).expect("parse"));
        assert!(text.contains("你好"), "codepage escape wrong: {text}");
        // The rest of the document still parses: one escape run must not swallow the tail.
        assert!(text.contains("plain"), "the tail after the escapes was lost: {text}");
        assert!(text.contains('\n'), "\\par is still a line break: {text:?}");
    }

    /// The same mechanism, one code page over, so this is not accidentally GBK-shaped.
    #[test]
    fn big5_byte_escapes_decode_to_chinese() {
        // \ansicpg950, `\'a7\'41\'a6\'6e` is Big5 for 你好.
        let raw = br"{\rtf1\ansi\ansicpg950\deff0{\fonttbl{\f0\fnil MingLiU;}}\'a7\'41\'a6\'6e}";
        let text = spans_to_text(&rtf_spans(raw).expect("parse"));
        assert!(text.contains("你好"), "big5 escape wrong: {text}");
    }

    /// A single-byte page must be left entirely alone — the normaliser only fires on DBCS.
    #[test]
    fn single_byte_pages_are_not_rewritten() {
        let raw = br"{\rtf1\ansi\ansicpg1252\deff0 caf\'e9}";
        assert_eq!(
            super::normalize_byte_escapes(raw),
            raw.to_vec(),
            "cp1252 must go through untouched: rclip-rtf already decodes it"
        );
    }

    #[test]
    fn junk_degrades_instead_of_panicking() {
        // Truncated mid-group, unbalanced braces, and not-RTF-at-all: all must come back as
        // "nothing usable", because the caller's fallback is the plain text.
        for input in [
            &br"{\rtf1\ansi\b"[..],
            &br"{\rtf1\ansi {{{{"[..],
            &b"\xff\xfe\x00\x01garbage"[..],
            &b""[..],
        ] {
            let _ = rtf_spans(input);
        }
    }
}
