//! Pure selection geometry: normalisation, handle hit testing, resize/move math,
//! size-label placement and magnifier placement.
//!
//! Everything in this module works in **physical pixels of one monitor's client
//! area**. The overlay window is borderless and exactly as large as the monitor, so
//! screen coordinates, client coordinates and back-buffer coordinates differ only by
//! the monitor origin. Keeping the maths here means DPI, multi-monitor and handle
//! behaviour can be unit tested without a window station.

use super::CaptureError;

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

/// One monitor's physical geometry and DPI.
///
/// The display device name is deliberately not carried here: it is only needed for
/// diagnostics, and reading it requires `MONITORINFOEXW` by value, which the
/// `windows-sys` bindings do not expose directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorLayout {
    /// Monitor rectangle in virtual-desktop physical pixels.
    pub bounds: Rect,
    /// Work area in virtual-desktop physical pixels (taskbar excluded).
    pub work_area: Rect,
    /// Effective DPI as reported by `GetDpiForMonitor` (96 = 100%).
    pub dpi: u32,
    pub primary: bool,
}

impl MonitorLayout {
    /// Scale factor from DIP to physical pixels.
    pub fn scale(&self) -> f32 {
        self.dpi as f32 / 96.0
    }

    /// Convert a virtual-desktop physical point into monitor-local physical pixels.
    pub fn to_local(&self, screen: Point) -> Point {
        Point::new(screen.x - self.bounds.left, screen.y - self.bounds.top)
    }

    /// Convert monitor-local physical pixels back into virtual-desktop coordinates.
    pub fn to_screen(&self, local: Point) -> Point {
        Point::new(local.x + self.bounds.left, local.y + self.bounds.top)
    }

    pub fn local_bounds(&self) -> Rect {
        Rect::from_origin_size(Point::new(0, 0), self.bounds.width(), self.bounds.height())
    }

    pub fn local_work_area(&self) -> Rect {
        Rect::new(
            self.work_area.left - self.bounds.left,
            self.work_area.top - self.bounds.top,
            self.work_area.right - self.bounds.left,
            self.work_area.bottom - self.bounds.top,
        )
    }
}

/// The eight resize grips, numbered clockwise from the top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl Handle {
    pub const ALL: [Handle; 8] = [
        Handle::TopLeft,
        Handle::Top,
        Handle::TopRight,
        Handle::Right,
        Handle::BottomRight,
        Handle::Bottom,
        Handle::BottomLeft,
        Handle::Left,
    ];

    /// Anchor point of the grip in monitor-local physical pixels.
    pub fn anchor(self, rect: Rect) -> Point {
        let mid_x = rect.left + rect.width() / 2;
        let mid_y = rect.top + rect.height() / 2;
        match self {
            Handle::TopLeft => Point::new(rect.left, rect.top),
            Handle::Top => Point::new(mid_x, rect.top),
            Handle::TopRight => Point::new(rect.right, rect.top),
            Handle::Right => Point::new(rect.right, mid_y),
            Handle::BottomRight => Point::new(rect.right, rect.bottom),
            Handle::Bottom => Point::new(mid_x, rect.bottom),
            Handle::BottomLeft => Point::new(rect.left, rect.bottom),
            Handle::Left => Point::new(rect.left, mid_y),
        }
    }

    /// Logical size of the grip in DIP. Visual size and hit size are independent;
    /// callers scale the hit size with DPI.
    pub fn visual_size(self) -> f32 {
        match self {
            Handle::Top
            | Handle::Right
            | Handle::Bottom
            | Handle::Left => 10.0,
            Handle::TopLeft | Handle::TopRight | Handle::BottomRight | Handle::BottomLeft => 12.0,
        }
    }
}

/// The four corner grips, checked before the edge grips.
pub const CORNER_HANDLES: [Handle; 4] = [
    Handle::TopLeft,
    Handle::TopRight,
    Handle::BottomRight,
    Handle::BottomLeft,
];

/// The four edge grips.
pub const EDGE_HANDLES: [Handle; 4] =
    [Handle::Top, Handle::Right, Handle::Bottom, Handle::Left];

/// Which edges of the selection the pointer is close to; used for the resize cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Edge {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl Edge {
    pub fn any(self) -> bool {
        self.left || self.right || self.top || self.bottom
    }

    /// Handle corresponding to an active edge pair, if exactly one is defined.
    pub fn handle(self) -> Option<Handle> {
        match (self.left, self.right, self.top, self.bottom) {
            (true, false, true, false) => Some(Handle::TopLeft),
            (false, false, true, false) => Some(Handle::Top),
            (false, true, true, false) => Some(Handle::TopRight),
            (false, true, false, false) => Some(Handle::Right),
            (false, true, false, true) => Some(Handle::BottomRight),
            (false, false, false, true) => Some(Handle::Bottom),
            (true, false, false, true) => Some(Handle::BottomLeft),
            (true, false, false, false) => Some(Handle::Left),
            _ => None,
        }
    }
}

/// How a drag mutates an existing selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeMode {
    /// Dragging inside the selection translates it.
    Move,
    /// Dragging a grip changes one or two edges.
    Handle(Handle),
}

/// Result of hit testing the pointer against the current selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionGeometry {
    /// Nothing selected yet: the drag creates a new selection.
    Create,
    Move,
    Resize(Handle),
    /// The selection exists but the pointer is outside it.
    Outside,
}

/// One immutable view of the selection used by rendering and hit testing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionSnapshot {
    pub rect: Rect,
    /// Corner radius in physical pixels.
    pub corner_radius: i32,
    /// Hit tolerance in physical pixels (already DPI scaled).
    pub hit_slop: i32,
}

impl SelectionSnapshot {
    pub fn new(rect: Rect, dpi: u32) -> Self {
        let scale = dpi.max(96) as f32 / 96.0;
        Self {
            rect,
            corner_radius: (8.0 * scale).round() as i32,
            hit_slop: (6.0 * scale).round() as i32,
        }
    }

    pub fn visual_handle_size(&self, handle: Handle, dpi: u32) -> f32 {
        let scale = dpi.max(96) as f32 / 96.0;
        handle.visual_size() * scale
    }

    /// Keep the effective grip size usable on high DPI displays.
    pub fn hit_handle_size(&self, handle: Handle, dpi: u32) -> i32 {
        (self.visual_handle_size(handle, dpi) + self.hit_slop as f32).round() as i32
    }

    /// Hit test a monitor-local point.
    ///
    /// Priority is deliberate:
    /// 1. an empty selection starts a new one;
    /// 2. the four corner grips win over everything, because a small selection can sit
    ///    entirely inside a corner's hit area;
    /// 3. the border itself is grabbable along the edges;
    /// 4. the interior moves the selection.
    pub fn hit_test(&self, point: Point, dpi: u32) -> SelectionGeometry {
        if self.rect.is_empty() {
            return SelectionGeometry::Create;
        }
        for handle in CORNER_HANDLES {
            let anchor = handle.anchor(self.rect);
            let size = self.hit_handle_size(handle, dpi);
            if (point.x - anchor.x).abs() <= size && (point.y - anchor.y).abs() <= size {
                return SelectionGeometry::Resize(handle);
            }
        }
        let edges = self.edges_at(point);
        if edges.any() {
            return SelectionGeometry::Resize(
                edges.handle().unwrap_or(Handle::BottomRight),
            );
        }
        for handle in EDGE_HANDLES {
            let anchor = handle.anchor(self.rect);
            let size = self.hit_handle_size(handle, dpi);
            if (point.x - anchor.x).abs() <= size && (point.y - anchor.y).abs() <= size {
                return SelectionGeometry::Resize(handle);
            }
        }
        if self.rect.contains(point) {
            return SelectionGeometry::Move;
        }
        SelectionGeometry::Outside
    }

    /// Which selection edges the pointer is within `hit_slop` of.
    pub fn edges_at(&self, point: Point) -> Edge {
        if self.rect.is_empty() {
            return Edge::default();
        }
        let slop = self.hit_slop.max(1);
        Edge {
            left: (point.x - self.rect.left).abs() <= slop,
            right: (point.x - self.rect.right).abs() <= slop,
            top: (point.y - self.rect.top).abs() <= slop,
            bottom: (point.y - self.rect.bottom).abs() <= slop,
        }
    }

    /// Enforce minimum size while keeping the dragged edges pinned.
    pub fn with_minimum_size(&self, rect: Rect, mode: ResizeMode, minimum: i32) -> Rect {
        let minimum = minimum.max(1);
        let mut result = rect;
        match mode {
            ResizeMode::Move => {}
            ResizeMode::Handle(handle) => {
                let grows_left = matches!(
                    handle,
                    Handle::TopLeft | Handle::BottomLeft | Handle::Left
                );
                let grows_top = matches!(handle, Handle::TopLeft | Handle::Top | Handle::TopRight);
                if result.width() < minimum {
                    if grows_left {
                        result.left = result.right - minimum;
                    } else {
                        result.right = result.left + minimum;
                    }
                }
                if result.height() < minimum {
                    if grows_top {
                        result.top = result.bottom - minimum;
                    } else {
                        result.bottom = result.top + minimum;
                    }
                }
            }
        }
        result
    }
}

/// Snapshot plus the pointer position where the current drag began.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionDrag {
    pub snapshot: SelectionSnapshot,
    pub anchor: Point,
}

impl SelectionDrag {
    /// Build a drag session from the selection and the pointer position that
    /// started it.
    pub fn new(rect: Rect, dpi: u32, anchor: Point) -> Self {
        Self {
            snapshot: SelectionSnapshot::new(rect, dpi),
            anchor,
        }
    }

    /// Apply a drag and clamp the result to the monitor.
    ///
    /// Resizing is edge based rather than delta based: the dragged edges follow the
    /// pointer exactly, which keeps the geometry stable when the drag crosses over
    /// the opposite edge or leaves the monitor.
    ///
    /// Moving translates and then re-fits, so the selection keeps its size even when
    /// it is larger than the monitor.
    pub fn apply(&self, mode: ResizeMode, pointer: Point, bounds: Rect) -> Rect {
        let rect = self.snapshot.rect;
        let candidate = match mode {
            ResizeMode::Move => {
                return rect
                    .translate(Point::new(
                        pointer.x - self.anchor.x,
                        pointer.y - self.anchor.y,
                    ))
                    .clamped_into(bounds);
            }
            ResizeMode::Handle(handle) => {
                let mut left = rect.left;
                let mut top = rect.top;
                let mut right = rect.right;
                let mut bottom = rect.bottom;
                match handle {
                    Handle::TopLeft => {
                        left = pointer.x;
                        top = pointer.y;
                    }
                    Handle::Top => top = pointer.y,
                    Handle::TopRight => {
                        right = pointer.x;
                        top = pointer.y;
                    }
                    Handle::Right => right = pointer.x,
                    Handle::BottomRight => {
                        right = pointer.x;
                        bottom = pointer.y;
                    }
                    Handle::Bottom => bottom = pointer.y,
                    Handle::BottomLeft => {
                        left = pointer.x;
                        bottom = pointer.y;
                    }
                    Handle::Left => left = pointer.x,
                }
                // Normalise: dragging an edge past the opposite edge flips the
                // rectangle instead of producing a negative width.
                Rect::from_corners(Point::new(left, top), Point::new(right, bottom))
            }
        };
        let candidate = clamp_edges(candidate, bounds);
        self.snapshot
            .with_minimum_size(candidate, mode, self.snapshot.minimum_size())
    }
}

impl SelectionSnapshot {
    pub fn minimum_size(&self) -> i32 {
        self.hit_slop.max(1) * 2
    }
}

fn clamp_edges(rect: Rect, bounds: Rect) -> Rect {
    Rect {
        left: rect.left.clamp(bounds.left, bounds.right),
        top: rect.top.clamp(bounds.top, bounds.bottom),
        right: rect.right.clamp(bounds.left, bounds.right),
        bottom: rect.bottom.clamp(bounds.top, bounds.bottom),
    }
}

/// Where the magnifier panel is drawn, in monitor-local physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MagnifierGeometry {
    /// Image area containing the magnified frame.
    pub panel: Rect,
    /// Dark metadata strip below the image area (swatch, hex, coordinates).
    pub info_panel: Rect,
    /// Union of image and metadata areas, used for damage tracking.
    pub bounds: Rect,
    /// Source rectangle sampled from the frozen back buffer (fixed source window).
    /// Always exactly cursor-centred — it may extend past `frame` at a screen
    /// edge, which is what keeps the cursor's own pixel in the panel's centre
    /// cell; the renderer clips it against the real bitmap bounds instead of
    /// translating the whole rect.
    pub source: Rect,
    /// Currently active zoom level (physical pixels per source pixel; < 1 means
    /// the loupe is downscaling).
    pub zoom: f32,
    /// Whether the panel was flipped horizontally / vertically to stay visible.
    pub flipped_x: bool,
    pub flipped_y: bool,
    /// The exact pixel coordinate being sampled (cursor clamped into frame).
    pub center: Point,
    /// 32×32 tile in texture coordinates covering the cursor; used for async
    /// staging color sampling. Cursor may move within this region without
    /// triggering a new GPU copy ("tile hit").
    pub tile: Rect,
}

/// Fixed magnifier configuration for Phase 4. `zoom`, the source window,
/// `tile_size`, the loupe panel and the info strip are in physical pixels and
/// are **not** scaled by DPI — the magnifier is a fixed-size widget working on
/// the raw pixel grid, and its info text must fit that DPI-invariant panel.
/// Only `gap` (the cursor offset) scales.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MagnifierConfig {
    pub zoom: f32,
    /// Columns of the magnifier source window (17 default → vertical band lands
    /// on column 9, the exact centre).
    pub source_width: i32,
    /// Rows of the magnifier source window (9 default → horizontal band lands
    /// on row 5, the exact centre).
    pub source_height: i32,
    /// Staging tile for async color sampling (32×32 BGRA).
    pub tile_size: i32,
    /// Offset between cursor and panel edge.
    pub gap: i32,
    /// Height of the info strip below the panel, in physical pixels (matches
    /// the DPI-invariant panel). Three rows — a colour+coordinates line and a
    /// 2×2 shortcut grid — vertically centred:
    /// 12+18+12+22+8+22+12 = 106 → rounded up to 108 px.
    pub info_height: i32,
}

impl Default for MagnifierConfig {
    fn default() -> Self {
        Self {
            // 20 px per source pixel keeps a single pixel visibly square on a
            // physical-pixel display; a 17×9 source window (153 cells) keeps the
            // sampled pixel exactly on column 9 / row 5 — the panel's centre —
            // while giving a wide, letterbox-shaped loupe (340×180 physical px).
            zoom: Self::ZOOM_DEFAULT,
            source_width: 17,
            source_height: 9,
            tile_size: 32,
            gap: 24,
            info_height: 108,
        }
    }
}

impl MagnifierConfig {
    /// Physical width/height the loupe panel keeps at every zoom step. The
    /// source window is derived from these so the loupe never visually jumps
    /// when the user adjusts zoom (`Z` + wheel) — only the pixel grid density
    /// changes.
    pub const PANEL_WIDTH_PHYSICAL: i32 = 340;
    pub const PANEL_HEIGHT_PHYSICAL: i32 = 180;
    pub const ZOOM_DEFAULT: f32 = 20.0;
    pub const ZOOM_MIN: f32 = 0.1;
    pub const ZOOM_MAX: f32 = 40.0;

    /// Predefined zoom ladder — 17 stops from 0.1× to 40×.
    /// Wheel up advances, wheel down retreats.
    pub const ZOOM_LEVELS: &'static [f32] = &[
        0.1, 0.2, 0.3, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 10.0, 15.0, 20.0, 30.0,
        40.0,
    ];

    /// Return the next higher (or lower) zoom stop from `current`.
    /// `direction > 0` → up, `direction < 0` → down, 0 → unchanged.
    pub fn zoom_step(current: f32, direction: i32) -> f32 {
        if direction == 0 {
            return current;
        }
        let levels = Self::ZOOM_LEVELS;
        let idx = levels
            .iter()
            .position(|&z| (z - current).abs() < f32::EPSILON)
            .unwrap_or_else(|| {
                // Nearest stop if current is off-ladder.
                levels
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        ((**a - current).abs()).partial_cmp(&(**b - current).abs()).unwrap()
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            });
        if direction > 0 {
            levels[idx.saturating_add(1).min(levels.len() - 1)]
        } else {
            levels[idx.saturating_sub(1)]
        }
    }

    /// Build a config for a specific `zoom` level while keeping the loupe panel
    /// at [`Self::PANEL_WIDTH_PHYSICAL`] × [`Self::PANEL_HEIGHT_PHYSICAL`].
    /// The source window is `panel / zoom`, floored and clamped to at least
    /// 1 cell; the panel's actual physical size may drift by up to
    /// `zoom - 1` pixels due to integer rounding — imperceptible in practice.
    pub fn with_zoom(zoom: f32) -> Self {
        let zoom = zoom.clamp(Self::ZOOM_MIN, Self::ZOOM_MAX);
        let source_width = ((Self::PANEL_WIDTH_PHYSICAL as f32 / zoom).round() as i32).max(1);
        let source_height = ((Self::PANEL_HEIGHT_PHYSICAL as f32 / zoom).round() as i32).max(1);
        Self {
            zoom,
            source_width,
            source_height,
            ..Self::default()
        }
    }

    /// Scale only DPI-dependent fields. Source, tile, and the loupe panel are
    /// fixed physical pixels (the panel is 340×180 regardless of DPI), so the
    /// info strip height stays physical too — its text is laid out in physical
    /// pixels to fit that DPI-invariant panel. Only `gap` scales.
    pub fn scaled(&self, dpi: u32) -> Self {
        let scale = dpi.max(96) as f32 / 96.0;
        Self {
            zoom: self.zoom,
            source_width: self.source_width,
            source_height: self.source_height,
            tile_size: self.tile_size,
            gap: (self.gap as f32 * scale).round() as i32,
            info_height: self.info_height,
        }
    }

    /// Physical pixel width of the magnified image panel.
    ///
    /// Always returns the fixed target; the source window adapts to zoom,
    /// not the other way around.
    pub fn panel_width(&self) -> i32 {
        Self::PANEL_WIDTH_PHYSICAL
    }

    /// Physical pixel height of the magnified image panel.
    pub fn panel_height(&self) -> i32 {
        Self::PANEL_HEIGHT_PHYSICAL
    }
}

/// Place the magnifier next to the cursor, flipping and clamping to stay on screen.
///
/// The panel never covers the cursor hotspot: a gap the size of the configured
/// offset is kept between the hotspot and the nearest panel edge.
///
/// Source rect is always exactly `config.source_width × config.source_height`, and
/// always centred on `center` — even at a frame edge, so the cursor's own pixel
/// maps to the panel's centre cell. The renderer clips it against the bitmap
/// bounds; whatever falls outside the frame is simply not drawn (see
/// `OverlayRenderer::draw_magnifier`), instead of translating the whole source
/// rect off-centre to keep it fully inside the frame.
pub fn magnifier_geometry(
    cursor: Point,
    config: MagnifierConfig,
    frame: Rect,
    work_area: Rect,
) -> MagnifierGeometry {
    let zoom = config.zoom.max(0.1);
    let panel_w = config.panel_width().max(1);
    let panel_h = config.panel_height().max(1);
    let source_width = config.source_width.max(1);
    let source_height = config.source_height.max(1);
    let tile_size = config.tile_size.max(source_width.max(source_height)); // tile >= source
    let half_w = source_width / 2;
    let half_h = source_height / 2;

    let info_height = config.info_height.max(1);
    let total_height = panel_h + info_height;

    // Horizontal placement: default right of cursor, flip left if clipped.
    let right_edge = cursor.x + config.gap + panel_w;
    let left_edge = cursor.x - config.gap - panel_w;
    let flipped_x = right_edge > work_area.right && left_edge >= work_area.left;
    let mut left = if flipped_x {
        cursor.x - config.gap - panel_w
    } else {
        cursor.x + config.gap
    };

    // Vertical placement: default below cursor, flip above if clipped.
    let bottom_edge = cursor.y + config.gap + total_height;
    let top_edge = cursor.y - config.gap - total_height;
    let flipped_y = bottom_edge > work_area.bottom && top_edge >= work_area.top;
    let mut top = if flipped_y {
        cursor.y - config.gap - total_height
    } else {
        cursor.y + config.gap
    };

    left = left.clamp(work_area.left, (work_area.right - panel_w).max(work_area.left));
    top = top.clamp(work_area.top, (work_area.bottom - total_height).max(work_area.top));
    let panel = Rect::from_origin_size(Point::new(left, top), panel_w, panel_h);
    let info_panel = Rect::from_origin_size(Point::new(left, top + panel_h), panel_w, info_height);
    let bounds = panel.union(info_panel);

    // Center pixel: the actual cursor position clamped to frame bounds.
    let center = Point::new(
        cursor.x.clamp(frame.left, frame.right - 1),
        cursor.y.clamp(frame.top, frame.bottom - 1),
    );

    // Source rect: fixed source_width×source_height, always exactly centred on the
    // cursor — deliberately NOT clamped to the frame. At a screen edge the
    // cursor's own pixel must stay in the panel's centre cell; the out-of-frame
    // margin is clipped by the renderer instead of shifting the whole source.
    let source_left = center.x - half_w;
    let source_top = center.y - half_h;
    let source = Rect::from_origin_size(
        Point::new(source_left, source_top),
        source_width,
        source_height,
    );

    // Tile: tile_size×tile_size region containing the cursor for async staging copy.
    // Translated at frame edges to maintain fixed tile_size.
    let tile_half = tile_size / 2;
    let tile_left = (center.x - tile_half)
        .clamp(frame.left, (frame.right - tile_size).max(frame.left));
    let tile_top = (center.y - tile_half)
        .clamp(frame.top, (frame.bottom - tile_size).max(frame.top));
    let tile = Rect::from_origin_size(
        Point::new(tile_left, tile_top),
        tile_size,
        tile_size,
    );

    MagnifierGeometry {
        panel,
        info_panel,
        bounds,
        source,
        zoom,
        flipped_x,
        flipped_y,
        center,
        tile,
    }
}

/// Half-length of the pointer reticle's guide segments, in DIP.
///
/// The crosshair is a *local* precision mark centred on the cursor, not a full-frame
/// guide. Keeping it bounded is what stops a hover from invalidating the whole surface
/// (docs/11 §5.1: 十字线默认限制为局部准星).
const CROSSHAIR_RADIUS_DIP: f32 = 28.0;

/// Reticle radius in physical pixels for a display DPI.
pub fn crosshair_radius(dpi: u32) -> i32 {
    (CROSSHAIR_RADIUS_DIP * (dpi.max(96) as f32 / 96.0)).round() as i32
}

/// The local crosshair footprint for a cursor at `cursor`, clamped to `frame`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrosshairGeometry {
    /// Horizontal 1px guide segment (right edge exclusive).
    pub horizontal: Rect,
    /// Vertical 1px guide segment (bottom edge exclusive).
    pub vertical: Rect,
    /// Union of the two segments, used for damage tracking.
    pub bounds: Rect,
}

/// Compute the local "准星" segments for `cursor`.
///
/// One implementation is shared by the draw code (`draw_magnifier`) and the invalidation
/// code (`cursor_damage`) so a hover repaints exactly the reticle box and never unions to
/// the full monitor.
pub fn crosshair_geometry(cursor: Point, radius: i32, frame: Rect) -> CrosshairGeometry {
    let radius = radius.max(0);
    let left = (cursor.x - radius).max(frame.left);
    let right = (cursor.x + radius + 1).min(frame.right);
    let top = (cursor.y - radius).max(frame.top);
    let bottom = (cursor.y + radius + 1).min(frame.bottom);
    let horizontal = Rect::new(left, cursor.y, right, cursor.y + 1);
    let vertical = Rect::new(cursor.x, top, cursor.x + 1, bottom);
    CrosshairGeometry {
        horizontal,
        vertical,
        bounds: Rect::new(left, top, right, bottom),
    }
}

/// Placement of the `width × height` label relative to the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeLabelPlacement {
    pub rect: Rect,
    /// Which step of the ladder produced it (docs/21 §5.22).
    pub where_: LabelWhere,
}

/// Where a label ended up, for the ladder's own diagnostics and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelWhere {
    /// Outside, above the box — the normal case.
    Above,
    /// Outside, below the box — the box is against the top edge.
    Below,
    /// **Inside** the box, top-left: the box spans the work area, so there is nowhere outside.
    /// A maximised window is exactly this, and it used to mean the label was dropped entirely.
    Inside,
    /// Beside the cursor, when the box can hold neither the label nor a place outside.
    AtCursor,
}

/// Place the size label near `selection`'s top-left, ladder-style (docs/21 §5.22).
///
/// 1. outside, above; 2. outside, below; 3. **inside** the box when the box is as large as the work
/// area (a maximised window has no room outside, and dropping the label there loses the only words
/// that say "窗口"); 4. beside the cursor, for a box that is both full-height and too narrow.
///
/// `cursor` is only consulted for step 4; pass `None` and the label is dropped instead, which is
/// what callers that have no cursor (exports, tests) want.
pub fn size_label_placement(
    selection: Rect,
    label_size: (i32, i32),
    work_area: Rect,
    gap: i32,
    cursor: Option<Point>,
) -> Option<SizeLabelPlacement> {
    let (label_width, label_height) = (label_size.0.max(1), label_size.1.max(1));
    let above_top = selection.top - gap - label_height;
    let above_fits = above_top >= work_area.top;
    let below_top = selection.bottom + gap;
    let below_fits = below_top + label_height <= work_area.bottom;

    let (mut left, mut top, where_) = if above_fits {
        (selection.left, above_top, LabelWhere::Above)
    } else if below_fits {
        (selection.left, below_top, LabelWhere::Below)
    } else if label_width + 2 * gap <= selection.width()
        && label_height + 2 * gap <= selection.height()
    {
        // Inside the box: it covers a corner of the content for as long as the preview lasts, and
        // never reaches the artifact (`render_export` clears the preview layers).
        (
            selection.left + gap,
            selection.top + gap,
            LabelWhere::Inside,
        )
    } else {
        // Step 4 only counts if the label actually *fits* beside the cursor. Clamping it into a
        // frame it does not fit in paints the panel over the whole selection instead of dropping it,
        // which is worse than having no label at all — so no fit, no label.
        let cursor = cursor?;
        let right = cursor.x + gap + 4;
        let left_to_the_right = right + label_width <= work_area.right;
        let left_to_the_left = cursor.x - gap - 4 - label_width >= work_area.left;
        let below = cursor.y + gap + 4;
        let below_fits = below + label_height <= work_area.bottom;
        let above_fits = cursor.y - gap - 4 - label_height >= work_area.top;
        let left = if left_to_the_right {
            right
        } else if left_to_the_left {
            cursor.x - gap - 4 - label_width
        } else {
            return None;
        };
        let top = if below_fits {
            below
        } else if above_fits {
            cursor.y - gap - 4 - label_height
        } else {
            return None;
        };
        (left, top, LabelWhere::AtCursor)
    };

    let max_left = (work_area.right - label_width).max(work_area.left);
    left = left.clamp(work_area.left, max_left);
    let max_top = (work_area.bottom - label_height).max(work_area.top);
    top = top.clamp(work_area.top, max_top);

    Some(SizeLabelPlacement {
        rect: Rect::from_origin_size(Point::new(left, top), label_width, label_height),
        where_,
    })
}

/// Where the level badge sits relative to the box (docs/21 §5.22).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipWhere {
    /// Inside the box's top-right corner — its home: the badge *is* the box's place on the chain.
    Inside,
    /// Outside, top-right, when the box is too small to hold it.
    OutsideAbove,
    /// Outside, bottom-right, when the box is also against the top edge.
    OutsideBelow,
    /// Beside the cursor, for a box with no room on any side.
    AtCursor,
}

/// Place the level badge near the box's top-right corner, ladder-style (docs/21 §5.22).
///
/// The badge is much smaller than the size label (a dot strip, ~75×18 for nine levels), which is why
/// "the box is too small" almost never comes up: a box 87 px wide and 30 px tall holds it, and
/// everything smaller still has room outside for something that size.
pub fn level_badge_placement(
    selection: Rect,
    badge_size: (i32, i32),
    work_area: Rect,
    gap: i32,
    cursor: Option<Point>,
) -> Option<(Rect, ChipWhere)> {
    let (width, height) = (badge_size.0.max(1), badge_size.1.max(1));
    let right_edge = selection.left + selection.width();
    let place = |left: i32, top: i32| {
        let max_left = (work_area.right - width).max(work_area.left);
        let max_top = (work_area.bottom - height).max(work_area.top);
        Rect::from_origin_size(
            Point::new(
                left.clamp(work_area.left, max_left),
                top.clamp(work_area.top, max_top),
            ),
            width,
            height,
        )
    };
    if width + 2 * gap <= selection.width() && height + 2 * gap <= selection.height() {
        return Some((
            place(right_edge - gap - width, selection.top + gap),
            ChipWhere::Inside,
        ));
    }
    if selection.top - gap - height >= work_area.top {
        return Some((
            place(right_edge - width, selection.top - gap - height),
            ChipWhere::OutsideAbove,
        ));
    }
    if selection.bottom + gap + height <= work_area.bottom {
        return Some((
            place(right_edge - width, selection.bottom + gap),
            ChipWhere::OutsideBelow,
        ));
    }
    let cursor = cursor?;
    let below = cursor.y + gap + 4;
    let below_fits = below + height <= work_area.bottom;
    let above_fits = cursor.y - gap - 4 - height >= work_area.top;
    let top = if below_fits {
        below
    } else if above_fits {
        cursor.y - gap - 4 - height
    } else {
        return None;
    };
    let right = cursor.x + gap + 4;
    let left = if right + width <= work_area.right {
        right
    } else if cursor.x - gap - 4 - width >= work_area.left {
        cursor.x - gap - 4 - width
    } else {
        return None;
    };
    Some((place(left, top), ChipWhere::AtCursor))
}

/// The level badge as a dot strip: `(width, height)` for a chain of `levels` and a 7 px pitch
/// (docs/21 §5.22). Past 12 levels the strip windows itself, so the width has a ceiling.
/// What the level badge shows: how many wheel stops remain in each direction (v3 A1, docs/21 §5.24).
///
/// `up` walks toward the window frame, `down` toward the published answer. Counting *stops* rather
/// than levels is the point: some levels have no ring of their own (they collapse), so the honest
/// answer to "how much further can I scroll" is how many notches land somewhere new.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LevelReach {
    pub up: usize,
    pub down: usize,
}

/// The badge's size in logical pixels (v3's `reach` chip: two cells of `↑n` / `↓n`).
///
/// Fixed rather than derived from the level count: a chip that grows with the chain jumps around
/// while the user is scrolling, and with counts instead of marks there is nothing to lay out.
pub const LEVEL_BADGE_SIZE: (i32, i32) = (54, 18);

/// Clip a virtual-desktop window rectangle to one monitor and return it in that
/// monitor's local physical pixels.
///
/// The window snapshot stores DWM frame bounds in **virtual-desktop physical pixels**
/// (docs/14 §5.1); the overlay paints in **monitor-local physical pixels**. This is the
/// only place that translates between the two, so the conversion is documented once
/// and unit tested with negative virtual-desktop origins.
///
/// A window that spans monitors is deliberately reduced to the part visible on
/// `monitor`: a v1 session only holds a frozen frame of one display, so promising the
/// whole window would promise pixels it cannot export (docs/14 §6.2). The result is
/// empty when the window does not reach this monitor at all.
pub fn window_rect_to_local(win: Rect, monitor: &MonitorLayout) -> Rect {
    let clipped = win.intersect(monitor.bounds);
    if clipped.is_empty() {
        // Normalise a disjoint intersection so callers only have to test `is_empty`.
        return Rect::new(0, 0, 0, 0);
    }
    Rect::new(
        clipped.left - monitor.bounds.left,
        clipped.top - monitor.bounds.top,
        clipped.right - monitor.bounds.left,
        clipped.bottom - monitor.bounds.top,
    )
}

/// Validate that a selection can produce an artifact.
pub fn validate_selection(rect: Rect, frame: Rect) -> Result<Rect, CaptureError> {
    let clipped = rect.intersect(frame);
    if clipped.is_empty() {
        return Err(CaptureError::InvalidState(
            "selection does not overlap the captured monitor".into(),
        ));
    }
    Ok(clipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor() -> Rect {
        Rect::new(0, 0, 1920, 1080)
    }

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
    fn selection_is_clipped_to_the_monitor() {
        let selection = Rect::new(-50, -20, 2500, 1400);
        let clipped = validate_selection(selection, monitor()).unwrap();
        assert_eq!(clipped, monitor());

        let outside = Rect::new(2000, 1200, 2200, 1300);
        assert!(validate_selection(outside, monitor()).is_err());
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

    #[test]
    fn handles_hit_test_at_their_anchor_points() {
        let rect = Rect::new(100, 100, 300, 200);
        let snapshot = SelectionSnapshot::new(rect, 96);
        for handle in Handle::ALL {
            let anchor = handle.anchor(rect);
            assert_eq!(
                snapshot.hit_test(anchor, 96),
                SelectionGeometry::Resize(handle),
                "{handle:?}"
            );
        }
    }

    #[test]
    fn dpi_scales_the_handle_hit_area() {
        let rect = Rect::new(100, 100, 900, 700);
        let point = Point::new(100 + 20, 100 + 20);
        let at_100 = SelectionSnapshot::new(rect, 96).hit_test(point, 96);
        let at_200 = SelectionSnapshot::new(rect, 192).hit_test(point, 192);
        // The visual grip is the same physical size, but the hit area grows with DPI.
        assert_eq!(at_100, SelectionGeometry::Move);
        assert_eq!(at_200, SelectionGeometry::Resize(Handle::TopLeft));
    }

    #[test]
    fn interior_hit_tests_as_move_and_exterior_as_outside() {
        let rect = Rect::new(100, 100, 300, 200);
        let snapshot = SelectionSnapshot::new(rect, 96);
        assert_eq!(
            snapshot.hit_test(Point::new(200, 150), 96),
            SelectionGeometry::Move
        );
        assert_eq!(
            snapshot.hit_test(Point::new(800, 800), 96),
            SelectionGeometry::Outside
        );
    }

    #[test]
    fn empty_snapshot_creates_a_new_selection() {
        let snapshot = SelectionSnapshot::new(Rect::default(), 96);
        assert_eq!(
            snapshot.hit_test(Point::new(10, 10), 96),
            SelectionGeometry::Create
        );
    }

    #[test]
    fn resize_follows_the_pointer_and_clamps_into_the_monitor() {
        let rect = Rect::new(100, 100, 300, 200);
        let drag = SelectionDrag::new(rect, 96, Point::new(300, 200));
        let resized = drag.apply(ResizeMode::Handle(Handle::BottomRight), Point::new(500, 400), monitor());
        assert_eq!(resized, Rect::new(100, 100, 500, 400));

        let clamped = drag.apply(
            ResizeMode::Handle(Handle::BottomRight),
            Point::new(5000, 5000),
            monitor(),
        );
        assert_eq!(clamped, Rect::new(100, 100, 1920, 1080));
    }

    #[test]
    fn resize_can_cross_over_the_opposite_edge() {
        let rect = Rect::new(100, 100, 300, 200);
        let drag = SelectionDrag::new(rect, 96, Point::new(300, 200));
        // Drag the right edge to the left of the left edge: the rectangle must
        // normalise instead of inverting.
        let resized = drag.apply(ResizeMode::Handle(Handle::Right), Point::new(50, 200), monitor());
        assert_eq!(resized, Rect::new(50, 100, 100, 200));
    }

    #[test]
    fn minimum_size_is_enforced_while_resizing() {
        let rect = Rect::new(100, 100, 300, 200);
        let drag = SelectionDrag::new(rect, 96, Point::new(300, 200));
        let tiny = drag.apply(
            ResizeMode::Handle(Handle::BottomRight),
            Point::new(102, 102),
            monitor(),
        );
        assert!(tiny.width() >= drag.snapshot.minimum_size());
        assert!(tiny.height() >= drag.snapshot.minimum_size());
        assert_eq!(tiny.left, 100);
        assert_eq!(tiny.top, 100);
    }

    #[test]
    fn move_keeps_the_size_and_stays_inside_the_monitor() {
        let rect = Rect::new(100, 100, 300, 200);
        let drag = SelectionDrag::new(rect, 96, Point::new(200, 150));
        let moved = drag.apply(ResizeMode::Move, Point::new(250, 200), monitor());
        assert_eq!(moved, Rect::new(150, 150, 350, 250));

        let pinned = drag.apply(ResizeMode::Move, Point::new(-500, -500), monitor());
        assert_eq!(pinned, Rect::new(0, 0, 200, 100));

        let pinned = drag.apply(ResizeMode::Move, Point::new(5000, 5000), monitor());
        assert_eq!(pinned, Rect::new(1720, 980, 1920, 1080));
    }

    #[test]
    fn local_coordinates_follow_the_monitor_origin() {
        let monitor_layout = MonitorLayout {
            bounds: Rect::new(-1920, 200, 0, 1280),
            work_area: Rect::new(-1920, 200, 0, 1240),
            dpi: 144,
            primary: false,
        };
        assert_eq!(monitor_layout.to_local(Point::new(-1920, 200)), Point::new(0, 0));
        assert_eq!(monitor_layout.to_screen(Point::new(0, 0)), Point::new(-1920, 200));
        assert_eq!(monitor_layout.local_bounds(), Rect::new(0, 0, 1920, 1080));
        assert_eq!(monitor_layout.local_work_area(), Rect::new(0, 0, 1920, 1040));
        assert!((monitor_layout.scale() - 1.5).abs() < f32::EPSILON);
    }

    #[test]
    fn size_label_prefers_above_and_aligns_to_left_edge() {
        let work_area = Rect::new(0, 0, 1000, 800);
        let selection = Rect::new(400, 400, 600, 500);
        let above = size_label_placement(selection, (120, 28), work_area, 8, None).unwrap();
        assert_eq!(above.where_, LabelWhere::Above);
        assert_eq!(above.rect.top, 364);
        assert_eq!(above.rect.left, 400);

        // Nothing fits above at the monitor's top edge: place it below.
        let top_selection = Rect::new(400, 5, 600, 50);
        let below = size_label_placement(top_selection, (120, 28), work_area, 8, None).unwrap();
        assert_eq!(below.where_, LabelWhere::Below);
        assert_eq!(below.rect.top, 58);
        assert_eq!(below.rect.left, 400);
    }

    #[test]
    fn the_size_label_ladder_keeps_the_words_the_fallback_needs() {
        let work_area = Rect::new(0, 0, 1000, 800);
        // A selection taller than the work area: outside is impossible, so the label moves *inside*
        // — for a maximised window this is the only place `窗口` can be read (docs/21 §5.22).
        let tall_selection = Rect::new(400, 0, 600, 800);
        let inside = size_label_placement(tall_selection, (120, 28), work_area, 8, None).unwrap();
        assert_eq!(inside.where_, LabelWhere::Inside);
        assert_eq!(inside.rect.left, 408);
        assert_eq!(inside.rect.top, 8);

        // A selection that fills the whole work area: same story.
        let filling = size_label_placement(work_area, (120, 28), work_area, 8, None).unwrap();
        assert_eq!(filling.where_, LabelWhere::Inside);

        // Full height *and* too narrow for the label: only the cursor has room left — and with no
        // cursor the label is dropped rather than painted over the selection.
        let sliver = Rect::new(0, 0, 30, 800);
        assert_eq!(
            size_label_placement(sliver, (120, 28), work_area, 8, None),
            None
        );
        let at_cursor =
            size_label_placement(sliver, (120, 28), work_area, 8, Some(Point::new(600, 400)))
                .unwrap();
        assert_eq!(at_cursor.where_, LabelWhere::AtCursor);
        assert!(
            at_cursor.rect.left > sliver.right,
            "beside the cursor, not over the box"
        );
    }

    #[test]
    fn the_level_badge_prefers_the_boxs_own_corner() {
        let work_area = Rect::new(0, 0, 1000, 800);
        // A normal box: the badge sits inside its top-right corner.
        let roomy = Rect::new(100, 100, 500, 400);
        let (rect, where_) =
            level_badge_placement(roomy, LEVEL_BADGE_SIZE, work_area, 6, None).unwrap();
        assert_eq!(where_, ChipWhere::Inside);
        assert_eq!(rect.right, roomy.right - 6);
        assert_eq!(rect.top, roomy.top + 6);

        // A single-line text run (20 px tall) cannot hold an 18 px badge plus padding: it goes
        // outside, still right-aligned to the box.
        let line = Rect::new(100, 300, 460, 320);
        let (rect, where_) =
            level_badge_placement(line, LEVEL_BADGE_SIZE, work_area, 6, None).unwrap();
        assert_eq!(where_, ChipWhere::OutsideAbove);
        assert_eq!(rect.right, line.right);
        assert_eq!(rect.bottom, line.top - 6);

        // …and against the top edge, below it instead.
        let line_at_top = Rect::new(100, 2, 460, 22);
        let (_, where_) =
            level_badge_placement(line_at_top, LEVEL_BADGE_SIZE, work_area, 6, None).unwrap();
        assert_eq!(where_, ChipWhere::OutsideBelow);

        // The chip's size is fixed (v3 A1): `↑3 ↓5` needs two cells whatever the chain's length, and
        // a badge that grew with the chain would jump under the cursor while the user scrolls.
        assert_eq!(LEVEL_BADGE_SIZE, (54, 18));
    }

    #[test]
    fn size_label_stays_inside_horizontal_work_area_bounds() {
        let work_area = Rect::new(0, 0, 1000, 800);
        let selection = Rect::new(0, 100, 40, 200);
        let placement = size_label_placement(selection, (200, 28), work_area, 8, None).unwrap();
        assert!(placement.rect.left >= work_area.left);
        assert!(placement.rect.right <= work_area.right);
    }

    #[test]
    fn magnifier_flips_on_every_screen_edge_and_stays_visible() {
        let frame = Rect::new(0, 0, 1920, 1080);
        let work_area = frame;
        let config = MagnifierConfig::default().scaled(96);
        let panel_w = config.panel_width();
        let panel_h = config.panel_height();

        // Centre: no flip.
        let centre = magnifier_geometry(Point::new(960, 540), config, frame, work_area);
        assert!(!centre.flipped_x && !centre.flipped_y);
        assert!(centre.panel.right <= work_area.right);
        assert!(centre.panel.bottom <= work_area.bottom);

        // Right edge: flip horizontally.
        let right = magnifier_geometry(Point::new(1910, 540), config, frame, work_area);
        assert!(right.flipped_x);
        assert!(right.panel.left >= work_area.left);
        assert!(right.panel.right <= work_area.right);

        // Bottom edge: flip vertically.
        let bottom = magnifier_geometry(Point::new(960, 1070), config, frame, work_area);
        assert!(bottom.flipped_y);
        assert!(bottom.panel.top >= work_area.top);
        assert!(bottom.panel.bottom <= work_area.bottom);

        // Bottom-right corner: both.
        let corner = magnifier_geometry(Point::new(1918, 1078), config, frame, work_area);
        assert!(corner.flipped_x && corner.flipped_y);

        // Top-left corner: no room to the left or above, so no flip is possible and
        // the panel must be clamped instead.
        let origin = magnifier_geometry(Point::new(0, 0), config, frame, work_area);
        assert!(origin.panel.left >= work_area.left);
        assert!(origin.panel.top >= work_area.top);
        assert_eq!(origin.panel.width(), panel_w);
        assert_eq!(origin.panel.height(), panel_h);

        for geometry in [centre, right, bottom, corner, origin] {
            assert!(geometry.panel.left >= work_area.left);
            assert!(geometry.panel.top >= work_area.top);
            assert!(geometry.panel.right <= work_area.right);
            assert!(geometry.panel.bottom <= work_area.bottom);
            assert!(geometry.info_panel.left >= work_area.left);
            assert!(geometry.info_panel.top >= work_area.top);
            assert!(geometry.info_panel.right <= work_area.right);
            assert!(geometry.info_panel.bottom <= work_area.bottom);
            // The source rect is deliberately allowed to reach past the frame at
            // an edge — that is what keeps `center` (the pixel the crosshair is
            // on) in the panel's centre cell instead of being translated aside.
            // The renderer clips it against the real bitmap bounds.
            assert_eq!(geometry.source.center(), geometry.center);
            assert_eq!(geometry.source.width(), config.source_width);
            assert_eq!(geometry.source.height(), config.source_height);
            // Tile keeps fixed size at every edge
            assert_eq!(geometry.tile.width(), config.tile_size);
            assert_eq!(geometry.tile.height(), config.tile_size);
            assert!(geometry.tile.left >= frame.left);
            assert!(geometry.tile.top >= frame.top);
            assert!(geometry.tile.right <= frame.right);
            assert!(geometry.tile.bottom <= frame.bottom);
        }
    }

    #[test]
    fn magnifier_source_keeps_the_cursor_centred_away_from_edges() {
        let frame = Rect::new(0, 0, 1920, 1080);
        let config = MagnifierConfig::default().scaled(96);
        let geometry = magnifier_geometry(Point::new(960, 540), config, frame, frame);
        assert_eq!(geometry.source.center(), Point::new(960, 540));
        assert_eq!(geometry.source.width(), config.source_width);
        assert_eq!(geometry.source.height(), config.source_height);
        assert_eq!(geometry.panel.width(), config.panel_width());
        assert_eq!(geometry.panel.height(), config.panel_height());
        assert_eq!(geometry.panel.width(), geometry.source.width() * geometry.zoom as i32);
        assert_eq!(geometry.panel.height(), geometry.source.height() * geometry.zoom as i32);
        assert_eq!(geometry.zoom, 20.0);
        // A 17×9 window is odd in both axes, so the cursor's own pixel is the
        // exact centre cell: column 9 of 17, row 5 of 9 (1-based).
        assert_eq!(config.source_width, 17);
        assert_eq!(config.source_height, 9);
        assert_eq!(geometry.source.width() / 2, 8); // 0-based → column 9
        assert_eq!(geometry.source.height() / 2, 4); // 0-based → row 5
        // Tile must contain the cursor and be fixed-size
        assert_eq!(geometry.tile.width(), config.tile_size);
        assert_eq!(geometry.tile.height(), config.tile_size);
        assert!(geometry.tile.contains(geometry.center));
    }

    #[test]
    fn edges_report_the_matching_handle() {
        let rect = Rect::new(100, 100, 300, 200);
        let snapshot = SelectionSnapshot::new(rect, 96);
        assert_eq!(
            snapshot.edges_at(Point::new(100, 100)).handle(),
            Some(Handle::TopLeft)
        );
        assert_eq!(
            snapshot.edges_at(Point::new(300, 150)).handle(),
            Some(Handle::Right)
        );
        assert_eq!(snapshot.edges_at(Point::new(150, 150)).handle(), None);
    }

    #[test]
    fn crosshair_is_local_and_centred_on_the_cursor() {
        let frame = Rect::new(0, 0, 1920, 1080);
        let geometry = crosshair_geometry(Point::new(960, 540), 28, frame);
        assert_eq!(geometry.horizontal, Rect::new(932, 540, 989, 541));
        assert_eq!(geometry.vertical, Rect::new(960, 512, 961, 569));
        assert_eq!(geometry.bounds, Rect::new(932, 512, 989, 569));
        // A hover must never invalidate the whole monitor: the reticle box stays small.
        assert!((geometry.bounds.area()) < frame.area());
    }

    #[test]
    fn crosshair_clamps_to_the_frame_at_the_edges() {
        let frame = Rect::new(0, 0, 1920, 1080);
        let top_left = crosshair_geometry(Point::new(0, 0), 28, frame);
        assert_eq!(top_left.horizontal, Rect::new(0, 0, 29, 1));
        assert_eq!(top_left.vertical, Rect::new(0, 0, 1, 29));
        assert_eq!(top_left.bounds, Rect::new(0, 0, 29, 29));
        let bottom_right = crosshair_geometry(Point::new(1919, 1079), 28, frame);
        assert_eq!(bottom_right.bounds, Rect::new(1891, 1051, 1920, 1080));
    }

    #[test]
    fn crosshair_radius_scales_with_dpi() {
        assert_eq!(crosshair_radius(96), 28);
        assert_eq!(crosshair_radius(144), 42);
        assert!(crosshair_radius(0) == 28, "dpi below 96 clamps to the 96 baseline");
    }

    fn layout(bounds: Rect, dpi: u32) -> MonitorLayout {
        MonitorLayout {
            bounds,
            work_area: bounds,
            dpi,
            primary: false,
        }
    }

    #[test]
    fn window_rect_conversion_subtracts_the_monitor_origin() {
        let secondary = layout(Rect::new(-1920, 200, 0, 1280), 144);
        // A window entirely on the secondary display, including its title bar.
        let screen = Rect::new(-1800, 300, -1200, 900);
        assert_eq!(
            window_rect_to_local(screen, &secondary),
            Rect::new(120, 100, 720, 700)
        );
        // A window on the primary display is unchanged when the monitor origin is 0.
        let primary = layout(Rect::new(0, 0, 3840, 2160), 144);
        assert_eq!(
            window_rect_to_local(Rect::new(10, 20, 30, 40), &primary),
            Rect::new(10, 20, 30, 40)
        );
    }

    #[test]
    fn window_rect_conversion_clips_a_window_that_spans_monitors() {
        let primary = layout(Rect::new(0, 0, 3840, 2160), 144);
        // A window hanging off the left and right edges keeps only the visible column
        // band, and a window hanging off the bottom keeps only the visible row band.
        assert_eq!(
            window_rect_to_local(Rect::new(-500, 100, 4200, 900), &primary),
            Rect::new(0, 100, 3840, 900)
        );
        assert_eq!(
            window_rect_to_local(Rect::new(100, -300, 900, 2400), &primary),
            Rect::new(100, 0, 900, 2160)
        );
    }

    #[test]
    fn window_rect_conversion_rejects_rects_off_this_monitor() {
        let primary = layout(Rect::new(0, 0, 1920, 1080), 96);
        // Entirely to the left of the monitor.
        assert!(window_rect_to_local(Rect::new(-800, 0, -10, 400), &primary).is_empty());
        // Entirely below it.
        assert!(window_rect_to_local(Rect::new(0, 1200, 400, 1400), &primary).is_empty());
        // Touching the right edge is *not* an overlap: the rectangle is half-open.
        assert!(window_rect_to_local(Rect::new(1920, 0, 2200, 400), &primary).is_empty());
    }

    #[test]
    fn window_rect_conversion_normalises_degenerate_input() {
        let primary = layout(Rect::new(0, 0, 1920, 1080), 96);
        // Zero-size and inverted rectangles never produce a negative-size local rect.
        let zero = window_rect_to_local(Rect::new(50, 50, 50, 80), &primary);
        assert!(zero.is_empty());
        let inverted = window_rect_to_local(Rect::new(300, 300, 200, 200), &primary);
        assert!(inverted.is_empty());
        assert!(inverted.left >= 0 && inverted.top >= 0);
    }

    #[test]
    fn window_rect_conversion_keeps_a_full_monitor_window_exact() {
        let secondary = layout(Rect::new(-2560, -200, 0, 1240), 192);
        // A maximised window on the secondary display maps onto the whole local frame.
        assert_eq!(
            window_rect_to_local(Rect::new(-2560, -200, 0, 1240), &secondary),
            secondary.local_bounds()
        );
    }

    #[test]
    fn a_window_on_another_display_never_lands_on_this_overlay() {
        // The multi-display property a single-screen machine cannot demonstrate by hand: a
        // session covers **only** the display the cursor was on when it started (docs/14 §4.1),
        // so another display's window must never be painted here. Displays never overlap in the
        // virtual desktop and this mapping is a pure translation followed by a clip, which is
        // why the property holds by construction — and why the test pins it down.
        let secondary = layout(Rect::new(-1920, 0, 0, 1080), 96);
        let primary = layout(Rect::new(0, 0, 3840, 2160), 192);

        let on_primary = Rect::new(100, 120, 900, 700);
        assert!(
            window_rect_to_local(on_primary, &secondary).is_empty(),
            "the primary display's window must not be drawn on the secondary overlay"
        );
        assert_eq!(
            window_rect_to_local(on_primary, &primary),
            Rect::new(100, 120, 900, 700),
            "…and it is drawn unchanged on the display it belongs to"
        );

        let on_secondary = Rect::new(-1800, 100, -200, 900);
        assert!(window_rect_to_local(on_secondary, &primary).is_empty());
        assert_eq!(
            window_rect_to_local(on_secondary, &secondary),
            Rect::new(120, 100, 1720, 900)
        );

        // Mixed DPI does not enter the physical-pixel mapping: two displays of the same size
        // at 100% and 200% agree exactly on where a window is.
        let same_size_96 = layout(Rect::new(0, 0, 3840, 2160), 96);
        let same_size_192 = layout(Rect::new(0, 0, 3840, 2160), 192);
        assert_eq!(
            window_rect_to_local(on_primary, &same_size_96),
            window_rect_to_local(on_primary, &same_size_192)
        );
    }
}
