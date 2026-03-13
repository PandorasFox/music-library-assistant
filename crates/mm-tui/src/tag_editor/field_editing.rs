//! Field Editing Operations
//!
//! Methods for manipulating tag fields, buffer operations, and field navigation.
//! These are kept as an `impl UnifiedTagEditorState` block to maintain access to state.

use super::state::UnifiedTagEditorState;
use super::types::{
    AggregatedTagField, AggregatedValue, FieldEditState, TagField, UnifiedTagEditorModal,
};

impl UnifiedTagEditorState {
    // ========================================================================
    // Buffer Operations
    // ========================================================================

    /// Load the current field's values into the editing input states.
    pub(super) fn load_field_buffer(&mut self) {
        if self.is_aggregated_mode() {
            // Aggregated mode - load from aggregated_fields
            if let Some(ref agg_fields) = self.aggregated_fields {
                if let Some(field) = agg_fields.get(self.current_field_idx) {
                    self.name_input.set_value(&field.name);
                    let val = match &field.value {
                        AggregatedValue::Consistent(v) => v.as_str(),
                        AggregatedValue::Edited(v) => v.as_str(),
                        AggregatedValue::Various | AggregatedValue::VariousConfirming => "",
                    };
                    self.value_input.set_value(val);
                }
            }
        } else {
            // Individual mode - load from tag_fields
            if let Some(fields) = self.tag_fields.get(self.current_item_idx) {
                if let Some(field) = fields.get(self.current_field_idx) {
                    self.name_input.set_value(&field.name);
                    self.value_input.set_value(&field.value);
                }
            }
        }
    }

    /// Commit the editing input states back to the field data.
    pub(super) fn commit_field_buffer(&mut self) {
        if self.is_aggregated_mode() {
            // Aggregated mode - update aggregated_fields
            if let Some(ref mut agg_fields) = self.aggregated_fields {
                if let Some(field) = agg_fields.get_mut(self.current_field_idx) {
                    match self.field_edit_state {
                        FieldEditState::EditingName => {
                            field.name = self.name_input.value().to_string();
                        }
                        FieldEditState::EditingValue => {
                            // Mark as Edited with new value
                            field.value =
                                AggregatedValue::Edited(self.value_input.value().to_string());
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
                            let new_name = self.name_input.value().to_string();
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
                            field.value = self.value_input.value().to_string();
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
        let field = match self
            .aggregated_fields
            .as_ref()
            .and_then(|f| f.get(self.current_field_idx))
        {
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
        let insert_pos = if self.is_aggregated_mode() {
            if let Some(ref mut agg_fields) = self.aggregated_fields {
                let pos = agg_fields.len().saturating_sub(1);
                agg_fields.insert(
                    pos,
                    AggregatedTagField {
                        name: "new_tag".to_string(),
                        value: AggregatedValue::Edited(String::new()),
                        original_value: AggregatedValue::Consistent(String::new()),
                    },
                );
                Some(pos)
            } else {
                None
            }
        } else {
            let new_field = TagField {
                name: "new_tag".to_string(),
                value: String::new(),
                editable: true,
                deleted: false,
            };
            if let Some(fields) = self.tag_fields.get_mut(self.current_item_idx) {
                let pos = fields.len().saturating_sub(1);
                fields.insert(pos, new_field);
                Some(pos)
            } else {
                None
            }
        };

        if let Some(pos) = insert_pos {
            self.current_field_idx = pos;
            // Scroll to show the new tag
            if self.field_visible_height > 0 {
                let visible_end =
                    self.field_scroll_offset + self.field_visible_height.saturating_sub(1);
                if self.current_field_idx >= visible_end {
                    self.field_scroll_offset = self
                        .current_field_idx
                        .saturating_sub(self.field_visible_height.saturating_sub(2));
                }
            }
        }

        self.field_edit_state = FieldEditState::EditingName;
        self.name_input.set_value("new_tag");
        self.value_input.clear();
        self.focus_on_value = false;
    }

    // ========================================================================
    // Character Manipulation
    // ========================================================================

    /// Get a mutable reference to the active text input (name or value), if editing.
    pub(super) fn active_input_mut(&mut self) -> Option<&mut crate::widgets::TextInputState> {
        match self.field_edit_state {
            FieldEditState::EditingName => Some(&mut self.name_input),
            FieldEditState::EditingValue => Some(&mut self.value_input),
            FieldEditState::NonEditable => None,
        }
    }

    /// Clear the current field (input state or actual field value).
    pub(super) fn clear_current_field(&mut self) {
        match self.field_edit_state {
            FieldEditState::EditingName => {
                self.name_input.clear();
            }
            FieldEditState::EditingValue => {
                self.value_input.clear();
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
            edit_input: crate::widgets::TextInputState::new(),
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
