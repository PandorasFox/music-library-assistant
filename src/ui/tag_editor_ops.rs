//! Tag Editor Operations
//!
//! Functions for launching and navigating the unified tag editor in various
//! contexts (single file, bulk edit, directory edit, tag search results).

use crossterm::event;

use crate::corpus::db::types::AudioFile;
use crate::corpus::paths;
use crate::ui::tag_editor;
use crate::ui::suspended_views::SuspendTarget;
use crate::ui::ActiveView;
use super::App;

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
        let resolver = paths::get_resolver();

        // Convert absolute path to relative for DB queries (corpus browser uses corpus paths)
        let rel_path = match resolver.to_relative(path) {
            Some(p) => p,
            None => {
                self.abort_to_health(format!(
                    "Path not in corpus: {}",
                    path.display()
                ));
                return;
            }
        };

        // Load audio files from database using relative path
        let rel_path_owned = rel_path.to_path_buf();
        let (audio_files, selected_idx) = self.cache.query(move |db| {
            if recursive {
                // Get all audio files in directory and subdirectories
                let files = db.get_audio_files_for_tag_editing(&rel_path_owned)
                    .unwrap_or_default();
                (files, 0usize)
            } else {
                let rel_parent = match rel_path_owned.parent() {
                    Some(p) => p.to_path_buf(),
                    None => return (Vec::new(), 0usize),
                };

                // Load all audio files from parent directory
                let dir_files = db.get_audio_files_for_tag_editing(&rel_parent)
                    .unwrap_or_default();

                // Filter to only files directly in this directory (not subdirectories)
                let rel_path_str = rel_path_owned.to_string_lossy().to_string();
                let rel_parent_str = rel_parent.to_string_lossy().to_string();
                let files_in_dir: Vec<_> = dir_files
                    .into_iter()
                    .filter(|f| {
                        if let Some(suffix) = f.path().strip_prefix(&rel_parent_str) {
                            let suffix = suffix.trim_start_matches(std::path::MAIN_SEPARATOR);
                            !suffix.contains(std::path::MAIN_SEPARATOR)
                        } else {
                            false
                        }
                    })
                    .collect();

                let selected_idx = files_in_dir
                    .iter()
                    .position(|f| f.path() == rel_path_str)
                    .unwrap_or(0);

                if files_in_dir.is_empty() {
                    // Fallback: try to get just the single audio file
                    match db.get_audio_file_by_path(&rel_path_str) {
                        Ok(Some(audio_file)) => (vec![audio_file], 0),
                        _ => (Vec::new(), 0),
                    }
                } else {
                    (files_in_dir, selected_idx)
                }
            }
        }).recv();

        if audio_files.is_empty() {
            self.abort_to_health(format!(
                "No indexed files at: {}",
                path.display()
            ));
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
        let editor = tag_editor::UnifiedTagEditorState::single_file(
            audio_file,
            source,
            group_context,
        );

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
        );

        // Drain any keypresses that accumulated during loading
        drain_input_buffer();

        self.view = ActiveView::UnifiedTagEditor(editor);
    }

    /// Open the unified tag editor for a directory path.
    ///
    /// The input is an absolute filesystem path. We convert to relative for DB queries.
    pub(super) fn open_unified_tag_editor_for_directory(&mut self, directory: &std::path::Path) {
        let resolver = paths::get_resolver();

        // Convert absolute path to relative for DB query
        let rel_dir = match resolver.to_relative(directory) {
            Some(p) => p,
            None => {
                self.status_message = Some(format!(
                    "Directory not in corpus: {}",
                    directory.display()
                ));
                return;
            }
        };

        let audio_files = self.cache.query(move |db| {
            db.get_audio_files_for_tag_editing(&rel_dir).unwrap_or_default()
        }).recv();

        if audio_files.is_empty() {
            self.status_message = Some(format!(
                "No indexed files found in {}",
                directory.display()
            ));
            return;
        }

        // Start transaction for directory edits
        let _ = self.witch.start_transaction("Directory tag edits");

        // Use directory_aggregated for aggregated tag view across all files
        // NOTE: This is slow - reads tags from disk for all files
        let editor = tag_editor::UnifiedTagEditorState::directory_aggregated(audio_files);

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
        let editor = tag_editor::UnifiedTagEditorState::new(
            mode,
            audio_files,
            tag_editor::TagEditorSource::HealthModal,
            None,
        ).with_embedded_mode(decision_key, decision_label);

        drain_input_buffer();

        self.push_and_switch(SuspendTarget::EmbeddedTagEditor(editor));
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
        );

        // Drain any keypresses that accumulated during loading
        drain_input_buffer();

        self.view = ActiveView::UnifiedTagEditor(editor);
    }
}
