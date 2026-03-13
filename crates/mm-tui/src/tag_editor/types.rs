//! Tag Editor Types
//!
//! Core type definitions for the tag editor workflow.

use mm_meta::db_types::AudioFile;
use mm_meta::decisions::DecisionKey;
use mm_meta::mutations::Mutation;
use crate::widgets::TextInputState;

// ============================================================================
// Unified Types (New)
// ============================================================================

/// Source modal that spawned the tag editor (for navigation context)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagEditorSource {
    /// Single file from corpus browser
    CorpusBrowser,
    /// Bulk edit from directory selection
    DirectoryEdit,
    /// From tag search results
    TagSearch,
    /// Embedded within a health modal (tag canonicity, compound split, etc.)
    HealthModal,
}

/// Launch mode for the tag editor — standalone (owns transaction) or embedded (parent owns transaction).
#[derive(Debug, Clone)]
pub enum TagEditorLaunchMode {
    /// Normal standalone mode — tag editor owns the transaction lifecycle.
    Standalone,
    /// Embedded within a health modal — parent owns the transaction.
    /// Changes are collected and staged as a single decision at the parent's decision key.
    Embedded {
        decision_key: DecisionKey,
        decision_label: String,
    },
}

/// Editing mode for the tag editor
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TagEditorMode {
    /// Edit tracks one at a time (Tab to navigate between tracks)
    #[default]
    Individual,
    /// Edit aggregated view (changes apply to all tracks)
    /// Shows "(various)" when tracks have different values for a tag
    Aggregated,
}

/// Context for tag editing - determines mode and available features
#[derive(Debug, Clone)]
pub enum TagEditContext {
    /// Single file editing with signals display
    SingleFile {
        audio_file: AudioFile,
        source: TagEditorSource,
        group_context: Option<GroupContext>,
    },
    /// Bulk editing with aggregated values
    BulkEdit {
        audio_files: Vec<AudioFile>,
        source: TagEditorSource,
    },
}

/// Group context for multi-step workflows (duplicate resolution, deploy conflicts)
#[derive(Debug, Clone)]
pub struct GroupContext {
    pub group_index: usize,
    pub total_groups: usize,
}

/// Unified focus enum for the tag editor
///
/// Note: ContextList (left pane) is display-only and not focusable.
/// Tab/Shift-Tab navigate between items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnifiedTagEditorFocus {
    /// Tag fields pane (center) - default
    #[default]
    TagFields,
    /// Actions pane (right)
    Actions,
}

/// Action buttons in the unified tag editor
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TagEditorButton {
    /// Review all staged decisions (opens transaction review)
    #[default]
    ReviewAll,
    /// Revert changes for current file (revert to original)
    RevertThisFile,
    /// Fill from disk (when OOB signal present)
    FillFromDisk,
    /// Fill from database (when OOB signal present)
    FillFromDb,
}

/// Unified action result - transaction-based
#[derive(Debug)]
pub enum UnifiedTagEditorAction {
    /// No action
    None,
    /// Stage decision AND navigate (confirmation modal approved staging)
    StageDecisionAndNavigate {
        key: DecisionKey,
        mutations: Vec<Mutation>,
        direction: NavigationDirection,
    },
    /// Stage decision AND show transaction review (for aggregated mode or single-item contexts)
    StageDecisionAndReview {
        key: DecisionKey,
        mutations: Vec<Mutation>,
    },
    /// Discard all staged decisions and exit
    DiscardTransaction,
    /// Navigate to next item (within current transaction)
    NextItem,
    /// Navigate to previous item
    PrevItem,
    /// Close the current modal
    CloseModal,
    /// Display a status message
    StatusMessage(String),
    /// Request to fill tags from database (requires DB access from UI layer)
    /// inode is None if the file hasn't been indexed yet
    RequestFillFromDb { inode: Option<i64> },
    /// Request to re-read tags from disk (requires domain query from UI layer)
    RequestFillFromDisk,
    /// Request to show transaction review (requires daemon access to populate decisions)
    RequestTransactionReview,
    /// Close embedded tag editor without staging (Esc from embedded mode)
    CloseEmbedded,
    /// Stage collected mutations at parent's decision key and close embedded editor
    StageAndCloseEmbedded {
        decision_key: DecisionKey,
        decision_label: String,
        mutations: Vec<Mutation>,
    },
}

/// Modal dialogs for the unified tag editor
#[derive(Debug)]
pub enum UnifiedTagEditorModal {
    /// Warn about unsaved changes when trying to exit
    UnsavedChanges {
        /// Selected button (defaults to KeepEditing for safety)
        selected_button: UnsavedChangesButton,
    },
    /// Edit multiple values for a single tag (FLAC/Vorbis multi-value support)
    MultiValueEditor {
        /// Index of the field in the tag_fields list
        field_idx: usize,
        /// All values for this tag
        values: Vec<String>,
        /// Currently selected value index (includes "+ Add value" entry at end)
        current_value_idx: usize,
        /// Whether we're currently editing a value
        editing: bool,
        /// Text input state for the current value
        edit_input: TextInputState,
    },
    /// Confirm staging changes before Tab/Shift-Tab navigation.
    ///
    /// Shown when the user presses Tab/Shift-Tab with unsaved changes.
    /// Requires an explicit Enter keypress to stage the decision (witness provenance).
    StageChangesConfirm {
        /// Which direction to navigate after
        direction: NavigationDirection,
        /// Selected button (defaults to Yes for easy single-Enter confirmation)
        selected_button: StageChangesButton,
    },
}

/// Buttons on the unsaved changes modal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnsavedChangesButton {
    /// Keep editing (safe default - stuck Enter won't discard)
    #[default]
    KeepEditing,
    /// Discard changes and proceed with navigation
    DiscardAndProceed,
}

/// Direction for Tab/Shift-Tab navigation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationDirection {
    Next,
    Prev,
}

/// Buttons on the stage-changes confirmation modal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StageChangesButton {
    /// Stage changes and navigate (safe default - easy single Enter)
    #[default]
    Yes,
    /// Skip staging, navigate anyway (edits remain in local state)
    No,
    /// Cancel - stay on current file
    Cancel,
}

// ============================================================================
// Legacy Types (to be removed after refactoring)
// ============================================================================

/// Edit mode for tag fields
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldEditState {
    /// Field is not being edited (navigation mode)
    NonEditable,
    /// User is editing the tag name
    EditingName,
    /// User is editing the tag value
    EditingValue,
}

/// A single tag field with its metadata
#[derive(Debug, Clone, PartialEq)]
pub struct TagField {
    pub name: String,
    pub value: String,
    pub editable: bool,
    /// Marked for deletion (shown with strikethrough)
    pub deleted: bool,
}

/// A single change to a tag field
#[derive(Debug, Clone)]
pub struct TagChange {
    pub track_idx: usize,
    pub field_name: String,
    pub old_value: String,
    pub new_value: String,
}

// ============================================================================
// Directory Tag Editor Types
// ============================================================================

/// Value state for an aggregated tag field across multiple files
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggregatedValue {
    /// All files have the same value - directly editable
    Consistent(String),
    /// Files have different values - shown as "(various values)", needs two-phase Enter
    Various,
    /// User is confirming they want to overwrite various values
    VariousConfirming,
    /// User has entered a new value that will fill to all files
    Edited(String),
}

/// An aggregated tag field representing the same field across all files
#[derive(Debug, Clone)]
pub struct AggregatedTagField {
    pub name: String,
    pub value: AggregatedValue,
    pub original_value: AggregatedValue,
}
