//! Spatial value objects, in **physical pixels of one monitor's client area**.
//!
//! The overlay window is borderless and exactly as large as the monitor, so screen
//! coordinates, client coordinates and back-buffer coordinates differ only by the
//! monitor origin. Keeping the maths in a dependency-free crate means DPI,
//! multi-monitor and handle behaviour stay unit-testable without a window station.

/// Point in physical pixels. `x`/`y` may be negative for monitors left of or above
/// the primary display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// Axis-aligned rectangle in physical pixels.
///
/// `right`/`bottom` are exclusive. Construction through [`Rect::from_corners`]
/// always normalises so callers never have to reason about drag direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    /// Normalised rectangle spanning two arbitrary corners.
    pub fn from_corners(a: Point, b: Point) -> Self {
        Self {
            left: a.x.min(b.x),
            top: a.y.min(b.y),
            right: a.x.max(b.x),
            bottom: a.y.max(b.y),
        }
    }

    pub fn from_origin_size(origin: Point, width: i32, height: i32) -> Self {
        Self {
            left: origin.x,
            top: origin.y,
            right: origin.x + width,
            bottom: origin.y + height,
        }
    }

    pub fn width(&self) -> i32 {
        self.right.saturating_sub(self.left)
    }

    pub fn height(&self) -> i32 {
        self.bottom.saturating_sub(self.top)
    }

    /// A selection is usable only when it covers at least one pixel in both axes.
    pub fn is_empty(&self) -> bool {
        self.width() <= 0 || self.height() <= 0
    }

    pub fn area(&self) -> i64 {
        i64::from(self.width()) * i64::from(self.height())
    }

    pub fn center(&self) -> Point {
        Point::new(
            self.left + self.width() / 2,
            self.top + self.height() / 2,
        )
    }

    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.left
            && point.x < self.right
            && point.y >= self.top
            && point.y < self.bottom
    }

    /// Whether `other` lies inside `self` (edges may touch).
    ///
    /// The ancestor walk (docs/21 §5.17) needs this: a level chain is only meaningful when each level
    /// contains the next, and that has to be checkable separately from point containment.
    pub fn contains_rect(&self, other: Rect) -> bool {
        other.left >= self.left
            && other.top >= self.top
            && other.right <= self.right
            && other.bottom <= self.bottom
    }

    pub fn inflate(&self, amount: i32) -> Self {
        Self {
            left: self.left - amount,
            top: self.top - amount,
            right: self.right + amount,
            bottom: self.bottom + amount,
        }
    }

    pub fn translate(&self, delta: Point) -> Self {
        Self {
            left: self.left + delta.x,
            top: self.top + delta.y,
            right: self.right + delta.x,
            bottom: self.bottom + delta.y,
        }
    }

    /// Move the rectangle so it fits inside `bounds` without changing its size.
    ///
    /// When the rectangle is wider or taller than `bounds` it is pinned to the bounds
    /// on that axis rather than left off-screen: dragging a large selection far past an
    /// edge must keep it visible.
    pub fn clamped_into(&self, bounds: Rect) -> Self {
        let mut result = *self;
        if result.width() <= bounds.width() {
            if result.left < bounds.left {
                result.right += bounds.left - result.left;
                result.left = bounds.left;
            }
            if result.right > bounds.right {
                result.left -= result.right - bounds.right;
                result.right = bounds.right;
            }
        } else if result.right <= bounds.left {
            let width = result.width();
            result.left = bounds.left;
            result.right = bounds.left + width;
        } else if result.left >= bounds.right {
            let width = result.width();
            result.right = bounds.right;
            result.left = bounds.right - width;
        }
        if result.height() <= bounds.height() {
            if result.top < bounds.top {
                result.bottom += bounds.top - result.top;
                result.top = bounds.top;
            }
            if result.bottom > bounds.bottom {
                result.top -= result.bottom - bounds.bottom;
                result.bottom = bounds.bottom;
            }
        } else if result.bottom <= bounds.top {
            let height = result.height();
            result.top = bounds.top;
            result.bottom = bounds.top + height;
        } else if result.top >= bounds.bottom {
            let height = result.height();
            result.bottom = bounds.bottom;
            result.top = bounds.bottom - height;
        }
        // Final guarantee: an oversized rectangle is pinned to the bounds so its origin
        // stays on screen even when it cannot fit.
        result.left = result.left.clamp(bounds.left, (bounds.right - result.width()).max(bounds.left));
        result.top = result.top.clamp(bounds.top, (bounds.bottom - result.height()).max(bounds.top));
        result.right = result.left + result.width();
        result.bottom = result.top + result.height();
        result
    }

    pub fn intersect(&self, other: Rect) -> Self {
        Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        }
    }

    pub fn union(&self, other: Rect) -> Self {
        Self {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }

    /// Four rectangles covering everything inside `self` but outside `hole`.
    /// Used by the L1 mask so the selected pixels keep the raw back-buffer
    /// brightness instead of being re-drawn.
    pub fn surround(&self, hole: Rect) -> [Rect; 4] {
        let hole = hole.intersect(*self);
        if hole.is_empty() {
            return [*self, Rect::default(), Rect::default(), Rect::default()];
        }
        [
            Rect::new(self.left, self.top, self.right, hole.top),
            Rect::new(self.left, hole.bottom, self.right, self.bottom),
            Rect::new(self.left, hole.top, hole.left, hole.bottom),
            Rect::new(hole.right, hole.top, self.right, hole.bottom),
        ]
    }
}

/// Pixel size of an image payload.
///
/// Serialised to the front end, so the field names and the `camelCase` rename are
/// part of the IPC contract — do not change them without changing
/// `src/shared/contracts.ts`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageDimensions {
    pub width: u32,
    pub height: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ——— 与 `capture/geometry.rs` 逐字一致的回归（搬移时原样带过来） ———

    #[test]
    fn drag_in_any_direction_normalises_the_rectangle() {
        let corners = [
            (Point::new(100, 200), Point::new(400, 500)),
            (Point::new(400, 500), Point::new(100, 200)),
            (Point::new(400, 200), Point::new(100, 500)),
            (Point::new(100, 500), Point::new(400, 200)),
        ];
        for (start, end) in corners {
            let rect = Rect::from_corners(start, end);
            assert_eq!(rect, Rect::new(100, 200, 400, 500), "{start:?} -> {end:?}");
            assert!(rect.left <= rect.right && rect.top <= rect.bottom);
        }
    }

    #[test]
    fn rectangles_surrounding_a_hole_cover_the_mask() {
        let frame = Rect::new(0, 0, 100, 100);
        let hole = Rect::new(20, 30, 60, 80);
        let bands = frame.surround(hole);
        let covered = bands.iter().map(Rect::area).sum::<i64>();
        assert_eq!(covered, frame.area() - hole.area());
        for band in bands {
            assert_eq!(band.intersect(hole).area(), 0, "{band:?} overlaps the hole");
        }
    }

    #[test]
    fn surrounds_with_no_hole_returns_the_whole_frame() {
        let frame = Rect::new(0, 0, 100, 100);
        let bands = frame.surround(Rect::default());
        assert_eq!(bands[0], frame);
        assert_eq!(bands[1].area(), 0);
    }

    // ——— 值对象自身的边界（负坐标 / 包含关系 / 尺寸零值） ———

    #[test]
    fn monitors_left_of_the_origin_keep_negative_coordinates() {
        // A second display placed to the left of the primary one lives at negative x.
        let rect = Rect::from_corners(Point::new(-1920, 0), Point::new(-100, 1080));
        assert_eq!(rect, Rect::new(-1920, 0, -100, 1080));
        assert_eq!(rect.width(), 1820);
        assert_eq!(rect.height(), 1080);
        assert!(rect.contains(Point::new(-1920, 0)));
        assert!(rect.contains(Point::new(-101, 1079)));
        assert!(!rect.contains(Point::new(-100, 0)), "right/bottom are exclusive");
        assert!(!rect.contains(Point::new(-1921, 0)));
    }

    #[test]
    fn a_rect_below_or_left_of_the_origin_still_measures_its_size() {
        let rect = Rect::new(-500, -320, -100, -20);
        assert_eq!(rect.width(), 400);
        assert_eq!(rect.height(), 300);
        assert_eq!(rect.area(), 120_000);
        assert_eq!(rect.center(), Point::new(-300, -170));
    }

    #[test]
    fn width_is_signed_and_is_empty_is_the_guard_against_inverted_rects() {
        // `saturating_sub` only guards against i32 overflow — it does **not** clamp to
        // zero, so an inverted rect (right < left) reports a *negative* size and
        // `is_empty()` is the only thing that rejects it. Behaviour preserved verbatim
        // from the original `capture/geometry.rs`; this test pins it so a later
        // "cleanup" that clamps to zero fails loudly instead of changing semantics.
        let inverted = Rect::new(400, 300, 100, 50);
        assert_eq!(inverted.width(), -300);
        assert_eq!(inverted.height(), -250);
        assert!(inverted.is_empty());
        // Trap for callers: area() multiplies two negatives, so an inverted rect has a
        // *positive* area. Always check `is_empty()` before using `area()`.
        assert_eq!(inverted.area(), 75_000);

        assert!(Rect::default().is_empty());
        assert!(!Rect::new(0, 0, 1, 1).is_empty(), "one pixel is not empty");
    }

    #[test]
    fn containment_counts_touching_edges_and_rejects_oversized_rects() {
        let outer = Rect::new(10, 20, 110, 120);
        // Touching edges count as contained: the ancestor walk relies on this.
        assert!(outer.contains_rect(Rect::new(10, 20, 110, 120)));
        assert!(outer.contains_rect(Rect::new(10, 20, 110, 119)));
        assert!(!outer.contains_rect(Rect::new(9, 20, 110, 120)));
        assert!(!outer.contains_rect(Rect::new(10, 20, 111, 120)));
        // An oversize rect that merely overlaps is not contained.
        assert!(!outer.contains_rect(Rect::new(0, 0, 200, 200)));
        // Negative-coordinate nesting behaves the same way.
        let negative = Rect::new(-100, -100, -50, -50);
        assert!(negative.contains_rect(Rect::new(-90, -90, -60, -60)));
        assert!(negative.contains(Point::new(-100, -100)));
    }

    #[test]
    fn inflate_and_translate_move_exactly_the_requested_amount() {
        let rect = Rect::new(100, 100, 200, 200);
        assert_eq!(rect.inflate(5), Rect::new(95, 95, 205, 205));
        assert_eq!(rect.inflate(-5), Rect::new(105, 105, 195, 195));
        assert_eq!(rect.translate(Point::new(-10, 20)), Rect::new(90, 120, 190, 220));
        // Inflating a negative-coordinate rect moves away from the origin, not toward it.
        let negative = Rect::new(-200, -200, -100, -100);
        assert_eq!(negative.inflate(10), Rect::new(-210, -210, -90, -90));
    }

    #[test]
    fn clamp_keeps_the_size_and_pins_oversized_rects_inside_the_bounds() {
        let bounds = Rect::new(0, 0, 1920, 1080);
        let inside = Rect::new(100, 100, 200, 200);
        assert_eq!(inside.clamped_into(bounds), inside);

        let past_the_left = Rect::new(-50, 100, 50, 200);
        let clamped = past_the_left.clamped_into(bounds);
        assert_eq!(clamped, Rect::new(0, 100, 100, 200));
        assert_eq!(clamped.width(), past_the_left.width());

        // Wider than the monitor: pinned to the left edge, size preserved.
        let too_wide = Rect::new(-3000, 10, -300, 110);
        let pinned = too_wide.clamped_into(bounds);
        assert_eq!(pinned.left, 0);
        assert_eq!(pinned.width(), too_wide.width());
    }

    #[test]
    fn intersect_and_union_cover_the_disjoint_and_overlapping_cases() {
        let a = Rect::new(0, 0, 100, 100);
        assert_eq!(a.intersect(a), a);
        assert_eq!(a.union(a), a);

        // Overlapping.
        let b = Rect::new(50, 50, 150, 150);
        assert_eq!(a.intersect(b), Rect::new(50, 50, 100, 100));
        assert_eq!(a.union(b), Rect::new(0, 0, 150, 150));

        // Disjoint: intersection is empty, union spans both.
        let c = Rect::new(200, 200, 300, 300);
        assert!(a.intersect(c).is_empty());
        assert_eq!(a.union(c), Rect::new(0, 0, 300, 300));
    }

    #[test]
    fn image_dimensions_keep_their_wire_format() {
        // The front end reads `imageDimensions: { width, height }`, and the rename is
        // there for consistency with the sibling payload types. Pin the exact JSON so
        // a derive/rename edit cannot silently change the IPC contract
        // (src/shared/contracts.ts).
        let json = serde_json::to_string(&ImageDimensions { width: 1920, height: 1080 }).unwrap();
        assert_eq!(json, r#"{"width":1920,"height":1080}"#);
    }
}
