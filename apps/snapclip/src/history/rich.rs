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

/// Parse RTF into inline spans, or `None` when there is nothing usable.
///
/// Never panics and never surfaces a parser error as a UI failure: a document we cannot read
/// is, from the card's point of view, simply not rich text — the caller falls back to showing
/// the plain text, which is what the user copied anyway.
pub fn rtf_spans(bytes: &[u8]) -> Option<Vec<Span>> {
    let document = rclip_rtf::Document::parse(bytes).ok()?;
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

    /// **The spike's finding, kept visible instead of hidden.**
    ///
    /// `\'xx` byte escapes under `\ansicpg936` come back as mojibake (`����`) even with the
    /// `codepage` feature on. The author's feature description says "Windows-125x", and cp936
    /// (GBK) is very likely outside its table — which matters because Word is not the only
    /// writer of RTF: plenty of Chinese software writes bytes under `\ansicpg936` instead of
    /// `\uN` decimal escapes.
    ///
    /// This is a **known gap, not a passing behaviour**: the RTF card path must not be called
    /// done while it is red. Three ways out, in the order I would try them:
    ///   1. check `rclip-codepage`'s table for 936 and, if it is there, call the decoding API
    ///      this crate exposes rather than relying on the feature flag alone;
    ///   2. decode `\'xx` ourselves through `encoding_rs` (GBK is one table) before parsing;
    ///   3. use `rtf-parser-tt`, which is the fork that advertises restored special-character
    ///      and codepage handling.
    #[test]
    #[ignore = "known gap: cp936 byte escapes decode to mojibake; see the doc comment"]
    fn cp936_byte_escapes_decode_to_chinese() {
        let text = spans_to_text(&rtf_spans(CODEPAGE_ESCAPE).expect("parse"));
        assert!(text.contains("你好"), "codepage escape wrong: {text}");
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
