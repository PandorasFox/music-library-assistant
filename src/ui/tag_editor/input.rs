//! Tag Editor Input Handling
//!
//! Keyboard input handling for the unified tag editor.
//! Dispatches key events to appropriate handlers based on focus state.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::state::UnifiedTagEditorState;
use super::types::{
    FieldEditState, NavigationDirection, StageChangesButton, TagEditorButton,
    UnifiedTagEditorAction, UnifiedTagEditorFocus, UnifiedTagEditorModal,
    UnsavedChangesButton,
};

impl UnifiedTagEditorState {
    /// Handle a key event.
    pub fn handle_key(&mut self, key: KeyEvent) -> UnifiedTagEditorAction {
        // Handle modal first if active
        if self.modal.is_some() {
            return self.handle_modal_key(key);
        }

        match self.focus {
            UnifiedTagEditorFocus::TagFields => self.handle_tag_fields_key(key),
            UnifiedTagEditorFocus::Actions => self.handle_actions_key(key),
        }
    }

    fn handle_modal_key(&mut self, key: KeyEvent) -> UnifiedTagEditorAction {
        match &mut self.modal {
            Some(UnifiedTagEditorModal::UnsavedChanges { selected_button }) => {
                match key.code {
                    KeyCode::Enter => {
                        // Execute selected button action
                        match selected_button {
                            UnsavedChangesButton::KeepEditing => {
                                self.modal = None;
                                UnifiedTagEditorAction::CloseModal
                            }
                            UnsavedChangesButton::DiscardAndProceed => {
                                self.drop_changes_for_current_item();
                                self.modal = None;
                                // UnsavedChanges is now only used for Exit
                                UnifiedTagEditorAction::DiscardTransaction
                            }
                        }
                    }
                    KeyCode::Esc => {
                        // Escape always cancels (keeps editing)
                        self.modal = None;
                        UnifiedTagEditorAction::CloseModal
                    }
                    KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
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

                match key.code {
                    KeyCode::Esc => {
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
                    KeyCode::Enter => {
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
                    KeyCode::Up => {
                        if !*editing && *current_value_idx > 0 {
                            *current_value_idx -= 1;
                        }
                        UnifiedTagEditorAction::None
                    }
                    KeyCode::Down => {
                        if !*editing && *current_value_idx < add_entry_idx {
                            *current_value_idx += 1;
                        }
                        UnifiedTagEditorAction::None
                    }
                    KeyCode::Delete | KeyCode::Backspace if !*editing => {
                        // Delete current value (not the add entry)
                        if *current_value_idx < num_values && num_values > 0 {
                            values.remove(*current_value_idx);
                            if *current_value_idx >= values.len() && !values.is_empty() {
                                *current_value_idx = values.len() - 1;
                            }
                        }
                        UnifiedTagEditorAction::None
                    }
                    KeyCode::Backspace if *editing => {
                        edit_buffer.pop();
                        UnifiedTagEditorAction::None
                    }
                    KeyCode::Char(c) if *editing => {
                        edit_buffer.push(c);
                        UnifiedTagEditorAction::None
                    }
                    _ => UnifiedTagEditorAction::None,
                }
            }
            Some(UnifiedTagEditorModal::StageChangesConfirm { direction, selected_button }) => {
                match key.code {
                    KeyCode::Enter => {
                        let direction = *direction;
                        match selected_button {
                            StageChangesButton::Yes => {
                                // Stage decision via proper Enter keypress, then navigate
                                let mutations = self.generate_mutations_for_current_item();
                                self.modal = None;
                                UnifiedTagEditorAction::StageDecisionAndNavigate {
                                    index: self.current_item_idx,
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
                    KeyCode::Esc => {
                        self.modal = None;
                        UnifiedTagEditorAction::CloseModal
                    }
                    KeyCode::Left => {
                        *selected_button = match selected_button {
                            StageChangesButton::Yes => StageChangesButton::Yes,
                            StageChangesButton::No => StageChangesButton::Yes,
                            StageChangesButton::Cancel => StageChangesButton::No,
                        };
                        UnifiedTagEditorAction::None
                    }
                    KeyCode::Right | KeyCode::Tab => {
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

    fn handle_tag_fields_key(&mut self, key: KeyEvent) -> UnifiedTagEditorAction {
        match key.code {
            KeyCode::Esc => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    self.field_edit_state = FieldEditState::NonEditable;
                    UnifiedTagEditorAction::None
                } else if self.has_changes_for_current_item() {
                    self.modal = Some(UnifiedTagEditorModal::UnsavedChanges {
                        selected_button: UnsavedChangesButton::default(),
                    });
                    UnifiedTagEditorAction::None
                } else {
                    UnifiedTagEditorAction::DiscardTransaction
                }
            }
            KeyCode::Up => {
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
            KeyCode::Down => {
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
            KeyCode::Left => {
                // Left arrow: move focus from value to name (ContextList is not focusable)
                if self.field_edit_state == FieldEditState::NonEditable && self.focus_on_value {
                    self.focus_on_value = false;
                }
                UnifiedTagEditorAction::None
            }
            KeyCode::Right => {
                if self.field_edit_state == FieldEditState::NonEditable {
                    if self.focus_on_value {
                        self.focus = UnifiedTagEditorFocus::Actions;
                    } else {
                        self.focus_on_value = true;
                    }
                }
                UnifiedTagEditorAction::None
            }
            KeyCode::Tab => {
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
            KeyCode::BackTab => {
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
            KeyCode::Enter => {
                self.handle_field_enter();
                UnifiedTagEditorAction::None
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+R: Request transaction review
                // UI layer will query daemon for decisions and populate the modal
                UnifiedTagEditorAction::RequestTransactionReview
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.clear_current_field();
                UnifiedTagEditorAction::None
            }
            KeyCode::Char(c) => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    self.insert_char(c);
                }
                UnifiedTagEditorAction::None
            }
            KeyCode::Backspace => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    self.delete_char();
                } else {
                    // In navigation mode, toggle deletion mark on current field
                    self.toggle_current_field_deleted();
                }
                UnifiedTagEditorAction::None
            }
            KeyCode::Delete => {
                if self.field_edit_state == FieldEditState::NonEditable {
                    // In navigation mode, toggle deletion mark on current field
                    self.toggle_current_field_deleted();
                }
                UnifiedTagEditorAction::None
            }
            _ => UnifiedTagEditorAction::None,
        }
    }

    fn handle_actions_key(&mut self, key: KeyEvent) -> UnifiedTagEditorAction {
        let buttons = self.available_buttons();
        let current_idx = buttons.iter().position(|b| *b == self.selected_button).unwrap_or(0);

        match key.code {
            KeyCode::Left | KeyCode::Esc => {
                self.focus = UnifiedTagEditorFocus::TagFields;
                UnifiedTagEditorAction::None
            }
            KeyCode::Up => {
                if current_idx > 0 {
                    self.selected_button = buttons[current_idx - 1];
                }
                UnifiedTagEditorAction::None
            }
            KeyCode::Down => {
                if current_idx < buttons.len().saturating_sub(1) {
                    self.selected_button = buttons[current_idx + 1];
                }
                UnifiedTagEditorAction::None
            }
            KeyCode::Enter => {
                match self.selected_button {
                    TagEditorButton::ReviewAll => {
                        let current_unstaged = self.has_changes_for_current_item()
                            && !self.changes_match_staged();
                        let has_anything = current_unstaged || self.staged_decision_count > 0;

                        if current_unstaged {
                            // Stage current file's changes, then open review
                            let mutations = self.generate_mutations_for_current_item();
                            UnifiedTagEditorAction::StageDecisionAndReview {
                                index: self.current_item_idx,
                                mutations,
                            }
                        } else if has_anything {
                            // Already-staged decisions exist, go straight to review
                            UnifiedTagEditorAction::RequestTransactionReview
                        } else {
                            UnifiedTagEditorAction::StatusMessage("No changes to review".to_string())
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
