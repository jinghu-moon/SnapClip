use snapclip_model::geometry::{Point, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropdownKey {
    Up,
    Down,
    Enter,
    Escape,
    Home,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropdownTarget {
    Button,
    Item(usize),
    Padding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropdown {
    pub button: Rect,
    pub menu: Option<Rect>,
    pub items: Vec<Rect>,
    pub selected: Option<usize>,
    pub hovered: Option<usize>,
    pub open: bool,
    pub focused: bool,
    pub disabled: bool,
}

impl Dropdown {
    pub fn new(button: Rect, menu: Rect, items: Vec<Rect>, selected: Option<usize>) -> Self {
        Self {
            button,
            menu: Some(menu),
            items,
            selected,
            hovered: None,
            open: false,
            focused: false,
            disabled: false,
        }
    }

    pub fn target_at(&self, point: Point) -> Option<DropdownTarget> {
        if self.button.contains(point) {
            return Some(DropdownTarget::Button);
        }
        let menu = self.menu?;
        if !menu.contains(point) {
            return None;
        }
        self.items
            .iter()
            .position(|rect| rect.contains(point))
            .map(DropdownTarget::Item)
            .or(Some(DropdownTarget::Padding))
    }

    pub fn pointer_down(&mut self, point: Point) -> bool {
        if self.disabled || !self.button.contains(point) {
            return false;
        }
        self.focused = true;
        self.open = !self.open;
        if self.open && self.hovered.is_none() {
            self.hovered = self.selected;
        }
        true
    }

    pub fn pointer_move(&mut self, point: Point) -> Option<usize> {
        if !self.open {
            return None;
        }
        self.hovered = self.items.iter().position(|rect| rect.contains(point));
        self.hovered
    }

    pub fn pointer_up(&mut self, point: Point) -> Option<usize> {
        if !self.open {
            return None;
        }
        let selected = self.items.iter().position(|rect| rect.contains(point));
        if let Some(index) = selected {
            self.selected = Some(index);
        }
        if selected.is_some() || self.menu.is_some_and(|rect| !rect.contains(point)) {
            self.open = false;
        }
        selected
    }

    pub fn key(&mut self, key: DropdownKey) -> Option<usize> {
        if self.disabled {
            return None;
        }
        match key {
            DropdownKey::Escape => {
                self.open = false;
                None
            }
            DropdownKey::Enter => {
                if !self.open {
                    self.open = true;
                    self.focused = true;
                    None
                } else {
                    let chosen = self.hovered.or(self.selected);
                    self.selected = chosen;
                    self.open = false;
                    chosen
                }
            }
            DropdownKey::Up | DropdownKey::Down | DropdownKey::Home | DropdownKey::End => {
                if self.items.is_empty() {
                    return None;
                }
                self.open = true;
                let current = self.hovered.or(self.selected).unwrap_or(0);
                let next = match key {
                    DropdownKey::Up => current.saturating_sub(1),
                    DropdownKey::Down => (current + 1).min(self.items.len() - 1),
                    DropdownKey::Home => 0,
                    DropdownKey::End => self.items.len() - 1,
                    DropdownKey::Enter | DropdownKey::Escape => current,
                };
                self.hovered = Some(next);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_hit_testing_checks_items_before_padding() {
        let dropdown = Dropdown {
            button: Rect::new(0, 0, 80, 24),
            menu: Some(Rect::new(0, 24, 80, 80)),
            items: vec![Rect::new(4, 28, 76, 50), Rect::new(4, 52, 76, 76)],
            selected: None,
            hovered: None,
            open: false,
            focused: false,
            disabled: false,
        };
        assert_eq!(
            dropdown.target_at(Point::new(8, 8)),
            Some(DropdownTarget::Button)
        );
        assert_eq!(
            dropdown.target_at(Point::new(8, 35)),
            Some(DropdownTarget::Item(0))
        );
        assert_eq!(
            dropdown.target_at(Point::new(1, 25)),
            Some(DropdownTarget::Padding)
        );
        assert_eq!(dropdown.target_at(Point::new(90, 35)), None);
    }

    #[test]
    fn keyboard_navigation_selects_and_closes_the_menu() {
        let mut dropdown = Dropdown::new(
            Rect::new(0, 0, 80, 24),
            Rect::new(0, 24, 80, 80),
            vec![Rect::new(4, 28, 76, 50), Rect::new(4, 52, 76, 76)],
            Some(0),
        );
        dropdown.key(DropdownKey::Enter);
        dropdown.key(DropdownKey::Down);
        assert_eq!(dropdown.hovered, Some(1));
        assert_eq!(dropdown.key(DropdownKey::Enter), Some(1));
        assert_eq!(dropdown.selected, Some(1));
        assert!(!dropdown.open);
    }
}

#[cfg(windows)]
pub mod d2d {
    use super::{Dropdown, DropdownTarget};
    use crate::panel::d2d::PanelPainter;
    use snapclip_model::geometry::Rect;
    use windows::Win32::Graphics::Direct2D::ID2D1SolidColorBrush;
    use windows::Win32::Graphics::DirectWrite::IDWriteTextFormat;

    pub struct DropdownColors<'a> {
        pub button: &'a ID2D1SolidColorBrush,
        pub menu: &'a ID2D1SolidColorBrush,
        pub selected: &'a ID2D1SolidColorBrush,
        pub line: &'a ID2D1SolidColorBrush,
        pub text: &'a ID2D1SolidColorBrush,
        pub muted: &'a ID2D1SolidColorBrush,
        pub focus: &'a ID2D1SolidColorBrush,
    }

    impl Dropdown {
        pub fn paint(
            &self,
            painter: &PanelPainter<'_>,
            labels: &[&str],
            format: &IDWriteTextFormat,
            selected: Option<usize>,
            hovered: Option<DropdownTarget>,
            focused: bool,
            colors: DropdownColors<'_>,
        ) {
            painter.fill_round(self.button, 5, colors.button);
            painter.stroke_round(self.button, 5, colors.line, 1.0);
            if focused {
                painter.stroke_round(self.button, 5, colors.focus, 2.0);
            }
            if let Some(index) = selected.and_then(|index| labels.get(index)) {
                painter.draw_text(index, format, inset_text(self.button), colors.text);
            }
            if let Some(menu) = self.menu {
                painter.fill_round(menu, 6, colors.menu);
                painter.stroke_round(menu, 6, colors.line, 1.0);
                for (index, rect) in self.items.iter().enumerate() {
                    if hovered == Some(DropdownTarget::Item(index)) {
                        painter.fill_round(*rect, 4, colors.selected);
                    }
                    if let Some(label) = labels.get(index) {
                        painter.draw_text(
                            label,
                            format,
                            inset_text(*rect),
                            if selected == Some(index) {
                                colors.text
                            } else {
                                colors.muted
                            },
                        );
                    }
                }
            }
        }
    }

    fn inset_text(rect: Rect) -> Rect {
        Rect::new(rect.left + 8, rect.top, rect.right - 8, rect.bottom)
    }
}
