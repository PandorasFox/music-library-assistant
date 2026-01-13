//! Directory Tag Editor Input Handling
//!
//! Keyboard input processing for the directory tag editor.
//!
//! TODO: Update controls to match deploy conflict editor pattern for group workflows:
//! - Shift+Up/Down for track/file navigation within the group
//! - Tab/Shift+Tab for group-to-group navigation (sibling directories)
//! See docs/UX.md "Multi-Track Navigation Pattern" for the standard control scheme.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::directory_state::DirectoryTagEditorState;
use super::types::{
    DirectoryTagEditorAction, DirectoryTagEditorFocus, DirectoryTagEditorModal, FieldEditState,
};

impl DirectoryTagEditorState {
    /// Handle a key event
    pub fn handle_key(&mut self, key: KeyEvent) -> DirectoryTagEditorAction {
        // During gathering, only Esc is handled
        if self.is_gathering() {
            if key.code == KeyCode::Esc {
                self.cancel_gathering();
                return DirectoryTagEditorAction::Exit;
            }
            return DirectoryTagEditorAction::None;
        }

        // Handle action pane separately
        if matches!(self.focus, DirectoryTagEditorFocus::ActionPane) {
            return self.handle_action_pane_key(key);
        }

        match key.code {
            KeyCode::Esc => self.handle_escape(),
            KeyCode::Up => {
                self.move_up();
                DirectoryTagEditorAction::None
            }
            KeyCode::Down => {
                self.move_down();
                DirectoryTagEditorAction::None
            }
            KeyCode::Left => {
                if self.field_edit_state == FieldEditState::NonEditable {
                    // Left does nothing in directory mode (no name editing)
                }
                DirectoryTagEditorAction::None
            }
            KeyCode::Right => {
                if self.field_edit_state == FieldEditState::NonEditable {
                    // Right from tag fields -> action pane
                    self.focus = DirectoryTagEditorFocus::ActionPane;
                }
                DirectoryTagEditorAction::None
            }
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    // Shift+Tab -> previous sibling
                    if self.has_changes() {
                        DirectoryTagEditorAction::ShowModal(DirectoryTagEditorModal::UnsavedChanges {
                            going_next: false,
                        })
                    } else {
                        DirectoryTagEditorAction::SwitchDirectory(false)
                    }
                } else {
                    // Tab -> next sibling
                    if self.has_changes() {
                        DirectoryTagEditorAction::ShowModal(DirectoryTagEditorModal::UnsavedChanges {
                            going_next: true,
                        })
                    } else {
                        DirectoryTagEditorAction::SwitchDirectory(true)
                    }
                }
            }
            KeyCode::BackTab => {
                // BackTab is how most terminals send Shift+Tab
                if self.has_changes() {
                    DirectoryTagEditorAction::ShowModal(DirectoryTagEditorModal::UnsavedChanges {
                        going_next: false,
                    })
                } else {
                    DirectoryTagEditorAction::SwitchDirectory(false)
                }
            }
            KeyCode::Enter => self.handle_enter(),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.clear_current_field();
                DirectoryTagEditorAction::None
            }
            KeyCode::Char(c) => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    self.insert_char(c);
                }
                DirectoryTagEditorAction::None
            }
            KeyCode::Backspace => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    self.delete_char();
                }
                DirectoryTagEditorAction::None
            }
            _ => DirectoryTagEditorAction::None,
        }
    }

    /// Handle keys when action pane is focused
    fn handle_action_pane_key(&mut self, key: KeyEvent) -> DirectoryTagEditorAction {
        match key.code {
            KeyCode::Left | KeyCode::Esc => {
                // Return to tag fields
                self.focus = DirectoryTagEditorFocus::TagFields;
                DirectoryTagEditorAction::None
            }
            KeyCode::Enter => {
                // Show change preview modal
                let changes = self.compute_changes();
                if changes.is_empty() {
                    DirectoryTagEditorAction::StatusMessage("No changes to save".to_string())
                } else {
                    DirectoryTagEditorAction::ShowModal(DirectoryTagEditorModal::ChangePreview {
                        scroll_offset: 0,
                        save_and_next: false,
                    })
                }
            }
            _ => DirectoryTagEditorAction::None,
        }
    }
}
