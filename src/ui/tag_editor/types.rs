//! Tag Editor Types
//!
//! Core type definitions for the tag editor workflow.

use crate::corpus::db::Track;

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

/// Which pane currently has focus in the tag editor
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TagEditorFocus {
    /// Track list pane (left)
    TrackList,
    /// Tag fields pane (middle)
    #[default]
    TagFields,
    /// Action pane (right) - "Proceed" button
    ActionPane,
}

/// A single tag field with its metadata
#[derive(Debug, Clone)]
pub struct TagField {
    pub name: String,
    pub value: String,
    pub editable: bool,
    /// Fields like title/track_number are unique per track and cannot be filled to all
    pub is_unique_per_track: bool,
}

/// A single change to a tag field
#[derive(Debug, Clone)]
pub struct TagChange {
    pub track_idx: usize,
    pub field_name: String,
    pub old_value: String,
    pub new_value: String,
}

/// Multiple tracks with the same change (for preview display)
#[derive(Debug, Clone)]
pub struct GroupedChange {
    pub field_name: String,
    pub old_value: String,
    pub new_value: String,
    pub track_indices: Vec<usize>,
}

/// Information about a duplicate group being resolved
#[derive(Debug, Clone)]
pub struct DuplicateGroupInfo {
    pub group_id: i64,
    pub tracks: Vec<Track>,
    pub resolved: bool,
}

/// Type of duplicate (for legacy workflow)
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum DuplicateGroupType {
    ExactMatch,    // Same file in multiple locations
    MetadataMatch, // Same metadata, different files
}

/// Modal states for tag editor
#[derive(Debug)]
pub enum TagEditorModal {
    /// Save confirmation modal (shown when tabbing past last track)
    SaveConfirmation {
        selected_button: usize, // 0 = Save All, 1 = Save & Next, 2 = Return
    },
    /// Change preview modal (shown before saving)
    ChangePreview {
        grouped_changes: Vec<GroupedChange>,
        single_changes: Vec<TagChange>,
        scroll_offset: usize,
        save_and_next: bool,
    },
}

/// Result of handling a key press in tag editor
#[derive(Debug)]
pub enum TagEditorAction {
    /// No action, continue in tag editor
    None,
    /// Show a modal
    ShowModal(TagEditorModal),
    /// Save all changes and exit
    SaveAll,
    /// Save and advance to next duplicate group
    SaveAndNext,
    /// Exit without saving
    Exit,
    /// Update status message
    StatusMessage(String),
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

/// Which pane has focus in the directory tag editor
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DirectoryTagEditorFocus {
    /// Tag fields pane (middle)
    #[default]
    TagFields,
    /// Action pane (right) - "Proceed" button
    ActionPane,
}

/// Modal states for directory tag editor
#[derive(Debug)]
pub enum DirectoryTagEditorModal {
    /// Change preview modal (shown before saving)
    ChangePreview {
        scroll_offset: usize,
        save_and_next: bool,
    },
    /// Unsaved changes prompt when switching directories
    UnsavedChanges {
        /// Direction: true = next sibling, false = previous sibling
        going_next: bool,
    },
}

/// Result of handling a key press in directory tag editor
#[derive(Debug)]
pub enum DirectoryTagEditorAction {
    /// No action, continue in editor
    None,
    /// Show a modal
    ShowModal(DirectoryTagEditorModal),
    /// Save all changes to files
    SaveAll,
    /// Save and advance to next sibling directory
    SaveAndNext,
    /// Exit without saving
    Exit,
    /// Update status message
    StatusMessage(String),
    /// Switch to sibling directory (true = next, false = prev)
    SwitchDirectory(bool),
}

/// Progress message for metadata gathering
#[derive(Debug, Clone)]
pub enum GatheringMessage {
    /// Found total number of files
    TotalFiles(usize),
    /// Processing a file (index, path)
    Processing(usize, String),
    /// File processed with tags
    FileProcessed {
        index: usize,
        path: String,
        tags: Vec<(String, String)>,
    },
    /// Error processing file
    FileError {
        index: usize,
        path: String,
        error: String,
    },
    /// All files processed
    Complete,
}
