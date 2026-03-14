//! Types for OOB tag sync resolution modal.
//!
//! Button, action, and context types are defined in mm-ui. The complex
//! `OobSyncState` stays here because it uses bulk selection and filtering
//! that don't fit the `ResolutionState` pattern yet.

use crate::action_handlers::witness::ConfirmationGesture;
use crate::input::InputAction;

use mm_meta::views::{OobSyncDirection, OobSyncFile};
use crate::bulk_selection::BulkSelectionState;
use crate::widgets::{FocusPane, FrameInputResult, TextInputState};
use crate::widgets::modal_frame::ModalFrameCore;
use crate::widgets::modal_frame::FrameState;

// Re-export button/action/context types from mm-ui
pub use mm_ui::resolutions::oob_sync::{OobSyncAction, OobSyncButton, OobSyncButtonCtx};

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
    /// Shared frame state (focus, buttons, click targets).
    pub frame: FrameState<OobSyncButton>,
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
        let mut frame = FrameState::new();
        if has_disk_to_index {
            frame.buttons.selected = OobSyncButton::AcceptDisk;
        } else {
            frame.buttons.selected = OobSyncButton::AcceptDb;
        }

        let file_count = files.len();
        let mut selection = BulkSelectionState::new();
        selection.select_all(file_count);

        Self {
            files,
            current_file: 0,
            scroll: 0,
            frame,
            selection,
            filter_input: TextInputState::new(),
            filter_active: false,
            filter_text: None,
            filtered_indices: None,
        }
    }

    /// Build the button context from current state.
    pub fn button_ctx(&self) -> OobSyncButtonCtx {
        OobSyncButtonCtx {
            disk_to_index_count: self.disk_to_index_count(),
            index_to_disk_count: self.index_to_disk_count(),
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
    pub(crate) fn handle_click(
        &mut self,
        x: u16,
        y: u16,
        _gesture: &ConfirmationGesture,
    ) -> Option<OobSyncAction> {
        let ctx = self.button_ctx();
        if let Some(action) = self.frame.buttons.handle_click(x, y, &ctx) {
            self.frame.focus_pane = FocusPane::Buttons;
            Some(action)
        } else {
            None
        }
    }

    pub fn handle_input(&mut self, action: &InputAction) -> OobSyncAction {
        // Inline filter bar captures all input when active
        if self.filter_active {
            match action {
                InputAction::Confirm => { self.apply_filter(); return OobSyncAction::None; }
                InputAction::Cancel => { self.clear_filter(); return OobSyncAction::None; }
                other => { self.filter_input.handle_input(other); return OobSyncAction::None; }
            }
        }

        // Modal-specific keys (ungated nav, selection, filter)
        match action {
            InputAction::TextHome => {
                let indices = self.get_filtered_indices();
                self.selection.toggle_all_filtered(&indices);
                return OobSyncAction::None;
            }
            InputAction::OpenFilter => {
                self.filter_active = true;
                self.filter_input.focused = true;
                return OobSyncAction::None;
            }
            InputAction::Toggle => {
                if !self.files.is_empty() { self.selection.toggle(self.current_file); }
                return OobSyncAction::None;
            }
            // Nav always works regardless of focus pane
            InputAction::NavUp => {
                if self.current_file > 0 { self.current_file -= 1; }
                return OobSyncAction::None;
            }
            InputAction::NavDown => {
                if self.current_file + 1 < self.files.len() { self.current_file += 1; }
                return OobSyncAction::None;
            }
            _ => {}
        }

        // Common keys: FocusUp/Down, NavLeft/Right (buttons), Confirm (buttons), Cancel, PageUp/Down
        match self.handle_frame_input(action) {
            FrameInputResult::Action(a) => a,
            _ => OobSyncAction::None,
        }
    }
}
