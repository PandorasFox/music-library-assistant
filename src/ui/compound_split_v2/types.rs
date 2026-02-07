//! Compound Split V2 Types
//!
//! Data structures for the three-pane compound tag split resolution modal.
//!
//! Key behaviors:
//! - Left pane: Split parts with exists/new indicators
//! - Middle pane: Files with selection checkboxes for exclusion
//! - Right pane: Tag values for selected file (informational)
//! - Editable split parts in review mode
//! - Ctrl+Q to canonicalize (mark as single entity, don't split)

use std::collections::HashSet;
use std::path::Path;

use crate::corpus::db::types::AggregateSignal;
use crate::corpus::db::ReadOnlyDb;
use crate::corpus::mutations::{Mutation, TagOp};
use crate::corpus::paths;
use crate::corpus::tags::TagSet;
use crate::ui::widgets::TextInputState;

// ============================================================================
// Data Types (loaded from signal)
// ============================================================================

/// File info with cached tag values for display.
#[derive(Debug, Clone)]
pub struct FileTagInfo {
    /// Inode of the file
    pub inode: i64,
    /// Display name (basename)
    pub filename: String,
    /// Full corpus-relative path
    pub path: String,
    /// All tags for this file (tag_name, tag_value)
    pub tag_values: Vec<(String, String)>,
}

/// A single compound value from the signal's compounds array.
#[derive(Debug, Clone)]
pub struct CompoundEntry {
    /// Tag name (e.g., "artist", "genre")
    pub tag_name: String,
    /// Original compound value (e.g., "Rock; Metal")
    pub compound_value: String,
    /// Split parts (e.g., ["Rock", "Metal"])
    pub split_parts: Vec<String>,
    /// Which parts exist in corpus
    pub matching_parts: Vec<String>,
    /// Separator used (e.g., "; ")
    pub separator: String,
}

impl CompoundEntry {
    /// Whether all split parts exist in corpus (safe to split).
    pub fn is_safe(&self) -> bool {
        !self.split_parts.is_empty()
            && self.split_parts.len() == self.matching_parts.len()
    }

    /// Check if a specific part exists in corpus.
    pub fn part_exists(&self, part: &str) -> bool {
        self.matching_parts.iter().any(|m| m == part)
    }
}

/// Modal data loaded from a compound tag signal.
#[derive(Debug, Clone)]
pub struct CompoundSplitDataV2 {
    /// The first compound entry (we process one at a time)
    pub compound: CompoundEntry,
    /// Inode from the signal
    pub inode: i64,
    /// Per-file tag info with cached tag values
    pub files: Vec<FileTagInfo>,
}

impl CompoundSplitDataV2 {
    /// Parse from an AggregateSignal's metadata JSON.
    ///
    /// Signal format (per-file):
    /// {
    ///   "inode": 12345,
    ///   "compounds": [
    ///     {
    ///       "tag_name": "artist",
    ///       "compound_value": "A & B",
    ///       "split_parts": ["A", "B"],
    ///       "matching_parts": ["A", "B"],
    ///       "separator": " & "
    ///     }
    ///   ]
    /// }
    pub fn from_signal_with_files(signal: &AggregateSignal, read_db: &ReadOnlyDb) -> Option<Self> {
        let metadata = signal.metadata_json.as_ref()?;
        let json: serde_json::Value = serde_json::from_str(metadata).ok()?;

        let inode = json.get("inode")?.as_i64()?;

        // Get the compounds array
        let compounds_arr = json.get("compounds")?.as_array()?;
        if compounds_arr.is_empty() {
            return None;
        }

        // Take the first compound (we process one at a time per signal)
        let c = &compounds_arr[0];
        let compound = CompoundEntry {
            tag_name: c.get("tag_name")?.as_str()?.to_string(),
            compound_value: c.get("compound_value")?.as_str()?.to_string(),
            split_parts: c
                .get("split_parts")?
                .as_array()?
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            matching_parts: c
                .get("matching_parts")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            separator: c
                .get("separator")
                .and_then(|v| v.as_str())
                .unwrap_or("; ")
                .to_string(),
        };

        // Load file info
        let resolver = paths::get_resolver();
        let mut files = Vec::new();

        if let Ok(Some(audio_file)) =
            read_db.get_audio_file_by_inode(inode, crate::corpus::db::types::FileSource::Corpus)
        {
            let path = audio_file.path();
            let filename = Path::new(path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string());

            // Load tags from disk
            let abs_path = resolver.resolve(Path::new(path));
            let tagset = TagSet::from_file(&abs_path).unwrap_or_else(|_| TagSet::empty());

            let tag_values: Vec<(String, String)> = tagset
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();

            files.push(FileTagInfo {
                inode,
                filename,
                path: path.to_string(),
                tag_values,
            });
        }

        // Sort files alphabetically by filename for consistent display
        files.sort_by(|a, b| a.filename.cmp(&b.filename));

        Some(Self {
            compound,
            inode,
            files,
        })
    }

    /// Create a CanonicalTag signal emission mutation.
    pub fn create_canonical_signal(&self) -> Mutation {
        Mutation::EmitCanonicalTag {
            tag_name: self.compound.tag_name.clone(),
            canonical_value: self.compound.compound_value.clone(),
        }
    }
}

// ============================================================================
// Focus Pane
// ============================================================================

/// Which pane has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPaneV2 {
    /// Left pane: split parts
    #[default]
    Parts,
    /// Middle pane: file list
    Files,
}

// ============================================================================
// State
// ============================================================================

/// State for the three-pane compound split modal.
#[derive(Debug, Clone)]
pub struct CompoundSplitStateV2 {
    /// Loaded data (immutable during interaction)
    pub data: CompoundSplitDataV2,

    /// Editable split parts (initialized from data.compound.split_parts)
    pub edited_parts: Vec<String>,

    /// Which files are selected for the operation (indices into data.files)
    /// All selected by default; user can deselect to exclude
    pub selected_files: HashSet<usize>,

    /// Currently highlighted split part (0-indexed)
    pub part_cursor: usize,

    /// Currently highlighted file in files pane
    pub file_cursor: usize,

    /// Which pane has focus
    pub focus_pane: FocusPaneV2,

    /// Text input for editing a part (used in review mode)
    pub part_input: TextInputState,

    /// Index of part currently being edited (None if not editing)
    pub editing_part_index: Option<usize>,

    /// Whether this is a "safe" split (bulk mode - no editing)
    pub is_safe_mode: bool,

    /// Scroll offsets
    pub part_scroll: usize,
    pub file_scroll: usize,

    /// Progress tracking
    pub group_index: usize,
    pub total_groups: usize,
}

impl CompoundSplitStateV2 {
    /// Create a new state from data.
    pub fn new(
        data: CompoundSplitDataV2,
        is_safe_mode: bool,
        group_index: usize,
        total_groups: usize,
    ) -> Self {
        let edited_parts = data.compound.split_parts.clone();
        let selected_files: HashSet<usize> = (0..data.files.len()).collect();

        Self {
            data,
            edited_parts,
            selected_files,
            part_cursor: 0,
            file_cursor: 0,
            focus_pane: FocusPaneV2::Parts,
            part_input: TextInputState::new(),
            editing_part_index: None,
            is_safe_mode,
            part_scroll: 0,
            file_scroll: 0,
            group_index,
            total_groups,
        }
    }

    /// Check if the staged decision was a canonicalize action (EmitCanonicalTag).
    ///
    /// Returns true if the mutations indicate the user chose to mark this value
    /// as canonical rather than split it.
    pub fn is_canonicalize_decision(mutations: &[Mutation]) -> bool {
        mutations.iter().any(|m| matches!(m, Mutation::EmitCanonicalTag { .. }))
    }

    /// Restore UI state from a previously staged decision's mutations.
    ///
    /// When navigating back to a cluster that already has a staged decision,
    /// this method extracts the edited parts and selected files from the
    /// stored mutations and applies them to the modal state.
    ///
    /// Note: For canonicalize decisions (EmitCanonicalTag), callers should
    /// use `is_canonicalize_decision()` first and handle separately if needed.
    pub fn restore_from_mutations(&mut self, mutations: &[Mutation]) {
        // Find ApplyTagOps mutation and extract tag operations
        let ops: Vec<&TagOp> = mutations
            .iter()
            .filter_map(|m| match m {
                Mutation::ApplyTagOps { ops } => Some(ops.iter()),
                _ => None,
            })
            .flatten()
            .collect();

        if ops.is_empty() {
            return;
        }

        // Extract edited_parts from the ops:
        // - First op with new_value is the replacement for the compound value
        // - Subsequent add_tag ops (old_value=None) are the additional parts
        let mut parts: Vec<String> = Vec::new();

        // First: find the replace operation (has old_value matching compound value)
        if let Some(replace_op) = ops.iter().find(|op| {
            op.old_value.as_ref() == Some(&self.data.compound.compound_value)
        }) {
            if let Some(ref new_val) = replace_op.new_value {
                parts.push(new_val.clone());
            }
        }

        // Then: find all add operations (old_value=None) for this tag
        for op in &ops {
            if op.old_value.is_none() && op.new_value.is_some() {
                if let Some(ref new_val) = op.new_value {
                    parts.push(new_val.clone());
                }
            }
        }

        // Find which files were selected (inodes present in ops)
        let inodes_in_ops: HashSet<i64> = ops.iter().map(|op| op.inode).collect();
        let mut selected_files: HashSet<usize> = HashSet::new();
        for (idx, file) in self.data.files.iter().enumerate() {
            if inodes_in_ops.contains(&file.inode) {
                selected_files.insert(idx);
            }
        }

        // Apply restored state
        if !parts.is_empty() {
            self.edited_parts = parts;
        }
        self.selected_files = selected_files;
    }

    /// Move part cursor up.
    pub fn part_cursor_up(&mut self) {
        if self.part_cursor > 0 {
            self.part_cursor -= 1;
        }
    }

    /// Move part cursor down.
    pub fn part_cursor_down(&mut self) {
        if self.part_cursor + 1 < self.edited_parts.len() {
            self.part_cursor += 1;
        }
    }

    /// Move file cursor up.
    pub fn file_cursor_up(&mut self) {
        if self.file_cursor > 0 {
            self.file_cursor -= 1;
        }
    }

    /// Move file cursor down.
    pub fn file_cursor_down(&mut self) {
        if self.file_cursor + 1 < self.data.files.len() {
            self.file_cursor += 1;
        }
    }

    /// Toggle file selection (for exclusion).
    pub fn toggle_file_selection(&mut self) {
        let idx = self.file_cursor;
        if idx < self.data.files.len() {
            if self.selected_files.contains(&idx) {
                self.selected_files.remove(&idx);
            } else {
                self.selected_files.insert(idx);
            }
        }
    }

    /// Start editing the current part (review mode only).
    pub fn start_editing(&mut self) {
        if self.is_safe_mode {
            return; // No editing in safe mode
        }
        if self.part_cursor < self.edited_parts.len() {
            self.editing_part_index = Some(self.part_cursor);
            self.part_input.set_value(self.edited_parts[self.part_cursor].clone());
            self.part_input.focused = true;
        }
    }

    /// Confirm the current edit.
    pub fn confirm_edit(&mut self) {
        if let Some(idx) = self.editing_part_index {
            if idx < self.edited_parts.len() {
                let new_value = self.part_input.value().trim().to_string();
                if !new_value.is_empty() {
                    self.edited_parts[idx] = new_value;
                }
            }
        }
        self.cancel_edit();
    }

    /// Cancel editing.
    pub fn cancel_edit(&mut self) {
        self.editing_part_index = None;
        self.part_input.focused = false;
    }

    /// Check if currently editing.
    pub fn is_editing(&self) -> bool {
        self.editing_part_index.is_some()
    }

    /// Check if we can submit (at least one file selected).
    pub fn can_submit(&self) -> bool {
        !self.selected_files.is_empty()
    }

    /// Generate mutations for the split using incremental TagOps.
    pub fn mutations(&self) -> Vec<Mutation> {
        if self.selected_files.is_empty() {
            return Vec::new();
        }

        let resolver = paths::get_resolver();
        let mut ops = Vec::new();

        for &file_idx in &self.selected_files {
            let Some(file) = self.data.files.get(file_idx) else {
                continue;
            };

            // Load current tags to verify file still has the compound value
            let abs_path = resolver.resolve(Path::new(&file.path));
            let Ok(current_tagset) = TagSet::from_file(&abs_path) else {
                continue;
            };

            // Verify file still has the compound value
            if !current_tagset.contains(&self.data.compound.tag_name, &self.data.compound.compound_value) {
                continue;
            }

            // Replace compound value with first edited part
            if let Some(first_part) = self.edited_parts.first() {
                ops.push(TagOp::replace_tag(
                    file.inode,
                    &self.data.compound.tag_name,
                    &self.data.compound.compound_value,
                    first_part,
                ));
            }

            // Add remaining parts
            for part in self.edited_parts.iter().skip(1) {
                ops.push(TagOp::add_tag(
                    file.inode,
                    &self.data.compound.tag_name,
                    part,
                ));
            }
        }

        if ops.is_empty() {
            Vec::new()
        } else {
            vec![Mutation::ApplyTagOps { ops }]
        }
    }
}

// ============================================================================
// Action
// ============================================================================

/// Action returned from handling input in the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompoundSplitActionV2 {
    /// No action, continue showing modal
    None,
    /// User confirmed split - stage decision and advance
    Confirmed,
    /// User chose to canonicalize (Ctrl+Q) - mark as single entity
    Canonicalize,
    /// User cancelled entire flow (Esc)
    Cancelled,
    /// User navigated to next/prev signal (Tab/Shift-Tab) - does NOT stage decision
    Navigate {
        /// True = forward (Tab), false = backward (Shift-Tab)
        forward: bool,
    },
    /// User requested review screen (Ctrl+R)
    ShowReview,
    /// Stage ALL and show review (Ctrl+A) - for bulk operations
    StageAllAndReview,
}

// ============================================================================
// Cluster Navigation
// ============================================================================

/// Tracks navigation through compound tag split signals.
#[derive(Debug, Clone)]
pub struct CompoundSplitClustersV2 {
    /// Signal IDs in display order
    signal_ids: Vec<i64>,
    /// Current index
    current: usize,
}

impl CompoundSplitClustersV2 {
    pub fn new(signal_ids: Vec<i64>) -> Self {
        Self {
            signal_ids,
            current: 0,
        }
    }

    pub fn current_signal_id(&self) -> Option<i64> {
        self.signal_ids.get(self.current).copied()
    }

    pub fn current_index(&self) -> usize {
        self.current
    }

    pub fn total(&self) -> usize {
        self.signal_ids.len()
    }

    pub fn all_signal_ids(&self) -> &[i64] {
        &self.signal_ids
    }

    pub fn is_last(&self) -> bool {
        self.current >= self.signal_ids.len().saturating_sub(1)
    }

    pub fn is_first(&self) -> bool {
        self.current == 0
    }

    pub fn next(&mut self) -> bool {
        if self.current < self.signal_ids.len().saturating_sub(1) {
            self.current += 1;
            true
        } else {
            false
        }
    }

    pub fn prev(&mut self) -> bool {
        if self.current > 0 {
            self.current -= 1;
            true
        } else {
            false
        }
    }
}
