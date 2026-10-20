use snapclip_model::geometry::{Point, Rect};

use super::interaction::PointerEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadioOption {
    pub rect: Rect,
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RadioGroup {
    pub options: Vec<RadioOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RadioGroupState {
    pub selected: Option<usize>,
    pub hovered: Option<usize>,
    pub focused: Option<usize>,
    pub pressed: Option<usize>,
}

impl RadioGroup {
    pub fn target_at(&self, point: Point) -> Option<usize> {
        self.options
            .iter()
            .position(|option| option.rect.contains(point))
    }

    pub fn pointer_event(
        &self,
        state: &mut RadioGroupState,
        event: PointerEvent,
        point: Point,
    ) -> Option<usize> {
        match event {
            PointerEvent::Enter => state.hovered = self.target_at(point),
            PointerEvent::Leave => state.hovered = None,
            PointerEvent::Down => {
                let Some(index) = self.target_at(point) else {
                    return None;
                };
                if !self.options[index].disabled {
                    state.pressed = Some(index);
                    state.focused = Some(index);
                    state.hovered = Some(index);
                }
            }
            PointerEvent::Up => {
                let target = self.target_at(point);
                let activated = state
                    .pressed
                    .filter(|index| target == Some(*index) && !self.options[*index].disabled);
                state.pressed = None;
                state.hovered = target;
                if let Some(index) = activated {
                    state.selected = Some(index);
                    return Some(index);
                }
            }
            PointerEvent::Cancel => state.pressed = None,
        }
        None
    }

    pub fn set_focused(&self, state: &mut RadioGroupState, index: Option<usize>) {
        state.focused = index.filter(|index| {
            self.options
                .get(*index)
                .is_some_and(|option| !option.disabled)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group() -> RadioGroup {
        RadioGroup {
            options: vec![
                RadioOption {
                    rect: Rect::new(0, 0, 20, 20),
                    disabled: false,
                },
                RadioOption {
                    rect: Rect::new(24, 0, 44, 20),
                    disabled: false,
                },
            ],
        }
    }

    #[test]
    fn radio_hit_testing_returns_only_the_option_rect() {
        let group = group();
        assert_eq!(group.target_at(Point::new(5, 5)), Some(0));
        assert_eq!(group.target_at(Point::new(30, 5)), Some(1));
        assert_eq!(group.target_at(Point::new(20, 5)), None);
    }

    #[test]
    fn release_inside_selects_the_pressed_option() {
        let group = group();
        let mut state = RadioGroupState {
            selected: Some(1),
            ..Default::default()
        };
        group.pointer_event(&mut state, PointerEvent::Down, Point::new(5, 5));
        assert_eq!(
            group.pointer_event(&mut state, PointerEvent::Up, Point::new(5, 5)),
            Some(0)
        );
        assert_eq!(state.selected, Some(0));
        assert_eq!(state.pressed, None);
    }

    #[test]
    fn release_outside_does_not_change_radio_selection() {
        let group = group();
        let mut state = RadioGroupState::default();
        group.pointer_event(&mut state, PointerEvent::Down, Point::new(5, 5));
        assert_eq!(
            group.pointer_event(&mut state, PointerEvent::Up, Point::new(60, 5)),
            None
        );
        assert_eq!(state.selected, None);
        assert_eq!(state.pressed, None);
    }

    #[test]
    fn disabled_radio_cannot_be_selected_or_focused() {
        let group = RadioGroup {
            options: vec![RadioOption {
                rect: Rect::new(0, 0, 20, 20),
                disabled: true,
            }],
        };
        let mut state = RadioGroupState::default();
        group.pointer_event(&mut state, PointerEvent::Down, Point::new(5, 5));
        assert_eq!(
            group.pointer_event(&mut state, PointerEvent::Up, Point::new(5, 5)),
            None
        );
        group.set_focused(&mut state, Some(0));
        assert_eq!(state.selected, None);
        assert_eq!(state.focused, None);
    }
}

#[cfg(windows)]
pub mod d2d {
    use super::{RadioGroup, RadioGroupState};
    use crate::panel::d2d::PanelPainter;
    use windows::Win32::Graphics::Direct2D::ID2D1SolidColorBrush;

    pub struct RadioColors<'a> {
        pub ring: &'a ID2D1SolidColorBrush,
        pub selected: &'a ID2D1SolidColorBrush,
        pub focus: &'a ID2D1SolidColorBrush,
        pub disabled: &'a ID2D1SolidColorBrush,
    }

    impl RadioGroup {
        pub fn paint(
            &self,
            painter: &PanelPainter<'_>,
            state: &RadioGroupState,
            colors: RadioColors<'_>,
        ) {
            for (index, option) in self.options.iter().enumerate() {
                let center = option.rect.center();
                let radius = option.rect.width().min(option.rect.height()).max(1) as f32 / 2.0;
                let ring = if option.disabled {
                    colors.disabled
                } else {
                    colors.ring
                };
                painter.stroke_circle(
                    (center.x as f32, center.y as f32),
                    radius,
                    ring,
                    if state.hovered == Some(index) {
                        2.0
                    } else {
                        1.5
                    },
                );
                if state.selected == Some(index) {
                    painter.fill_circle(
                        (center.x as f32, center.y as f32),
                        (radius - 4.0).max(1.0),
                        if option.disabled {
                            colors.disabled
                        } else {
                            colors.selected
                        },
                    );
                }
                if state.focused == Some(index) && !option.disabled {
                    painter.stroke_circle(
                        (center.x as f32, center.y as f32),
                        radius + 3.0,
                        colors.focus,
                        1.5,
                    );
                }
            }
        }
    }
}
