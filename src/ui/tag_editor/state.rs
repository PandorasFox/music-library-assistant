//! Tag Editor State Management
//!
//! Core state structure for the unified tag editor.
//!
//! Contains `UnifiedTagEditorState` for transaction-based editing.

use std::path::Path;

use crate::corpus::db::types::AudioFile;
use crate::meta::mutations::Mutation;

use super::mutations::{
    aggregate_tags_across_audio_files, audio_file_to_tag_fields, changes_to_mutations,
    compute_changes,
};
use super::types::{
    AggregatedTagField, AggregatedValue, FieldEditState, GroupContext, TagChange,
    TagEditContext, TagEditorButton, TagEditorLaunchMode, TagEditorMode, TagEditorSource,
    TagField, UnifiedTagEditorFocus, UnifiedTagEditorModal,
};

// ============================================================================
// Unified Tag Editor State (New)
// ============================================================================

/// Unified tag editor state that handles both single-file and bulk edit contexts.
///
/// This replaces the dual `TagEditorState` / `DirectoryTagEditorState` pattern
/// with a single state machine that branches on `TagEditContext`.
pub struct UnifiedTagEditorState {
    // ========================================================================
    // Context & Mode
    // ========================================================================

    /// The context (SingleFile or BulkEdit) determines behavior
    pub context: TagEditContext,

    /// Editing mode: Individual (one track at a time) or Aggregated (unified view)
    pub mode: TagEditorMode,

    // ========================================================================
    // Item Navigation (within current transaction)
    // ========================================================================

    /// Current item index (track in SingleFile, file in BulkEdit)
    pub current_item_idx: usize,

    /// Total items count
    pub total_items: usize,

    // ========================================================================
    // Field Navigation & Editing
    // ========================================================================

    /// Current field index
    pub current_field_idx: usize,

    /// Field scroll offset (for long tag lists)
    pub field_scroll_offset: usize,

    /// Visible height for tag field area
    pub field_visible_height: usize,

    /// Current edit mode
    pub field_edit_state: FieldEditState,

    /// Buffer for editing tag name (SingleFile only)
    pub name_buffer: String,

    /// Buffer for editing tag value
    pub value_buffer: String,

    /// Whether focus is on value (true) or name (false) in SingleFile mode
    pub focus_on_value: bool,

    // ========================================================================
    // Tag Data
    // ========================================================================

    /// SingleFile: tag fields per track; BulkEdit: aggregated fields (single Vec)
    ///
    /// For SingleFile: outer Vec is tracks, inner Vec is fields per track
    /// For BulkEdit: single inner Vec of aggregated fields, wrapped in outer Vec
    pub tag_fields: Vec<Vec<TagField>>,

    /// Original state for change detection
    pub original_tag_fields: Vec<Vec<TagField>>,

    /// BulkEdit: aggregated fields (alternative representation)
    pub aggregated_fields: Option<Vec<AggregatedTagField>>,

    // ========================================================================
    // UI State
    // ========================================================================

    /// Which pane currently has focus
    pub focus: UnifiedTagEditorFocus,

    /// Currently selected action button
    pub selected_button: TagEditorButton,

    /// Currently active modal dialog (if any)
    pub modal: Option<UnifiedTagEditorModal>,

    /// Whether OOB (out-of-band) tag change signal is present
    pub has_oob_signal: bool,

    // ========================================================================
    // Context List Scrolling (BulkEdit file list)
    // ========================================================================

    /// Scroll offset for the file list in BulkEdit mode
    pub context_list_scroll_offset: usize,

    /// Visible height for the file list (set during render)
    pub context_list_visible_height: usize,

    // ========================================================================
    // Staged Mutations Tracking (for skipping redundant confirmations)
    // ========================================================================

    /// Number of decisions staged in the current transaction
    pub staged_decision_count: usize,

    /// Staged mutations for current item (set after StageDecision, cleared on item change)
    /// Used to skip confirmation dialog when changes match what's already staged.
    pub staged_mutations_for_current: Option<Vec<Mutation>>,

    // ========================================================================
    // Launch Mode
    // ========================================================================

    /// Whether this editor is standalone (owns transaction) or embedded (parent owns transaction).
    pub launch_mode: TagEditorLaunchMode,
}

impl UnifiedTagEditorState {
    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        match &self.context {
            TagEditContext::SingleFile { audio_file, .. } => Some(audio_file.path()),
            TagEditContext::BulkEdit { audio_files, .. } => {
                audio_files.get(self.current_item_idx).map(|af| af.path())
            }
        }
    }

    // ========================================================================
    // Constructors
    // ========================================================================

    /// Create a new unified tag editor state.
    ///
    /// This is the primary constructor that handles both Individual and Aggregated modes.
    ///
    /// - **Individual mode**: Edit audio files one at a time (Tab navigates between files)
    /// - **Aggregated mode**: Edit unified view (changes apply to all files)
    pub fn new(
        mode: TagEditorMode,
        audio_files: Vec<AudioFile>,
        source: TagEditorSource,
        group_context: Option<GroupContext>,
    ) -> Self {
        // All files must belong to the same zone — tag mutations are per-zone.
        debug_assert!(
            audio_files.windows(2).all(|w| w[0].entry.zone == w[1].entry.zone),
            "Tag editor opened with files from mixed zones"
        );

        let total_items = audio_files.len();

        // Load per-file tag fields from disk
        let tag_fields: Vec<Vec<TagField>> = audio_files.iter().map(audio_file_to_tag_fields).collect();
        let original_tag_fields = tag_fields.clone();

        // For Aggregated mode, also build aggregated view
        let aggregated_fields = if mode == TagEditorMode::Aggregated {
            Some(aggregate_tags_across_audio_files(&audio_files))
        } else {
            None
        };

        // Build context based on file count
        let context = if audio_files.len() == 1 {
            let audio_file = audio_files.into_iter().next().unwrap();
            TagEditContext::SingleFile {
                audio_file,
                source,
                group_context,
            }
        } else {
            TagEditContext::BulkEdit {
                audio_files,
                source,
            }
        };

        Self {
            context,
            mode,
            current_item_idx: 0,
            total_items,
            current_field_idx: 0,
            field_scroll_offset: 0,
            field_visible_height: 10,
            field_edit_state: FieldEditState::NonEditable,
            name_buffer: String::new(),
            value_buffer: String::new(),
            focus_on_value: true,
            tag_fields,
            original_tag_fields,
            aggregated_fields,
            focus: UnifiedTagEditorFocus::TagFields,
            selected_button: TagEditorButton::ReviewAll,
            modal: None,
            has_oob_signal: false,
            context_list_scroll_offset: 0,
            context_list_visible_height: 0,
            staged_decision_count: 0,
            staged_mutations_for_current: None,
            launch_mode: TagEditorLaunchMode::Standalone,
        }
    }

    /// Create a new unified state for single-file editing (convenience wrapper).
    pub fn single_file(audio_file: AudioFile, source: TagEditorSource, group_context: Option<GroupContext>) -> Self {
        Self::new(TagEditorMode::Individual, vec![audio_file], source, group_context)
    }

    /// Create a new unified state for bulk editing from pre-loaded audio files (convenience wrapper).
    ///
    /// Uses Individual mode - each file is edited separately, Tab navigates between them.
    pub fn bulk_from_audio_files(
        audio_files: Vec<AudioFile>,
        source: TagEditorSource,
        group_context: Option<GroupContext>,
    ) -> Self {
        Self::new(TagEditorMode::Individual, audio_files, source, group_context)
    }

    /// Create a new unified state for aggregated bulk editing (convenience wrapper).
    ///
    /// Uses Aggregated mode - shows unified view, changes apply to all files at once.
    pub fn aggregated_bulk(
        audio_files: Vec<AudioFile>,
        source: TagEditorSource,
    ) -> Self {
        Self::new(TagEditorMode::Aggregated, audio_files, source, None)
    }

    /// Create a new unified state for directory editing with aggregated tags (convenience wrapper).
    ///
    /// Uses Aggregated mode - shows unified view, changes apply to all files.
    pub fn directory_aggregated(
        audio_files: Vec<AudioFile>,
    ) -> Self {
        Self::new(TagEditorMode::Aggregated, audio_files, TagEditorSource::DirectoryEdit, None)
    }

    /// Builder method to set embedded mode (called after construction).
    pub fn with_embedded_mode(mut self, decision_key: crate::meta::decisions::DecisionKey, decision_label: String) -> Self {
        self.launch_mode = TagEditorLaunchMode::Embedded { decision_key, decision_label };
        self
    }

    /// Whether this editor is running in embedded mode (parent owns transaction).
    pub fn is_embedded(&self) -> bool {
        matches!(self.launch_mode, TagEditorLaunchMode::Embedded { .. })
    }

    /// Check if using Aggregated mode (unified view across all tracks)
    pub fn is_aggregated_mode(&self) -> bool {
        self.mode == TagEditorMode::Aggregated
    }

    // ========================================================================
    // Query Methods
    // ========================================================================

    /// Build a stable decision key item from the mutations being staged.
    ///
    /// Derives the key from the mutation content itself (inodes + tag names),
    /// so it's collision-free across editor sessions and works for individual,
    /// aggregated, directory, and search-result editing contexts alike.
    ///
    /// Same inode + same tags = overwrites (re-editing the same thing).
    /// Same inode + different tags = coexists (separate decisions pile up).
    pub fn decision_key_item(&self, mutations: &[Mutation]) -> String {
        use std::collections::BTreeSet;

        let mut inodes = BTreeSet::new();
        let mut tag_names = BTreeSet::new();

        for m in mutations {
            if let Mutation::ApplyTagOps(ref atm) = m {
                for op in &atm.ops {
                    inodes.insert(op.inode);
                    tag_names.insert(op.tag_name.to_uppercase());
                }
            }
        }

        let inode_part: String = inodes.iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");

        let tags_part: String = tag_names.into_iter()
            .collect::<Vec<_>>()
            .join(",");

        if tags_part.is_empty() {
            inode_part
        } else {
            format!("{}:{}", inode_part, tags_part)
        }
    }

    /// Get a label for the current item (for transaction decision labels)
    pub fn current_item_label(&self) -> String {
        match &self.context {
            TagEditContext::SingleFile { audio_file, .. } => {
                Path::new(audio_file.path())
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Unknown".to_string())
            }
            TagEditContext::BulkEdit { audio_files, .. } => {
                if let Some(audio_file) = audio_files.first() {
                    Path::new(audio_file.path())
                        .parent()
                        .and_then(|p| p.file_name())
                        .map(|d| d.to_string_lossy().to_string())
                        .unwrap_or_else(|| "Unknown".to_string())
                } else {
                    "Unknown".to_string()
                }
            }
        }
    }

    /// Check if there are any unsaved changes for the current item only
    pub fn has_changes_for_current_item(&self) -> bool {
        if self.is_aggregated_mode() {
            // In aggregated mode, changes apply to all tracks
            self.has_aggregated_changes()
        } else {
            compute_changes(&self.original_tag_fields, &self.tag_fields)
                .iter()
                .any(|c| c.track_idx == self.current_item_idx)
        }
    }

    /// Check if a specific item (by index) has unsaved changes
    pub fn item_has_changes(&self, idx: usize) -> bool {
        match (self.tag_fields.get(idx), self.original_tag_fields.get(idx)) {
            (Some(current), Some(original)) => current != original,
            _ => false,
        }
    }

    /// Check if any aggregated fields have been edited
    fn has_aggregated_changes(&self) -> bool {
        if let Some(ref agg_fields) = self.aggregated_fields {
            agg_fields.iter().any(|f| matches!(f.value, AggregatedValue::Edited(_)))
        } else {
            false
        }
    }

    /// Compute changes from aggregated fields, applying to all tracks
    fn compute_aggregated_changes(&self) -> Vec<TagChange> {
        let agg_fields = match &self.aggregated_fields {
            Some(f) => f,
            None => return Vec::new(),
        };

        let num_tracks = self.tag_fields.len();
        let mut changes = Vec::new();

        for field in agg_fields {
            if let AggregatedValue::Edited(new_value) = &field.value {
                // This field was edited - create a change for each track
                for track_idx in 0..num_tracks {
                    // Get the original value for this track
                    let old_value = self.original_tag_fields
                        .get(track_idx)
                        .and_then(|fields| {
                            fields.iter()
                                .find(|f| f.name.eq_ignore_ascii_case(&field.name))
                                .map(|f| f.value.clone())
                        })
                        .unwrap_or_default();

                    // Only add change if value actually differs
                    if old_value != *new_value {
                        changes.push(TagChange {
                            track_idx,
                            field_name: field.name.clone(),
                            old_value,
                            new_value: new_value.clone(),
                        });
                    }
                }
            }
        }

        changes
    }

    /// Generate mutations from current changes (all items)
    pub fn generate_mutations(&self) -> Vec<Mutation> {
        let changes = if self.is_aggregated_mode() {
            self.compute_aggregated_changes()
        } else {
            compute_changes(&self.original_tag_fields, &self.tag_fields)
        };

        let audio_files = match &self.context {
            TagEditContext::SingleFile { audio_file, .. } => vec![audio_file.clone()],
            TagEditContext::BulkEdit { audio_files, .. } => audio_files.clone(),
        };

        changes_to_mutations(&changes, &audio_files, &self.tag_fields)
    }

    /// Generate mutations for the current item only.
    /// This is what should be used when staging a decision for one file.
    pub fn generate_mutations_for_current_item(&self) -> Vec<Mutation> {
        if self.is_aggregated_mode() {
            // In aggregated mode, all changes apply to all files
            return self.generate_mutations();
        }

        let all_changes = compute_changes(&self.original_tag_fields, &self.tag_fields);

        // Filter to only changes for current_item_idx
        let current_changes: Vec<_> = all_changes
            .into_iter()
            .filter(|c| c.track_idx == self.current_item_idx)
            .collect();

        let audio_files = match &self.context {
            TagEditContext::SingleFile { audio_file, .. } => vec![audio_file.clone()],
            TagEditContext::BulkEdit { audio_files, .. } => audio_files.clone(),
        };

        changes_to_mutations(&current_changes, &audio_files, &self.tag_fields)
    }

    /// Collect mutations across ALL items that have changes.
    ///
    /// Used by embedded mode to gather edits from all files the user confirmed
    /// via Tab navigation, combined into a single mutation set for staging at
    /// the parent's decision index.
    pub fn collect_all_mutations(&self) -> Vec<Mutation> {
        if self.is_aggregated_mode() {
            return self.generate_mutations();
        }

        let audio_files = match &self.context {
            TagEditContext::SingleFile { audio_file, .. } => vec![audio_file.clone()],
            TagEditContext::BulkEdit { audio_files, .. } => audio_files.clone(),
        };

        let all_changes = compute_changes(&self.original_tag_fields, &self.tag_fields);
        if all_changes.is_empty() {
            return Vec::new();
        }

        changes_to_mutations(&all_changes, &audio_files, &self.tag_fields)
    }

    /// Revert to original state for current item only
    pub fn drop_changes_for_current_item(&mut self) {
        if let (Some(orig), Some(current)) = (
            self.original_tag_fields.get(self.current_item_idx),
            self.tag_fields.get_mut(self.current_item_idx),
        ) {
            *current = orig.clone();
        }
        self.field_edit_state = FieldEditState::NonEditable;
    }

    /// Check if current item's changes match what's already staged in the transaction.
    /// Used to skip confirmation dialogs when the user hasn't made new changes.
    pub fn changes_match_staged(&self) -> bool {
        match &self.staged_mutations_for_current {
            None => false, // Nothing staged yet, so can't match
            Some(staged) => {
                let current = self.generate_mutations_for_current_item();
                // Compare mutation sets - simple equality check
                // (mutations should be generated in same order)
                current == *staged
            }
        }
    }

    /// Set the staged mutations for the current item (called after staging a decision)
    pub fn set_staged_mutations(&mut self, mutations: Vec<Mutation>) {
        self.staged_mutations_for_current = Some(mutations);
    }

    /// Clear staged mutations (called when switching to a different item)
    pub fn clear_staged_mutations(&mut self) {
        self.staged_mutations_for_current = None;
    }

    /// Reset field navigation state (called when navigating between items)
    ///
    /// Combines the common pattern of resetting field_idx, scroll offset, and staged mutations.
    pub fn reset_field_state(&mut self) {
        self.current_field_idx = 0;
        self.field_scroll_offset = 0;
        self.clear_staged_mutations();
    }

    /// Get available action buttons based on context and signals
    pub fn available_buttons(&self) -> Vec<TagEditorButton> {
        let mut buttons = vec![TagEditorButton::ReviewAll, TagEditorButton::RevertThisFile];

        // Add Fill from Disk / Fill from DB only when OOB signal present
        if self.has_oob_signal {
            buttons.push(TagEditorButton::FillFromDisk);
            buttons.push(TagEditorButton::FillFromDb);
        }

        buttons
    }

    /// Get the current audio file being edited
    pub fn get_current_audio_file(&self) -> Option<&AudioFile> {
        match &self.context {
            TagEditContext::SingleFile { audio_file, .. } => Some(audio_file),
            TagEditContext::BulkEdit { audio_files, .. } => audio_files.get(self.current_item_idx),
        }
    }

    /// Re-read tags from disk for the current audio file
    pub fn fill_from_disk(&mut self) {
        let audio_file = match &self.context {
            TagEditContext::SingleFile { audio_file, .. } => audio_file.clone(),
            TagEditContext::BulkEdit { audio_files, .. } => {
                match audio_files.get(self.current_item_idx) {
                    Some(af) => af.clone(),
                    None => return,
                }
            }
        };

        // Re-read tags from disk
        let new_fields = audio_file_to_tag_fields(&audio_file);

        // Update current item's tag fields
        if let Some(fields) = self.tag_fields.get_mut(self.current_item_idx) {
            *fields = new_fields.clone();
        }

        // Also update original to reflect new baseline
        if let Some(orig_fields) = self.original_tag_fields.get_mut(self.current_item_idx) {
            *orig_fields = new_fields;
        }

        // Reset field position
        self.current_field_idx = 0;
        self.field_scroll_offset = 0;
        self.field_edit_state = FieldEditState::NonEditable;

        // Clear OOB signal since we've resolved it
        self.has_oob_signal = false;
    }

    /// Update tags from database result (called by UI layer after DB query)
    pub fn fill_from_db_result(&mut self, tags: Vec<(String, String)>) {
        // Convert database tags to TagField format
        let mut new_fields: Vec<TagField> = tags
            .into_iter()
            .map(|(name, value)| TagField {
                name,
                value,
                editable: true,
                deleted: false,
            })
            .collect();

        // Sort alphabetically by name
        new_fields.sort_by(|a, b| a.name.to_uppercase().cmp(&b.name.to_uppercase()));

        // Add "New Tag" placeholder at the end
        new_fields.push(TagField {
            name: "New Tag".to_string(),
            value: String::new(),
            editable: false,
            deleted: false,
        });

        // Update current item's tag fields
        if let Some(fields) = self.tag_fields.get_mut(self.current_item_idx) {
            *fields = new_fields.clone();
        }

        // Also update original to reflect new baseline
        if let Some(orig_fields) = self.original_tag_fields.get_mut(self.current_item_idx) {
            *orig_fields = new_fields;
        }

        // Reset field position
        self.current_field_idx = 0;
        self.field_scroll_offset = 0;
        self.field_edit_state = FieldEditState::NonEditable;

        // Clear OOB signal since we've resolved it
        self.has_oob_signal = false;
    }
}

impl std::fmt::Debug for UnifiedTagEditorState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnifiedTagEditorState")
            .field("context", &self.context)
            .field("current_item_idx", &self.current_item_idx)
            .field("total_items", &self.total_items)
            .field("current_field_idx", &self.current_field_idx)
            .field("field_edit_state", &self.field_edit_state)
            .field("focus", &self.focus)
            .field("selected_button", &self.selected_button)
            .field("tag_fields_len", &self.tag_fields.len())
            .finish_non_exhaustive()
    }
}
