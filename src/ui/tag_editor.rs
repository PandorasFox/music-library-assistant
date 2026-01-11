//! Tag Editor UI
//!
//! Provides a multi-track metadata editing workflow with:
//! - 3-column layout: track list, tag editor, search panel
//! - Support for duplicate resolution workflow
//! - Fill-to-all functionality for shared fields
//! - Change preview before saving

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::db::Track;
use crate::metadata;

// ============================================================================
// Types
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
// TagField Conversion
// ============================================================================

/// Convert a Track to editable tag fields.
///
/// Priority order: track_number, title, artist, album, album_artist, date, genre, isrc
/// Optimized for compilation tagging workflow (common: track_number -> title -> artist)
/// Read-only fields (path, file_type, duration, bitrate) are NOT included.
pub fn track_to_tag_fields(track: &Track) -> Vec<TagField> {
    let path = Path::new(&track.path);

    // Try to read all tags from the file (as Vec to preserve duplicates)
    let all_tags_vec: Vec<(String, String)> = match metadata::read_all_tags(path) {
        Ok(tags) => tags,
        Err(_) => {
            // If reading fails, fall back to database fields only
            let mut vec = Vec::new();
            if let Some(ref artist) = track.artist {
                vec.push(("artist".to_string(), artist.clone()));
            }
            if let Some(ref album) = track.album {
                vec.push(("album".to_string(), album.clone()));
            }
            if let Some(ref album_artist) = track.album_artist {
                vec.push(("album_artist".to_string(), album_artist.clone()));
            }
            if let Some(ref title) = track.title {
                vec.push(("title".to_string(), title.clone()));
            }
            if let Some(track_num) = track.track_number {
                vec.push(("track_number".to_string(), track_num.to_string()));
            }
            if let Some(ref isrc) = track.isrc {
                vec.push(("isrc".to_string(), isrc.clone()));
            }
            vec
        }
    };

    let mut tag_fields = Vec::new();

    // Priority fields in specific order (name, is_unique_per_track)
    // Optimized for compilation tagging: track_number -> title -> artist
    let priority_fields = vec![
        ("track_number", true), // Unique per track
        ("title", true),        // Unique per track
        ("artist", false),
        ("album", false),
        ("album_artist", false), // Can have multiple values
        ("date", false),
        ("genre", false),
        ("isrc", false),
    ];

    // Add priority fields first (handle album_artist specially for multiple values)
    for (field_name, is_unique) in &priority_fields {
        let matching_values: Vec<String> = all_tags_vec
            .iter()
            .filter(|(k, _)| k == field_name)
            .map(|(_, v)| v.clone())
            .collect();

        if field_name == &"album_artist" {
            // Allow multiple album_artist entries
            if matching_values.is_empty() {
                // Add one empty field if none exist
                tag_fields.push(TagField {
                    name: field_name.to_string(),
                    value: String::new(),
                    editable: true,
                    is_unique_per_track: *is_unique,
                });
            } else {
                // Add all existing album_artist values
                for value in matching_values {
                    tag_fields.push(TagField {
                        name: field_name.to_string(),
                        value,
                        editable: true,
                        is_unique_per_track: *is_unique,
                    });
                }
            }
        } else {
            // Single value for other fields (use first if multiple exist)
            if *is_unique && matching_values.len() > 1 {
                let _ = crate::config::log_message(&format!(
                    "Warning: Multiple {} tags found ({}), using first value",
                    field_name,
                    matching_values.len()
                ));
            }
            tag_fields.push(TagField {
                name: field_name.to_string(),
                value: matching_values.first().cloned().unwrap_or_default(),
                editable: true,
                is_unique_per_track: *is_unique,
            });
        }
    }

    // Add other tags alphabetically (excluding priority fields and read-only fields)
    let skip_fields: HashSet<&str> = priority_fields
        .iter()
        .map(|(name, _)| *name)
        .chain(
            ["bitrate", "sample_rate", "duration", "path", "file_type"]
                .iter()
                .copied(),
        )
        .collect();

    let mut other_tags: Vec<_> = all_tags_vec
        .iter()
        .filter(|(key, _)| !skip_fields.contains(key.as_str()))
        .collect();
    other_tags.sort_by_key(|(key, _)| key.as_str());

    for (key, value) in other_tags {
        tag_fields.push(TagField {
            name: key.clone(),
            value: value.clone(),
            editable: true,
            is_unique_per_track: false,
        });
    }

    // Add "New Tag" line as last interactable field
    tag_fields.push(TagField {
        name: "New Tag".to_string(),
        value: "[Press Enter to create]".to_string(),
        editable: true,
        is_unique_per_track: false,
    });

    tag_fields
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

// ============================================================================
// TagEditorState Implementation
// ============================================================================

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
        })
    }

    /// Handle a key event
    pub fn handle_key(&mut self, key: KeyEvent) -> TagEditorAction {
        match key.code {
            KeyCode::Esc => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    // Exit edit mode, restore original values
                    self.field_edit_state = FieldEditState::NonEditable;
                    TagEditorAction::None
                } else {
                    TagEditorAction::Exit
                }
            }
            KeyCode::Up => {
                self.move_up();
                TagEditorAction::None
            }
            KeyCode::Down => {
                self.move_down();
                TagEditorAction::None
            }
            KeyCode::Left => {
                self.focus_on_value = false;
                TagEditorAction::None
            }
            KeyCode::Right => {
                self.focus_on_value = true;
                TagEditorAction::None
            }
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.prev_track();
                } else {
                    let at_end = self.next_track();
                    if at_end {
                        // Tabbing past last track triggers save modal
                        return TagEditorAction::ShowModal(TagEditorModal::SaveConfirmation {
                            selected_button: 0,
                        });
                    }
                }
                TagEditorAction::None
            }
            KeyCode::Enter => {
                self.handle_enter();
                TagEditorAction::None
            }
            KeyCode::Char('f') | KeyCode::Char('F') => {
                if self.fill_to_all() {
                    TagEditorAction::StatusMessage("Value copied to all tracks".to_string())
                } else {
                    TagEditorAction::None
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.clear_current_field();
                TagEditorAction::None
            }
            KeyCode::Char(c) => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    self.insert_char(c);
                }
                TagEditorAction::None
            }
            KeyCode::Backspace => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    self.delete_char();
                }
                TagEditorAction::None
            }
            _ => TagEditorAction::None,
        }
    }

    fn move_up(&mut self) {
        if self.field_edit_state != FieldEditState::NonEditable {
            self.commit_current_buffer();
        }
        if self.current_field_idx > 0 {
            self.current_field_idx -= 1;
            self.load_buffers();
        }
    }

    fn move_down(&mut self) {
        if self.field_edit_state != FieldEditState::NonEditable {
            self.commit_current_buffer();
        }
        let max_fields = self.tag_fields.get(self.current_track_idx).map(|f| f.len()).unwrap_or(0);
        if self.current_field_idx < max_fields.saturating_sub(1) {
            self.current_field_idx += 1;
            self.load_buffers();
        }
    }

    fn next_track(&mut self) -> bool {
        if self.field_edit_state != FieldEditState::NonEditable {
            self.commit_current_buffer();
        }
        self.field_edit_state = FieldEditState::NonEditable;

        if self.current_track_idx < self.tracks.len().saturating_sub(1) {
            self.current_track_idx += 1;
            self.current_field_idx = 0;
            self.load_buffers();
            false
        } else {
            // At last track - signal end
            true
        }
    }

    fn prev_track(&mut self) {
        if self.field_edit_state != FieldEditState::NonEditable {
            self.commit_current_buffer();
        }
        self.field_edit_state = FieldEditState::NonEditable;

        if self.current_track_idx > 0 {
            self.current_track_idx -= 1;
            self.current_field_idx = 0;
            self.load_buffers();
        }
    }

    fn handle_enter(&mut self) {
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

    fn create_new_tag(&mut self) {
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

    fn load_buffers(&mut self) {
        if let Some(fields) = self.tag_fields.get(self.current_track_idx) {
            if let Some(field) = fields.get(self.current_field_idx) {
                self.name_buffer = field.name.clone();
                self.value_buffer = field.value.clone();
                self.original_name = field.name.clone();
                self.original_value = field.value.clone();
            }
        }
    }

    fn commit_current_buffer(&mut self) {
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

    fn insert_char(&mut self, c: char) {
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

    fn delete_char(&mut self) {
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

    fn fill_to_all(&mut self) -> bool {
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

    fn clear_current_field(&mut self) {
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

    /// Check if there are any changes to save
    pub fn has_changes(&self) -> bool {
        !compute_changes(&self.original_tag_fields, &self.tag_fields).is_empty()
    }

    /// Get changes for preview
    pub fn get_changes_for_preview(&self) -> (Vec<GroupedChange>, Vec<TagChange>) {
        let changes = compute_changes(&self.original_tag_fields, &self.tag_fields);
        group_common_changes(&changes)
    }

    /// Render the tag editor
    pub fn render(&mut self, f: &mut Frame, area: Rect, status_message: Option<&str>) {
        // Layout: info pane | 3-column | status box
        let editor_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(5),  // Info pane
                Constraint::Min(15),    // 3-column area
                Constraint::Length(12), // Status box
            ])
            .split(area);

        self.render_info_pane(f, editor_layout[0]);
        self.render_three_column(f, editor_layout[1]);
        self.render_status_box(f, editor_layout[2], status_message);
    }

    fn render_info_pane(&self, f: &mut Frame, area: Rect) {
        let current_track = &self.tracks[self.current_track_idx];

        // Format duration
        let duration_str = if let Some(duration_ms) = current_track.duration_ms {
            let total_seconds = duration_ms / 1000;
            let minutes = total_seconds / 60;
            let seconds = total_seconds % 60;
            format!("{}:{:02}", minutes, seconds)
        } else {
            "Unknown".to_string()
        };

        // Format file size
        let size_str = if current_track.file_size < 1024 {
            format!("{} B", current_track.file_size)
        } else if current_track.file_size < 1024 * 1024 {
            format!("{:.1} KB", current_track.file_size as f64 / 1024.0)
        } else {
            format!(
                "{:.2} MB",
                current_track.file_size as f64 / (1024.0 * 1024.0)
            )
        };

        let sample_rate_str = current_track
            .sample_rate
            .map(|sr| format!("{} Hz", sr))
            .unwrap_or_else(|| "Unknown".to_string());

        let bitrate_str = current_track
            .bitrate_kbps
            .map(|b| format!("{} kbps", b))
            .unwrap_or_else(|| "Unknown".to_string());

        let info_lines = vec![
            Line::from(format!("Path: {}", current_track.path)),
            Line::from(format!(
                "Format: {} | Size: {} | Duration: {} | Bitrate: {} | Sample Rate: {}",
                current_track.file_type.to_uppercase(),
                size_str,
                duration_str,
                bitrate_str,
                sample_rate_str
            )),
            Line::from(format!(
                "Source: {} | Inode: {}",
                current_track.source, current_track.inode
            )),
        ];

        let info_para = Paragraph::new(info_lines).block(
            Block::default().borders(Borders::ALL).title(format!(
                "File Info [Track {}/{}]",
                self.current_track_idx + 1,
                self.tracks.len()
            )),
        );
        f.render_widget(info_para, area);
    }

    fn render_three_column(&self, f: &mut Frame, area: Rect) {
        let three_column = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(30), // Track list
                Constraint::Percentage(60), // Tag editor
                Constraint::Percentage(10), // Search panel
            ])
            .split(area);

        self.render_track_list(f, three_column[0]);
        self.render_tag_fields(f, three_column[1]);
        self.render_search_panel(f, three_column[2]);
    }

    fn render_track_list(&self, f: &mut Frame, area: Rect) {
        let track_items: Vec<ListItem> = self
            .tracks
            .iter()
            .enumerate()
            .map(|(idx, track)| {
                let prefix = if idx == self.current_track_idx {
                    ">> "
                } else {
                    "   "
                };
                let artist = track.artist.as_deref().unwrap_or("Unknown");
                let title = track.title.as_deref().unwrap_or("Unknown");
                ListItem::new(Line::from(format!("{}{} - {}", prefix, artist, title))).style(
                    if idx == self.current_track_idx {
                        Style::default().bg(Color::DarkGray)
                    } else {
                        Style::default()
                    },
                )
            })
            .collect();

        let track_list =
            List::new(track_items).block(Block::default().borders(Borders::ALL).title("Tracks"));
        f.render_widget(track_list, area);
    }

    fn render_tag_fields(&self, f: &mut Frame, area: Rect) {
        let fields = &self.tag_fields[self.current_track_idx];
        let field_lines: Vec<Line> = fields
            .iter()
            .enumerate()
            .map(|(idx, field)| {
                let is_current = idx == self.current_field_idx;

                let name_display =
                    if is_current && matches!(self.field_edit_state, FieldEditState::EditingName) {
                        self.name_buffer.clone() + "_"
                    } else {
                        field.name.clone()
                    };

                let value_display =
                    if is_current && matches!(self.field_edit_state, FieldEditState::EditingValue) {
                        self.value_buffer.clone() + "_"
                    } else {
                        field.value.clone()
                    };

                let fill_button =
                    if is_current && field.editable && !field.is_unique_per_track && !field.value.is_empty()
                    {
                        " [F]"
                    } else {
                        ""
                    };

                let line_text = format!("{:18} : {}{}", name_display, value_display, fill_button);

                let style = if is_current {
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                Line::from(line_text).style(style)
            })
            .collect();

        let tag_para = Paragraph::new(field_lines)
            .block(Block::default().borders(Borders::ALL).title("Tag Editor"));
        f.render_widget(tag_para, area);
    }

    fn render_search_panel(&self, f: &mut Frame, area: Rect) {
        let search_para = Paragraph::new(vec![Line::from("(TODO)")])
            .block(Block::default().borders(Borders::ALL).title("Search"))
            .alignment(Alignment::Center);
        f.render_widget(search_para, area);
    }

    fn render_status_box(&self, f: &mut Frame, area: Rect, status_message: Option<&str>) {
        let mut status_lines = vec![];

        if let Some(msg) = status_message {
            status_lines.push(Line::from(msg).style(Style::default().fg(Color::Yellow)));
            status_lines.push(Line::from(""));
        }

        status_lines.extend(vec![
            Line::from("Navigation: Tab/Shift+Tab = next/prev track | Left/Right = toggle name/value | Up/Down = navigate fields | Enter = edit/commit"),
            Line::from(""),
            Line::from("Actions: F = fill to all | Ctrl+U = clear | Esc = exit | Tab past last track to save all"),
        ]);

        let status_para = Paragraph::new(status_lines)
            .block(Block::default().borders(Borders::ALL).title("Status"));
        f.render_widget(status_para, area);
    }
}

// ============================================================================
// Modal Rendering
// ============================================================================

/// Render the save confirmation modal
pub fn render_save_confirmation_modal(f: &mut Frame, area: Rect, selected_button: usize) {
    use super::helpers::centered_rect;

    let popup_area = centered_rect(60, 30, area);

    // Clear background
    let clear_block = Block::default().style(Style::default().bg(Color::Reset));
    f.render_widget(clear_block, area);

    // Modal box
    let modal_block = Block::default()
        .borders(Borders::ALL)
        .title("Save Changes?")
        .border_style(Style::default().fg(Color::Yellow));

    let inner = modal_block.inner(popup_area);
    f.render_widget(modal_block, popup_area);

    let buttons = [
        "[ Save All Changes ]",
        "[ Save All & Next Set ]",
        "[ Return to Editing ]",
    ];

    let mut button_lines: Vec<Line> = vec![
        Line::from(""),
        Line::from("All edits will be written to files."),
        Line::from(""),
    ];

    for (i, label) in buttons.iter().enumerate() {
        let style = if i == selected_button {
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        button_lines.push(Line::from(*label).style(style));
        if i < buttons.len() - 1 {
            button_lines.push(Line::from(""));
        }
    }

    button_lines.push(Line::from(""));
    button_lines.push(
        Line::from("Use Left/Right to select, Enter to confirm, Esc to cancel")
            .style(Style::default().fg(Color::DarkGray)),
    );

    let paragraph = Paragraph::new(button_lines).alignment(Alignment::Center);
    f.render_widget(paragraph, inner);
}

/// Render the change preview modal
pub fn render_change_preview_modal(
    f: &mut Frame,
    area: Rect,
    grouped_changes: &[GroupedChange],
    single_changes: &[TagChange],
    scroll_offset: usize,
) {
    use super::helpers::centered_rect;

    let modal_area = centered_rect(80, 80, area);

    // Clear background
    let clear_block = Block::default().style(Style::default().bg(Color::Reset));
    f.render_widget(clear_block, area);

    // Modal box
    let modal_block = Block::default()
        .borders(Borders::ALL)
        .title("Review Changes Before Saving")
        .border_style(Style::default().fg(Color::Yellow));

    let inner = modal_block.inner(modal_area);
    f.render_widget(modal_block, modal_area);

    // Build lines for preview
    let mut lines = Vec::new();

    let total_changes = grouped_changes
        .iter()
        .map(|g| g.track_indices.len())
        .sum::<usize>()
        + single_changes.len();
    lines.push(
        Line::from(format!("Total changes: {}", total_changes)).style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Cyan),
        ),
    );
    lines.push(Line::from(""));

    // Grouped changes section
    if !grouped_changes.is_empty() {
        lines.push(
            Line::from("Common Changes (multiple tracks):").style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Green),
            ),
        );
        lines.push(Line::from(""));

        for group in grouped_changes {
            let track_list = if group.track_indices.len() <= 5 {
                group
                    .track_indices
                    .iter()
                    .map(|i| (i + 1).to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                format!("{} tracks", group.track_indices.len())
            };

            let line_text = format!(
                "  [{}] {}: '{}' -> '{}'",
                track_list,
                group.field_name,
                if group.old_value.is_empty() {
                    "(empty)"
                } else {
                    &group.old_value
                },
                if group.new_value.is_empty() {
                    "(empty)"
                } else {
                    &group.new_value
                }
            );
            lines.push(Line::from(line_text).style(Style::default().fg(Color::Cyan)));
        }
        lines.push(Line::from(""));
    }

    // Single-track changes section
    if !single_changes.is_empty() {
        lines.push(
            Line::from("Individual Track Changes:").style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Yellow),
            ),
        );
        lines.push(Line::from(""));

        for change in single_changes {
            let line_text = format!(
                "  Track {}: {}: '{}' -> '{}'",
                change.track_idx + 1,
                change.field_name,
                if change.old_value.is_empty() {
                    "(empty)"
                } else {
                    &change.old_value
                },
                if change.new_value.is_empty() {
                    "(empty)"
                } else {
                    &change.new_value
                }
            );
            lines.push(Line::from(line_text).style(Style::default().fg(Color::White)));
        }
        lines.push(Line::from(""));
    }

    // Instructions
    lines.push(Line::from(""));
    lines.push(
        Line::from("Press Enter/Y to confirm and save, Esc/N to cancel")
            .style(Style::default().fg(Color::DarkGray)),
    );
    lines.push(
        Line::from("Use Up/Down or PgUp/PgDn to scroll")
            .style(Style::default().fg(Color::DarkGray)),
    );

    // Apply scroll offset
    let max_scroll = lines.len().saturating_sub(inner.height as usize);
    let clamped_offset = scroll_offset.min(max_scroll);
    let visible_lines: Vec<Line> = lines
        .into_iter()
        .skip(clamped_offset)
        .take(inner.height as usize)
        .collect();

    let paragraph = Paragraph::new(visible_lines);
    f.render_widget(paragraph, inner);
}
