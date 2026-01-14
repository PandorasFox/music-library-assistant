//! Directory Tag Editor State Management
//!
//! Handles bulk tag editing for all files in a directory with aggregated field display.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use walkdir::WalkDir;

use crate::config::is_audio_extension;
use crate::corpus::metadata;

use super::types::{
    AggregatedTagField, AggregatedValue, DirectoryTagEditorAction, DirectoryTagEditorFocus,
    FieldEditState, GatheringMessage, VariousConfirmState,
};

// ============================================================================
// Core Types
// ============================================================================

/// A file with its loaded tags
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub filename: String,
    pub tags: Vec<(String, String)>,
}

/// State for async metadata gathering
pub struct GatheringState {
    pub total_files: usize,
    pub processed_files: usize,
    pub current_file: Option<String>,
    pub cancel_flag: Arc<AtomicBool>,
    pub receiver: Receiver<GatheringMessage>,
}

// ============================================================================
// Directory Tag Editor State
// ============================================================================

/// State for directory-based bulk tag editing
pub struct DirectoryTagEditorState {
    // File data
    pub files: Vec<FileEntry>,
    pub aggregated_fields: Vec<AggregatedTagField>,
    pub original_aggregated_fields: Vec<AggregatedTagField>,

    // Directory navigation
    pub current_directory: PathBuf,
    pub sibling_directories: Vec<PathBuf>,
    pub current_sibling_idx: usize,

    // Field navigation & editing
    pub current_field_idx: usize,
    pub field_edit_state: FieldEditState,
    pub value_buffer: String,
    pub various_confirm_state: Option<VariousConfirmState>,

    // Metadata gathering state (Some while gathering)
    pub gathering_state: Option<GatheringState>,

    // UI state
    pub focus: DirectoryTagEditorFocus,
    pub tag_scroll_offset: usize,
    pub tag_visible_height: usize,
    pub file_scroll_offset: usize,
    pub file_visible_height: usize,
    pub selected_file_idx: usize,
}

impl DirectoryTagEditorState {
    /// Start gathering metadata for a directory (returns state in gathering mode)
    pub fn start_gathering(directory: PathBuf) -> Self {
        let siblings = discover_siblings(&directory);
        let current_idx = siblings
            .iter()
            .position(|p| p == &directory)
            .unwrap_or(0);

        // Set up channel for progress updates
        let (tx, rx) = mpsc::channel();
        let cancel_flag = Arc::new(AtomicBool::new(false));

        // Start background gathering
        let dir_clone = directory.clone();
        let cancel_clone = cancel_flag.clone();
        thread::spawn(move || {
            gather_files_recursive(&dir_clone, tx, cancel_clone);
        });

        Self {
            files: Vec::new(),
            aggregated_fields: Vec::new(),
            original_aggregated_fields: Vec::new(),
            current_directory: directory,
            sibling_directories: siblings,
            current_sibling_idx: current_idx,
            current_field_idx: 0,
            field_edit_state: FieldEditState::NonEditable,
            value_buffer: String::new(),
            various_confirm_state: None,
            gathering_state: Some(GatheringState {
                total_files: 0,
                processed_files: 0,
                current_file: None,
                cancel_flag,
                receiver: rx,
            }),
            focus: DirectoryTagEditorFocus::TagFields,
            tag_scroll_offset: 0,
            tag_visible_height: 10,
            file_scroll_offset: 0,
            file_visible_height: 10,
            selected_file_idx: 0,
        }
    }

    /// Poll for gathering progress updates. Returns true if gathering is complete.
    pub fn poll_gathering(&mut self) -> bool {
        let gathering = match &mut self.gathering_state {
            Some(g) => g,
            None => return true, // Already complete
        };

        // Process all available messages
        while let Ok(msg) = gathering.receiver.try_recv() {
            match msg {
                GatheringMessage::TotalFiles(count) => {
                    gathering.total_files = count;
                }
                GatheringMessage::Processing(idx, path) => {
                    gathering.processed_files = idx;
                    gathering.current_file = Some(path);
                }
                GatheringMessage::FileProcessed { path, tags, .. } => {
                    let filename = Path::new(&path)
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.clone());
                    self.files.push(FileEntry {
                        path: PathBuf::from(&path),
                        filename,
                        tags,
                    });
                }
                GatheringMessage::FileError { path, error, .. } => {
                    let _ = crate::config::log_message(&format!(
                        "Warning: Could not read tags from {}: {}",
                        path, error
                    ));
                }
                GatheringMessage::Complete => {
                    // Finalize: sort files and aggregate tags
                    self.files.sort_by(|a, b| a.path.cmp(&b.path));
                    self.aggregated_fields = aggregate_tags(&self.files);
                    self.original_aggregated_fields = self.aggregated_fields.clone();
                    self.gathering_state = None;
                    return true;
                }
            }
        }

        false
    }

    /// Cancel gathering
    pub fn cancel_gathering(&mut self) {
        if let Some(gathering) = &self.gathering_state {
            gathering.cancel_flag.store(true, Ordering::SeqCst);
        }
    }

    /// Check if currently gathering
    pub fn is_gathering(&self) -> bool {
        self.gathering_state.is_some()
    }

    // ========================================================================
    // Navigation
    // ========================================================================

    pub fn move_up(&mut self) {
        if self.field_edit_state != FieldEditState::NonEditable {
            self.commit_current_buffer();
        }
        if self.current_field_idx > 0 {
            self.current_field_idx -= 1;
            if self.current_field_idx < self.tag_scroll_offset {
                self.tag_scroll_offset = self.current_field_idx;
            }
            self.load_buffer();
        }
    }

    pub fn move_down(&mut self) {
        if self.field_edit_state != FieldEditState::NonEditable {
            self.commit_current_buffer();
        }
        if self.current_field_idx < self.aggregated_fields.len().saturating_sub(1) {
            self.current_field_idx += 1;
            let visible_end = self.tag_scroll_offset + self.tag_visible_height.saturating_sub(1);
            if self.current_field_idx >= visible_end {
                self.tag_scroll_offset =
                    self.current_field_idx.saturating_sub(self.tag_visible_height.saturating_sub(2));
            }
            self.load_buffer();
        }
    }

    // ========================================================================
    // Buffer Management
    // ========================================================================

    pub fn load_buffer(&mut self) {
        if let Some(field) = self.aggregated_fields.get(self.current_field_idx) {
            self.value_buffer = match &field.value {
                AggregatedValue::Consistent(s) => s.clone(),
                AggregatedValue::Edited(s) => s.clone(),
                AggregatedValue::Various | AggregatedValue::VariousConfirming => String::new(),
            };
        }
    }

    pub fn commit_current_buffer(&mut self) {
        if let Some(field) = self.aggregated_fields.get_mut(self.current_field_idx) {
            if self.field_edit_state == FieldEditState::EditingValue {
                // Any edit becomes an Edited value (fills to all files)
                field.value = AggregatedValue::Edited(self.value_buffer.clone());
            }
        }
        self.various_confirm_state = None;
    }

    // ========================================================================
    // Editing
    // ========================================================================

    pub fn handle_enter(&mut self) -> DirectoryTagEditorAction {
        let field = match self.aggregated_fields.get(self.current_field_idx) {
            Some(f) => f,
            None => return DirectoryTagEditorAction::None,
        };

        if field.name == "New Tag" {
            self.create_new_tag();
            return DirectoryTagEditorAction::None;
        }

        match &field.value {
            AggregatedValue::Various => {
                // First Enter on Various -> confirm
                if let Some(field) = self.aggregated_fields.get_mut(self.current_field_idx) {
                    field.value = AggregatedValue::VariousConfirming;
                }
                self.various_confirm_state = Some(VariousConfirmState::Confirming);
                DirectoryTagEditorAction::None
            }
            AggregatedValue::VariousConfirming => {
                // Second Enter -> enter edit mode
                self.various_confirm_state = Some(VariousConfirmState::Editing);
                self.field_edit_state = FieldEditState::EditingValue;
                self.value_buffer.clear();
                DirectoryTagEditorAction::None
            }
            AggregatedValue::Consistent(_) | AggregatedValue::Edited(_) => {
                if self.field_edit_state == FieldEditState::NonEditable {
                    // Enter edit mode
                    self.field_edit_state = FieldEditState::EditingValue;
                    self.load_buffer();
                } else {
                    // Commit and exit edit mode
                    self.commit_current_buffer();
                    self.field_edit_state = FieldEditState::NonEditable;
                }
                DirectoryTagEditorAction::None
            }
        }
    }

    fn create_new_tag(&mut self) {
        let new_field = AggregatedTagField {
            name: "new_tag".to_string(),
            value: AggregatedValue::Edited(String::new()),
            original_value: AggregatedValue::Various, // Didn't exist before
            editable: true,
        };

        // Insert before "New Tag" placeholder
        let insert_pos = self.aggregated_fields.len().saturating_sub(1);
        self.aggregated_fields.insert(insert_pos, new_field);
        self.current_field_idx = insert_pos;

        // Enter edit mode for value (name editing not supported in directory mode)
        self.field_edit_state = FieldEditState::EditingValue;
        self.value_buffer.clear();
    }

    pub fn insert_char(&mut self, c: char) {
        if self.field_edit_state == FieldEditState::EditingValue {
            self.value_buffer.push(c);
        }
    }

    pub fn delete_char(&mut self) {
        if self.field_edit_state == FieldEditState::EditingValue {
            self.value_buffer.pop();
        }
    }

    pub fn handle_escape(&mut self) -> DirectoryTagEditorAction {
        if self.field_edit_state != FieldEditState::NonEditable {
            // Exit edit mode without saving
            self.field_edit_state = FieldEditState::NonEditable;
            self.various_confirm_state = None;
            // Revert VariousConfirming back to Various
            if let Some(field) = self.aggregated_fields.get_mut(self.current_field_idx) {
                if matches!(field.value, AggregatedValue::VariousConfirming) {
                    field.value = AggregatedValue::Various;
                }
            }
            self.load_buffer();
            DirectoryTagEditorAction::None
        } else {
            DirectoryTagEditorAction::Exit
        }
    }

    pub fn clear_current_field(&mut self) {
        if self.field_edit_state == FieldEditState::EditingValue {
            self.value_buffer.clear();
        } else if let Some(field) = self.aggregated_fields.get_mut(self.current_field_idx) {
            field.value = AggregatedValue::Edited(String::new());
        }
    }

    // ========================================================================
    // Change Tracking
    // ========================================================================

    pub fn has_changes(&self) -> bool {
        for (field, original) in self
            .aggregated_fields
            .iter()
            .zip(self.original_aggregated_fields.iter())
        {
            if field.name == "New Tag" {
                continue;
            }
            if field.value != original.value {
                return true;
            }
        }
        // Also check for new fields
        self.aggregated_fields.len() != self.original_aggregated_fields.len()
    }

    pub fn compute_changes(&self) -> Vec<DirectoryTagChange> {
        let mut changes = Vec::new();

        for (field, original) in self
            .aggregated_fields
            .iter()
            .zip(self.original_aggregated_fields.iter())
        {
            if field.name == "New Tag" {
                continue;
            }
            if field.value != original.value {
                if let AggregatedValue::Edited(new_value) = &field.value {
                    changes.push(DirectoryTagChange {
                        field_name: field.name.clone(),
                        new_value: new_value.clone(),
                        affected_files: self.files.len(),
                    });
                }
            }
        }

        // Check for completely new fields
        for field in self.aggregated_fields.iter().skip(self.original_aggregated_fields.len()) {
            if field.name != "New Tag" {
                if let AggregatedValue::Edited(new_value) = &field.value {
                    changes.push(DirectoryTagChange {
                        field_name: field.name.clone(),
                        new_value: new_value.clone(),
                        affected_files: self.files.len(),
                    });
                }
            }
        }

        changes
    }

    // ========================================================================
    // Directory Navigation
    // ========================================================================

    pub fn switch_to_sibling(&self, next: bool) -> Option<PathBuf> {
        if self.sibling_directories.is_empty() {
            return None;
        }

        let new_idx = if next {
            if self.current_sibling_idx < self.sibling_directories.len() - 1 {
                self.current_sibling_idx + 1
            } else {
                return None; // At end
            }
        } else {
            if self.current_sibling_idx > 0 {
                self.current_sibling_idx - 1
            } else {
                return None; // At start
            }
        };

        Some(self.sibling_directories[new_idx].clone())
    }
}

// ============================================================================
// Change Type
// ============================================================================

/// A change to apply to all files
#[derive(Debug, Clone)]
pub struct DirectoryTagChange {
    pub field_name: String,
    pub new_value: String,
    pub affected_files: usize,
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Discover sibling directories (same parent level)
fn discover_siblings(directory: &Path) -> Vec<PathBuf> {
    let parent = match directory.parent() {
        Some(p) => p,
        None => return vec![directory.to_path_buf()],
    };

    let mut siblings: Vec<PathBuf> = std::fs::read_dir(parent)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false)
                && !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with('.')
        })
        .map(|entry| entry.path())
        .collect();

    siblings.sort();
    siblings
}

/// Gather files recursively from a directory
fn gather_files_recursive(directory: &Path, tx: Sender<GatheringMessage>, cancel_flag: Arc<AtomicBool>) {
    // First, count total files
    let files: Vec<PathBuf> = WalkDir::new(directory)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_file()
                && e.path()
                    .extension()
                    .map(|ext| is_audio_extension(&ext.to_string_lossy()))
                    .unwrap_or(false)
        })
        .map(|e| e.path().to_path_buf())
        .collect();

    let _ = tx.send(GatheringMessage::TotalFiles(files.len()));

    // Process each file
    for (idx, path) in files.iter().enumerate() {
        if cancel_flag.load(Ordering::SeqCst) {
            return;
        }

        let path_str = path.to_string_lossy().to_string();
        let _ = tx.send(GatheringMessage::Processing(idx, path_str.clone()));

        match metadata::read_all_tags(path) {
            Ok(tags) => {
                let _ = tx.send(GatheringMessage::FileProcessed {
                    index: idx,
                    path: path_str,
                    tags,
                });
            }
            Err(e) => {
                let _ = tx.send(GatheringMessage::FileError {
                    index: idx,
                    path: path_str,
                    error: e.to_string(),
                });
            }
        }
    }

    let _ = tx.send(GatheringMessage::Complete);
}

/// Aggregate tags from multiple files
fn aggregate_tags(files: &[FileEntry]) -> Vec<AggregatedTagField> {
    if files.is_empty() {
        return vec![AggregatedTagField {
            name: "New Tag".to_string(),
            value: AggregatedValue::Consistent("[Press Enter to create]".to_string()),
            original_value: AggregatedValue::Consistent("[Press Enter to create]".to_string()),
            editable: true,
        }];
    }

    // Core fields in display order
    let core_fields = [
        "track_number",
        "title",
        "artist",
        "album",
        "album_artist",
        "genre",
        "isrc",
    ];

    // Collect all unique field names
    let mut all_field_names: HashSet<String> = HashSet::new();
    for file in files {
        for (name, _) in &file.tags {
            all_field_names.insert(name.clone());
        }
    }

    // Build aggregated fields
    let mut aggregated = Vec::new();

    // First, core fields in order
    for core_name in &core_fields {
        let values: Vec<Option<&str>> = files
            .iter()
            .map(|f| {
                f.tags
                    .iter()
                    .find(|(name, _)| name == *core_name)
                    .map(|(_, v)| v.as_str())
            })
            .collect();

        let agg_value = aggregate_values(&values);
        aggregated.push(AggregatedTagField {
            name: core_name.to_string(),
            value: agg_value.clone(),
            original_value: agg_value,
            editable: true,
        });
        all_field_names.remove(*core_name);
    }

    // Then extended fields alphabetically
    let mut extended: Vec<_> = all_field_names.into_iter().collect();
    extended.sort();

    for field_name in extended {
        let values: Vec<Option<&str>> = files
            .iter()
            .map(|f| {
                f.tags
                    .iter()
                    .find(|(name, _)| name == &field_name)
                    .map(|(_, v)| v.as_str())
            })
            .collect();

        let agg_value = aggregate_values(&values);
        aggregated.push(AggregatedTagField {
            name: field_name,
            value: agg_value.clone(),
            original_value: agg_value,
            editable: true,
        });
    }

    // Add "New Tag" placeholder
    aggregated.push(AggregatedTagField {
        name: "New Tag".to_string(),
        value: AggregatedValue::Consistent("[Press Enter to create]".to_string()),
        original_value: AggregatedValue::Consistent("[Press Enter to create]".to_string()),
        editable: true,
    });

    aggregated
}

/// Aggregate values from multiple files into a single AggregatedValue
fn aggregate_values(values: &[Option<&str>]) -> AggregatedValue {
    // Get the first non-None value
    let first_value = values.iter().find_map(|v| *v);

    match first_value {
        None => AggregatedValue::Consistent(String::new()), // All empty
        Some(first) => {
            // Check if all values match
            let all_match = values.iter().all(|v| match v {
                Some(s) => *s == first,
                None => first.is_empty(),
            });

            if all_match {
                AggregatedValue::Consistent(first.to_string())
            } else {
                AggregatedValue::Various
            }
        }
    }
}
