//! Types for OOB tag sync resolution modal.

use std::borrow::Cow;

use ratatui::style::Color;

use crate::action_handlers::witness::ConfirmationGesture;
use crate::input::InputAction;

use mm_meta::decisions::DecisionKey;
use mm_meta::views::{OobSyncDirection, OobSyncFile};
use mm_ui::protocol_binding::ProtocolBinding;
use crate::bulk_selection::BulkSelectionState;
use crate::widgets::modal_buttons::ModalButtons;
use crate::widgets::{FocusPane, FrameInputResult, TextInputState};
use crate::widgets::modal_frame::ModalFrameCore;
use crate::widgets::modal_frame::FrameState;

// ============================================================================
// Button Selection
// ============================================================================

/// Context for OobSyncButton enablement.
#[derive(Debug, Clone, Copy)]
pub struct OobSyncButtonCtx {
    pub disk_to_index_count: usize,
    pub index_to_disk_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OobSyncButton {
    #[default]
    AcceptDisk,
    AcceptDb,
    Cancel,
}

impl ModalButtons for OobSyncButton {
    type Context = OobSyncButtonCtx;
    type Action = OobSyncAction;

    fn all() -> &'static [Self] {
        &[Self::AcceptDisk, Self::AcceptDb, Self::Cancel]
    }

    fn label(&self, ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::AcceptDisk => format!("Accept Disk ({})", ctx.disk_to_index_count).into(),
            Self::AcceptDb => format!("Accept DB ({})", ctx.index_to_disk_count).into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, _ctx: &Self::Context) -> Color {
        match self {
            Self::AcceptDisk => Color::Cyan,
            Self::AcceptDb => Color::Magenta,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::AcceptDisk => ctx.disk_to_index_count > 0,
            Self::AcceptDb => ctx.index_to_disk_count > 0,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> OobSyncAction {
        match self {
            Self::AcceptDisk => OobSyncAction::AcceptDisk,
            Self::AcceptDb => OobSyncAction::AcceptDb,
            Self::Cancel => OobSyncAction::Cancel,
        }
    }

    fn protocol_binding(&self, _ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::AcceptDisk => ProtocolBinding::Transaction {
                decision_key: DecisionKey::OobSync,
                label: "Sync disk tags \u{2192} index".into(),
            },
            Self::AcceptDb => ProtocolBinding::Transaction {
                decision_key: DecisionKey::OobSync,
                label: "Sync index tags \u{2192} disk".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
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
