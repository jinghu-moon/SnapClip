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

/// Cut spans down to the card's lines, keeping each span's style where it lands.
///
/// This is the join between the two halves of the redesign: `card::preview_lines` decides how
/// much *text* a card shows, and this decides how that text is *styled* once it has been cut.
/// The awkward cases all live here:
///
///   * a styled run that contains a newline has to become **two** spans on two lines, each
///     keeping the style — dropping the style at the break is the classic bug;
///   * the character cap is **per line**, so a bold run that straddles the cap is cut, not the
///     whole line's budget being spent by whichever span happened to come first;
///   * running out of lines is different from running out of characters, and both have to end
///     in the same visible ellipsis.
pub fn spans_to_lines(spans: &[Span], max_lines: usize, max_chars: usize) -> Vec<Vec<Span>> {
    let mut lines: Vec<Vec<Span>> = Vec::new();
    let mut current: Vec<Span> = Vec::new();
    let mut used = 0usize;
    // Whitespace collapsing has to be *stateful across spans*: a trailing space in one run and a
    // leading space in the next are one separator, and dropping both is how "hello **world**"
    // turns into "helloworld".
    let mut line_has_text = false;
    let mut pending_space = false;
    // Truncation that still owes the user an ellipsis, versus a cut that already carries one.
    let mut unfinished = false;

    'outer: for span in spans {
        for (index, segment) in span.text.split('\n').enumerate() {
            if index > 0 {
                // A hard break. The line is full whether or not it has any text in it.
                lines.push(std::mem::take(&mut current));
                used = 0;
                line_has_text = false;
                pending_space = false;
                if lines.len() >= max_lines {
                    unfinished = true;
                    break 'outer;
                }
            }
            let leading_space = segment.starts_with(char::is_whitespace);
            let trailing_space = segment.ends_with(char::is_whitespace);
            let core = crate::history::card::collapse(segment);
            if core.is_empty() {
                // A run that is nothing but whitespace still separates what is around it.
                if line_has_text {
                    pending_space = true;
                }
                continue;
            }
            let room = max_chars.saturating_sub(used);
            if room == 0 {
                unfinished = true;
                break 'outer;
            }
            let separator = (pending_space || leading_space) && line_has_text;
            let budget = room.saturating_sub(usize::from(separator));
            if budget == 0 {
                unfinished = true;
                break 'outer;
            }
            let length = core.chars().count();
            // A cut spends one character of the budget on the ellipsis itself, or the line would
            // end up one character wider than its cap.
            let keep = if length <= budget { length } else { budget - 1 };
            let mut text: String = core.chars().take(keep).collect();
            let cut = keep < length;
            if cut {
                text.push('…');
            }
            used += keep + usize::from(separator) + usize::from(cut);
            let mut text = text;
            if separator {
                text.insert(0, ' ');
            }
            current.push(Span {
                text,
                bold: span.bold,
                italic: span.italic,
                underline: span.underline,
            });
            line_has_text = true;
            pending_space = trailing_space;
            if cut {
                break 'outer;
            }
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines.truncate(max_lines);

    if unfinished {
        // The ellipsis belongs to the last line, and it inherits that line's style so it does
        // not look like a stray character.
        if let Some(last) = lines.last_mut() {
            match last.last_mut() {
                Some(span) => span.text.push('…'),
                None => last.push(Span {
                    text: "…".to_string(),
                    bold: false,
                    italic: false,
                    underline: false,
                }),
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::{Span, is_rtf, rtf_spans, spans_to_lines, spans_to_text};

    fn plain(text: &str) -> Span {
        Span {
            text: text.to_string(),
            bold: false,
            italic: false,
            underline: false,
        }
    }

    fn bold(text: &str) -> Span {
        Span {
            text: text.to_string(),
            bold: true,
            italic: false,
            underline: false,
        }
    }

    #[test]
    fn a_styled_run_that_spans_lines_keeps_its_style_on_both() {
        let lines = spans_to_lines(&[bold("加粗\n换行")], 3, 40);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0][0].text, "加粗");
        assert_eq!(lines[1][0].text, "换行");
        // The bug this test exists for: the second line losing the bold.
        assert!(lines[0][0].bold && lines[1][0].bold);
    }

    #[test]
    fn a_line_keeps_its_mixed_styles_in_order() {
        let lines = spans_to_lines(&[plain("a "), bold("b"), plain(" c")], 3, 40);
        assert_eq!(lines.len(), 1);
        // The words must not glue together: which span ends up owning the separator is an
        // implementation detail, so the assertion is about the text a reader sees.
        assert_eq!(spans_to_text(&lines[0]), "a b c");
        assert!(!lines[0][0].bold && lines[0][1].bold && !lines[0][2].bold);

        // A run that is only whitespace still separates its neighbours.
        let lines = spans_to_lines(&[plain("a"), plain("   "), bold("b")], 3, 40);
        assert_eq!(spans_to_text(&lines[0]), "a b");
    }

    #[test]
    fn a_separator_that_would_overflow_the_cap_is_not_added() {
        // Four characters and a separator against a five-character budget: the separator fits,
        // the next word does not, and the ellipsis takes the last slot.
        let lines = spans_to_lines(&[plain("abcd"), bold("ef")], 3, 5);
        assert_eq!(spans_to_text(&lines[0]), "abcd…");
        assert_eq!(spans_to_text(&lines[0]).chars().count(), 5);
    }

    #[test]
    fn running_out_of_lines_says_so_with_an_ellipsis() {
        let lines = spans_to_lines(&[plain("one\ntwo\nthree\nfour")], 3, 40);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[2][0].text, "three…");
    }

    #[test]
    fn the_character_cap_is_per_line_and_cuts_the_span_that_hits_it() {
        // Ten characters of bold is more than the line has room for.
        let lines = spans_to_lines(&[bold("0123456789")], 3, 5);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0][0].text, "0123…");
        assert!(lines[0][0].bold, "the cut span keeps its style");

        // A line whose budget is spent by an earlier span must not overflow with a later one.
        let lines = spans_to_lines(&[plain("abcd"), bold("efgh")], 3, 5);
        assert_eq!(lines[0][0].text, "abcd");
        assert_eq!(lines[0][1].text, "…");
    }

    #[test]
    fn a_short_document_is_not_announced_as_truncated() {
        let lines = spans_to_lines(&[plain("a\nb")], 3, 40);
        assert_eq!(lines.len(), 2);
        assert!(!lines[1][0].text.contains('…'));
    }

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

    /// What a real clipboard document costs.
    ///
    /// The card re-renders the visible rows on every scroll frame, so this is a per-frame cost,
    /// not a one-off: the assertion is deliberately generous (50 ms) so a loaded machine cannot
    /// make it flaky, while the printed number is the one to watch. Run with `--nocapture` to
    /// see it.
    #[test]
    fn a_fifty_kilobyte_document_parses_well_inside_a_frame() {
        // Realistic shape: a Word-ish header, DBCS byte escapes (so our normaliser runs, which
        // is the expensive half) and bold runs to give the run list some length.
        let mut rtf = String::from(
            r"{\rtf1\ansi\ansicpg936\deff0{\fonttbl{\f0\fnil Arial;}}\fs24 ",
        );
        while rtf.len() < 50 * 1024 {
            rtf.push_str(r"\b 加粗文字\b0 \'c4\'e3\'ba\'c3 plain line here\par ");
        }
        rtf.push('}');
        let bytes = rtf.as_bytes();

        // Warm-up: the first call pages in the encoding tables, which a scroll does once.
        let _ = rtf_spans(bytes);

        let iterations = 20;
        let started = std::time::Instant::now();
        for _ in 0..iterations {
            let spans = rtf_spans(bytes).expect("the sample parses");
            assert!(!spans.is_empty());
        }
        let per_parse = started.elapsed() / iterations;
        eprintln!(
            "[bench] {} KiB RTF → parse+normalise in {:?}",
            bytes.len() / 1024,
            per_parse
        );
        assert!(
            per_parse.as_millis() < 50,
            "one card's rich text took {per_parse:?}; that is per scroll frame"
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
