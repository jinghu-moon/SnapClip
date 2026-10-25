//! Shared icon identities and baked bitmap assets.
//!
//! The enum and bytes are device-independent. WIC decoding, tinting and the D2D bitmap cache stay
//! in the renderer that owns the device lifetime.

/// The baked bitmaps' edge, in pixels: 24 units at 4x (covers up to 400% display scaling).
pub const ICON_BAKED_PX: u32 = 96;

/// How many glyphs are available to panel components.
pub const ICON_COUNT: usize = 14;

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
    /// A folder, for the path row's shortcut chips (`.path-chips`, `:548`).
    Folder = 9,
    /// An open folder, for the path row's "show it in Explorer" button (`.btn-ghost.icon-only`,
    /// `:546`).
    FolderOpen = 10,
    /// Braces with a dot, for the name row's insert-a-variable button (`.name-var`, `:557`).
    Braces = 11,
    /// An info circle, for the size row's over-resolution hint (.warn-hint, :622).
    InfoCircle = 12,
    /// A single bar, for the panel's minimize button. The mockup's header has only the close
    /// button (`:474`), so this one has no design row of its own: it is the same 14 dip glyph in
    /// the same 26 dip button, to the left of the close.
    Minus = 13,
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
        Self::Folder,
        Self::FolderOpen,
        Self::Braces,
        Self::InfoCircle,
        Self::Minus,
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
            Self::Folder => "folder",
            Self::FolderOpen => "folder-open",
            Self::Braces => "braces",
            Self::InfoCircle => "info-circle",
            Self::Minus => "minus",
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
            Self::Folder => include_bytes!("../../../../assets/icon/folder.png"),
            Self::FolderOpen => include_bytes!("../../../../assets/icon/folder-open.png"),
            Self::Braces => include_bytes!("../../../../assets/icon/braces.png"),
            Self::InfoCircle => include_bytes!("../../../../assets/icon/info-circle.png"),
            Self::Minus => include_bytes!("../../../../assets/icon/minus.png"),
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
            Self::Folder => include_bytes!("../../../../assets/icon/folder.rgba"),
            Self::FolderOpen => include_bytes!("../../../../assets/icon/folder-open.rgba"),
            Self::Braces => include_bytes!("../../../../assets/icon/braces.rgba"),
            Self::InfoCircle => include_bytes!("../../../../assets/icon/info-circle.rgba"),
            Self::Minus => include_bytes!("../../../../assets/icon/minus.rgba"),
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
/// Tinting multiplies the ink by the requested colour; the two alphas — the icon's own and the
/// colour's — are multiplied into one, and that combined alpha is then multiplied into each channel,
/// because a premultiplied bitmap expects `rgb * a`.
///
/// The colour's alpha matters and used to be dropped: `--muted` is white at sixty two percent, so an
/// icon tinted with it must be translucent and composite over what is behind it. Keeping only the
/// source alpha drew every icon opaque, which is why the close glyph and the variable wand were
/// full-strength grey rather than the muted grey the design asks for.
pub fn tint_rgba_to_premultiplied_bgra(rgba: &[u8], colour: [u8; 4]) -> Vec<u8> {
    let mut out = vec![0u8; rgba.len()];
    for (src, dst) in rgba.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
        let alpha = u32::from(src[3]) * u32::from(colour[3]) / 255;
        let channel = |value: u8, tint: u8| -> u8 {
            let tinted = u32::from(value) * u32::from(tint) / 255;
            ((tinted * alpha) / 255) as u8
        };
        dst[0] = channel(src[2], colour[2]); // blue
        dst[1] = channel(src[1], colour[1]); // green
        dst[2] = channel(src[0], colour[0]); // red
        dst[3] = alpha as u8;
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

/// A rounded square of one colour, as premultiplied BGRA ready for a bitmap.
///
/// The corners are baked into the alpha because a bitmap has none of its own: the fill row's
/// swatches are drawn with `draw_bitmap_at`, and a square of colour under a rounded border would
/// show its square corners outside it. The caller generates at several times the drawn size and
/// lets the bitmap scale down, which is where the edge smoothing comes from.
pub fn rounded_square_bgra(side: u32, colour: [u8; 3], radius: f32) -> Vec<u8> {
    let mut out = Vec::with_capacity(side as usize * side as usize * 4);
    let edge = side as f32;
    for y in 0..side {
        for x in 0..side {
            if rounded_rect_covers(x as f32 + 0.5, y as f32 + 0.5, edge, radius) {
                // Premultiplied, which is what the bitmap's alpha mode expects. The colour arrives
                // in the buffer's own order already (blue first), as `PanelValues::fill` does, and
                // alpha is full because a swatch is a solid colour.
                out.extend_from_slice(&[colour[0], colour[1], colour[2], 255]);
            } else {
                out.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    out
}

/// The custom-colour swatch: `.bg-opt.custom { background: conic-gradient(#f55, #fc4, #5d5, #4cf,
/// #55f, #f5c, #f55) }` (`:317`), the design's way of saying "pick any colour".
///
/// The design's gradient starts at the top and turns clockwise, so the hue is the angle measured
/// from straight up, and the swatch's rounded corners come from the same mask as the flat ones.
pub fn hue_wheel_bgra(side: u32, radius: f32) -> Vec<u8> {
    let mut out = Vec::with_capacity(side as usize * side as usize * 4);
    let edge = side as f32;
    let centre = edge / 2.0;
    for y in 0..side {
        for x in 0..side {
            if !rounded_rect_covers(x as f32 + 0.5, y as f32 + 0.5, edge, radius) {
                out.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            // `atan2(dx, -dy)`: nothing at the top of the wheel, turning clockwise, which is the
            // direction CSS's conic-gradient turns.
            let dx = x as f32 + 0.5 - centre;
            let dy = y as f32 + 0.5 - centre;
            let turn = (dx.atan2(-dy) / std::f32::consts::TAU).rem_euclid(1.0);
            let [r, g, b] = hue_to_rgb(turn);
            out.extend_from_slice(&[b, g, r, 255]);
        }
    }
    out
}

/// Full-saturation, full-value hue at `turn` of the circle, as RGB.
fn hue_to_rgb(turn: f32) -> [u8; 3] {
    let scaled = turn * 6.0;
    let sector = scaled.floor();
    let fraction = scaled - sector;
    let rising = (fraction * 255.0).round() as u8;
    let falling = ((1.0 - fraction) * 255.0).round() as u8;
    match sector as i32 % 6 {
        0 => [255, rising, 0],
        1 => [falling, 255, 0],
        2 => [0, 255, rising],
        3 => [0, falling, 255],
        4 => [rising, 0, 255],
        _ => [255, 0, falling],
    }
}

/// Whether a point in the mask's own square lies inside the rounded rectangle that fills it.
fn rounded_rect_covers(x: f32, y: f32, side: f32, radius: f32) -> bool {
    let radius = radius.min(side / 2.0);
    if radius <= 0.0 {
        return x >= 0.0 && x < side && y >= 0.0 && y < side;
    }
    // The nearest point of the inner rectangle: a corner region is only covered within `radius` of
    // the corner's own centre, everywhere else every point of the square is inside.
    let nearest_x = x.clamp(radius, side - radius);
    let nearest_y = y.clamp(radius, side - radius);
    let dx = x - nearest_x;
    let dy = y - nearest_y;
    dx * dx + dy * dy <= radius * radius
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

    /// A swatch is a rounded square: opaque through the middle, and nothing at all in the corners,
    /// which is what keeps a bitmap-drawn swatch from showing square corners inside a rounded border.
    #[test]
    fn a_rounded_square_is_ink_inside_and_empty_in_its_corners() {
        let side = 16;
        let pixels = rounded_square_bgra(side, [255, 255, 255], 4.0);
        let at = |x: usize, y: usize| -> [u8; 4] {
            let at = (y * side as usize + x) * 4;
            [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
        };
        assert_eq!(at(8, 8), [255, 255, 255, 255], "the middle is the colour");
        assert_eq!(at(0, 0)[3], 0, "the top left corner is empty");
        assert_eq!(at(15, 0)[3], 0, "the top right corner is empty");
        assert_eq!(at(0, 15)[3], 0, "the bottom left corner is empty");
        assert_eq!(at(15, 15)[3], 0, "the bottom right corner is empty");
        assert_eq!(at(8, 0)[3], 255, "the top edge's middle is ink");
    }

    /// The custom swatch says "any colour" by being more than one: if it painted a single colour the
    /// panel would look like it had picked one. Four points around the wheel must differ.
    #[test]
    fn the_custom_swatch_is_a_wheel_rather_than_one_colour() {
        let side = 32;
        let pixels = hue_wheel_bgra(side, 8.0);
        let rgb = |x: usize, y: usize| -> [u8; 3] {
            let at = (y * side as usize + x) * 4;
            [pixels[at + 2], pixels[at + 1], pixels[at]]
        };
        let points = [
            ("top", rgb(16, 4)),
            ("right", rgb(28, 16)),
            ("bottom", rgb(16, 28)),
            ("left", rgb(4, 16)),
        ];
        // Compared pairwise rather than each against the first: a loop that includes the reference
        // point in its own comparison asserts nothing about it, which is the shape of a test that
        // can never fail.
        for (index, (name, colour)) in points.iter().enumerate() {
            for (other_name, other) in points.iter().skip(index + 1) {
                assert_ne!(
                    colour, other,
                    "{name} and {other_name} are the same colour ({colour:?}), so the swatch is not a wheel"
                );
            }
        }
        assert_eq!(pixels[3], 0, "the corner outside the rounded mask stays empty");
    }
}
