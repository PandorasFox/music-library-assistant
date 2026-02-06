//! List Click Targets Widget
//!
//! Row-based click targets for list items. Tracks Y positions of list items
//! for click detection, enabling mouse-based selection in scrollable lists.

use ratatui::layout::Rect;

/// Row-based click targets for list items.
/// Tracks Y positions of list items for click detection.
#[derive(Debug, Clone, Default)]
pub struct ListClickTargets {
    /// The area containing the list (used for bounds checking)
    list_area: Option<Rect>,
    /// (identifier, y_position) - each item is 1 row tall
    rows: Vec<(String, u16)>,
}

impl ListClickTargets {
    /// Create a new empty click targets storage.
    pub fn new() -> Self {
        Self {
            list_area: None,
            rows: Vec::new(),
        }
    }

    /// Clear all stored targets (call at start of each render).
    pub fn clear(&mut self) {
        self.list_area = None;
        self.rows.clear();
    }

    /// Set the list area for bounds checking.
    pub fn set_list_area(&mut self, area: Rect) {
        self.list_area = Some(area);
    }

    /// Add a row target at the given Y position.
    pub fn add_row(&mut self, id: impl Into<String>, y: u16) {
        self.rows.push((id.into(), y));
    }

    /// Check if a click position hits any row.
    /// Returns the identifier of the clicked row if hit.
    pub fn hit_test(&self, x: u16, y: u16) -> Option<&str> {
        // First check if click is within the list area
        if let Some(area) = self.list_area {
            if x < area.x
                || x >= area.x.saturating_add(area.width)
                || y < area.y
                || y >= area.y.saturating_add(area.height)
            {
                return None;
            }
        }

        // Find the row at this Y position
        for (id, row_y) in &self.rows {
            if y == *row_y {
                return Some(id.as_str());
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_targets() {
        let targets = ListClickTargets::new();
        assert_eq!(targets.hit_test(10, 5), None);
    }

    #[test]
    fn test_hit_test_basic() {
        let mut targets = ListClickTargets::new();
        targets.set_list_area(Rect::new(5, 5, 30, 10));
        targets.add_row("item_0", 6);
        targets.add_row("item_1", 7);
        targets.add_row("item_2", 8);

        // Hit first row
        assert_eq!(targets.hit_test(10, 6), Some("item_0"));
        // Hit second row
        assert_eq!(targets.hit_test(15, 7), Some("item_1"));
        // Hit third row
        assert_eq!(targets.hit_test(20, 8), Some("item_2"));
        // Miss (between rows - not possible with single-height rows)
        assert_eq!(targets.hit_test(10, 9), None);
    }

    #[test]
    fn test_hit_test_outside_area() {
        let mut targets = ListClickTargets::new();
        targets.set_list_area(Rect::new(10, 10, 20, 5));
        targets.add_row("item_0", 11);

        // Click outside list area (left)
        assert_eq!(targets.hit_test(5, 11), None);
        // Click outside list area (above)
        assert_eq!(targets.hit_test(15, 8), None);
        // Click outside list area (below)
        assert_eq!(targets.hit_test(15, 16), None);
        // Click inside area on correct row
        assert_eq!(targets.hit_test(15, 11), Some("item_0"));
    }

    #[test]
    fn test_clear() {
        let mut targets = ListClickTargets::new();
        targets.set_list_area(Rect::new(0, 0, 100, 100));
        targets.add_row("item_0", 5);

        assert_eq!(targets.hit_test(10, 5), Some("item_0"));

        targets.clear();
        assert_eq!(targets.hit_test(10, 5), None);
    }
}
