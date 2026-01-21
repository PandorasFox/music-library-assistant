//! Tag Editor Types
//!
//! Core type definitions for the tag editor workflow.

use crate::corpus::db::Track;
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
    /// From duplicate detection flow
    DuplicateResolution,
    /// From deploy conflict resolution
    DeployConflict,
    /// From tag search results
    TagSearch,
}

impl TagEditorSource {
    /// Get the transaction label for this source.
    pub fn transaction_label(&self) -> &'static str {
        match self {
            Self::CorpusBrowser => "Tag edits",
            Self::DirectoryEdit => "Directory tag edits",
            Self::DuplicateResolution => "Duplicate resolution",
            Self::DeployConflict => "Deploy conflict resolution",
            Self::TagSearch => "Tag search edits",
        }
    }
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
        track: Track,
        source: TagEditorSource,
        group_context: Option<GroupContext>,
    },
    /// Bulk editing with aggregated values
    BulkEdit {
        tracks: Vec<Track>,
        source: TagEditorSource,
        group_context: Option<GroupContext>,
    },
}

/// Group context for multi-step workflows (duplicate resolution, deploy conflicts)
#[derive(Debug, Clone)]
pub struct GroupContext {
    pub source: TagEditorSource,
    pub group_index: usize,
    pub total_groups: usize,
}

/// Unified focus enum for the tag editor
///
/// Note: ContextList (left pane) is display-only and not focusable.
/// Tab/Shift-Tab navigate between siblings.
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
    /// Stage mutations for current item to the active transaction
    StageDecision { index: usize, mutations: Vec<Mutation> },
    /// Stage decision AND navigate to next sibling (combines stage + navigation)
    StageDecisionAndNext { index: usize, mutations: Vec<Mutation> },
    /// Stage decision AND show transaction review (for aggregated mode or single-sibling contexts)
    StageDecisionAndReview { index: usize, mutations: Vec<Mutation> },
    /// Commit all staged decisions and exit
    CommitTransaction,
    /// Discard all staged decisions and exit
    DiscardTransaction,
    /// Navigate to next item (within current transaction)
    NextItem,
    /// Navigate to previous item
    PrevItem,
    /// Navigate to next sibling (directory in DirectoryEdit, track in bulk edit)
    NextSibling,
    /// Navigate to previous sibling
    PrevSibling,
    /// Show an overlay modal
    ShowModal(UnifiedTagEditorModal),
    /// Close the current modal
    CloseModal,
    /// Display a status message
    StatusMessage(String),
    /// Request to fill tags from database (requires DB access from UI layer)
    /// track_id is None if the track hasn't been indexed yet
    RequestFillFromDb { track_id: Option<i64> },
    /// Request to show transaction review (requires daemon access to populate decisions)
    RequestTransactionReview,
}

/// Modal dialogs for the unified tag editor
#[derive(Debug)]
pub enum UnifiedTagEditorModal {
    /// Preview changes for current item before staging
    ChangePreview {
        changes: Vec<GroupedChange>,
        single_changes: Vec<TagChange>,
        scroll: usize,
    },
    /// Warn about unsaved changes when navigating away
    UnsavedChanges {
        /// Where the user is trying to go
        destination: UnsavedChangesDestination,
        /// Selected button (defaults to KeepEditing for safety)
        selected_button: UnsavedChangesButton,
    },
    /// Review all staged decisions before final commit
    TransactionReview {
        /// (index, label, mutation_count)
        decisions: Vec<(usize, String, usize)>,
        scroll: usize,
        selected_button: TransactionReviewButton,
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

/// Where the user is trying to navigate when unsaved changes exist
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsavedChangesDestination {
    /// Exiting the tag editor entirely
    Exit,
    /// Moving to next item
    NextItem,
    /// Moving to previous item
    PrevItem,
    /// Moving to next sibling (directory or track based on mode)
    NextSibling,
    /// Moving to previous sibling
    PrevSibling,
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

/// Buttons on the transaction review modal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransactionReviewButton {
    /// Commit all staged decisions
    CommitAll,
    /// Discard all staged decisions
    DiscardAll,
    /// Go back to editing (safe default - stuck Enter won't commit)
    #[default]
    BackToEditing,
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
    /// Fields like title/track_number are unique per track and cannot be filled to all
    pub is_unique_per_track: bool,
    /// Marked for deletion (shown with strikethrough)
    pub deleted: bool,
}

/// A tag field that may have multiple values (FLAC/Vorbis style).
/// Used for coalescing duplicate tag names into a single editable unit.
#[derive(Debug, Clone)]
pub struct CoalescedTagField {
    /// Display name (uses casing from first occurrence)
    pub name: String,
    /// Normalized name for comparison (lowercase)
    pub normalized_name: String,
    /// All values for this tag (may be empty after deletions)
    pub values: Vec<String>,
    pub editable: bool,
    /// Marked for deletion (all values will be removed)
    pub deleted: bool,
    /// Original values for change detection
    pub original_values: Vec<String>,
}

impl CoalescedTagField {
    /// Display value shown in the tag list
    pub fn display_value(&self) -> String {
        match self.values.len() {
            0 => "(empty)".to_string(),
            1 => self.values[0].clone(),
            n => format!("[{} values]", n),
        }
    }

    /// Check if field has been modified from original
    pub fn is_modified(&self) -> bool {
        self.values != self.original_values || self.deleted
    }
}

/// A single change to a tag field
#[derive(Debug, Clone)]
pub struct TagChange {
    pub track_idx: usize,
    pub field_name: String,
    pub old_value: String,
    pub new_value: String,
    /// Original tag name if renamed (e.g., "trackNumber" -> "track_number")
    pub old_name: Option<String>,
    /// Tag marked for deletion
    pub deleted: bool,
}

/// Multiple tracks with the same change (for preview display)
#[derive(Debug, Clone)]
pub struct GroupedChange {
    pub field_name: String,
    pub old_value: String,
    pub new_value: String,
    pub track_indices: Vec<usize>,
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

/// Confirmation state for editing "(various values)" fields
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariousConfirmState {
    /// First Enter pressed - showing "you sure?"
    Confirming,
    /// Second Enter pressed - now in edit mode
    Editing,
}

/// An aggregated tag field representing the same field across all files
#[derive(Debug, Clone)]
pub struct AggregatedTagField {
    pub name: String,
    pub value: AggregatedValue,
    pub original_value: AggregatedValue,
    pub editable: bool,
}

