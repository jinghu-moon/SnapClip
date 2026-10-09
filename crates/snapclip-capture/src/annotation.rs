//! Object-based annotation document: pure state, no Win32/GPU.
//!
//! Coordinates use **physical pixels** (same frame-local system as
//! [`super::geometry::Point`]) so that screen preview and PNG export replay
//! identical geometry. Undo history is pushed only at operation boundaries
//! (create, delete, style change, drag-end), never on every mouse move.

use super::geometry::{Point, Rect};

// ─── ID ───────────────────────────────────────────────────────────────────────

pub type AnnotationId = u64;

// ─── Kind & Geometry ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AnnotationKind {
    Rectangle,
    Ellipse,
    Arrow,
    Line,
    Text,
    Freehand,
    Highlight,
}

/// A low-frequency toolbar instruction issued by the overlay's toolbar, carried over
/// the annotation mailbox (`WindowsOverlay::request_annotation`) and applied to the
/// [`AnnotationDocument`] on the overlay thread.
///
/// Deliberately coarse: the toolbar never drives per-pixel or per-move work
/// (docs/11 §7.1 "工具栏不进入像素管线"). Style setters apply to the selected object
/// when one exists and, when none is selected, to the pending `default_style` used by
/// the next draft — so a style change only ever mutates the current selection (docs/11
/// §8.2 "修改颜色、线宽、字体只修改当前对象样式").
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AnnotationCommand {
    /// Switch to the select / move / resize tool (no drawing).
    SelectTool,
    /// Switch to a drawing tool.
    Tool(AnnotationKind),
    /// Stroke colour (RGBA 0–1) of the selection, else the default style.
    SetStrokeColor([f32; 4]),
    /// Fill colour of the selection, else the default style; `None` disables fill.
    SetFillColor(Option<[f32; 4]>),
    /// Stroke width in physical pixels of the selection, else the default style.
    SetStrokeWidth(f32),
    Undo,
    Redo,
    Delete,
    Duplicate,
    BringForward,
    SendBackward,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AnnotationGeometry {
    Rect {
        /// Normalised so left ≤ right, top ≤ bottom.
        bounds: Rect,
    },
    Ellipse {
        bounds: Rect,
    },
    Line {
        start: Point,
        end: Point,
    },
    Arrow {
        start: Point,
        end: Point,
    },
    Text {
        /// Top-left anchor in physical pixels.
        position: Point,
        content: String,
    },
    Freehand {
        points: Vec<Point>,
    },
    Highlight {
        points: Vec<Point>,
    },
}

// ─── Style ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AnnotationStyle {
    /// RGBA, each in 0.0–1.0.
    pub stroke_color: [f32; 4],
    /// `None` = no fill.
    pub fill_color: Option<[f32; 4]>,
    /// Stroke width in physical pixels.
    pub stroke_width: f32,
    /// 0.0–1.0; multiplied into stroke/fill alpha at draw time.
    pub opacity: f32,
    /// Font size in DIP (only meaningful for `AnnotationKind::Text`).
    pub font_size: f32,
    /// Arrow-head side length in physical pixels (Arrow only).
    pub arrow_head_size: f32,
}

impl Default for AnnotationStyle {
    fn default() -> Self {
        Self {
            // Accent blue, same as selection border.
            stroke_color: [0.0, 0.47, 0.83, 1.0],
            fill_color: None,
            stroke_width: 3.0,
            opacity: 1.0,
            font_size: 16.0,
            arrow_head_size: 14.0,
        }
    }
}

// ─── Item ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AnnotationItem {
    pub id: AnnotationId,
    pub kind: AnnotationKind,
    pub geometry: AnnotationGeometry,
    pub style: AnnotationStyle,
}

impl AnnotationItem {
    /// Axis-aligned bounding box **including** stroke width.
    pub fn bounds(&self) -> Rect {
        let pad = (self.style.stroke_width / 2.0).ceil() as i32 + 1;
        match &self.geometry {
            AnnotationGeometry::Rect { bounds } | AnnotationGeometry::Ellipse { bounds } => {
                bounds.inflate(pad)
            }
            AnnotationGeometry::Line { start, end }
            | AnnotationGeometry::Arrow { start, end } => {
                Rect::from_corners(*start, *end).inflate(pad + self.style.arrow_head_size.ceil() as i32)
            }
            AnnotationGeometry::Text { position, content } => {
                // Rough estimate; real width comes from DirectMeasure at draw time.
                let approx_w = (content.chars().count() as i32 * (self.style.font_size as i32)) / 2;
                let approx_h = self.style.font_size as i32 * 2;
                Rect::from_origin_size(*position, approx_w.max(10), approx_h).inflate(pad)
            }
            AnnotationGeometry::Freehand { points }
            | AnnotationGeometry::Highlight { points } => {
                if points.is_empty() {
                    return Rect::from_origin_size(Point::new(0, 0), 1, 1);
                }
                let mut r = Rect::from_origin_size(points[0], 1, 1);
                for p in &points[1..] {
                    r = r.union(Rect::from_origin_size(*p, 1, 1));
                }
                let extra = if matches!(self.kind, AnnotationKind::Highlight) { 16 } else { 0 };
                r.inflate(pad + extra)
            }
        }
    }

    /// Whether `point` is within this item's interactive region.
    pub fn hit_test(&self, point: Point) -> bool {
        match &self.geometry {
            AnnotationGeometry::Rect { bounds } => {
                bounds.inflate(5).contains(point)
            }
            AnnotationGeometry::Ellipse { bounds } => {
                bounds.inflate(5).contains(point)
            }
            AnnotationGeometry::Line { start, end }
            | AnnotationGeometry::Arrow { start, end } => {
                let thr = (self.style.stroke_width / 2.0 + 5.0) as f64;
                dist_point_to_segment(point, *start, *end) <= thr
            }
            AnnotationGeometry::Text { position, .. } => {
                self.bounds().contains(point)
                    || Point::new(position.x, position.y) == point
            }
            AnnotationGeometry::Freehand { points }
            | AnnotationGeometry::Highlight { points } => {
                let thr = (self.style.stroke_width / 2.0 + 5.0) as f64;
                points.windows(2).any(|w| dist_point_to_segment(point, w[0], w[1]) <= thr)
            }
        }
    }

    /// The control point nearest `point`, if one is within `tol` pixels.
    ///
    /// Only bounds- and segment-based kinds are resizable; freehand, highlight and
    /// text return `None` (they move but never resize).
    pub fn handle_at(&self, point: Point, tol: i32) -> Option<AnnotationHandle> {
        let candidates: Vec<(AnnotationHandle, Point)> = match &self.geometry {
            AnnotationGeometry::Rect { bounds } | AnnotationGeometry::Ellipse { bounds } => vec![
                (AnnotationHandle::Corner(0), Point::new(bounds.left, bounds.top)),
                (AnnotationHandle::Corner(1), Point::new(bounds.right, bounds.top)),
                (AnnotationHandle::Corner(2), Point::new(bounds.left, bounds.bottom)),
                (AnnotationHandle::Corner(3), Point::new(bounds.right, bounds.bottom)),
            ],
            AnnotationGeometry::Line { start, end } | AnnotationGeometry::Arrow { start, end } => {
                vec![
                    (AnnotationHandle::Endpoint(0), *start),
                    (AnnotationHandle::Endpoint(1), *end),
                ]
            }
            _ => return None,
        };
        candidates
            .into_iter()
            .find(|(_, p)| (p.x - point.x).abs() <= tol && (p.y - point.y).abs() <= tol)
            .map(|(h, _)| h)
    }

    /// Drag `handle` to `to`, keeping the opposite anchor fixed (no undo here;
    /// the caller commits a snapshot at resize-end via [`commit_drag`]).
    pub fn resize(&mut self, handle: AnnotationHandle, to: Point) {
        match (&mut self.geometry, handle) {
            (AnnotationGeometry::Rect { bounds } | AnnotationGeometry::Ellipse { bounds }, AnnotationHandle::Corner(c)) => {
                let opposite = match c {
                    0 => Point::new(bounds.right, bounds.bottom),
                    1 => Point::new(bounds.left, bounds.bottom),
                    2 => Point::new(bounds.right, bounds.top),
                    _ => Point::new(bounds.left, bounds.top),
                };
                *bounds = Rect::from_corners(to, opposite);
            }
            (AnnotationGeometry::Line { start, end } | AnnotationGeometry::Arrow { start, end }, AnnotationHandle::Endpoint(e)) => {
                if e == 0 {
                    *start = to;
                } else {
                    *end = to;
                }
            }
            _ => {}
        }
    }
}

// ─── Snapshot ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DocumentSnapshot {
    items: Vec<AnnotationItem>,
    selected_id: Option<AnnotationId>,
}

// ─── Resize handles ───────────────────────────────────────────────────────────

/// A draggable control point of an existing item.
///
/// Bounds-based items (`Rectangle`, `Ellipse`) expose four corners;
/// segment-based items (`Line`, `Arrow`) expose their two endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationHandle {
    /// 0 = top-left, 1 = top-right, 2 = bottom-left, 3 = bottom-right.
    Corner(u8),
    /// 0 = start, 1 = end.
    Endpoint(u8),
}

// ─── Document ─────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct AnnotationDocument {
    items: Vec<AnnotationItem>,
    selected_id: Option<AnnotationId>,
    undo: Vec<DocumentSnapshot>,
    redo: Vec<DocumentSnapshot>,
    /// Style applied to newly created items.
    pub default_style: AnnotationStyle,
    /// The item currently being drawn (not yet committed to `items`).
    pub draft: Option<AnnotationItem>,
    /// Monotonic per-document id source (see [`Self::alloc_id`]).
    next_id: AnnotationId,
}

impl AnnotationDocument {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            selected_id: None,
            undo: Vec::new(),
            redo: Vec::new(),
            default_style: AnnotationStyle::default(),
            draft: None,
            next_id: 1,
        }
    }

    // ── Accessors ─────────────────────────────────────────────────────────────

    pub fn items(&self) -> &[AnnotationItem] {
        &self.items
    }

    pub fn item(&self, id: AnnotationId) -> Option<&AnnotationItem> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn selected(&self) -> Option<&AnnotationItem> {
        self.selected_id.and_then(|id| self.items.iter().find(|i| i.id == id))
    }

    pub fn selected_id(&self) -> Option<AnnotationId> {
        self.selected_id
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.draft.is_none()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    // ── Mutation ──────────────────────────────────────────────────────────────

    /// Select an item by id (or clear selection with `None`). Does NOT push undo.
    pub fn select(&mut self, id: Option<AnnotationId>) {
        self.selected_id = id;
    }

    /// Create a new item from `kind` + `geometry` using `default_style`.
    /// Pushes an undo snapshot. Returns the new item's id.
    pub fn create_item(
        &mut self,
        kind: AnnotationKind,
        geometry: AnnotationGeometry,
    ) -> AnnotationId {
        self.push_undo();
        let id = self.alloc_id();
        let item = AnnotationItem {
            id,
            kind,
            geometry,
            style: self.default_style.clone(),
        };
        self.items.push(item);
        self.selected_id = Some(id);
        id
    }

    /// Replace the geometry of an existing item. Pushes an undo snapshot.
    /// Use this for discrete geometry changes (e.g., resize snap).
    pub fn set_geometry(&mut self, id: AnnotationId, geometry: AnnotationGeometry) {
        self.push_undo();
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.geometry = geometry;
        }
    }

    /// Update the style of a single item. Pushes an undo snapshot.
    pub fn set_style(&mut self, id: AnnotationId, style: AnnotationStyle) {
        self.push_undo();
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.style = style;
        }
    }

    /// Apply a style edit to the selected item, or to `default_style` when nothing is
    /// selected. A selection edit is one undo step; editing the pending default is not
    /// a document mutation and records no undo (docs/11 §8.2 "只修改当前对象样式").
    pub fn apply_style(&mut self, mutate: impl FnOnce(&mut AnnotationStyle)) {
        match self.selected_id {
            Some(id) => {
                self.push_undo();
                if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
                    mutate(&mut item.style);
                }
            }
            None => mutate(&mut self.default_style),
        }
    }

    /// Move an item by `(dx, dy)` **without** pushing undo (called on every mouse
    /// move). Use [`commit_drag`] when the drag ends to record the undo step.
    pub fn translate_item(&mut self, id: AnnotationId, dx: i32, dy: i32) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            translate_geometry(&mut item.geometry, dx, dy);
        }
    }

    /// Resize an item by dragging `handle` to `to`, **without** pushing undo (called
    /// on every mouse move). Commit the pre-resize snapshot with [`commit_drag`].
    pub fn resize_item(&mut self, id: AnnotationId, handle: AnnotationHandle, to: Point) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.resize(handle, to);
        }
    }

    /// The resize handle of item `id` under `point`, if any.
    pub fn handle_at(&self, id: AnnotationId, point: Point, tol: i32) -> Option<AnnotationHandle> {
        self.item(id).and_then(|i| i.handle_at(point, tol))
    }

    /// Call at drag-end to push the pre-drag state as a single undo step.
    /// `pre_drag_snapshot` should be taken before the first `translate_item` call.
    pub fn commit_drag(&mut self, snapshot: DocumentSnapshot) {
        self.undo.push(snapshot);
        self.redo.clear();
    }

    /// Take a snapshot to be passed to [`commit_drag`] at drag start.
    pub fn snapshot(&self) -> DocumentSnapshot {
        DocumentSnapshot {
            items: self.items.clone(),
            selected_id: self.selected_id,
        }
    }

    /// Delete the selected item and push an undo snapshot.
    pub fn delete_selected(&mut self) {
        let Some(id) = self.selected_id else { return };
        self.push_undo();
        self.items.retain(|i| i.id != id);
        self.selected_id = None;
    }

    /// Duplicate the selected item (offset by 10 px) and select the copy.
    pub fn duplicate_selected(&mut self) {
        let Some(id) = self.selected_id else { return };
        let Some(src) = self.items.iter().find(|i| i.id == id).cloned() else { return };
        self.push_undo();
        let new_id = self.alloc_id();
        let mut copy = src;
        copy.id = new_id;
        translate_geometry(&mut copy.geometry, 10, 10);
        self.items.push(copy);
        self.selected_id = Some(new_id);
    }

    /// Move the selected item to the top of the draw stack (end of the list).
    pub fn bring_to_front(&mut self) {
        let Some(id) = self.selected_id else { return };
        self.push_undo();
        if let Some(pos) = self.items.iter().position(|i| i.id == id) {
            let item = self.items.remove(pos);
            self.items.push(item);
        }
    }

    /// Move the selected item to the bottom of the draw stack (front of the list).
    pub fn send_to_back(&mut self) {
        let Some(id) = self.selected_id else { return };
        self.push_undo();
        if let Some(pos) = self.items.iter().position(|i| i.id == id) {
            let item = self.items.remove(pos);
            self.items.insert(0, item);
        }
    }

    /// Nudge the selected item one layer up the draw stack. A no-op — recording no
    /// undo step — when nothing is selected or the item is already on top, so the
    /// toolbar can stay enabled without polluting history.
    pub fn bring_forward(&mut self) {
        let Some(id) = self.selected_id else { return };
        let Some(pos) = self.items.iter().position(|i| i.id == id) else { return };
        if pos + 1 >= self.items.len() {
            return;
        }
        self.push_undo();
        self.items.swap(pos, pos + 1);
    }

    /// Nudge the selected item one layer down the draw stack. A no-op when nothing
    /// is selected or the item is already at the bottom.
    pub fn send_backward(&mut self) {
        let Some(id) = self.selected_id else { return };
        let Some(pos) = self.items.iter().position(|i| i.id == id) else { return };
        if pos == 0 {
            return;
        }
        self.push_undo();
        self.items.swap(pos, pos - 1);
    }

    /// Route a coarse toolbar [`AnnotationCommand`] to the matching document
    /// operation, so the IPC / overlay handler stays a thin forwarder instead of a
    /// `match` scatter (docs/12 Phase 5).
    ///
    /// Tool selection (`SelectTool` / `Tool`) is presentation state owned by the
    /// overlay controller, not the document, so those variants are intentionally
    /// no-ops here — the controller handles them before delegating the rest. The
    /// match is exhaustive on purpose: a new command variant must be routed
    /// explicitly rather than silently dropped by a wildcard arm.
    pub fn execute(&mut self, command: AnnotationCommand) {
        match command {
            AnnotationCommand::Undo => {
                self.undo();
            }
            AnnotationCommand::Redo => {
                self.redo();
            }
            AnnotationCommand::Delete => self.delete_selected(),
            AnnotationCommand::Duplicate => self.duplicate_selected(),
            AnnotationCommand::BringForward => self.bring_forward(),
            AnnotationCommand::SendBackward => self.send_backward(),
            AnnotationCommand::SetStrokeColor(color) => {
                self.apply_style(|style| style.stroke_color = color)
            }
            AnnotationCommand::SetFillColor(color) => {
                self.apply_style(|style| style.fill_color = color)
            }
            AnnotationCommand::SetStrokeWidth(width) => {
                self.apply_style(|style| style.stroke_width = width)
            }
            AnnotationCommand::SelectTool | AnnotationCommand::Tool(_) => {}
        }
    }

    /// Hit-test from top of the stack to bottom (reverse list order); returns the id
    /// of the first hit.
    pub fn hit_test(&self, point: Point) -> Option<AnnotationId> {
        self.items.iter().rev().find(|i| i.hit_test(point)).map(|i| i.id)
    }

    // ── Draft (in-progress creation) ──────────────────────────────────────────

    /// Begin an in-progress draft of `kind` anchored at `geometry`. Assigns a fresh
    /// id and `default_style`, but does **not** touch undo history — the draft only
    /// becomes undoable via [`commit_draft`] on pointer-up.
    pub fn start_draft(&mut self, kind: AnnotationKind, geometry: AnnotationGeometry) {
        let id = self.alloc_id();
        self.draft = Some(AnnotationItem {
            id,
            kind,
            geometry,
            style: self.default_style.clone(),
        });
        self.selected_id = None;
    }

    /// Replace the draft geometry in-place (called on every mouse move; no undo).
    pub fn update_draft(&mut self, geometry: AnnotationGeometry) {
        if let Some(draft) = self.draft.as_mut() {
            draft.geometry = geometry;
        }
    }

    /// Append a point to a freehand / highlight draft.
    pub fn push_draft_point(&mut self, point: Point) {
        if let Some(draft) = self.draft.as_mut() {
            match &mut draft.geometry {
                AnnotationGeometry::Freehand { points }
                | AnnotationGeometry::Highlight { points } => points.push(point),
                _ => {}
            }
        }
    }

    /// Set or replace the draft item (not yet in `items`, not yet undo-committed).
    pub fn set_draft(&mut self, item: AnnotationItem) {
        self.draft = Some(item);
    }

    /// Clear the draft without adding to `items`.
    pub fn clear_draft(&mut self) {
        self.draft = None;
    }

    /// Commit the draft into `items` and push undo.
    pub fn commit_draft(&mut self) -> Option<AnnotationId> {
        let item = self.draft.take()?;
        self.push_undo();
        let id = item.id;
        self.items.push(item);
        self.selected_id = Some(id);
        Some(id)
    }

    // ── Undo / Redo ───────────────────────────────────────────────────────────

    pub fn undo(&mut self) -> bool {
        let Some(snap) = self.undo.pop() else {
            return false;
        };
        self.redo.push(DocumentSnapshot {
            items: self.items.clone(),
            selected_id: self.selected_id,
        });
        self.items = snap.items;
        self.selected_id = snap.selected_id;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(snap) = self.redo.pop() else {
            return false;
        };
        self.undo.push(DocumentSnapshot {
            items: self.items.clone(),
            selected_id: self.selected_id,
        });
        self.items = snap.items;
        self.selected_id = snap.selected_id;
        true
    }

    // ── Reset ─────────────────────────────────────────────────────────────────

    /// Drop all items, history, selection, and draft.
    pub fn reset(&mut self) {
        self.items.clear();
        self.selected_id = None;
        self.undo.clear();
        self.redo.clear();
        self.draft = None;
    }

    // ── Internal ──────────────────────────────────────────────────────────────

    fn push_undo(&mut self) {
        self.undo.push(DocumentSnapshot {
            items: self.items.clone(),
            selected_id: self.selected_id,
        });
        self.redo.clear();
    }

    /// Allocate the next item id. Per-document rather than process-global so ids are
    /// deterministic and reproducible in tests, and so a future save/load or
    /// multi-document session cannot collide. Restoring an undo snapshot never rewinds
    /// the counter, so a fresh op can't reuse a still-referenced id.
    fn alloc_id(&mut self) -> AnnotationId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

// ─── Geometry helpers ─────────────────────────────────────────────────────────

fn translate_geometry(g: &mut AnnotationGeometry, dx: i32, dy: i32) {
    let delta = Point::new(dx, dy);
    match g {
        AnnotationGeometry::Rect { bounds } | AnnotationGeometry::Ellipse { bounds } => {
            *bounds = bounds.translate(delta);
        }
        AnnotationGeometry::Line { start, end }
        | AnnotationGeometry::Arrow { start, end } => {
            *start = Point::new(start.x + dx, start.y + dy);
            *end = Point::new(end.x + dx, end.y + dy);
        }
        AnnotationGeometry::Text { position, .. } => {
            *position = Point::new(position.x + dx, position.y + dy);
        }
        AnnotationGeometry::Freehand { points }
        | AnnotationGeometry::Highlight { points } => {
            for p in points.iter_mut() {
                *p = Point::new(p.x + dx, p.y + dy);
            }
        }
    }
}

/// Euclidean distance from `p` to the nearest point on segment `ab`.
fn dist_point_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let abx = (b.x - a.x) as f64;
    let aby = (b.y - a.y) as f64;
    let apx = (p.x - a.x) as f64;
    let apy = (p.y - a.y) as f64;
    let len2 = abx * abx + aby * aby;
    if len2 == 0.0 {
        return (apx * apx + apy * apy).sqrt();
    }
    let t = (apx * abx + apy * aby) / len2;
    let t = t.clamp(0.0, 1.0);
    let nx = a.x as f64 + t * abx;
    let ny = a.y as f64 + t * aby;
    let dx = p.x as f64 - nx;
    let dy = p.y as f64 - ny;
    (dx * dx + dy * dy).sqrt()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_item(x: i32, y: i32, w: i32, h: i32) -> AnnotationItem {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(1_000_000);
        AnnotationItem {
            id: NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed),
            kind: AnnotationKind::Rectangle,
            geometry: AnnotationGeometry::Rect {
                bounds: Rect::from_origin_size(Point::new(x, y), w, h),
            },
            style: AnnotationStyle::default(),
        }
    }

    #[test]
    fn new_document_is_empty() {
        let doc = AnnotationDocument::new();
        assert!(doc.is_empty());
        assert!(!doc.can_undo());
        assert!(!doc.can_redo());
    }

    #[test]
    fn create_item_pushes_undo_and_selects() {
        let mut doc = AnnotationDocument::new();
        let id = doc.create_item(
            AnnotationKind::Rectangle,
            AnnotationGeometry::Rect {
                bounds: Rect::from_origin_size(Point::new(10, 10), 100, 80),
            },
        );
        assert_eq!(doc.items().len(), 1);
        assert_eq!(doc.selected_id(), Some(id));
        assert!(doc.can_undo());
    }

    #[test]
    fn undo_restores_previous_state() {
        let mut doc = AnnotationDocument::new();
        let id = doc.create_item(
            AnnotationKind::Rectangle,
            AnnotationGeometry::Rect {
                bounds: Rect::from_origin_size(Point::new(0, 0), 50, 50),
            },
        );
        assert!(doc.undo());
        assert!(doc.items().is_empty());
        assert_eq!(doc.selected_id(), None);
        assert!(doc.can_redo());
        // redo brings it back
        assert!(doc.redo());
        assert_eq!(doc.items().len(), 1);
        assert_eq!(doc.items()[0].id, id);
    }

    #[test]
    fn delete_selected_pushes_undo() {
        let mut doc = AnnotationDocument::new();
        doc.create_item(
            AnnotationKind::Rectangle,
            AnnotationGeometry::Rect {
                bounds: Rect::from_origin_size(Point::new(0, 0), 10, 10),
            },
        );
        doc.delete_selected();
        assert!(doc.items().is_empty());
        assert!(doc.undo());
        assert_eq!(doc.items().len(), 1);
    }

    #[test]
    fn translate_does_not_push_undo() {
        let mut doc = AnnotationDocument::new();
        let id = doc.create_item(
            AnnotationKind::Rectangle,
            AnnotationGeometry::Rect {
                bounds: Rect::from_origin_size(Point::new(0, 0), 10, 10),
            },
        );
        let undo_len_before = doc.undo.len();
        doc.translate_item(id, 5, 5);
        assert_eq!(doc.undo.len(), undo_len_before);
    }

    #[test]
    fn commit_drag_records_one_undo_step() {
        let mut doc = AnnotationDocument::new();
        let id = doc.create_item(
            AnnotationKind::Rectangle,
            AnnotationGeometry::Rect {
                bounds: Rect::from_origin_size(Point::new(0, 0), 10, 10),
            },
        );
        // Take snapshot before drag
        let snap = doc.snapshot();
        // Multiple moves (no undo push)
        doc.translate_item(id, 3, 0);
        doc.translate_item(id, 3, 0);
        doc.translate_item(id, 4, 0);
        // Commit once
        doc.commit_drag(snap);
        // Undo should restore to before all moves
        assert!(doc.undo());
        let bounds = match &doc.items()[0].geometry {
            AnnotationGeometry::Rect { bounds } => *bounds,
            _ => unreachable!(),
        };
        assert_eq!(bounds.left, 0);
    }

    #[test]
    fn duplicate_selected_creates_new_item() {
        let mut doc = AnnotationDocument::new();
        doc.create_item(
            AnnotationKind::Rectangle,
            AnnotationGeometry::Rect {
                bounds: Rect::from_origin_size(Point::new(10, 10), 50, 50),
            },
        );
        doc.duplicate_selected();
        assert_eq!(doc.items().len(), 2);
    }

    #[test]
    fn hit_test_returns_top_most_item() {
        let mut doc = AnnotationDocument::new();
        let _id1 = doc.create_item(
            AnnotationKind::Rectangle,
            AnnotationGeometry::Rect {
                bounds: Rect::from_origin_size(Point::new(0, 0), 100, 100),
            },
        );
        let id2 = doc.create_item(
            AnnotationKind::Rectangle,
            AnnotationGeometry::Rect {
                bounds: Rect::from_origin_size(Point::new(50, 50), 100, 100),
            },
        );
        // Point (70,70) is in both; should return the topmost (id2)
        let hit = doc.hit_test(Point::new(70, 70));
        assert_eq!(hit, Some(id2));
    }

    #[test]
    fn reset_clears_all_state() {
        let mut doc = AnnotationDocument::new();
        doc.create_item(
            AnnotationKind::Rectangle,
            AnnotationGeometry::Rect {
                bounds: Rect::from_origin_size(Point::new(0, 0), 10, 10),
            },
        );
        doc.reset();
        assert!(doc.is_empty());
        assert!(!doc.can_undo());
        assert!(!doc.can_redo());
    }

    #[test]
    fn draft_commit_adds_item_and_pushes_undo() {
        let mut doc = AnnotationDocument::new();
        let item = rect_item(20, 20, 60, 40);
        let draft_id = item.id;
        doc.set_draft(item);
        assert!(doc.can_undo() == false); // draft itself is not undoable yet
        doc.commit_draft();
        assert_eq!(doc.items().len(), 1);
        assert_eq!(doc.selected_id(), Some(draft_id));
        assert!(doc.can_undo());
    }

    #[test]
    fn undo_after_create_is_empty_and_redo_restores() {
        let mut doc = AnnotationDocument::new();
        doc.create_item(
            AnnotationKind::Arrow,
            AnnotationGeometry::Arrow {
                start: Point::new(10, 10),
                end: Point::new(100, 100),
            },
        );
        doc.undo();
        assert!(doc.items().is_empty());
        doc.redo();
        assert_eq!(doc.items().len(), 1);
    }

    #[test]
    fn style_change_pushes_undo_once() {
        let mut doc = AnnotationDocument::new();
        let id = doc.create_item(
            AnnotationKind::Rectangle,
            AnnotationGeometry::Rect {
                bounds: Rect::from_origin_size(Point::new(0, 0), 50, 50),
            },
        );
        let new_style = AnnotationStyle {
            stroke_color: [1.0, 0.0, 0.0, 1.0],
            ..AnnotationStyle::default()
        };
        doc.set_style(id, new_style.clone());
        assert!(doc.undo());
        // After undo, original style restored
        assert_eq!(doc.items()[0].style.stroke_color, AnnotationStyle::default().stroke_color);
    }

    #[test]
    fn dist_to_horizontal_segment_correct() {
        assert!((dist_point_to_segment(
            Point::new(5, 3),
            Point::new(0, 0),
            Point::new(10, 0),
        ) - 3.0)
            .abs()
            < f64::EPSILON);
    }

    #[test]
    fn dist_to_degenerate_segment_is_point_distance() {
        let d = dist_point_to_segment(Point::new(3, 4), Point::new(0, 0), Point::new(0, 0));
        assert!((d - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn handle_at_finds_corner_and_resizes_bounds() {
        let mut doc = AnnotationDocument::new();
        let id = doc.create_item(
            AnnotationKind::Rectangle,
            AnnotationGeometry::Rect {
                bounds: Rect::new(100, 100, 200, 200),
            },
        );
        // Bottom-right corner is (200,200).
        let h = doc.handle_at(id, Point::new(202, 198), 6).unwrap();
        assert_eq!(h, AnnotationHandle::Corner(3));
        let snap = doc.snapshot();
        doc.resize_item(id, h, Point::new(300, 260));
        doc.commit_drag(snap);
        match &doc.items()[0].geometry {
            AnnotationGeometry::Rect { bounds } => {
                assert_eq!(*bounds, Rect::new(100, 100, 300, 260));
            }
            _ => unreachable!(),
        }
        // One undo step restores the pre-resize bounds.
        assert!(doc.undo());
        match &doc.items()[0].geometry {
            AnnotationGeometry::Rect { bounds } => assert_eq!(*bounds, Rect::new(100, 100, 200, 200)),
            _ => unreachable!(),
        }
    }

    #[test]
    fn resize_endpoint_moves_only_that_end() {
        let mut doc = AnnotationDocument::new();
        let id = doc.create_item(
            AnnotationKind::Arrow,
            AnnotationGeometry::Arrow {
                start: Point::new(0, 0),
                end: Point::new(50, 50),
            },
        );
        let h = doc.handle_at(id, Point::new(50, 50), 5).unwrap();
        assert_eq!(h, AnnotationHandle::Endpoint(1));
        doc.resize_item(id, h, Point::new(80, 20));
        match &doc.items()[0].geometry {
            AnnotationGeometry::Arrow { start, end } => {
                assert_eq!(*start, Point::new(0, 0));
                assert_eq!(*end, Point::new(80, 20));
            }
            _ => unreachable!(),
        }
    }

    /// IDs are per-document and start at 1, so they are deterministic and two
    /// documents never collide (regression guard for the old process-global counter).
    #[test]
    fn ids_are_deterministic_and_per_document() {
        let mut doc = AnnotationDocument::new();
        let a = doc.create_item(
            AnnotationKind::Rectangle,
            AnnotationGeometry::Rect {
                bounds: Rect::from_origin_size(Point::new(0, 0), 10, 10),
            },
        );
        let b = doc.create_item(
            AnnotationKind::Rectangle,
            AnnotationGeometry::Rect {
                bounds: Rect::from_origin_size(Point::new(0, 0), 10, 10),
            },
        );
        assert_eq!((a, b), (1, 2));
        // A fresh document restarts the sequence; ids are not process-global.
        let mut other = AnnotationDocument::new();
        let c = other.create_item(
            AnnotationKind::Rectangle,
            AnnotationGeometry::Rect {
                bounds: Rect::from_origin_size(Point::new(0, 0), 10, 10),
            },
        );
        assert_eq!(c, 1);
    }

    /// Draw order is the list order: `bring_to_front` moves to the end, `send_to_back`
    /// to the front, and `hit_test` returns the last matching item.
    #[test]
    fn z_order_is_the_list_position() {
        let mut doc = AnnotationDocument::new();
        let a = doc.create_item(
            AnnotationKind::Rectangle,
            AnnotationGeometry::Rect {
                bounds: Rect::from_origin_size(Point::new(0, 0), 100, 100),
            },
        );
        let _b = doc.create_item(
            AnnotationKind::Rectangle,
            AnnotationGeometry::Rect {
                bounds: Rect::from_origin_size(Point::new(0, 0), 10, 10),
            },
        );
        let order = |doc: &AnnotationDocument| -> Vec<AnnotationId> {
            doc.items().iter().map(|i| i.id).collect()
        };
        assert_eq!(order(&doc)[0], a, "a was created first, so it is at the bottom");

        doc.select(Some(a));
        doc.bring_to_front();
        assert_eq!(*order(&doc).last().unwrap(), a, "a must now be on top");

        doc.send_to_back();
        assert_eq!(order(&doc)[0], a, "a must be back at the bottom");
    }

    /// A style change hits only the selected object (one undo step); with nothing
    /// selected it edits the pending default and records no undo (docs/11 §8.2).
    #[test]
    fn apply_style_targets_selection_then_default() {
        let mut doc = AnnotationDocument::new();
        let a = doc.create_item(
            AnnotationKind::Rectangle,
            AnnotationGeometry::Rect {
                bounds: Rect::from_origin_size(Point::new(0, 0), 50, 50),
            },
        );
        doc.create_item(
            AnnotationKind::Rectangle,
            AnnotationGeometry::Rect {
                bounds: Rect::from_origin_size(Point::new(60, 60), 50, 50),
            },
        );
        let default_stroke = AnnotationStyle::default().stroke_color;
        let red = [1.0, 0.0, 0.0, 1.0];

        doc.select(Some(a));
        doc.apply_style(|s| s.stroke_color = red);
        assert_eq!(doc.items()[0].style.stroke_color, red, "selected item changed");
        assert_eq!(doc.items()[1].style.stroke_color, default_stroke, "other item untouched");
        assert!(doc.undo(), "the selection edit is one undo step");
        assert_eq!(doc.items()[0].style.stroke_color, default_stroke, "undo restored a");

        doc.select(None);
        let undo_before = doc.undo.len();
        doc.apply_style(|s| s.stroke_width = 9.0);
        assert_eq!(doc.default_style.stroke_width, 9.0, "no selection → default style");
        assert_eq!(doc.undo.len(), undo_before, "editing the default is not a document edit");
    }

    fn rect(x: i32, y: i32, w: i32, h: i32) -> AnnotationGeometry {
        AnnotationGeometry::Rect {
            bounds: Rect::from_origin_size(Point::new(x, y), w, h),
        }
    }

    /// `bring_forward` / `send_backward` move one layer at a time (matching the
    /// `AnnotationCommand::BringForward` / `SendBackward` names, not the absolute
    /// `bring_to_front` / `send_to_back`) and are no-ops at the stack ends.
    #[test]
    fn bring_forward_and_backward_nudge_one_step_and_noop_at_ends() {
        let mut doc = AnnotationDocument::new();
        let a = doc.create_item(AnnotationKind::Rectangle, rect(0, 0, 100, 100));
        let b = doc.create_item(AnnotationKind::Rectangle, rect(0, 0, 10, 10));
        let c = doc.create_item(AnnotationKind::Rectangle, rect(0, 0, 5, 5));
        let order = |doc: &AnnotationDocument| -> Vec<AnnotationId> {
            doc.items().iter().map(|i| i.id).collect()
        };
        assert_eq!(order(&doc), vec![a, b, c]);

        // `c` is already top-most: bring_forward changes nothing and records no undo.
        doc.select(Some(c));
        let undo_before = doc.undo.len();
        doc.bring_forward();
        assert_eq!(order(&doc), vec![a, b, c], "already top: unchanged");
        assert_eq!(doc.undo.len(), undo_before, "no-op records no undo");

        // `a` is already bottom-most: send_backward is a no-op too.
        doc.select(Some(a));
        doc.send_backward();
        assert_eq!(order(&doc), vec![a, b, c], "already bottom: unchanged");

        // One step up swaps `a` with `b`.
        doc.bring_forward();
        assert_eq!(order(&doc), vec![b, a, c]);

        // One step down swaps `c` with `a`.
        doc.select(Some(c));
        doc.send_backward();
        assert_eq!(order(&doc), vec![b, c, a]);
    }

    /// `execute` is the single routing point for toolbar commands; tool selection is
    /// deliberately ignored because it is controller, not document, state.
    #[test]
    fn execute_routes_commands_to_document_operations() {
        let mut doc = AnnotationDocument::new();
        let a = doc.create_item(AnnotationKind::Rectangle, rect(0, 0, 50, 50));
        doc.create_item(AnnotationKind::Rectangle, rect(60, 60, 50, 50));
        doc.select(Some(a));

        doc.execute(AnnotationCommand::SetStrokeColor([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(doc.items()[0].style.stroke_color, [1.0, 0.0, 0.0, 1.0]);
        doc.execute(AnnotationCommand::SetFillColor(Some([0.0, 1.0, 0.0, 1.0])));
        assert_eq!(doc.items()[0].style.fill_color, Some([0.0, 1.0, 0.0, 1.0]));
        doc.execute(AnnotationCommand::SetStrokeWidth(7.0));
        assert_eq!(doc.items()[0].style.stroke_width, 7.0);

        doc.execute(AnnotationCommand::Duplicate);
        assert_eq!(doc.items().len(), 3);
        doc.execute(AnnotationCommand::Undo);
        assert_eq!(doc.items().len(), 2, "undo reverts the duplicate");
        doc.execute(AnnotationCommand::Delete);
        assert_eq!(doc.items().len(), 1, "delete removes the selection");

        let undo_len = doc.undo.len();
        doc.execute(AnnotationCommand::Tool(AnnotationKind::Arrow));
        doc.execute(AnnotationCommand::SelectTool);
        assert_eq!(doc.undo.len(), undo_len, "tool selection is not a document edit");
    }

    /// The annotation data leaves are serializable so the document can be handed to
    /// the toolbar or persisted without a bespoke codec. History is intentionally not
    /// part of this surface (a saved document starts with a clean undo stack).
    #[test]
    fn item_round_trips_through_serde_json() {
        let item = AnnotationItem {
            id: 42,
            kind: AnnotationKind::Arrow,
            geometry: AnnotationGeometry::Arrow {
                start: Point::new(1, 2),
                end: Point::new(30, 40),
            },
            style: AnnotationStyle {
                stroke_color: [1.0, 0.0, 0.0, 1.0],
                fill_color: Some([0.0, 1.0, 0.0, 0.5]),
                stroke_width: 4.0,
                opacity: 0.8,
                font_size: 20.0,
                arrow_head_size: 12.0,
            },
        };
        let json = serde_json::to_string(&item).unwrap();
        let back: AnnotationItem = serde_json::from_str(&json).unwrap();
        assert_eq!(item, back);
    }
}
