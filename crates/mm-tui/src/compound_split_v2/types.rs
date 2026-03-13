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

use std::collections::{HashMap, HashSet};

pub use mm_meta::views::canonicity_compound::{CompoundEntry, CompoundSplitDataV2, FileTagInfo};

/// Pending tag edits from an embedded tag editor decision.
/// Maps inode → list of (tag_name, old_value, new_value) triples.
pub type PendingTagEdits = HashMap<i64, Vec<(String, String, String)>>;

use mm_meta::tags::TagSet;
use mm_meta::db_types::Zone;
use mm_meta::mutations::indexing::EmitCanonicalTagMutation;
use mm_meta::mutations::tag_edit::ApplyTagOpsMutation;
use mm_meta::mutations::{Mutation, TagOp};
use mm_meta::signals::data::CompoundGroup;
use crate::widgets::TextInputState;

/// Create a CanonicalTag signal emission mutation from compound split data.
pub fn create_canonical_signal(data: &CompoundSplitDataV2) -> Mutation {
    Mutation::EmitCanonicalTag(EmitCanonicalTagMutation {
        tag_name: data.compound.tag_name.clone(),
        canonical_value: data.compound.compound_value.clone(),
    })
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

    /// Zone for file lookups and tag mutations
    pub zone: Zone,

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

    /// Pending tag edits from an embedded tag editor decision (inode → [(tag, old, new)])
    pub pending_tag_edits: Option<PendingTagEdits>,
    /// Click targets for parts list items (set during render).
    pub part_click_targets: crate::widgets::ListClickTargets,
    /// Click targets for file list items (set during render).
    pub file_click_targets: crate::widgets::ListClickTargets,
    /// Stored pane Rects for click focus detection (set during render).
    pub parts_pane_rect: Option<ratatui::layout::Rect>,
    pub files_pane_rect: Option<ratatui::layout::Rect>,
}

impl CompoundSplitStateV2 {
    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.data
            .files
            .get(self.file_cursor)
            .map(|f| f.path.as_str())
    }

    /// Create a new state from data.
    pub fn new(
        data: CompoundSplitDataV2,
        is_safe_mode: bool,
        group_index: usize,
        total_groups: usize,
        zone: Zone,
    ) -> Self {
        let edited_parts = data.compound.split_parts.clone();
        let selected_files: HashSet<usize> = (0..data.files.len()).collect();

        Self {
            data,
            zone,
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
            pending_tag_edits: None,
            part_click_targets: Default::default(),
            file_click_targets: Default::default(),
            parts_pane_rect: None,
            files_pane_rect: None,
        }
    }

    /// Handle a mouse click at (x, y).
    pub fn handle_click(&mut self, x: u16, y: u16) {
        // Check parts list items
        if let Some(id) = self.part_click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.edited_parts.len() {
                    self.focus_pane = FocusPaneV2::Parts;
                    self.part_cursor = idx;
                }
            }
            return;
        }
        // Check file list items
        if let Some(id) = self.file_click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.data.files.len() {
                    self.focus_pane = FocusPaneV2::Files;
                    self.file_cursor = idx;
                }
            }
            return;
        }
        // Pane-level focus detection
        if let Some(rect) = self.parts_pane_rect {
            if crate::widgets::rect_contains(rect, x, y) {
                self.focus_pane = FocusPaneV2::Parts;
                return;
            }
        }
        if let Some(rect) = self.files_pane_rect {
            if crate::widgets::rect_contains(rect, x, y) {
                self.focus_pane = FocusPaneV2::Files;
            }
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
        // Find ApplyTagOps mutation and extract tag operations for THIS tag only.
        // Tag editor decisions may contain ops for unrelated tags which must not
        // pollute the edited parts or file selection.
        let ops: Vec<&TagOp> = mutations
            .iter()
            .filter_map(|m| match m {
                Mutation::ApplyTagOps(ref m) => Some(m.ops.iter()),
                _ => None,
            })
            .flatten()
            .filter(|op| {
                op.tag_name
                    .eq_ignore_ascii_case(&self.data.compound.tag_name)
            })
            .collect();

        if ops.is_empty() {
            return;
        }

        // Extract edited_parts from the ops:
        // - First op with new_value is the replacement for the compound value
        // - Subsequent add_tag ops (old_value=None) are the additional parts
        let mut parts: Vec<String> = Vec::new();

        // First: find the replace operation (has old_value matching compound value)
        if let Some(replace_op) = ops
            .iter()
            .find(|op| op.old_value.as_ref() == Some(&self.data.compound.compound_value))
        {
            if let Some(ref new_val) = replace_op.new_value {
                parts.push(new_val.clone());
            }
        }

        // Then: find unique add operations (old_value=None) for this tag.
        // Ops are duplicated per-file, so deduplicate by value.
        for op in &ops {
            if op.old_value.is_none() {
                if let Some(ref new_val) = op.new_value {
                    if !parts.contains(new_val) {
                        parts.push(new_val.clone());
                    }
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
            self.part_input
                .set_value(self.edited_parts[self.part_cursor].clone());
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

        let mut ops = Vec::new();

        for &file_idx in &self.selected_files {
            let Some(file) = self.data.files.get(file_idx) else {
                continue;
            };

            // Verify file still has the compound value using cached tag data
            let current_tagset = TagSet::new(file.tag_values.iter().cloned());
            if !current_tagset.contains(
                &self.data.compound.tag_name,
                &self.data.compound.compound_value,
            ) {
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
            vec![Mutation::ApplyTagOps(ApplyTagOpsMutation {
                ops,
                zone: self.zone,
            })]
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
        Self { groups, current: 0 }
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

    #[allow(clippy::should_implement_trait)] // Not an Iterator; advances group cursor
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
