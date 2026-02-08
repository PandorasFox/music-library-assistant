//! Tag Editor Types
//!
//! Core type definitions for the tag editor workflow.

use crate::corpus::db::types::AudioFile;
use crate::corpus::mutations::Mutation;

// ============================================================================
// Unified Types (New)
// ============================================================================

/// Source flow that spawned the tag editor (for navigation context)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagEditorSource {
    /// Single file from corpus browser
    CorpusBrowser,
    /// Bulk edit from directory selection
    DirectoryEdit,
    /// From tag search results
    TagSearch,
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
    /// Confirm changes and stage to transaction
    #[default]
    Confirm,
    /// Drop changes for current item (revert to original)
    DropChanges,
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
    /// Stage decision AND navigate to next item (Tab with changes)
    StageDecisionAndNext { index: usize, mutations: Vec<Mutation> },
    /// Stage decision AND navigate to previous item (Shift-Tab with changes)
    StageDecisionAndPrev { index: usize, mutations: Vec<Mutation> },
    /// Stage decision AND show transaction review (for aggregated mode or single-item contexts)
    StageDecisionAndReview { index: usize, mutations: Vec<Mutation> },
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
    /// Request to show transaction review (requires daemon access to populate decisions)
    RequestTransactionReview,
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
        /// Edit buffer for the current value
        edit_buffer: String,
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
#[derive(Debug, Clone)]
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

