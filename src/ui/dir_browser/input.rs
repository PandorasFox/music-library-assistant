//! Key handling for the directory browser.

use crossterm::event::{KeyCode, KeyEvent};

use super::state::DirBrowserState;
use super::types::DirBrowserAction;

impl DirBrowserState {
    /// Handle a key event, returning an action for the caller.
    pub fn handle_key(&mut self, key: KeyEvent) -> DirBrowserAction {
        match key.code {
            // Navigation
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                DirBrowserAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
                DirBrowserAction::None
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.expand_current();
                DirBrowserAction::None
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.collapse_or_parent();
                DirBrowserAction::None
            }

            // Selection
            KeyCode::Char(' ') => {
                self.toggle_selection();
                DirBrowserAction::None
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                self.select_all();
                DirBrowserAction::None
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.deselect_all();
                DirBrowserAction::None
            }

            // Actions
            KeyCode::Enter => self.get_proceed_action(),
            KeyCode::Esc => DirBrowserAction::Cancel,

            _ => DirBrowserAction::None,
        }
    }
}
