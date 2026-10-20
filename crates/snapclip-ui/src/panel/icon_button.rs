use snapclip_model::geometry::{Point, Rect};

use super::interaction::PointerEvent;
use super::{Button, ButtonState, Glyph};

/// A button whose icon is supplied by the renderer's shared asset cache.
///
/// The component owns the hit rectangle and interaction state. It deliberately does not own a WIC
/// decoder or a D2D bitmap: those resources belong to the device lifecycle of the composing
/// renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconButton {
    pub button: Button,
    pub glyph: Glyph,
}

impl IconButton {
    pub const fn new(rect: Rect, glyph: Glyph) -> Self {
        Self {
            button: Button::new(rect),
            glyph,
        }
    }

    pub const fn rect(self) -> Rect {
        self.button.rect
    }

    pub fn hit_test(self, point: Point) -> bool {
        self.button.hit_test(point)
    }

    pub fn pointer_event(self, state: &mut ButtonState, event: PointerEvent, point: Point) -> bool {
        self.button.pointer_event(state, event, point)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_button_uses_one_rect_for_paint_and_hit_testing() {
        let button = IconButton::new(Rect::new(10, 20, 34, 44), Glyph::Close);
        assert_eq!(button.rect(), Rect::new(10, 20, 34, 44));
        assert!(button.hit_test(Point::new(10, 20)));
        assert!(!button.hit_test(Point::new(34, 43)));
        assert_eq!(button.glyph, Glyph::Close);
    }

    #[test]
    fn icon_button_uses_the_same_press_and_release_contract_as_button() {
        let button = IconButton::new(Rect::new(10, 20, 34, 44), Glyph::Close);
        let mut state = ButtonState::default();
        assert!(!button.pointer_event(&mut state, PointerEvent::Down, Point::new(20, 30)));
        assert!(button.pointer_event(&mut state, PointerEvent::Up, Point::new(20, 30)));
    }
}
