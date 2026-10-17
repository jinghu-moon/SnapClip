//! Deterministic synthetic screenshot material.
//!
//! The corpus has to look like what SnapClip actually encodes — coloured small text, one pixel
//! lines, rounded corners, flat UI surfaces, gradients and (for one asset) transparency — because
//! encoder rankings on photographs say nothing about screenshots. Everything is generated from a
//! fixed seed, so two runs produce byte-identical input and the numbers are comparable over time.

/// One asset: BGRA pixels plus the name and size it was built for.
pub struct Asset {
    pub name: &'static str,
    pub width: usize,
    pub height: usize,
    /// BGRA, four bytes per pixel, rows top to bottom.
    pub pixels: Vec<u8>,
    /// Line boxes where text was drawn, in pixels. The generator knows this because it drew the
    /// text itself, and it is the only honest way to measure JPEG on text: whole-image SSIM is
    /// dominated by the flat surfaces around it (docs/34 §11).
    pub text_regions: Vec<TextRegion>,
}

/// Half-open pixel rectangle: `x0..x1` by `y0..y1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRegion {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl TextRegion {
    pub fn width(&self) -> usize {
        self.x1.saturating_sub(self.x0)
    }

    pub fn height(&self) -> usize {
        self.y1.saturating_sub(self.y0)
    }
}

/// Deterministic xorshift; good enough for texture, and reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn range(&mut self, low: i32, high: i32) -> i32 {
        if high <= low {
            return low;
        }
        let span = (high - low) as u64;
        low + (self.next() % span) as i32
    }
}

/// The four assets the plan asks for, in the order the report prints them.
pub fn corpus() -> Vec<Asset> {
    vec![
        ui("ui-1080", 1920, 1080, 0x5eed_1000),
        ui("ui-4k", 3840, 2160, 0x5eed_4000),
        scroll("scroll-tall", 1280, 30000, 0x5eed_7000),
        translucent("alpha-2k", 2000, 2000, 0x5eed_a000),
    ]
}

fn blank(width: usize, height: usize) -> Vec<u8> {
    vec![0u8; width * height * 4]
}

fn put(pixels: &mut [u8], width: usize, x: i32, y: i32, bgra: [u8; 4]) {
    if x < 0 || y < 0 {
        return;
    }
    let (x, y) = (x as usize, y as usize);
    if x >= width {
        return;
    }
    let index = (y * width + x) * 4;
    if index + 4 <= pixels.len() {
        pixels[index..index + 4].copy_from_slice(&bgra);
    }
}

fn horizontal(pixels: &mut [u8], width: usize, x0: i32, x1: i32, y: i32, bgra: [u8; 4]) {
    for x in x0..x1 {
        put(pixels, width, x, y, bgra);
    }
}

fn vertical(pixels: &mut [u8], width: usize, x: i32, y0: i32, y1: i32, bgra: [u8; 4]) {
    for y in y0..y1 {
        put(pixels, width, x, y, bgra);
    }
}

/// A rounded rectangle: the corner pixels outside the arc are left alone, which is exactly the
/// geometry the product's frame and mask share.
fn rounded_rect(
    pixels: &mut [u8],
    width: usize,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    radius: i32,
    bgra: [u8; 4],
) {
    for y in y0..y1 {
        for x in x0..x1 {
            let dx = if x - x0 < x1 - 1 - x { x - x0 } else { x1 - 1 - x };
            let dy = if y - y0 < y1 - 1 - y { y - y0 } else { y1 - 1 - y };
            if dx < radius && dy < radius {
                let cx = radius - dx;
                let cy = radius - dy;
                if cx * cx + cy * cy > radius * radius {
                    continue;
                }
            }
            put(pixels, width, x, y, bgra);
        }
    }
}

/// Rows of "text": short strokes of a few colours at text scale, which is what hurts JPEG.
///
/// Every line the loop actually draws is also recorded, so the quality run can restrict its
/// metrics to those boxes.
fn glyph_rows(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    rng: &mut Rng,
    top: i32,
    lines: i32,
    cell: i32,
    text_regions: &mut Vec<TextRegion>,
) {
    const INK: [[u8; 4]; 5] = [
        [40, 40, 40, 255],
        [160, 60, 40, 255],
        [40, 120, 200, 255],
        [30, 150, 70, 255],
        [200, 90, 170, 255],
    ];
    let mut y = top;
    for _ in 0..lines {
        if y + cell >= height as i32 {
            break;
        }
        let mut x = 24 + rng.range(0, 40);
        while x + cell < width as i32 - 24 {
            let ink = INK[rng.range(0, INK.len() as i32) as usize];
            for stroke in 0..3 {
                let offset = rng.range(0, cell - 2);
                horizontal(pixels, width, x + offset, x + offset + rng.range(1, 3), y + 2 + stroke * 3, ink);
            }
            x += cell + rng.range(2, 6);
        }
        // The line box: the full width the strokes may occupy, one text cell tall.
        text_regions.push(TextRegion {
            x0: 24,
            y0: y.max(0) as usize,
            x1: (width as i32 - 24).max(25) as usize,
            y1: (y + cell).min(height as i32).max(y + 1) as usize,
        });
        y += cell + 6;
    }
}

fn ui(name: &'static str, width: usize, height: usize, seed: u64) -> Asset {
    let mut rng = Rng(seed);
    let mut pixels = blank(width, height);
    let mut text_regions = Vec::new();
    // A vertical gradient, then flat panels, hairlines and text: the mix that separates encoders.
    for y in 0..height {
        let shade = 236 - (y * 40 / height) as u8;
        horizontal(&mut pixels, width, 0, width as i32, y as i32, [shade, shade, shade.saturating_add(3), 255]);
    }
    let panels = 4;
    for panel in 0..panels {
        let x0 = 40 + panel as i32 * (width as i32 / panels as i32);
        let y0 = 60;
        let x1 = x0 + width as i32 / panels as i32 - 80;
        let y1 = height as i32 - 120;
        let fill = [200 + rng.range(0, 20) as u8, 200 + rng.range(0, 20) as u8, 205, 255];
        rounded_rect(&mut pixels, width, x0, y0, x1, y1, 10, fill);
        rounded_rect(&mut pixels, width, x0, y0, x1, y0 + 34, 10, [90, 120, 160, 255]);
        vertical(&mut pixels, width, x0, y0, y1, [150, 150, 150, 255]);
        horizontal(&mut pixels, width, x0, x1, y1 - 1, [150, 150, 150, 255]);
        glyph_rows(&mut pixels, width, height, &mut rng, y0 + 60, 6, 11, &mut text_regions);
    }
    glyph_rows(&mut pixels, width, height, &mut rng, 24, 2, 13, &mut text_regions);
    Asset { name, width, height, pixels, text_regions }
}

fn scroll(name: &'static str, width: usize, height: usize, seed: u64) -> Asset {
    let mut rng = Rng(seed);
    let mut pixels = blank(width, height);
    let mut text_regions = Vec::new();
    // Repeated document bands: flat surfaces, hairlines and text, repeated far past any format
    // limit that WebP has.
    let band = 240;
    let mut y = 0;
    while y < height {
        horizontal(&mut pixels, width, 0, width as i32, y as i32, [245, 245, 245, 255]);
        horizontal(&mut pixels, width, 32, width as i32 - 32, (y + band - 1) as i32, [210, 210, 210, 255]);
        glyph_rows(&mut pixels, width, height, &mut rng, y as i32 + 16, 4, 10, &mut text_regions);
        y += band;
    }
    Asset { name, width, height, pixels, text_regions }
}

fn translucent(name: &'static str, width: usize, height: usize, seed: u64) -> Asset {
    let mut rng = Rng(seed);
    let mut pixels = blank(width, height);
    let mut text_regions = Vec::new();
    for y in 0..height {
        let shade = 30 + (y * 60 / height) as u8;
        horizontal(&mut pixels, width, 0, width as i32, y as i32, [shade + 20, shade, shade, 255]);
    }
    // Windows with real alpha, plus opaque content inside them.
    for window in 0..3 {
        let x0 = 120 + window * 560;
        let y0 = 160 + window * 300;
        rounded_rect(&mut pixels, width, x0, y0, x0 + 460, y0 + 300, 16, [250, 250, 250, 190]);
        rounded_rect(&mut pixels, width, x0, y0, x0 + 460, y0 + 36, 16, [40, 90, 140, 210]);
        glyph_rows(&mut pixels, width, height, &mut rng, y0 + 60, 3, 9, &mut text_regions);
    }
    Asset { name, width, height, pixels, text_regions }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_produces_the_same_bytes() {
        for seed in [0x5eed_1000u64, 0x5eed_a000] {
            let first = ui("a", 320, 200, seed);
            let second = ui("a", 320, 200, seed);
            assert_eq!(first.pixels, second.pixels, "seed {seed:#x} must be reproducible");
        }
    }

    #[test]
    fn different_seeds_produce_different_bytes() {
        let first = ui("a", 320, 200, 1);
        let second = ui("a", 320, 200, 2);
        assert_ne!(first.pixels, second.pixels);
    }

    #[test]
    fn every_asset_is_fully_sized_and_has_content() {
        for asset in corpus() {
            assert_eq!(asset.pixels.len(), asset.width * asset.height * 4, "{}", asset.name);
            // Not a blank canvas: at least one pixel differs from the first.
            let first = &asset.pixels[0..4];
            assert!(
                asset.pixels.chunks_exact(4).any(|pixel| pixel != first),
                "{} looks blank",
                asset.name
            );
        }
    }

    #[test]
    fn a_rounded_corner_leaves_the_outside_of_the_arc_alone() {
        let mut pixels = vec![0u8; 21 * 21 * 4];
        rounded_rect(&mut pixels, 21, 0, 0, 21, 21, 10, [255, 0, 0, 255]);
        // The very corner sits outside the arc and stays untouched.
        assert_eq!(&pixels[0..4], &[0, 0, 0, 0]);
        // The centre is filled.
        let centre = (10 * 21 + 10) * 4;
        assert_eq!(&pixels[centre..centre + 4], &[255, 0, 0, 255]);
    }
}
