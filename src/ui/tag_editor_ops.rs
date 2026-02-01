//! Tag Editor Operations
//!
//! Functions for launching and navigating the unified tag editor in various
//! contexts (single file, bulk edit, directory edit, tag search results).

use crate::corpus::db::types::AudioFile;
use crate::corpus::paths;
use crate::ui::tag_editor;
use crate::ui::types::UiMode;
use super::App;

impl App {
    /// Start tag editor for a file path (from tree browser or external trigger).
    ///
    /// The input path is an absolute filesystem path. We convert to relative
    /// for database queries since the DB stores paths relative to corpus_root.
    pub(super) fn start_tag_editor_for_path(&mut self, path: &std::path::Path, recursive: bool) {
        let read_db = self.read_db();
        let resolver = paths::get_resolver();

        // Convert absolute path to relative for DB queries (corpus browser uses corpus paths)
        let rel_path = match resolver.to_relative(path) {
            Some(p) => p,
            None => {
                self.abort_to_insights(format!(
                    "Path not in corpus: {}",
                    path.display()
                ));
                return;
            }
        };

        // Load audio files from database using relative path
        let (audio_files, selected_idx) = if recursive {
            // Get all audio files in directory and subdirectories (no fingerprint filter)
            match read_db.get_audio_files_for_tag_editing(&rel_path) {
                Ok(files) => (files, 0usize),
                Err(e) => {
                    self.abort_to_insights(format!(
                        "Query error for path '{}': {}",
                        path.display(),
                        e
                    ));
                    return;
                }
            }
        } else {
            // Get all audio files in the same directory for cycling with tab/shift-tab
            let rel_parent = match rel_path.parent() {
                Some(p) => p,
                None => {
                    self.abort_to_insights(format!(
                        "Cannot determine parent directory: {}",
                        path.display()
                    ));
                    return;
                }
            };

            // Load all audio files from parent directory (non-recursive, just this folder)
            let dir_files = match read_db.get_audio_files_for_tag_editing(rel_parent) {
                Ok(files) => files,
                Err(e) => {
                    self.abort_to_insights(format!(
                        "Query error for directory '{}': {}",
                        path.display(),
                        e
                    ));
                    return;
                }
            };

            // Filter to only files directly in this directory (not subdirectories)
            let rel_path_str = rel_path.to_string_lossy().to_string();
            let rel_parent_str = rel_parent.to_string_lossy().to_string();
            let files_in_dir: Vec<_> = dir_files
                .into_iter()
                .filter(|f| {
                    // Check if file is directly in rel_parent (no additional path separators)
                    if let Some(suffix) = f.path().strip_prefix(&rel_parent_str) {
                        let suffix = suffix.trim_start_matches(std::path::MAIN_SEPARATOR);
                        !suffix.contains(std::path::MAIN_SEPARATOR)
                    } else {
                        false
                    }
                })
                .collect();

            // Find the index of the selected file (comparing relative paths)
            let selected_idx = files_in_dir
                .iter()
                .position(|f| f.path() == rel_path_str)
                .unwrap_or(0);

            if files_in_dir.is_empty() {
                // Fallback: try to get just the single audio file
                match read_db.get_audio_file_by_path(&rel_path_str) {
                    Ok(Some(audio_file)) => (vec![audio_file], 0),
                    Ok(None) => {
                        self.abort_to_insights(format!(
                            "File not in index: {}",
                            path.display()
                        ));
                        return;
                    }
                    Err(e) => {
                        self.abort_to_insights(format!(
                            "Query error for '{}': {}",
                            path.display(),
                            e
                        ));
                        return;
                    }
                }
            } else {
                (files_in_dir, selected_idx)
            }
        };

        if audio_files.is_empty() {
            self.abort_to_insights(format!(
                "No indexed files at: {}",
                path.display()
            ));
            return;
        }

        // Use the unified tag editor for single-file editing
        self.tree_browser = None;
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
            if let Some(editor) = self.unified_tag_editor.as_mut() {
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
        if let Some(the_witch) = self.witch.as_mut() {
            let label = match source {
                tag_editor::TagEditorSource::CorpusBrowser => "Tag edits",
                tag_editor::TagEditorSource::DirectoryEdit => "Directory tag edits",
                tag_editor::TagEditorSource::TagSearch => "Tag search edits",
            };
            let _ = the_witch.start_transaction(label);
        }

        self.unified_tag_editor = Some(tag_editor::UnifiedTagEditorState::single_file(
            audio_file,
            source,
            group_context,
        ));
        self.mode = UiMode::UnifiedTagEditor;
    }

    /// Open the unified tag editor with multiple audio files
    pub(super) fn open_unified_tag_editor_bulk(
        &mut self,
        audio_files: Vec<AudioFile>,
        source: tag_editor::TagEditorSource,
        group_context: Option<tag_editor::GroupContext>,
    ) {
        // Start transaction
        if let Some(the_witch) = self.witch.as_mut() {
            let label = match source {
                tag_editor::TagEditorSource::CorpusBrowser => "Bulk tag edits",
                tag_editor::TagEditorSource::DirectoryEdit => "Directory tag edits",
                tag_editor::TagEditorSource::TagSearch => "Tag search edits",
            };
            let _ = the_witch.start_transaction(label);
        }

        self.unified_tag_editor = Some(tag_editor::UnifiedTagEditorState::bulk_from_audio_files(
            audio_files,
            source,
            group_context,
        ));
        self.mode = UiMode::UnifiedTagEditor;
    }

    /// Open the unified tag editor for a directory path.
    ///
    /// The input is an absolute filesystem path. We convert to relative for DB queries.
    pub(super) fn open_unified_tag_editor_for_directory(&mut self, directory: &std::path::Path) {
        // Query database for audio files in this directory
        let read_db = self.read_db();
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

        let audio_files = match read_db.get_audio_files_for_tag_editing(&rel_dir) {
            Ok(files) => files,
            Err(e) => {
                self.status_message = Some(format!("Failed to query audio files: {}", e));
                return;
            }
        };

        if audio_files.is_empty() {
            self.status_message = Some(format!(
                "No indexed files found in {}",
                directory.display()
            ));
            return;
        }

        self.tree_browser = None;

        // Start transaction for directory edits
        if let Some(the_witch) = self.witch.as_mut() {
            let _ = the_witch.start_transaction("Directory tag edits");
        }

        // Use directory_aggregated for aggregated tag view across all files
        let editor = tag_editor::UnifiedTagEditorState::directory_aggregated(audio_files);

        self.unified_tag_editor = Some(editor);
        self.mode = UiMode::UnifiedTagEditor;
    }

    /// Start unified tag editor for a single audio file from tag search results
    pub(super) fn start_unified_tag_editor_for_audio_file(&mut self, audio_file: AudioFile) {
        self.open_unified_tag_editor_single(
            audio_file,
            tag_editor::TagEditorSource::TagSearch,
            None,
        );
    }

    /// Start unified tag editor for aggregated bulk editing from tag search results
    pub(super) fn start_unified_tag_editor_for_audio_files(&mut self, audio_files: Vec<AudioFile>) {
        // Start transaction
        if let Some(the_witch) = self.witch.as_mut() {
            let _ = the_witch.start_transaction("Tag search bulk edit");
        }

        // Use aggregated mode - all files edited as one unit
        self.unified_tag_editor = Some(tag_editor::UnifiedTagEditorState::aggregated_bulk(
            audio_files,
            tag_editor::TagEditorSource::TagSearch,
        ));
        self.mode = UiMode::UnifiedTagEditor;
    }
}
