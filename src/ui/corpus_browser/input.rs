//! Corpus Browser Input Handling

use crossterm::event::{KeyCode, KeyEvent};

use super::state::CorpusBrowserState;
use super::types::CorpusBrowserAction;

impl CorpusBrowserState {
    /// Handle a key event.
    pub fn handle_key(&mut self, key: KeyEvent) -> CorpusBrowserAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                CorpusBrowserAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
                CorpusBrowserAction::None
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.expand_current();
                CorpusBrowserAction::None
            }
            KeyCode::Left | KeyCode::Char('h') => {
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
            _ => CorpusBrowserAction::None,
        }
    }
}
