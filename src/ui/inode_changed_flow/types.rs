//! Types for inode changed acknowledgement flow.

use crossterm::event::{KeyCode, KeyEvent};

use crate::corpus::db::types::InodeChangedFile;
use crate::ui::widgets::FocusPane;

// ============================================================================
// Button Selection
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InodeChangedButton {
    #[default]
    Acknowledge,
    Cancel,
}

impl InodeChangedButton {
    pub fn left(self) -> Self {
        match self {
            Self::Acknowledge => Self::Acknowledge,
            Self::Cancel => Self::Acknowledge,
        }
    }

    pub fn right(self) -> Self {
        match self {
            Self::Acknowledge => Self::Cancel,
            Self::Cancel => Self::Cancel,
        }
    }
}

// ============================================================================
// Action Enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InodeChangedAction {
    None,
    /// Acknowledge all inode changes (updates tracks.inode, clears signals)
    Acknowledge,
    /// Cancel and return to Insights
    Cancel,
}

// ============================================================================
// State
// ============================================================================

pub struct InodeChangedState {
    /// Files with inode changed signals (cached at modal open)
    pub files: Vec<InodeChangedFile>,
    /// Currently selected file in the list
    pub current_file: usize,
    /// Scroll offset for file list
    pub scroll: usize,
    /// Currently selected button
    pub selected_button: InodeChangedButton,
    /// Current focus pane (List or Buttons)
    pub focus_pane: FocusPane,
}

impl InodeChangedState {
    pub fn new(files: Vec<InodeChangedFile>) -> Self {
        Self {
            files,
            current_file: 0,
            scroll: 0,
            selected_button: InodeChangedButton::Acknowledge,
            focus_pane: FocusPane::List,
        }
    }

    /// Get track IDs for all files (for the mutation)
    pub fn track_ids(&self) -> Vec<i64> {
        self.files.iter().map(|f| f.track_id).collect()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> InodeChangedAction {
        // Shift+Up / Shift+Down: cycle focus pane
        if key.modifiers.contains(crossterm::event::KeyModifiers::SHIFT) {
            match key.code {
                KeyCode::Up => {
                    self.focus_pane = self.focus_pane.prev();
                    return InodeChangedAction::None;
                }
                KeyCode::Down => {
                    self.focus_pane = self.focus_pane.next();
                    return InodeChangedAction::None;
                }
                _ => {}
            }
        }

        match key.code {
            // Up/Down: navigate file list
            KeyCode::Up | KeyCode::Char('k') => {
                if self.current_file > 0 {
                    self.current_file -= 1;
                }
                InodeChangedAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.current_file + 1 < self.files.len() {
                    self.current_file += 1;
                }
                InodeChangedAction::None
            }

            // Left/Right: navigate buttons when focused on buttons pane
            KeyCode::Left | KeyCode::Char('h') => {
                if self.focus_pane == FocusPane::Buttons {
                    self.selected_button = self.selected_button.left();
                }
                InodeChangedAction::None
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if self.focus_pane == FocusPane::Buttons {
                    self.selected_button = self.selected_button.right();
                }
                InodeChangedAction::None
            }

            // Confirm selected button (when focused on buttons)
            KeyCode::Enter => {
                if self.focus_pane == FocusPane::Buttons {
                    match self.selected_button {
                        InodeChangedButton::Acknowledge => {
                            if !self.files.is_empty() {
                                InodeChangedAction::Acknowledge
                            } else {
                                InodeChangedAction::None
                            }
                        }
                        InodeChangedButton::Cancel => InodeChangedAction::Cancel,
                    }
                } else {
                    InodeChangedAction::None
                }
            }

            KeyCode::Esc => InodeChangedAction::Cancel,

            _ => InodeChangedAction::None,
        }
    }
}
