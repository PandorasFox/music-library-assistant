//! Geometry primitives: focus panes, button rects, hit-testing.

use ratatui::layout::Rect;

/// Focus pane for resolution modals.
///
/// Spatial order top-to-bottom: Field → List → Buttons.
/// Details pane is not focusable (informational only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPane {
    /// Decision text field (above list, only present in some modals)
    Field,
    /// Item list pane (default)
    #[default]
    List,
    /// Decision buttons bar
    Buttons,
}

impl FocusPane {
    /// Cycle to next focus pane downward (Shift+Down).
    ///
    /// Order: Field → List → Buttons → wrap.
    /// When `has_field` is false, Field is skipped.
    pub fn next(self, has_field: bool) -> Self {
        match self {
            Self::Field => Self::List,
            Self::List => Self::Buttons,
            Self::Buttons => if has_field { Self::Field } else { Self::List },
        }
    }

    /// Cycle to previous focus pane upward (Shift+Up).
    ///
    /// Order: Buttons → List → Field → wrap.
    /// When `has_field` is false, Field is skipped.
    pub fn prev(self, has_field: bool) -> Self {
        match self {
            Self::Field => Self::Buttons,
            Self::List => if has_field { Self::Field } else { Self::Buttons },
            Self::Buttons => Self::List,
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
    fn focus_cycling_without_field() {
        let focus = FocusPane::List;
        assert_eq!(focus.next(false), FocusPane::Buttons);
        assert_eq!(focus.next(false).next(false), FocusPane::List);
        assert_eq!(focus.prev(false), FocusPane::Buttons);
    }

    #[test]
    fn focus_cycling_with_field() {
        // Forward: Field → List → Buttons → Field
        assert_eq!(FocusPane::Field.next(true), FocusPane::List);
        assert_eq!(FocusPane::List.next(true), FocusPane::Buttons);
        assert_eq!(FocusPane::Buttons.next(true), FocusPane::Field);

        // Backward: Buttons → List → Field → Buttons
        assert_eq!(FocusPane::Buttons.prev(true), FocusPane::List);
        assert_eq!(FocusPane::List.prev(true), FocusPane::Field);
        assert_eq!(FocusPane::Field.prev(true), FocusPane::Buttons);
    }

    #[test]
    fn focus_cycling_skips_field_when_absent() {
        // Forward: List → Buttons → List (no Field)
        assert_eq!(FocusPane::List.next(false), FocusPane::Buttons);
        assert_eq!(FocusPane::Buttons.next(false), FocusPane::List);

        // Backward: List → Buttons → List (no Field)
        assert_eq!(FocusPane::List.prev(false), FocusPane::Buttons);
        assert_eq!(FocusPane::Buttons.prev(false), FocusPane::List);
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
