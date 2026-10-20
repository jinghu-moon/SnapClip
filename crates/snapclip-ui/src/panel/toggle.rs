use snapclip_model::geometry::{Point, Rect};

use super::interaction::{InteractionState, PointerEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ToggleState {
    pub checked: bool,
    pub interaction: InteractionState,
}

pub type SwitchState = ToggleState;

/// A binary switch with a movable thumb. Use [`super::Tag`] for selectable text pills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Toggle {
    pub rect: Rect,
}

impl Toggle {
    pub const fn new(rect: Rect) -> Self {
        Self { rect }
    }

    pub fn hit_test(self, point: Point) -> bool {
        self.rect.contains(point)
    }

    pub fn pointer_event(self, state: &mut ToggleState, event: PointerEvent, point: Point) -> bool {
        if state.interaction.disabled {
            state.interaction.pressed = false;
            return false;
        }
        match event {
            PointerEvent::Enter | PointerEvent::Leave | PointerEvent::Cancel => {
                state.interaction.apply(event);
                false
            }
            PointerEvent::Down if self.hit_test(point) => {
                state.interaction.apply(event);
                false
            }
            PointerEvent::Down => false,
            PointerEvent::Up => {
                let activated = state.interaction.pressed && self.hit_test(point);
                state.interaction.hovered = self.hit_test(point);
                state.interaction.apply(event);
                if activated {
                    state.checked = !state.checked;
                }
                activated
            }
        }
    }

    pub fn radius(self) -> i32 {
        (self.rect.height() / 2).max(0)
    }

    pub fn thumb_rect(self, checked: bool, inset: i32) -> Rect {
        let diameter = (self.rect.height() - inset * 2).max(1);
        let left = if checked {
            self.rect.right - inset - diameter
        } else {
            self.rect.left + inset
        };
        Rect::from_origin_size(Point::new(left, self.rect.top + inset), diameter, diameter)
    }

    pub fn thumb_rect_at(self, progress: f32, inset: i32, pressed: bool) -> Rect {
        let diameter = if pressed {
            self.rect.height().max(1)
        } else {
            (self.rect.height() - inset * 2).max(1)
        };
        let from = self.rect.left + inset;
        let to = self.rect.right - inset - diameter;
        let left = (from as f32 + (to - from) as f32 * progress.clamp(0.0, 1.0)).round() as i32;
        Rect::from_origin_size(
            Point::new(left, self.rect.top + (self.rect.height() - diameter) / 2),
            diameter,
            diameter,
        )
    }
}

pub type Switch = Toggle;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_moves_between_the_two_stable_endpoints() {
        let toggle = Toggle::new(Rect::new(0, 0, 46, 24));
        assert_eq!(toggle.radius(), 12);
        assert_eq!(toggle.thumb_rect(false, 2), Rect::new(2, 2, 22, 22));
        assert_eq!(toggle.thumb_rect(true, 2), Rect::new(24, 2, 44, 22));
    }

    #[test]
    fn release_inside_toggles_and_release_outside_cancels() {
        let toggle = Toggle::new(Rect::new(0, 0, 46, 24));
        let mut state = ToggleState::default();
        toggle.pointer_event(&mut state, PointerEvent::Down, Point::new(8, 12));
        assert!(toggle.pointer_event(&mut state, PointerEvent::Up, Point::new(8, 12)));
        assert!(state.checked);
        toggle.pointer_event(&mut state, PointerEvent::Down, Point::new(38, 12));
        assert!(!toggle.pointer_event(&mut state, PointerEvent::Up, Point::new(60, 12)));
        assert!(state.checked);
    }

    #[test]
    fn animated_thumb_has_monotonic_position_and_pressed_size() {
        let toggle = Toggle::new(Rect::new(0, 0, 46, 24));
        let start = toggle.thumb_rect_at(0.0, 2, false);
        let middle = toggle.thumb_rect_at(0.5, 2, false);
        let end = toggle.thumb_rect_at(1.0, 2, false);
        assert!(start.left < middle.left && middle.left < end.left);
        assert!(toggle.thumb_rect_at(0.5, 2, true).height() > middle.height());
    }
}

#[cfg(windows)]
pub mod d2d {
    use super::{Toggle, ToggleState};
    use crate::panel::d2d::PanelPainter;
    use windows::Win32::Graphics::Direct2D::ID2D1SolidColorBrush;

    pub struct ToggleColors<'a> {
        pub off_track: &'a ID2D1SolidColorBrush,
        pub off_hover: &'a ID2D1SolidColorBrush,
        pub on_track: &'a ID2D1SolidColorBrush,
        pub off_line: &'a ID2D1SolidColorBrush,
        pub on_line: &'a ID2D1SolidColorBrush,
        pub thumb: &'a ID2D1SolidColorBrush,
        pub focus: &'a ID2D1SolidColorBrush,
        pub disabled: &'a ID2D1SolidColorBrush,
    }

    impl Toggle {
        pub fn paint(
            self,
            painter: &PanelPainter<'_>,
            state: ToggleState,
            colors: ToggleColors<'_>,
            inset: i32,
            progress: f32,
        ) {
            let progress = progress.clamp(0.0, 1.0);
            let track = if state.interaction.disabled {
                colors.disabled
            } else if state.checked {
                colors.on_track
            } else if state.interaction.hovered {
                colors.off_hover
            } else {
                colors.off_track
            };
            painter.fill_round(self.rect, self.radius(), track);
            let line = if state.interaction.disabled {
                colors.disabled
            } else if state.checked {
                colors.on_line
            } else {
                colors.off_line
            };
            painter.stroke_round(self.rect, self.radius(), line, 1.0);
            let thumb = self.thumb_rect_at(progress, inset, state.interaction.pressed);
            painter.fill_circle(
                (thumb.center().x as f32, thumb.center().y as f32),
                thumb.width().max(1) as f32 / 2.0,
                colors.thumb,
            );
            if state.interaction.focused && !state.interaction.disabled {
                painter.stroke_round(self.rect, self.radius(), colors.focus, 2.0);
            }
        }
    }
}
