//! Tag Editor Input Handling
//!
//! Keyboard input processing for the tag editor.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::state::TagEditorState;
use super::types::{FieldEditState, TagEditorAction, TagEditorFocus, TagEditorModal};

impl TagEditorState {
    /// Handle a key event
    pub fn handle_key(&mut self, key: KeyEvent) -> TagEditorAction {
        // Handle action pane separately
        if matches!(self.focus, TagEditorFocus::ActionPane) {
            return self.handle_action_pane_key(key);
        }

        // In duplicate workflow: Shift+arrows = track nav, Tab = group nav
        // In standalone mode: Tab = track nav (legacy)
        let in_workflow = self.is_in_duplicate_workflow();

        match key.code {
            KeyCode::Esc => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    // Exit edit mode, restore original values
                    self.field_edit_state = FieldEditState::NonEditable;
                    TagEditorAction::None
                } else {
                    TagEditorAction::Exit
                }
            }
            KeyCode::Up => {
                if key.modifiers.contains(KeyModifiers::SHIFT) && in_workflow {
                    // Shift+Up: navigate to previous track in group
                    self.prev_track();
                } else {
                    // Plain Up: navigate fields within current track
                    self.move_up();
                }
                TagEditorAction::None
            }
            KeyCode::Down => {
                if key.modifiers.contains(KeyModifiers::SHIFT) && in_workflow {
                    // Shift+Down: navigate to next track in group
                    let _ = self.next_track(); // Ignore at-end signal for track nav
                } else {
                    // Plain Down: navigate fields within current track
                    self.move_down();
                }
                TagEditorAction::None
            }
            KeyCode::Left => {
                if self.field_edit_state == FieldEditState::NonEditable {
                    self.focus_on_value = false;
                }
                TagEditorAction::None
            }
            KeyCode::Right => {
                if self.field_edit_state == FieldEditState::NonEditable {
                    if self.focus_on_value {
                        // Right from value field -> action pane
                        self.focus = TagEditorFocus::ActionPane;
                    } else {
                        // Right from name field -> value field
                        self.focus_on_value = true;
                    }
                }
                TagEditorAction::None
            }
            KeyCode::Tab => {
                if in_workflow {
                    // In duplicate workflow: Tab advances to next group
                    // Show change preview with save_and_next flag
                    let changes = super::state::compute_changes(
                        &self.original_tag_fields,
                        &self.tag_fields,
                    );
                    let (grouped, single) = super::state::group_common_changes(&changes);
                    return TagEditorAction::ShowModal(TagEditorModal::ChangePreview {
                        grouped_changes: grouped,
                        single_changes: single,
                        scroll_offset: 0,
                        save_and_next: true,
                    });
                } else {
                    // Legacy standalone mode: Tab navigates tracks
                    if key.modifiers.contains(KeyModifiers::SHIFT) {
                        self.prev_track();
                    } else {
                        let at_end = self.next_track();
                        if at_end {
                            // Tabbing past last track triggers save modal
                            return TagEditorAction::ShowModal(TagEditorModal::SaveConfirmation {
                                selected_button: 2,
                            });
                        }
                    }
                }
                TagEditorAction::None
            }
            KeyCode::BackTab => {
                if in_workflow {
                    // In duplicate workflow: Shift+Tab goes to previous group
                    // For now, just show a message since going back isn't implemented yet
                    // TODO: Implement going back to previous group with accumulated state
                    TagEditorAction::StatusMessage("Previous group navigation not yet implemented".to_string())
                } else {
                    // Legacy standalone mode: navigate tracks
                    self.prev_track();
                    TagEditorAction::None
                }
            }
            KeyCode::Enter => {
                self.handle_enter();
                TagEditorAction::None
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.clear_current_field();
                TagEditorAction::None
            }
            KeyCode::Char(c) => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    // In edit mode: insert character
                    self.insert_char(c);
                    TagEditorAction::None
                } else {
                    // Not editing: check for hotkeys
                    match c {
                        'f' | 'F' => {
                            if self.fill_to_all() {
                                TagEditorAction::StatusMessage("Value copied to all tracks".to_string())
                            } else {
                                TagEditorAction::None
                            }
                        }
                        _ => TagEditorAction::None,
                    }
                }
            }
            KeyCode::Backspace => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    self.delete_char();
                }
                TagEditorAction::None
            }
            _ => TagEditorAction::None,
        }
    }

    /// Handle keys when action pane is focused
    fn handle_action_pane_key(&mut self, key: KeyEvent) -> TagEditorAction {
        match key.code {
            KeyCode::Left | KeyCode::Esc => {
                // Return to tag fields
                self.focus = TagEditorFocus::TagFields;
                TagEditorAction::None
            }
            KeyCode::Enter => {
                // Show change preview modal
                let changes = super::state::compute_changes(
                    &self.original_tag_fields,
                    &self.tag_fields,
                );
                let (grouped, single) = super::state::group_common_changes(&changes);
                if grouped.is_empty() && single.is_empty() {
                    TagEditorAction::StatusMessage("No changes to save".to_string())
                } else {
                    TagEditorAction::ShowModal(TagEditorModal::ChangePreview {
                        grouped_changes: grouped,
                        single_changes: single,
                        scroll_offset: 0,
                        save_and_next: false,
                    })
                }
            }
            _ => TagEditorAction::None,
        }
    }
}
