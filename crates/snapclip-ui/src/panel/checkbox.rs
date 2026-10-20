use snapclip_model::geometry::{Point, Rect};

use super::interaction::{InteractionState, PointerEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CheckboxState {
    pub interaction: InteractionState,
    pub checked: bool,
    pub indeterminate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checkbox {
    pub rect: Rect,
}

impl Checkbox {
    pub const fn new(rect: Rect) -> Self {
        Self { rect }
    }

    pub fn hit_test(self, point: Point) -> bool {
        self.rect.contains(point)
    }

    /// Update pointer state and toggle on release inside, matching native checkbox behavior.
    pub fn pointer_event(
        self,
        state: &mut CheckboxState,
        event: PointerEvent,
        point: Point,
    ) -> bool {
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
                    state.indeterminate = false;
                }
                activated
            }
        }
    }
}

#[cfg(windows)]
pub mod d2d {
    use super::{Checkbox, CheckboxState};
    use crate::panel::d2d::PanelPainter;
    use snapclip_model::geometry::{Point, Rect};
    use windows::Win32::Graphics::Direct2D::ID2D1SolidColorBrush;

    pub struct CheckboxColors<'a> {
        pub surface: &'a ID2D1SolidColorBrush,
        pub border: &'a ID2D1SolidColorBrush,
        pub selected: &'a ID2D1SolidColorBrush,
        pub hover: &'a ID2D1SolidColorBrush,
        pub focus: &'a ID2D1SolidColorBrush,
        pub mark: &'a ID2D1SolidColorBrush,
        pub disabled: &'a ID2D1SolidColorBrush,
    }

    impl Checkbox {
        /// Paint the check/dash reveal at normalized progress `0..=1`.
        pub fn paint(
            self,
            painter: &PanelPainter<'_>,
            state: CheckboxState,
            colors: CheckboxColors<'_>,
            progress: f32,
            radius: i32,
        ) {
            let interaction = state.interaction;
            let disabled = interaction.disabled;
            let box_fill = if disabled {
                colors.surface
            } else if state.checked || state.indeterminate {
                colors.selected
            } else {
                colors.surface
            };
            painter.fill_round(self.rect, radius, box_fill);
            let border = if disabled {
                colors.disabled
            } else if interaction.hovered || interaction.pressed || state.checked {
                colors.selected
            } else {
                colors.border
            };
            painter.stroke_round(self.rect, radius, border, 1.0);

            let progress = progress.clamp(0.0, 1.0);
            let mark = if disabled {
                colors.disabled
            } else {
                colors.mark
            };
            if state.indeterminate {
                let center_y = (self.rect.top + self.rect.bottom) / 2;
                let half_width = self.rect.width() / 4;
                let half_line = (half_width as f32 * progress).round() as i32;
                painter.line(
                    (self.rect.center().x - half_line, center_y),
                    (self.rect.center().x + half_line, center_y),
                    mark,
                    2.0,
                );
            } else if state.checked && progress > 0.0 {
                let a = Point::new(
                    self.rect.left + self.rect.width() / 4,
                    self.rect.top + self.rect.height() / 2,
                );
                let b = Point::new(
                    self.rect.left + self.rect.width() * 7 / 16,
                    self.rect.top + self.rect.height() * 3 / 4,
                );
                let c = Point::new(
                    self.rect.left + self.rect.width() * 3 / 4,
                    self.rect.top + self.rect.height() / 4,
                );
                let first_share = 0.42;
                if progress <= first_share {
                    let t = progress / first_share;
                    painter.line(
                        (a.x, a.y),
                        (lerp(a.x, b.x, t), lerp(a.y, b.y, t)),
                        mark,
                        2.0,
                    );
                } else {
                    painter.line((a.x, a.y), (b.x, b.y), mark, 2.0);
                    let t = (progress - first_share) / (1.0 - first_share);
                    painter.line(
                        (b.x, b.y),
                        (lerp(b.x, c.x, t), lerp(b.y, c.y, t)),
                        mark,
                        2.0,
                    );
                }
            }
            if interaction.focused && !disabled {
                let outset = 3;
                painter.stroke_round(
                    Rect::new(
                        self.rect.left - outset,
                        self.rect.top - outset,
                        self.rect.right + outset,
                        self.rect.bottom + outset,
                    ),
                    radius + outset,
                    colors.focus,
                    2.0,
                );
            }
        }
    }

    fn lerp(from: i32, to: i32, t: f32) -> i32 {
        (from as f32 + (to - from) as f32 * t.clamp(0.0, 1.0)).round() as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_toggles_only_when_released_inside() {
        let checkbox = Checkbox::new(Rect::new(0, 0, 16, 16));
        let mut state = CheckboxState::default();
        checkbox.pointer_event(&mut state, PointerEvent::Down, Point::new(8, 8));
        assert!(checkbox.pointer_event(&mut state, PointerEvent::Up, Point::new(8, 8)));
        assert!(state.checked);
        assert!(!state.indeterminate);
    }

    #[test]
    fn releasing_outside_does_not_toggle_and_clears_pressed() {
        let checkbox = Checkbox::new(Rect::new(0, 0, 16, 16));
        let mut state = CheckboxState::default();
        checkbox.pointer_event(&mut state, PointerEvent::Down, Point::new(8, 8));
        assert!(!checkbox.pointer_event(&mut state, PointerEvent::Up, Point::new(20, 8)));
        assert!(!state.checked);
        assert!(!state.interaction.pressed);
    }

    #[test]
    fn toggling_from_indeterminate_selects_checked() {
        let checkbox = Checkbox::new(Rect::new(0, 0, 16, 16));
        let mut state = CheckboxState {
            indeterminate: true,
            ..Default::default()
        };
        checkbox.pointer_event(&mut state, PointerEvent::Down, Point::new(8, 8));
        checkbox.pointer_event(&mut state, PointerEvent::Up, Point::new(8, 8));
        assert!(state.checked);
        assert!(!state.indeterminate);
    }

    #[test]
    fn disabled_checkbox_cannot_toggle() {
        let checkbox = Checkbox::new(Rect::new(0, 0, 16, 16));
        let mut state = CheckboxState {
            interaction: InteractionState {
                disabled: true,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!checkbox.pointer_event(&mut state, PointerEvent::Down, Point::new(8, 8)));
        assert!(!checkbox.pointer_event(&mut state, PointerEvent::Up, Point::new(8, 8)));
        assert!(!state.checked);
    }
}
