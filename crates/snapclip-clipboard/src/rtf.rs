use snapclip_model::pin::TextSpan;

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
pub fn normalize_byte_escapes(raw: &[u8]) -> Vec<u8> {
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
        while index + 3 < raw.len() && raw[index] == b'\\' && raw[index + 1] == b'\'' {
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
pub fn rtf_spans(bytes: &[u8]) -> Option<Vec<TextSpan>> {
    if bytes.len() > 2 * 1024 * 1024 {
        return None;
    }
    let normalized = normalize_byte_escapes(bytes);
    let document = rclip_rtf::Document::parse(&normalized).ok()?;
    let spans: Vec<TextSpan> = document
        .runs
        .iter()
        .filter_map(|run| {
            let text = document.run_text(run).to_string();
            if text.is_empty() {
                return None;
            }
            Some(TextSpan::new(
                text,
                run.props.bold,
                run.props.italic,
                run.props.underline,
            ))
        })
        .collect();
    if spans.is_empty() { None } else { Some(spans) }
}
