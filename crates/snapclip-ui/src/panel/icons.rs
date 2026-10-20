//! Shared icon identities and baked bitmap assets.
//!
//! The enum and bytes are device-independent. WIC decoding, tinting and the D2D bitmap cache stay
//! in the renderer that owns the device lifetime.

/// The baked bitmaps' edge, in pixels: 24 units at 4x (covers up to 400% display scaling).
pub const ICON_BAKED_PX: u32 = 96;

/// How many glyphs are available to panel components.
pub const ICON_COUNT: usize = 6;

/// A panel glyph baked from `assets/icon/*.svg`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum Glyph {
    /// Swap width and height.
    Swap = 0,
    /// Center the selection on the screen.
    Center = 1,
    /// Move the selection to the origin.
    Origin = 2,
    /// Close a panel.
    Close = 3,
    /// Dropdown chevron; the renderer can flip it while open.
    Chevron = 4,
    /// Check mark for a selected menu item.
    Check = 5,
}

impl Glyph {
    pub const ALL: [Self; ICON_COUNT] = [
        Self::Swap,
        Self::Center,
        Self::Origin,
        Self::Close,
        Self::Chevron,
        Self::Check,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Swap => "swap",
            Self::Center => "center",
            Self::Origin => "origin",
            Self::Close => "close",
            Self::Chevron => "chevron",
            Self::Check => "check",
        }
    }

    /// White-on-transparent PNG; the renderer supplies the state colour.
    pub fn png(self) -> &'static [u8] {
        match self {
            Self::Swap => include_bytes!("../../../../assets/icon/swap.png"),
            Self::Center => include_bytes!("../../../../assets/icon/center.png"),
            Self::Origin => include_bytes!("../../../../assets/icon/origin.png"),
            Self::Close => include_bytes!("../../../../assets/icon/close.png"),
            Self::Chevron => include_bytes!("../../../../assets/icon/chevron.png"),
            Self::Check => include_bytes!("../../../../assets/icon/check.png"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

    #[test]
    fn every_glyph_has_a_stable_slot_and_baked_png() {
        for (slot, glyph) in Glyph::ALL.iter().enumerate() {
            assert_eq!(glyph.index(), slot);
            let png = glyph.png();
            assert_eq!(png.get(..8), Some(&PNG_SIGNATURE[..]), "{}", glyph.name());
            assert_eq!(&png[12..16], b"IHDR");
            let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
            let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
            assert_eq!((width, height), (ICON_BAKED_PX, ICON_BAKED_PX));
        }
    }

    #[test]
    fn every_glyph_has_distinct_asset_bytes() {
        for (slot, glyph) in Glyph::ALL.iter().enumerate() {
            for other in Glyph::ALL.iter().skip(slot + 1) {
                assert_ne!(
                    glyph.png(),
                    other.png(),
                    "{} vs {}",
                    glyph.name(),
                    other.name()
                );
            }
        }
    }
}
