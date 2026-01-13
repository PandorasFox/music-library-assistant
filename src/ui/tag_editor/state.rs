//! Tag Editor State Management
//!
//! Core state structure and conversion functions.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::corpus::db::Track;
use crate::corpus::metadata;

use super::types::{
    DuplicateGroupInfo, FieldEditState, GroupedChange, TagChange, TagEditorFocus, TagField,
};

/// Tag editor navigation state
#[derive(Debug)]
pub struct TagEditorState {
    /// Tracks being edited
    pub tracks: Vec<Track>,
    /// Tag fields for each track (current state)
    pub tag_fields: Vec<Vec<TagField>>,
    /// Original state for change preview
    pub original_tag_fields: Vec<Vec<TagField>>,
    /// Currently selected track index
    pub current_track_idx: usize,
    /// Currently selected field index within track
    pub current_field_idx: usize,
    /// Current edit mode
    pub field_edit_state: FieldEditState,
    /// Buffer for editing tag name
    pub name_buffer: String,
    /// Buffer for editing tag value
    pub value_buffer: String,
    /// Original name before editing started
    pub original_name: String,
    /// Original value before editing started
    pub original_value: String,
    /// Duplicate groups being worked through
    pub duplicate_groups: Vec<DuplicateGroupInfo>,
    /// Current group index if in duplicate workflow
    pub current_group_idx: Option<usize>,
    /// Whether focus is on value (true) or name (false) when editing
    pub focus_on_value: bool,
    /// Scroll offset for tag field list (for songs with many tags)
    pub tag_scroll_offset: usize,
    /// Visible height for tag field area (set during render)
    pub tag_visible_height: usize,
    /// Which pane currently has focus
    pub focus: TagEditorFocus,
}

impl TagEditorState {
    /// Create a new tag editor state for the given tracks
    pub fn new(tracks: Vec<Track>, duplicate_groups: Vec<DuplicateGroupInfo>) -> Self {
        let tag_fields: Vec<Vec<TagField>> = tracks.iter().map(track_to_tag_fields).collect();
        let original_tag_fields = tag_fields.clone();

        Self {
            tracks,
            tag_fields,
            original_tag_fields,
            current_track_idx: 0,
            current_field_idx: 0,
            field_edit_state: FieldEditState::NonEditable,
            name_buffer: String::new(),
            value_buffer: String::new(),
            original_name: String::new(),
            original_value: String::new(),
            duplicate_groups,
            current_group_idx: None,
            focus_on_value: true,
            tag_scroll_offset: 0,
            tag_visible_height: 10, // Default, updated during render
            focus: TagEditorFocus::TagFields,
        }
    }

    /// Create a tag editor state for a duplicate workflow
    pub fn new_for_duplicate_workflow(
        groups: Vec<DuplicateGroupInfo>,
        starting_group_idx: usize,
    ) -> Option<Self> {
        if groups.is_empty() || starting_group_idx >= groups.len() {
            return None;
        }

        let tracks = groups[starting_group_idx].tracks.clone();
        let tag_fields: Vec<Vec<TagField>> = tracks.iter().map(track_to_tag_fields).collect();
        let original_tag_fields = tag_fields.clone();

        Some(Self {
            tracks,
            tag_fields,
            original_tag_fields,
            current_track_idx: 0,
            current_field_idx: 0,
            field_edit_state: FieldEditState::NonEditable,
            name_buffer: String::new(),
            value_buffer: String::new(),
            original_name: String::new(),
            original_value: String::new(),
            duplicate_groups: groups,
            current_group_idx: Some(starting_group_idx),
            focus_on_value: true,
            tag_scroll_offset: 0,
            tag_visible_height: 10,
            focus: TagEditorFocus::TagFields,
        })
    }

    // ========================================================================
    // Navigation
    // ========================================================================

    pub(super) fn move_up(&mut self) {
        if self.field_edit_state != FieldEditState::NonEditable {
            self.commit_current_buffer();
        }
        if self.current_field_idx > 0 {
            self.current_field_idx -= 1;
            // Scroll up if cursor goes above visible area
            if self.current_field_idx < self.tag_scroll_offset {
                self.tag_scroll_offset = self.current_field_idx;
            }
            self.load_buffers();
        }
    }

    pub(super) fn move_down(&mut self) {
        if self.field_edit_state != FieldEditState::NonEditable {
            self.commit_current_buffer();
        }
        let max_fields = self.tag_fields.get(self.current_track_idx).map(|f| f.len()).unwrap_or(0);
        if self.current_field_idx < max_fields.saturating_sub(1) {
            self.current_field_idx += 1;
            // Scroll down if cursor goes below visible area
            // Leave 1 line margin at bottom for visibility
            let visible_end = self.tag_scroll_offset + self.tag_visible_height.saturating_sub(1);
            if self.current_field_idx >= visible_end {
                self.tag_scroll_offset = self.current_field_idx.saturating_sub(self.tag_visible_height.saturating_sub(2));
            }
            self.load_buffers();
        }
    }

    /// Returns true if we're at the last track (signal to show save modal)
    pub(super) fn next_track(&mut self) -> bool {
        if self.field_edit_state != FieldEditState::NonEditable {
            self.commit_current_buffer();
        }
        self.field_edit_state = FieldEditState::NonEditable;

        if self.current_track_idx < self.tracks.len().saturating_sub(1) {
            self.current_track_idx += 1;
            self.current_field_idx = 0;
            self.tag_scroll_offset = 0; // Reset scroll on track change
            self.load_buffers();
            false
        } else {
            // At last track - signal end
            true
        }
    }

    pub(super) fn prev_track(&mut self) {
        if self.field_edit_state != FieldEditState::NonEditable {
            self.commit_current_buffer();
        }
        self.field_edit_state = FieldEditState::NonEditable;

        if self.current_track_idx > 0 {
            self.current_track_idx -= 1;
            self.current_field_idx = 0;
            self.tag_scroll_offset = 0; // Reset scroll on track change
            self.load_buffers();
        }
    }

    // ========================================================================
    // Buffer Management
    // ========================================================================

    pub(super) fn load_buffers(&mut self) {
        if let Some(fields) = self.tag_fields.get(self.current_track_idx) {
            if let Some(field) = fields.get(self.current_field_idx) {
                self.name_buffer = field.name.clone();
                self.value_buffer = field.value.clone();
                self.original_name = field.name.clone();
                self.original_value = field.value.clone();
            }
        }
    }

    pub(super) fn commit_current_buffer(&mut self) {
        if let Some(fields) = self.tag_fields.get_mut(self.current_track_idx) {
            if let Some(field) = fields.get_mut(self.current_field_idx) {
                match self.field_edit_state {
                    FieldEditState::EditingName => {
                        field.name = self.name_buffer.clone();
                    }
                    FieldEditState::EditingValue => {
                        field.value = self.value_buffer.clone();
                    }
                    FieldEditState::NonEditable => {}
                }
            }
        }
    }

    // ========================================================================
    // Editing
    // ========================================================================

    pub(super) fn handle_enter(&mut self) {
        let fields = match self.tag_fields.get(self.current_track_idx) {
            Some(f) => f,
            None => return,
        };
        let field = match fields.get(self.current_field_idx) {
            Some(f) => f,
            None => return,
        };

        if field.name == "New Tag" {
            // Create new tag
            self.create_new_tag();
        } else if self.field_edit_state == FieldEditState::NonEditable {
            // Enter edit mode
            self.field_edit_state = if self.focus_on_value {
                FieldEditState::EditingValue
            } else {
                FieldEditState::EditingName
            };
            self.load_buffers();
        } else {
            // Commit and exit edit mode
            self.commit_current_buffer();
            self.field_edit_state = FieldEditState::NonEditable;
        }
    }

    pub(super) fn create_new_tag(&mut self) {
        let new_field = TagField {
            name: "new_tag".to_string(),
            value: String::new(),
            editable: true,
            is_unique_per_track: false,
        };

        // Insert before "New Tag" placeholder
        if let Some(fields) = self.tag_fields.get_mut(self.current_track_idx) {
            let insert_pos = fields.len().saturating_sub(1);
            fields.insert(insert_pos, new_field);
            self.current_field_idx = insert_pos;
        }

        // Enter edit mode for name
        self.field_edit_state = FieldEditState::EditingName;
        self.name_buffer = "new_tag".to_string();
        self.value_buffer.clear();
        self.focus_on_value = false;
    }

    pub(super) fn insert_char(&mut self, c: char) {
        match self.field_edit_state {
            FieldEditState::EditingName => {
                self.name_buffer.push(c);
            }
            FieldEditState::EditingValue => {
                self.value_buffer.push(c);
            }
            FieldEditState::NonEditable => {}
        }
    }

    pub(super) fn delete_char(&mut self) {
        match self.field_edit_state {
            FieldEditState::EditingName => {
                self.name_buffer.pop();
            }
            FieldEditState::EditingValue => {
                self.value_buffer.pop();
            }
            FieldEditState::NonEditable => {}
        }
    }

    pub(super) fn fill_to_all(&mut self) -> bool {
        let (field_name, field_value, is_unique) = {
            let fields = match self.tag_fields.get(self.current_track_idx) {
                Some(f) => f,
                None => return false,
            };
            let field = match fields.get(self.current_field_idx) {
                Some(f) => f,
                None => return false,
            };
            if field.is_unique_per_track || field.value.is_empty() || field.name == "New Tag" {
                return false;
            }
            (field.name.clone(), field.value.clone(), field.is_unique_per_track)
        };

        if is_unique {
            return false;
        }

        // Apply to all tracks
        for fields in &mut self.tag_fields {
            for field in fields {
                if field.name == field_name && !field.is_unique_per_track {
                    field.value = field_value.clone();
                }
            }
        }

        true
    }

    pub(super) fn clear_current_field(&mut self) {
        match self.field_edit_state {
            FieldEditState::EditingName => {
                self.name_buffer.clear();
            }
            FieldEditState::EditingValue => {
                self.value_buffer.clear();
            }
            FieldEditState::NonEditable => {
                // Clear the field value directly
                if let Some(fields) = self.tag_fields.get_mut(self.current_track_idx) {
                    if let Some(field) = fields.get_mut(self.current_field_idx) {
                        field.value.clear();
                    }
                }
            }
        }
    }

    // ========================================================================
    // Change Tracking
    // ========================================================================

    /// Check if there are any changes to save
    pub fn has_changes(&self) -> bool {
        !compute_changes(&self.original_tag_fields, &self.tag_fields).is_empty()
    }

    /// Get changes for preview
    pub fn get_changes_for_preview(&self) -> (Vec<GroupedChange>, Vec<TagChange>) {
        let changes = compute_changes(&self.original_tag_fields, &self.tag_fields);
        group_common_changes(&changes)
    }
}

// ============================================================================
// Conversion Functions
// ============================================================================

/// Convert a Track to editable tag fields.
///
/// Database-first design: Core fields come from the Track struct (database),
/// extended fields (comment, composer, etc.) come from disk.
///
/// Priority order: track_number, title, artist, album, album_artist, genre, isrc
/// Optimized for compilation tagging workflow (common: track_number -> title -> artist)
/// Read-only fields (path, file_type, duration, bitrate) are NOT included.
pub fn track_to_tag_fields(track: &Track) -> Vec<TagField> {
    let mut tag_fields = Vec::new();

    // =========================================================================
    // CORE FIELDS (from database Track struct)
    // These are the source of truth - database values always shown
    // =========================================================================

    // 1. track_number (unique per track)
    tag_fields.push(TagField {
        name: "track_number".to_string(),
        value: track.track_number.map(|n| n.to_string()).unwrap_or_default(),
        editable: true,
        is_unique_per_track: true,
    });

    // 2. title (unique per track)
    tag_fields.push(TagField {
        name: "title".to_string(),
        value: track.title.clone().unwrap_or_default(),
        editable: true,
        is_unique_per_track: true,
    });

    // 3. artist
    tag_fields.push(TagField {
        name: "artist".to_string(),
        value: track.artist.clone().unwrap_or_default(),
        editable: true,
        is_unique_per_track: false,
    });

    // 4. album
    tag_fields.push(TagField {
        name: "album".to_string(),
        value: track.album.clone().unwrap_or_default(),
        editable: true,
        is_unique_per_track: false,
    });

    // 5. album_artist
    tag_fields.push(TagField {
        name: "album_artist".to_string(),
        value: track.album_artist.clone().unwrap_or_default(),
        editable: true,
        is_unique_per_track: false,
    });

    // 6. genre
    tag_fields.push(TagField {
        name: "genre".to_string(),
        value: track.genre.clone().unwrap_or_default(),
        editable: true,
        is_unique_per_track: false,
    });

    // 7. isrc
    tag_fields.push(TagField {
        name: "isrc".to_string(),
        value: track.isrc.clone().unwrap_or_default(),
        editable: true,
        is_unique_per_track: false,
    });

    // =========================================================================
    // EXTENDED FIELDS (from disk)
    // Tags that exist in the file but aren't in the Track struct
    // =========================================================================
    let extended = load_extended_fields_from_disk(&track.path);
    tag_fields.extend(extended);

    // =========================================================================
    // NEW TAG placeholder
    // =========================================================================
    tag_fields.push(TagField {
        name: "New Tag".to_string(),
        value: "[Press Enter to create]".to_string(),
        editable: true,
        is_unique_per_track: false,
    });

    tag_fields
}

/// Load extended fields from disk that aren't stored in the Track struct.
/// Returns fields sorted alphabetically.
fn load_extended_fields_from_disk(path: &str) -> Vec<TagField> {
    // Core fields we get from database - skip these from disk
    let core_fields: HashSet<&str> = [
        "artist", "album", "album_artist", "title",
        "track_number", "genre", "isrc",
        // Also skip read-only technical fields
        "bitrate", "sample_rate", "duration", "path", "file_type",
    ].into_iter().collect();

    let disk_path = Path::new(path);
    let all_tags = match metadata::read_all_tags(disk_path) {
        Ok(tags) => tags,
        Err(e) => {
            let _ = crate::config::log_message(&format!(
                "Warning: Could not read extended tags from {}: {}",
                path, e
            ));
            return Vec::new();
        }
    };

    // Filter to only extended fields, sort alphabetically
    let mut extended: Vec<_> = all_tags
        .into_iter()
        .filter(|(key, _)| !core_fields.contains(key.as_str()))
        .collect();
    extended.sort_by(|(a, _), (b, _)| a.cmp(b));

    // Convert to TagField
    extended
        .into_iter()
        .map(|(name, value)| TagField {
            name,
            value,
            editable: true,
            is_unique_per_track: false,
        })
        .collect()
}

/// Compute all changes between original and current tag fields
pub fn compute_changes(original: &[Vec<TagField>], current: &[Vec<TagField>]) -> Vec<TagChange> {
    let mut changes = Vec::new();

    for (track_idx, (orig_fields, curr_fields)) in original.iter().zip(current.iter()).enumerate() {
        // Compare field by field
        for (orig_field, curr_field) in orig_fields.iter().zip(curr_fields.iter()) {
            // Skip "New Tag" placeholder
            if orig_field.name == "New Tag" || curr_field.name == "New Tag" {
                continue;
            }

            // Detect changes in value
            if orig_field.value != curr_field.value {
                changes.push(TagChange {
                    track_idx,
                    field_name: curr_field.name.clone(),
                    old_value: orig_field.value.clone(),
                    new_value: curr_field.value.clone(),
                });
            }
        }
    }

    changes
}

/// Group changes that are identical across multiple tracks
pub fn group_common_changes(changes: &[TagChange]) -> (Vec<GroupedChange>, Vec<TagChange>) {
    // Group by (field_name, old_value, new_value)
    let mut groups: HashMap<(String, String, String), Vec<usize>> = HashMap::new();

    for change in changes {
        let key = (
            change.field_name.clone(),
            change.old_value.clone(),
            change.new_value.clone(),
        );
        groups.entry(key).or_default().push(change.track_idx);
    }

    // Split into grouped (2+ tracks) and single-track changes
    let mut grouped = Vec::new();
    let mut singles = Vec::new();

    for ((field_name, old_value, new_value), track_indices) in groups {
        if track_indices.len() > 1 {
            grouped.push(GroupedChange {
                field_name,
                old_value,
                new_value,
                track_indices,
            });
        } else {
            // Find the original TagChange for this single track
            let track_idx = track_indices[0];
            if let Some(change) = changes
                .iter()
                .find(|c| c.track_idx == track_idx && c.field_name == field_name)
            {
                singles.push(change.clone());
            }
        }
    }

    (grouped, singles)
}
