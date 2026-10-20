use snapclip_model::geometry::{Point, Rect};

use super::interaction::{InteractionState, PointerEvent};

/// State for a selectable pill-shaped tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TagState {
    pub selected: bool,
    pub interaction: InteractionState,
}

/// A compact selectable label. Text is painted by the composing panel so its font and alignment
/// remain domain-specific; this component owns only the pill geometry and state chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tag {
    pub rect: Rect,
}

impl Tag {
    pub const fn new(rect: Rect) -> Self {
        Self { rect }
    }

    pub fn hit_test(self, point: Point) -> bool {
        self.rect.contains(point)
    }

    pub fn radius(self) -> i32 {
        (self.rect.height() / 2).max(0)
    }

    pub fn pointer_event(self, state: &mut TagState, event: PointerEvent, point: Point) -> bool {
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
                    state.selected = !state.selected;
                }
                activated
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_uses_its_pill_rect_for_hit_testing() {
        let tag = Tag::new(Rect::new(10, 20, 74, 44));
        assert_eq!(tag.radius(), 12);
        assert!(tag.hit_test(Point::new(10, 20)));
        assert!(tag.hit_test(Point::new(73, 43)));
        assert!(!tag.hit_test(Point::new(74, 43)));
    }

    #[test]
    fn tag_toggles_only_when_released_inside() {
        let tag = Tag::new(Rect::new(10, 20, 74, 44));
        let mut state = TagState::default();
        tag.pointer_event(&mut state, PointerEvent::Down, Point::new(20, 30));
        assert!(tag.pointer_event(&mut state, PointerEvent::Up, Point::new(20, 30)));
        assert!(state.selected);
        tag.pointer_event(&mut state, PointerEvent::Down, Point::new(20, 30));
        assert!(!tag.pointer_event(&mut state, PointerEvent::Up, Point::new(100, 30)));
        assert!(state.selected);
    }
}

#[cfg(windows)]
pub mod d2d {
    use super::{Tag, TagState};
    use crate::panel::d2d::PanelPainter;
    use windows::Win32::Graphics::Direct2D::ID2D1SolidColorBrush;

    pub struct TagColors<'a> {
        pub off_fill: &'a ID2D1SolidColorBrush,
        pub off_hover: &'a ID2D1SolidColorBrush,
        pub on_fill: &'a ID2D1SolidColorBrush,
        pub line: &'a ID2D1SolidColorBrush,
        pub on_line: &'a ID2D1SolidColorBrush,
        pub focus: &'a ID2D1SolidColorBrush,
        pub disabled: &'a ID2D1SolidColorBrush,
    }

    impl Tag {
        pub fn paint(self, painter: &PanelPainter<'_>, state: TagState, colors: TagColors<'_>) {
            let fill = if state.interaction.disabled {
                colors.disabled
            } else if state.selected {
                colors.on_fill
            } else if state.interaction.hovered {
                colors.off_hover
            } else {
                colors.off_fill
            };
            painter.fill_round(self.rect, self.radius(), fill);

            let line = if state.interaction.disabled {
                colors.disabled
            } else if state.selected {
                colors.on_line
            } else {
                colors.line
            };
            painter.stroke_round(self.rect, self.radius(), line, 1.0);
            if state.interaction.focused && !state.interaction.disabled {
                painter.stroke_round(self.rect, self.radius(), colors.focus, 2.0);
            }
        }
    }
}
