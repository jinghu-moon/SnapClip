//! Pure async color-sampling state machine for the magnifier.
//!
//! Coordinates tile-hit detection, 60 Hz throttling, stale-retention and color
//! formatting. This module is platform-independent: it does not touch GPU resources
//! directly. The overlay/renderer drives it with results from the D3D11 async sample
//! buffer (see `win/d3d11.rs`).

use std::time::{Duration, Instant};

use super::geometry::{Point, Rect};

/// Minimum interval between GPU sample submissions (~60 Hz).
const THROTTLE_INTERVAL: Duration = Duration::from_micros(16_667);

/// Tile size in pixels (must match `MagnifierConfig::tile_size`).
const TILE_SIZE: i32 = 32;

/// How the sampled colour is rendered in the magnifier info panel's primary
/// slot. `S` cycles the whole set — `Hex → Rgb → Hsl → Hex` — so the panel
/// shows exactly one live format at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorFormat {
    Hex,
    Rgb,
    Hsl,
}

impl Default for ColorFormat {
    fn default() -> Self {
        Self::Hex
    }
}

impl ColorFormat {
    /// The next format in the `S` cycle (`Hex -> Rgb -> Hsl -> Hex`).
    pub fn next(self) -> Self {
        match self {
            ColorFormat::Hex => ColorFormat::Rgb,
            ColorFormat::Rgb => ColorFormat::Hsl,
            ColorFormat::Hsl => ColorFormat::Hex,
        }
    }
}

/// Internal info about a sampling tile.
#[derive(Debug, Clone)]
struct TileInfo {
    /// Top-left origin in frame coordinates.
    origin: Point,
    /// Decoded pixel data (32×32 BGRA tightly packed). Only present after completion.
    pixels: Option<Vec<u8>>,
}

/// Tracks the state of async color sampling for the magnifier info panel.
#[derive(Debug)]
pub struct ColorSampler {
    /// The last completed tile (pixel data available).
    cached: Option<TileInfo>,
    /// The tile currently in-flight on the GPU (no data yet).
    pending: Option<TileInfo>,
    /// Currently displayed RGB.
    rgb: Option<(u8, u8, u8)>,
    /// Formatted #RRGGBB string.
    hex: Option<String>,
    /// Whether the displayed color differs from the last rendered value.
    dirty: bool,
    /// Timestamp of last submission for throttle.
    last_submit: Option<Instant>,
}

impl ColorSampler {
    pub fn new() -> Self {
        Self {
            cached: None,
            pending: None,
            rgb: None,
            hex: None,
            dirty: false,
            last_submit: None,
        }
    }

    /// Whether a new sample should be submitted to the GPU.
    ///
    /// Returns `false` if:
    /// - The cursor is still within the cached tile ("tile hit").
    /// - The same tile is already pending in-flight.
    /// - Throttle prevents submission (less than ~16 ms since last submit).
    pub fn should_request(&self, cursor: Point, tile: Rect, now: Instant) -> bool {
        // Tile hit: cursor within cached pixels.
        if let Some(ref c) = self.cached {
            let cached_rect = Rect::from_origin_size(c.origin, TILE_SIZE, TILE_SIZE);
            if cached_rect.contains(cursor) {
                return false;
            }
        }
        // Already pending this tile.
        if let Some(ref p) = self.pending
            && tile.contains(cursor)
            && p.origin == Point::new(tile.left, tile.top)
        {
            return false;
        }
        // 60 Hz throttle.
        if let Some(t) = self.last_submit
            && now.duration_since(t) < THROTTLE_INTERVAL
        {
            return false;
        }
        true
    }

    /// Record that a tile has been submitted to the GPU for copy.
    pub fn mark_submitted(&mut self, tile_origin: Point, now: Instant) {
        self.pending = Some(TileInfo {
            origin: tile_origin,
            pixels: None,
        });
        self.last_submit = Some(now);
    }

    /// Origin of the tile currently in-flight on the GPU, if any.
    ///
    /// The async completion must be labelled with the origin *submitted* — the
    /// cursor may have drifted into another tile by the time the copy lands, and
    /// recomputing the tile from the current cursor would misattribute the pixels.
    pub fn pending_origin(&self) -> Option<Point> {
        self.pending.as_ref().map(|p| p.origin)
    }

    /// Called when the GPU copy completes and pixel data is available.
    ///
    /// `pixels` is 32×32 BGRA tightly packed (4096 bytes). Reads the center pixel
    /// corresponding to `cursor` position and updates color.
    pub fn complete(&mut self, tile_origin: Point, pixels: Vec<u8>, cursor: Point) {
        let info = TileInfo {
            origin: tile_origin,
            pixels: Some(pixels),
        };
        self.pending = None;
        // Extract the center pixel from the completed tile.
        if let Some(rgb) = pixel_at(&info, cursor) {
            let old = self.rgb;
            self.rgb = Some(rgb);
            self.hex = Some(format!("#{:02X}{:02X}{:02X}", rgb.0, rgb.1, rgb.2));
            self.dirty = old != Some(rgb);
        }
        self.cached = Some(info);
    }

    /// Called when the GPU copy fails or device is lost. Preserves the last good color.
    pub fn mark_stale(&mut self) {
        self.pending = None;
        // Keep existing rgb/hex — stale means "no update", not "clear".
    }

    /// Update the displayed color for cursor position within the cached tile.
    ///
    /// Returns `true` if the color changed (requires info panel repaint).
    pub fn update_cursor(&mut self, cursor: Point) -> bool {
        let Some(ref cached) = self.cached else {
            return false;
        };
        if let Some(rgb) = pixel_at(cached, cursor)
            && self.rgb != Some(rgb)
        {
            self.rgb = Some(rgb);
            self.hex = Some(format!("#{:02X}{:02X}{:02X}", rgb.0, rgb.1, rgb.2));
            self.dirty = true;
            return true;
        }
        false
    }

    /// Current #RRGGBB hex string, or `None` if no sample has ever completed.
    pub fn hex(&self) -> Option<&str> {
        self.hex.as_deref()
    }

    /// The sampled colour rendered in `format`, ready for the info panel
    /// (`#RRGGBB`, `rgb(r,g,b)` or `hsl(h,s%,l%)`).
    pub fn formatted(&self, format: ColorFormat) -> Option<String> {
        let (r, g, b) = self.rgb?;
        Some(match format {
            ColorFormat::Hex => format!("#{r:02X}{g:02X}{b:02X}"),
            ColorFormat::Rgb => format!("rgb({r},{g},{b})"),
            ColorFormat::Hsl => {
                let (h, s, l) = rgb_to_hsl(r, g, b);
                format!("hsl({h},{s}%,{l}%)")
            }
        })
    }

    /// Current RGB tuple.
    pub fn rgb(&self) -> Option<(u8, u8, u8)> {
        self.rgb
    }

    /// Whether the color changed since the last render pass.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Clear dirty flag after the info panel has been repainted.
    pub fn mark_rendered(&mut self) {
        self.dirty = false;
    }

    /// Reset all state (e.g. on session end).
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

impl Default for ColorSampler {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert 8-bit sRGB to HSL (hue in degrees 0..359, S/L in percent 0..100).
pub fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (u32, u32, u32) {
    let rf = r as f32 / 255.0;
    let gf = g as f32 / 255.0;
    let bf = b as f32 / 255.0;
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let lightness = (max + min) / 2.0;
    let delta = max - min;
    if delta == 0.0 {
        return (0, 0, (lightness * 100.0).round() as u32);
    }
    let saturation = if lightness > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let hue = if max == rf {
        (gf - bf) / delta + if gf < bf { 6.0 } else { 0.0 }
    } else if max == gf {
        (bf - rf) / delta + 2.0
    } else {
        (rf - gf) / delta + 4.0
    } * 60.0;
    (
        hue.round() as u32 % 360,
        (saturation * 100.0).round() as u32,
        (lightness * 100.0).round() as u32,
    )
}

/// Read a pixel from a completed tile at the cursor's position.
///
/// Returns (R, G, B) in sRGB byte order. BGRA tile data stored in row-major 32×32.
fn pixel_at(tile: &TileInfo, cursor: Point) -> Option<(u8, u8, u8)> {
    let pixels = tile.pixels.as_ref()?;
    let dx = cursor.x - tile.origin.x;
    let dy = cursor.y - tile.origin.y;
    if dx < 0 || dy < 0 || dx >= TILE_SIZE || dy >= TILE_SIZE {
        return None;
    }
    let offset = (dy as usize * TILE_SIZE as usize + dx as usize) * 4;
    if offset + 3 >= pixels.len() {
        return None;
    }
    // BGRA → RGB
    Some((pixels[offset + 2], pixels[offset + 1], pixels[offset]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Helper: create a 32×32 tile filled with a single color.
    fn solid_tile(r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut pixels = vec![0u8; 32 * 32 * 4];
        for chunk in pixels.chunks_mut(4) {
            chunk[0] = b; // B
            chunk[1] = g; // G
            chunk[2] = r; // R
            chunk[3] = 255; // A
        }
        pixels
    }

    #[test]
    fn initial_state_has_no_color() {
        let sampler = ColorSampler::new();
        assert!(sampler.hex().is_none());
        assert!(sampler.rgb().is_none());
        assert!(!sampler.is_dirty());
    }

    #[test]
    fn should_request_when_no_cache() {
        let sampler = ColorSampler::new();
        let cursor = Point::new(100, 100);
        let tile = Rect::from_origin_size(Point::new(84, 84), 32, 32);
        assert!(sampler.should_request(cursor, tile, Instant::now()));
    }

    #[test]
    fn tile_hit_prevents_request() {
        let mut sampler = ColorSampler::new();
        let tile_origin = Point::new(84, 84);
        let cursor = Point::new(100, 100);
        // Complete a sample at this tile
        sampler.complete(tile_origin, solid_tile(255, 0, 0), cursor);
        // Cursor still within tile → hit, no request
        let cursor2 = Point::new(110, 110);
        let tile2 = Rect::from_origin_size(tile_origin, 32, 32);
        assert!(!sampler.should_request(cursor2, tile2, Instant::now()));
    }

    #[test]
    fn tile_miss_allows_request() {
        let mut sampler = ColorSampler::new();
        let tile_origin = Point::new(84, 84);
        let cursor = Point::new(100, 100);
        sampler.complete(tile_origin, solid_tile(255, 0, 0), cursor);
        // Move cursor far outside cached tile
        let new_cursor = Point::new(200, 200);
        let new_tile = Rect::from_origin_size(Point::new(184, 184), 32, 32);
        assert!(sampler.should_request(new_cursor, new_tile, Instant::now()));
    }

    #[test]
    fn throttle_prevents_rapid_requests() {
        let now = Instant::now();
        let cursor = Point::new(100, 100);
        let tile = Rect::from_origin_size(Point::new(84, 84), 32, 32);
        // Simulate a just-submitted request
        let mut sampler2 = ColorSampler::new();
        sampler2.mark_submitted(Point::new(84, 84), now);
        // Pending tile contains cursor → no request
        assert!(!sampler2.should_request(cursor, tile, now));
        // Even if cursor moves outside cached (pending counts), throttle blocks
        let later = now + Duration::from_micros(5_000); // 5ms, within 16.67ms
        let far_cursor = Point::new(300, 300);
        let far_tile = Rect::from_origin_size(Point::new(284, 284), 32, 32);
        // No cached tile yet, but pending was different tile → check throttle
        sampler2.cached = None;
        sampler2.pending = None;
        sampler2.last_submit = Some(now);
        assert!(!sampler2.should_request(far_cursor, far_tile, later));
        // After 17ms, throttle releases
        let after = now + Duration::from_micros(17_000);
        assert!(sampler2.should_request(far_cursor, far_tile, after));
    }

    #[test]
    fn complete_extracts_center_pixel() {
        let mut sampler = ColorSampler::new();
        let tile_origin = Point::new(84, 84);
        let cursor = Point::new(100, 100);
        // Create a tile with specific pixel at (16, 16) = cursor offset
        let mut pixels = vec![0u8; 32 * 32 * 4];
        let idx = (16 * 32 + 16) * 4;
        pixels[idx] = 128; // B
        pixels[idx + 1] = 64; // G
        pixels[idx + 2] = 200; // R
        pixels[idx + 3] = 255; // A
        sampler.complete(tile_origin, pixels, cursor);
        assert_eq!(sampler.rgb(), Some((200, 64, 128)));
        assert_eq!(sampler.hex(), Some("#C84080"));
        assert!(sampler.is_dirty());
    }

    #[test]
    fn pending_origin_reports_the_submitted_tile() {
        let mut sampler = ColorSampler::new();
        assert_eq!(sampler.pending_origin(), None);
        sampler.mark_submitted(Point::new(84, 84), Instant::now());
        assert_eq!(sampler.pending_origin(), Some(Point::new(84, 84)));
        // Completion clears the in-flight record.
        sampler.complete(Point::new(84, 84), solid_tile(1, 2, 3), Point::new(100, 100));
        assert_eq!(sampler.pending_origin(), None);
    }

    #[test]
    fn stale_preserves_last_color() {
        let mut sampler = ColorSampler::new();
        let tile_origin = Point::new(84, 84);
        let cursor = Point::new(100, 100);
        sampler.complete(tile_origin, solid_tile(10, 20, 30), cursor);
        sampler.mark_rendered();
        assert!(!sampler.is_dirty());
        // Stale
        sampler.mark_stale();
        assert_eq!(sampler.rgb(), Some((10, 20, 30)));
        assert_eq!(sampler.hex(), Some("#0A141E"));
        assert!(!sampler.is_dirty()); // stale doesn't make dirty
    }

    #[test]
    fn update_cursor_within_tile_detects_color_change() {
        let mut sampler = ColorSampler::new();
        let tile_origin = Point::new(84, 84);
        let cursor = Point::new(100, 100);
        // Fill with gradient: each pixel has unique R = (x + y) as u8
        let mut pixels = vec![0u8; 32 * 32 * 4];
        for y in 0..32usize {
            for x in 0..32usize {
                let i = (y * 32 + x) * 4;
                pixels[i] = 0; // B
                pixels[i + 1] = 0; // G
                pixels[i + 2] = (x + y) as u8; // R
                pixels[i + 3] = 255;
            }
        }
        sampler.complete(tile_origin, pixels, cursor);
        // cursor at (100,100) → dx=16, dy=16 → R=(16+16)%256=32
        assert_eq!(sampler.rgb(), Some((32, 0, 0)));
        sampler.mark_rendered();
        // Move cursor within tile → dx=17, dy=16 → R=(17+16)%256=33
        let new_cursor = Point::new(101, 100);
        let changed = sampler.update_cursor(new_cursor);
        assert!(changed);
        assert_eq!(sampler.rgb(), Some((33, 0, 0)));
        assert!(sampler.is_dirty());
    }

    #[test]
    fn format_hex_uppercase() {
        let mut sampler = ColorSampler::new();
        let tile_origin = Point::new(0, 0);
        let cursor = Point::new(0, 0);
        let mut pixels = vec![0u8; 32 * 32 * 4];
        // pixel at (0,0): B=0xAB, G=0xCD, R=0xEF
        pixels[0] = 0xAB;
        pixels[1] = 0xCD;
        pixels[2] = 0xEF;
        pixels[3] = 255;
        sampler.complete(tile_origin, pixels, cursor);
        assert_eq!(sampler.hex(), Some("#EFCDAB"));
    }

    #[test]
    fn formatted_renders_each_format() {
        let mut sampler = ColorSampler::new();
        // Pure red.
        sampler.complete(Point::new(0, 0), solid_tile(255, 0, 0), Point::new(0, 0));
        assert_eq!(sampler.formatted(ColorFormat::Hex).as_deref(), Some("#FF0000"));
        assert_eq!(sampler.formatted(ColorFormat::Rgb).as_deref(), Some("rgb(255,0,0)"));
        assert_eq!(sampler.formatted(ColorFormat::Hsl).as_deref(), Some("hsl(0,100%,50%)"));
        // No sample yet → nothing to render.
        sampler.reset();
        assert_eq!(sampler.formatted(ColorFormat::Hex), None);
    }

    #[test]
    fn format_cycles_hex_rgb_hsl() {
        let mut format = ColorFormat::default();
        assert_eq!(format, ColorFormat::Hex);
        format = format.next();
        assert_eq!(format, ColorFormat::Rgb);
        format = format.next();
        assert_eq!(format, ColorFormat::Hsl);
        format = format.next();
        assert_eq!(format, ColorFormat::Hex);
    }

    #[test]
    fn rgb_to_hsl_known_colors() {
        assert_eq!(rgb_to_hsl(255, 0, 0), (0, 100, 50)); // red
        assert_eq!(rgb_to_hsl(0, 255, 0), (120, 100, 50)); // green
        assert_eq!(rgb_to_hsl(0, 0, 255), (240, 100, 50)); // blue
        assert_eq!(rgb_to_hsl(255, 255, 255), (0, 0, 100)); // white
        assert_eq!(rgb_to_hsl(0, 0, 0), (0, 0, 0)); // black
        assert_eq!(rgb_to_hsl(128, 128, 128), (0, 0, 50)); // grey
        // Cyan sits at 180 degrees.
        assert_eq!(rgb_to_hsl(0, 255, 255), (180, 100, 50));
    }

    #[test]
    fn reset_clears_all_state() {
        let mut sampler = ColorSampler::new();
        sampler.complete(Point::new(0, 0), solid_tile(1, 2, 3), Point::new(0, 0));
        sampler.reset();
        assert!(sampler.hex().is_none());
        assert!(sampler.rgb().is_none());
        assert!(!sampler.is_dirty());
    }
}
