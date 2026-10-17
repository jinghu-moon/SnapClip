//! The overlay panel's glyphs (`assets/icon/*.svg`, baked to PNG — docs/23 §14.54).
//!
//! Pure data plus the baked bytes, so it is testable without a GPU. The files themselves are
//! rasterised by the `icon-bake` tool (resvg) into white-on-transparent 96×96 PNGs, which are
//! compiled in here; the Direct2D side decodes them with WIC and tints them at draw time
//! (`windows::win::d2d::panel_icon`).
//!
//! Why a bake step and not SVG at run time: the whole story is in `assets/icon/README.md`, but the
//! short version is that Direct2D's own `DrawSvgDocument` painted nothing on this machine, and
//! redrawing the shapes by hand made them visibly different from the design. A real SVG renderer
//! produces the picture; the overlay only blits and tints it.

/// The baked bitmaps' edge, in pixels: 24 units at 4× (covers up to 400 % display scaling).
pub const ICON_BAKED_PX: u32 = 96;

/// How many glyphs there are; the device-side cache is sized from it.
pub const ICON_COUNT: usize = 8;

/// A glyph the panel draws.
///
/// The names are the file names under `assets/icon/`, so a mismatch is a compile error rather than a
/// blank square: [`Glyph::png`] is an `include_bytes!` of that exact file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum Glyph {
    /// 互换宽高（尺寸卡）。
    Swap = 0,
    /// 锁定比例（按下态）。
    Lock = 1,
    /// 锁定比例（未按下）。
    LockOpen = 2,
    /// 居中到屏幕。
    Center = 3,
    /// 移到原点。
    Origin = 4,
    /// 关闭面板。
    Close = 5,
    /// 下拉箭头（展开时上下翻转，不另存一份资产）。
    Chevron = 6,
    /// 下拉里选中那一项的勾。
    Check = 7,
}

impl Glyph {
    /// Every glyph, in the order [`Glyph::index`] reports.
    pub const ALL: [Self; ICON_COUNT] = [
        Self::Swap,
        Self::Lock,
        Self::LockOpen,
        Self::Center,
        Self::Origin,
        Self::Close,
        Self::Chevron,
        Self::Check,
    ];

    /// This glyph's slot in [`Glyph::ALL`] and in the device-side cache.
    pub fn index(self) -> usize {
        self as usize
    }

    /// The icon's name (`assets/icon/<name>.svg` and `<name>.png`): how a log names it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Swap => "swap",
            Self::Lock => "lock",
            Self::LockOpen => "lock-open",
            Self::Center => "center",
            Self::Origin => "origin",
            Self::Close => "close",
            Self::Chevron => "chevron",
            Self::Check => "check",
        }
    }

    /// The baked PNG, white on transparent, [`ICON_BAKED_PX`] square.
    ///
    /// `include_bytes!` of `assets/icon/<name>.png`: the file is the tool's output, committed to the
    /// repository, and `cargo test -p icon-bake` fails if it no longer matches its SVG.
    pub fn png(self) -> &'static [u8] {
        match self {
            Self::Swap => include_bytes!("../../../assets/icon/swap.png"),
            Self::Lock => include_bytes!("../../../assets/icon/lock.png"),
            Self::LockOpen => include_bytes!("../../../assets/icon/lock-open.png"),
            Self::Center => include_bytes!("../../../assets/icon/center.png"),
            Self::Origin => include_bytes!("../../../assets/icon/origin.png"),
            Self::Close => include_bytes!("../../../assets/icon/close.png"),
            Self::Chevron => include_bytes!("../../../assets/icon/chevron.png"),
            Self::Check => include_bytes!("../../../assets/icon/check.png"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The PNG signature every baked icon starts with, and the IHDR size fields' offsets.
    const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    const PNG_IHDR_WIDTH: std::ops::Range<usize> = 16..20;
    const PNG_IHDR_HEIGHT: std::ops::Range<usize> = 20..24;

    #[test]
    fn every_glyph_sits_in_the_slot_its_index_names() {
        for (slot, glyph) in Glyph::ALL.iter().enumerate() {
            assert_eq!(glyph.index(), slot, "{}", glyph.name());
        }
        assert_eq!(Glyph::ALL.len(), ICON_COUNT);
    }

    #[test]
    fn every_glyph_carries_a_png_of_the_baked_size() {
        for glyph in Glyph::ALL {
            let png = glyph.png();
            assert_eq!(
                png.get(..8),
                Some(&PNG_SIGNATURE[..]),
                "{} is not a PNG; run `cargo run -p icon-bake`",
                glyph.name()
            );
            assert_eq!(
                &png[12..16],
                b"IHDR",
                "{}: first chunk is not IHDR",
                glyph.name()
            );
            let width = u32::from_be_bytes(png[PNG_IHDR_WIDTH].try_into().unwrap());
            let height = u32::from_be_bytes(png[PNG_IHDR_HEIGHT].try_into().unwrap());
            assert_eq!(
                (width, height),
                (ICON_BAKED_PX, ICON_BAKED_PX),
                "{} is not {ICON_BAKED_PX}×{ICON_BAKED_PX}",
                glyph.name()
            );
        }
    }

    #[test]
    fn every_glyph_has_its_own_picture() {
        // 复制一个新变体却忘了改 `include_bytes!` 路径，是这类表最常见的错。
        for (slot, glyph) in Glyph::ALL.iter().enumerate() {
            for other in Glyph::ALL.iter().skip(slot + 1) {
                assert_ne!(
                    glyph.png(),
                    other.png(),
                    "{} 与 {} 指向同一张图",
                    glyph.name(),
                    other.name()
                );
            }
        }
    }
}
