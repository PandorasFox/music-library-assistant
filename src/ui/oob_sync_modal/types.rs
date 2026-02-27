//! Types for OOB tag sync resolution modal.

use crate::ui::input::InputAction;

use crate::meta::views::{OobSyncDirection, OobSyncFile};
use crate::ui::bulk_selection::BulkSelectionState;
use crate::ui::filter_popup::FilterCondition;
use crate::ui::widgets::{ButtonRects, FocusPane};

// ============================================================================
// Button Selection
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OobSyncButton {
    AcceptDisk,
    AcceptDb,
    Cancel,
}

impl OobSyncButton {
    pub fn left(self) -> Self {
        match self {
            Self::AcceptDisk => Self::AcceptDisk,
            Self::AcceptDb => Self::AcceptDisk,
            Self::Cancel => Self::AcceptDb,
        }
    }

    pub fn right(self) -> Self {
        match self {
            Self::AcceptDisk => Self::AcceptDb,
            Self::AcceptDb => Self::Cancel,
            Self::Cancel => Self::Cancel,
        }
    }
}

// ============================================================================
// Action Enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OobSyncAction {
    None,
    /// Accept disk values — sync DiskToIndex files to DB
    AcceptDisk,
    /// Accept DB values — sync IndexToDisk files to disk
    AcceptDb,
    /// Cancel and return to Insights
    Cancel,
    /// Open filter popup (Ctrl+F)
    OpenFilter,
}

// ============================================================================
// State
// ============================================================================

pub struct OobSyncState {
    /// Files with syncable tag mismatches (cached at modal open)
    pub files: Vec<OobSyncFile>,
    /// Currently selected file in the list
    pub current_file: usize,
    /// Scroll offset for file list
    pub scroll: usize,
    /// Currently selected button
    pub selected_button: OobSyncButton,
    /// Current focus pane (List or Buttons)
    pub focus_pane: FocusPane,
    /// Button rectangles for click detection (set during render)
    pub button_rects: ButtonRects,
    /// Bulk selection state for multi-file operations
    pub selection: BulkSelectionState,
    /// Active filter condition (from Ctrl+F popup)
    pub filter: Option<FilterCondition>,
    /// Filtered indices (cached, updated when filter changes)
    pub filtered_indices: Option<Vec<usize>>,
}

impl OobSyncState {
    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.files.get(self.current_file).map(|f| f.path.as_str())
    }

    pub fn new(files: Vec<OobSyncFile>) -> Self {
        // Default to AcceptDisk if there are disk-to-index files, otherwise AcceptDb
        let has_disk_to_index = files.iter().any(|f| f.direction == OobSyncDirection::DiskToIndex);
        let default_button = if has_disk_to_index {
            OobSyncButton::AcceptDisk
        } else {
            OobSyncButton::AcceptDb
        };

        let file_count = files.len();
        let mut selection = BulkSelectionState::new();
        selection.select_all(file_count);

        Self {
            files,
            current_file: 0,
            scroll: 0,
            selected_button: default_button,
            focus_pane: FocusPane::List,
            button_rects: ButtonRects::new(),
            selection,
            filter: None,
            filtered_indices: None,
        }
    }

    /// Get indices of files that match the current filter.
    pub fn get_filtered_indices(&self) -> Vec<usize> {
        if let Some(ref indices) = self.filtered_indices {
            indices.clone()
        } else {
            (0..self.files.len()).collect()
        }
    }

    /// Apply a filter condition and compute filtered indices.
    pub fn apply_filter(&mut self, condition: FilterCondition) {
        if condition.is_active() {
            let indices: Vec<usize> = self.files
                .iter()
                .enumerate()
                .filter(|(_, _f)| {
                    // OobSyncFile doesn't have sample_rate/bitrate/duration,
                    // so we only filter by file type based on path extension
                    // TODO: Add full metadata filtering when track info is available
                    true // For now, accept all
                })
                .map(|(idx, _)| idx)
                .collect();
            self.filtered_indices = Some(indices);
            self.filter = Some(condition);
        } else {
            self.filter = None;
            self.filtered_indices = None;
        }
    }

    /// Clear the current filter.
    pub fn clear_filter(&mut self) {
        self.filter = None;
        self.filtered_indices = None;
    }

    /// Count of files by direction.
    pub fn disk_to_index_count(&self) -> usize {
        self.files.iter().filter(|f| f.direction == OobSyncDirection::DiskToIndex).count()
    }

    pub fn index_to_disk_count(&self) -> usize {
        self.files.iter().filter(|f| f.direction == OobSyncDirection::IndexToDisk).count()
    }

    pub fn handle_input(&mut self, action: &InputAction) -> OobSyncAction {
        match action {
            // Shift+Up / Shift+Down: cycle focus pane
            InputAction::FocusUp => {
                self.focus_pane = self.focus_pane.prev();
                OobSyncAction::None
            }
            InputAction::FocusDown => {
                self.focus_pane = self.focus_pane.next();
                OobSyncAction::None
            }

            // Ctrl+A: toggle all selection (respects active filter)
            InputAction::TextHome => {
                let indices = self.get_filtered_indices();
                self.selection.toggle_all_filtered(&indices);
                OobSyncAction::None
            }

            // Ctrl+F: open filter popup
            InputAction::OpenFilter => OobSyncAction::OpenFilter,

            // Space: toggle selection on current file
            InputAction::Toggle => {
                if !self.files.is_empty() {
                    self.selection.toggle(self.current_file);
                }
                OobSyncAction::None
            }

            // Up/Down: navigate file list (regardless of focus)
            InputAction::NavUp => {
                if self.current_file > 0 {
                    self.current_file -= 1;
                }
                OobSyncAction::None
            }
            InputAction::NavDown => {
                if self.current_file + 1 < self.files.len() {
                    self.current_file += 1;
                }
                OobSyncAction::None
            }

            // Left/Right: navigate buttons when focused on buttons pane
            InputAction::NavLeft => {
                if self.focus_pane == FocusPane::Buttons {
                    self.selected_button = self.selected_button.left();
                }
                OobSyncAction::None
            }
            InputAction::NavRight => {
                if self.focus_pane == FocusPane::Buttons {
                    self.selected_button = self.selected_button.right();
                }
                OobSyncAction::None
            }

            // Confirm selected button (when focused on buttons)
            InputAction::Confirm => {
                if self.focus_pane == FocusPane::Buttons {
                    match self.selected_button {
                        OobSyncButton::AcceptDisk => {
                            if self.disk_to_index_count() > 0 {
                                OobSyncAction::AcceptDisk
                            } else {
                                OobSyncAction::None
                            }
                        }
                        OobSyncButton::AcceptDb => {
                            if self.index_to_disk_count() > 0 {
                                OobSyncAction::AcceptDb
                            } else {
                                OobSyncAction::None
                            }
                        }
                        OobSyncButton::Cancel => OobSyncAction::Cancel,
                    }
                } else {
                    // When on list, Enter could expand/select - for now do nothing
                    OobSyncAction::None
                }
            }

            InputAction::Cancel => OobSyncAction::Cancel,

            _ => OobSyncAction::None,
        }
    }
}
