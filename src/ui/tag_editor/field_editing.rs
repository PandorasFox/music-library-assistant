//! Field Editing Operations
//!
//! Methods for manipulating tag fields, buffer operations, and field navigation.
//! These are kept as an `impl UnifiedTagEditorState` block to maintain access to state.

use super::state::UnifiedTagEditorState;
use super::types::{
    AggregatedValue, FieldEditState, TagField, UnifiedTagEditorModal,
};

impl UnifiedTagEditorState {
    // ========================================================================
    // Buffer Operations
    // ========================================================================

    /// Load the current field's values into the editing buffers.
    pub(super) fn load_field_buffer(&mut self) {
        if self.is_aggregated_mode() {
            // Aggregated mode - load from aggregated_fields
            if let Some(ref agg_fields) = self.aggregated_fields {
                if let Some(field) = agg_fields.get(self.current_field_idx) {
                    self.name_buffer = field.name.clone();
                    self.value_buffer = match &field.value {
                        AggregatedValue::Consistent(v) => v.clone(),
                        AggregatedValue::Edited(v) => v.clone(),
                        AggregatedValue::Various | AggregatedValue::VariousConfirming => String::new(),
                    };
                }
            }
        } else {
            // Individual mode - load from tag_fields
            if let Some(fields) = self.tag_fields.get(self.current_item_idx) {
                if let Some(field) = fields.get(self.current_field_idx) {
                    self.name_buffer = field.name.clone();
                    self.value_buffer = field.value.clone();
                }
            }
        }
    }

    /// Commit the editing buffers back to the field data.
    pub(super) fn commit_field_buffer(&mut self) {
        if self.is_aggregated_mode() {
            // Aggregated mode - update aggregated_fields
            if let Some(ref mut agg_fields) = self.aggregated_fields {
                if let Some(field) = agg_fields.get_mut(self.current_field_idx) {
                    match self.field_edit_state {
                        FieldEditState::EditingName => {
                            field.name = self.name_buffer.clone();
                        }
                        FieldEditState::EditingValue => {
                            // Mark as Edited with new value
                            field.value = AggregatedValue::Edited(self.value_buffer.clone());
                        }
                        FieldEditState::NonEditable => {}
                    }
                }
            }
        } else {
            // Individual mode - update tag_fields
            if let Some(fields) = self.tag_fields.get_mut(self.current_item_idx) {
                match self.field_edit_state {
                    FieldEditState::EditingName => {
                        // Get old normalized name before renaming
                        let old_normalized = fields
                            .get(self.current_field_idx)
                            .map(|f| f.name.to_uppercase());

                        if let Some(old_norm) = old_normalized {
                            let new_name = self.name_buffer.clone();
                            // Rename all fields with the same normalized name
                            // (handles multi-value tags: renaming "genre" renames all genre entries)
                            for f in fields.iter_mut() {
                                if f.name.to_uppercase() == old_norm {
                                    f.name = new_name.clone();
                                }
                            }
                        }
                    }
                    FieldEditState::EditingValue => {
                        if let Some(field) = fields.get_mut(self.current_field_idx) {
                            field.value = self.value_buffer.clone();
                        }
                    }
                    FieldEditState::NonEditable => {}
                }
            }
        }
    }

    // ========================================================================
    // Enter Key Handling
    // ========================================================================

    /// Handle Enter key press on a field.
    pub(super) fn handle_field_enter(&mut self) {
        if self.is_aggregated_mode() {
            self.handle_aggregated_field_enter();
        } else {
            self.handle_individual_field_enter();
        }
    }

    fn handle_aggregated_field_enter(&mut self) {
        let field = match self.aggregated_fields.as_ref().and_then(|f| f.get(self.current_field_idx)) {
            Some(f) => f.clone(),
            None => return,
        };

        if self.field_edit_state == FieldEditState::NonEditable {
            if !self.focus_on_value {
                // Name column focused - enter name editing
                self.field_edit_state = FieldEditState::EditingName;
                self.load_field_buffer();
            } else if matches!(field.value, AggregatedValue::Various) {
                // Check if this is a Various value that needs confirmation
                // First Enter - mark as confirming
                if let Some(ref mut agg_fields) = self.aggregated_fields {
                    if let Some(f) = agg_fields.get_mut(self.current_field_idx) {
                        f.value = AggregatedValue::VariousConfirming;
                    }
                }
            } else if matches!(field.value, AggregatedValue::VariousConfirming) {
                // Second Enter - now enter edit mode
                self.field_edit_state = FieldEditState::EditingValue;
                self.load_field_buffer();
            } else {
                // Consistent or Edited value - enter edit mode directly
                self.field_edit_state = FieldEditState::EditingValue;
                self.load_field_buffer();
            }
        } else {
            // Already editing - commit and exit edit mode
            self.commit_field_buffer();
            self.field_edit_state = FieldEditState::NonEditable;
        }
    }

    fn handle_individual_field_enter(&mut self) {
        let fields = match self.tag_fields.get(self.current_item_idx) {
            Some(f) => f,
            None => return,
        };
        let field = match fields.get(self.current_field_idx) {
            Some(f) => f,
            None => return,
        };

        if field.name == "New Tag" {
            self.create_new_tag();
        } else if self.field_edit_state == FieldEditState::NonEditable {
            if !self.focus_on_value {
                // Name column focused - enter name editing (rename = drop old + add new)
                self.field_edit_state = FieldEditState::EditingName;
                self.load_field_buffer();
            } else if self.is_multi_value_field().is_some() {
                // Value column, multi-value tag - open multi-value editor modal
                self.open_multi_value_editor();
            } else {
                // Value column, single value - enter value editing
                self.field_edit_state = FieldEditState::EditingValue;
                self.load_field_buffer();
            }
        } else {
            self.commit_field_buffer();
            self.field_edit_state = FieldEditState::NonEditable;
        }
    }

    // ========================================================================
    // Tag Creation
    // ========================================================================

    pub(super) fn create_new_tag(&mut self) {
        let new_field = TagField {
            name: "new_tag".to_string(),
            value: String::new(),
            editable: true,
            deleted: false,
        };

        if let Some(fields) = self.tag_fields.get_mut(self.current_item_idx) {
            let insert_pos = fields.len().saturating_sub(1);
            fields.insert(insert_pos, new_field);
            self.current_field_idx = insert_pos;
            // Scroll to show the new tag
            if self.field_visible_height > 0 {
                let visible_end = self.field_scroll_offset + self.field_visible_height.saturating_sub(1);
                if self.current_field_idx >= visible_end {
                    self.field_scroll_offset = self.current_field_idx.saturating_sub(self.field_visible_height.saturating_sub(2));
                }
            }
        }

        self.field_edit_state = FieldEditState::EditingName;
        self.name_buffer = "new_tag".to_string();
        self.value_buffer.clear();
        self.focus_on_value = false;
    }

    // ========================================================================
    // Character Manipulation
    // ========================================================================

    /// Insert a character into the active buffer.
    pub(super) fn insert_char(&mut self, c: char) {
        match self.field_edit_state {
            FieldEditState::EditingName => {
                self.name_buffer.push(c);
            }
            FieldEditState::EditingValue => {
                self.value_buffer.push(c);
            }
            FieldEditState::NonEditable => {}
        }
    }

    /// Delete the last character from the active buffer.
    pub(super) fn delete_char(&mut self) {
        match self.field_edit_state {
            FieldEditState::EditingName => {
                self.name_buffer.pop();
            }
            FieldEditState::EditingValue => {
                self.value_buffer.pop();
            }
            FieldEditState::NonEditable => {}
        }
    }

    /// Clear the current field (buffer or actual field value).
    pub(super) fn clear_current_field(&mut self) {
        match self.field_edit_state {
            FieldEditState::EditingName => {
                self.name_buffer.clear();
            }
            FieldEditState::EditingValue => {
                self.value_buffer.clear();
            }
            FieldEditState::NonEditable => {
                if let Some(fields) = self.tag_fields.get_mut(self.current_item_idx) {
                    if let Some(field) = fields.get_mut(self.current_field_idx) {
                        field.value.clear();
                    }
                }
            }
        }
    }

    /// Toggle the deleted flag on the current field.
    pub(super) fn toggle_current_field_deleted(&mut self) {
        if let Some(fields) = self.tag_fields.get_mut(self.current_item_idx) {
            if let Some(field) = fields.get_mut(self.current_field_idx) {
                // Only allow deletion toggle on editable fields that aren't the "New Tag" placeholder
                if field.editable && field.name != "New Tag" {
                    field.deleted = !field.deleted;
                }
            }
        }
    }

    // ========================================================================
    // Multi-Value Field Support
    // ========================================================================

    /// Check if current field is part of a multi-value group and get the count.
    pub(super) fn is_multi_value_field(&self) -> Option<usize> {
        let fields = self.tag_fields.get(self.current_item_idx)?;
        let current_field = fields.get(self.current_field_idx)?;
        let normalized_name = current_field.name.to_uppercase();

        // Count fields with same normalized name
        let count = fields
            .iter()
            .filter(|f| f.name.to_uppercase() == normalized_name && f.name != "New Tag")
            .count();

        if count > 1 {
            Some(count)
        } else {
            None
        }
    }

    /// Open multi-value editor modal for current field.
    pub(super) fn open_multi_value_editor(&mut self) {
        let fields = match self.tag_fields.get(self.current_item_idx) {
            Some(f) => f,
            None => return,
        };
        let current_field = match fields.get(self.current_field_idx) {
            Some(f) => f,
            None => return,
        };

        let normalized_name = current_field.name.to_uppercase();
        let values: Vec<String> = fields
            .iter()
            .filter(|f| f.name.to_uppercase() == normalized_name)
            .map(|f| f.value.clone())
            .collect();

        self.modal = Some(UnifiedTagEditorModal::MultiValueEditor {
            field_idx: self.current_field_idx,
            values,
            current_value_idx: 0,
            editing: false,
            edit_buffer: String::new(),
        });
    }

    /// Apply multi-value changes back to the underlying tag_fields.
    pub(super) fn apply_multi_value_changes(&mut self, field_idx: usize, new_values: Vec<String>) {
        // Get the field name and determine which fields need updating
        let field_name = match self
            .tag_fields
            .get(self.current_item_idx)
            .and_then(|fields| fields.get(field_idx))
        {
            Some(f) => f.name.clone(),
            None => return,
        };

        let fields = match self.tag_fields.get_mut(self.current_item_idx) {
            Some(f) => f,
            None => return,
        };

        // Find all fields with the same normalized name
        let normalized_name = field_name.to_uppercase();
        let matching_indices: Vec<usize> = fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.name.to_uppercase() == normalized_name)
            .map(|(i, _)| i)
            .collect();

        // Remove existing fields with this name (in reverse order to preserve indices)
        for idx in matching_indices.into_iter().rev() {
            fields.remove(idx);
        }

        // Insert new values at the original position
        let insert_pos = field_idx.min(fields.len());
        for (i, value) in new_values.into_iter().enumerate() {
            fields.insert(
                insert_pos + i,
                TagField {
                    name: field_name.clone(),
                    value,
                    editable: true,
                    deleted: false,
                },
            );
        }
    }
}
