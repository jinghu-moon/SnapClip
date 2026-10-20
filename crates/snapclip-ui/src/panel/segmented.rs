use snapclip_model::geometry::{Point, Rect};

use super::interaction::PointerEvent;
use super::motion::SEGMENT_SLIDE;

pub const SEGMENT_PAD_DIP: f32 = 8.0;
pub const SEGMENT_RADIUS_DIP: f32 = 7.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub rect: Rect,
    pub selected: bool,
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SegmentState {
    pub track: Rect,
    pub segments: Vec<Segment>,
    pub hovered: Option<usize>,
    pub pressed: Option<usize>,
    pub focused: bool,
    pub slide: Option<(usize, f32)>,
}

impl SegmentState {
    pub fn target_at(&self, point: Point) -> Option<usize> {
        self.segments
            .iter()
            .position(|segment| segment.rect.contains(point))
    }

    pub fn pointer_event(&mut self, event: PointerEvent, point: Point) -> Option<usize> {
        match event {
            PointerEvent::Enter => self.hovered = self.target_at(point),
            PointerEvent::Leave => self.hovered = None,
            PointerEvent::Down => {
                let Some(index) = self.target_at(point) else {
                    return None;
                };
                if !self.segments[index].disabled {
                    self.pressed = Some(index);
                    self.hovered = Some(index);
                    self.focused = true;
                }
            }
            PointerEvent::Up => {
                let target = self.target_at(point);
                let activated = self
                    .pressed
                    .filter(|index| target == Some(*index) && !self.segments[*index].disabled);
                self.pressed = None;
                self.hovered = target;
                if let Some(index) = activated {
                    for (candidate, segment) in self.segments.iter_mut().enumerate() {
                        segment.selected = candidate == index;
                    }
                    return Some(index);
                }
            }
            PointerEvent::Cancel => self.pressed = None,
        }
        None
    }
}

pub const fn segment_radius(outer: i32, inset: i32) -> i32 {
    if outer > inset { outer - inset } else { 0 }
}

pub fn segment_outer_radius(scale: f32) -> i32 {
    (SEGMENT_RADIUS_DIP * scale.max(1.0)).round() as i32
}

pub fn segment_slide(from: Rect, to: Rect, t: f32) -> Rect {
    let eased = SEGMENT_SLIDE.progress(SEGMENT_SLIDE.duration.mul_f32(t.clamp(0.0, 1.0)), false);
    let lerp = |a: i32, b: i32| (a as f32 + (b - a) as f32 * eased).round() as i32;
    Rect::new(
        lerp(from.left, to.left),
        lerp(from.top, to.top),
        lerp(from.right, to.right),
        lerp(from.bottom, to.bottom),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_testing_uses_the_segment_rectangles() {
        let state = SegmentState {
            track: Rect::new(0, 0, 100, 24),
            segments: vec![
                Segment {
                    rect: Rect::new(4, 4, 48, 20),
                    selected: true,
                    disabled: false,
                },
                Segment {
                    rect: Rect::new(52, 4, 96, 20),
                    selected: false,
                    disabled: false,
                },
            ],
            hovered: None,
            pressed: None,
            focused: false,
            slide: None,
        };

        assert_eq!(state.target_at(Point::new(4, 4)), Some(0));
        assert_eq!(state.target_at(Point::new(48, 10)), None);
        assert_eq!(state.target_at(Point::new(52, 4)), Some(1));
    }

    #[test]
    fn slide_clamps_and_lands_on_both_endpoints() {
        let from = Rect::new(0, 0, 10, 10);
        let to = Rect::new(20, 4, 30, 14);
        assert_eq!(segment_slide(from, to, 0.0), from);
        assert_eq!(segment_slide(from, to, 1.0), to);
        assert_eq!(segment_slide(from, to, -1.0), from);
        assert_eq!(segment_slide(from, to, 2.0), to);
    }

    #[test]
    fn nested_radius_never_becomes_negative() {
        assert_eq!(segment_radius(7, 3), 4);
        assert_eq!(segment_radius(7, 9), 0);
    }

    #[test]
    fn pointer_release_selects_one_segment_and_respects_disabled_segments() {
        let mut state = SegmentState {
            track: Rect::new(0, 0, 100, 24),
            segments: vec![
                Segment {
                    rect: Rect::new(4, 4, 48, 20),
                    selected: true,
                    disabled: false,
                },
                Segment {
                    rect: Rect::new(52, 4, 96, 20),
                    selected: false,
                    disabled: true,
                },
            ],
            hovered: None,
            pressed: None,
            focused: false,
            slide: None,
        };
        state.pointer_event(PointerEvent::Down, Point::new(60, 10));
        assert_eq!(
            state.pointer_event(PointerEvent::Up, Point::new(60, 10)),
            None
        );
        assert!(state.segments[0].selected);
        state.pointer_event(PointerEvent::Down, Point::new(10, 10));
        assert_eq!(
            state.pointer_event(PointerEvent::Up, Point::new(10, 10)),
            Some(0)
        );
        assert_eq!(state.pressed, None);
    }
}
