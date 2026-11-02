//! Canonical output pixels. Editing units never becomes a second source of truth.
use crate::save::scale::{Size, SizeMode, display_text, parse_size};

/// This ordinary-selection editor uses a whole-frame decoder; long-image artifacts
/// retain their separate streaming export path. Refuse before allocating a draft.
pub(super) fn validate_preview_size(size: Size) -> Result<(), &'static str> {
    if size.width == 0 || size.height == 0 {
        return Err("尺寸必须大于 0");
    }
    if size.width > 32768
        || size.height > 32768
        || u64::from(size.width) * u64::from(size.height) > 64_000_000
    {
        return Err("预览面板最多支持 6400 万像素，单边不超过 32768 px");
    }
    Ok(())
}

pub(super) struct Dimensions {
    pub source: Size,
    pub output: Size,
    pub mode: SizeMode,
    pub locked: bool,
    ratio: Size,
}

impl Dimensions {
    pub fn new(source: Size) -> Self {
        Self {
            source,
            output: source,
            mode: SizeMode::Pixels,
            locked: true,
            ratio: source,
        }
    }

    pub fn text(&self) -> (String, String) {
        display_text(self.mode, self.output, self.source)
    }

    pub fn preset(&mut self, percent: u32) {
        self.output = Size::new(
            percent_of(self.source.width, percent),
            percent_of(self.source.height, percent),
        );
        self.ratio = self.source;
    }

    pub fn set_locked(&mut self, locked: bool) {
        if locked && !self.locked {
            self.ratio = self.output;
        }
        self.locked = locked;
    }

    /// Invalid drafts stay in the input; they cannot silently change the export size.
    pub fn edit(&mut self, width: bool, text: &str) -> Result<(), &'static str> {
        let value = parse_size(text).ok_or("请输入大于 0 的整数")?;
        let base = if width {
            self.source.width
        } else {
            self.source.height
        };
        let pixels = match self.mode {
            SizeMode::Pixels => value,
            SizeMode::Percent => percent_of(base, value),
        };
        // Bound the conversion before rounding/casting can hide an overflow.
        let counterpart = if self.locked {
            let other = if width {
                self.ratio.height
            } else {
                self.ratio.width
            };
            let base = if width {
                self.ratio.width
            } else {
                self.ratio.height
            };
            let scaled =
                (u64::from(pixels) * u64::from(other) + u64::from(base) / 2) / u64::from(base);
            u32::try_from(scaled.max(1)).map_err(|_| "尺寸过大")?
        } else if width {
            self.output.height
        } else {
            self.output.width
        };
        let next = if width {
            Size::new(pixels, counterpart)
        } else {
            Size::new(counterpart, pixels)
        };
        // The preview and clipboard also allocate pixels. Refuse absurd drafts before allocation.
        validate_preview_size(next)?;
        self.output = next;
        Ok(())
    }
}

fn percent_of(base: u32, percent: u32) -> u32 {
    ((u64::from(base) * u64::from(percent) + 50) / 100).clamp(1, u64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relocking_captures_the_current_aspect_ratio() {
        let mut d = Dimensions::new(Size::new(800, 600));
        d.set_locked(false);
        d.edit(true, "200").unwrap();
        d.set_locked(true);
        d.edit(true, "400").unwrap();
        assert_eq!(d.output, Size::new(400, 1200));
        d.preset(100);
        d.edit(true, "400").unwrap();
        assert_eq!(d.output, Size::new(400, 300));
    }

    #[test]
    fn units_presets_and_reset_use_the_capture_not_mockup_dimensions() {
        let mut d = Dimensions::new(Size::new(1920, 1080));
        d.preset(50);
        assert_eq!(d.output, Size::new(960, 540));
        d.mode = SizeMode::Percent;
        assert_eq!(d.text(), ("50".into(), "50".into()));
        assert_eq!(d.output, Size::new(960, 540));
        d.edit(true, "75").unwrap();
        assert_eq!(d.output, Size::new(1440, 810));
        d.preset(100);
        assert_eq!(d.output, d.source);
    }

    #[test]
    fn ratio_is_based_on_original_pixels_and_unlocked_axes_are_independent() {
        let mut d = Dimensions::new(Size::new(800, 600));
        d.edit(false, "300").unwrap();
        assert_eq!(d.output, Size::new(400, 300));
        d.locked = false;
        d.edit(true, "200").unwrap();
        assert_eq!(d.output, Size::new(200, 300));
    }

    #[test]
    fn invalid_or_huge_drafts_do_not_mutate_the_last_valid_output() {
        let mut d = Dimensions::new(Size::new(800, 600));
        for draft in ["", "0", "-1", "1.5", "abc", "4294967295"] {
            assert!(d.edit(true, draft).is_err(), "{draft}");
            assert_eq!(d.output, d.source);
        }
    }
}
