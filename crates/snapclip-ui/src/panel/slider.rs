use snapclip_model::geometry::{Point, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliderKey {
    Left,
    Right,
    Home,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slider {
    pub rect: Rect,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub value: f32,
    pub hovered: bool,
    pub pressed: bool,
    pub focused: bool,
    pub disabled: bool,
}

impl Slider {
    pub fn new(rect: Rect, min: f32, max: f32, value: f32, step: f32) -> Self {
        assert!(max >= min, "slider max must be >= min");
        Self {
            rect,
            min,
            max,
            step: step.max(f32::EPSILON),
            value: value.clamp(min, max),
            hovered: false,
            pressed: false,
            focused: false,
            disabled: false,
        }
    }

    pub fn normalized(&self) -> f32 {
        if self.max <= self.min {
            return 0.0;
        }
        (self.value - self.min) / (self.max - self.min)
    }

    pub fn thumb_center(&self) -> Point {
        let x = self.rect.left as f32 + self.normalized() * self.rect.width() as f32;
        Point::new(x.round() as i32, self.rect.center().y)
    }

    pub fn hit_test(&self, point: Point) -> bool {
        self.rect.inflate(8).contains(point)
    }

    pub fn set_value(&mut self, value: f32) -> bool {
        let next = snap(value, self.min, self.max, self.step);
        if (next - self.value).abs() < f32::EPSILON {
            return false;
        }
        self.value = next;
        true
    }

    pub fn pointer_down(&mut self, point: Point) -> bool {
        if self.disabled || !self.hit_test(point) {
            return false;
        }
        self.hovered = true;
        self.pressed = true;
        self.focused = true;
        self.set_from_pointer(point)
    }

    pub fn pointer_move(&mut self, point: Point) -> bool {
        self.hovered = self.hit_test(point);
        if self.pressed && !self.disabled {
            return self.set_from_pointer(point);
        }
        false
    }

    pub fn pointer_up(&mut self, point: Point) -> bool {
        let changed = self.pressed && self.set_from_pointer(point);
        self.hovered = self.hit_test(point);
        self.pressed = false;
        changed
    }

    pub fn cancel(&mut self) {
        self.pressed = false;
    }

    pub fn key(&mut self, key: SliderKey) -> bool {
        if self.disabled {
            return false;
        }
        match key {
            SliderKey::Left => self.set_value(self.value - self.step),
            SliderKey::Right => self.set_value(self.value + self.step),
            SliderKey::Home => self.set_value(self.min),
            SliderKey::End => self.set_value(self.max),
        }
    }

    fn set_from_pointer(&mut self, point: Point) -> bool {
        let width = self.rect.width().max(1) as f32;
        let t = (point.x - self.rect.left) as f32 / width;
        self.set_value(self.min + t.clamp(0.0, 1.0) * (self.max - self.min))
    }
}

fn snap(value: f32, min: f32, max: f32, step: f32) -> f32 {
    let steps = ((value - min) / step).round();
    (min + steps * step).clamp(min, max)
}

#[cfg(windows)]
pub mod d2d {
    use super::Slider;
    use crate::panel::d2d::PanelPainter;
    use snapclip_model::geometry::Rect;
    use windows::Win32::Graphics::Direct2D::ID2D1SolidColorBrush;

    pub struct SliderColors<'a> {
        pub track: &'a ID2D1SolidColorBrush,
        pub fill: &'a ID2D1SolidColorBrush,
        pub thumb: &'a ID2D1SolidColorBrush,
        pub hover: &'a ID2D1SolidColorBrush,
        pub focus: &'a ID2D1SolidColorBrush,
        pub disabled: &'a ID2D1SolidColorBrush,
    }

    impl Slider {
        pub fn paint(&self, painter: &PanelPainter<'_>, colors: SliderColors<'_>) {
            let track = Rect::new(
                self.rect.left,
                self.rect.center().y - 2,
                self.rect.right,
                self.rect.center().y + 2,
            );
            painter.fill_round(
                track,
                2,
                if self.disabled {
                    colors.disabled
                } else {
                    colors.track
                },
            );
            let thumb = self.thumb_center();
            if thumb.x > track.left {
                painter.fill_round(
                    Rect::new(track.left, track.top, thumb.x, track.bottom),
                    2,
                    colors.fill,
                );
            }
            painter.fill_circle(
                (thumb.x as f32, thumb.y as f32),
                7.0,
                if self.disabled {
                    colors.disabled
                } else if self.pressed || self.hovered {
                    colors.hover
                } else {
                    colors.thumb
                },
            );
            if self.focused && !self.disabled {
                painter.stroke_circle((thumb.x as f32, thumb.y as f32), 10.0, colors.focus, 2.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_and_keyboard_values_snap_to_step() {
        let mut slider = Slider::new(Rect::new(10, 0, 110, 20), 0.0, 10.0, 0.0, 1.0);
        slider.pointer_down(Point::new(65, 10));
        assert_eq!(slider.value, 6.0);
        slider.pointer_up(Point::new(65, 10));
        slider.key(SliderKey::Right);
        assert_eq!(slider.value, 7.0);
    }

    #[test]
    fn release_and_cancel_end_capture() {
        let mut slider = Slider::new(Rect::new(0, 0, 100, 20), 0.0, 1.0, 0.5, 0.1);
        slider.pointer_down(Point::new(50, 10));
        assert!(slider.pressed);
        slider.cancel();
        assert!(!slider.pressed);
    }
}
