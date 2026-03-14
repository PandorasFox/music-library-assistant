//! Wizard Pane: scroll state machine for the scrollable right-side info panel.
//!
//! Rendering lives in mm-tui. This module is purely state + input handling.

use crate::input::InputAction;

/// Scroll state for the wizard pane.
#[derive(Debug, Clone, Default)]
pub struct WizardPaneState {
    pub scroll: usize,
    pub content_height: usize,
    pub visible_height: usize,
}

impl WizardPaneState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.scroll = 0;
        self.content_height = 0;
        self.visible_height = 0;
    }

    /// Handle scroll input. Returns `true` if the input was consumed.
    pub fn handle_input(&mut self, action: &InputAction) -> bool {
        let max_scroll = self.content_height.saturating_sub(self.visible_height);

        match action {
            InputAction::NavUp => {
                self.scroll = self.scroll.saturating_sub(1);
                true
            }
            InputAction::NavDown => {
                self.scroll = (self.scroll + 1).min(max_scroll);
                true
            }
            InputAction::PageUp => {
                let page = self.visible_height.saturating_sub(1).max(1);
                self.scroll = self.scroll.saturating_sub(page);
                true
            }
            InputAction::PageDown => {
                let page = self.visible_height.saturating_sub(1).max(1);
                self.scroll = (self.scroll + page).min(max_scroll);
                true
            }
            InputAction::Home => {
                self.scroll = 0;
                true
            }
            InputAction::End => {
                self.scroll = max_scroll;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state(content: usize, visible: usize) -> WizardPaneState {
        WizardPaneState {
            scroll: 0,
            content_height: content,
            visible_height: visible,
        }
    }

    #[test]
    fn scroll_down_clamps() {
        let mut s = make_state(20, 10);
        s.scroll = 10; // at max
        s.handle_input(&InputAction::NavDown);
        assert_eq!(s.scroll, 10);
    }

    #[test]
    fn scroll_up_clamps() {
        let mut s = make_state(20, 10);
        s.scroll = 0;
        s.handle_input(&InputAction::NavUp);
        assert_eq!(s.scroll, 0);
    }

    #[test]
    fn page_down() {
        let mut s = make_state(30, 10);
        s.handle_input(&InputAction::PageDown);
        assert_eq!(s.scroll, 9); // visible_height - 1
    }

    #[test]
    fn page_up_from_middle() {
        let mut s = make_state(30, 10);
        s.scroll = 15;
        s.handle_input(&InputAction::PageUp);
        assert_eq!(s.scroll, 6); // 15 - 9
    }

    #[test]
    fn home_end() {
        let mut s = make_state(30, 10);
        s.scroll = 5;
        s.handle_input(&InputAction::Home);
        assert_eq!(s.scroll, 0);

        s.handle_input(&InputAction::End);
        assert_eq!(s.scroll, 20); // 30 - 10
    }

    #[test]
    fn unhandled_input_returns_false() {
        let mut s = make_state(20, 10);
        assert!(!s.handle_input(&InputAction::Confirm));
        assert!(!s.handle_input(&InputAction::Cancel));
    }

    #[test]
    fn reset_clears_state() {
        let mut s = make_state(20, 10);
        s.scroll = 5;
        s.reset();
        assert_eq!(s.scroll, 0);
        assert_eq!(s.content_height, 0);
        assert_eq!(s.visible_height, 0);
    }

    #[test]
    fn short_content_no_scroll() {
        let mut s = make_state(5, 10);
        s.handle_input(&InputAction::NavDown);
        assert_eq!(s.scroll, 0); // max_scroll = 0
    }
}
