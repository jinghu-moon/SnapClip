use super::interaction::{InteractionState, PointerEvent};
use snapclip_model::geometry::{Point, Rect};

pub type ButtonState = InteractionState;

/// The geometry contract for a panel button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Button {
    pub rect: Rect,
}

impl Button {
    pub const fn new(rect: Rect) -> Self {
        Self { rect }
    }

    pub fn hit_test(self, point: Point) -> bool {
        self.rect.contains(point)
    }

    /// Apply pointer input and return whether this button was activated.
    ///
    /// The caller owns OS pointer capture. While captured, `Leave` clears hover but preserves the
    /// press; releasing outside or cancelling capture never activates the button.
    pub fn pointer_event(self, state: &mut ButtonState, event: PointerEvent, point: Point) -> bool {
        if state.disabled {
            state.pressed = false;
            return false;
        }
        match event {
            PointerEvent::Enter => {
                state.apply(event);
            }
            PointerEvent::Leave => {
                state.apply(event);
            }
            PointerEvent::Down if self.hit_test(point) => {
                state.apply(event);
            }
            PointerEvent::Down => {}
            PointerEvent::Up => {
                let activated = state.pressed && self.hit_test(point);
                state.hovered = self.hit_test(point);
                state.apply(event);
                return activated;
            }
            PointerEvent::Cancel => {
                state.apply(event);
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_testing_uses_the_paint_rect() {
        let button = Button::new(Rect::new(10, 20, 40, 50));
        assert!(button.hit_test(Point::new(10, 20)));
        assert!(button.hit_test(Point::new(39, 49)));
        assert!(!button.hit_test(Point::new(40, 49)));
        assert!(!button.hit_test(Point::new(25, 50)));
    }

    #[test]
    fn pointer_release_activates_only_when_it_ends_inside_after_a_press_inside() {
        let button = Button::new(Rect::new(10, 20, 40, 50));
        let mut state = ButtonState::default();
        assert!(!button.pointer_event(&mut state, PointerEvent::Down, Point::new(20, 30)));
        assert!(state.pressed && state.focused);
        assert!(button.pointer_event(&mut state, PointerEvent::Up, Point::new(20, 30)));
        assert!(!state.pressed);
    }

    #[test]
    fn leaving_during_capture_keeps_press_but_release_outside_cancels() {
        let button = Button::new(Rect::new(10, 20, 40, 50));
        let mut state = ButtonState::default();
        button.pointer_event(&mut state, PointerEvent::Down, Point::new(20, 30));
        button.pointer_event(&mut state, PointerEvent::Leave, Point::new(50, 60));
        assert!(state.pressed);
        assert!(!state.hovered);
        assert!(!button.pointer_event(&mut state, PointerEvent::Up, Point::new(50, 60)));
        assert!(!state.pressed);
    }

    #[test]
    fn disabled_button_never_activates() {
        let button = Button::new(Rect::new(10, 20, 40, 50));
        let mut state = ButtonState {
            disabled: true,
            ..Default::default()
        };
        assert!(!button.pointer_event(&mut state, PointerEvent::Down, Point::new(20, 30)));
        assert!(!button.pointer_event(&mut state, PointerEvent::Up, Point::new(20, 30)));
        assert!(!state.pressed);
    }
}

#[cfg(windows)]
pub mod d2d {
    use super::{Button, ButtonState};
    use crate::panel::d2d::PanelPainter;
    use windows::Win32::Graphics::Direct2D::ID2D1SolidColorBrush;

    pub struct ButtonColors<'a> {
        pub fill: &'a ID2D1SolidColorBrush,
        pub hover: &'a ID2D1SolidColorBrush,
        pub pressed: &'a ID2D1SolidColorBrush,
        pub focus: &'a ID2D1SolidColorBrush,
        pub disabled: &'a ID2D1SolidColorBrush,
    }

    impl Button {
        pub fn paint(
            self,
            painter: &PanelPainter<'_>,
            state: ButtonState,
            colors: ButtonColors<'_>,
            radius: i32,
        ) {
            let brush = if state.disabled {
                colors.disabled
            } else if state.pressed {
                colors.pressed
            } else if state.hovered {
                colors.hover
            } else {
                colors.fill
            };
            painter.fill_round(self.rect, radius, brush);
            if state.focused && !state.disabled {
                painter.stroke_round(self.rect, radius, colors.focus, 2.0);
            }
        }
    }
}
