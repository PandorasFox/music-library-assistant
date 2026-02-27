//! Tag Editor Input Handling
//!
//! Keyboard input handling for the unified tag editor.
//! Dispatches key events to appropriate handlers based on focus state.

use super::state::UnifiedTagEditorState;
use crate::meta::decisions::{DecisionKey, DecisionSource};
use crate::ui::input::InputAction;
use super::types::{
    FieldEditState, NavigationDirection, StageChangesButton, TagEditorButton,
    TagEditorLaunchMode, UnifiedTagEditorAction, UnifiedTagEditorFocus,
    UnifiedTagEditorModal, UnsavedChangesButton,
};

impl UnifiedTagEditorState {
    /// Handle a semantic input action.
    pub fn handle_input(&mut self, action: &InputAction) -> UnifiedTagEditorAction {
        // Handle modal first if active
        if self.modal.is_some() {
            return self.handle_modal_input(action);
        }

        match self.focus {
            UnifiedTagEditorFocus::TagFields => self.handle_tag_fields_input(action),
            UnifiedTagEditorFocus::Actions => self.handle_actions_input(action),
        }
    }

    fn handle_modal_input(&mut self, action: &InputAction) -> UnifiedTagEditorAction {
        match &mut self.modal {
            Some(UnifiedTagEditorModal::UnsavedChanges { selected_button }) => {
                match action {
                    InputAction::Confirm => {
                        // Execute selected button action
                        match selected_button {
                            UnsavedChangesButton::KeepEditing => {
                                self.modal = None;
                                UnifiedTagEditorAction::CloseModal
                            }
                            UnsavedChangesButton::DiscardAndProceed => {
                                self.drop_changes_for_current_item();
                                self.modal = None;
                                if self.is_embedded() {
                                    UnifiedTagEditorAction::CloseEmbedded
                                } else {
                                    UnifiedTagEditorAction::DiscardTransaction
                                }
                            }
                        }
                    }
                    InputAction::Cancel => {
                        // Escape always cancels (keeps editing)
                        self.modal = None;
                        UnifiedTagEditorAction::CloseModal
                    }
                    InputAction::NavLeft | InputAction::NavRight | InputAction::CycleNext => {
                        // Toggle button selection
                        *selected_button = match selected_button {
                            UnsavedChangesButton::KeepEditing => {
                                UnsavedChangesButton::DiscardAndProceed
                            }
                            UnsavedChangesButton::DiscardAndProceed => {
                                UnsavedChangesButton::KeepEditing
                            }
                        };
                        UnifiedTagEditorAction::None
                    }
                    _ => UnifiedTagEditorAction::None,
                }
            }
            Some(UnifiedTagEditorModal::MultiValueEditor {
                field_idx,
                values,
                current_value_idx,
                editing,
                edit_buffer,
            }) => {
                let num_values = values.len();
                let add_entry_idx = num_values; // "+ Add value" is at end

                match action {
                    InputAction::Cancel => {
                        if *editing {
                            // Cancel edit, revert buffer
                            *editing = false;
                            edit_buffer.clear();
                        } else {
                            // Close modal, apply changes back to tag_fields
                            let field_idx_copy = *field_idx;
                            let values_copy = values.clone();
                            self.apply_multi_value_changes(field_idx_copy, values_copy);
                            self.modal = None;
                        }
                        UnifiedTagEditorAction::None
                    }
                    InputAction::Confirm => {
                        if *editing {
                            // Commit edit
                            if *current_value_idx < num_values {
                                values[*current_value_idx] = edit_buffer.clone();
                            } else if *current_value_idx == add_entry_idx && !edit_buffer.is_empty() {
                                // Adding new value
                                values.push(edit_buffer.clone());
                            }
                            *editing = false;
                            edit_buffer.clear();
                        } else if *current_value_idx < num_values {
                            // Start editing existing value
                            *editing = true;
                            *edit_buffer = values[*current_value_idx].clone();
                        } else if *current_value_idx == add_entry_idx {
                            // Start adding new value
                            *editing = true;
                            edit_buffer.clear();
                        }
                        UnifiedTagEditorAction::None
                    }
                    InputAction::NavUp => {
                        if !*editing && *current_value_idx > 0 {
                            *current_value_idx -= 1;
                        }
                        UnifiedTagEditorAction::None
                    }
                    InputAction::NavDown => {
                        if !*editing && *current_value_idx < add_entry_idx {
                            *current_value_idx += 1;
                        }
                        UnifiedTagEditorAction::None
                    }
                    InputAction::Delete | InputAction::Backspace if !*editing => {
                        // Delete current value (not the add entry)
                        if *current_value_idx < num_values && num_values > 0 {
                            values.remove(*current_value_idx);
                            if *current_value_idx >= values.len() && !values.is_empty() {
                                *current_value_idx = values.len() - 1;
                            }
                        }
                        UnifiedTagEditorAction::None
                    }
                    InputAction::Backspace if *editing => {
                        edit_buffer.pop();
                        UnifiedTagEditorAction::None
                    }
                    InputAction::Char(c) if *editing => {
                        edit_buffer.push(*c);
                        UnifiedTagEditorAction::None
                    }
                    _ => UnifiedTagEditorAction::None,
                }
            }
            Some(UnifiedTagEditorModal::StageChangesConfirm { direction, selected_button }) => {
                match action {
                    InputAction::Confirm => {
                        let direction = *direction;
                        match selected_button {
                            StageChangesButton::Yes => {
                                // Stage decision via proper Enter keypress, then navigate
                                let mutations = self.generate_mutations_for_current_item();
                                let key_item = self.decision_key_item(&mutations);
                                self.modal = None;
                                UnifiedTagEditorAction::StageDecisionAndNavigate {
                                    key: DecisionKey::new(DecisionSource::TagEdit, key_item),
                                    mutations,
                                    direction,
                                }
                            }
                            StageChangesButton::No => {
                                // Navigate without staging - edits remain in local state
                                self.modal = None;
                                match direction {
                                    NavigationDirection::Next => UnifiedTagEditorAction::NextItem,
                                    NavigationDirection::Prev => UnifiedTagEditorAction::PrevItem,
                                }
                            }
                            StageChangesButton::Cancel => {
                                // Stay on current file
                                self.modal = None;
                                UnifiedTagEditorAction::CloseModal
                            }
                        }
                    }
                    InputAction::Cancel => {
                        self.modal = None;
                        UnifiedTagEditorAction::CloseModal
                    }
                    InputAction::NavLeft => {
                        *selected_button = match selected_button {
                            StageChangesButton::Yes => StageChangesButton::Yes,
                            StageChangesButton::No => StageChangesButton::Yes,
                            StageChangesButton::Cancel => StageChangesButton::No,
                        };
                        UnifiedTagEditorAction::None
                    }
                    InputAction::NavRight | InputAction::CycleNext => {
                        *selected_button = match selected_button {
                            StageChangesButton::Yes => StageChangesButton::No,
                            StageChangesButton::No => StageChangesButton::Cancel,
                            StageChangesButton::Cancel => StageChangesButton::Cancel,
                        };
                        UnifiedTagEditorAction::None
                    }
                    _ => UnifiedTagEditorAction::None,
                }
            }
            None => UnifiedTagEditorAction::None,
        }
    }

    fn handle_tag_fields_input(&mut self, action: &InputAction) -> UnifiedTagEditorAction {
        match action {
            InputAction::Cancel => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    self.field_edit_state = FieldEditState::NonEditable;
                    UnifiedTagEditorAction::None
                } else if self.has_changes_for_current_item() {
                    self.modal = Some(UnifiedTagEditorModal::UnsavedChanges {
                        selected_button: UnsavedChangesButton::default(),
                    });
                    UnifiedTagEditorAction::None
                } else if self.is_embedded() {
                    UnifiedTagEditorAction::CloseEmbedded
                } else {
                    UnifiedTagEditorAction::DiscardTransaction
                }
            }
            InputAction::NavUp => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    self.commit_field_buffer();
                }
                if self.current_field_idx > 0 {
                    self.current_field_idx -= 1;
                    if self.current_field_idx < self.field_scroll_offset {
                        self.field_scroll_offset = self.current_field_idx;
                    }
                }
                self.load_field_buffer();
                UnifiedTagEditorAction::None
            }
            InputAction::NavDown => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    self.commit_field_buffer();
                }
                let max_fields = if self.is_aggregated_mode() {
                    self.aggregated_fields.as_ref().map(|f| f.len()).unwrap_or(0)
                } else {
                    self.tag_fields.get(self.current_item_idx).map(|f| f.len()).unwrap_or(0)
                };
                if self.current_field_idx < max_fields.saturating_sub(1) {
                    self.current_field_idx += 1;
                    let visible_end = self.field_scroll_offset + self.field_visible_height.saturating_sub(1);
                    if self.current_field_idx >= visible_end {
                        self.field_scroll_offset = self.current_field_idx.saturating_sub(self.field_visible_height.saturating_sub(2));
                    }
                }
                self.load_field_buffer();
                UnifiedTagEditorAction::None
            }
            InputAction::NavLeft => {
                // Left arrow: move focus from value to name (ContextList is not focusable)
                if self.field_edit_state == FieldEditState::NonEditable && self.focus_on_value {
                    self.focus_on_value = false;
                }
                UnifiedTagEditorAction::None
            }
            InputAction::NavRight => {
                if self.field_edit_state == FieldEditState::NonEditable {
                    if self.focus_on_value {
                        self.focus = UnifiedTagEditorFocus::Actions;
                    } else {
                        self.focus_on_value = true;
                    }
                }
                UnifiedTagEditorAction::None
            }
            InputAction::CycleNext => {
                // Tab: advance to next item (individual mode only)
                // In aggregated mode, Tab does nothing - use ReviewAll button
                if self.is_aggregated_mode() {
                    UnifiedTagEditorAction::None
                } else if self.has_changes_for_current_item() && !self.changes_match_staged() {
                    // Unsaved changes - show confirmation modal (requires Enter to stage)
                    self.modal = Some(UnifiedTagEditorModal::StageChangesConfirm {
                        direction: NavigationDirection::Next,
                        selected_button: StageChangesButton::default(),
                    });
                    UnifiedTagEditorAction::None
                } else {
                    UnifiedTagEditorAction::NextItem
                }
            }
            InputAction::CyclePrev => {
                // Shift-Tab: go to previous item (individual mode only)
                // In aggregated mode, Shift-Tab does nothing - use ReviewAll button
                if self.is_aggregated_mode() {
                    UnifiedTagEditorAction::None
                } else if self.has_changes_for_current_item() && !self.changes_match_staged() {
                    // Unsaved changes - show confirmation modal (requires Enter to stage)
                    self.modal = Some(UnifiedTagEditorModal::StageChangesConfirm {
                        direction: NavigationDirection::Prev,
                        selected_button: StageChangesButton::default(),
                    });
                    UnifiedTagEditorAction::None
                } else {
                    UnifiedTagEditorAction::PrevItem
                }
            }
            InputAction::Confirm => {
                self.handle_field_enter();
                UnifiedTagEditorAction::None
            }
            InputAction::Shortcut('r') => {
                if self.is_embedded() {
                    // Ctrl+R disabled in embedded mode (parent owns transaction)
                    UnifiedTagEditorAction::None
                } else {
                    // Ctrl+R: Request transaction review
                    UnifiedTagEditorAction::RequestTransactionReview
                }
            }
            InputAction::KillToStart => {
                self.clear_current_field();
                UnifiedTagEditorAction::None
            }
            InputAction::Char('n') if self.field_edit_state == FieldEditState::NonEditable => {
                self.create_new_tag();
                UnifiedTagEditorAction::None
            }
            InputAction::Char(c) => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    self.insert_char(*c);
                }
                UnifiedTagEditorAction::None
            }
            InputAction::Backspace => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    self.delete_char();
                } else {
                    // In navigation mode, toggle deletion mark on current field
                    self.toggle_current_field_deleted();
                }
                UnifiedTagEditorAction::None
            }
            InputAction::Delete => {
                if self.field_edit_state == FieldEditState::NonEditable {
                    // In navigation mode, toggle deletion mark on current field
                    self.toggle_current_field_deleted();
                }
                UnifiedTagEditorAction::None
            }
            _ => UnifiedTagEditorAction::None,
        }
    }

    fn handle_actions_input(&mut self, action: &InputAction) -> UnifiedTagEditorAction {
        let buttons = self.available_buttons();
        let current_idx = buttons.iter().position(|b| *b == self.selected_button).unwrap_or(0);

        match action {
            InputAction::NavLeft | InputAction::Cancel => {
                self.focus = UnifiedTagEditorFocus::TagFields;
                UnifiedTagEditorAction::None
            }
            InputAction::NavUp => {
                if current_idx > 0 {
                    self.selected_button = buttons[current_idx - 1];
                }
                UnifiedTagEditorAction::None
            }
            InputAction::NavDown => {
                if current_idx < buttons.len().saturating_sub(1) {
                    self.selected_button = buttons[current_idx + 1];
                }
                UnifiedTagEditorAction::None
            }
            InputAction::Confirm => {
                match self.selected_button {
                    TagEditorButton::ReviewAll => {
                        if self.is_embedded() {
                            // Embedded mode: collect all mutations and return to parent
                            let mutations = self.collect_all_mutations();
                            if mutations.is_empty() {
                                UnifiedTagEditorAction::CloseEmbedded
                            } else if let TagEditorLaunchMode::Embedded { ref decision_key, ref decision_label } = self.launch_mode {
                                UnifiedTagEditorAction::StageAndCloseEmbedded {
                                    decision_key: decision_key.clone(),
                                    decision_label: decision_label.clone(),
                                    mutations,
                                }
                            } else {
                                unreachable!()
                            }
                        } else {
                            let current_unstaged = self.has_changes_for_current_item()
                                && !self.changes_match_staged();
                            let has_anything = current_unstaged || self.staged_decision_count > 0;

                            if current_unstaged {
                                // Stage current file's changes, then open review
                                let mutations = self.generate_mutations_for_current_item();
                                let key_item = self.decision_key_item(&mutations);
                                UnifiedTagEditorAction::StageDecisionAndReview {
                                    key: DecisionKey::new(DecisionSource::TagEdit, key_item),
                                    mutations,
                                }
                            } else if has_anything {
                                // Already-staged decisions exist, go straight to review
                                UnifiedTagEditorAction::RequestTransactionReview
                            } else {
                                UnifiedTagEditorAction::StatusMessage("No changes to review".to_string())
                            }
                        }
                    }
                    TagEditorButton::RevertThisFile => {
                        self.drop_changes_for_current_item();
                        UnifiedTagEditorAction::StatusMessage("Changes reverted".to_string())
                    }
                    TagEditorButton::FillFromDisk => {
                        self.fill_from_disk();
                        UnifiedTagEditorAction::StatusMessage("Tags refreshed from disk".to_string())
                    }
                    TagEditorButton::FillFromDb => {
                        // Return action for UI layer to handle (requires DB access)
                        let inode = self.get_current_audio_file().map(|af| af.inode());
                        UnifiedTagEditorAction::RequestFillFromDb { inode }
                    }
                }
            }
            _ => UnifiedTagEditorAction::None,
        }
    }
}
