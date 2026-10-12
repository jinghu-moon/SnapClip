//! The shortcut guide (docs/design/overlay-ui-demo.html): the bottom-left panel that lists what the
//! keys do, built from the keymap, grouped, and laid into columns that never split a group.
//!
//! The layout is a pure function of the groups, the scale and the frame height, measured through a
//! closure so it is testable without a device; the renderer caches it, since the groups only change
//! between sessions.

use super::*;
use crate::keymap::GuideGroup;

/// The guide as the overlay wants it drawn this frame.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GuideView {
    pub groups: Arc<[GuideGroup]>,
    /// The fade, `0.0..=1.0`.
    pub alpha: f32,
}

// Layout, in DIP (× k = dpi / 96 when laid out).
const PAD: f32 = 14.0;
const ROW_H: f32 = 30.0;
const CAP_H: f32 = 22.0;
const HEAD_H: f32 = 22.0;
/// The group name is centred this far below the group's top.
const HEAD_MID: f32 = 8.0;
const COL_GAP: f32 = 22.0;
/// Between the key column and the label column.
const GAP_L: f32 = 14.0;
/// The `/` between alternatives and the `+` between keys.
const SEP_W: f32 = 16.0;
const MARGIN: f32 = 20.0;
const RADIUS: f32 = 12.0;
const CAP_RADIUS: f32 = 5.0;
/// A cap is its text plus this, and never narrower than it is tall.
const CAP_TEXT_PAD: f32 = 14.0;
/// A column is at most this share of the screen's height (but never under `MIN_COLUMN_H`).
const COLUMN_SHARE: f32 = 0.55;
const MIN_COLUMN_H: f32 = 160.0;
const CAP_FONT: f32 = 12.0;
const LABEL_FONT: f32 = 13.0;
const HEAD_FONT: f32 = 11.0;
const SEP_FONT: f32 = 12.0;

/// How close the pointer may come to the guide, in DIP, before it steps aside.
pub(crate) const GUIDE_AVOID_DIP: f32 = 48.0;

/// What a mark is, which decides its font and its paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MarkKind {
    Header,
    Cap,
    Sep,
    Label,
}

impl MarkKind {
    fn font(self) -> (f32, DWRITE_FONT_WEIGHT, bool) {
        match self {
            Self::Header => (HEAD_FONT, DWRITE_FONT_WEIGHT_NORMAL, false),
            Self::Cap => (CAP_FONT, DWRITE_FONT_WEIGHT_SEMI_BOLD, true),
            Self::Sep => (SEP_FONT, DWRITE_FONT_WEIGHT_NORMAL, true),
            Self::Label => (LABEL_FONT, DWRITE_FONT_WEIGHT_NORMAL, false),
        }
    }
}

/// One thing to draw, in physical pixels from the panel's top-left. The text is vertically centred
/// in `rect`; caps and separators are centred across it too.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct GuideMark {
    pub kind: MarkKind,
    pub rect: (f32, f32, f32, f32),
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct GuideLayout {
    pub width: f32,
    pub height: f32,
    pub marks: Vec<GuideMark>,
}

/// A layout and what it was laid out for.
pub(super) struct CachedGuide {
    groups: Arc<[GuideGroup]>,
    dpi: u32,
    frame_height: i32,
    layout: Option<GuideLayout>,
}

/// Lay the guide out at scale `k` for a screen `frame_height` physical pixels tall; `None` when
/// there is nothing to show. `measure(text, kind)` is the text's width in physical pixels.
pub(super) fn layout_guide(
    groups: &[GuideGroup],
    k: f32,
    frame_height: f32,
    mut measure: impl FnMut(&str, MarkKind) -> Result<f32, String>,
) -> Result<Option<GuideLayout>, String> {
    if groups.iter().all(|group| group.rows.is_empty()) {
        return Ok(None);
    }
    let group_h = |group: &GuideGroup| (HEAD_H + group.rows.len() as f32 * ROW_H) * k;
    let max_h = ((frame_height / k * COLUMN_SHARE).max(MIN_COLUMN_H) - 2.0 * PAD) * k;
    let mut columns: Vec<Vec<&GuideGroup>> = vec![Vec::new()];
    let mut filled = 0.0;
    for group in groups.iter().filter(|group| !group.rows.is_empty()) {
        let height = group_h(group);
        if filled > 0.0 && filled + height > max_h {
            columns.push(Vec::new());
            filled = 0.0;
        }
        columns.last_mut().expect("never empty").push(group);
        filled += height;
    }

    let mut marks = Vec::new();
    let mut x = PAD * k;
    let mut height: f32 = 0.0;
    for column in &columns {
        let mut rows = Vec::new();
        for group in column {
            for row in &group.rows {
                let mut caps = Vec::new();
                for chord in &row.chords {
                    caps.push(
                        chord
                            .iter()
                            .map(|token| {
                                Ok((measure(token, MarkKind::Cap)? + CAP_TEXT_PAD * k)
                                    .max(CAP_H * k))
                            })
                            .collect::<Result<Vec<f32>, String>>()?,
                    );
                }
                let keys_w = caps
                    .iter()
                    .map(|chord| chord.iter().sum::<f32>() + sep_count(chord.len()) * SEP_W * k)
                    .sum::<f32>()
                    + sep_count(caps.len()) * SEP_W * k;
                rows.push((caps, keys_w, measure(row.label, MarkKind::Label)?));
            }
        }
        let kcol = rows.iter().map(|row| row.1).fold(0.0, f32::max);
        let lcol = rows.iter().map(|row| row.2).fold(0.0, f32::max);

        let mut rows = rows.into_iter();
        let mut y = PAD * k;
        for group in column {
            marks.push(GuideMark {
                kind: MarkKind::Header,
                rect: (x, y, x + kcol + GAP_L * k + lcol, y + 2.0 * HEAD_MID * k),
                text: group.name.to_string(),
            });
            y += HEAD_H * k;
            for row in &group.rows {
                let (caps, keys_w, label_w) = rows.next().expect("one measure per row");
                let (top, bottom) = (y, y + CAP_H * k);
                // The key column is right-aligned, so labels start on one line.
                let mut kx = x + kcol - keys_w;
                let sep = |kx: &mut f32, text: &str, marks: &mut Vec<GuideMark>| {
                    marks.push(GuideMark {
                        kind: MarkKind::Sep,
                        rect: (*kx, top, *kx + SEP_W * k, bottom),
                        text: text.to_string(),
                    });
                    *kx += SEP_W * k;
                };
                for (i, (chord, widths)) in row.chords.iter().zip(&caps).enumerate() {
                    if i > 0 {
                        sep(&mut kx, "/", &mut marks);
                    }
                    for (j, (token, width)) in chord.iter().zip(widths).enumerate() {
                        if j > 0 {
                            sep(&mut kx, "+", &mut marks);
                        }
                        marks.push(GuideMark {
                            kind: MarkKind::Cap,
                            rect: (kx, top, kx + width, bottom),
                            text: token.clone(),
                        });
                        kx += width;
                    }
                }
                let lx = x + kcol + GAP_L * k;
                marks.push(GuideMark {
                    kind: MarkKind::Label,
                    rect: (lx, top, lx + label_w + 2.0, bottom),
                    text: row.label.to_string(),
                });
                y += ROW_H * k;
            }
        }
        height = height.max(y - PAD * k);
        x += kcol + GAP_L * k + lcol + COL_GAP * k;
    }
    Ok(Some(GuideLayout {
        width: x - COL_GAP * k + PAD * k,
        // The last row's spacing below its caps is not part of the panel.
        height: height + 2.0 * PAD * k - (ROW_H - CAP_H) * k,
        marks,
    }))
}

fn sep_count(items: usize) -> f32 {
    items.saturating_sub(1) as f32
}

/// Where a `width × height` guide sits on `frame`: bottom-left, `MARGIN` in.
pub(super) fn guide_rect(frame: Rect, k: f32, width: f32, height: f32) -> Rect {
    let margin = MARGIN * k;
    let left = frame.left + margin.round() as i32;
    let bottom = frame.bottom - margin.round() as i32;
    Rect::new(
        left,
        bottom - height.ceil() as i32,
        left + width.ceil() as i32,
        bottom,
    )
}

impl OverlayRenderer {
    /// Where the guide for `groups` sits on `frame`, laying it out if it is not cached; `None` when
    /// it has nothing to show. The overlay asks this to keep the guide out of the pointer's way.
    pub(crate) fn guide_bounds(
        &mut self,
        groups: &Arc<[GuideGroup]>,
        frame: Rect,
    ) -> Result<Option<Rect>, String> {
        let dpi = self.metrics.dpi;
        let fresh = self.guide_layout.as_ref().is_some_and(|cached| {
            cached.dpi == dpi && cached.frame_height == frame.height() && cached.groups == *groups
        });
        if !fresh {
            let k = dpi.max(96) as f32 / 96.0;
            let layout = layout_guide(groups, k, frame.height() as f32, |text, kind| {
                let (size, weight, _) = kind.font();
                let format = self.info_text_format_mut(false, size, weight, false)?;
                self.measure_text_width_in(text, &format)
            })?;
            self.guide_layout = Some(CachedGuide {
                groups: groups.clone(),
                dpi,
                frame_height: frame.height(),
                layout,
            });
        }
        let k = dpi.max(96) as f32 / 96.0;
        Ok(self
            .guide_layout
            .as_ref()
            .and_then(|cached| cached.layout.as_ref())
            .map(|layout| guide_rect(frame, k, layout.width, layout.height)))
    }

    /// The guide: the blurred frame cropped to a rounded panel, a tint, then the marks — all in one
    /// layer whose opacity is the fade.
    pub(super) fn draw_guide(&mut self, guide: &GuideView, frame: Rect) -> Result<(), String> {
        let alpha = guide.alpha.clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return Ok(());
        }
        let Some(bounds) = self.guide_bounds(&guide.groups, frame)? else {
            return Ok(());
        };
        let Some(cached) = self.guide_layout.take() else {
            return Ok(());
        };
        let result = self.paint_guide(&cached, bounds, alpha);
        self.guide_layout = Some(cached);
        result
    }

    fn paint_guide(
        &mut self,
        cached: &CachedGuide,
        bounds: Rect,
        alpha: f32,
    ) -> Result<(), String> {
        let Some(layout) = cached.layout.as_ref() else {
            return Ok(());
        };
        let k = self.metrics.dpi.max(96) as f32 / 96.0;
        let tint = self.require_brush(&self.guide_tint_brush, "guide tint")?;
        let cap_fill = self.require_brush(&self.guide_cap_brush, "guide cap")?;
        let cap_text = self.require_brush(&self.guide_cap_text_brush, "guide cap text")?;
        let label = self.require_brush(&self.guide_label_brush, "guide label")?;
        let head = self.require_brush(&self.guide_head_brush, "guide head")?;
        let sep = self.require_brush(&self.guide_sep_brush, "guide separator")?;
        let mut formats = Vec::new();
        for kind in [
            MarkKind::Header,
            MarkKind::Cap,
            MarkKind::Sep,
            MarkKind::Label,
        ] {
            let (size, weight, centred) = kind.font();
            formats.push((
                kind,
                self.info_text_format_mut(false, size, weight, centred)?,
            ));
        }

        let panel = D2D1_ROUNDED_RECT {
            rect: to_d2d(bounds),
            radiusX: RADIUS * k,
            radiusY: RADIUS * k,
        };
        let factory = unsafe { self.d2d.GetFactory() }
            .map_err(|e| super::super::hresult("ID2D1DeviceContext::GetFactory", &e))?;
        let mask: ID2D1Geometry = unsafe { factory.CreateRoundedRectangleGeometry(&panel) }
            .map_err(|e| super::super::hresult("CreateRoundedRectangleGeometry", &e))?
            .cast()
            .map_err(|e| super::super::hresult("cast to ID2D1Geometry", &e))?;
        let layer = unsafe { self.d2d.CreateLayer(None) }
            .map_err(|e| super::super::hresult("CreateLayer", &e))?;
        let parameters = D2D1_LAYER_PARAMETERS1 {
            contentBounds: to_d2d(bounds),
            geometricMask: std::mem::ManuallyDrop::new(Some(mask)),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: windows_numerics::Matrix3x2::identity(),
            opacity: alpha,
            opacityBrush: std::mem::ManuallyDrop::new(None),
            layerOptions: Default::default(),
        };
        let (ox, oy) = (bounds.left as f32, bounds.top as f32);
        unsafe {
            self.d2d.PushLayer(&parameters, &layer);
            if let Some(blurred) = self.blurred_frame.as_ref() {
                let rect = to_d2d(bounds);
                self.d2d.DrawBitmap(
                    blurred,
                    Some(&rect),
                    1.0,
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    Some(&rect),
                    None,
                );
            }
            self.d2d.FillRectangle(&to_d2d(bounds), &tint);
            for mark in &layout.marks {
                let (l, t, r, b) = mark.rect;
                let rect = D2D_RECT_F {
                    left: ox + l,
                    top: oy + t,
                    right: ox + r,
                    bottom: oy + b,
                };
                let brush = match mark.kind {
                    MarkKind::Header => &head,
                    MarkKind::Cap => {
                        self.d2d.FillRoundedRectangle(
                            &D2D1_ROUNDED_RECT {
                                rect,
                                radiusX: CAP_RADIUS * k,
                                radiusY: CAP_RADIUS * k,
                            },
                            &cap_fill,
                        );
                        &cap_text
                    }
                    MarkKind::Sep => &sep,
                    MarkKind::Label => &label,
                };
                let format = &formats
                    .iter()
                    .find(|(kind, _)| *kind == mark.kind)
                    .expect("a format per kind")
                    .1;
                let wide = mark.text.encode_utf16().collect::<Vec<u16>>();
                self.d2d.DrawText(
                    &wide,
                    format,
                    &rect,
                    brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
            self.d2d.PopLayer();
        }
        let _ = std::mem::ManuallyDrop::into_inner(parameters.geometricMask);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{Context, Keymap};

    /// A monospace stand-in for DirectWrite: every character is half the font size wide.
    fn measure(text: &str, kind: MarkKind) -> Result<f32, String> {
        Ok(text.chars().count() as f32 * kind.font().0 * 0.5)
    }

    fn default_guide() -> Vec<GuideGroup> {
        Keymap::default().guide(Context::Pick, false)
    }

    #[test]
    fn the_default_guide_is_one_column_on_a_1080p_screen() {
        let layout = layout_guide(&default_guide(), 1.0, 1080.0, measure)
            .unwrap()
            .unwrap();
        let headers: Vec<_> = layout
            .marks
            .iter()
            .filter(|mark| mark.kind == MarkKind::Header)
            .map(|mark| mark.rect.0)
            .collect();
        assert!(headers.len() >= 3, "{headers:?}");
        assert!(headers.iter().all(|x| *x == PAD), "one column: {headers:?}");
        let rows = default_guide()
            .iter()
            .map(|group| group.rows.len())
            .sum::<usize>() as f32;
        let groups = headers.len() as f32;
        assert_eq!(
            layout.height,
            groups * HEAD_H + rows * ROW_H + 2.0 * PAD - (ROW_H - CAP_H)
        );
    }

    #[test]
    fn a_short_screen_splits_columns_between_groups_never_inside_one() {
        let groups = default_guide();
        let layout = layout_guide(&groups, 1.0, 300.0, measure).unwrap().unwrap();
        let header_x: Vec<f32> = layout
            .marks
            .iter()
            .filter(|mark| mark.kind == MarkKind::Header)
            .map(|mark| mark.rect.0)
            .collect();
        assert!(header_x.iter().any(|x| *x > PAD), "more than one column");
        // Every label sits in its own group's column.
        let mut column = 0.0;
        for mark in &layout.marks {
            match mark.kind {
                MarkKind::Header => column = mark.rect.0,
                MarkKind::Label => assert!(mark.rect.0 > column, "{mark:?}"),
                _ => {}
            }
        }
        let column_h = (300.0 * COLUMN_SHARE).max(MIN_COLUMN_H) - 2.0 * PAD;
        for group in &groups {
            assert!(HEAD_H + group.rows.len() as f32 * ROW_H <= column_h.max(HEAD_H + 4.0 * ROW_H));
        }
    }

    #[test]
    fn keys_are_right_aligned_and_labels_share_one_edge() {
        let layout = layout_guide(&default_guide(), 1.5, 1440.0, measure)
            .unwrap()
            .unwrap();
        let labels: Vec<f32> = layout
            .marks
            .iter()
            .filter(|mark| mark.kind == MarkKind::Label)
            .map(|mark| mark.rect.0)
            .collect();
        assert!(
            labels.windows(2).all(|pair| pair[0] == pair[1]),
            "{labels:?}"
        );
        // The last key of every row ends GAP_L before the labels.
        let mut last_key_end = None;
        for mark in &layout.marks {
            match mark.kind {
                MarkKind::Cap => last_key_end = Some(mark.rect.2),
                MarkKind::Label => {
                    let end = last_key_end.take().expect("a row has keys");
                    assert!((mark.rect.0 - end - GAP_L * 1.5).abs() < 0.01, "{mark:?}");
                }
                _ => {}
            }
        }
    }

    #[test]
    fn alternatives_get_a_slash_and_held_keys_a_plus() {
        let layout = layout_guide(&default_guide(), 1.0, 1080.0, measure)
            .unwrap()
            .unwrap();
        let seps: Vec<&str> = layout
            .marks
            .iter()
            .filter(|mark| mark.kind == MarkKind::Sep)
            .map(|mark| mark.text.as_str())
            .collect();
        assert!(seps.contains(&"+"), "Z+滚轮: {seps:?}");
        assert!(seps.contains(&"/"), "↑ / ↓: {seps:?}");
        let caps_are_tall = layout
            .marks
            .iter()
            .filter(|mark| mark.kind == MarkKind::Cap)
            .all(|mark| mark.rect.2 - mark.rect.0 >= CAP_H && mark.rect.3 - mark.rect.1 == CAP_H);
        assert!(caps_are_tall);
    }

    #[test]
    fn an_empty_guide_has_no_layout() {
        assert_eq!(layout_guide(&[], 1.0, 1080.0, measure).unwrap(), None);
    }

    #[test]
    fn the_guide_sits_bottom_left_with_a_margin() {
        let frame = Rect::new(0, 0, 2560, 1440);
        assert_eq!(
            guide_rect(frame, 1.5, 300.4, 200.2),
            Rect::new(30, 1440 - 30 - 201, 30 + 301, 1440 - 30)
        );
    }
}
