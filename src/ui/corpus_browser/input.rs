//! Corpus Browser Input Handling

use crossterm::event::{KeyCode, KeyEvent};

use super::state::CorpusBrowserState;
use super::types::CorpusBrowserAction;

impl CorpusBrowserState {
    /// Handle a key event.
    pub fn handle_key(&mut self, key: KeyEvent) -> CorpusBrowserAction {
        // Handle match selection mode (modal) first
        if self.is_match_selection_mode() {
            return self.handle_match_selection_key(key);
        }

        // Handle search mode
        if self.search().is_active() {
            return self.handle_search_key(key);
        }

        // Normal navigation mode
        self.handle_navigation_key(key)
    }

    /// Handle key events in normal navigation mode.
    fn handle_navigation_key(&mut self, key: KeyEvent) -> CorpusBrowserAction {
        match key.code {
            KeyCode::Up => {
                self.move_up();
                CorpusBrowserAction::None
            }
            KeyCode::Down => {
                self.move_down();
                CorpusBrowserAction::None
            }
            KeyCode::Right => {
                self.expand_current();
                CorpusBrowserAction::None
            }
            KeyCode::Left => {
                self.collapse_or_parent();
                CorpusBrowserAction::None
            }
            KeyCode::Enter => {
                if let Some(entry) = self.current_entry() {
                    let path = entry.path.clone();
                    if entry.is_directory {
                        CorpusBrowserAction::EditDirectory(path)
                    } else {
                        CorpusBrowserAction::EditFile(path)
                    }
                } else {
                    CorpusBrowserAction::None
                }
            }
            KeyCode::Esc => CorpusBrowserAction::Cancel,
            // Start search on any alphanumeric character
            KeyCode::Char(c) if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' => {
                self.search_push_char(c);
                CorpusBrowserAction::None
            }
            _ => CorpusBrowserAction::None,
        }
    }

    /// Handle key events in search mode.
    fn handle_search_key(&mut self, key: KeyEvent) -> CorpusBrowserAction {
        match key.code {
            KeyCode::Esc => {
                self.cancel_search();
                CorpusBrowserAction::None
            }
            KeyCode::Backspace => {
                self.search_pop_char();
                CorpusBrowserAction::None
            }
            KeyCode::Tab => {
                self.apply_suggestion();
                CorpusBrowserAction::None
            }
            KeyCode::Enter => {
                // jump_to_match returns false if we need to show modal
                self.jump_to_match();
                CorpusBrowserAction::None
            }
            KeyCode::Char(c) => {
                self.search_push_char(c);
                CorpusBrowserAction::None
            }
            // Allow arrow navigation while searching
            KeyCode::Up => {
                self.move_up();
                CorpusBrowserAction::None
            }
            KeyCode::Down => {
                self.move_down();
                CorpusBrowserAction::None
            }
            _ => CorpusBrowserAction::None,
        }
    }

    /// Handle key events in match selection mode (modal).
    fn handle_match_selection_key(&mut self, key: KeyEvent) -> CorpusBrowserAction {
        match key.code {
            KeyCode::Esc => {
                self.cancel_match_selection();
                CorpusBrowserAction::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.match_selection_up();
                CorpusBrowserAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.match_selection_down();
                CorpusBrowserAction::None
            }
            KeyCode::Enter => {
                self.confirm_match_selection();
                CorpusBrowserAction::None
            }
            _ => CorpusBrowserAction::None,
        }
    }
}
