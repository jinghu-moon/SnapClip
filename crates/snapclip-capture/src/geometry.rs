//! Pure selection geometry: normalisation, handle hit testing, resize/move math,
//! size-label placement and magnifier placement.
//!
//! Everything in this module works in **physical pixels of one monitor's client
//! area**. The overlay window is borderless and exactly as large as the monitor, so
//! screen coordinates, client coordinates and back-buffer coordinates differ only by
//! the monitor origin. Keeping the maths here means DPI, multi-monitor and handle
//! behaviour can be unit tested without a window station.

use super::CaptureError;
use crate::region::{Delay, Destination, PanelSlot, RegionField, RATIO_MENU_LEN};
use crate::units::{LabelToggle, LengthUnit};

// 过渡期转发（docs/23 T0.4）：定义已搬到 `snapclip-model`，这里是唯一实现的转出口。
// 删除条件：P1 结束时（T1.10）转发必须为零——到那时本文件只留**派生布局**
// （标签/放大镜摆放、窗口矩形换算等），值对象从 `snapclip_model` 直接 import。
pub use snapclip_model::geometry::{Point, Rect};

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
            Handle::Top | Handle::Right | Handle::Bottom | Handle::Left => 10.0,
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
pub const EDGE_HANDLES: [Handle; 4] = [Handle::Top, Handle::Right, Handle::Bottom, Handle::Left];

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

/// How far a grip's hit area reaches from its anchor, in physical pixels.
///
/// This is the primitive behind [`SelectionSnapshot::hit_handle_size`], and it is a free function
/// because a caller can need the number without a selection: the scroll preview panel is pinned to
/// the work area's right edge (docs/30 §19.7) and a grip centred on that edge reaches this far past
/// it, so the panel's margin has to be at least this large or the panel's last column belongs to the
/// grip. At 100%/150%/200% the DIP margin and this reach happen to agree; at 125%/175% they do not,
/// because the two round independently (`P5.05`).
pub fn grip_hit_reach(handle: Handle, dpi: u32) -> i32 {
    let scale = dpi.max(96) as f32 / 96.0;
    (handle.visual_size() * scale + (6.0 * scale).round()).round() as i32
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
        grip_hit_reach(handle, dpi)
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
            return SelectionGeometry::Resize(edges.handle().unwrap_or(Handle::BottomRight));
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
                let grows_left =
                    matches!(handle, Handle::TopLeft | Handle::BottomLeft | Handle::Left);
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
        self.apply_with_constraints(mode, pointer, bounds, false)
    }

    /// Apply a drag, optionally preserving the original aspect ratio for a corner grip.
    ///
    /// The constraint is evaluated from the immutable drag snapshot rather than the
    /// current selection. This keeps the ratio stable while the pointer moves and makes
    /// the rule independent from render/message frequency. It also means the result is a
    /// pure function of the pointer: pressing or releasing Shift while the button is
    /// already down re-constrains the *same* pointer position, so it can never desync
    /// from a drag that held the modifier from the button-down. Edge grips and moves
    /// ignore the constraint because they have no unambiguous opposite corner to
    /// preserve.
    pub fn apply_with_constraints(
        &self,
        mode: ResizeMode,
        pointer: Point,
        bounds: Rect,
        preserve_aspect: bool,
    ) -> Rect {
        if preserve_aspect
            && matches!(mode, ResizeMode::Handle(handle) if CORNER_HANDLES.contains(&handle))
            && !self.snapshot.rect.is_empty()
        {
            return self.apply_corner_aspect(mode, pointer, bounds);
        }

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

    /// Resize around the corner opposite the dragged grip using the original aspect
    /// ratio.
    ///
    /// Only two things decide the result: the ratio and the fixed corner of the drag's
    /// snapshot, plus where the pointer is right now. There is no anchor delta and no
    /// per-move gate, which is what makes the geometry identical whether Shift was held
    /// from the button-down or pressed mid-drag — the earlier anchor-relative rule made
    /// the constraint depend on *when* the modifier arrived, so a mid-drag Shift either
    /// froze the selection on a single-axis move or locked the already-distorted ratio
    /// instead of the one the drag started with.
    ///
    /// The pointer's quadrant around the fixed corner supplies the corner signs, so the
    /// rectangle follows the pointer through the fixed corner and mirrors into the
    /// opposite quadrant — the same normalisation the free path gets from
    /// `Rect::from_corners`. Clamping the distance to zero instead pinned the selection
    /// at its minimum size and made a reverse drag impossible.
    ///
    /// Of the two axes the tighter one wins, so the rectangle never grows past the
    /// pointer on either axis. Pixel dimensions are rounded independently so a 300x200
    /// selection can still move one physical pixel at a time.
    fn apply_corner_aspect(&self, mode: ResizeMode, pointer: Point, bounds: Rect) -> Rect {
        let handle = match mode {
            ResizeMode::Handle(handle) => handle,
            ResizeMode::Move => unreachable!("aspect locking only applies to corner handles"),
        };
        let original = self.snapshot.rect;
        let width = i64::from(original.width());
        let height = i64::from(original.height());
        let minimum = i64::from(self.snapshot.minimum_size());

        let fixed = match handle {
            Handle::TopLeft => Point::new(original.right, original.bottom),
            Handle::TopRight => Point::new(original.left, original.bottom),
            Handle::BottomRight => Point::new(original.left, original.top),
            Handle::BottomLeft => Point::new(original.right, original.top),
            _ => unreachable!("aspect locking only applies to corner handles"),
        };
        let fixed_x = i64::from(fixed.x);
        let fixed_y = i64::from(fixed.y);
        let pointer_x = i64::from(pointer.x);
        let pointer_y = i64::from(pointer.y);
        let sign_x = if pointer_x < fixed_x { -1_i64 } else { 1 };
        let sign_y = if pointer_y < fixed_y { -1_i64 } else { 1 };
        let distance_x = (pointer_x - fixed_x).abs();
        let distance_y = (pointer_y - fixed_y).abs();
        let (distance, dimension) = if distance_x * height <= distance_y * width {
            (distance_x, width)
        } else {
            (distance_y, height)
        };
        let mut result_width = round_ratio(width * distance, dimension);
        let mut result_height = round_ratio(height * distance, dimension);
        if result_width < minimum || result_height < minimum {
            if width >= height {
                result_height = minimum;
                result_width = round_ratio(width * minimum, height).max(minimum);
            } else {
                result_width = minimum;
                result_height = round_ratio(height * minimum, width).max(minimum);
            }
        }

        // A fixed corner is always inside the monitor. Limit the scale to the largest
        // ratio-preserving rectangle that can still fit in the pointer's quadrant.
        let room_x = if sign_x < 0 {
            (fixed_x - i64::from(bounds.left)).max(0)
        } else {
            (i64::from(bounds.right) - fixed_x).max(0)
        };
        let room_y = if sign_y < 0 {
            (fixed_y - i64::from(bounds.top)).max(0)
        } else {
            (i64::from(bounds.bottom) - fixed_y).max(0)
        };
        if room_x <= 0 || room_y <= 0 {
            return Rect::new(fixed.x, fixed.y, fixed.x, fixed.y);
        }
        if result_width > room_x || result_height > room_y {
            let (limit_num, limit_den) = if room_x * height <= room_y * width {
                (room_x, width)
            } else {
                (room_y, height)
            };
            result_width = round_ratio(width * limit_num, limit_den).max(1);
            result_height = round_ratio(height * limit_num, limit_den).max(1);
        }
        let result_width = result_width.min(room_x) as i32;
        let result_height = result_height.min(room_y) as i32;
        let active = Point::new(
            fixed.x.saturating_add((sign_x * i64::from(result_width)) as i32),
            fixed.y.saturating_add((sign_y * i64::from(result_height)) as i32),
        );
        Rect::from_corners(fixed, active)
    }
}

fn round_ratio(numerator: i64, denominator: i64) -> i64 {
    (numerator + denominator / 2) / denominator.max(1)
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
    /// Height of the info strip below the panel. Its text is DIP-sized, so the strip is too:
    /// 76 DIP (a colour+coordinates line and one shortcut row, 12+18+12+22+12) in
    /// [`Default`], physical pixels after [`MagnifierConfig::scaled`].
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
            info_height: Self::INFO_HEIGHT_DIP,
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
    /// The info strip's height at 96 DPI: 12 + 18 (colour row) + 12 + 22 (hint row) + 12.
    pub const INFO_HEIGHT_DIP: i32 = 76;
    pub const ZOOM_MIN: f32 = 0.1;
    pub const ZOOM_MAX: f32 = 40.0;

    /// Predefined zoom ladder — 17 stops from 0.1× to 40×.
    /// Wheel up advances, wheel down retreats.
    pub const ZOOM_LEVELS: &'static [f32] = &[
        0.1, 0.2, 0.3, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 10.0, 15.0, 20.0, 30.0, 40.0,
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
                        ((**a - current).abs())
                            .partial_cmp(&(**b - current).abs())
                            .unwrap()
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

    /// Scale the DPI-dependent fields. Source, tile, and the loupe panel are fixed physical
    /// pixels (the panel is 340×180 regardless of DPI); the gap and the info strip, whose text is
    /// DIP-sized, scale.
    pub fn scaled(&self, dpi: u32) -> Self {
        let scale = dpi.max(96) as f32 / 96.0;
        Self {
            zoom: self.zoom,
            source_width: self.source_width,
            source_height: self.source_height,
            tile_size: self.tile_size,
            gap: (self.gap as f32 * scale).round() as i32,
            info_height: (self.info_height as f32 * scale).round() as i32,
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

    left = left.clamp(
        work_area.left,
        (work_area.right - panel_w).max(work_area.left),
    );
    top = top.clamp(
        work_area.top,
        (work_area.bottom - total_height).max(work_area.top),
    );
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
    let tile_left =
        (center.x - tile_half).clamp(frame.left, (frame.right - tile_size).max(frame.left));
    let tile_top =
        (center.y - tile_half).clamp(frame.top, (frame.bottom - tile_size).max(frame.top));
    let tile = Rect::from_origin_size(Point::new(tile_left, tile_top), tile_size, tile_size);

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

/// 分段器选中块在两端之间插值；`t` 是 0..1 的原始进度（越界夹住，内部做 ease-out）。
///
/// 缓动用三次方 ease-out：起步快、落位稳，正是"滑过去停住"的手感。矩形是整数，插值后取整 ——
/// 半像素的块挨着发丝线会看出抖动。纯函数：不碰时间、不碰设备，所以能单测（docs/23 §14.55.5）。
pub fn segment_slide(from: Rect, to: Rect, t: f32) -> Rect {
    let eased = 1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3);
    let lerp = |a: i32, b: i32| (a as f32 + (b - a) as f32 * eased).round() as i32;
    Rect::new(
        lerp(from.left, to.left),
        lerp(from.top, to.top),
        lerp(from.right, to.right),
        lerp(from.bottom, to.bottom),
    )
}

/// 面板与它每个控件的矩形，都在窗口坐标里（dip × 显示器缩放）。
///
/// 一份矩形同时是"画在哪"和"点哪算命中"：`target_at` 读的就是这里，所以画出来的控件和
/// 能点到的控件不可能对不上。
#[derive(Debug, Clone, PartialEq)]


pub struct RegionPanelLayout {
    pub panel: Rect,
    pub title: Rect,
    
/// 分段器的**容器**：两块选中区都缩在它里面。
    pub unit_track: Rect,
    /// 页头那组单位：两个按钮各一个矩形（已相对容器内缩）。
    pub unit: [(LengthUnit, Rect); 2],
    pub scale: Rect,
    pub close: Rect,
    pub cards: [Rect; 2],
    pub card_titles: [Rect; 2],
    /// 组内动作：互换、居中、移到原点（顺序与 `PanelSlot` 的 Tab 顺序一致）。
    ///
    /// 原来的「锁定」图标删掉了：比例就是唯一的锁定表达（`无比例` = 不锁）。
    pub actions: [(PanelSlot, Rect); 3],
    /// 四个数字格：命中区是数字连同它下面那条下划线。
    pub fields: [(RegionField, Rect); 4],
    pub field_labels: [Rect; 4],
    /// 两张卡里的分隔：尺寸卡的 `×` 与位置卡的竖线（画用，不是控件）。
    pub separators: [Rect; 2],
    /// 比例按钮：住在「尺寸」卡头里，右对齐在交换按钮左侧。
    pub ratio: Rect,
    pub destination_label: Rect,
    /// 去向分段器的**容器**（复制 / 贴图）。
    pub destination_track: Rect,
    pub destination: [(Destination, Rect); 2],
    pub delay_label: Rect,
    /// 分段器的**容器**：四段都缩在它里面（与 `unit_track` 同一套结构）。
    pub delay_track: Rect,
    pub delay: [(Delay, Rect); 4],
    pub toggle_label: Rect,
    pub toggles: [(LabelToggle, Rect); 3],
    /// 图片卡：整张卡、整行卡头（点击开关）、卡头右侧的摘要文字区、折叠箭头。
    pub image_card: Rect,
    pub image_head: Rect,
    pub image_summary: Rect,
    pub image_chevron: Rect,
    /// 圆角行：行标签、滑动条的命中区、可见轨道、滑块、值块，以及上限（像素）。
    pub radius_label: Rect,
    pub radius_track: Rect,
    pub radius_groove: Rect,
    pub radius_thumb: Rect,
    pub radius_value: Rect,
    pub radius_max: i32,
    pub status: Rect,
    pub cancel: Rect,
    pub start: Rect,
    /// 比例下拉：展开时才有，且已经夹进工作区。
    pub menu: Option<Rect>,
    pub menu_items: [Rect; RATIO_MENU_LEN],
}

/// 面板上被指到的东西。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionPanelTarget {
    /// 一个焦点槽（数值格、图标、预设、延时、开关、取消、开始）。
    Slot(PanelSlot),
    /// 页头那组单位里的一个按钮。
    Unit(LengthUnit),
    /// 延时分段器里的一段。
    Delay(Delay),
    /// 去向分段器里的一段。
    Destination(Destination),
    /// 预设下拉里的第几项。
    MenuItem(usize),
    /// 下拉面板自己的内边距：不是控件，但点击要停在下拉里。
    MenuPadding,
    /// 圆角滑块上按下的位置换算出来的像素值（0..=radius_max）。
    Radius(u32),
}

/// 面板的尺寸与各处间距（逻辑单位，按显示器缩放）。
const REGION_WIDTH_DIP: f32 = 420.0;
const REGION_PAD_DIP: f32 = 16.0;
const REGION_HEAD_TOP_DIP: f32 = 10.0;
const REGION_HEAD_ROW_DIP: f32 = 26.0;
const REGION_HEAD_BOTTOM_DIP: f32 = 8.0;
/// 标题与关闭按钮之间的最小间距：标题原来被单位分段器的左边界截住，现在只受这一条限制。
const REGION_HEAD_GAP_DIP: f32 = 8.0;
const REGION_UNIT_BUTTON_DIP: f32 = 32.0;
/// 分段器里选中块相对容器的内缩：选中背景要缩在容器内，不能与容器等大。
const REGION_UNIT_INSET_DIP: f32 = 3.0;
/// 分段器的外层圆角（内层按嵌套规则减掉实际内缩）。
const REGION_SEGMENT_RADIUS_DIP: f32 = 7.0;

const REGION_SCALE_WIDTH_DIP: f32 = 46.0;

/// 单位分段器与倍率块之间的间距：沿用它们原来并排放在页头里时的值。
const REGION_UNIT_SCALE_GAP_DIP: f32 = 8.0;
/// 角标行里「左侧三个开关」与「右侧两个控件」之间的最小间距。位置由右对齐决定，
/// 这条是底线，由测试守着（docs/23 §14.55.3）。
const REGION_TOGGLE_CONTROLS_GAP_DIP: f32 = 12.0;
const REGION_BODY_TOP_DIP: f32 = 4.0;
const REGION_CARD_GAP_DIP: f32 = 10.0;
const REGION_CARD_PAD_DIP: f32 = 10.0;
/// 卡片底部的留白：标签下面也要有气口，否则「宽/高/X/Y」看起来贴在边框上。
const REGION_CARD_PAD_BOTTOM_DIP: f32 = 12.0;
const REGION_CARD_HEAD_DIP: f32 = 20.0;
/// 卡片标题行与数字之间的呼吸。
const REGION_CARD_GAP_IN_DIP: f32 = 6.0;
/// 数字那一行的高度：它的下边缘就是下划线。
const REGION_VALUE_ROW_DIP: f32 = 28.0;
const REGION_VALUE_LABEL_GAP_DIP: f32 = 6.0;
const REGION_VALUE_LABEL_DIP: f32 = 14.0;
/// 两张卡里数字之间的分隔宽度。
const REGION_SEPARATOR_DIP: f32 = 16.0;
const REGION_ROW_GAP_DIP: f32 = 10.0;
/// 预设行 / 延时分段所在那一行的高度。
const REGION_ROW_DIP: f32 = 28.0;
/// 行首标签（预设 / 浮层 / 延时）的宽度。
const REGION_ROW_LABEL_DIP: f32 = 26.0;

// ── 图片卡（docs/23 §14.55.19）──────────────────────────────────────────
const REGION_RADIUS_GROOVE_DIP: f32 = 4.0;
const REGION_RADIUS_THUMB_DIP: f32 = 14.0;
const REGION_RADIUS_VALUE_GAP_DIP: f32 = 12.0;
const REGION_RADIUS_VALUE_WIDTH_DIP: f32 = 46.0;
const REGION_RADIUS_VALUE_HEIGHT_DIP: f32 = 24.0;
const REGION_CHEVRON_DIP: f32 = 14.0;

/// 图片卡此刻的形状：展开与否、圆角多少。布局要靠它算高度与滑块位置，
/// 所以跟着 `menu_open` 一起作为纯输入传进来.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelCard {
    pub open: bool,
    pub radius_px: i32,
}

impl PanelCard {
    pub fn collapsed() -> Self {
        Self { open: false, radius_px: 0 }
    }

    pub fn expanded(radius_px: i32) -> Self {
        Self { open: true, radius_px }
    }
}
const REGION_ICON_DIP: f32 = 22.0;
/// 比例按钮的兜底宽（dip）：`无比例` 三个汉字 + 12 的箭头 + 两侧内边距。
const REGION_RATIO_LABEL_DIP: f32 = 64.0;
/// 比例菜单的兜底宽（dip）：最宽项（`无比例`）+ 26 检查位 + 10 右内边距 + 5×2 菜单内边距。
const REGION_RATIO_MENU_DIP: f32 = 82.0;
const REGION_SEGMENT_DIP: f32 = 30.0;

/// 分段器每段里，文字两侧的内边距（dip）。量宽在设备层做：最宽选项的文本宽度 + 两侧这份内边距
/// 就是每段的宽度 —— 两个分段器用同一条规矩（docs/23 §14.55.6）。
pub const REGION_SEGMENT_PAD_DIP: f32 = 8.0;

/// 分段器每段的宽度（物理像素），由设备层按**最宽选项**量出来。
///
/// 量文字宽度只能在设备层做（要 `IDWriteTextFormat`），而布局是纯函数，所以宽度作为值传进来。
/// **必须只有一份**：绘制与命中各调一次布局，两次都要读同一个值，否则会出现"画的位置和点的位置
/// 不一致"。拿不到时用 [`SegmentWidths::fixed`] 兜底，布局因此永远不需要 `Option`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentWidths {
    /// 单位分段器（PX / DP）每段的宽度。
    pub unit: i32,
    /// 延时分段器每段的宽度。
    pub delay: i32,
    /// 去向分段器（复制 / 贴图）每段的宽度。
    pub destination: i32,
    /// 比例按钮的宽度：**最宽的那个比例文案**（`无比例`）量出来的 + 箭头 + 内边距。
    ///
    /// 不随选中项变宽变窄 —— 按钮宽度一跳，卡头里的东西就跟着晃（docs/23 §14.55.15）。
    pub ratio_label: i32,
    /// 比例菜单的宽度：所有项里最宽的那个文本 + 检查位 + 内边距。与按钮宽解耦。
    pub ratio_menu: i32,
}

impl SegmentWidths {
    /// 还没量过时的固定值：取量宽之前的历史尺寸，界面不会因为"没量到"而走样。
    pub fn fixed(scale: f32) -> Self {
        let dip = |value: f32| (value * scale).round() as i32;
        Self {
            // 历史那两个常量描述的是"容器里每格的大小"，而段是缩在容器里的，所以段宽 = 每格 − 内缩。
            // 单位那一格本来就是"容器的一半"，延时那一格是 `REGION_SEGMENT_DIP`，两者同一条配方。
            unit: dip(REGION_UNIT_BUTTON_DIP) - dip(REGION_UNIT_INSET_DIP),
            delay: dip(REGION_SEGMENT_DIP) - dip(REGION_DELAY_INSET_DIP),
            // 两个汉字（复制 / 贴图）在 11 dip 下的经验宽度；真值由设备层量（`panel_metrics`）。
            destination: dip(34.0),
            ratio_label: dip(REGION_RATIO_LABEL_DIP),
            ratio_menu: dip(REGION_RATIO_MENU_DIP),
        }
    }
}
/// 延时分段器里四段相对容器的内缩（与 `REGION_UNIT_INSET_DIP` 同一套规矩）。
const REGION_DELAY_INSET_DIP: f32 = 3.0;
const REGION_TOGGLE_ROW_DIP: f32 = 24.0;
const REGION_TOGGLE_DIP: f32 = 46.0;
/// 「尺寸 / 位置 / 单位」三块之间的间距（docs/23 §14.55.7）。
const REGION_TOGGLE_GAP_DIP: f32 = 8.0;
const REGION_BODY_BOTTOM_DIP: f32 = 12.0;
const REGION_FOOT_TOP_DIP: f32 = 10.0;
const REGION_BUTTON_HEIGHT_DIP: f32 = 28.0;
/// 页脚两个按钮共用的宽度。
///
/// 两个文案都是两个字（取消 / 确定），按同一条规范取值；各写一个常量只会让"两个按钮宽度不一致"
/// 这种偏差有机会发生（docs/23 §14.55.8）。
const REGION_FOOTER_BUTTON_WIDTH_DIP: f32 = 60.0;
const REGION_FOOT_BOTTOM_DIP: f32 = 11.0;
/// 面板放进选框时留的边，以及放到选框外时与它的缝。
const REGION_INSIDE_INSET_DIP: f32 = 12.0;
const REGION_OUTSIDE_GAP_DIP: f32 = 10.0;
/// 下拉菜单：内边距、每项高度，以及它与触发器之间的缝。
const REGION_MENU_PAD_DIP: f32 = 5.0;
const REGION_MENU_ITEM_DIP: f32 = 30.0;
const REGION_MENU_GAP_DIP: f32 = 6.0;

/// 胶囊形圆角：半径 = 高度的一半，左右两端就是半圆、上下边缘平直。
///
/// 三处芯片（尺寸角标、↑/↓ 方向芯片、角标行右边那个预览标签）共用它——之前它们各写一套
/// （`3.0` / `dip(6.0)` / `height/2`），于是只有一个真的成了胶囊（docs/23 §14.55）。
/// 高度为奇数时给半值（D2D 会把半径夹到短边的一半，所以仍是正圆端）；高度为 0 或负返回 0，
/// 避免负半径。
pub fn pill_radius(rect: Rect) -> f32 {
    (rect.height() as f32 / 2.0).max(0.0)
}

/// 嵌套圆角：外层半径 `outer` 的容器里，内缩 `inset` 的内层取 `outer - inset`（不为负）。
///
/// 两层同心，内块的圆弧才不会戳出外块。分段器（容器 + 选中块）走这一条，别再各写一个数；
/// 它**不是**胶囊规则 —— 胶囊是 [`pill_radius`]（docs/23 §14.55.4）。
pub fn segment_radius(outer: i32, inset: i32) -> i32 {
    (outer - inset).max(0)
}

/// 分段器外层圆角，按显示器缩放取整。
pub fn segment_outer_radius(scale: f32) -> i32 {
    (REGION_SEGMENT_RADIUS_DIP * scale).round() as i32
}

/// 按显示器 `scale` 排一块自定义截图面板。
///
/// 位置先试**选框正中间**——面板改的就是那块区域，所以它该和区域在一起；选框装不下就挪到外面
/// （先下方、下方不行就上方），最后夹进工作区。与尺寸标、徽标菜单同一个梯子：够得着的控件比
/// 好看的控件重要。高度由各段相加，所以改一个间距不会把内容挤出去。
///
/// `selection` 是会话此刻的选框（本屏像素）；下拉展开时挂在触发器下面，并夹进工作区。
pub fn region_panel_layout(
    scale: f32,
    work_area: Rect,
    selection: Rect,
    menu_open: bool,
    card: PanelCard,
    widths: SegmentWidths,
) -> RegionPanelLayout {
    let k = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let dip = |value: f32| (value * k).round() as i32;

    let pad = dip(REGION_PAD_DIP);
    let head_top = dip(REGION_HEAD_TOP_DIP);
    let head_row = dip(REGION_HEAD_ROW_DIP);
    let head_bottom = dip(REGION_HEAD_BOTTOM_DIP);
    let body_top = dip(REGION_BODY_TOP_DIP);
    let card_gap = dip(REGION_CARD_GAP_DIP);
    let card_pad = dip(REGION_CARD_PAD_DIP);
    let card_pad_bottom = dip(REGION_CARD_PAD_BOTTOM_DIP);
    let card_head = dip(REGION_CARD_HEAD_DIP);
    let card_gap_in = dip(REGION_CARD_GAP_IN_DIP);
    let value_row = dip(REGION_VALUE_ROW_DIP);
    let value_label_gap = dip(REGION_VALUE_LABEL_GAP_DIP);
    let value_label = dip(REGION_VALUE_LABEL_DIP);
    let separator = dip(REGION_SEPARATOR_DIP);
    let row_gap = dip(REGION_ROW_GAP_DIP);
    let row = dip(REGION_ROW_DIP);
    let row_label = dip(REGION_ROW_LABEL_DIP);
    let icon = dip(REGION_ICON_DIP);
    let toggle_row = dip(REGION_TOGGLE_ROW_DIP);
    let toggle_width = dip(REGION_TOGGLE_DIP);
    let toggle_gap = dip(REGION_TOGGLE_GAP_DIP);
    let body_bottom = dip(REGION_BODY_BOTTOM_DIP);
    let foot_top = dip(REGION_FOOT_TOP_DIP);
    let button_height = dip(REGION_BUTTON_HEIGHT_DIP);
    let start_width = dip(REGION_FOOTER_BUTTON_WIDTH_DIP);
    let cancel_width = start_width;
    let foot_bottom = dip(REGION_FOOT_BOTTOM_DIP);

    let card_height =
        card_pad + card_head + card_gap_in + value_row + value_label_gap + value_label + card_pad_bottom;
    // 图片卡：收起只有卡头一行，展开再加一行圆角（docs/23 §14.55.19）。它是新内容，
    // 所以这是唯一允许把面板变高的地方。
    let image_collapsed_h = card_pad + card_head + card_pad_bottom;
    let image_height = if card.open {
        image_collapsed_h + card_gap_in + row
    } else {
        image_collapsed_h
    };
    let height = head_top
        + head_row
        + head_bottom
        + body_top
        + card_height
        + row_gap
        + row
        + row_gap
        + toggle_row
        + card_gap
        + image_height
        + body_bottom
        + foot_top
        + button_height
        + foot_bottom;
    let width = dip(REGION_WIDTH_DIP);
    let inset = dip(REGION_INSIDE_INSET_DIP);
    let outside_gap = dip(REGION_OUTSIDE_GAP_DIP);
    // 先试"放进选框正中间"：面板改的就是那块区域，它该和区域在一起。装不下就挪到外面：
    // 先下方（选框下面通常是空的），下方放不下再上方，最后夹进工作区。
    let fits_inside =
        selection.width() >= width + 2 * inset && selection.height() >= height + 2 * inset;
    let centered_x = selection.left + (selection.width() - width) / 2;
    let origin = if fits_inside {
        Point::new(
            centered_x,
            selection.top + (selection.height() - height) / 2,
        )
    } else {
        let below = selection.bottom + outside_gap;
        let above = selection.top - outside_gap - height;
        let top = if below + height <= work_area.bottom {
            below
        } else if above >= work_area.top {
            above
        } else {
            below
        };
        Point::new(centered_x, top)
    };
    let panel = Rect::from_origin_size(origin, width, height).clamped_into(work_area);

    let left = panel.left + pad;
    let right = panel.right - pad;
    let content = right - left;

    // ── 页头：只剩标题与关闭（单位分段器与倍率搬到了角标行右侧，docs/23 §14.55.3）─────
    let head_y = panel.top + head_top;
    let close = Rect::from_origin_size(
        Point::new(right - head_row, head_y),
        head_row,
        head_row,
    );
    // 标题原来被单位分段器的左边界截住，现在只受"不压到关闭按钮"这一条限制。
    let title = Rect::new(
        left,
        head_y,
        close.left - dip(REGION_HEAD_GAP_DIP),
        head_y + head_row,
    );

    // ── 两张卡：尺寸（宽/高）与位置（X/Y） ───────────────────────────────────
    let cards_y = panel.top + head_top + head_row + head_bottom + body_top;
    let card_width = (content - card_gap) / 2;
    let cards = [
        Rect::from_origin_size(Point::new(left, cards_y), card_width, card_height),
        Rect::from_origin_size(
            Point::new(left + card_width + card_gap, cards_y),
            card_width,
            card_height,
        ),
    ];
    let card_title_y = cards_y + card_pad;
    let title_height = dip(REGION_CARD_HEAD_DIP).max(icon);
    let card_titles = [
        Rect::new(cards[0].left + card_pad, card_title_y, cards[0].right - card_pad, card_title_y + title_height),
        Rect::new(cards[1].left + card_pad, card_title_y, cards[1].right - card_pad, card_title_y + title_height),
    ];
    // 组内动作靠右上，两个一组；顺序就是 Tab 顺序。
    let actions_at = |card: Rect, first: PanelSlot, second: PanelSlot| {
        let y = card_title_y + (title_height - icon) / 2;
        let second_x = card.right - card_pad - icon;
        [
            (first, Rect::from_origin_size(Point::new(second_x - icon - 2, y), icon, icon)),
            (second, Rect::from_origin_size(Point::new(second_x, y), icon, icon)),
        ]
    };
    // 尺寸卡头：右边是交换图标（22×22），它左边是比例按钮。比例按钮的宽度由设备层按
    // **最宽的比例文案**量出来（`widths.ratio_label`），所以选中项换名字时按钮不会跳。
    let icon_y = card_title_y + (title_height - icon) / 2;
    let swap = (
        PanelSlot::Swap,
        Rect::from_origin_size(
            Point::new(cards[0].right - card_pad - icon, icon_y),
            icon,
            icon,
        ),
    );
    let ratio_width = widths.ratio_label.max(1);
    let ratio = Rect::from_origin_size(
        Point::new(swap.1.left - 2 - ratio_width, icon_y),
        ratio_width,
        icon,
    );
    let [center, origin] = actions_at(cards[1], PanelSlot::Center, PanelSlot::Origin);
    let actions = [swap, center, origin];

    // 两张卡里的数字按同一套列宽排：两个格子 + 一条分隔。
    let inner = card_width - 2 * card_pad;
    let cell = (inner - separator) / 2;
    let value_y = cards_y + card_pad + title_height + card_gap_in;
    let label_y = value_y + value_row + value_label_gap;
    let cells_in = |card: Rect| {
        let first = card.left + card_pad;
        let second = first + cell + separator;
        (first, second)
    };
    let (size_first, size_second) = cells_in(cards[0]);
    let (pos_first, pos_second) = cells_in(cards[1]);
    let field = |x: i32, field: RegionField| {
        (
            field,
            Rect::from_origin_size(Point::new(x, value_y), cell, value_row),
        )
    };
    let label = |x: i32| Rect::from_origin_size(Point::new(x, label_y), cell, value_label);
    let fields = [
        field(size_first, RegionField::Width),
        field(size_second, RegionField::Height),
        field(pos_first, RegionField::X),
        field(pos_second, RegionField::Y),
    ];
    let field_labels = [
        label(size_first),
        label(size_second),
        label(pos_first),
        label(pos_second),
    ];
    let separators = [
        Rect::new(
            size_first + cell,
            value_y,
            size_first + cell + separator,
            label_y + value_label,
        ),
        Rect::new(
            pos_first + cell,
            value_y,
            pos_first + cell + separator,
            label_y + value_label,
        ),
    ];

    // ── 去向行（左）与延时行（右） ──────────────────────────────────────────
    //
    // 比例原来占的那一行现在给去向了：比例搬进了「尺寸」卡头，行数没变，所以面板高度仍是 277。
    let preset_y = cards_y + card_height + row_gap;
    let destination_label = Rect::new(
        left,
        preset_y + (row - value_label) / 2,
        left + row_label,
        preset_y + (row - value_label) / 2 + value_label,
    );
    let destination_h = dip(REGION_SEGMENT_DIP);
    let destination_inset = dip(REGION_DELAY_INSET_DIP);
    let destination_w = widths.destination.max(1);
    let destination_track_w = destination_w * Destination::ALL.len() as i32 + destination_inset * 2;
    let destination_track = Rect::from_origin_size(
        Point::new(
            destination_label.right,
            preset_y + (row - destination_h) / 2,
        ),
        destination_track_w,
        destination_h,
    );
    let destination = Destination::ALL.map(|destination| {
        let index = destination.index() as i32;
        (
            destination,
            Rect::from_origin_size(
                Point::new(
                    destination_track.left + destination_inset + index * destination_w,
                    destination_track.top + destination_inset,
                ),
                destination_w,
                destination_h - 2 * destination_inset,
            ),
        )
    });
    // 每段同宽：宽度由设备层按**最宽选项**量出来（`SegmentWidths`），容器 = 段数 × 段宽 + 两侧内缩。
    // 这样段与段之间不再有"边界按比例取整"留下的 1 px 差（docs/23 §14.55.6）。
    let delay_w = widths.delay.max(1);
    let delay_inset = dip(REGION_DELAY_INSET_DIP);
    let delay_track_w = delay_w * Delay::ALL.len() as i32 + delay_inset * 2;
    let delay_group_width = row_label + delay_track_w;
    let delay_label_y = preset_y + (row - value_label) / 2;
    let delay_label = Rect::new(
        right - delay_group_width,
        delay_label_y,
        right - delay_group_width + row_label,
        delay_label_y + value_label,
    );
    let delay_h = dip(REGION_SEGMENT_DIP);
    let delay_y = preset_y + (row - delay_h) / 2;
    let delay_track = Rect::from_origin_size(
        Point::new(delay_label.right, delay_y),
        delay_track_w,
        delay_h,
    );
    let delay = Delay::ALL.map(|delay| {
        let index = delay.index() as i32;
        (
            delay,
            Rect::from_origin_size(
                Point::new(
                    delay_track.left + delay_inset + index * delay_w,
                    delay_track.top + delay_inset,
                ),
                delay_w,
                delay_h - 2 * delay_inset,
            ),
        )
    });

    // ── 角标行：行标签 + 三个开关；右侧是单位分段器与倍率（从页头搬来）─────────
    let toggle_y = preset_y + row + row_gap;
    let toggle_label = Rect::new(
        left,
        toggle_y + (toggle_row - value_label) / 2,
        left + row_label,
        toggle_y + (toggle_row - value_label) / 2 + value_label,
    );
    let toggles = LabelToggle::ALL.map(|toggle| {
        let index = LabelToggle::ALL
            .iter()
            .position(|item| *item == toggle)
            .unwrap_or(0) as i32;
        (
            toggle,
            Rect::from_origin_size(
                Point::new(toggle_label.right + index * (toggle_width + toggle_gap), toggle_y),
                toggle_width,
                toggle_row,
            ),
        )
    });

    // 右侧两个控件：右边缘与延时行、页脚同一条竖线（= 内容右边界 `right`）；两个都在角标行内
    // 垂直居中；分段器与倍率之间的间距沿用它们原来在页头里挨着时的值。字段名不变，只是位置变了。
    let scale_width = dip(REGION_SCALE_WIDTH_DIP);
    // 高度只有一个来源：这一行的胶囊高度（`toggle_row`）。分段器与缩放块都比照它，
    // 所以三者顶底对齐；原来各自写一个常量会让缩放块矮 4 dip（docs/23 §14.55.7）。
    let scale_height = toggle_row;
    let scale = Rect::from_origin_size(
        Point::new(
            right - scale_width,
            toggle_y,
        ),
        scale_width,
        scale_height,
    );
    let unit_inset = dip(REGION_UNIT_INSET_DIP);
    let unit_height = toggle_row;
    // 与延时同一套：每段同宽（`widths.unit`），容器 = 两块 × 段宽 + 两侧内缩；选中块四面内缩，
    // 所以选中背景与容器外框之间始终看得见间距（docs/23 §14.55.6）。
    let unit_w = widths.unit.max(1);
    let unit_track_w = unit_w * 2 + unit_inset * 2;
    let unit_track = Rect::from_origin_size(
        Point::new(
            scale.left - dip(REGION_UNIT_SCALE_GAP_DIP) - unit_track_w,
            toggle_y,
        ),
        unit_track_w,
        unit_height,
    );
    let unit_segment = |index: i32| {
        Rect::from_origin_size(
            Point::new(
                unit_track.left + unit_inset + index * unit_w,
                unit_track.top + unit_inset,
            ),
            unit_w,
            unit_height - 2 * unit_inset,
        )
    };
    let unit = [
        (LengthUnit::Px, unit_segment(0)),
        (LengthUnit::Dp, unit_segment(1)),
    ];
    // 底线：左侧三个开关与右侧两个控件之间必须留出 `REGION_TOGGLE_CONTROLS_GAP_DIP`。位置由右对齐
    // 决定，所以这里只做开发期断言——真挤不下时该改的是设计参数，不是悄悄把控件挪位。
    debug_assert!(
        unit_track.left - toggles[2].1.right >= dip(REGION_TOGGLE_CONTROLS_GAP_DIP),
        "角标行挤了：开关组与右侧控件之间的间距不足"
    );

    // ── 图片卡：整宽、常驻在页脚上方；卡头一行是开关（docs/23 §14.55.19）──────
    let image_y = toggle_y + toggle_row + card_gap;
    let image_card = Rect::from_origin_size(
        Point::new(left, image_y),
        content,
        image_height,
    );
    let image_head = Rect::new(
        image_card.left + card_pad,
        image_y + card_pad,
        image_card.right - card_pad,
        image_y + card_pad + card_head,
    );
    let chevron = dip(REGION_CHEVRON_DIP);
    let image_chevron = Rect::from_origin_size(
        Point::new(
            image_head.right - chevron,
            image_head.top + (image_head.height() - chevron) / 2,
        ),
        chevron,
        chevron,
    );
    // 摘要右对齐在箭头左侧；收起时才画（展开时下面的行本来就在显示它）。
    let image_summary = Rect::new(
        image_head.left,
        image_head.top,
        image_chevron.left - dip(8.0),
        image_head.bottom,
    );

    // 圆角行：标签 + 弹性滑动条 + 右侧值块（46×24，与倍率块同规格）。
    //
    // 收起时这五个矩形全部压成空矩形（同一个点、尺寸 0）：它们既不占位置，也不会被
    // `target_at` 命中 —— 否则收起的卡片会在页脚附近留下一个隐形的滑块。
    let radius_row_y = image_head.bottom + card_gap_in;
    let radius_value_w = dip(REGION_RADIUS_VALUE_WIDTH_DIP);
    let radius_value_h = dip(REGION_RADIUS_VALUE_HEIGHT_DIP);
    let radius_value = Rect::from_origin_size(
        Point::new(
            image_card.right - card_pad - radius_value_w,
            radius_row_y + (row - radius_value_h) / 2,
        ),
        radius_value_w,
        radius_value_h,
    );
    let radius_label = Rect::new(
        image_card.left + card_pad,
        radius_row_y + (row - value_label) / 2,
        image_card.left + card_pad + row_label,
        radius_row_y + (row - value_label) / 2 + value_label,
    );
    let track_left = radius_label.right;
    let track_right = radius_value.left - dip(REGION_RADIUS_VALUE_GAP_DIP);
    let radius_track = Rect::new(track_left, radius_row_y, track_right, radius_row_y + row);
    let groove_h = dip(REGION_RADIUS_GROOVE_DIP);
    let radius_groove = Rect::new(
        track_left,
        radius_row_y + (row - groove_h) / 2,
        track_right,
        radius_row_y + (row + groove_h) / 2,
    );
    let radius_max =
        (selection.width().min(selection.height()).max(0) / 2).min(crate::region::RADIUS_MAX_PX);
    let thumb = dip(REGION_RADIUS_THUMB_DIP);
    let span = (radius_groove.right - radius_groove.left - thumb).max(0);
    let t = if radius_max > 0 {
        (card.radius_px.clamp(0, radius_max) as f32) / radius_max as f32
    } else {
        0.0
    };
    let radius_thumb = Rect::from_origin_size(
        Point::new(
            radius_groove.left + (span as f32 * t).round() as i32,
            radius_groove.center().y - thumb / 2,
        ),
        thumb,
        thumb,
    );
    let (radius_label, radius_track, radius_groove, radius_thumb, radius_value) = if card.open {
        (
            radius_label,
            radius_track,
            radius_groove,
            radius_thumb,
            radius_value,
        )
    } else {
        let empty = Rect::from_origin_size(Point::new(image_card.left, image_card.top), 0, 0);
        (empty, empty, empty, empty, empty)
    };

    // ── 页脚：一句话 + 两个按钮 ─────────────────────────────────────────────
    let foot_y = panel.bottom - foot_bottom - button_height;
    let start = Rect::from_origin_size(
        Point::new(right - start_width, foot_y),
        start_width,
        button_height,
    );
    let cancel = Rect::from_origin_size(
        Point::new(start.left - 8 - cancel_width, foot_y),
        cancel_width,
        button_height,
    );
    let status = Rect::new(left, foot_y, cancel.left - 8, foot_y + button_height);

    // ── 下拉：挂在比例按钮下面、右缘对齐尺寸卡右缘，夹进工作区 ────────────────
    let menu_width = widths.ratio_menu.max(1);
    let (menu, menu_items) = if menu_open {
        let menu_pad = dip(REGION_MENU_PAD_DIP);
        let item_height = dip(REGION_MENU_ITEM_DIP);
        let gap = dip(REGION_MENU_GAP_DIP);
        let menu_height = 2 * menu_pad + RATIO_MENU_LEN as i32 * item_height;
        // 右缘与卡片右缘对齐（按钮比菜单窄，菜单向左展开）。
        let menu = Rect::from_origin_size(
            Point::new(cards[0].right - menu_width, ratio.bottom + gap),
            menu_width,
            menu_height,
        )
        .clamped_into(work_area);
        let items = [0usize, 1, 2, 3, 4, 5, 6, 7, 8].map(|index| {
            Rect::from_origin_size(
                Point::new(menu.left, menu.top + menu_pad + index as i32 * item_height),
                menu.width(),
                item_height,
            )
        });
        (Some(menu), items)
    } else {
        let empty = Rect::from_origin_size(Point::new(ratio.left, ratio.bottom), menu_width, 0);
        (None, [empty; RATIO_MENU_LEN])
    };

    RegionPanelLayout {
        panel,
        title,
        unit_track,
        unit,
        scale,
        close,
        cards,
        card_titles,
        actions,
        fields,
        field_labels,
        separators,
        ratio,
        destination_label,
        destination_track,
        destination,
        delay_label,
        delay_track,
        delay,
        toggle_label,
        toggles,
        image_card,
        image_head,
        image_summary,
        image_chevron,
        radius_label,
        radius_track,
        radius_groove,
        radius_thumb,
        radius_value,
        radius_max,
        status,
        cancel,
        start,
        menu,
        menu_items,
    }
}

impl RegionPanelLayout {
    /// 滑动条上某个 x 对应的像素值：点击与拖动共用这一处映射（`target_at` 也走它）。
    pub fn radius_at(&self, x: i32) -> u32 {
        let thumb = self.radius_thumb.width();
        let span = (self.radius_groove.width() - thumb).max(1);
        let clamped = (x - self.radius_groove.left - thumb / 2).clamp(0, span);
        let value = (clamped as f32 / span as f32 * self.radius_max as f32).round() as i32;
        value.max(0) as u32
    }

    /// 哪个控件在 `point` 上；面板的内边距、卡片留白与页脚空白返回 `None`。
    ///
    /// 下拉展开时它压着下面的行，所以先问下拉。
    pub fn target_at(&self, point: Point) -> Option<RegionPanelTarget> {
        if let Some(menu) = self.menu {
            if menu.contains(point) {
                for (index, item) in self.menu_items.iter().enumerate() {
                    if item.contains(point) {
                        return Some(RegionPanelTarget::MenuItem(index));
                    }
                }
                return Some(RegionPanelTarget::MenuPadding);
            }
        }
        for (unit, rect) in self.unit {
            if rect.contains(point) {
                return Some(RegionPanelTarget::Unit(unit));
            }
        }
        if self.close.contains(point) {
            return Some(RegionPanelTarget::Slot(PanelSlot::Cancel));
        }
        for (slot, rect) in self.actions {
            if rect.contains(point) {
                return Some(RegionPanelTarget::Slot(slot));
            }
        }
        for (field, rect) in self.fields {
            if rect.contains(point) {
                return Some(RegionPanelTarget::Slot(field.slot()));
            }
        }
        // 图片卡：整行卡头是开关；展开后滑块把 x 换算成像素值（与绘制共用同一段轨道）。
        if self.image_head.contains(point) {
            return Some(RegionPanelTarget::Slot(PanelSlot::Image));
        }
        if self.radius_track.width() > 0 && self.radius_track.contains(point) {
            return Some(RegionPanelTarget::Radius(self.radius_at(point.x)));
        }
        if self.ratio.contains(point) {
            return Some(RegionPanelTarget::Slot(PanelSlot::Ratio));
        }
        for (destination, rect) in self.destination {
            if rect.contains(point) {
                return Some(RegionPanelTarget::Destination(destination));
            }
        }
        for (delay, rect) in self.delay {
            if rect.contains(point) {
                return Some(RegionPanelTarget::Delay(delay));
            }
        }
        for (toggle, rect) in self.toggles {
            if rect.contains(point) {
                return Some(RegionPanelTarget::Slot(toggle.slot()));
            }
        }
        if self.start.contains(point) {
            return Some(RegionPanelTarget::Slot(PanelSlot::Start));
        }
        if self.cancel.contains(point) {
            return Some(RegionPanelTarget::Slot(PanelSlot::Cancel));
        }
        None
    }

    /// 下拉里第几项被点到；没展开或没点到项时 `None`。
    pub fn menu_item_at(&self, point: Point) -> Option<usize> {
        self.menu?;
        self.menu_items
            .iter()
            .position(|item| item.contains(point))
    }
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
        let resized = drag.apply(
            ResizeMode::Handle(Handle::BottomRight),
            Point::new(500, 400),
            monitor(),
        );
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
        let resized = drag.apply(
            ResizeMode::Handle(Handle::Right),
            Point::new(50, 200),
            monitor(),
        );
        assert_eq!(resized, Rect::new(50, 100, 100, 200));
    }

    #[test]
    fn shift_corner_resize_preserves_the_original_ratio_for_all_corners() {
        let rect = Rect::new(100, 100, 400, 300);
        let drag_points = [
            (Handle::TopLeft, Point::new(0, 0)),
            (Handle::TopRight, Point::new(500, 0)),
            (Handle::BottomRight, Point::new(500, 400)),
            (Handle::BottomLeft, Point::new(0, 400)),
        ];
        for (handle, pointer) in drag_points {
            let drag = SelectionDrag::new(rect, 96, handle.anchor(rect));
            let resized = drag.apply_with_constraints(
                ResizeMode::Handle(handle),
                pointer,
                monitor(),
                true,
            );
            let ratio_error = (i64::from(resized.width()) * i64::from(rect.height())
                - i64::from(resized.height()) * i64::from(rect.width()))
            .abs();
            assert!(ratio_error <= i64::from(rect.width()), "{handle:?}: {resized:?}");
            assert!(monitor().contains_rect(resized), "{handle:?}: {resized:?}");
        }
    }

    #[test]
    fn shift_corner_resize_scales_from_the_pointer_on_a_single_axis_move() {
        // The previous rule demanded that both axes travel in the corner's diagonal
        // direction and otherwise returned `original`, which froze the selection. The
        // scale is now fitted from the pointer alone: on a horizontal-only move the
        // vertical distance is the tighter axis and therefore sets the scale.
        let rect = Rect::new(100, 100, 400, 300);
        let drag = SelectionDrag::new(rect, 96, Point::new(400, 300));

        // The pointer stayed on the original bottom edge, so the scale is exactly 1.
        assert_eq!(
            drag.apply_with_constraints(
                ResizeMode::Handle(Handle::BottomRight),
                Point::new(500, 300),
                monitor(),
                true,
            ),
            rect
        );

        // x grew but y halved: the box must shrink proportionally instead of ignoring
        // the move because it is not diagonal.
        let single_axis = drag.apply_with_constraints(
            ResizeMode::Handle(Handle::BottomRight),
            Point::new(700, 200),
            monitor(),
            true,
        );
        assert_eq!(single_axis, Rect::new(100, 100, 250, 200));
        assert!(single_axis.width() < rect.width());
        assert!(single_axis.height() < rect.height());

        let diagonal = drag.apply_with_constraints(
            ResizeMode::Handle(Handle::BottomRight),
            Point::new(500, 400),
            monitor(),
            true,
        );
        assert!(diagonal.width() > rect.width());
        assert!(diagonal.height() > rect.height());
    }

    #[test]
    fn shift_corner_resize_mirrors_past_the_fixed_corner() {
        // Regression: clamping the distance at zero pinned the rectangle to its minimum
        // size, so the bottom-right grip could not be dragged back past the top-left
        // corner. The pointer's quadrant around the fixed corner now decides the corner
        // signs, mirroring the box into the opposite quadrant exactly like the free path
        // does through `Rect::from_corners`.
        let rect = Rect::new(100, 100, 400, 300);
        let drag = SelectionDrag::new(rect, 96, Point::new(400, 300));

        // Straight through the fixed corner: both signs flip, the ratio is kept.
        let mirrored = drag.apply_with_constraints(
            ResizeMode::Handle(Handle::BottomRight),
            Point::new(0, 50),
            monitor(),
            true,
        );
        assert_eq!(mirrored, Rect::new(25, 50, 100, 100));

        // Past the fixed corner on one axis only.
        let quarter = drag.apply_with_constraints(
            ResizeMode::Handle(Handle::BottomRight),
            Point::new(50, 200),
            monitor(),
            true,
        );
        assert_eq!(quarter, Rect::new(50, 100, 100, 133));

        for resized in [mirrored, quarter] {
            assert!(resized.width() > 0 && resized.height() > 0, "{resized:?}");
            assert!(monitor().contains_rect(resized), "{resized:?}");
            let ratio_error = (i64::from(resized.width()) * i64::from(rect.height())
                - i64::from(resized.height()) * i64::from(rect.width()))
            .abs();
            assert!(ratio_error <= i64::from(rect.width()), "{resized:?}");
        }
    }

    #[test]
    fn shift_corner_resize_can_shrink_back_toward_the_fixed_corner() {
        let rect = Rect::new(100, 100, 400, 300);
        let drag = SelectionDrag::new(rect, 96, Point::new(400, 300));
        let resized = drag.apply_with_constraints(
            ResizeMode::Handle(Handle::BottomRight),
            Point::new(250, 200),
            monitor(),
            true,
        );
        assert!(resized.width() < rect.width());
        assert!(resized.height() < rect.height());
        let ratio_error = (i64::from(resized.width()) * i64::from(rect.height())
            - i64::from(resized.height()) * i64::from(rect.width()))
        .abs();
        assert!(ratio_error <= i64::from(rect.width()));
    }

    #[test]
    fn shift_corner_resize_ignores_the_press_point_and_the_hit_slop_offset() {
        // The old rule measured from the press point, so where the button landed (and
        // whether Shift was already down when it landed) changed the result. The
        // geometry now depends only on the drag's snapshot rectangle, its ratio and the
        // current pointer, which is what makes a mid-drag Shift identical to a Shift held
        // from the button-down.
        let rect = Rect::new(100, 100, 400, 300);
        let exact = SelectionDrag::new(rect, 96, Point::new(400, 300));
        // Pressed five pixels outside the visual corner but still inside the hit area.
        let sloppy = SelectionDrag::new(rect, 96, Point::new(405, 305));
        // Pressed on the opposite grip: an absurd anchor that must not change anything.
        let opposite = SelectionDrag::new(rect, 96, Point::new(100, 100));
        for pointer in [
            Point::new(250, 200),
            Point::new(500, 400),
            Point::new(90, 90),
            Point::new(700, 200),
        ] {
            let mode = ResizeMode::Handle(Handle::BottomRight);
            let expected = exact.apply_with_constraints(mode, pointer, monitor(), true);
            assert_eq!(
                sloppy.apply_with_constraints(mode, pointer, monitor(), true),
                expected,
                "hit slop changed the result at {pointer:?}"
            );
            assert_eq!(
                opposite.apply_with_constraints(mode, pointer, monitor(), true),
                expected,
                "press point changed the result at {pointer:?}"
            );
        }

        // Reversing toward the fixed corner still shrinks the box.
        let resized = sloppy.apply_with_constraints(
            ResizeMode::Handle(Handle::BottomRight),
            Point::new(250, 200),
            monitor(),
            true,
        );
        assert!(resized.width() < rect.width());
        assert!(resized.height() < rect.height());
    }

    #[test]
    fn shift_does_not_constrain_edge_or_move_drags() {
        let rect = Rect::new(100, 100, 400, 300);
        let drag = SelectionDrag::new(rect, 96, Point::new(400, 300));
        assert_eq!(
            drag.apply_with_constraints(
                ResizeMode::Handle(Handle::Right),
                Point::new(550, 300),
                monitor(),
                true,
            ),
            Rect::new(100, 100, 550, 300)
        );
        assert_eq!(
            drag.apply_with_constraints(ResizeMode::Move, Point::new(500, 400), monitor(), true),
            Rect::new(200, 200, 500, 400)
        );
    }

    #[test]
    fn shift_corner_resize_keeps_ratio_when_clamped_or_minimized() {
        let rect = Rect::new(100, 100, 400, 300);
        let drag = SelectionDrag::new(rect, 96, Point::new(400, 300));
        let clamped = drag.apply_with_constraints(
            ResizeMode::Handle(Handle::BottomRight),
            Point::new(5000, 5000),
            monitor(),
            true,
        );
        assert_eq!(clamped, Rect::new(100, 100, 1570, 1080));
        let clamped_error = (i64::from(clamped.width()) * i64::from(rect.height())
            - i64::from(clamped.height()) * i64::from(rect.width()))
        .abs();
        assert!(clamped_error <= i64::from(rect.width()));

        let minimum = drag.apply_with_constraints(
            ResizeMode::Handle(Handle::BottomRight),
            Point::new(101, 101),
            monitor(),
            true,
        );
        assert!(minimum.width() >= drag.snapshot.minimum_size());
        assert!(minimum.height() >= drag.snapshot.minimum_size());
        let minimum_error = (i64::from(minimum.width()) * i64::from(rect.height())
            - i64::from(minimum.height()) * i64::from(rect.width()))
        .abs();
        assert!(minimum_error <= i64::from(rect.width()));
    }

    #[test]
    fn shift_corner_resize_changes_by_pixels_instead_of_ratio_units() {
        let rect = Rect::new(100, 100, 400, 300);
        let drag = SelectionDrag::new(rect, 96, Point::new(400, 300));
        let first = drag.apply_with_constraints(
            ResizeMode::Handle(Handle::BottomRight),
            Point::new(401, 301),
            monitor(),
            true,
        );
        let second = drag.apply_with_constraints(
            ResizeMode::Handle(Handle::BottomRight),
            Point::new(402, 302),
            monitor(),
            true,
        );
        assert!(first.width() >= rect.width());
        assert!(second.width() > first.width());
        assert!((second.width() - first.width()) <= 2);
        assert!((second.height() - first.height()) <= 2);
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
        assert_eq!(
            monitor_layout.to_local(Point::new(-1920, 200)),
            Point::new(0, 0)
        );
        assert_eq!(
            monitor_layout.to_screen(Point::new(0, 0)),
            Point::new(-1920, 200)
        );
        assert_eq!(monitor_layout.local_bounds(), Rect::new(0, 0, 1920, 1080));
        assert_eq!(
            monitor_layout.local_work_area(),
            Rect::new(0, 0, 1920, 1040)
        );
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
        assert_eq!(
            geometry.panel.width(),
            geometry.source.width() * geometry.zoom as i32
        );
        assert_eq!(
            geometry.panel.height(),
            geometry.source.height() * geometry.zoom as i32
        );
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
        assert!(
            crosshair_radius(0) == 28,
            "dpi below 96 clamps to the 96 baseline"
        );
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

    /// 把整块工作区当作选框：面板按"放进选框正中间"排，等于居中在工作区里。
    fn whole(work_area: Rect) -> Rect {
        Rect::new(
            work_area.left,
            work_area.top,
            work_area.right,
            work_area.bottom,
        )
    }

    /// 图片卡是唯一允许让面板变高的东西：收起 42（10 + 22 + 10），展开再加一行 28 与 6 的行距。
    #[test]
    fn the_image_card_grows_the_panel_only_when_it_is_open() {
        let work_area = Rect::from_origin_size(Point::new(0, 0), 3840, 2160);
        let selection = whole(work_area);
        let closed = region_panel_layout(
            1.0,
            work_area,
            selection,
            false,
            PanelCard::collapsed(),
            SegmentWidths::fixed(1.0),
        );
        let open = region_panel_layout(
            1.0,
            work_area,
            selection,
            false,
            PanelCard::expanded(12),
            SegmentWidths::fixed(1.0),
        );
        assert_eq!(closed.panel.height(), 329, "277 + 卡间距 10 + 卡头一行 42");
        assert_eq!(open.panel.height(), 363, "再多一行圆角 28 与 6 的行距");
        // 卡片整宽、居内边距之内；卡头在卡顶，摘要不越过箭头。
        assert_eq!(closed.image_card.width(), closed.panel.width() - 32);
        assert_eq!(closed.image_head.left, closed.image_card.left + 10);
        assert!(closed.image_summary.right <= closed.image_chevron.left);
        assert_eq!(closed.radius_max, 128, "短边 2160 / 2 已超过 128 的封顶");
    }

    /// 命中与绘制同源：卡头就是开关；滑块把 x 映射成像素值；收起时那一行不存在。
    #[test]
    fn the_image_card_head_toggles_and_the_slider_maps_positions_to_values() {
        let work_area = Rect::from_origin_size(Point::new(0, 0), 3840, 2160);
        let selection = whole(work_area);
        let closed = region_panel_layout(
            1.0,
            work_area,
            selection,
            false,
            PanelCard::collapsed(),
            SegmentWidths::fixed(1.0),
        );
        assert_eq!(closed.radius_track.width(), 0, "收起时滑块压成空矩形");
        assert_eq!(
            closed.target_at(closed.image_head.center()),
            Some(RegionPanelTarget::Slot(PanelSlot::Image)),
        );

        let open = region_panel_layout(
            1.0,
            work_area,
            selection,
            false,
            PanelCard::expanded(64),
            SegmentWidths::fixed(1.0),
        );
        // 两端：滑块中心在轨道起点 ⇒ 0，在终点 ⇒ 上限；整段中点 ⇒ 一半。
        let thumb = open.radius_thumb.width();
        let left = Point::new(
            open.radius_groove.left + thumb / 2,
            open.radius_groove.center().y,
        );
        let right = Point::new(
            open.radius_groove.right - thumb / 2,
            open.radius_groove.center().y,
        );
        assert_eq!(open.target_at(left), Some(RegionPanelTarget::Radius(0)));
        assert_eq!(
            open.target_at(right),
            Some(RegionPanelTarget::Radius(open.radius_max as u32)),
        );
        assert_eq!(
            open.target_at(open.radius_groove.center()),
            Some(RegionPanelTarget::Radius(64)),
        );
        // 滑块跟着值走：64 的拇指在 0 的右边。
        let zero = region_panel_layout(
            1.0,
            work_area,
            selection,
            false,
            PanelCard::expanded(0),
            SegmentWidths::fixed(1.0),
        );
        assert!(open.radius_thumb.left > zero.radius_thumb.left);
        // 值块不在命中区里（它是读数，不是控件）。
        assert_eq!(open.target_at(open.radius_value.center()), None);
    }

    #[test]
    fn the_segment_radius_nests_inside_its_container_and_never_goes_negative() {
        assert_eq!(segment_radius(7, 3), 4);
        assert_eq!(segment_radius(7, 0), 7);
        assert_eq!(segment_radius(0, 0), 0);
        assert_eq!(segment_radius(7, 7), 0);
        assert_eq!(segment_radius(7, 9), 0, "内缩比外层还大时取 0，不给负半径");
        assert_eq!(segment_radius(0, 3), 0);
    }

    #[test]
    fn the_segment_slide_lands_on_both_ends_and_moves_monotonically() {
        let from = Rect::new(10, 20, 50, 44);
        let to = Rect::new(90, 20, 130, 44);
        assert_eq!(segment_slide(from, to, 0.0), from);
        assert_eq!(segment_slide(from, to, 1.0), to);
        // 越界夹住，不会滑到框外。
        assert_eq!(segment_slide(from, to, -1.0), from);
        assert_eq!(segment_slide(from, to, 2.0), to);
        // 中间落在两端之间（含端点：整数取整会让 ease-out 在 t≈0.9 时已经贴到终点），且一路向前。
        let mut previous = from.left;
        for step in 1..10 {
            let t = step as f32 / 10.0;
            let mid = segment_slide(from, to, t);
            assert!(
                mid.left >= from.left && mid.left <= to.left,
                "t={t} 跑到两端之外"
            );
            assert!(mid.left >= previous, "t={t} 往回走了");
            assert_eq!(mid.top, from.top, "只有横向在动");
            if t <= 0.8 {
                assert!(mid.left > from.left && mid.left < to.left, "t={t} 该在两端之间");
            }
            previous = mid.left;
        }
        // 缓动：四分之一时间就走完一半路程以上（ease-out），但不会提前到终点。
        let quarter = segment_slide(from, to, 0.25);
        assert!(quarter.left > from.left + (to.left - from.left) / 2);
        assert!(quarter.left < to.left);
    }

    #[test]
    fn the_pill_radius_is_half_the_height_and_never_negative() {
        assert_eq!(pill_radius(Rect::new(0, 0, 40, 24)), 12.0);
        // 奇数高度取半值：D2D 会把半径夹到短边的一半，所以两端仍是正圆。
        assert_eq!(pill_radius(Rect::new(0, 0, 40, 25)), 12.5);
        assert_eq!(pill_radius(Rect::new(0, 0, 40, 1)), 0.5);
        assert_eq!(pill_radius(Rect::new(0, 0, 40, 0)), 0.0);
        assert_eq!(pill_radius(Rect::new(0, 10, 40, 4)), 0.0, "倒置矩形不给负半径");
    }

    /// 分段器显示大写、角标保留小写，且两者只差大小写——这样加新单位时不会一边改一边忘。
    #[test]
    fn the_segmented_control_capitalises_what_the_badge_keeps_lowercase() {
        for unit in LengthUnit::ALL {
            assert_eq!(unit.segment_label(), unit.suffix().to_uppercase());
            assert_ne!(unit.segment_label(), unit.suffix());
        }
    }

    #[test]
    fn the_region_panel_is_420_dip_wide_and_centred() {
        let work_area = Rect::new(0, 0, 3840, 2160);
        let panel = region_panel_layout(1.0, work_area, whole(work_area), false, PanelCard::collapsed(), SegmentWidths::fixed(1.0));
        assert_eq!(panel.panel.width(), 420);
        assert_eq!(
            panel.panel.left + panel.panel.width() / 2,
            work_area.left + work_area.width() / 2,
            "水平居中"
        );
        // 面板高度随内容变化，奇偶取整会差 1 px —— 居中允许 1 px 容差（与别处的取整断言同一套）。
        assert!(
            (panel.panel.top + panel.panel.height() / 2 - (work_area.top + work_area.height() / 2))
                .abs()
                <= 1,
            "垂直居中（±1 px 取整）"
        );
        // 内容不出面板：页头、两张卡、两行、页脚都在里面。
        for card in panel.cards {
            assert!(panel.panel.contains_rect(card));
        }
        assert!(panel.panel.contains_rect(panel.ratio));
        assert!(panel.panel.contains_rect(panel.destination_track));
        assert!(panel.panel.contains_rect(panel.start));
        assert!(panel.panel.contains_rect(panel.close));
        assert_eq!(panel.cancel.right + 8, panel.start.left);
        assert_eq!(panel.start.right, panel.panel.right - 16);
        // 一张卡里：标题行在上，数字在下，标签在数字下面。
        assert!(panel.card_titles[0].bottom <= panel.fields[0].1.top);
        assert!(panel.fields[0].1.bottom <= panel.field_labels[0].top);
        // 卡片底部要留出气口（标签不贴边框），两张卡用同一套数值（docs/23 §14.55）。
        for card in panel.cards {
            assert!(
                panel.field_labels[0].bottom + 8 <= card.bottom,
                "宽/高/X/Y 标签距卡片下沿太近"
            );
        }
        assert_eq!(panel.field_labels[0].bottom, panel.field_labels[2].bottom);
        // 分段器：选中块四面都缩在容器里，容器宽度不变（页头不受影响）。
        for (_, segment) in panel.unit {
            assert!(segment.left >= panel.unit_track.left + 2);
            assert!(segment.top >= panel.unit_track.top + 2);
            assert!(segment.right <= panel.unit_track.right - 2);
            assert!(segment.bottom <= panel.unit_track.bottom - 2);
        }
        assert_eq!(panel.unit_track.width(), 64);
        // 三个开关之间要有具名常量给的间距（docs/23 §14.55.7）。
        for pair in panel.toggles.windows(2) {
            assert_eq!(
                pair[1].1.left - pair[0].1.right,
                REGION_TOGGLE_GAP_DIP as i32,
                "开关之间的间距必须正好是 REGION_TOGGLE_GAP_DIP"
            );
        }
        // 分段器与缩放块的高度只有一个来源：这一行的胶囊高度。
        let chip_height = panel.toggles[0].1.height();
        assert_eq!(panel.unit_track.height(), chip_height, "分段器与胶囊不等高");
        assert_eq!(panel.scale.height(), chip_height, "缩放块与胶囊不等高");
        assert_eq!(
            panel.unit[0].1.top - panel.unit_track.top,
            REGION_UNIT_INSET_DIP as i32,
            "高度变了，内缩仍要成立"
        );
        // 单位分段器与倍率已从页头搬到角标行右侧（docs/23 §14.55.3）。
        let row = panel.toggles[0].1;
        for rect in [panel.unit_track, panel.scale] {
            assert!(
                rect.top >= row.top && rect.bottom <= row.bottom,
                "两个控件要落在角标行内"
            );
            assert_eq!(rect.center().y, row.center().y, "两个控件在角标行里垂直居中");
        }
        assert_eq!(panel.scale.right, panel.start.right, "倍率右侧与页脚按钮同一条竖线");
        assert_eq!(panel.scale.right, panel.delay_track.right, "与延时行右对齐");
        assert_eq!(
            panel.unit_track.right + 8,
            panel.scale.left,
            "分段器与倍率的间距沿用页头原值"
        );
        assert!(
            panel.unit_track.left - panel.toggles[2].1.right
                // 本测试 scale = 1.0，dip 即原值。
                >= REGION_TOGGLE_CONTROLS_GAP_DIP as i32,
            "开关组与右侧控件之间要留够间距"
        );
        assert!(panel.title.bottom <= panel.unit_track.top, "页头里不再有这两个控件");
        assert!(panel.title.right + 8 <= panel.close.left, "标题不许压到关闭按钮");
        // 延时行与单位分段器同一套：四段都在容器内、四面内缩 >=2；容器宽 = 四段原宽（4 x 30）。
        for (_, segment) in panel.delay {
            assert!(segment.left >= panel.delay_track.left + 2);
            assert!(segment.top >= panel.delay_track.top + 2);
            assert!(segment.right <= panel.delay_track.right - 2);
            assert!(segment.bottom <= panel.delay_track.bottom - 2);
        }
        for pair in panel.delay.windows(2) {
            assert_eq!(pair[0].1.right, pair[1].1.left, "四段之间不该有缝");
        }
        // 容器 = 段数 × 段宽 + 两侧内缩；兜底段宽 = REGION_SEGMENT_DIP − 内缩 ⇒ 4×27 + 6 = 114。
        assert_eq!(panel.delay_track.width(), 114);
    }

    #[test]
    fn the_four_fields_share_one_column_per_card() {
        let work_area = Rect::new(0, 0, 3840, 2160);
        let panel = region_panel_layout(1.0, work_area, whole(work_area), false, PanelCard::collapsed(), SegmentWidths::fixed(1.0));
        // 两张卡用同一套内部列几何：宽与 X 在各自卡里的第一列、高与 Y 在第二列，
        // 所以左右两卡的数字在卡内对齐（卡与卡之间差着各自的位置，不该相同）。
        for index in [0, 2] {
            assert_eq!(
                panel.fields[index].1.left - panel.cards[index / 2].left,
                panel.fields[0].1.left - panel.cards[0].left
            );
        }
        assert_eq!(
            panel.fields[1].1.left - panel.cards[0].left,
            panel.fields[3].1.left - panel.cards[1].left
        );
        assert_eq!(panel.fields[0].1.width(), panel.fields[1].1.width());
        assert_eq!(panel.fields[0].1.width(), panel.fields[2].1.width());
        assert_eq!(panel.fields[0].1.top, panel.fields[1].1.top);
        assert_eq!(panel.fields[2].1.top, panel.fields[3].1.top);
        // 标签与它的数字同列同宽。
        for index in 0..4 {
            assert_eq!(panel.fields[index].1.left, panel.field_labels[index].left);
            assert_eq!(panel.fields[index].1.width(), panel.field_labels[index].width());
        }
        // 分隔符在两条数字之间，不压住任何一格。
        assert_eq!(panel.separators[0].left, panel.fields[0].1.right);
        assert_eq!(panel.separators[0].right, panel.fields[1].1.left);
        assert_eq!(panel.separators[1].left, panel.fields[2].1.right);
        assert_eq!(panel.separators[1].right, panel.fields[3].1.left);
    }

    #[test]
    fn every_control_the_region_panel_draws_can_be_hit() {
        let work_area = Rect::new(0, 0, 3840, 2160);
        let panel = region_panel_layout(1.0, work_area, whole(work_area), false, PanelCard::collapsed(), SegmentWidths::fixed(1.0));
        for (unit, rect) in panel.unit {
            assert_eq!(
                panel.target_at(rect.center()),
                Some(RegionPanelTarget::Unit(unit))
            );
        }
        for (slot, rect) in panel.actions {
            assert_eq!(
                panel.target_at(rect.center()),
                Some(RegionPanelTarget::Slot(slot))
            );
        }
        for (field, rect) in panel.fields {
            assert_eq!(
                panel.target_at(rect.center()),
                Some(RegionPanelTarget::Slot(field.slot()))
            );
        }
        for (toggle, rect) in panel.toggles {
            assert_eq!(
                panel.target_at(rect.center()),
                Some(RegionPanelTarget::Slot(toggle.slot()))
            );
        }
        // 比例按钮在尺寸卡头里，命中与绘制同源。
        assert_eq!(
            panel.target_at(panel.ratio.center()),
            Some(RegionPanelTarget::Slot(PanelSlot::Ratio))
        );
        for (destination, rect) in panel.destination {
            assert_eq!(
                panel.target_at(rect.center()),
                Some(RegionPanelTarget::Destination(destination)),
                "去向每一段都要命中自己"
            );
        }
        for (delay, rect) in panel.delay {
            assert_eq!(
                panel.target_at(rect.center()),
                Some(RegionPanelTarget::Delay(delay)),
                "延时的每一段都要命中自己，逐段 hover 才有依据"
            );
        }
        assert_eq!(
            panel.target_at(panel.start.center()),
            Some(RegionPanelTarget::Slot(PanelSlot::Start))
        );
        assert_eq!(
            panel.target_at(panel.close.center()),
            Some(RegionPanelTarget::Slot(PanelSlot::Cancel))
        );
        assert_eq!(
            panel.target_at(panel.cancel.center()),
            Some(RegionPanelTarget::Slot(PanelSlot::Cancel))
        );
        // 面板的空白：不是控件，点击落到面板自己身上（不穿透到下面的截图）。
        assert_eq!(panel.target_at(panel.status.center()), None);
        // 卡头左边还是空白（比例按钮与交换图标在右边），点它不该命中任何控件。
        let blank = Point::new(panel.card_titles[0].left + 4, panel.card_titles[0].center().y);
        assert_eq!(panel.target_at(blank), None);
        assert_eq!(panel.menu_item_at(panel.ratio.center()), None);
    }

    #[test]
    fn the_ratio_menu_hangs_below_its_button_and_stays_on_screen() {
        let work_area = Rect::new(0, 0, 3840, 2160);
        let closed = region_panel_layout(1.0, work_area, whole(work_area), false, PanelCard::collapsed(), SegmentWidths::fixed(1.0));
        assert_eq!(closed.menu, None, "收起时没有下拉");
        assert_eq!(closed.menu_items[0].height(), 0);

        let open = region_panel_layout(1.0, work_area, whole(work_area), true, PanelCard::collapsed(), SegmentWidths::fixed(1.0));
        let menu = open.menu.expect("展开时有下拉");
        // 菜单与按钮**解耦**：右缘对齐卡片右缘，宽度是量出来的最宽项，比按钮宽。
        assert_eq!(menu.right, open.cards[0].right);
        assert_eq!(menu.width(), SegmentWidths::fixed(1.0).ratio_menu);
        assert!(menu.width() > open.ratio.width(), "菜单比按钮宽，向右展开");
        assert!(menu.top >= open.ratio.bottom, "挂在按钮下面");
        assert!(work_area.contains_rect(menu));
        for (index, item) in open.menu_items.iter().enumerate() {
            assert!(menu.contains_rect(*item), "第 {index} 项在下拉里");
            assert_eq!(
                open.target_at(item.center()),
                Some(RegionPanelTarget::MenuItem(index))
            );
            assert_eq!(open.menu_item_at(item.center()), Some(index));
        }
        // 下拉自己的内边距：是面板的一部分（点击不该穿透），但不是控件。
        let padding = Point::new(menu.left + 2, menu.top + 2);
        assert_eq!(open.target_at(padding), Some(RegionPanelTarget::MenuPadding));
        assert_eq!(open.menu_item_at(padding), None);

        // 面板贴着工作区底边时，下拉被拉回屏内而不是画到外面。
        let short = Rect::new(0, 0, 3840, 300);
        let pinned = region_panel_layout(1.0, short, whole(short), true, PanelCard::collapsed(), SegmentWidths::fixed(1.0)).menu.expect("下拉");
        assert!(short.contains_rect(pinned), "{pinned:?}");
    }

    #[test]
    fn the_panel_goes_outside_a_selection_too_small_to_hold_it() {
        let work_area = Rect::new(0, 0, 3840, 2160);
        // 一个装不下面板的小选框：面板挪到它下面，而不是压住它。
        let selection = Rect::new(600, 400, 1000, 620);
        let panel = region_panel_layout(1.0, work_area, selection, false, PanelCard::collapsed(), SegmentWidths::fixed(1.0));
        assert!(panel.panel.top >= selection.bottom, "{:?}", panel.panel);
        assert!(work_area.contains_rect(panel.panel));
        assert_eq!(
            panel.panel.left + panel.panel.width() / 2,
            selection.left + selection.width() / 2,
            "水平方向仍然对着选框"
        );
        // 下方也没地方：翻到上面。
        let low = Rect::new(600, 2160 - 300, 1000, 2160 - 80);
        let above = region_panel_layout(1.0, work_area, low, false, PanelCard::collapsed(), SegmentWidths::fixed(1.0));
        assert!(above.panel.bottom <= low.top, "{:?}", above.panel);
        assert!(work_area.contains_rect(above.panel));
        // 选框装得下就放进正中间——那是用户点尺寸栏时最想看到的位置。
        let roomy = Rect::new(200, 200, 3000, 1800);
        let inside = region_panel_layout(1.0, work_area, roomy, false, PanelCard::collapsed(), SegmentWidths::fixed(1.0));
        assert!(roomy.contains_rect(inside.panel), "{:?}", inside.panel);
        assert_eq!(
            inside.panel.left + inside.panel.width() / 2,
            roomy.left + roomy.width() / 2
        );
    }

    #[test]
    fn the_region_panel_grows_with_the_display_scale() {
        let work_area = Rect::new(0, 0, 3840, 2160);
        let at_100 = region_panel_layout(1.0, work_area, whole(work_area), false, PanelCard::collapsed(), SegmentWidths::fixed(1.0));
        let at_200 = region_panel_layout(2.0, work_area, whole(work_area), false, PanelCard::collapsed(), SegmentWidths::fixed(2.0));
        assert_eq!(at_200.panel.width(), at_100.panel.width() * 2);
        assert_eq!(at_200.panel.height(), at_100.panel.height() * 2);
        assert_eq!(at_200.toggles[0].1.width(), at_100.toggles[0].1.width() * 2);
        assert_eq!(at_200.delay[0].1.height(), at_100.delay[0].1.height() * 2);
        // 由卡片宽度折半算出来的列宽会带上半像素的取整差：一像素之内，不是两套布局。
        let cell_100 = at_100.fields[0].1.width();
        let cell_200 = at_200.fields[0].1.width();
        assert!(
            (cell_200 - cell_100 * 2).abs() <= 1,
            "列宽 {cell_100} → {cell_200}，超过半像素取整能解释的范围"
        );
        // 退化缩放仍然排出一块能用的面板，而不是塌成 0。
        let degenerate = region_panel_layout(0.0, work_area, whole(work_area), false, PanelCard::collapsed(), SegmentWidths::fixed(0.0));
        assert_eq!(degenerate.panel.width(), at_100.panel.width());
    }

    /// 分段器：**每段同宽**，宽度就是传进来的那个（两个分段器各一份），且右对齐不受宽度影响。
    #[test]
    fn the_segment_widths_give_every_option_the_same_width() {
        let work_area = Rect::new(0, 0, 3840, 2160);
        for widths in [
            SegmentWidths {
                unit: 24,
                delay: 60,
                ..SegmentWidths::fixed(1.0)
            },
            SegmentWidths {
                unit: 40,
                delay: 34,
                ..SegmentWidths::fixed(1.0)
            },
            SegmentWidths {
                unit: 1,
                delay: 1,
                ..SegmentWidths::fixed(1.0)
            },
        ] {
            let panel = region_panel_layout(1.0, work_area, whole(work_area), false, PanelCard::collapsed(), widths);
            for (_, rect) in panel.unit {
                assert_eq!(rect.width(), widths.unit, "单位段宽 = 传进来的值");
                assert!(panel.unit_track.contains_rect(rect), "段在容器内");
            }
            for (_, rect) in panel.delay {
                assert_eq!(rect.width(), widths.delay, "延时每段同宽");
                assert!(panel.delay_track.contains_rect(rect), "段在容器内");
            }
            // 容器 = 段数 × 段宽 + 两侧内缩（没有多余的空隙，也没有 1 px 的差）。
            let unit_inset = panel.unit[0].1.top - panel.unit_track.top;
            let delay_inset = panel.delay[0].1.top - panel.delay_track.top;
            assert!(unit_inset > 0 && delay_inset > 0, "段必须缩在容器内");
            assert_eq!(panel.unit_track.width(), widths.unit * 2 + unit_inset * 2);
            assert_eq!(panel.delay_track.width(), widths.delay * 4 + delay_inset * 2);
            // 右对齐是钉住的：容器右缘与倍率的关系不随宽度变。
            assert_eq!(panel.unit_track.right, panel.scale.left - 8);
            assert_eq!(panel.delay_track.right, panel.start.right);
            assert_eq!(panel.scale.right, panel.start.right);
        }
    }

    /// 命中判定与绘制读的是同一份宽度：段中心的 `target_at` 必须命中该段本身。
    ///
    /// 这条守的是上一轮没做完第 1 项的原因——**绘制与命中各调一次布局**，一旦两次传的宽度不同，
    /// 点击位置就会和画出来的错开。
    #[test]
    fn the_hit_test_reads_the_same_rectangles_the_drawing_does() {
        let work_area = Rect::new(0, 0, 3840, 2160);
        let widths = SegmentWidths {
            unit: 37,
            delay: 41,
            ..SegmentWidths::fixed(1.0)
        };
        let panel = region_panel_layout(1.0, work_area, whole(work_area), false, PanelCard::collapsed(), widths);
        for (delay, rect) in panel.delay {
            assert_eq!(rect.width(), widths.delay);
            assert_eq!(
                panel.target_at(rect.center()),
                Some(RegionPanelTarget::Delay(delay)),
                "延时 {delay:?} 的中心必须命中它自己"
            );
        }
        for (unit, rect) in panel.unit {
            assert_eq!(rect.width(), widths.unit);
            assert_eq!(
                panel.target_at(rect.center()),
                Some(RegionPanelTarget::Unit(unit)),
                "单位 {unit:?} 的中心必须命中它自己"
            );
        }
    }
}
