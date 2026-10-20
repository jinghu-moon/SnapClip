/// Pointer events understood by every interactive panel control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerEvent {
    Enter,
    Leave,
    Down,
    Up,
    Cancel,
}

/// The input state shared by controls. Pointer capture is represented by `pressed`: leaving the
/// paint rect while the button is held must not cancel the press until an up/cancel event arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InteractionState {
    pub hovered: bool,
    pub pressed: bool,
    pub focused: bool,
    pub disabled: bool,
}

impl InteractionState {
    /// Applies one pointer event and reports whether the state changed.
    pub fn apply(&mut self, event: PointerEvent) -> bool {
        let before = *self;
        if self.disabled {
            self.pressed = false;
            return before != *self;
        }
        match event {
            PointerEvent::Enter => self.hovered = true,
            PointerEvent::Leave => self.hovered = false,
            PointerEvent::Down => {
                self.hovered = true;
                self.pressed = true;
                self.focused = true;
            }
            PointerEvent::Up | PointerEvent::Cancel => self.pressed = false,
        }
        before != *self
    }

    pub fn set_disabled(&mut self, disabled: bool) -> bool {
        if self.disabled == disabled {
            return false;
        }
        self.disabled = disabled;
        if disabled {
            self.pressed = false;
            self.focused = false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_survives_pointer_leave_until_release() {
        let mut state = InteractionState::default();
        state.apply(PointerEvent::Enter);
        state.apply(PointerEvent::Down);
        state.apply(PointerEvent::Leave);
        assert!(!state.hovered);
        assert!(state.pressed);
        state.apply(PointerEvent::Up);
        assert!(!state.pressed);
    }

    #[test]
    fn disabled_controls_cannot_start_or_keep_a_press() {
        let mut state = InteractionState::default();
        state.set_disabled(true);
        state.apply(PointerEvent::Down);
        assert_eq!(
            state,
            InteractionState {
                disabled: true,
                ..Default::default()
            }
        );
    }

    #[test]
    fn cancel_releases_pointer_capture() {
        let mut state = InteractionState::default();
        state.apply(PointerEvent::Down);
        assert!(state.pressed);
        state.apply(PointerEvent::Cancel);
        assert!(!state.pressed);
    }
}
