//! Tag Editor State Management
//!
//! Thin wrapper around `mm_ui::tag_editor_state::TagEditorState` that adds
//! TUI-specific concerns: full audio file metadata for rendering, modal dialogs,
//! click targets, OOB signal tracking.

use std::path::Path;

use mm_meta::db_types::AudioFile;
use mm_meta::mutations::Mutation;

use mm_ui::tag_editor_state::{
    AudioFileInfo, TagEditorMode, TagEditorState,
};
use mm_ui::tag_set::TagSet;

use super::mutations::load_tag_sets_batch;
use super::types::{
    GroupContext, TagEditorLaunchMode, TagEditorSource, UnifiedTagEditorModal,
};

// ============================================================================
// UnifiedTagEditorState
// ============================================================================

/// TUI tag editor state — wraps shared `TagEditorState` with TUI-specific fields.
pub struct UnifiedTagEditorState {
    /// Core shared state (tag data, form, buttons, file nav).
    pub core: TagEditorState,

    /// Full audio file data (for info pane rendering, art preview, fill queries).
    pub audio_files: Vec<AudioFile>,

    /// Source context (where the editor was launched from).
    pub source: TagEditorSource,

    /// Group context for multi-step workflows.
    pub group_context: Option<GroupContext>,

    /// Active modal dialog (if any).
    pub modal: Option<UnifiedTagEditorModal>,

    /// Whether OOB (out-of-band) tag change signal is present.
    pub has_oob_signal: bool,

    /// Staged mutations for current item (for skip-confirmation logic).
    pub staged_mutations_for_current: Option<Vec<Mutation>>,

    /// Launch mode (standalone vs embedded).
    pub launch_mode: TagEditorLaunchMode,

    // ========================================================================
    // Click targets (TUI-specific, set during render)
    // ========================================================================
    pub field_click_targets: crate::widgets::ListClickTargets,
    pub action_click_targets: crate::widgets::ListClickTargets,
    pub fields_pane_rect: Option<ratatui::layout::Rect>,
    pub actions_pane_rect: Option<ratatui::layout::Rect>,
}

impl UnifiedTagEditorState {
    // ========================================================================
    // Constructors
    // ========================================================================

    /// Create editor state with pre-loaded TagSets.
    pub fn new(
        mode: TagEditorMode,
        audio_files: Vec<AudioFile>,
        source: TagEditorSource,
        group_context: Option<GroupContext>,
        tag_sets: Vec<TagSet>,
    ) -> Self {
        debug_assert!(
            audio_files
                .windows(2)
                .all(|w| w[0].entry.zone == w[1].entry.zone),
            "Tag editor opened with files from mixed zones"
        );

        let file_infos: Vec<AudioFileInfo> = audio_files
            .iter()
            .map(|af| AudioFileInfo {
                inode: af.inode(),
                zone: af.entry.zone,
                path: af.path().to_string(),
            })
            .collect();

        let core = TagEditorState::new(mode, file_infos, tag_sets);

        Self {
            core,
            audio_files,
            source,
            group_context,
            modal: None,
            has_oob_signal: false,
            staged_mutations_for_current: None,
            launch_mode: TagEditorLaunchMode::Standalone,
            field_click_targets: Default::default(),
            action_click_targets: Default::default(),
            fields_pane_rect: None,
            actions_pane_rect: None,
        }
    }

    /// Convenience: single file editor (loads tags from disk).
    pub(crate) fn single_file(
        audio_file: AudioFile,
        source: TagEditorSource,
        group_context: Option<GroupContext>,
        app: &mut crate::App,
    ) -> Self {
        let files = vec![audio_file];
        let tag_sets = load_tag_sets_batch(&files, app);
        Self::new(TagEditorMode::Individual, files, source, group_context, tag_sets)
    }

    /// Convenience: bulk individual mode (loads tags from disk).
    pub(crate) fn bulk_from_audio_files(
        audio_files: Vec<AudioFile>,
        source: TagEditorSource,
        group_context: Option<GroupContext>,
        app: &mut crate::App,
    ) -> Self {
        let tag_sets = load_tag_sets_batch(&audio_files, app);
        Self::new(TagEditorMode::Individual, audio_files, source, group_context, tag_sets)
    }

    /// Convenience: aggregated bulk mode (loads tags from disk).
    pub(crate) fn aggregated_bulk(
        audio_files: Vec<AudioFile>,
        source: TagEditorSource,
        app: &mut crate::App,
    ) -> Self {
        let tag_sets = load_tag_sets_batch(&audio_files, app);
        Self::new(TagEditorMode::Aggregated, audio_files, source, None, tag_sets)
    }

    /// Convenience: directory aggregated mode (loads tags from disk).
    pub(crate) fn directory_aggregated(
        audio_files: Vec<AudioFile>,
        app: &mut crate::App,
    ) -> Self {
        let tag_sets = load_tag_sets_batch(&audio_files, app);
        Self::new(
            TagEditorMode::Aggregated,
            audio_files,
            TagEditorSource::DirectoryEdit,
            None,
            tag_sets,
        )
    }

    /// Builder: set embedded mode.
    pub fn with_embedded_mode(
        mut self,
        decision_key: mm_meta::decisions::DecisionKey,
        decision_label: String,
    ) -> Self {
        self.core.launch_mode = mm_ui::tag_editor_state::TagEditorLaunchMode::Embedded {
            decision_key: decision_key.clone(),
            decision_label: decision_label.clone(),
        };
        self.launch_mode = TagEditorLaunchMode::Embedded {
            decision_key,
            decision_label,
        };
        self
    }

    // ========================================================================
    // Forwarding: identity & navigation
    // ========================================================================

    /// Whether this editor is embedded (parent owns transaction).
    pub fn is_embedded(&self) -> bool {
        matches!(self.launch_mode, TagEditorLaunchMode::Embedded { .. })
    }

    /// Whether using aggregated mode.
    pub fn is_aggregated_mode(&self) -> bool {
        self.core.mode == TagEditorMode::Aggregated
    }

    /// Path of the currently selected file.
    pub fn selected_path(&self) -> Option<&str> {
        self.audio_files
            .get(self.core.current_file)
            .map(|af| af.path())
    }

    /// Total number of files/items.
    pub fn total_items(&self) -> usize {
        self.audio_files.len()
    }

    /// Current item index.
    pub fn current_item_idx(&self) -> usize {
        self.core.current_file
    }

    /// Set current item index.
    pub fn set_current_item_idx(&mut self, idx: usize) {
        self.core.current_file = idx;
    }

    /// Get the current audio file.
    pub fn get_current_audio_file(&self) -> Option<&AudioFile> {
        self.audio_files.get(self.core.current_file)
    }

    /// Get a label for the current item (for decision labels).
    pub fn current_item_label(&self) -> String {
        if self.audio_files.len() == 1 {
            Path::new(self.audio_files[0].path())
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "Unknown".to_string())
        } else if let Some(af) = self.audio_files.first() {
            Path::new(af.path())
                .parent()
                .and_then(|p| p.file_name())
                .map(|d| d.to_string_lossy().to_string())
                .unwrap_or_else(|| "Unknown".to_string())
        } else {
            "Unknown".to_string()
        }
    }

    // ========================================================================
    // Forwarding: mutation generation
    // ========================================================================

    /// Check if current file has unsaved changes.
    pub fn has_changes_for_current_item(&self) -> bool {
        self.core.has_changes()
    }

    /// Check if a specific file has unsaved changes.
    pub fn item_has_changes(&self, idx: usize) -> bool {
        self.core.has_changes_for_file(idx)
    }

    /// Generate mutations for current item.
    pub fn generate_mutations_for_current_item(&self) -> Vec<Mutation> {
        self.core.mutations_for_current()
    }

    /// Generate mutations for all items.
    pub fn generate_mutations(&self) -> Vec<Mutation> {
        self.core.mutations_for_all()
    }

    /// Collect all mutations (for embedded mode).
    pub fn collect_all_mutations(&self) -> Vec<Mutation> {
        self.core.mutations_for_all()
    }

    /// Decision key from mutations.
    pub fn decision_key_item(&self, mutations: &[Mutation]) -> String {
        self.core.decision_key_item(mutations)
    }

    // ========================================================================
    // Forwarding: state management
    // ========================================================================

    /// Reset field navigation state (called when switching items).
    pub fn reset_field_state(&mut self) {
        self.core.form.reset();
        self.clear_staged_mutations();
    }

    /// Revert current file to original tags.
    pub fn drop_changes_for_current_item(&mut self) {
        self.core.revert_current();
    }

    /// Set staged mutations for skip-confirmation logic.
    pub fn set_staged_mutations(&mut self, mutations: Vec<Mutation>) {
        self.staged_mutations_for_current = Some(mutations);
    }

    /// Clear staged mutations.
    pub fn clear_staged_mutations(&mut self) {
        self.staged_mutations_for_current = None;
    }

    /// Check if current changes match what's already staged.
    pub fn changes_match_staged(&self) -> bool {
        match &self.staged_mutations_for_current {
            None => false,
            Some(staged) => {
                let current = self.generate_mutations_for_current_item();
                current == *staged
            }
        }
    }

    // ========================================================================
    // Fill operations
    // ========================================================================

    /// Get current file's inode and zone for a fill-from-disk query.
    pub fn current_file_for_disk_query(&self) -> Option<(i64, mm_meta::db_types::Zone)> {
        self.audio_files
            .get(self.core.current_file)
            .map(|af| (af.inode(), af.entry.zone))
    }

    /// Apply re-read tags from disk for the current file.
    pub fn fill_from_disk_with_tags(&mut self, tags: Vec<(String, String)>) {
        let new_set = TagSet::from_pairs(tags);
        let idx = self.core.current_file;

        if let Some(current) = self.core.tag_sets.get_mut(idx) {
            *current = new_set.clone();
        }
        if let Some(original) = self.core.original_tag_sets.get_mut(idx) {
            *original = new_set;
        }

        self.core.form.reset();
        self.has_oob_signal = false;
    }

    /// Apply tags from database for the current file.
    pub fn fill_from_db_result(&mut self, tags: Vec<(String, String)>) {
        let new_set = TagSet::from_pairs(tags);
        let idx = self.core.current_file;

        if let Some(current) = self.core.tag_sets.get_mut(idx) {
            *current = new_set.clone();
        }
        if let Some(original) = self.core.original_tag_sets.get_mut(idx) {
            *original = new_set;
        }

        self.core.form.reset();
        self.has_oob_signal = false;
    }

    // ========================================================================
    // Click handling
    // ========================================================================

    /// Handle mouse click for focus and cursor changes.
    pub fn handle_click(&mut self, x: u16, y: u16) {
        use crate::widgets::rect_contains;
        use mm_ui::tag_editor_state::FocusPane;

        // Check action button click targets
        if let Some(id) = self.action_click_targets.hit_test(x, y) {
            self.core.focus = FocusPane::Buttons;
            if let Ok(idx) = id.parse::<usize>() {
                use mm_ui::modal_buttons::ModalButtons;
                let ctx = self.core.button_ctx();
                let all = mm_ui::tag_editor_state::TagEditorButton::all();
                if let Some(&button) = all.get(idx) {
                    if button.enabled(&ctx) {
                        self.core.buttons.selected = button;
                    }
                }
            }
            return;
        }

        // Check field click targets
        if let Some(id) = self.field_click_targets.hit_test(x, y) {
            self.core.focus = FocusPane::Content;
            if let Ok(idx) = id.parse::<usize>() {
                let total = if self.is_aggregated_mode() {
                    self.core
                        .aggregated
                        .as_ref()
                        .map(|a| a.entry_count())
                        .unwrap_or(0)
                } else {
                    self.core
                        .tag_sets
                        .get(self.core.current_file)
                        .map(|ts| ts.entry_count())
                        .unwrap_or(0)
                };
                // +1 for "New Tag" sentinel
                if idx <= total {
                    self.core.form.cursor = idx;
                }
            }
            return;
        }

        // Pane-level focus detection
        if let Some(rect) = self.actions_pane_rect {
            if rect_contains(rect, x, y) {
                self.core.focus = FocusPane::Buttons;
                return;
            }
        }
        if let Some(rect) = self.fields_pane_rect {
            if rect_contains(rect, x, y) {
                self.core.focus = FocusPane::Content;
            }
        }
    }
}

impl std::fmt::Debug for UnifiedTagEditorState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnifiedTagEditorState")
            .field("source", &self.source)
            .field("current_file", &self.core.current_file)
            .field("total_items", &self.audio_files.len())
            .field("focus", &self.core.focus)
            .finish_non_exhaustive()
    }
}
