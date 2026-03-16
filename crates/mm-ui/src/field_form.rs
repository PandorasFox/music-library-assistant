//! FieldForm: widget state for editing ordered key-value entries.
//!
//! Analogous to StandardList but for form fields rather than read-only items.
//! Operates directly on a `TagSet` — edits are immediate mutations.

use crate::input::InputAction;
use crate::tag_set::TagSet;
use crate::text_input::TextInputState;

// ============================================================================
// FieldColumn
// ============================================================================

/// Which column has focus during navigation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FieldColumn {
    Name,
    #[default]
    Value,
}

// ============================================================================
// FieldEditMode
// ============================================================================

/// What the field form is currently doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FieldEditMode {
    /// Browsing entries — arrow keys navigate, Enter starts editing.
    #[default]
    Navigating,
    /// Editing the tag name.
    EditingName,
    /// Editing a tag value.
    EditingValue,
}

// ============================================================================
// FieldFormResult
// ============================================================================

/// Result of handling an input action in the field form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldFormResult {
    /// Input consumed (scroll, internal state change).
    Consumed,
    /// Cursor moved to a different entry.
    CursorMoved,
    /// Entry/value was modified (name change, value edit, add, delete).
    Modified,
    /// Input not handled — bubble up.
    Unhandled,
}

// ============================================================================
// MultiValueExpansion
// ============================================================================

/// State for expanded multi-value view (Z on a multi-value entry).
#[derive(Debug, Clone)]
pub struct MultiValueExpansion {
    /// Which entry is expanded.
    pub entry_idx: usize,
    /// Cursor within the values list (includes "+ Add value" sentinel).
    pub value_cursor: usize,
    /// Whether currently editing a value in the expansion.
    pub editing: bool,
}

// ============================================================================
// FieldFormState
// ============================================================================

/// State for a key-value field editor form.
#[derive(Debug, Clone)]
pub struct FieldFormState {
    /// Which entry the cursor is on.
    pub cursor: usize,
    /// Scroll offset for the entry list.
    pub scroll_offset: usize,
    /// Visible height (set during render).
    pub visible_height: usize,
    /// Current editing mode.
    pub edit_mode: FieldEditMode,
    /// Which column has focus during navigation mode.
    pub active_column: FieldColumn,
    /// Text input for editing tag names.
    pub name_input: TextInputState,
    /// Text input for editing tag values.
    pub value_input: TextInputState,
    /// Multi-value expansion state (Z key).
    pub expansion: Option<MultiValueExpansion>,
}

impl Default for FieldFormState {
    fn default() -> Self {
        Self::new()
    }
}

impl FieldFormState {
    pub fn new() -> Self {
        Self {
            cursor: 0,
            scroll_offset: 0,
            visible_height: 10,
            edit_mode: FieldEditMode::Navigating,
            active_column: FieldColumn::default(),
            name_input: TextInputState::new(),
            value_input: TextInputState::new(),
            expansion: None,
        }
    }

    /// Handle input, mutating the TagSet directly.
    pub fn handle_input(
        &mut self,
        action: &InputAction,
        tag_set: &mut TagSet,
    ) -> FieldFormResult {
        // If we're in an expanded multi-value view, delegate there
        if self.expansion.is_some() {
            return self.handle_expansion_input(action, tag_set);
        }

        match self.edit_mode {
            FieldEditMode::Navigating => self.handle_navigating(action, tag_set),
            FieldEditMode::EditingName => self.handle_editing_name(action, tag_set),
            FieldEditMode::EditingValue => self.handle_editing_value(action, tag_set),
        }
    }

    /// Whether the form is actively editing (name or value).
    pub fn is_editing(&self) -> bool {
        self.edit_mode != FieldEditMode::Navigating
    }

    /// Clamp cursor to valid range for the given tag set.
    pub fn clamp_cursor(&mut self, tag_set: &TagSet) {
        let len = tag_set.entry_count();
        if len == 0 {
            self.cursor = 0;
        } else if self.cursor >= len {
            self.cursor = len - 1;
        }
    }

    /// Reset to navigating mode at position 0.
    pub fn reset(&mut self) {
        self.cursor = 0;
        self.scroll_offset = 0;
        self.edit_mode = FieldEditMode::Navigating;
        self.active_column = FieldColumn::default();
        self.expansion = None;
    }

    // ========================================================================
    // Navigating mode
    // ========================================================================

    fn handle_navigating(
        &mut self,
        action: &InputAction,
        tag_set: &mut TagSet,
    ) -> FieldFormResult {
        let entry_count = tag_set.entry_count();

        match action {
            InputAction::NavUp => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.scroll_to_cursor();
                    FieldFormResult::CursorMoved
                } else {
                    FieldFormResult::Consumed
                }
            }
            InputAction::NavDown => {
                // Allow cursor to go to entry_count (the "New Tag" sentinel position)
                if self.cursor < entry_count {
                    self.cursor += 1;
                    self.scroll_to_cursor();
                    FieldFormResult::CursorMoved
                } else {
                    FieldFormResult::Unhandled
                }
            }
            InputAction::NavLeft => {
                if self.active_column == FieldColumn::Value {
                    self.active_column = FieldColumn::Name;
                    FieldFormResult::Consumed
                } else {
                    FieldFormResult::Unhandled
                }
            }
            InputAction::NavRight => {
                if self.active_column == FieldColumn::Name {
                    self.active_column = FieldColumn::Value;
                    FieldFormResult::Consumed
                } else {
                    FieldFormResult::Unhandled
                }
            }
            InputAction::Confirm => {
                if self.cursor == entry_count {
                    // "New Tag" sentinel — create new entry and start editing name
                    tag_set.add_entry("new_tag".into(), String::new());
                    self.cursor = entry_count; // now points at the new entry
                    self.name_input.set_value("new_tag");
                    self.value_input.clear();
                    self.edit_mode = FieldEditMode::EditingName;
                    self.scroll_to_cursor();
                    FieldFormResult::Modified
                } else if let Some(entry) = tag_set.get(self.cursor) {
                    // Start editing the active column
                    let first_value = entry.values.first().map(|s| s.as_str()).unwrap_or("");
                    self.value_input.set_value(first_value);
                    self.name_input.set_value(&entry.name);
                    match self.active_column {
                        FieldColumn::Name => {
                            self.edit_mode = FieldEditMode::EditingName;
                        }
                        FieldColumn::Value => {
                            self.edit_mode = FieldEditMode::EditingValue;
                        }
                    }
                    FieldFormResult::Consumed
                } else {
                    FieldFormResult::Consumed
                }
            }
            InputAction::Delete | InputAction::Backspace => {
                if self.cursor < entry_count {
                    tag_set.drop_entry(self.cursor);
                    self.clamp_cursor(tag_set);
                    FieldFormResult::Modified
                } else {
                    FieldFormResult::Consumed
                }
            }
            InputAction::Char('n') => {
                // Quick-create new tag
                tag_set.add_entry("new_tag".into(), String::new());
                self.cursor = tag_set.entry_count() - 1;
                self.name_input.set_value("new_tag");
                self.value_input.clear();
                self.edit_mode = FieldEditMode::EditingName;
                self.scroll_to_cursor();
                FieldFormResult::Modified
            }
            InputAction::Char('z') | InputAction::Char('Z') => {
                // Expand multi-value entry
                if self.cursor < entry_count {
                    if let Some(entry) = tag_set.get(self.cursor) {
                        if entry.values.len() > 1 {
                            self.expansion = Some(MultiValueExpansion {
                                entry_idx: self.cursor,
                                value_cursor: 0,
                                editing: false,
                            });
                            return FieldFormResult::Consumed;
                        }
                    }
                }
                FieldFormResult::Consumed
            }
            _ => FieldFormResult::Unhandled,
        }
    }

    // ========================================================================
    // Editing name mode
    // ========================================================================

    fn handle_editing_name(
        &mut self,
        action: &InputAction,
        tag_set: &mut TagSet,
    ) -> FieldFormResult {
        match action {
            InputAction::Confirm => {
                // Commit name edit
                let new_name = self.name_input.value().to_string();
                tag_set.rename_entry(self.cursor, new_name);
                self.edit_mode = FieldEditMode::Navigating;
                FieldFormResult::Modified
            }
            InputAction::Cancel => {
                // Cancel name edit
                self.edit_mode = FieldEditMode::Navigating;
                FieldFormResult::Consumed
            }
            InputAction::CycleNext => {
                // Tab: switch to editing value
                let new_name = self.name_input.value().to_string();
                tag_set.rename_entry(self.cursor, new_name);
                if let Some(entry) = tag_set.get(self.cursor) {
                    let first_value = entry.values.first().map(|s| s.as_str()).unwrap_or("");
                    self.value_input.set_value(first_value);
                }
                self.edit_mode = FieldEditMode::EditingValue;
                FieldFormResult::Consumed
            }
            _ => {
                // Delegate text editing to name_input
                if self.name_input.handle_input(action) {
                    FieldFormResult::Consumed
                } else {
                    FieldFormResult::Unhandled
                }
            }
        }
    }

    // ========================================================================
    // Editing value mode
    // ========================================================================

    fn handle_editing_value(
        &mut self,
        action: &InputAction,
        tag_set: &mut TagSet,
    ) -> FieldFormResult {
        match action {
            InputAction::Confirm => {
                // Commit value edit
                let new_value = self.value_input.value().to_string();
                tag_set.set_value(self.cursor, 0, new_value);
                self.edit_mode = FieldEditMode::Navigating;
                FieldFormResult::Modified
            }
            InputAction::Cancel => {
                // Cancel value edit
                self.edit_mode = FieldEditMode::Navigating;
                FieldFormResult::Consumed
            }
            InputAction::CycleNext => {
                // Tab: switch to editing name
                let new_value = self.value_input.value().to_string();
                tag_set.set_value(self.cursor, 0, new_value);
                if let Some(entry) = tag_set.get(self.cursor) {
                    self.name_input.set_value(&entry.name);
                }
                self.edit_mode = FieldEditMode::EditingName;
                FieldFormResult::Consumed
            }
            _ => {
                // Delegate text editing to value_input
                if self.value_input.handle_input(action) {
                    FieldFormResult::Consumed
                } else {
                    FieldFormResult::Unhandled
                }
            }
        }
    }

    // ========================================================================
    // Multi-value expansion
    // ========================================================================

    fn handle_expansion_input(
        &mut self,
        action: &InputAction,
        tag_set: &mut TagSet,
    ) -> FieldFormResult {
        let expansion = self.expansion.as_mut().unwrap();
        let entry_idx = expansion.entry_idx;

        let value_count = tag_set
            .get(entry_idx)
            .map(|e| e.values.len())
            .unwrap_or(0);
        let add_sentinel = value_count; // index of "+ Add value"

        if expansion.editing {
            // Editing a value within the expansion
            match action {
                InputAction::Confirm => {
                    let new_value = self.value_input.value().to_string();
                    if expansion.value_cursor < value_count {
                        tag_set.set_value(entry_idx, expansion.value_cursor, new_value);
                    } else if expansion.value_cursor == add_sentinel && !new_value.is_empty() {
                        tag_set.add_value(entry_idx, new_value);
                    }
                    expansion.editing = false;
                    self.value_input.clear();
                    FieldFormResult::Modified
                }
                InputAction::Cancel => {
                    expansion.editing = false;
                    self.value_input.clear();
                    FieldFormResult::Consumed
                }
                _ => {
                    if self.value_input.handle_input(action) {
                        FieldFormResult::Consumed
                    } else {
                        FieldFormResult::Unhandled
                    }
                }
            }
        } else {
            // Navigating within the expansion
            match action {
                InputAction::NavUp => {
                    if expansion.value_cursor > 0 {
                        expansion.value_cursor -= 1;
                    }
                    FieldFormResult::Consumed
                }
                InputAction::NavDown => {
                    if expansion.value_cursor < add_sentinel {
                        expansion.value_cursor += 1;
                    }
                    FieldFormResult::Consumed
                }
                InputAction::Confirm => {
                    if expansion.value_cursor < value_count {
                        // Edit existing value
                        if let Some(entry) = tag_set.get(entry_idx) {
                            if let Some(val) = entry.values.get(expansion.value_cursor) {
                                self.value_input.set_value(val);
                            }
                        }
                        expansion.editing = true;
                    } else if expansion.value_cursor == add_sentinel {
                        // Add new value
                        self.value_input.clear();
                        expansion.editing = true;
                    }
                    FieldFormResult::Consumed
                }
                InputAction::Delete | InputAction::Backspace => {
                    if expansion.value_cursor < value_count {
                        tag_set.drop_value(entry_idx, expansion.value_cursor);
                        // Check if entry was removed (last value dropped)
                        if tag_set.get(entry_idx).is_none()
                            || tag_set.get(entry_idx).map(|e| e.values.is_empty()).unwrap_or(true)
                        {
                            self.expansion = None;
                            self.clamp_cursor(tag_set);
                        } else if expansion.value_cursor >= tag_set.get(entry_idx).unwrap().values.len()
                        {
                            expansion.value_cursor =
                                tag_set.get(entry_idx).unwrap().values.len().saturating_sub(1);
                        }
                        FieldFormResult::Modified
                    } else {
                        FieldFormResult::Consumed
                    }
                }
                InputAction::Cancel => {
                    // Close expansion
                    self.expansion = None;
                    FieldFormResult::Consumed
                }
                _ => FieldFormResult::Unhandled,
            }
        }
    }

    // ========================================================================
    // Scroll helpers
    // ========================================================================

    fn scroll_to_cursor(&mut self) {
        if self.visible_height == 0 {
            return;
        }
        if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        }
        let visible_end = self.scroll_offset + self.visible_height.saturating_sub(1);
        if self.cursor >= visible_end {
            self.scroll_offset = self
                .cursor
                .saturating_sub(self.visible_height.saturating_sub(2));
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tag_set() -> TagSet {
        TagSet::from_pairs(vec![
            ("ARTIST".into(), "Bach".into()),
            ("TITLE".into(), "Fugue".into()),
            ("ALBUM".into(), "WTC".into()),
        ])
    }

    #[test]
    fn navigate_up_down() {
        let mut form = FieldFormState::new();
        let mut ts = make_tag_set();

        assert_eq!(form.cursor, 0);

        let r = form.handle_input(&InputAction::NavDown, &mut ts);
        assert_eq!(r, FieldFormResult::CursorMoved);
        assert_eq!(form.cursor, 1);

        let r = form.handle_input(&InputAction::NavDown, &mut ts);
        assert_eq!(r, FieldFormResult::CursorMoved);
        assert_eq!(form.cursor, 2);

        // Can go to sentinel (entry_count = 3, cursor = 3)
        let r = form.handle_input(&InputAction::NavDown, &mut ts);
        assert_eq!(r, FieldFormResult::CursorMoved);
        assert_eq!(form.cursor, 3);

        // Can't go past sentinel — returns Unhandled so caller can move focus
        let r = form.handle_input(&InputAction::NavDown, &mut ts);
        assert_eq!(r, FieldFormResult::Unhandled);
        assert_eq!(form.cursor, 3);

        let r = form.handle_input(&InputAction::NavUp, &mut ts);
        assert_eq!(r, FieldFormResult::CursorMoved);
        assert_eq!(form.cursor, 2);
    }

    #[test]
    fn enter_edit_value_then_commit() {
        let mut form = FieldFormState::new();
        let mut ts = make_tag_set();

        // Enter on first entry starts editing value
        let r = form.handle_input(&InputAction::Confirm, &mut ts);
        assert_eq!(r, FieldFormResult::Consumed);
        assert_eq!(form.edit_mode, FieldEditMode::EditingValue);

        // Type a new value
        form.value_input.clear();
        form.value_input.set_value("Handel");

        // Confirm commits
        let r = form.handle_input(&InputAction::Confirm, &mut ts);
        assert_eq!(r, FieldFormResult::Modified);
        assert_eq!(form.edit_mode, FieldEditMode::Navigating);
        assert_eq!(ts.entries()[0].values[0], "Handel");
    }

    #[test]
    fn cancel_edit_reverts() {
        let mut form = FieldFormState::new();
        let mut ts = make_tag_set();

        form.handle_input(&InputAction::Confirm, &mut ts);
        assert_eq!(form.edit_mode, FieldEditMode::EditingValue);

        form.value_input.clear();
        form.value_input.set_value("WRONG");

        // Cancel does NOT commit
        let r = form.handle_input(&InputAction::Cancel, &mut ts);
        assert_eq!(r, FieldFormResult::Consumed);
        assert_eq!(form.edit_mode, FieldEditMode::Navigating);
        assert_eq!(ts.entries()[0].values[0], "Bach"); // unchanged
    }

    #[test]
    fn tab_toggles_name_value() {
        let mut form = FieldFormState::new();
        let mut ts = make_tag_set();

        // Start editing value
        form.handle_input(&InputAction::Confirm, &mut ts);
        assert_eq!(form.edit_mode, FieldEditMode::EditingValue);

        // Tab switches to name editing (commits value first)
        let r = form.handle_input(&InputAction::CycleNext, &mut ts);
        assert_eq!(r, FieldFormResult::Consumed);
        assert_eq!(form.edit_mode, FieldEditMode::EditingName);

        // Tab again switches back to value editing (commits name first)
        let r = form.handle_input(&InputAction::CycleNext, &mut ts);
        assert_eq!(r, FieldFormResult::Consumed);
        assert_eq!(form.edit_mode, FieldEditMode::EditingValue);
    }

    #[test]
    fn delete_entry() {
        let mut form = FieldFormState::new();
        let mut ts = make_tag_set();
        assert_eq!(ts.entry_count(), 3);

        let r = form.handle_input(&InputAction::Delete, &mut ts);
        assert_eq!(r, FieldFormResult::Modified);
        assert_eq!(ts.entry_count(), 2);
        // ARTIST was removed, TITLE is now first
        assert_eq!(ts.entries()[0].name, "TITLE");
    }

    #[test]
    fn new_tag_sentinel() {
        let mut form = FieldFormState::new();
        let mut ts = make_tag_set();

        // Navigate to sentinel
        form.cursor = ts.entry_count(); // = 3

        // Enter on sentinel creates new entry
        let r = form.handle_input(&InputAction::Confirm, &mut ts);
        assert_eq!(r, FieldFormResult::Modified);
        assert_eq!(ts.entry_count(), 4);
        assert_eq!(form.edit_mode, FieldEditMode::EditingName);
    }

    #[test]
    fn n_key_creates_tag() {
        let mut form = FieldFormState::new();
        let mut ts = make_tag_set();

        let r = form.handle_input(&InputAction::Char('n'), &mut ts);
        assert_eq!(r, FieldFormResult::Modified);
        assert_eq!(ts.entry_count(), 4);
        assert_eq!(form.edit_mode, FieldEditMode::EditingName);
    }

    #[test]
    fn expansion_navigate_and_edit() {
        let mut ts = TagSet::from_pairs(vec![
            ("GENRE".into(), "Classical".into()),
            ("GENRE".into(), "Baroque".into()),
        ]);
        let mut form = FieldFormState::new();

        // Z expands multi-value
        let r = form.handle_input(&InputAction::Char('z'), &mut ts);
        assert_eq!(r, FieldFormResult::Consumed);
        assert!(form.expansion.is_some());

        // Navigate down
        form.handle_input(&InputAction::NavDown, &mut ts);
        assert_eq!(form.expansion.as_ref().unwrap().value_cursor, 1);

        // Enter to edit
        form.handle_input(&InputAction::Confirm, &mut ts);
        assert!(form.expansion.as_ref().unwrap().editing);

        // Type new value
        form.value_input.clear();
        form.value_input.set_value("Chamber");

        // Confirm commits
        let r = form.handle_input(&InputAction::Confirm, &mut ts);
        assert_eq!(r, FieldFormResult::Modified);
        assert_eq!(ts.entries()[0].values[1], "Chamber");
        assert!(!form.expansion.as_ref().unwrap().editing);

        // Escape closes expansion
        form.handle_input(&InputAction::Cancel, &mut ts);
        assert!(form.expansion.is_none());
    }
}
