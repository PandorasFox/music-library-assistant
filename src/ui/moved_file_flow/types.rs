//! Types for moved file acknowledgement flow.

use crossterm::event::{KeyCode, KeyEvent};

use crate::corpus::db::types::MovedFileInfo;
use crate::ui::widgets::FocusPane;

// ============================================================================
// Button Selection
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MovedFileButton {
    #[default]
    Acknowledge,
    Cancel,
}

impl MovedFileButton {
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
pub enum MovedFileAction {
    None,
    /// Acknowledge all moved files (updates paths in DB, clears signals)
    Acknowledge,
    /// Cancel and return to Insights
    Cancel,
}

// ============================================================================
// State
// ============================================================================

pub struct MovedFileState {
    /// Files with moved signals (cached at modal open)
    pub files: Vec<MovedFileInfo>,
    /// Currently selected file in the list
    pub current_file: usize,
    /// Scroll offset for file list
    pub scroll: usize,
    /// Currently selected button
    pub selected_button: MovedFileButton,
    /// Current focus pane (List or Buttons)
    pub focus_pane: FocusPane,
}

impl MovedFileState {
    pub fn new(files: Vec<MovedFileInfo>) -> Self {
        Self {
            files,
            current_file: 0,
            scroll: 0,
            selected_button: MovedFileButton::Acknowledge,
            focus_pane: FocusPane::List,
        }
    }

    /// Get data needed for UpdateFilePath mutations.
    ///
    /// Returns (inode, new_path) pairs for queuing mutations.
    pub fn files_for_mutation(&self) -> Vec<(i64, String)> {
        self.files
            .iter()
            .map(|f| (f.inode, f.new_path.clone()))
            .collect()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> MovedFileAction {
        // Shift+Up / Shift+Down: cycle focus pane
        if key.modifiers.contains(crossterm::event::KeyModifiers::SHIFT) {
            match key.code {
                KeyCode::Up => {
                    self.focus_pane = self.focus_pane.prev();
                    return MovedFileAction::None;
                }
                KeyCode::Down => {
                    self.focus_pane = self.focus_pane.next();
                    return MovedFileAction::None;
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
                MovedFileAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.current_file + 1 < self.files.len() {
                    self.current_file += 1;
                }
                MovedFileAction::None
            }

            // Left/Right: navigate buttons when focused on buttons pane
            KeyCode::Left | KeyCode::Char('h') => {
                if self.focus_pane == FocusPane::Buttons {
                    self.selected_button = self.selected_button.left();
                }
                MovedFileAction::None
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if self.focus_pane == FocusPane::Buttons {
                    self.selected_button = self.selected_button.right();
                }
                MovedFileAction::None
            }

            // Confirm selected button (when focused on buttons)
            KeyCode::Enter => {
                if self.focus_pane == FocusPane::Buttons {
                    match self.selected_button {
                        MovedFileButton::Acknowledge => {
                            if !self.files.is_empty() {
                                MovedFileAction::Acknowledge
                            } else {
                                MovedFileAction::None
                            }
                        }
                        MovedFileButton::Cancel => MovedFileAction::Cancel,
                    }
                } else {
                    MovedFileAction::None
                }
            }

            KeyCode::Esc => MovedFileAction::Cancel,

            _ => MovedFileAction::None,
        }
    }
}
