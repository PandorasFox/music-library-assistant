//! Resolution Layout Widget
//!
//! Provides a standard two-pane layout for resolution modals with:
//! - Info bar at top (full-width, shows untruncated path)
//! - Left pane (~33%) for file list
//! - Right pane (~67%) for details (informational, not focusable)
//! - Bottom bar for decision buttons (focusable via Shift+Up/Down)
//!
//! ```text
//! +------------------------------------------+
//! | Full path info bar (untruncated)         |  <- 3 lines
//! +------------------------------------------+
//! | ~33% List     | ~67% Details             |  <- Min(5)
//! |               |                          |
//! +------------------------------------------+
//! | Decision Buttons                         |  <- 2 lines
//! +------------------------------------------+
//! ```

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Focus pane for resolution modals.
///
/// Details pane is not focusable (informational only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPane {
    /// File list pane (default)
    #[default]
    List,
    /// Decision buttons bar
    Buttons,
}

impl FocusPane {
    /// Cycle to next focus pane (Shift+Down)
    pub fn next(self) -> Self {
        match self {
            Self::List => Self::Buttons,
            Self::Buttons => Self::List,
        }
    }

    /// Cycle to previous focus pane (Shift+Up)
    pub fn prev(self) -> Self {
        // With only 2 panes, prev == next
        self.next()
    }
}

/// Layout areas for a resolution modal.
#[derive(Debug, Clone, Copy)]
pub struct ResolutionLayout {
    /// Area for the full-width info bar showing selected item's full path
    pub info_bar: Rect,
    /// Area for the left pane (file list)
    pub list_pane: Rect,
    /// Area for the right pane (details) - informational only, not focusable
    pub details_pane: Rect,
    /// Area for bottom decision buttons
    pub buttons: Rect,
}

impl ResolutionLayout {
    /// Build a resolution layout from the given area.
    ///
    /// # Arguments
    /// * `area` - The full area to divide
    /// * `info_height` - Height of the info bar (typically 3 for bordered)
    /// * `buttons_height` - Height of the buttons bar (typically 2)
    /// * `list_percent` - Percentage of horizontal space for list pane (default 33)
    pub fn new(area: Rect, info_height: u16, buttons_height: u16, list_percent: u16) -> Self {
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(info_height),   // Info bar
                Constraint::Min(5),                // Content panes
                Constraint::Length(buttons_height), // Buttons
            ])
            .split(area);

        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(list_percent),
                Constraint::Percentage(100 - list_percent),
            ])
            .split(vertical[1]);

        Self {
            info_bar: vertical[0],
            list_pane: horizontal[0],
            details_pane: horizontal[1],
            buttons: vertical[2],
        }
    }

    /// Apply 1-cell padding to an area (for full-area modals).
    pub fn padded(area: Rect) -> Rect {
        Rect {
            x: area.x.saturating_add(1),
            y: area.y.saturating_add(1),
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        }
    }
}

/// Storage for button rectangles to enable click detection.
#[derive(Debug, Clone, Default)]
pub struct ButtonRects {
    /// Map of button identifier to its rendered rectangle.
    /// Stored as Vec to maintain order and allow arbitrary button names.
    rects: Vec<(String, Rect)>,
}

impl ButtonRects {
    /// Create a new empty button rects storage.
    pub fn new() -> Self {
        Self { rects: Vec::new() }
    }

    /// Clear all stored rects (call at start of each render).
    pub fn clear(&mut self) {
        self.rects.clear();
    }

    /// Store a button's rendered rectangle.
    pub fn set(&mut self, name: impl Into<String>, rect: Rect) {
        let name = name.into();
        // Update existing or add new
        if let Some(entry) = self.rects.iter_mut().find(|(n, _)| n == &name) {
            entry.1 = rect;
        } else {
            self.rects.push((name, rect));
        }
    }

    /// Check if a click position hits any button.
    /// Returns the name of the clicked button if hit.
    pub fn hit_test(&self, x: u16, y: u16) -> Option<&str> {
        for (name, rect) in &self.rects {
            if x >= rect.x
                && x < rect.x.saturating_add(rect.width)
                && y >= rect.y
                && y < rect.y.saturating_add(rect.height)
            {
                return Some(name.as_str());
            }
        }
        None
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_focus_pane_cycling() {
        let focus = FocusPane::List;
        assert_eq!(focus.next(), FocusPane::Buttons);
        assert_eq!(focus.next().next(), FocusPane::List);
    }

    #[test]
    fn test_button_rects_hit_test() {
        let mut rects = ButtonRects::new();
        rects.set("accept", Rect::new(10, 5, 10, 1));
        rects.set("cancel", Rect::new(25, 5, 8, 1));

        assert_eq!(rects.hit_test(15, 5), Some("accept"));
        assert_eq!(rects.hit_test(28, 5), Some("cancel"));
        assert_eq!(rects.hit_test(0, 0), None);
        assert_eq!(rects.hit_test(22, 5), None); // Between buttons
    }

    #[test]
    fn test_padded_area() {
        let area = Rect::new(0, 0, 100, 50);
        let padded = ResolutionLayout::padded(area);
        assert_eq!(padded.x, 1);
        assert_eq!(padded.y, 1);
        assert_eq!(padded.width, 98);
        assert_eq!(padded.height, 48);
    }
}
