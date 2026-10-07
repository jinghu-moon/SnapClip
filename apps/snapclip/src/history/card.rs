//! What one history card shows, as pure functions (docs/23, card redesign).
//!
//! The card itself is layout; *what it says* is arithmetic: how many lines a clip's text
//! gets, how a byte count reads, how a timestamp reads, and how a character count is spoken.
//! Keeping that here means the layout can be rewritten without re-deriving the rules, and the
//! rules can be tested without a window.
//!
//! The reference for the shape is `prototypes/demo.html` (the "历史卡片 · 设计稿 v3" sheet):
//! content first, then one meta line of `类型 · 时间 · 字符数 · 体积 ｜ 来源`.

/// How many lines of content a card shows, and how many characters each line may hold.
///
/// Three lines is the sheet's rule. The per-line cap is an approximation of the 360px column:
/// text that still overflows is ellipsised by the row's own `truncate()`, but guessing close
/// keeps the ellipsis from appearing in the middle of a word on every line.
pub const PREVIEW_LINES: usize = 3;
pub const PREVIEW_CHARS_PER_LINE: usize = 40;

/// Split `text` into at most `max_lines` display lines, each capped at `max_chars`.
///
/// Newlines are *kept* as line breaks (a code block is recognisable by its shape, which is the
/// whole reason the card stopped collapsing it to one line), while tabs, carriage returns and
/// other control characters fold into spaces. A cap of zero means "no cap", which is what the
/// image and file cards want for their one-line lead.
pub fn preview_lines(text: &str, max_lines: usize, max_chars: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.lines() {
        if lines.len() >= max_lines {
            break;
        }
        let collapsed = collapse(raw);
        if collapsed.is_empty() {
            // A blank line inside a code block is content (it separates blocks), but a blank
            // line at the very start would waste one of the three.
            if !lines.is_empty() {
                lines.push(String::new());
            }
            continue;
        }
        lines.push(cap(&collapsed, max_chars));
    }
    lines
}

/// One line's worth of text: whitespace runs and control characters become single spaces.
///
/// Shared with the rich-text path (`history::rich`), because a tab must fold into a space the
/// same way whether the span it sits in is bold or not.
pub(crate) fn collapse(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut pending_space = false;
    for character in line.chars() {
        if character.is_whitespace() || character.is_control() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(character);
    }
    out
}

/// Cut `text` to `max_chars` characters, saying so with an ellipsis when it was cut.
fn cap(text: &str, max_chars: usize) -> String {
    if max_chars == 0 || text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(1).max(1);
    let mut out: String = text.chars().take(keep).collect();
    out.push('…');
    out
}

/// A byte count the way a person reads it.
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let value = bytes as f64;
    if value < KB {
        return format!("{bytes} B");
    }
    if value < MB {
        return format!("{:.1} KB", value / KB);
    }
    format!("{:.1} MB", value / MB)
}

/// A character count the way Chinese text reads it: 万 above ten thousand, separators below.
pub fn format_chars(count: u64) -> String {
    if count >= 10_000 {
        let wan = count as f64 / 10_000.0;
        // 10,000..100,000 keeps one decimal (1.2万), above that the decimal is noise.
        if count >= 100_000 {
            return format!("{:.0}万 字符", wan);
        }
        return format!("{:.1}万 字符", wan);
    }
    format!("{} 字符", with_thousands(count))
}

/// Group digits in threes: `1234` → `1,234`.
fn with_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(character);
    }
    out
}

/// A timestamp as the card's meta line shows it.
///
/// Today is just the clock (`22:29`) because the date would be noise; the current year names
/// the day (`09-30 19:46`); anything older carries its year, because "09-30" alone would be a
/// lie. `local_offset_seconds` is injected rather than read here so this stays testable and so
/// the caller decides where local time comes from.
pub fn format_timestamp(
    captured_at_unix_ms: i64,
    now_unix_ms: i64,
    local_offset_seconds: i64,
) -> String {
    let captured = captured_at_unix_ms + local_offset_seconds * 1000;
    let now = now_unix_ms + local_offset_seconds * 1000;
    let (captured_year, captured_month, captured_day, hour, minute) = civil_from_unix_ms(captured);
    let (now_year, now_month, now_day, _, _) = civil_from_unix_ms(now);
    if (captured_year, captured_month, captured_day) == (now_year, now_month, now_day) {
        return format!("{hour:02}:{minute:02}");
    }
    if captured_year == now_year {
        return format!("{captured_month:02}-{captured_day:02} {hour:02}:{minute:02}");
    }
    format!(
        "{captured_year:04}-{captured_month:02}-{captured_day:02} {hour:02}:{minute:02}"
    )
}

/// Civil date and clock time from a Unix millisecond stamp, in whatever zone it is given.
///
/// Howard Hinnant's days-from-civil inverse: integer-only, valid for the whole range we can
/// see in a clipboard history, and — unlike a hand-rolled leap-year loop — it is one place to
/// get wrong instead of four.
fn civil_from_unix_ms(unix_ms: i64) -> (i32, u32, u32, u32, u32) {
    let seconds = unix_ms.div_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let hour = (seconds_of_day / 3600) as u32;
    let minute = ((seconds_of_day % 3600) / 60) as u32;

    // Shift the epoch to 0000-03-01 so leap days land at the end of the year.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = (month_prime + if month_prime < 10 { 3 } else { -9 }) as u32;
    let year = (year + i64::from(month <= 2)) as i32;
    (year, month, day, hour, minute)
}

#[cfg(test)]
mod tests {
    use super::{
        PREVIEW_CHARS_PER_LINE, PREVIEW_LINES, format_bytes, format_chars, format_timestamp,
        preview_lines,
    };

    #[test]
    fn a_code_block_keeps_its_shape_for_three_lines() {
        let text = "<!doctype html>\n<html lang=\"zh-CN\">\n<head>\n<meta charset=\"utf-8\">";
        let lines = preview_lines(text, PREVIEW_LINES, PREVIEW_CHARS_PER_LINE);
        assert_eq!(
            lines,
            [
                "<!doctype html>".to_string(),
                "<html lang=\"zh-CN\">".to_string(),
                "<head>".to_string()
            ],
            "three lines, in order, and no fourth"
        );

        // A blank line in the middle is content (it separates code blocks)...
        let spaced = preview_lines("a\n\nb", 3, 40);
        assert_eq!(spaced, ["a".to_string(), String::new(), "b".to_string()]);
        // ...but one at the start would waste one of the three.
        assert_eq!(preview_lines("\n\na", 3, 40), ["a".to_string()]);
    }

    #[test]
    fn one_long_line_is_cut_with_a_visible_ellipsis() {
        let lines = preview_lines("x".repeat(100).as_str(), 3, 10);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].chars().count(), 10);
        assert!(lines[0].ends_with('…'));

        // Exactly at the cap: nothing was lost, so nothing is announced.
        let exact = preview_lines("yyyyyyyyyy", 3, 10);
        assert_eq!(exact[0], "yyyyyyyyyy");
    }

    #[test]
    fn bytes_read_the_way_a_person_says_them() {
        assert_eq!(format_bytes(48), "48 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(76_595), "74.8 KB");
        assert_eq!(format_bytes(2_516_582), "2.4 MB");
    }

    #[test]
    fn character_counts_switch_to_wan_where_chinese_does() {
        assert_eq!(format_chars(1), "1 字符");
        assert_eq!(format_chars(1234), "1,234 字符");
        assert_eq!(format_chars(9999), "9,999 字符");
        assert_eq!(format_chars(10_000), "1.0万 字符");
        assert_eq!(format_chars(69_000), "6.9万 字符");
        assert_eq!(format_chars(123_456), "12万 字符");
    }

    #[test]
    fn a_timestamp_says_only_as_much_as_it_must() {
        // Anchored on the leap-day instant the test below pins down (2024-02-29 12:34 UTC), so
        // this test never has to re-derive a calendar by hand.
        let now = 1_709_210_040_000;
        let offset = 0;

        // Same day: the clock alone.
        assert_eq!(format_timestamp(now, now, offset), "12:34");
        assert_eq!(format_timestamp(now - 3 * 60 * 1000, now, offset), "12:31");

        // Same year, an earlier day: the date, without the year.
        assert_eq!(
            format_timestamp(now - 86_400_000, now, offset),
            "02-28 12:34"
        );

        // An older year must carry the year, or "02-28" would be a lie.
        let older = format_timestamp(now - 400 * 86_400_000, now, offset);
        assert!(older.starts_with("2023-"), "got {older}");
        assert!(older.ends_with(" 12:34"), "got {older}");

        // And the offset is applied, not ignored: the same instant in +08:00 is 20:34.
        assert_eq!(format_timestamp(now, now, 8 * 3600), "20:34");
    }

    #[test]
    fn the_civil_calendar_handles_a_known_leap_day() {
        // 2024-02-29 12:34 UTC — the case a naive month table gets wrong.
        let stamp = 1_709_210_040_000;
        assert_eq!(
            super::civil_from_unix_ms(stamp),
            (2024, 2, 29, 12, 34)
        );
    }
}
