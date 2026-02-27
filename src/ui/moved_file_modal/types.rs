//! Types for moved file acknowledgement modal.

use crate::ui::input::InputAction;

use crate::meta::views::MovedFileInfo;
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
    /// Currently selected button
    pub selected_button: MovedFileButton,
    /// Current focus pane (List or Buttons)
    pub focus_pane: FocusPane,
}

impl MovedFileState {
    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.files.get(self.current_file).map(|f| f.new_path.as_str())
    }

    pub fn new(files: Vec<MovedFileInfo>) -> Self {
        Self {
            files,
            current_file: 0,
            selected_button: MovedFileButton::Acknowledge,
            focus_pane: FocusPane::List,
        }
    }

    /// Get data needed for UpdateFilePath mutations.
    ///
    /// Returns (inode, new_path, old_zone, new_zone) tuples for queuing mutations.
    pub fn files_for_mutation(&self) -> Vec<(i64, String, String, String)> {
        self.files
            .iter()
            .map(|f| (f.inode, f.new_path.clone(), f.old_zone.clone(), f.new_zone.clone()))
            .collect()
    }

    pub fn handle_input(&mut self, action: &InputAction) -> MovedFileAction {
        // FocusUp / FocusDown: cycle focus pane
        match action {
            InputAction::FocusUp => {
                self.focus_pane = self.focus_pane.prev();
                return MovedFileAction::None;
            }
            InputAction::FocusDown => {
                self.focus_pane = self.focus_pane.next();
                return MovedFileAction::None;
            }
            _ => {}
        }

        match action {
            // Up/Down: navigate file list
            InputAction::NavUp => {
                if self.current_file > 0 {
                    self.current_file -= 1;
                }
                MovedFileAction::None
            }
            InputAction::NavDown => {
                if self.current_file + 1 < self.files.len() {
                    self.current_file += 1;
                }
                MovedFileAction::None
            }

            // Left/Right: navigate buttons when focused on buttons pane
            InputAction::NavLeft => {
                if self.focus_pane == FocusPane::Buttons {
                    self.selected_button = self.selected_button.left();
                }
                MovedFileAction::None
            }
            InputAction::NavRight => {
                if self.focus_pane == FocusPane::Buttons {
                    self.selected_button = self.selected_button.right();
                }
                MovedFileAction::None
            }

            // Confirm selected button (when focused on buttons)
            InputAction::Confirm => {
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

            InputAction::Cancel => MovedFileAction::Cancel,

            _ => MovedFileAction::None,
        }
    }
}
