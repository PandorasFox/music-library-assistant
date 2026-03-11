//! Types for OOB tag sync resolution modal.

use crate::ui::action_handlers::witness::ConfirmationGesture;
use crate::ui::input::InputAction;

use crate::meta::views::{OobSyncDirection, OobSyncFile};
use crate::ui::bulk_selection::BulkSelectionState;
use crate::ui::widgets::{ButtonRects, FocusPane, TextInputState};

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
    /// Inline text filter input
    pub filter_input: TextInputState,
    /// Whether the inline filter bar is actively accepting input
    pub filter_active: bool,
    /// Active filter text (applied on Enter)
    pub filter_text: Option<String>,
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
        let has_disk_to_index = files
            .iter()
            .any(|f| f.direction == OobSyncDirection::DiskToIndex);
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
            filter_input: TextInputState::new(),
            filter_active: false,
            filter_text: None,
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

    /// Apply the current filter input text as a path substring filter.
    pub fn apply_filter(&mut self) {
        let query = self.filter_input.value().trim().to_lowercase();
        if query.is_empty() {
            self.clear_filter();
        } else {
            let indices: Vec<usize> = self
                .files
                .iter()
                .enumerate()
                .filter(|(_, f)| f.path.to_lowercase().contains(&query))
                .map(|(idx, _)| idx)
                .collect();
            self.filtered_indices = Some(indices);
            self.filter_text = Some(query);
        }
        self.filter_active = false;
        self.filter_input.focused = false;
    }

    /// Clear the current filter.
    pub fn clear_filter(&mut self) {
        self.filter_text = None;
        self.filtered_indices = None;
        self.filter_active = false;
        self.filter_input.focused = false;
        self.filter_input.clear();
    }

    /// Count of files by direction.
    pub fn disk_to_index_count(&self) -> usize {
        self.files
            .iter()
            .filter(|f| f.direction == OobSyncDirection::DiskToIndex)
            .count()
    }

    pub fn index_to_disk_count(&self) -> usize {
        self.files
            .iter()
            .filter(|f| f.direction == OobSyncDirection::IndexToDisk)
            .count()
    }

    /// Handle a mouse click at (x, y). Returns an action if a button was clicked.
    pub fn handle_click(
        &mut self,
        x: u16,
        y: u16,
        _gesture: &ConfirmationGesture,
    ) -> Option<OobSyncAction> {
        if let Some(button_name) = self.button_rects.hit_test(x, y) {
            self.focus_pane = FocusPane::Buttons;
            match button_name {
                "accept_disk" => {
                    self.selected_button = OobSyncButton::AcceptDisk;
                    Some(OobSyncAction::AcceptDisk)
                }
                "accept_db" => {
                    self.selected_button = OobSyncButton::AcceptDb;
                    Some(OobSyncAction::AcceptDb)
                }
                "cancel" => {
                    self.selected_button = OobSyncButton::Cancel;
                    Some(OobSyncAction::Cancel)
                }
                _ => None,
            }
        } else {
            None
        }
    }

    pub fn handle_input(&mut self, action: &InputAction) -> OobSyncAction {
        // Inline filter bar captures all input when active
        if self.filter_active {
            match action {
                InputAction::Confirm => {
                    self.apply_filter();
                    return OobSyncAction::None;
                }
                InputAction::Cancel => {
                    self.clear_filter();
                    return OobSyncAction::None;
                }
                other => {
                    self.filter_input.handle_input(other);
                    return OobSyncAction::None;
                }
            }
        }

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

            // Ctrl+/: activate inline filter
            InputAction::OpenFilter => {
                self.filter_active = true;
                self.filter_input.focused = true;
                OobSyncAction::None
            }

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
