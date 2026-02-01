//! Tag Editor State Management
//!
//! Core state structure and conversion functions.
//!
//! Contains `UnifiedTagEditorState` for transaction-based editing.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::corpus::db::types::AudioFile;
use crate::corpus::mutations::Mutation;
use crate::corpus::paths;

use super::types::{
    AggregatedTagField, AggregatedValue, FieldEditState, GroupContext, GroupedChange,
    NavigationDirection, TagChange, TagEditContext, TagEditorButton, TagEditorMode,
    TagEditorSource, TagField, UnifiedTagEditorAction, UnifiedTagEditorFocus,
    UnifiedTagEditorModal,
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
    // Staged Mutations Tracking (for skipping redundant confirmations)
    // ========================================================================

    /// Staged mutations for current item (set after StageDecision, cleared on item change)
    /// Used to skip confirmation dialog when changes match what's already staged.
    pub staged_mutations_for_current: Option<Vec<Mutation>>,
}

impl UnifiedTagEditorState {
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
            selected_button: TagEditorButton::Confirm,
            modal: None,
            has_oob_signal: false,
            staged_mutations_for_current: None,
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

    /// Check if using Aggregated mode (unified view across all tracks)
    pub fn is_aggregated_mode(&self) -> bool {
        self.mode == TagEditorMode::Aggregated
    }

    /// Check if there are multiple items to navigate between
    pub fn has_multiple_items(&self) -> bool {
        self.total_items > 1
    }

    // ========================================================================
    // Query Methods
    // ========================================================================

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
                if let Some(audio_file) = audio_files.get(self.current_item_idx) {
                    Path::new(audio_file.path())
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
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

    /// Check if any aggregated fields have been edited
    fn has_aggregated_changes(&self) -> bool {
        if let Some(ref agg_fields) = self.aggregated_fields {
            agg_fields.iter().any(|f| matches!(f.value, AggregatedValue::Edited(_)))
        } else {
            false
        }
    }

    /// Get changes for preview (all items)
    /// Get changes for preview (current item only)
    pub fn get_changes_for_current_item_preview(&self) -> (Vec<GroupedChange>, Vec<TagChange>) {
        if self.is_aggregated_mode() {
            // In aggregated mode, changes apply to all tracks - show them all
            let changes = self.compute_aggregated_changes();
            group_common_changes(&changes)
        } else {
            let all_changes = compute_changes(&self.original_tag_fields, &self.tag_fields);
            let current_changes: Vec<_> = all_changes
                .into_iter()
                .filter(|c| c.track_idx == self.current_item_idx)
                .collect();
            group_common_changes(&current_changes)
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
        let mut buttons = vec![TagEditorButton::Confirm, TagEditorButton::DropChanges];

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
        new_fields.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

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

// ============================================================================
// Unified Tag Editor Input Handling
// ============================================================================

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl UnifiedTagEditorState {
    /// Handle a key event
    pub fn handle_key(&mut self, key: KeyEvent) -> UnifiedTagEditorAction {
        // Handle modal first if active
        if self.modal.is_some() {
            return self.handle_modal_key(key);
        }

        match self.focus {
            UnifiedTagEditorFocus::TagFields => self.handle_tag_fields_key(key),
            UnifiedTagEditorFocus::Actions => self.handle_actions_key(key),
        }
    }

    fn handle_modal_key(&mut self, key: KeyEvent) -> UnifiedTagEditorAction {
        // Handle ChangePreview Enter/Esc separately to avoid borrow conflicts
        if let Some(UnifiedTagEditorModal::ChangePreview { direction, .. }) = &self.modal {
            let dir = *direction;
            match key.code {
                KeyCode::Enter => {
                    // Confirm changes -> stage decision for current item
                    let mutations = self.generate_mutations_for_current_item();
                    self.modal = None;

                    // If there are multiple items to navigate, go in the requested direction.
                    // Otherwise (aggregated mode or single item), go directly to review.
                    if self.has_multiple_items() {
                        match dir {
                            NavigationDirection::Forward => {
                                return UnifiedTagEditorAction::StageDecisionAndNext {
                                    index: self.current_item_idx,
                                    mutations,
                                };
                            }
                            NavigationDirection::Backward => {
                                return UnifiedTagEditorAction::StageDecisionAndPrev {
                                    index: self.current_item_idx,
                                    mutations,
                                };
                            }
                        }
                    } else {
                        return UnifiedTagEditorAction::StageDecisionAndReview {
                            index: self.current_item_idx,
                            mutations,
                        };
                    }
                }
                KeyCode::Esc => {
                    self.modal = None;
                    return UnifiedTagEditorAction::CloseModal;
                }
                _ => {} // Fall through to mutable match for scroll handling
            }
        }

        match &mut self.modal {
            Some(UnifiedTagEditorModal::ChangePreview { scroll, .. }) => {
                match key.code {
                    KeyCode::Up => {
                        *scroll = scroll.saturating_sub(1);
                        UnifiedTagEditorAction::None
                    }
                    KeyCode::Down => {
                        *scroll += 1;
                        UnifiedTagEditorAction::None
                    }
                    _ => UnifiedTagEditorAction::None,
                }
            }
            Some(UnifiedTagEditorModal::UnsavedChanges { selected_button }) => {
                match key.code {
                    KeyCode::Enter => {
                        // Execute selected button action
                        match selected_button {
                            super::types::UnsavedChangesButton::KeepEditing => {
                                self.modal = None;
                                UnifiedTagEditorAction::CloseModal
                            }
                            super::types::UnsavedChangesButton::DiscardAndProceed => {
                                self.drop_changes_for_current_item();
                                self.modal = None;
                                // UnsavedChanges is now only used for Exit
                                UnifiedTagEditorAction::DiscardTransaction
                            }
                        }
                    }
                    KeyCode::Esc => {
                        // Escape always cancels (keeps editing)
                        self.modal = None;
                        UnifiedTagEditorAction::CloseModal
                    }
                    KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                        // Toggle button selection
                        *selected_button = match selected_button {
                            super::types::UnsavedChangesButton::KeepEditing => {
                                super::types::UnsavedChangesButton::DiscardAndProceed
                            }
                            super::types::UnsavedChangesButton::DiscardAndProceed => {
                                super::types::UnsavedChangesButton::KeepEditing
                            }
                        };
                        UnifiedTagEditorAction::None
                    }
                    _ => UnifiedTagEditorAction::None,
                }
            }
            Some(UnifiedTagEditorModal::MultiValueEditor {
                field_idx,
                values,
                current_value_idx,
                editing,
                edit_buffer,
            }) => {
                let num_values = values.len();
                let add_entry_idx = num_values; // "+ Add value" is at end

                match key.code {
                    KeyCode::Esc => {
                        if *editing {
                            // Cancel edit, revert buffer
                            *editing = false;
                            edit_buffer.clear();
                        } else {
                            // Close modal, apply changes back to tag_fields
                            let field_idx_copy = *field_idx;
                            let values_copy = values.clone();
                            self.apply_multi_value_changes(field_idx_copy, values_copy);
                            self.modal = None;
                        }
                        UnifiedTagEditorAction::None
                    }
                    KeyCode::Enter => {
                        if *editing {
                            // Commit edit
                            if *current_value_idx < num_values {
                                values[*current_value_idx] = edit_buffer.clone();
                            } else if *current_value_idx == add_entry_idx && !edit_buffer.is_empty() {
                                // Adding new value
                                values.push(edit_buffer.clone());
                            }
                            *editing = false;
                            edit_buffer.clear();
                        } else if *current_value_idx < num_values {
                            // Start editing existing value
                            *editing = true;
                            *edit_buffer = values[*current_value_idx].clone();
                        } else if *current_value_idx == add_entry_idx {
                            // Start adding new value
                            *editing = true;
                            edit_buffer.clear();
                        }
                        UnifiedTagEditorAction::None
                    }
                    KeyCode::Up => {
                        if !*editing && *current_value_idx > 0 {
                            *current_value_idx -= 1;
                        }
                        UnifiedTagEditorAction::None
                    }
                    KeyCode::Down => {
                        if !*editing && *current_value_idx < add_entry_idx {
                            *current_value_idx += 1;
                        }
                        UnifiedTagEditorAction::None
                    }
                    KeyCode::Delete | KeyCode::Backspace if !*editing => {
                        // Delete current value (not the add entry)
                        if *current_value_idx < num_values && num_values > 0 {
                            values.remove(*current_value_idx);
                            if *current_value_idx >= values.len() && !values.is_empty() {
                                *current_value_idx = values.len() - 1;
                            }
                        }
                        UnifiedTagEditorAction::None
                    }
                    KeyCode::Backspace if *editing => {
                        edit_buffer.pop();
                        UnifiedTagEditorAction::None
                    }
                    KeyCode::Char(c) if *editing => {
                        edit_buffer.push(c);
                        UnifiedTagEditorAction::None
                    }
                    _ => UnifiedTagEditorAction::None,
                }
            }
            None => UnifiedTagEditorAction::None,
        }
    }

    fn handle_tag_fields_key(&mut self, key: KeyEvent) -> UnifiedTagEditorAction {
        match key.code {
            KeyCode::Esc => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    self.field_edit_state = FieldEditState::NonEditable;
                    UnifiedTagEditorAction::None
                } else if self.has_changes_for_current_item() {
                    self.modal = Some(UnifiedTagEditorModal::UnsavedChanges {
                        selected_button: super::types::UnsavedChangesButton::default(),
                    });
                    UnifiedTagEditorAction::None
                } else {
                    UnifiedTagEditorAction::DiscardTransaction
                }
            }
            KeyCode::Up => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    self.commit_field_buffer();
                }
                if self.current_field_idx > 0 {
                    self.current_field_idx -= 1;
                    if self.current_field_idx < self.field_scroll_offset {
                        self.field_scroll_offset = self.current_field_idx;
                    }
                }
                self.load_field_buffer();
                UnifiedTagEditorAction::None
            }
            KeyCode::Down => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    self.commit_field_buffer();
                }
                let max_fields = if self.is_aggregated_mode() {
                    self.aggregated_fields.as_ref().map(|f| f.len()).unwrap_or(0)
                } else {
                    self.tag_fields.get(self.current_item_idx).map(|f| f.len()).unwrap_or(0)
                };
                if self.current_field_idx < max_fields.saturating_sub(1) {
                    self.current_field_idx += 1;
                    let visible_end = self.field_scroll_offset + self.field_visible_height.saturating_sub(1);
                    if self.current_field_idx >= visible_end {
                        self.field_scroll_offset = self.current_field_idx.saturating_sub(self.field_visible_height.saturating_sub(2));
                    }
                }
                self.load_field_buffer();
                UnifiedTagEditorAction::None
            }
            KeyCode::Left => {
                // Left arrow: move focus from value to name (ContextList is not focusable)
                if self.field_edit_state == FieldEditState::NonEditable && self.focus_on_value {
                    self.focus_on_value = false;
                }
                UnifiedTagEditorAction::None
            }
            KeyCode::Right => {
                if self.field_edit_state == FieldEditState::NonEditable {
                    if self.focus_on_value {
                        self.focus = UnifiedTagEditorFocus::Actions;
                    } else {
                        self.focus_on_value = true;
                    }
                }
                UnifiedTagEditorAction::None
            }
            KeyCode::Tab => {
                // Tab: advance to next item (show change preview if current item has changes)
                // Skip confirmation if changes match what's already staged
                if self.has_changes_for_current_item() && !self.changes_match_staged() {
                    let (grouped, single) = self.get_changes_for_current_item_preview();
                    self.modal = Some(UnifiedTagEditorModal::ChangePreview {
                        changes: grouped,
                        single_changes: single,
                        scroll: 0,
                        direction: NavigationDirection::Forward,
                    });
                    UnifiedTagEditorAction::None
                } else {
                    UnifiedTagEditorAction::NextItem
                }
            }
            KeyCode::BackTab => {
                // Shift-Tab: go to previous item (show change preview if current item has changes)
                // Skip confirmation if current item's changes match what's already staged
                if self.has_changes_for_current_item() && !self.changes_match_staged() {
                    let (grouped, single) = self.get_changes_for_current_item_preview();
                    self.modal = Some(UnifiedTagEditorModal::ChangePreview {
                        changes: grouped,
                        single_changes: single,
                        scroll: 0,
                        direction: NavigationDirection::Backward,
                    });
                    UnifiedTagEditorAction::None
                } else {
                    UnifiedTagEditorAction::PrevItem
                }
            }
            KeyCode::Enter => {
                self.handle_field_enter();
                UnifiedTagEditorAction::None
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+R: Request transaction review
                // UI layer will query daemon for decisions and populate the modal
                UnifiedTagEditorAction::RequestTransactionReview
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.clear_current_field();
                UnifiedTagEditorAction::None
            }
            KeyCode::Char(c) => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    self.insert_char(c);
                }
                UnifiedTagEditorAction::None
            }
            KeyCode::Backspace => {
                if self.field_edit_state != FieldEditState::NonEditable {
                    self.delete_char();
                } else {
                    // In navigation mode, toggle deletion mark on current field
                    self.toggle_current_field_deleted();
                }
                UnifiedTagEditorAction::None
            }
            KeyCode::Delete => {
                if self.field_edit_state == FieldEditState::NonEditable {
                    // In navigation mode, toggle deletion mark on current field
                    self.toggle_current_field_deleted();
                }
                UnifiedTagEditorAction::None
            }
            _ => UnifiedTagEditorAction::None,
        }
    }

    fn handle_actions_key(&mut self, key: KeyEvent) -> UnifiedTagEditorAction {
        let buttons = self.available_buttons();
        let current_idx = buttons.iter().position(|b| *b == self.selected_button).unwrap_or(0);

        match key.code {
            KeyCode::Left | KeyCode::Esc => {
                self.focus = UnifiedTagEditorFocus::TagFields;
                UnifiedTagEditorAction::None
            }
            KeyCode::Up => {
                if current_idx > 0 {
                    self.selected_button = buttons[current_idx - 1];
                }
                UnifiedTagEditorAction::None
            }
            KeyCode::Down => {
                if current_idx < buttons.len().saturating_sub(1) {
                    self.selected_button = buttons[current_idx + 1];
                }
                UnifiedTagEditorAction::None
            }
            KeyCode::Enter => {
                match self.selected_button {
                    TagEditorButton::Confirm => {
                        let (grouped, single) = self.get_changes_for_current_item_preview();
                        if grouped.is_empty() && single.is_empty() {
                            UnifiedTagEditorAction::StatusMessage("No changes to save".to_string())
                        } else {
                            self.modal = Some(UnifiedTagEditorModal::ChangePreview {
                                changes: grouped,
                                single_changes: single,
                                scroll: 0,
                                direction: NavigationDirection::Forward,
                            });
                            UnifiedTagEditorAction::None
                        }
                    }
                    TagEditorButton::DropChanges => {
                        self.drop_changes_for_current_item();
                        UnifiedTagEditorAction::StatusMessage("Changes dropped".to_string())
                    }
                    TagEditorButton::FillFromDisk => {
                        self.fill_from_disk();
                        UnifiedTagEditorAction::StatusMessage("Tags refreshed from disk".to_string())
                    }
                    TagEditorButton::FillFromDb => {
                        // Return action for UI layer to handle (requires DB access)
                        let track_id = self.get_current_audio_file().map(|af| af.inode());
                        UnifiedTagEditorAction::RequestFillFromDb { track_id }
                    }
                }
            }
            _ => UnifiedTagEditorAction::None,
        }
    }

    // ========================================================================
    // Field Editing Helpers
    // ========================================================================

    fn load_field_buffer(&mut self) {
        if self.is_aggregated_mode() {
            // Aggregated mode - load from aggregated_fields
            if let Some(ref agg_fields) = self.aggregated_fields {
                if let Some(field) = agg_fields.get(self.current_field_idx) {
                    self.name_buffer = field.name.clone();
                    self.value_buffer = match &field.value {
                        AggregatedValue::Consistent(v) => v.clone(),
                        AggregatedValue::Edited(v) => v.clone(),
                        AggregatedValue::Various | AggregatedValue::VariousConfirming => String::new(),
                    };
                }
            }
        } else {
            // Individual mode - load from tag_fields
            if let Some(fields) = self.tag_fields.get(self.current_item_idx) {
                if let Some(field) = fields.get(self.current_field_idx) {
                    self.name_buffer = field.name.clone();
                    self.value_buffer = field.value.clone();
                }
            }
        }
    }

    fn commit_field_buffer(&mut self) {
        if self.is_aggregated_mode() {
            // Aggregated mode - update aggregated_fields
            if let Some(ref mut agg_fields) = self.aggregated_fields {
                if let Some(field) = agg_fields.get_mut(self.current_field_idx) {
                    match self.field_edit_state {
                        FieldEditState::EditingName => {
                            field.name = self.name_buffer.clone();
                        }
                        FieldEditState::EditingValue => {
                            // Mark as Edited with new value
                            field.value = AggregatedValue::Edited(self.value_buffer.clone());
                        }
                        FieldEditState::NonEditable => {}
                    }
                }
            }
        } else {
            // Individual mode - update tag_fields
            if let Some(fields) = self.tag_fields.get_mut(self.current_item_idx) {
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
    }

    fn handle_field_enter(&mut self) {
        if self.is_aggregated_mode() {
            self.handle_aggregated_field_enter();
        } else {
            self.handle_individual_field_enter();
        }
    }

    fn handle_aggregated_field_enter(&mut self) {
        let field = match self.aggregated_fields.as_ref().and_then(|f| f.get(self.current_field_idx)) {
            Some(f) => f.clone(),
            None => return,
        };

        if self.field_edit_state == FieldEditState::NonEditable {
            // Check if this is a Various value that needs confirmation
            if matches!(field.value, AggregatedValue::Various) {
                // First Enter - mark as confirming
                if let Some(ref mut agg_fields) = self.aggregated_fields {
                    if let Some(f) = agg_fields.get_mut(self.current_field_idx) {
                        f.value = AggregatedValue::VariousConfirming;
                    }
                }
            } else if matches!(field.value, AggregatedValue::VariousConfirming) {
                // Second Enter - now enter edit mode
                self.field_edit_state = FieldEditState::EditingValue;
                self.load_field_buffer();
            } else {
                // Consistent or Edited value - enter edit mode directly
                self.field_edit_state = FieldEditState::EditingValue;
                self.load_field_buffer();
            }
        } else {
            // Already editing - commit and exit edit mode
            self.commit_field_buffer();
            self.field_edit_state = FieldEditState::NonEditable;
        }
    }

    fn handle_individual_field_enter(&mut self) {
        let fields = match self.tag_fields.get(self.current_item_idx) {
            Some(f) => f,
            None => return,
        };
        let field = match fields.get(self.current_field_idx) {
            Some(f) => f,
            None => return,
        };

        if field.name == "New Tag" {
            self.create_new_tag();
        } else if self.field_edit_state == FieldEditState::NonEditable {
            // Check if this field has multiple values (multi-value tag)
            if self.is_multi_value_field().is_some() {
                // Open multi-value editor modal
                self.open_multi_value_editor();
            } else {
                // Single value - enter normal edit mode
                self.field_edit_state = if self.focus_on_value {
                    FieldEditState::EditingValue
                } else {
                    FieldEditState::EditingName
                };
                self.load_field_buffer();
            }
        } else {
            self.commit_field_buffer();
            self.field_edit_state = FieldEditState::NonEditable;
        }
    }

    fn create_new_tag(&mut self) {
        let new_field = TagField {
            name: "new_tag".to_string(),
            value: String::new(),
            editable: true,
            deleted: false,
        };

        if let Some(fields) = self.tag_fields.get_mut(self.current_item_idx) {
            let insert_pos = fields.len().saturating_sub(1);
            fields.insert(insert_pos, new_field);
            self.current_field_idx = insert_pos;
        }

        self.field_edit_state = FieldEditState::EditingName;
        self.name_buffer = "new_tag".to_string();
        self.value_buffer.clear();
        self.focus_on_value = false;
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

    fn clear_current_field(&mut self) {
        match self.field_edit_state {
            FieldEditState::EditingName => {
                self.name_buffer.clear();
            }
            FieldEditState::EditingValue => {
                self.value_buffer.clear();
            }
            FieldEditState::NonEditable => {
                if let Some(fields) = self.tag_fields.get_mut(self.current_item_idx) {
                    if let Some(field) = fields.get_mut(self.current_field_idx) {
                        field.value.clear();
                    }
                }
            }
        }
    }

    fn toggle_current_field_deleted(&mut self) {
        if let Some(fields) = self.tag_fields.get_mut(self.current_item_idx) {
            if let Some(field) = fields.get_mut(self.current_field_idx) {
                // Only allow deletion toggle on editable fields that aren't the "New Tag" placeholder
                if field.editable && field.name != "New Tag" {
                    field.deleted = !field.deleted;
                }
            }
        }
    }

    /// Check if current field is part of a multi-value group and get the count
    fn is_multi_value_field(&self) -> Option<usize> {
        let fields = self.tag_fields.get(self.current_item_idx)?;
        let current_field = fields.get(self.current_field_idx)?;
        let normalized_name = current_field.name.to_lowercase();

        // Count fields with same normalized name
        let count = fields
            .iter()
            .filter(|f| f.name.to_lowercase() == normalized_name && f.name != "New Tag")
            .count();

        if count > 1 {
            Some(count)
        } else {
            None
        }
    }

    /// Open multi-value editor modal for current field
    fn open_multi_value_editor(&mut self) {
        let fields = match self.tag_fields.get(self.current_item_idx) {
            Some(f) => f,
            None => return,
        };
        let current_field = match fields.get(self.current_field_idx) {
            Some(f) => f,
            None => return,
        };

        let normalized_name = current_field.name.to_lowercase();
        let values: Vec<String> = fields
            .iter()
            .filter(|f| f.name.to_lowercase() == normalized_name)
            .map(|f| f.value.clone())
            .collect();

        self.modal = Some(UnifiedTagEditorModal::MultiValueEditor {
            field_idx: self.current_field_idx,
            values,
            current_value_idx: 0,
            editing: false,
            edit_buffer: String::new(),
        });
    }
}

// ============================================================================
// Unified Tag Editor Rendering
// ============================================================================

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::ui::widgets::{PaneConfig, ThreePaneLayout};

impl UnifiedTagEditorState {
    /// Render the unified tag editor
    pub fn render(&mut self, f: &mut Frame, area: Rect, status_message: Option<&str>) {
        // Layout: info pane | 3-column | status box
        let editor_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(5),  // Info pane
                Constraint::Min(15),    // 3-column area
            ])
            .split(area);

        self.render_info_pane(f, editor_layout[0]);
        self.render_three_column(f, editor_layout[1]);
        // TODO: Controls hints should be displayed in the centralized bottom panel
        // based on active UiMode/modal. Design a ControlsContext trait or similar
        // that each mode can implement to provide context-sensitive controls.
        let _ = status_message; // Status messages also need centralized handling

        // Render modal overlay if active
        if let Some(modal) = &self.modal {
            self.render_modal(f, area, modal);
        }
    }

    fn render_info_pane(&self, f: &mut Frame, area: Rect) {
        let (path, file_type, file_size, duration_ms, bitrate, sample_rate) = match &self.context {
            TagEditContext::SingleFile { audio_file, .. } => (
                audio_file.path().to_string(),
                audio_file.audio.file_type.clone(),
                audio_file.entry.file_size,
                audio_file.audio.duration_ms,
                audio_file.audio.bitrate_kbps,
                audio_file.audio.sample_rate,
            ),
            TagEditContext::BulkEdit { audio_files, .. } => {
                if let Some(audio_file) = audio_files.get(self.current_item_idx) {
                    (
                        audio_file.path().to_string(),
                        audio_file.audio.file_type.clone(),
                        audio_file.entry.file_size,
                        audio_file.audio.duration_ms,
                        audio_file.audio.bitrate_kbps,
                        audio_file.audio.sample_rate,
                    )
                } else {
                    return;
                }
            }
        };

        let duration_str = duration_ms
            .map(|ms| {
                let total_seconds = ms / 1000;
                format!("{}:{:02}", total_seconds / 60, total_seconds % 60)
            })
            .unwrap_or_else(|| "Unknown".to_string());

        let size_str = if file_size < 1024 {
            format!("{} B", file_size)
        } else if file_size < 1024 * 1024 {
            format!("{:.1} KB", file_size as f64 / 1024.0)
        } else {
            format!("{:.2} MB", file_size as f64 / (1024.0 * 1024.0))
        };

        let bitrate_str = bitrate
            .map(|b| format!("{} kbps", b))
            .unwrap_or_else(|| "Unknown".to_string());

        let sample_rate_str = sample_rate
            .map(|sr| format!("{} Hz", sr))
            .unwrap_or_else(|| "Unknown".to_string());

        let mut info_lines = vec![
            Line::from(format!("Path: {}", path)),
            Line::from(format!(
                "Format: {} | Size: {} | Duration: {} | Bitrate: {} | Sample Rate: {}",
                file_type.to_uppercase(),
                size_str,
                duration_str,
                bitrate_str,
                sample_rate_str
            )),
        ];

        // Add MP3 warning if applicable
        let is_mp3 = file_type.to_lowercase() == "mp3";
        if is_mp3 {
            info_lines.push(Line::from(
                Span::styled(
                    "Warning: MP3 files use ID3v2.3 tags with limited field support. Some tag writes may fail.",
                    Style::default().fg(Color::Rgb(255, 140, 0)) // Orange
                )
            ));
        }

        let title = format!(
            "File Info [{}/{}]",
            self.current_item_idx + 1,
            self.total_items
        );

        let info_para = Paragraph::new(info_lines)
            .block(Block::default().borders(Borders::ALL).title(title));
        f.render_widget(info_para, area);
    }

    fn render_three_column(&mut self, f: &mut Frame, area: Rect) {
        let layout = ThreePaneLayout::horizontal()
            .left(PaneConfig::new("", 30))
            .middle(PaneConfig::new("", 55))
            .right(PaneConfig::new("", 15))
            .build(area);

        self.render_context_list(f, layout.left.area);
        self.render_tag_fields_pane(f, layout.middle.area);
        self.render_action_panel(f, layout.right.area);
    }

    fn render_context_list(&self, f: &mut Frame, area: Rect) {
        // ContextList is display-only (not focusable), so border is never highlighted

        let (items, title): (Vec<Line>, &str) = if self.is_aggregated_mode() {
            // Aggregated mode - show single summary entry (no individual track navigation)
            let count = self.total_items;
            let summary = format!(">> {} tracks", count);
            let lines = vec![
                Line::from(summary).style(Style::default().bg(Color::DarkGray)),
                Line::from(""),
                Line::from("(bulk edit)").style(Style::default().fg(Color::DarkGray)),
            ];
            (lines, "Selection")
        } else {
            // Standard file-based rendering (Individual mode)
            let items: Vec<Line> = match &self.context {
                TagEditContext::SingleFile { audio_file, group_context, .. } => {
                    // Single file mode - show the filename
                    let filename = std::path::Path::new(audio_file.path())
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Unknown");
                    let line = format!(">> {}", filename);

                    let mut lines = vec![Line::from(line).style(Style::default().bg(Color::DarkGray))];

                    // Show group context if present
                    if let Some(gc) = group_context {
                        lines.push(Line::from(""));
                        lines.push(Line::from(format!(
                            "Group {}/{}",
                            gc.group_index + 1,
                            gc.total_groups
                        )).style(Style::default().fg(Color::DarkGray)));
                    }

                    lines
                }
                TagEditContext::BulkEdit { audio_files, .. } => {
                    // Bulk mode with Individual editing - show all files by filename
                    audio_files
                        .iter()
                        .enumerate()
                        .map(|(idx, audio_file)| {
                            let prefix = if idx == self.current_item_idx { ">> " } else { "   " };
                            let filename = std::path::Path::new(audio_file.path())
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("Unknown");
                            let line = format!("{}{}", prefix, filename);

                            let style = if idx == self.current_item_idx {
                                Style::default().bg(Color::DarkGray)
                            } else {
                                Style::default()
                            };
                            Line::from(line).style(style)
                        })
                        .collect()
                }
            };

            let title = match &self.context {
                TagEditContext::SingleFile { source, .. } => match source {
                    TagEditorSource::CorpusBrowser => "File",
                    TagEditorSource::DirectoryEdit => "File",
                    TagEditorSource::TagSearch => "Search Result",
                },
                TagEditContext::BulkEdit { source, .. } => match source {
                    TagEditorSource::DirectoryEdit => "Directory",
                    _ => "Files",
                },
            };
            (items, title)
        };

        let list_para = Paragraph::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title),
        );
        f.render_widget(list_para, area);
    }

    fn render_tag_fields_pane(&mut self, f: &mut Frame, area: Rect) {
        let is_focused = matches!(self.focus, UnifiedTagEditorFocus::TagFields);

        // Check if we should render aggregated fields (directory edit mode)
        if let Some(ref agg_fields) = self.aggregated_fields {
            self.render_aggregated_fields_pane(f, area, is_focused, agg_fields.clone());
            return;
        }

        let fields = match self.tag_fields.get(self.current_item_idx) {
            Some(f) => f,
            None => return,
        };

        let original_fields = self.original_tag_fields.get(self.current_item_idx);

        // Calculate visible height
        let visible_height = area.height.saturating_sub(2) as usize;
        self.field_visible_height = visible_height;

        // Build field lines
        let field_lines: Vec<Line> = fields
            .iter()
            .enumerate()
            .map(|(idx, field)| {
                let is_current = idx == self.current_field_idx;

                // Check if modified: field is unmodified if an original (name, value) pair exists
                // This correctly handles multi-value tags (e.g., multiple Genre entries)
                let is_modified = original_fields
                    .map(|orig| {
                        !orig.iter().any(|o| {
                            o.name.eq_ignore_ascii_case(&field.name)
                                && o.value == field.value
                                && !o.deleted
                        })
                    })
                    .unwrap_or(true);

                let name_display = if is_current && matches!(self.field_edit_state, FieldEditState::EditingName) {
                    format!("{}▌", self.name_buffer)
                } else {
                    field.name.clone()
                };

                // Show indicator: deleted (✗), modified (✎), or none
                let name_with_indicator = if field.deleted {
                    format!("✗ {}", name_display)
                } else if is_modified {
                    format!("✎ {}", name_display)
                } else {
                    format!("  {}", name_display)
                };

                // Check if this field is part of a multi-value group
                let normalized_name = field.name.to_lowercase();
                let value_count = fields
                    .iter()
                    .filter(|f| f.name.to_lowercase() == normalized_name && f.name != "New Tag")
                    .count();
                let is_first_of_group = fields
                    .iter()
                    .position(|f| f.name.to_lowercase() == normalized_name)
                    .map(|pos| pos == idx)
                    .unwrap_or(true);

                let value_display = if is_current && matches!(self.field_edit_state, FieldEditState::EditingValue) {
                    format!("{}▌", self.value_buffer)
                } else if value_count > 1 && is_first_of_group {
                    // First occurrence of a multi-value tag - show count
                    format!("[{} values]", value_count)
                } else if value_count > 1 {
                    // Subsequent occurrence - show value with indent
                    format!("  └ {}", field.value)
                } else {
                    field.value.clone()
                };

                // Pad name to fixed width for alignment
                let name_padded = format!("{:18}", name_with_indicator);

                // Determine styles for name and value separately
                let (mut name_style, mut value_style) = if is_current {
                    // When current row, highlight the focused part (name or value)
                    let base = Style::default().add_modifier(Modifier::BOLD);
                    if self.focus_on_value {
                        // Value is focused - highlight value, dim name
                        (
                            base.bg(Color::DarkGray),
                            base.bg(Color::Cyan).fg(Color::Black),
                        )
                    } else {
                        // Name is focused - highlight name, dim value
                        (
                            base.bg(Color::Cyan).fg(Color::Black),
                            base.bg(Color::DarkGray),
                        )
                    }
                } else if field.deleted {
                    // Deleted fields shown in red
                    let del_style = Style::default().fg(Color::Red);
                    (del_style, del_style)
                } else if is_modified {
                    let mod_style = Style::default().fg(Color::Yellow);
                    (mod_style, mod_style)
                } else {
                    (Style::default(), Style::default())
                };

                // Apply strikethrough for deleted fields
                if field.deleted {
                    name_style = name_style.add_modifier(Modifier::CROSSED_OUT);
                    value_style = value_style.add_modifier(Modifier::CROSSED_OUT);
                }

                Line::from(vec![
                    Span::styled(name_padded, name_style),
                    Span::raw(" : "),
                    Span::styled(value_display, value_style),
                ])
            })
            .collect();

        // Scroll indicator
        let total_fields = field_lines.len();
        let scroll_indicator = if total_fields > visible_height {
            let pos = self.field_scroll_offset + 1;
            let max = total_fields.saturating_sub(visible_height) + 1;
            format!(" [{}/{}]", pos, max)
        } else {
            String::new()
        };

        // Apply scroll
        let visible_lines: Vec<Line> = field_lines
            .into_iter()
            .skip(self.field_scroll_offset)
            .take(visible_height)
            .collect();

        let border_style = if is_focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        };

        let tag_para = Paragraph::new(visible_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Tag Fields{}", scroll_indicator))
                .border_style(border_style),
        );
        f.render_widget(tag_para, area);
    }

    /// Render aggregated fields for directory-level editing.
    /// Shows whether each tag is consistent across all files or varies.
    fn render_aggregated_fields_pane(
        &mut self,
        f: &mut Frame,
        area: Rect,
        is_focused: bool,
        agg_fields: Vec<AggregatedTagField>,
    ) {
        let visible_height = area.height.saturating_sub(2) as usize;
        self.field_visible_height = visible_height;

        let field_lines: Vec<Line> = agg_fields
            .iter()
            .enumerate()
            .map(|(idx, field)| {
                let is_current = idx == self.current_field_idx;

                // Check if modified
                let is_modified = field.value != field.original_value;

                let name_display = if is_current
                    && matches!(self.field_edit_state, FieldEditState::EditingName)
                {
                    format!("{}▌", self.name_buffer)
                } else {
                    field.name.clone()
                };

                // Show indicator: modified (✎) or none
                let name_with_indicator = if is_modified {
                    format!("✎ {}", name_display)
                } else {
                    format!("  {}", name_display)
                };

                // Value display depends on AggregatedValue state
                let value_display = if is_current
                    && matches!(self.field_edit_state, FieldEditState::EditingValue)
                {
                    format!("{}▌", self.value_buffer)
                } else {
                    match &field.value {
                        AggregatedValue::Consistent(v) => {
                            if v.is_empty() {
                                "(empty)".to_string()
                            } else {
                                v.clone()
                            }
                        }
                        AggregatedValue::Various => "(various values)".to_string(),
                        AggregatedValue::VariousConfirming => "(press Enter to edit all)".to_string(),
                        AggregatedValue::Edited(v) => format!("→ {}", v),
                    }
                };

                // Pad name to fixed width for alignment
                let name_padded = format!("{:18}", name_with_indicator);

                // Determine styles
                let (name_style, value_style) = if is_current {
                    let base = Style::default().add_modifier(Modifier::BOLD);
                    if self.focus_on_value {
                        (
                            base.bg(Color::DarkGray),
                            base.bg(Color::Cyan).fg(Color::Black),
                        )
                    } else {
                        (
                            base.bg(Color::Cyan).fg(Color::Black),
                            base.bg(Color::DarkGray),
                        )
                    }
                } else if is_modified {
                    let mod_style = Style::default().fg(Color::Yellow);
                    (mod_style, mod_style)
                } else if matches!(field.value, AggregatedValue::Various) {
                    // Various values shown in magenta to draw attention
                    let var_style = Style::default().fg(Color::Magenta);
                    (Style::default(), var_style)
                } else {
                    (Style::default(), Style::default())
                };

                Line::from(vec![
                    Span::styled(name_padded, name_style),
                    Span::raw(" : "),
                    Span::styled(value_display, value_style),
                ])
            })
            .collect();

        // Scroll indicator
        let total_fields = field_lines.len();
        let scroll_indicator = if total_fields > visible_height {
            let pos = self.field_scroll_offset + 1;
            let max = total_fields.saturating_sub(visible_height) + 1;
            format!(" [{}/{}]", pos, max)
        } else {
            String::new()
        };

        let visible_lines: Vec<Line> = field_lines
            .into_iter()
            .skip(self.field_scroll_offset)
            .take(visible_height)
            .collect();

        let border_style = if is_focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        };

        // Show track count in title for directory view
        let title = format!("Directory Tags ({} files){}", self.total_items, scroll_indicator);

        let tag_para = Paragraph::new(visible_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(border_style),
        );
        f.render_widget(tag_para, area);
    }

    fn render_action_panel(&self, f: &mut Frame, area: Rect) {
        let is_focused = matches!(self.focus, UnifiedTagEditorFocus::Actions);
        let buttons = self.available_buttons();

        let mut lines = vec![Line::from("")];

        for button in &buttons {
            let is_selected = *button == self.selected_button;
            let label = match button {
                TagEditorButton::Confirm => "Confirm",
                TagEditorButton::DropChanges => "Drop Changes",
                TagEditorButton::FillFromDisk => "Fill from Disk",
                TagEditorButton::FillFromDb => "Fill from DB",
            };

            let style = if is_focused && is_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else if is_selected {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let text = if is_selected {
                format!("[ {} ]", label)
            } else {
                format!("  {}  ", label)
            };

            lines.push(Line::from(text).style(style));
        }

        // Change count
        let change_count = compute_changes(&self.original_tag_fields, &self.tag_fields).len();
        lines.push(Line::from(""));
        lines.push(
            Line::from(format!("{} changes", change_count))
                .style(Style::default().fg(Color::DarkGray)),
        );

        let border_style = if is_focused {
            Style::default().fg(Color::Green)
        } else {
            Style::default()
        };

        let action_para = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Actions")
                    .border_style(border_style),
            )
            .alignment(Alignment::Center);
        f.render_widget(action_para, area);
    }

    fn render_modal(&self, f: &mut Frame, area: Rect, modal: &UnifiedTagEditorModal) {
        match modal {
            UnifiedTagEditorModal::ChangePreview { changes, single_changes, scroll, direction: _ } => {
                self.render_change_preview_modal(f, area, changes, single_changes, *scroll);
            }
            UnifiedTagEditorModal::UnsavedChanges { selected_button } => {
                self.render_unsaved_changes_modal(f, area, *selected_button);
            }
            UnifiedTagEditorModal::MultiValueEditor {
                field_idx,
                values,
                current_value_idx,
                editing,
                edit_buffer,
            } => {
                self.render_multi_value_editor_modal(
                    f, area, *field_idx, values, *current_value_idx, *editing, edit_buffer,
                );
            }
        }
    }

    fn render_change_preview_modal(
        &self,
        f: &mut Frame,
        area: Rect,
        grouped_changes: &[GroupedChange],
        single_changes: &[TagChange],
        scroll_offset: usize,
    ) {
        let modal_area = crate::ui::helpers::centered_rect(80, 80, area);
        f.render_widget(Clear, modal_area);

        let modal_block = Block::default()
            .borders(Borders::ALL)
            .title("Review Changes")
            .border_style(Style::default().fg(Color::Yellow))
            .style(Style::default().bg(Color::Black));

        let inner = modal_block.inner(modal_area);
        f.render_widget(modal_block, modal_area);

        let mut lines = Vec::new();

        let total_changes = grouped_changes
            .iter()
            .map(|g| g.track_indices.len())
            .sum::<usize>()
            + single_changes.len();

        lines.push(
            Line::from(format!("Total changes: {}", total_changes))
                .style(Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan)),
        );
        lines.push(Line::from(""));

        if !grouped_changes.is_empty() {
            lines.push(
                Line::from("Common Changes:")
                    .style(Style::default().add_modifier(Modifier::BOLD).fg(Color::Green)),
            );
            for group in grouped_changes {
                let track_list = if group.track_indices.len() <= 5 {
                    group.track_indices.iter().map(|i| (i + 1).to_string()).collect::<Vec<_>>().join(", ")
                } else {
                    format!("{} tracks", group.track_indices.len())
                };
                lines.push(Line::from(format!(
                    "  [{}] {}: '{}' -> '{}'",
                    track_list,
                    group.field_name,
                    if group.old_value.is_empty() { "(empty)" } else { &group.old_value },
                    if group.new_value.is_empty() { "(empty)" } else { &group.new_value }
                )).style(Style::default().fg(Color::Cyan)));
            }
            lines.push(Line::from(""));
        }

        if !single_changes.is_empty() {
            lines.push(
                Line::from("Individual Changes:")
                    .style(Style::default().add_modifier(Modifier::BOLD).fg(Color::Yellow)),
            );
            for change in single_changes {
                lines.push(Line::from(format!(
                    "  Track {}: {}: '{}' -> '{}'",
                    change.track_idx + 1,
                    change.field_name,
                    if change.old_value.is_empty() { "(empty)" } else { &change.old_value },
                    if change.new_value.is_empty() { "(empty)" } else { &change.new_value }
                )).style(Style::default().fg(Color::White)));
            }
            lines.push(Line::from(""));
        }

        lines.push(Line::from(""));
        lines.push(
            Line::from("Enter = Stage to transaction | Esc = Cancel")
                .style(Style::default().fg(Color::DarkGray)),
        );

        let max_scroll = lines.len().saturating_sub(inner.height as usize);
        let clamped_offset = scroll_offset.min(max_scroll);
        let visible_lines: Vec<Line> = lines
            .into_iter()
            .skip(clamped_offset)
            .take(inner.height as usize)
            .collect();

        let paragraph = Paragraph::new(visible_lines).style(Style::default().bg(Color::Black));
        f.render_widget(paragraph, inner);
    }

    fn render_unsaved_changes_modal(
        &self,
        f: &mut Frame,
        area: Rect,
        selected_button: super::types::UnsavedChangesButton,
    ) {
        let modal_area = crate::ui::helpers::centered_rect_fixed(60, 12, area);
        f.render_widget(Clear, modal_area);

        let modal_block = Block::default()
            .borders(Borders::ALL)
            .title("Unsaved Changes")
            .border_style(Style::default().fg(Color::Yellow))
            .style(Style::default().bg(Color::Black));

        let inner = modal_block.inner(modal_area);
        f.render_widget(modal_block, modal_area);

        // Style buttons based on selection state (safe option selected by default)
        let (keep_style, discard_style) = match selected_button {
            super::types::UnsavedChangesButton::KeepEditing => (
                Style::default().fg(Color::Black).bg(Color::Green),
                Style::default().fg(Color::Red),
            ),
            super::types::UnsavedChangesButton::DiscardAndProceed => (
                Style::default().fg(Color::Green),
                Style::default().fg(Color::Black).bg(Color::Red),
            ),
        };

        let lines = vec![
            Line::from(""),
            Line::from("You have unsaved changes."),
            Line::from("Discard changes and exit the editor?"),
            Line::from(""),
            Line::from(vec![
                Span::styled(" Keep Editing ", keep_style),
                Span::raw("    "),
                Span::styled(" Discard ", discard_style),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "←/→/Tab to switch  •  Enter to confirm  •  Esc to cancel",
                Style::default().fg(Color::DarkGray),
            )),
        ];

        let paragraph = Paragraph::new(lines)
            .alignment(Alignment::Center)
            .style(Style::default().bg(Color::Black));
        f.render_widget(paragraph, inner);
    }

    fn render_multi_value_editor_modal(
        &self,
        f: &mut Frame,
        area: Rect,
        field_idx: usize,
        values: &[String],
        current_value_idx: usize,
        editing: bool,
        edit_buffer: &str,
    ) {
        // Get the field name for the title
        let field_name = self
            .tag_fields
            .get(self.current_item_idx)
            .and_then(|fields| fields.get(field_idx))
            .map(|f| f.name.as_str())
            .unwrap_or("Tag");

        // Size modal based on content
        let height = (values.len() + 5).min(15) as u16; // values + add entry + padding + controls
        let width = 50u16;
        let modal_area = crate::ui::helpers::centered_rect_fixed(width, height, area);
        f.render_widget(Clear, modal_area);

        let modal_block = Block::default()
            .borders(Borders::ALL)
            .title(format!("Edit: {}", field_name))
            .border_style(Style::default().fg(Color::Cyan))
            .style(Style::default().bg(Color::Black));

        let inner = modal_block.inner(modal_area);
        f.render_widget(modal_block, modal_area);

        let mut lines = Vec::new();

        // Render each value
        for (i, value) in values.iter().enumerate() {
            let is_current = i == current_value_idx;
            let display = if is_current && editing {
                format!("  > {}▌", edit_buffer)
            } else if is_current {
                format!("  > {}", value)
            } else {
                format!("    {}", value)
            };

            let style = if is_current {
                Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            lines.push(Line::from(display).style(style));
        }

        // "+ Add value" entry
        let add_idx = values.len();
        let is_add_current = current_value_idx == add_idx;
        let add_display = if is_add_current && editing {
            format!("  > + {}▌", edit_buffer)
        } else if is_add_current {
            "  > + Add value".to_string()
        } else {
            "    + Add value".to_string()
        };
        let add_style = if is_add_current {
            Style::default().fg(Color::Green).bg(Color::DarkGray).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };
        lines.push(Line::from(add_display).style(add_style));

        // Controls hint
        lines.push(Line::from(""));
        lines.push(
            Line::from("↑↓ Navigate | Enter Edit | Del Remove | Esc Close")
                .style(Style::default().fg(Color::DarkGray)),
        );

        let paragraph = Paragraph::new(lines).style(Style::default().bg(Color::Black));
        f.render_widget(paragraph, inner);
    }

    /// Apply multi-value changes back to the underlying tag_fields
    fn apply_multi_value_changes(&mut self, field_idx: usize, new_values: Vec<String>) {
        // Get the field name and determine which fields need updating
        let field_name = match self
            .tag_fields
            .get(self.current_item_idx)
            .and_then(|fields| fields.get(field_idx))
        {
            Some(f) => f.name.clone(),
            None => return,
        };

        let fields = match self.tag_fields.get_mut(self.current_item_idx) {
            Some(f) => f,
            None => return,
        };

        // Find all fields with the same normalized name
        let normalized_name = field_name.to_lowercase();
        let matching_indices: Vec<usize> = fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.name.to_lowercase() == normalized_name)
            .map(|(i, _)| i)
            .collect();

        // Remove existing fields with this name (in reverse order to preserve indices)
        for idx in matching_indices.into_iter().rev() {
            fields.remove(idx);
        }

        // Insert new values at the original position
        let insert_pos = field_idx.min(fields.len());
        for (i, value) in new_values.into_iter().enumerate() {
            fields.insert(
                insert_pos + i,
                TagField {
                    name: field_name.clone(),
                    value,
                    editable: true,
                        deleted: false,
                },
            );
        }
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

/// Convert TagField list to (tag_name, tag_value) pairs for mutations.
///
/// Filters out "New Tag" placeholder and deleted fields.
/// Returns the complete tag set as the final desired state.
fn tag_fields_to_tags(fields: &[TagField]) -> Vec<(String, String)> {
    fields
        .iter()
        .filter(|f| {
            f.name != "New Tag"
                && !f.deleted
                && !f.value.is_empty()
                && f.value != "[Press Enter to create]"
        })
        .map(|f| (f.name.to_lowercase(), f.value.clone()))
        .collect()
}

/// Convert changes to mutations for the daemon.
///
/// Uses the DB-first pattern with spawn chaining: for each file with changes, generates
/// SetTrackTagsDb which writes tags to DB, sets needs_disk_flush=true, and spawns
/// ApplyDbTagsToDisk to sync to disk and clear the flag.
///
/// This pattern ensures DB is always ahead of or in sync with disk, enabling
/// recovery via OOB flow if disk write fails/is interrupted.
fn changes_to_mutations(changes: &[TagChange], audio_files: &[AudioFile], all_tag_fields: &[Vec<TagField>]) -> Vec<Mutation> {
    // Get unique file indices that have changes
    let mut changed_files: HashSet<usize> = HashSet::new();
    for change in changes {
        changed_files.insert(change.track_idx);
    }

    // Generate one mutation per file: SetTrackTagsDb (spawns ApplyDbTagsToDisk)
    let mut mutations = Vec::new();
    for file_idx in changed_files {
        if let (Some(audio_file), Some(current_fields)) = (audio_files.get(file_idx), all_tag_fields.get(file_idx)) {
            // Use inode as track_id for the mutation
            let track_id = audio_file.inode();

            // Get complete desired tag set from current UI state
            let tags = tag_fields_to_tags(current_fields);

            // DB-first pattern with spawn chaining:
            // SetTrackTagsDb writes to DB and spawns ApplyDbTagsToDisk for disk sync
            mutations.push(Mutation::SetTrackTagsDb {
                track_id,
                tags,
            });
        }
    }

    mutations
}

// ============================================================================
// Conversion Functions
// ============================================================================

/// Load tag fields from disk for an audio file.
///
/// All tags are loaded from the audio file and sorted alphabetically.
/// Multi-value tags (e.g., multiple genres) are loaded as separate entries.
pub fn audio_file_to_tag_fields(audio_file: &AudioFile) -> Vec<TagField> {
    use crate::corpus::tags::TagSet;

    let resolver = paths::get_resolver();
    let disk_path = resolver.resolve(Path::new(audio_file.path()));

    let tag_set = match TagSet::from_file(&disk_path) {
        Ok(tags) => tags,
        Err(e) => {
            crate::logging::log_error(format!(
                "Could not read tags from {}: {}",
                disk_path.display(), e
            ));
            TagSet::empty()
        }
    };

    // Convert to TagField - TagSet is already sorted
    let mut tag_fields: Vec<TagField> = tag_set
        .into_vec()
        .into_iter()
        .map(|(name, value)| TagField {
            name,
            value,
            editable: true,
            deleted: false,
        })
        .collect();

    // Add "New Tag" placeholder at the end
    tag_fields.push(TagField {
        name: "New Tag".to_string(),
        value: "[Press Enter to create]".to_string(),
        editable: true,
        deleted: false,
    });

    tag_fields
}

/// Compute all changes between original and current tag fields
///
/// This function handles multi-value tags by comparing values semantically
/// rather than by position. It detects added, removed, and modified values
/// for each tag name.
pub fn compute_changes(original: &[Vec<TagField>], current: &[Vec<TagField>]) -> Vec<TagChange> {
    let mut changes = Vec::new();

    for (track_idx, (orig_fields, curr_fields)) in original.iter().zip(current.iter()).enumerate() {
        // Build maps of tag name -> values for both original and current
        let mut orig_values: HashMap<String, Vec<&TagField>> = HashMap::new();
        let mut curr_values: HashMap<String, Vec<&TagField>> = HashMap::new();

        for field in orig_fields {
            if field.name != "New Tag" {
                orig_values
                    .entry(field.name.to_lowercase())
                    .or_default()
                    .push(field);
            }
        }

        for field in curr_fields {
            if field.name != "New Tag" {
                curr_values
                    .entry(field.name.to_lowercase())
                    .or_default()
                    .push(field);
            }
        }

        // Collect all unique tag names
        let mut all_names: HashSet<String> = orig_values.keys().cloned().collect();
        all_names.extend(curr_values.keys().cloned());

        for normalized_name in all_names {
            let orig = orig_values.get(&normalized_name).map(|v| v.as_slice()).unwrap_or(&[]);
            let curr = curr_values.get(&normalized_name).map(|v| v.as_slice()).unwrap_or(&[]);

            // Get display name from current or original
            let display_name = curr
                .first()
                .map(|f| f.name.clone())
                .or_else(|| orig.first().map(|f| f.name.clone()))
                .unwrap_or_else(|| normalized_name.clone());

            // Check for deleted tags (marked deleted in current)
            for field in curr {
                if field.deleted {
                    // Find corresponding original value
                    let orig_value = orig
                        .iter()
                        .find(|o| o.value == field.value)
                        .map(|o| o.value.clone())
                        .unwrap_or_default();

                    changes.push(TagChange {
                        track_idx,
                        field_name: display_name.clone(),
                        old_value: orig_value,
                        new_value: field.value.clone(),
                    });
                }
            }

            // Skip further comparison if all values are deleted
            let curr_active: Vec<_> = curr.iter().filter(|f| !f.deleted).collect();
            if curr_active.is_empty() && !orig.is_empty() {
                continue; // Deletion changes already added above
            }

            // Compare active (non-deleted) values
            let orig_value_set: HashSet<_> = orig.iter().map(|f| &f.value).collect();
            let curr_value_set: HashSet<_> = curr_active.iter().map(|f| &f.value).collect();

            // Added values (in current but not in original)
            for field in &curr_active {
                if !orig_value_set.contains(&field.value) && !field.deleted {
                    changes.push(TagChange {
                        track_idx,
                        field_name: display_name.clone(),
                        old_value: String::new(), // New value, no old
                        new_value: field.value.clone(),
                    });
                }
            }

            // Removed values (in original but not in current active)
            for field in orig {
                if !curr_value_set.contains(&field.value) {
                    // Check if it's not already covered by a deletion flag
                    let is_deleted_explicitly = curr.iter().any(|c| c.value == field.value && c.deleted);
                    if !is_deleted_explicitly {
                        changes.push(TagChange {
                            track_idx,
                            field_name: display_name.clone(),
                            old_value: field.value.clone(),
                            new_value: String::new(), // Removed
                        });
                    }
                }
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
// Directory-Level Tag Aggregation (across all audio files)
// ============================================================================

/// Aggregate tags across all audio files in a directory.
///
/// For each tag name (case-insensitive):
/// - If all files have the same value → `AggregatedValue::Consistent(value)`
/// - If values differ across files → `AggregatedValue::Various`
///
/// Empty values are filtered out. Tags are sorted alphabetically.
/// This provides a unified view for directory-level tag editing where the user
/// can see which tags are consistent and which need attention.
pub fn aggregate_tags_across_audio_files(audio_files: &[AudioFile]) -> Vec<AggregatedTagField> {
    use std::collections::HashMap;

    if audio_files.is_empty() {
        return Vec::new();
    }

    // Collect all tag values per tag name across all files
    // Key: normalized tag name, Value: (display name, set of unique non-empty values)
    let mut tag_values: HashMap<String, (String, HashSet<String>)> = HashMap::new();

    for audio_file in audio_files {
        let fields = audio_file_to_tag_fields(audio_file);
        for field in fields {
            if field.name == "New Tag" {
                continue;
            }

            // Skip empty values
            if field.value.is_empty() {
                continue;
            }

            let normalized = field.name.to_lowercase();

            tag_values
                .entry(normalized.clone())
                .or_insert_with(|| (field.name.clone(), HashSet::new()))
                .1
                .insert(field.value);
        }
    }

    // Convert to AggregatedTagField entries
    let mut result: Vec<AggregatedTagField> = tag_values
        .into_iter()
        .map(|(_normalized, (display_name, values))| {
            let value = if values.len() == 1 {
                // All files have the same value
                AggregatedValue::Consistent(values.into_iter().next().unwrap_or_default())
            } else {
                // Files have different values
                AggregatedValue::Various
            };

            AggregatedTagField {
                name: display_name,
                value: value.clone(),
                original_value: value,
            }
        })
        .collect();

    // Sort alphabetically by tag name
    result.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    // Add "New Tag" placeholder at end
    result.push(AggregatedTagField {
        name: "New Tag".to_string(),
        value: AggregatedValue::Consistent(String::new()),
        original_value: AggregatedValue::Consistent(String::new()),
    });

    result
}
