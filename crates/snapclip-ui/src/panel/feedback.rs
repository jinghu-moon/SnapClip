use snapclip_model::geometry::Rect;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tooltip {
    pub anchor: Rect,
    pub text: String,
    pub visible: bool,
    pub delay_ms: u64,
    pub elapsed_ms: u64,
}

impl Tooltip {
    pub fn new(anchor: Rect, text: impl Into<String>) -> Self {
        Self {
            anchor,
            text: text.into(),
            visible: false,
            delay_ms: 450,
            elapsed_ms: 0,
        }
    }
    pub fn set_hovered(&mut self, hovered: bool) {
        if hovered {
            self.elapsed_ms = 0;
        } else {
            self.visible = false;
        }
    }
    pub fn tick(&mut self, delta_ms: u64) -> bool {
        if self.visible {
            return false;
        }
        self.elapsed_ms = self.elapsed_ms.saturating_add(delta_ms);
        if self.elapsed_ms >= self.delay_ms {
            self.visible = true;
            return true;
        }
        false
    }
    pub fn rect(&self, width: i32, height: i32) -> Rect {
        Rect::new(
            self.anchor.left,
            self.anchor.top - height - 8,
            self.anchor.left + width,
            self.anchor.top - 8,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub text: String,
    pub visible: bool,
    pub duration_ms: u64,
    pub elapsed_ms: u64,
}

impl Toast {
    pub fn new(duration_ms: u64) -> Self {
        Self {
            text: String::new(),
            visible: false,
            duration_ms,
            elapsed_ms: 0,
        }
    }
    pub fn show(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.elapsed_ms = 0;
        self.visible = true;
    }
    pub fn dismiss(&mut self) {
        self.visible = false;
    }
    pub fn tick(&mut self, delta_ms: u64) -> bool {
        if !self.visible {
            return false;
        }
        self.elapsed_ms = self.elapsed_ms.saturating_add(delta_ms);
        if self.elapsed_ms >= self.duration_ms {
            self.visible = false;
            return true;
        }
        false
    }
    pub fn opacity(&self) -> f32 {
        if !self.visible {
            return 0.0;
        }
        if self.elapsed_ms == 0 {
            return 1.0;
        }
        let fade = 180_u64;
        let remaining = self.duration_ms.saturating_sub(self.elapsed_ms);
        (self.elapsed_ms.min(fade) as f32 / fade as f32).min(1.0)
            * (remaining.min(fade) as f32 / fade as f32).min(1.0)
    }
}

#[cfg(windows)]
pub mod d2d {
    use super::{Toast, Tooltip};
    use crate::panel::d2d::PanelPainter;
    use snapclip_model::geometry::Rect;
    use windows::Win32::Graphics::Direct2D::ID2D1SolidColorBrush;
    use windows::Win32::Graphics::DirectWrite::IDWriteTextFormat;

    pub struct FeedbackColors<'a> {
        pub fill: &'a ID2D1SolidColorBrush,
        pub line: &'a ID2D1SolidColorBrush,
        pub text: &'a ID2D1SolidColorBrush,
    }

    impl Tooltip {
        pub fn paint(
            &self,
            painter: &PanelPainter<'_>,
            format: &IDWriteTextFormat,
            width: i32,
            colors: FeedbackColors<'_>,
        ) {
            if !self.visible {
                return;
            }
            let rect = self.rect(width, 28);
            painter.fill_round(rect, 5, colors.fill);
            painter.stroke_round(rect, 5, colors.line, 1.0);
            painter.draw_text(
                &self.text,
                format,
                Rect::new(rect.left + 8, rect.top, rect.right - 8, rect.bottom),
                colors.text,
            );
        }
    }

    impl Toast {
        pub fn paint(
            &self,
            painter: &PanelPainter<'_>,
            format: &IDWriteTextFormat,
            rect: Rect,
            colors: FeedbackColors<'_>,
        ) {
            if !self.visible {
                return;
            }
            painter.fill_round(rect, 7, colors.fill);
            painter.stroke_round(rect, 7, colors.line, 1.0);
            painter.draw_text(
                &self.text,
                format,
                Rect::new(rect.left + 12, rect.top, rect.right - 12, rect.bottom),
                colors.text,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_appears_after_delay_and_hides_on_leave() {
        let mut tooltip = Tooltip::new(Rect::new(10, 20, 30, 40), "help");
        tooltip.set_hovered(true);
        assert!(!tooltip.tick(449));
        assert!(tooltip.tick(1));
        assert!(tooltip.visible);
        tooltip.set_hovered(false);
        assert!(!tooltip.visible);
    }

    #[test]
    fn toast_expires_and_reports_progress() {
        let mut toast = Toast::new(1000);
        toast.show("saved");
        assert!(toast.opacity() > 0.0);
        assert!(!toast.tick(999));
        assert!(toast.tick(1));
        assert!(!toast.visible);
    }
}
