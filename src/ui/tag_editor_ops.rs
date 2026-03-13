//! Tag Editor Operations
//!
//! Functions for launching and navigating the unified tag editor in various
//! contexts (single file, bulk edit, directory edit, tag search results).

use crossterm::event;

use super::App;
use crate::db::types::AudioFile;
use crate::ui::suspended_views::SuspendTarget;
use crate::ui::tag_editor;
use crate::ui::ActiveView;


/// Drain any pending input events from the terminal buffer.
///
/// Call this after slow operations (like loading tags from disk) to prevent
/// buffered keypresses from being processed as if they were intentional input
/// in the newly loaded UI state.
fn drain_input_buffer() {
    while event::poll(std::time::Duration::ZERO).unwrap_or(false) {
        let _ = event::read();
    }
}

impl App {
    /// Start tag editor for a file path (from tree browser or external trigger).
    ///
    /// The input path is an absolute filesystem path. We convert to relative
    /// for database queries since the DB stores paths relative to corpus_root.
    pub(super) fn start_tag_editor_for_path(&mut self, path: &std::path::Path, recursive: bool) {
        // Convert absolute path to relative for DB queries (corpus browser uses corpus paths)
        let rel_path = match self.resolver.to_relative(path) {
            Some(p) => p,
            None => {
                self.abort_to_health(format!("Path not in corpus: {}", path.display()));
                return;
            }
        };

        // Load audio files from database using relative path
        let mode = if recursive {
            crate::db::domain::TagEditorLoadMode::Directory
        } else {
            crate::db::domain::TagEditorLoadMode::SingleFile
        };
        let (audio_files, selected_idx) = self
            .witch
            .query(crate::db::domain::GetTagEditorFiles {
                rel_path: rel_path.to_path_buf(),
                mode,
            });

        if audio_files.is_empty() {
            self.abort_to_health(format!("No indexed files at: {}", path.display()));
            return;
        }

        // Use the unified tag editor for single-file editing
        if audio_files.len() == 1 {
            // Single file - use single file mode
            self.open_unified_tag_editor_single(
                audio_files.into_iter().next().unwrap(),
                tag_editor::TagEditorSource::CorpusBrowser,
                None,
            );
        } else {
            // Multiple tracks in same directory - use bulk mode with CorpusBrowser source
            // This allows cycling through items with tab/shift-tab
            self.open_unified_tag_editor_bulk(
                audio_files,
                tag_editor::TagEditorSource::CorpusBrowser,
                None,
            );
            // Position on the selected file
            if let ActiveView::UnifiedTagEditor(ref mut editor) = self.view {
                editor.current_item_idx = selected_idx;
            }
        }
        self.status_message = Some(format!("Editing tags for {}", path.display()));
    }

    /// Open the unified tag editor with a single audio file
    pub(super) fn open_unified_tag_editor_single(
        &mut self,
        audio_file: AudioFile,
        source: tag_editor::TagEditorSource,
        group_context: Option<tag_editor::GroupContext>,
    ) {
        // Start transaction
        let label = match source {
            tag_editor::TagEditorSource::CorpusBrowser => "Tag edits",
            tag_editor::TagEditorSource::DirectoryEdit => "Directory tag edits",
            tag_editor::TagEditorSource::TagSearch => "Tag search edits",
            tag_editor::TagEditorSource::HealthModal => "Health tag edits",
        };
        let _ = self.witch.start_transaction(label);

        // NOTE: single_file reads tags from disk
        let editor =
            tag_editor::UnifiedTagEditorState::single_file(audio_file, source, group_context, &self.witch);

        // Drain any keypresses that accumulated during loading
        drain_input_buffer();

        self.view = ActiveView::UnifiedTagEditor(editor);
    }

    /// Open the unified tag editor with multiple audio files
    pub(super) fn open_unified_tag_editor_bulk(
        &mut self,
        audio_files: Vec<AudioFile>,
        source: tag_editor::TagEditorSource,
        group_context: Option<tag_editor::GroupContext>,
    ) {
        // Start transaction
        let label = match source {
            tag_editor::TagEditorSource::CorpusBrowser => "Bulk tag edits",
            tag_editor::TagEditorSource::DirectoryEdit => "Directory tag edits",
            tag_editor::TagEditorSource::TagSearch => "Tag search edits",
            tag_editor::TagEditorSource::HealthModal => "Health tag edits",
        };
        let _ = self.witch.start_transaction(label);

        // NOTE: bulk_from_audio_files reads tags from disk for all files
        let editor = tag_editor::UnifiedTagEditorState::bulk_from_audio_files(
            audio_files,
            source,
            group_context,
            &self.witch,
        );

        // Drain any keypresses that accumulated during loading
        drain_input_buffer();

        self.view = ActiveView::UnifiedTagEditor(editor);
    }

    /// Open the unified tag editor for a directory path.
    ///
    /// The input is an absolute filesystem path. We convert to relative for DB queries.
    pub(super) fn open_unified_tag_editor_for_directory(&mut self, directory: &std::path::Path) {
        // Convert absolute path to relative for DB query
        let rel_dir = match self.resolver.to_relative(directory) {
            Some(p) => p,
            None => {
                self.status_message =
                    Some(format!("Directory not in corpus: {}", directory.display()));
                return;
            }
        };

        let (audio_files, _) = self
            .witch
            .query(crate::db::domain::GetTagEditorFiles {
                rel_path: rel_dir,
                mode: crate::db::domain::TagEditorLoadMode::Directory,
            });

        if audio_files.is_empty() {
            self.status_message =
                Some(format!("No indexed files found in {}", directory.display()));
            return;
        }

        // Start transaction for directory edits
        let _ = self.witch.start_transaction("Directory tag edits");

        // Use directory_aggregated for aggregated tag view across all files
        // NOTE: This is slow - reads tags from disk for all files
        let editor = tag_editor::UnifiedTagEditorState::directory_aggregated(audio_files, &self.witch);

        // Drain any keypresses that accumulated during the slow loading
        drain_input_buffer();

        self.view = ActiveView::UnifiedTagEditor(editor);
    }

    /// Start unified tag editor for a single audio file from tag search results
    pub(super) fn start_unified_tag_editor_for_audio_file(&mut self, audio_file: AudioFile) {
        self.open_unified_tag_editor_single(
            audio_file,
            tag_editor::TagEditorSource::TagSearch,
            None,
        );
    }

    /// Open an embedded tag editor from a health modal.
    ///
    /// Unlike standalone launch, this does NOT start a transaction — the parent
    /// health modal's transaction is already active. Changes are collected locally
    /// and staged at the parent's decision index when the user saves.
    pub(super) fn open_embedded_tag_editor(
        &mut self,
        mode: tag_editor::TagEditorMode,
        audio_files: Vec<AudioFile>,
        decision_key: crate::meta::decisions::DecisionKey,
        decision_label: String,
    ) {
        let tag_fields = tag_editor::mutations::load_tag_fields_batch(&audio_files, &self.witch);
        let editor = tag_editor::UnifiedTagEditorState::new(
            mode,
            audio_files,
            tag_editor::TagEditorSource::HealthModal,
            None,
            tag_fields,
        )
        .with_embedded_mode(decision_key, decision_label);

        drain_input_buffer();

        self.push_and_switch(SuspendTarget::EmbeddedTagEditor(Box::new(editor)));
    }

    /// Start unified tag editor for aggregated bulk editing from tag search results
    pub(super) fn start_unified_tag_editor_for_audio_files(&mut self, audio_files: Vec<AudioFile>) {
        // Start transaction
        let _ = self.witch.start_transaction("Tag search bulk edit");

        // Use aggregated mode - all files edited as one unit
        // NOTE: aggregated_bulk reads tags from disk for all files
        let editor = tag_editor::UnifiedTagEditorState::aggregated_bulk(
            audio_files,
            tag_editor::TagEditorSource::TagSearch,
            &self.witch,
        );

        // Drain any keypresses that accumulated during loading
        drain_input_buffer();

        self.view = ActiveView::UnifiedTagEditor(editor);
    }
}
