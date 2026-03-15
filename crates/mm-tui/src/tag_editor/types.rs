//! Tag Editor Types
//!
//! TUI-specific type definitions for the tag editor workflow.
//! Shared types (TagSet, FieldFormState, TagEditorState) live in mm-ui.

use mm_meta::decisions::DecisionKey;
use mm_meta::mutations::Mutation;
use crate::widgets::TextInputState;

// ============================================================================
// Source & Context
// ============================================================================

/// Source modal that spawned the tag editor (for navigation context).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagEditorSource {
    /// Single file from corpus browser
    CorpusBrowser,
    /// Bulk edit from directory selection
    DirectoryEdit,
    /// From tag search results
    TagSearch,
    /// Embedded within a health modal
    HealthModal,
}

/// Group context for multi-step workflows (duplicate resolution, deploy conflicts).
#[derive(Debug, Clone)]
pub struct GroupContext {
    pub group_index: usize,
    pub total_groups: usize,
}

/// Launch mode — standalone (owns transaction) or embedded (parent owns).
#[derive(Debug, Clone)]
pub enum TagEditorLaunchMode {
    Standalone,
    Embedded {
        decision_key: DecisionKey,
        decision_label: String,
    },
}

// ============================================================================
// Actions
// ============================================================================

/// Action result from input handling.
#[derive(Debug)]
pub enum UnifiedTagEditorAction {
    /// No action.
    None,
    /// Stage decision AND navigate.
    StageDecisionAndNavigate {
        key: DecisionKey,
        mutations: Vec<Mutation>,
        direction: NavigationDirection,
    },
    /// Stage decision AND show review.
    StageDecisionAndReview {
        key: DecisionKey,
        mutations: Vec<Mutation>,
    },
    /// Discard all staged decisions and exit.
    DiscardTransaction,
    /// Navigate to next item.
    NextItem,
    /// Navigate to previous item.
    PrevItem,
    /// Close the current modal.
    CloseModal,
    /// Display a status message.
    StatusMessage(String),
    /// Request to fill tags from database.
    RequestFillFromDb { inode: Option<i64> },
    /// Request to re-read tags from disk.
    RequestFillFromDisk,
    /// Request to show transaction review.
    RequestTransactionReview,
    /// Close embedded editor without staging.
    CloseEmbedded,
    /// Stage and close embedded editor.
    StageAndCloseEmbedded {
        decision_key: DecisionKey,
        decision_label: String,
        mutations: Vec<Mutation>,
    },
}

// ============================================================================
// Modals
// ============================================================================

/// Modal dialogs for the tag editor.
#[derive(Debug)]
pub enum UnifiedTagEditorModal {
    /// Warn about unsaved changes when trying to exit.
    UnsavedChanges {
        selected_button: UnsavedChangesButton,
    },
    /// Edit multiple values for a single tag (Z-key expansion).
    MultiValueEditor {
        field_idx: usize,
        values: Vec<String>,
        current_value_idx: usize,
        editing: bool,
        edit_input: TextInputState,
    },
    /// Confirm staging changes before Tab/Shift-Tab navigation.
    StageChangesConfirm {
        direction: NavigationDirection,
        selected_button: StageChangesButton,
    },
}

/// Buttons on the unsaved changes modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnsavedChangesButton {
    #[default]
    KeepEditing,
    DiscardAndProceed,
}

/// Direction for Tab/Shift-Tab navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationDirection {
    Next,
    Prev,
}

/// Buttons on the stage-changes confirmation modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StageChangesButton {
    #[default]
    Yes,
    No,
    Cancel,
}
