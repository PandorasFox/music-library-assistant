//! Tag Editor Types
//!
//! Core type definitions for the tag editor workflow.

use crate::db::Track;

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
