//! Geometry primitives: focus panes, button rects, hit-testing.

use ratatui::layout::Rect;

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
            if rect_contains(*rect, x, y) {
                return Some(name.as_str());
            }
        }
        None
    }
}

/// Check if a point (x, y) falls within a Rect's bounds.
pub fn rect_contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x
        && x < rect.x.saturating_add(rect.width)
        && y >= rect.y
        && y < rect.y.saturating_add(rect.height)
}

/// Apply 1-cell padding to an area (for full-area modals).
pub fn padded_rect(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
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
        let padded = padded_rect(area);
        assert_eq!(padded.x, 1);
        assert_eq!(padded.y, 1);
        assert_eq!(padded.width, 98);
        assert_eq!(padded.height, 48);
    }
}
