//! State management for the directory browser.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use super::types::{DirBrowserAction, DirBrowserConfig, DirEntry};

/// Audio file extensions we count
const AUDIO_EXTENSIONS: &[&str] = &["mp3", "flac", "m4a", "ogg", "opus", "wav", "aiff", "aac"];

/// Directory browser state.
#[derive(Debug)]
pub struct DirBrowserState {
    /// Configuration for this browser instance
    config: DirBrowserConfig,
    /// Flattened visible tree entries
    entries: Vec<DirEntry>,
    /// Current cursor position in entries
    cursor_idx: usize,
    /// Paths that have been selected (space-toggled)
    selected_paths: HashSet<PathBuf>,
    /// Root directory being browsed
    root_path: PathBuf,
    /// Scroll offset for tall trees
    scroll_offset: usize,
    /// Visible height (set during render)
    visible_height: usize,
}

impl DirBrowserState {
    /// Create a new browser rooted at the given path.
    pub fn new(root: PathBuf, config: DirBrowserConfig) -> Self {
        let mut state = Self {
            config,
            entries: Vec::new(),
            cursor_idx: 0,
            selected_paths: HashSet::new(),
            root_path: root,
            scroll_offset: 0,
            visible_height: 20,
        };
        state.rebuild_entries();
        state
    }

    /// Get the browser configuration.
    pub fn config(&self) -> &DirBrowserConfig {
        &self.config
    }

    /// Get all visible entries.
    pub fn entries(&self) -> &[DirEntry] {
        &self.entries
    }

    /// Get current cursor index.
    pub fn cursor_idx(&self) -> usize {
        self.cursor_idx
    }

    /// Get selected paths.
    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected_paths.iter().cloned().collect()
    }

    /// Check if a path is selected.
    pub fn is_selected(&self, path: &PathBuf) -> bool {
        self.selected_paths.contains(path)
    }

    /// Get scroll offset.
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Set visible height (called during render).
    pub fn set_visible_height(&mut self, height: usize) {
        self.visible_height = height;
    }

    /// Get current entry under cursor.
    pub fn current_entry(&self) -> Option<&DirEntry> {
        self.entries.get(self.cursor_idx)
    }

    // === Navigation ===

    /// Move cursor up.
    pub fn move_up(&mut self) {
        if self.cursor_idx > 0 {
            self.cursor_idx -= 1;
            self.adjust_scroll();
        }
    }

    /// Move cursor down.
    pub fn move_down(&mut self) {
        if self.cursor_idx + 1 < self.entries.len() {
            self.cursor_idx += 1;
            self.adjust_scroll();
        }
    }

    /// Expand current directory (Right arrow).
    pub fn expand_current(&mut self) {
        if let Some(entry) = self.entries.get(self.cursor_idx).cloned() {
            if entry.has_children && !entry.is_expanded {
                // Mark as expanded
                if let Some(e) = self.entries.get_mut(self.cursor_idx) {
                    e.is_expanded = true;
                }
                // Insert children after current entry
                let children = self.load_children(&entry.path, entry.depth + 1);
                let insert_pos = self.cursor_idx + 1;
                for (i, child) in children.into_iter().enumerate() {
                    self.entries.insert(insert_pos + i, child);
                }
            }
        }
    }

    /// Collapse current directory, or jump to parent if already collapsed (Left arrow).
    pub fn collapse_or_parent(&mut self) {
        if let Some(entry) = self.entries.get(self.cursor_idx).cloned() {
            if entry.is_expanded {
                // Collapse: remove all descendants
                self.collapse_entry(self.cursor_idx);
            } else if entry.depth > 0 {
                // Jump to parent
                self.jump_to_parent();
            }
        }
    }

    /// Collapse entry at given index, removing all its descendants.
    fn collapse_entry(&mut self, idx: usize) {
        if let Some(entry) = self.entries.get_mut(idx) {
            entry.is_expanded = false;
            let depth = entry.depth;

            // Remove all entries after this one that have greater depth
            let mut remove_count = 0;
            for i in (idx + 1)..self.entries.len() {
                if self.entries[i].depth > depth {
                    remove_count += 1;
                } else {
                    break;
                }
            }

            for _ in 0..remove_count {
                self.entries.remove(idx + 1);
            }
        }
    }

    /// Jump cursor to parent directory.
    fn jump_to_parent(&mut self) {
        if let Some(current) = self.entries.get(self.cursor_idx) {
            let current_depth = current.depth;
            if current_depth > 0 {
                // Search backwards for entry with depth = current_depth - 1
                for i in (0..self.cursor_idx).rev() {
                    if self.entries[i].depth == current_depth - 1 {
                        self.cursor_idx = i;
                        self.adjust_scroll();
                        break;
                    }
                }
            }
        }
    }

    // === Selection ===

    /// Check if path is an ancestor of any selected path.
    /// Used to grey out and prevent selection of parent directories.
    pub fn is_ancestor_of_selected(&self, path: &PathBuf) -> bool {
        self.selected_paths.iter().any(|selected| {
            selected.starts_with(path) && selected != path
        })
    }

    /// Check if path is selectable (not an ancestor of a selection).
    pub fn is_selectable(&self, path: &PathBuf) -> bool {
        !self.is_ancestor_of_selected(path)
    }

    /// Toggle selection on current entry (Space).
    /// Skips if the entry is an ancestor of an existing selection.
    pub fn toggle_selection(&mut self) {
        if let Some(entry) = self.entries.get(self.cursor_idx) {
            let path = entry.path.clone();

            // Can't toggle ancestors of selected paths
            if self.is_ancestor_of_selected(&path) {
                return;
            }

            if self.selected_paths.contains(&path) {
                self.selected_paths.remove(&path);
            } else {
                if !self.config.multi_select {
                    self.selected_paths.clear();
                }
                self.selected_paths.insert(path);
            }
        }
    }

    /// Select all visible entries (A).
    /// Skips entries that are ancestors of existing selections.
    pub fn select_all(&mut self) {
        if self.config.multi_select {
            for entry in &self.entries {
                if self.is_selectable(&entry.path) {
                    self.selected_paths.insert(entry.path.clone());
                }
            }
        }
    }

    /// Deselect all entries (N).
    pub fn deselect_all(&mut self) {
        self.selected_paths.clear();
    }

    // === Actions ===

    /// Get action for Enter key.
    pub fn get_proceed_action(&self) -> DirBrowserAction {
        if self.selected_paths.is_empty() {
            // No explicit selection - use current cursor position
            if let Some(entry) = self.current_entry() {
                DirBrowserAction::Proceed(vec![entry.path.clone()])
            } else {
                DirBrowserAction::Cancel
            }
        } else {
            DirBrowserAction::Proceed(self.selected_paths())
        }
    }

    // === Internal ===

    /// Rebuild entries list from scratch (initial load).
    fn rebuild_entries(&mut self) {
        self.entries.clear();
        self.cursor_idx = 0;
        self.scroll_offset = 0;

        // Load top-level directories
        let children = self.load_children(&self.root_path, 0);
        self.entries = children;
    }

    /// Load immediate children of a directory.
    fn load_children(&self, parent: &PathBuf, depth: usize) -> Vec<DirEntry> {
        let mut children = Vec::new();

        if let Ok(read_dir) = fs::read_dir(parent) {
            let mut dirs: Vec<_> = read_dir
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
                .filter(|e| {
                    // Skip hidden directories
                    !e.file_name().to_string_lossy().starts_with('.')
                })
                .collect();

            // Sort alphabetically
            dirs.sort_by_key(|a| a.file_name());

            for dir_entry in dirs {
                let path = dir_entry.path();
                let name = dir_entry.file_name().to_string_lossy().to_string();
                let has_children = self.has_subdirectories(&path);
                let item_count = self.count_audio_files(&path);

                children.push(DirEntry {
                    path,
                    name,
                    depth,
                    is_expanded: false,
                    has_children,
                    item_count,
                });
            }
        }

        children
    }

    /// Check if directory has any subdirectories.
    fn has_subdirectories(&self, path: &PathBuf) -> bool {
        if let Ok(read_dir) = fs::read_dir(path) {
            for entry in read_dir.filter_map(|e| e.ok()) {
                if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                    let name = entry.file_name();
                    if !name.to_string_lossy().starts_with('.') {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Count audio files in directory (non-recursive).
    fn count_audio_files(&self, path: &PathBuf) -> usize {
        if let Ok(read_dir) = fs::read_dir(path) {
            read_dir
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
                .filter(|e| {
                    if let Some(ext) = e.path().extension() {
                        AUDIO_EXTENSIONS.contains(&ext.to_string_lossy().to_lowercase().as_str())
                    } else {
                        false
                    }
                })
                .count()
        } else {
            0
        }
    }

    /// Adjust scroll offset to keep cursor visible.
    fn adjust_scroll(&mut self) {
        if self.visible_height == 0 {
            return;
        }

        // Ensure cursor is within visible range
        if self.cursor_idx < self.scroll_offset {
            self.scroll_offset = self.cursor_idx;
        } else if self.cursor_idx >= self.scroll_offset + self.visible_height {
            self.scroll_offset = self.cursor_idx - self.visible_height + 1;
        }
    }
}
