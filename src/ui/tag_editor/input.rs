//! Tag Editor Input Handling
//!
//! Keyboard input processing for the tag editor.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::state::TagEditorState;
use super::types::{FieldEditState, TagEditorAction, TagEditorModal};

impl TagEditorState {
    /// Handle a key event
    pub fn handle_key(&mut self, key: KeyEvent) -> TagEditorAction {
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
                self.move_up();
                TagEditorAction::None
            }
            KeyCode::Down => {
                self.move_down();
                TagEditorAction::None
            }
            KeyCode::Left => {
                self.focus_on_value = false;
                TagEditorAction::None
            }
            KeyCode::Right => {
                self.focus_on_value = true;
                TagEditorAction::None
            }
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.prev_track();
                } else {
                    let at_end = self.next_track();
                    if at_end {
                        // Tabbing past last track triggers save modal
                        return TagEditorAction::ShowModal(TagEditorModal::SaveConfirmation {
                            selected_button: 0,
                        });
                    }
                }
                TagEditorAction::None
            }
            KeyCode::BackTab => {
                // BackTab is how most terminals send Shift+Tab
                self.prev_track();
                TagEditorAction::None
            }
            KeyCode::Enter => {
                self.handle_enter();
                TagEditorAction::None
            }
            KeyCode::Char('f') | KeyCode::Char('F') => {
                if self.fill_to_all() {
                    TagEditorAction::StatusMessage("Value copied to all tracks".to_string())
                } else {
                    TagEditorAction::None
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.clear_current_field();
                TagEditorAction::None
            }
            KeyCode::Char(c) => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    self.insert_char(c);
                }
                TagEditorAction::None
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
}
