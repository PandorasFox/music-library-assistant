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

use crate::meta::signals::data::CompoundGroup;
use crate::corpus::db::ReadOnlyDb;
use crate::meta::mutations::{Mutation, TagOp};
use crate::meta::mutations::indexing::EmitCanonicalTagMutation;
use crate::meta::mutations::tag_edit::ApplyTagOpsMutation;
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
}

impl CompoundEntry {
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
    /// Per-file tag info with cached tag values
    pub files: Vec<FileTagInfo>,
}

impl CompoundSplitDataV2 {
    /// Construct from a `CompoundGroup` (aggregated by compound value).
    ///
    /// Loads the compound entry from the first inode's signal data, then
    /// loads file info for ALL inodes in the group. This lets the operator
    /// decide once per unique compound value, seeing all affected files.
    pub fn from_compound_group(group: &CompoundGroup, read_db: &ReadOnlyDb) -> Option<Self> {
        if group.inodes.is_empty() {
            return None;
        }

        // Load compound entry from the first inode's signal
        let first_signal = read_db.get_compound_tag_signal(group.inodes[0]).ok()??;
        let c = first_signal.compounds.iter()
            .find(|c| c.tag_name == group.tag_name && c.compound_value == group.compound_value)?;
        let compound = CompoundEntry {
            tag_name: c.tag_name.clone(),
            compound_value: c.compound_value.clone(),
            split_parts: c.split_parts.clone(),
            matching_parts: c.matching_parts.clone(),
        };

        // Load file info for all inodes in the group
        let resolver = paths::get_resolver();
        let mut files = Vec::new();

        for &inode in &group.inodes {
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
        }

        // Sort files alphabetically by filename for consistent display
        files.sort_by(|a, b| a.filename.cmp(&b.filename));

        Some(Self {
            compound,
            files,
        })
    }

    /// Create a CanonicalTag signal emission mutation.
    pub fn create_canonical_signal(&self) -> Mutation {
        Mutation::EmitCanonicalTag(EmitCanonicalTagMutation {
            tag_name: self.compound.tag_name.clone(),
            canonical_value: self.compound.compound_value.clone(),
        })
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

    /// Whether the canonicalize confirmation popup is showing
    pub confirming_canonicalize: bool,

    /// Whether the bulk-stage-all confirmation popup is showing
    pub confirming_bulk_stage: bool,
}

impl CompoundSplitStateV2 {
    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.data.files.get(self.file_cursor).map(|f| f.path.as_str())
    }

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
            confirming_canonicalize: false,
            confirming_bulk_stage: false,
        }
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
                Mutation::ApplyTagOps(ref m) => Some(m.ops.iter()),
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
            vec![Mutation::ApplyTagOps(ApplyTagOpsMutation { ops })]
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
    /// User cancelled entire modal (Esc)
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
    /// Open tag editor for current group's files (individual mode, T key)
    OpenTagEditorIndividual,
    /// Open tag editor for current group's files (aggregated mode, Shift+T key)
    OpenTagEditorAggregated,
}

// ============================================================================
// Cluster Navigation
// ============================================================================

/// Tracks navigation through compound tag split groups (aggregated by value).
#[derive(Debug, Clone)]
pub struct CompoundSplitClustersV2 {
    /// Groups in display order (one per unique compound value)
    groups: Vec<CompoundGroup>,
    /// Current index
    current: usize,
}

impl CompoundSplitClustersV2 {
    pub fn new(groups: Vec<CompoundGroup>) -> Self {
        Self {
            groups,
            current: 0,
        }
    }

    pub fn current_group(&self) -> Option<&CompoundGroup> {
        self.groups.get(self.current)
    }

    pub fn current_index(&self) -> usize {
        self.current
    }

    pub fn total(&self) -> usize {
        self.groups.len()
    }

    pub fn all_groups(&self) -> &[CompoundGroup] {
        &self.groups
    }

    pub fn is_last(&self) -> bool {
        self.current >= self.groups.len().saturating_sub(1)
    }

    pub fn is_first(&self) -> bool {
        self.current == 0
    }

    pub fn next(&mut self) -> bool {
        if self.current < self.groups.len().saturating_sub(1) {
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
