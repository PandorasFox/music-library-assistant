//! Bulk Selection State
//!
//! Centralized selection management for file listings.

use std::collections::HashSet;

/// Selection mode for bulk operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionMode {
    /// No multi-select active (single cursor navigation)
    #[default]
    None,
    /// Multi-select active (Space toggles individual items)
    Active,
}

/// Centralized selection state for file listings.
///
/// This is designed to work with any list that has indexable items.
/// Resolution flows store indices; the actual items are looked up from
/// the owning state's file list.
#[derive(Debug, Clone, Default)]
pub struct BulkSelectionState {
    /// Set of selected indices
    selected_indices: HashSet<usize>,
    /// Current selection mode
    mode: SelectionMode,
}

impl BulkSelectionState {
    /// Create a new empty selection state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if selection mode is active.
    pub fn is_active(&self) -> bool {
        self.mode == SelectionMode::Active
    }

    /// Get count of selected items.
    pub fn selection_count(&self) -> usize {
        self.selected_indices.len()
    }

    /// Check if an index is selected.
    pub fn is_selected(&self, idx: usize) -> bool {
        self.selected_indices.contains(&idx)
    }

    /// Toggle selection of an index.
    ///
    /// Automatically activates selection mode when first item is selected.
    pub fn toggle(&mut self, idx: usize) {
        if self.selected_indices.contains(&idx) {
            self.selected_indices.remove(&idx);
        } else {
            self.selected_indices.insert(idx);
        }
        // Activate selection mode when first item is toggled
        if !self.selected_indices.is_empty() {
            self.mode = SelectionMode::Active;
        } else {
            self.mode = SelectionMode::None;
        }
    }

    /// Toggle select all / deselect all for filtered view.
    ///
    /// If all filtered items are selected, deselects all filtered items.
    /// Otherwise, selects all filtered items.
    pub fn toggle_all_filtered(&mut self, filtered_indices: &[usize]) {
        if filtered_indices.is_empty() {
            return;
        }

        let all_selected = filtered_indices
            .iter()
            .all(|idx| self.selected_indices.contains(idx));

        if all_selected {
            // Deselect only filtered items
            for idx in filtered_indices {
                self.selected_indices.remove(idx);
            }
        } else {
            // Select all filtered items
            self.selected_indices
                .extend(filtered_indices.iter().copied());
        }

        // Update mode based on result
        if self.selected_indices.is_empty() {
            self.mode = SelectionMode::None;
        } else {
            self.mode = SelectionMode::Active;
        }
    }

    /// Select all items up to the given count.
    ///
    /// Activates selection mode and selects indices 0..count.
    pub fn select_all(&mut self, count: usize) {
        self.selected_indices = (0..count).collect();
        self.mode = if count > 0 {
            SelectionMode::Active
        } else {
            SelectionMode::None
        };
    }

    /// Get all selected indices as a sorted vector.
    pub fn selected_indices(&self) -> Vec<usize> {
        let mut indices: Vec<_> = self.selected_indices.iter().copied().collect();
        indices.sort_unstable();
        indices
    }

    /// Selection marker for rendering.
    ///
    /// Returns "[x]" if selected, "[ ]" if not.
    pub fn marker(&self, idx: usize) -> &'static str {
        if self.is_selected(idx) {
            "[x]"
        } else {
            "[ ]"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toggle_selection() {
        let mut state = BulkSelectionState::new();
        assert!(!state.is_active());
        assert_eq!(state.selection_count(), 0);

        state.toggle(5);
        assert!(state.is_active());
        assert_eq!(state.selection_count(), 1);
        assert!(state.is_selected(5));

        state.toggle(5);
        assert!(!state.is_active());
        assert_eq!(state.selection_count(), 0);
        assert!(!state.is_selected(5));
    }

    #[test]
    fn test_toggle_all_filtered() {
        let mut state = BulkSelectionState::new();
        let filtered = vec![1, 3, 5];

        // Select all
        state.toggle_all_filtered(&filtered);
        assert!(state.is_selected(1));
        assert!(state.is_selected(3));
        assert!(state.is_selected(5));
        assert!(!state.is_selected(2));
        assert_eq!(state.selection_count(), 3);

        // Toggle again should deselect all
        state.toggle_all_filtered(&filtered);
        assert!(!state.is_selected(1));
        assert!(!state.is_selected(3));
        assert!(!state.is_selected(5));
        assert_eq!(state.selection_count(), 0);
    }

    #[test]
    fn test_marker() {
        let mut state = BulkSelectionState::new();
        assert_eq!(state.marker(0), "[ ]");

        state.toggle(0);
        assert_eq!(state.marker(0), "[x]");
        assert_eq!(state.marker(1), "[ ]");
    }
}
