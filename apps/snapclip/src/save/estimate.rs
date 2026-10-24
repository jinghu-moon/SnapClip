//! What a save would weigh, and how to make it fit.
//!
//! Everything here is a decision over a **measured** number. The measurement itself lives with
//! the encoder (`assemble::weigh`), and the panel is careful to treat these answers as policy:
//! the specification is explicit that a final check must be made against the encoded length and
//! not against any estimate (`docs/design/SnapClip 保存面板 · 功能与 UI 规格.md:235`), which is
//! why nothing in this module ever predicts a size it has not been given.

use super::scale::Size;

/// The attempt budget for a shrink pass. Each attempt is a real encode, so the budget is small
/// on purpose: ten halvings of the longer edge reach a single pixel from any capture we can take.
pub const SHRINK_ATTEMPTS: usize = 10;

/// How much room to leave on each shrink step.
///
/// A file's length is not linear in the pixel count — headers, the last partial block and the
/// encoder's own fixed costs all survive a resize — so asking for exactly `limit / measured` of
/// the pixels lands slightly over. Ninety seven percent is the margin: it wastes at most three
/// percent of the limit and it converges in one step for the cases that matter.
const MARGIN: f64 = 0.97;

/// `227.31 KB` or `1.20 MB`, the two shapes the row shows
/// (`docs/design/SnapClip 保存面板 · 功能与 UI 规格.md:242`).
///
/// Binary units, two decimals: the mockup's `227.31 KB` is consistent with a thousand and
/// twenty four byte kilobyte, and the panel should say the same thing the file manager will.
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let size = bytes as f64;
    if size >= MB {
        format!("{:.2} MB", size / MB)
    } else {
        format!("{:.2} KB", size / KB)
    }
}

/// The next size to try when the measured bytes were over the limit.
///
/// Returns `None` when the answer is "give up": a size that would not be smaller than the one
/// just measured cannot make progress, and a loop that keeps asking for the same picture is how
/// a shrink pass turns into a hang.
pub fn shrink_step(size: Size, measured: u64, limit: u64) -> Option<Size> {
    if measured == 0 || limit == 0 || limit >= measured {
        return None;
    }
    // Area scales with the square of the edge, so the edge scales with the square root of the
    // ratio. The margin sits on the ratio, not on the edge, so it shrinks the area, not the
    // length of a side.
    let ratio = (limit as f64 / measured as f64).sqrt() * MARGIN;
    let width = ((f64::from(size.width) * ratio).floor() as u32).max(1);
    let height = ((f64::from(size.height) * ratio).floor() as u32).max(1);
    let next = Size::new(width.min(size.width), height.min(size.height));
    if next == size || next.width >= size.width && next.height >= size.height {
        return None;
    }
    Some(next)
}

/// The largest size at most `source` whose **measured** bytes fit under `limit`.
///
/// `weigh` is called once per attempt and must encode, not estimate. The first attempt is the
/// source size itself, because most saves fit and there is no reason to shrink what already
/// passes.
///
/// `None` means no size down to a single pixel fit: the limit is below what the format's own
/// header costs, and the caller should say so rather than write something larger than asked.
pub fn fit_under<F>(source: Size, limit: u64, mut weigh: F) -> Option<(Size, u64)>
where
    F: FnMut(Size) -> Option<u64>,
{
    if limit == 0 || source.width == 0 || source.height == 0 {
        return None;
    }
    let mut size = source;
    let mut measured = weigh(size)?;
    if measured <= limit {
        return Some((size, measured));
    }
    for _ in 0..SHRINK_ATTEMPTS {
        size = shrink_step(size, measured, limit)?;
        measured = weigh(size)?;
        if measured <= limit {
            return Some((size, measured));
        }
    }
    None
}

/// The highest quality tier whose **measured** bytes fit under `limit`, with the bytes of the
/// tier that was chosen.
///
/// Tiers are tried from best to worst, which is the order the specification asks for
/// (`:233`). When none fits the lowest tier's measurement comes back with `None`, because the
/// panel still has something to say about it — it is the number in the warning.
pub fn best_tier<F>(limit: u64, mut weigh_tier: F) -> (Option<usize>, u64)
where
    F: FnMut(usize) -> Option<u64>,
{
    let mut lowest = 0;
    for tier in 0..super::assemble::TIER_QUALITY.len() {
        let Some(bytes) = weigh_tier(tier) else {
            continue;
        };
        lowest = bytes;
        if limit == 0 || bytes <= limit {
            return (Some(tier), bytes);
        }
    }
    (None, lowest)
}

/// Which of the four presets, as a fraction of the source, equals this size exactly
/// (`:224`: the chip lights up only when the size is exactly that preset).
pub const PRESET_PERCENTS: [u32; 4] = [100, 75, 50, 25];

/// The preset a size equals, if any. Compared by cross multiplication so that the rounded
/// pixel sizes the presets produce still match: `1440 * 75 / 100` is `1080` exactly, and a
/// width that rounds down is still that preset.
pub fn preset_of(size: Size, source: Size) -> Option<u32> {
    if source.width == 0 || source.height == 0 {
        return None;
    }
    for percent in PRESET_PERCENTS {
        let want = super::scale::output_size(
            super::scale::SizeMode::Percent,
            &percent.to_string(),
            &percent.to_string(),
            source,
        );
        if want == size {
            return Some(percent);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_are_shown_in_the_units_the_mockup_shows() {
        assert_eq!(format_bytes(232_710), "227.26 KB");
        assert_eq!(format_bytes(1_258_291), "1.20 MB");
        assert_eq!(format_bytes(0), "0.00 KB");
        // The boundary belongs to the larger unit, because 1024 KB is a megabyte.
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes(1024 * 1024 - 1), "1024.00 KB");
    }

    #[test]
    fn a_shrink_step_always_makes_the_picture_smaller() {
        let source = Size::new(1440, 900);
        let next = shrink_step(source, 2_000_000, 500_000).expect("a step");
        assert!(next.width < source.width && next.height < source.height);
        // A quarter of the bytes is about half the area, so about half the edge, less the margin:
        // sqrt(0.25) x 0.97 is 0.485, so 1440 becomes 698.4 and 900 becomes 436.5, both floored.
        // Written as the arithmetic rather than as a range, because a range wide enough to be
        // convenient stops noticing a wrong margin.
        assert_eq!(next.height, 436, "900 x 0.485, floored");
        assert_eq!(next.width, 698, "1440 x 0.485, floored");
    }

    #[test]
    fn a_shrink_step_gives_up_rather_than_repeating_itself() {
        // Already fits: nothing to do.
        assert!(shrink_step(Size::new(100, 100), 10, 100).is_none());
        // One pixel is the floor, and it cannot shrink further.
        assert!(shrink_step(Size::new(1, 1), 10, 5).is_none());
        // A limit of zero cannot be satisfied by any picture.
        assert!(shrink_step(Size::new(100, 100), 10, 0).is_none());
    }

    #[test]
    fn fitting_under_a_limit_stops_at_the_first_size_that_fits() {
        let source = Size::new(1440, 900);
        let mut weighed = Vec::new();
        let found = fit_under(source, 400_000, |size| {
            weighed.push(size);
            // A pretend encoder: bytes follow the pixel count.
            Some(u64::from(size.width) * u64::from(size.height) * 2)
        })
        .expect("something fits");
        assert!(found.1 <= 400_000);
        assert_eq!(weighed.first(), Some(&source), "the source size is tried first");
        assert!(weighed.len() > 1, "it had to shrink at least once");
        assert_eq!(weighed.last(), Some(&found.0));
    }

    #[test]
    fn fitting_under_a_limit_that_already_holds_does_not_shrink() {
        let source = Size::new(800, 600);
        let mut calls = 0;
        let found = fit_under(source, 10_000_000, |size| {
            calls += 1;
            Some(u64::from(size.width) * u64::from(size.height) * 4)
        })
        .expect("fits");
        assert_eq!(found.0, source);
        assert_eq!(calls, 1, "no second encode when the first one fits");
    }

    #[test]
    fn fitting_gives_up_when_no_size_can_pass() {
        let source = Size::new(64, 64);
        let found = fit_under(source, 1, |_| Some(1_000_000));
        assert!(found.is_none());
    }

    #[test]
    fn the_best_tier_is_the_first_one_that_fits() {
        let bytes = [900_000u64, 400_000, 200_000];
        let (tier, chosen) = best_tier(500_000, |tier| Some(bytes[tier]));
        assert_eq!((tier, chosen), (Some(1), 400_000));
    }

    #[test]
    fn when_no_tier_fits_the_lowest_one_is_reported() {
        let bytes = [900_000u64, 400_000, 200_000];
        let (tier, chosen) = best_tier(100_000, |tier| Some(bytes[tier]));
        assert_eq!((tier, chosen), (None, 200_000));
    }

    #[test]
    fn a_size_that_equals_a_preset_names_it() {
        let source = Size::new(1440, 900);
        assert_eq!(preset_of(Size::new(1440, 900), source), Some(100));
        assert_eq!(preset_of(Size::new(1080, 675), source), Some(75));
        assert_eq!(preset_of(Size::new(720, 450), source), Some(50));
        assert_eq!(preset_of(Size::new(360, 225), source), Some(25));
        assert_eq!(preset_of(Size::new(1000, 625), source), None);
    }
}
