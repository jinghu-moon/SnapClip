//! Shortening a path so both of its informative ends survive.
//!
//! The save path is shown on one line (`:167`, `height: 32px`, no wrapping) and it is read-only, so
//! nothing scrolls it into view: whatever does not fit is simply not there. Truncating the tail is
//! the wrong end for a path — the drive letter says which volume the file lands on and the last
//! segment says which folder, and both are the reasons the row exists. The middle, a run of parent
//! folders, is the part the reader can afford to lose.
//!
//! Measuring is the caller's: text width needs a device-independent format and a factory, and a
//! rule that could only be tested with both would be tested less. The tests measure characters.

/// A path shortened to fit a width, keeping the drive and as many trailing segments as fit.
///
/// The longest suffix that fits wins, not the shortest: the design's own example keeps two trailing
/// segments (`D:\…\assets\icon`), and stopping at the first candidate would have shown one.
pub fn elide_middle<F>(text: &str, max_width: i32, mut measure: F) -> String
where
    F: FnMut(&str) -> i32,
{
    if max_width <= 0 {
        return String::new();
    }
    if measure(text) <= max_width {
        return text.to_string();
    }

    // Separators are counted in the original so the kept tail is a slice of it: rebuilding a path
    // from segments would have to pick a separator, and a path that mixes the two would change.
    let separators: Vec<usize> = text
        .char_indices()
        .filter(|(_, character)| *character == '\\' || *character == '/')
        .map(|(index, _)| index)
        .collect();
    // `C:\` — the drive is the first separator inclusive, and it is what every candidate keeps.
    let head_end = separators.first().map_or(0, |first| first + 1);
    let head = &text[..head_end];

    // Suffixes that start at a separator, from the shortest to the longest. Longer candidates are
    // wider, so the first one that does not fit ends the walk.
    let mut best: Option<String> = None;
    for &start in separators.iter().rev() {
        if start < head_end {
            break;
        }
        let candidate = format!("{head}…{}", &text[start..]);
        if measure(&candidate) <= max_width {
            best = Some(candidate);
        } else {
            break;
        }
    }
    if let Some(best) = best {
        return best;
    }

    // Nothing but the drive fits, so the last segment gives way too: keep its beginning, because a
    // folder is recognised by how it starts (`SnapClip` not `Clip`), unlike an extension.
    let last = separators.last().map_or(0, |separator| separator + 1);
    let tail = &text[last..];
    let mut keep = tail.chars().count();
    while keep > 0 {
        let candidate: String = format!("…{}", tail.chars().take(keep).collect::<String>());
        if measure(&candidate) <= max_width {
            return candidate;
        }
        keep -= 1;
    }
    "…".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seven pixels per character. Wide enough that a few segments fit, and exact so that an
    /// expected width can be written down rather than discovered.
    fn measure(text: &str) -> i32 {
        text.chars().count() as i32 * 7
    }

    /// A path that fits is not touched: an ellipsis in a short path would be noise.
    #[test]
    fn a_path_that_fits_is_returned_whole() {
        assert_eq!(elide_middle("D:\\a", 70, measure), "D:\\a");
        assert_eq!(elide_middle("", 70, measure), "");
    }

    /// Exactly as wide as the room is still whole: `<=` rather than `<`, because a path that ends
    /// on the boundary has not overflowed.
    #[test]
    fn a_path_exactly_as_wide_as_the_room_is_returned_whole() {
        let exact = measure("D:\\ab"); // 5 characters, 35 pixels
        assert_eq!(elide_middle("D:\\ab", exact, measure), "D:\\ab");
        assert_eq!(elide_middle("D:\\ab", exact - 1, measure), "…ab");
    }

    /// The drive survives and the last segment survives: those are the two halves that say where
    /// the file goes, and the longer suffix wins while it fits.
    #[test]
    fn the_drive_and_the_last_segment_stay() {
        let long = "D:\\100_Projects\\110_Daily\\SnapClip\\assets\\icon";
        // Room for `D:\` + `…` + `\assets\icon`: 3 + 1 + 12 = 16 characters, 112 pixels.
        assert_eq!(elide_middle(long, 112, measure), "D:\\…\\assets\\icon");
        // Narrower, so only the last segment fits beside the drive.
        assert_eq!(elide_middle(long, 70, measure), "D:\\…\\icon");
    }

    /// When even the last segment cannot be shown whole, its beginning is kept rather than its end:
    /// a folder is recognised by how it starts.
    #[test]
    fn the_last_segment_gives_up_its_end_rather_than_its_start() {
        let long = "C:\\Users\\someone\\SnapClips";
        // Six characters is exactly the room (`…SnapC` = 6 x 7 = 42), so the longest that fits wins
        // and the tail keeps its start rather than its end.
        assert_eq!(elide_middle(long, 42, measure), "…SnapC");
    }

    /// No separator at all is not a path, but it still must not overflow: the same rule applies to
    /// whatever the user typed.
    #[test]
    fn a_value_without_a_separator_still_shortens() {
        assert_eq!(elide_middle("abcdefghij", 42, measure), "…abcde");
    }

    /// No room means nothing drawn, not an ellipsis wider than the field.
    #[test]
    fn no_room_returns_nothing() {
        assert_eq!(elide_middle("D:\\a", 0, measure), "");
        assert_eq!(elide_middle("D:\\a", -5, measure), "");
    }
}
