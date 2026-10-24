//! Shared icon identities and baked bitmap assets.
//!
//! The enum and bytes are device-independent. WIC decoding, tinting and the D2D bitmap cache stay
//! in the renderer that owns the device lifetime.

/// The baked bitmaps' edge, in pixels: 24 units at 4x (covers up to 400% display scaling).
pub const ICON_BAKED_PX: u32 = 96;

/// How many glyphs are available to panel components.
pub const ICON_COUNT: usize = 9;

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
    /// Chain link, for the size row's lock between the two cells.
    Link = 6,
    /// Circular arrow, for the size row's reset to the captured size.
    Refresh = 7,
    /// Two stacked sheets, for the footer's copy-to-clipboard button.
    Copy = 8,
}

impl Glyph {
    pub const ALL: [Self; ICON_COUNT] = [
        Self::Swap,
        Self::Center,
        Self::Origin,
        Self::Close,
        Self::Chevron,
        Self::Check,
        Self::Link,
        Self::Refresh,
        Self::Copy,
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
            Self::Link => "link",
            Self::Refresh => "refresh",
            Self::Copy => "copy",
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
            Self::Link => include_bytes!("../../../../assets/icon/link.png"),
            Self::Refresh => include_bytes!("../../../../assets/icon/refresh.png"),
            Self::Copy => include_bytes!("../../../../assets/icon/copy.png"),
        }
    }

    /// The same picture as the PNG, as raw RGBA bytes.
    ///
    /// The capture overlay decodes the PNG and tints it with a Direct2D colour matrix effect, which
    /// needs a device context. The save panel paints through a DC render target, which is not a
    /// device context, so it takes the pixels and tints them on the processor instead
    /// ([`tint_rgba_to_premultiplied_bgra`]).
    pub fn raw(self) -> &'static [u8] {
        match self {
            Self::Swap => include_bytes!("../../../../assets/icon/swap.rgba"),
            Self::Center => include_bytes!("../../../../assets/icon/center.rgba"),
            Self::Origin => include_bytes!("../../../../assets/icon/origin.rgba"),
            Self::Close => include_bytes!("../../../../assets/icon/close.rgba"),
            Self::Chevron => include_bytes!("../../../../assets/icon/chevron.rgba"),
            Self::Check => include_bytes!("../../../../assets/icon/check.rgba"),
            Self::Link => include_bytes!("../../../../assets/icon/link.rgba"),
            Self::Refresh => include_bytes!("../../../../assets/icon/refresh.rgba"),
            Self::Copy => include_bytes!("../../../../assets/icon/copy.rgba"),
        }
    }
}

/// Colour a white-on-transparent icon and reorder its channels for a Direct2D bitmap.
///
/// The input is what `icon-bake` writes, RGBA with white where the icon has ink. The output is
/// **BGRA and premultiplied**, which is what the panel's bitmaps are created as, so this name says
/// the output order rather than leaving it to be remembered. Getting that order wrong has cost this
/// project three separate times.
///
/// Tinting multiplies the ink by the requested colour; the alpha is kept and then multiplied into
/// each channel, because a premultiplied bitmap expects `rgb * a`.
pub fn tint_rgba_to_premultiplied_bgra(rgba: &[u8], colour: [u8; 4]) -> Vec<u8> {
    let mut out = vec![0u8; rgba.len()];
    for (src, dst) in rgba.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
        let alpha = u32::from(src[3]);
        let channel = |value: u8, tint: u8| -> u8 {
            let tinted = u32::from(value) * u32::from(tint) / 255;
            ((tinted * alpha) / 255) as u8
        };
        dst[0] = channel(src[2], colour[2]); // blue
        dst[1] = channel(src[1], colour[1]); // green
        dst[2] = channel(src[0], colour[0]); // red
        dst[3] = src[3];
    }
    out
}

/// Turn a baked icon by half a turn.
///
/// `.fmt-btn[aria-expanded="true"] .chev { transform: rotate(180deg) }` (`save-panel-demo-v3.md`
/// `:178`) is the design's only rotated glyph: the chevron points down when the menu is closed and up
/// when it is open. The panel has no transform, so the rotation happens on the pixels — a square
/// buffer read from the far corner inwards. Pure, so it is asserted without a device.
pub fn flipped_180(rgba: &[u8], side: usize) -> Vec<u8> {
    let mut out = vec![0u8; rgba.len()];
    for y in 0..side {
        for x in 0..side {
            let from = (y * side + x) * 4;
            let to = ((side - 1 - y) * side + (side - 1 - x)) * 4;
            out[to..to + 4].copy_from_slice(&rgba[from..from + 4]);
        }
    }
    out
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

    #[test]
    fn every_glyph_has_raw_pixels_of_the_baked_size() {
        for glyph in Glyph::ALL {
            assert_eq!(
                glyph.raw().len(),
                (ICON_BAKED_PX * ICON_BAKED_PX * 4) as usize,
                "{}",
                glyph.name()
            );
        }
    }

    #[test]
    fn tinting_colours_the_ink_and_keeps_the_alpha() {
        // One white pixel and one transparent one, in the order icon-bake writes them.
        let rgba = [255, 255, 255, 255, 255, 255, 255, 0];
        let out = tint_rgba_to_premultiplied_bgra(&rgba, [94, 162, 245, 255]);
        assert_eq!(&out[0..4], &[245, 162, 94, 255], "the ink takes the colour, blue first");
        assert_eq!(&out[4..8], &[0, 0, 0, 0], "a transparent pixel stays transparent");
    }

    #[test]
    fn a_translucent_pixel_is_premultiplied() {
        // Half alpha: the colour channels have to come out at half strength.
        let rgba = [255, 255, 255, 128];
        let out = tint_rgba_to_premultiplied_bgra(&rgba, [94, 162, 245, 255]);
        assert_eq!(out[3], 128, "alpha is carried through");
        assert_eq!(out[2], 47, "red is tinted to 94 and then halved: 94 * 128 / 255");
        assert_eq!(out[0], 122, "blue is tinted to 245 and then halved: 245 * 128 / 255");
    }

    /// Half a turn is a corner-to-corner read, and doing it twice is doing nothing — which is the
    /// property that makes it a rotation rather than a shuffle.
    #[test]
    fn turning_a_glyph_twice_restores_it() {
        // A three by three buffer with one distinct pixel in each corner.
        let mut rgba = vec![0u8; 3 * 3 * 4];
        let mut put = |x: usize, y: usize, value: u8| {
            let at = (y * 3 + x) * 4;
            rgba[at] = value;
            rgba[at + 3] = 255;
        };
        put(0, 0, 10);
        put(2, 0, 20);
        put(0, 2, 30);
        put(2, 2, 40);

        let once = flipped_180(&rgba, 3);
        let px = |buffer: &[u8], x: usize, y: usize| buffer[(y * 3 + x) * 4];
        assert_eq!(px(&once, 2, 2), 10, "top left goes to bottom right");
        assert_eq!(px(&once, 0, 2), 20, "top right goes to bottom left");
        assert_eq!(px(&once, 2, 0), 30, "bottom left goes to top right");
        assert_eq!(px(&once, 0, 0), 40, "bottom right goes to top left");
        assert_eq!(px(&once, 1, 1), 0, "the centre stays, and stays empty");

        let twice = flipped_180(&once, 3);
        assert_eq!(twice, rgba, "twice is the identity");
        assert_eq!(once.len(), rgba.len(), "the buffer keeps its size");
    }
}
