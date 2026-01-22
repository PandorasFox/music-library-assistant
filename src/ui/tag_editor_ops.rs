//! Tag Editor Operations
//!
//! Functions for launching and navigating the unified tag editor in various
//! contexts (single file, bulk edit, directory edit, tag search results).

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
        let db = self.db();
        let resolver = paths::get_resolver();

        // Convert absolute path to relative for DB queries (corpus browser uses corpus paths)
        let rel_path = match resolver.to_relative_corpus(path) {
            Some(p) => p,
            None => {
                self.abort_to_insights(format!(
                    "Path not in corpus: {}",
                    path.display()
                ));
                return;
            }
        };

        // Load tracks from database using relative path
        let (tracks, selected_idx) = if recursive {
            // Get all tracks in directory and subdirectories (no fingerprint filter)
            match db.get_tracks_for_tag_editing(&rel_path) {
                Ok(t) => (t, 0usize),
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
            // Get all tracks in the same directory for cycling with tab/shift-tab
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

            // Load all tracks from parent directory (non-recursive, just this folder)
            let dir_tracks = match db.get_tracks_for_tag_editing(rel_parent) {
                Ok(t) => t,
                Err(e) => {
                    self.abort_to_insights(format!(
                        "Query error for directory '{}': {}",
                        path.display(),
                        e
                    ));
                    return;
                }
            };

            // Filter to only tracks directly in this directory (not subdirectories)
            let rel_path_str = rel_path.to_string_lossy().to_string();
            let rel_parent_str = rel_parent.to_string_lossy().to_string();
            let tracks_in_dir: Vec<_> = dir_tracks
                .into_iter()
                .filter(|t| {
                    // Check if track is directly in rel_parent (no additional path separators)
                    if let Some(suffix) = t.path.strip_prefix(&rel_parent_str) {
                        let suffix = suffix.trim_start_matches(std::path::MAIN_SEPARATOR);
                        !suffix.contains(std::path::MAIN_SEPARATOR)
                    } else {
                        false
                    }
                })
                .collect();

            // Find the index of the selected track (comparing relative paths)
            let selected_idx = tracks_in_dir
                .iter()
                .position(|t| t.path == rel_path_str)
                .unwrap_or(0);

            if tracks_in_dir.is_empty() {
                // Fallback: try to get just the single track
                match db.get_track_by_path(&rel_path_str) {
                    Ok(Some(track)) => (vec![track], 0),
                    Ok(None) => {
                        self.abort_to_insights(format!(
                            "Track not in index: {}",
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
                (tracks_in_dir, selected_idx)
            }
        };

        if tracks.is_empty() {
            self.abort_to_insights(format!(
                "No indexed tracks at: {}",
                path.display()
            ));
            return;
        }

        // Use the unified tag editor for single-file editing
        self.tree_browser = None;
        if tracks.len() == 1 {
            // Single track - use single file mode
            self.open_unified_tag_editor_single(
                tracks.into_iter().next().unwrap(),
                tag_editor::TagEditorSource::CorpusBrowser,
                None,
            );
        } else {
            // Multiple tracks in same directory - use bulk mode with CorpusBrowser source
            // This allows cycling through sibling files with tab/shift-tab
            self.open_unified_tag_editor_bulk(
                tracks,
                tag_editor::TagEditorSource::CorpusBrowser,
                None,
            );
            // Position on the selected track
            if let Some(editor) = self.unified_tag_editor.as_mut() {
                editor.current_item_idx = selected_idx;
            }
        }
        self.status_message = Some(format!("Editing tags for {}", path.display()));
    }

    /// Open the unified tag editor with a single track
    pub(super) fn open_unified_tag_editor_single(
        &mut self,
        track: crate::corpus::db::Track,
        source: tag_editor::TagEditorSource,
        group_context: Option<tag_editor::GroupContext>,
    ) {
        // Start transaction
        if let Some(the_witch) = self.witch.as_mut() {
            let label = match source {
                tag_editor::TagEditorSource::CorpusBrowser => "Tag edits",
                tag_editor::TagEditorSource::DirectoryEdit => "Directory tag edits",
                tag_editor::TagEditorSource::DuplicateResolution => "Duplicate resolution",
                tag_editor::TagEditorSource::DeployConflict => "Deploy conflict resolution",
                tag_editor::TagEditorSource::TagSearch => "Tag search edits",
            };
            let _ = the_witch.start_transaction(label);
        }

        self.unified_tag_editor = Some(tag_editor::UnifiedTagEditorState::single_file(
            track,
            source,
            group_context,
        ));
        self.mode = UiMode::UnifiedTagEditor;
    }

    /// Open the unified tag editor with multiple tracks
    pub(super) fn open_unified_tag_editor_bulk(
        &mut self,
        tracks: Vec<crate::corpus::db::Track>,
        source: tag_editor::TagEditorSource,
        group_context: Option<tag_editor::GroupContext>,
    ) {
        // Start transaction
        if let Some(the_witch) = self.witch.as_mut() {
            let label = match source {
                tag_editor::TagEditorSource::CorpusBrowser => "Bulk tag edits",
                tag_editor::TagEditorSource::DirectoryEdit => "Directory tag edits",
                tag_editor::TagEditorSource::DuplicateResolution => "Duplicate resolution",
                tag_editor::TagEditorSource::DeployConflict => "Deploy conflict resolution",
                tag_editor::TagEditorSource::TagSearch => "Tag search edits",
            };
            let _ = the_witch.start_transaction(label);
        }

        self.unified_tag_editor = Some(tag_editor::UnifiedTagEditorState::bulk_from_tracks(
            tracks,
            source,
            group_context,
        ));
        self.mode = UiMode::UnifiedTagEditor;
    }

    /// Open the unified tag editor for a directory path.
    ///
    /// The input is an absolute filesystem path. We convert to relative for DB queries.
    pub(super) fn open_unified_tag_editor_for_directory(&mut self, directory: &std::path::Path) {
        // Query database for tracks in this directory
        let db = self.db();
        let resolver = paths::get_resolver();

        // Convert absolute path to relative for DB query
        let rel_dir = match resolver.to_relative_corpus(directory) {
            Some(p) => p,
            None => {
                self.status_message = Some(format!(
                    "Directory not in corpus: {}",
                    directory.display()
                ));
                return;
            }
        };

        let tracks = match db.get_tracks_for_tag_editing(&rel_dir) {
            Ok(tracks) => tracks,
            Err(e) => {
                self.status_message = Some(format!("Failed to query tracks: {}", e));
                return;
            }
        };

        if tracks.is_empty() {
            self.status_message = Some(format!(
                "No indexed tracks found in {}",
                directory.display()
            ));
            return;
        }

        // Find sibling directories (other directories at the same level)
        let sibling_directories = if let Some(parent) = directory.parent() {
            std::fs::read_dir(parent)
                .ok()
                .map(|entries| {
                    let mut dirs: Vec<std::path::PathBuf> = entries
                        .filter_map(|e| e.ok())
                        .filter(|e| e.path().is_dir())
                        .map(|e| e.path())
                        .collect();
                    dirs.sort();
                    dirs
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        self.tree_browser = None;

        // Start transaction for directory edits
        if let Some(the_witch) = self.witch.as_mut() {
            let _ = the_witch.start_transaction("Directory tag edits");
        }

        // Use directory_aggregated for aggregated tag view across all files
        let mut editor = tag_editor::UnifiedTagEditorState::directory_aggregated(tracks, None);
        editor.set_sibling_directories(directory.to_path_buf(), sibling_directories);

        self.unified_tag_editor = Some(editor);
        self.mode = UiMode::UnifiedTagEditor;
    }

    /// Navigate to the next sibling in the tag editor.
    /// For DirectoryEdit mode: next sibling directory.
    /// For bulk edit mode: next track.
    pub(super) fn navigate_to_next_sibling(&mut self) {
        let is_directory_edit = self.unified_tag_editor
            .as_ref()
            .map(|e| e.is_directory_edit())
            .unwrap_or(false);

        if is_directory_edit {
            // Navigate to next sibling directory
            if let Some(ref editor) = self.unified_tag_editor {
                let next_idx = editor.current_sibling_idx + 1;
                if next_idx < editor.sibling_directories.len() {
                    let next_dir = editor.sibling_directories[next_idx].clone();
                    // Re-open the tag editor for the new directory
                    self.open_unified_tag_editor_for_directory(&next_dir);
                }
            }
        } else {
            // Navigate to next track (same as NextItem)
            if let Some(ref mut editor) = self.unified_tag_editor {
                if editor.current_item_idx < editor.total_items.saturating_sub(1) {
                    editor.current_item_idx += 1;
                    editor.reset_field_state();
                }
            }
        }
    }

    /// Navigate to the previous sibling in the tag editor.
    /// For DirectoryEdit mode: previous sibling directory.
    /// For bulk edit mode: previous track.
    pub(super) fn navigate_to_prev_sibling(&mut self) {
        let is_directory_edit = self.unified_tag_editor
            .as_ref()
            .map(|e| e.is_directory_edit())
            .unwrap_or(false);

        if is_directory_edit {
            // Navigate to previous sibling directory
            if let Some(ref editor) = self.unified_tag_editor {
                if editor.current_sibling_idx > 0 {
                    let prev_idx = editor.current_sibling_idx - 1;
                    let prev_dir = editor.sibling_directories[prev_idx].clone();
                    // Re-open the tag editor for the new directory
                    self.open_unified_tag_editor_for_directory(&prev_dir);
                }
            }
        } else {
            // Navigate to previous track (same as PrevItem)
            if let Some(ref mut editor) = self.unified_tag_editor {
                if editor.current_item_idx > 0 {
                    editor.current_item_idx -= 1;
                    editor.reset_field_state();
                }
            }
        }
    }

    /// Start unified tag editor for a single track from tag search results
    pub(super) fn start_unified_tag_editor_for_track(&mut self, track: crate::corpus::db::Track) {
        self.open_unified_tag_editor_single(
            track,
            tag_editor::TagEditorSource::TagSearch,
            None,
        );
    }

    /// Start unified tag editor for aggregated bulk editing from tag search results
    pub(super) fn start_unified_tag_editor_for_tracks(&mut self, tracks: Vec<crate::corpus::db::Track>) {
        // Start transaction
        if let Some(the_witch) = self.witch.as_mut() {
            let _ = the_witch.start_transaction("Tag search bulk edit");
        }

        // Use aggregated mode - all tracks edited as one unit
        self.unified_tag_editor = Some(tag_editor::UnifiedTagEditorState::aggregated_bulk(
            tracks,
            tag_editor::TagEditorSource::TagSearch,
        ));
        self.mode = UiMode::UnifiedTagEditor;
    }
}
