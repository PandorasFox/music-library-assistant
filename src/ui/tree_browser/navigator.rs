//! Tree Navigator
//!
//! Core tree navigation state shared by all browser variants.
//! Handles entry management, cursor movement, expand/collapse, and scrolling.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::AUDIO_EXTENSIONS;

use super::entry::{DeployMarker, TreeEntry};

/// Filter configuration for entry loading.
#[derive(Debug, Clone, Copy, Default)]
pub struct EntryFilter {
    /// Include audio files in tree (false = directories only)
    pub include_files: bool,
    /// Include hidden files/directories (starting with '.')
    pub include_hidden: bool,
}


impl EntryFilter {
    /// Filter that shows only directories.
    pub fn directories_only() -> Self {
        Self {
            include_files: false,
            include_hidden: false,
        }
    }

    /// Filter that shows directories and audio files.
    pub fn with_files() -> Self {
        Self {
            include_files: true,
            include_hidden: false,
        }
    }
}

/// Core tree navigation state.
///
/// Manages the flattened tree structure, cursor position, and scrolling.
/// Variant-specific behavior is handled by the variant types.
#[derive(Debug)]
pub struct TreeNavigator {
    /// Flattened visible tree entries
    entries: Vec<TreeEntry>,
    /// Current cursor position in entries
    cursor_idx: usize,
    /// Root directory being browsed
    root_path: PathBuf,
    /// Scroll offset for viewport
    scroll_offset: usize,
    /// Visible height (set during render)
    visible_height: usize,
    /// Entry filter for this navigator
    filter: EntryFilter,
    /// Whether root itself is shown as an entry (vs just its children)
    show_root: bool,
    /// Active path filter - when Some, only paths in this set are shown.
    /// Includes both files that match and their ancestor directories.
    active_path_filter: Option<HashSet<PathBuf>>,
    /// Absolute paths of configured deployment source directories.
    /// Directories equal to or under these paths are flagged as configured for deploy.
    deploy_source_paths: Vec<PathBuf>,
    /// When true, prepend a synthetic "[+ new directory]" entry as the first
    /// child of each expanded directory.
    pub show_new_dir_entry: bool,
}

impl TreeNavigator {
    /// Create a new navigator rooted at the given path.
    ///
    /// - `show_root`: If true, root directory is shown as first entry (corpus browser style).
    ///   If false, only root's children are shown (directory selector style).
    /// - `deploy_source_paths`: Absolute paths of configured deployment source directories.
    pub fn new(root_path: PathBuf, filter: EntryFilter, show_root: bool, deploy_source_paths: Vec<PathBuf>) -> Self {
        let mut nav = Self {
            entries: Vec::new(),
            cursor_idx: 0,
            root_path,
            scroll_offset: 0,
            visible_height: 20,
            filter,
            show_root,
            active_path_filter: None,
            deploy_source_paths,
            show_new_dir_entry: false,
        };
        nav.load_initial();
        nav
    }

    /// Set an active path filter. Only paths in this set will be shown.
    /// The set should include both target files and their ancestor directories.
    pub fn set_path_filter(&mut self, paths: HashSet<PathBuf>) {
        self.active_path_filter = Some(paths);
        self.load_initial();
    }

    /// Clear the active path filter, restoring full tree view.
    pub fn clear_path_filter(&mut self) {
        self.active_path_filter = None;
        self.load_initial();
    }

    /// Check if a path filter is currently active.
    pub fn has_path_filter(&self) -> bool {
        self.active_path_filter.is_some()
    }

    /// Get count of matching paths (files only, not directories).
    pub fn filtered_file_count(&self) -> Option<usize> {
        self.active_path_filter.as_ref().map(|paths| {
            paths.iter().filter(|p| p.is_file()).count()
        })
    }

    /// Load initial entries based on configuration.
    fn load_initial(&mut self) {
        self.entries.clear();
        self.cursor_idx = 0;
        self.scroll_offset = 0;

        if self.show_root {
            // Corpus browser style: show root as first entry, expanded
            let root_name = self
                .root_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| self.root_path.to_string_lossy().to_string());

            let has_children = self.path_has_children(&self.root_path) || self.show_new_dir_entry;
            let item_count = self.count_audio_files(&self.root_path);

            let root_marker = self.deploy_marker_for(&self.root_path);
            let mut root_entry = TreeEntry::directory(
                self.root_path.clone(),
                root_name,
                0,
                has_children,
                item_count,
            );
            root_entry.deploy_marker = root_marker;
            root_entry.is_expanded = true;
            self.entries.push(root_entry);

            // Load children of root
            self.load_children_at(0);
        } else {
            // Directory selector style: show only root's children
            let children = self.load_children_of(&self.root_path, 0);
            self.entries = children;
        }
    }

    // =========================================================================
    // Navigation
    // =========================================================================

    /// Move cursor up.
    pub fn move_up(&mut self) {
        if self.cursor_idx > 0 {
            self.cursor_idx -= 1;
            self.ensure_visible();
        }
    }

    /// Move cursor down.
    pub fn move_down(&mut self) {
        if self.cursor_idx + 1 < self.entries.len() {
            self.cursor_idx += 1;
            self.ensure_visible();
        }
    }

    /// Expand current directory if collapsed.
    pub fn expand_current(&mut self) {
        if let Some(entry) = self.entries.get(self.cursor_idx).cloned() {
            if entry.is_directory && entry.has_children && !entry.is_expanded {
                // Mark as expanded
                if let Some(e) = self.entries.get_mut(self.cursor_idx) {
                    e.is_expanded = true;
                }
                // Insert children after current entry
                self.load_children_at(self.cursor_idx);
            }
        }
    }

    /// Collapse current directory if expanded, otherwise jump to parent.
    pub fn collapse_or_parent(&mut self) {
        if let Some(entry) = self.entries.get(self.cursor_idx).cloned() {
            if entry.is_directory && entry.is_expanded {
                // Collapse: remove all descendants
                self.collapse_at(self.cursor_idx);
            } else if entry.depth > 0 || (!self.show_root && entry.depth == 0) {
                // Jump to parent
                self.jump_to_parent();
            }
        }
    }

    /// Collapse entry at given index, removing all its descendants.
    fn collapse_at(&mut self, idx: usize) {
        if let Some(entry) = self.entries.get_mut(idx) {
            entry.is_expanded = false;
            let depth = entry.depth;

            // Count entries to remove (all with greater depth until we hit same/lower)
            let mut remove_count = 0;
            for entry in self.entries.iter().skip(idx + 1) {
                if entry.depth > depth {
                    remove_count += 1;
                } else {
                    break;
                }
            }

            // Remove descendants
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
                    if self.entries[i].depth == current_depth - 1 && self.entries[i].is_directory {
                        self.cursor_idx = i;
                        self.ensure_visible();
                        break;
                    }
                }
            }
        }
    }

    /// Navigate to a specific path, expanding ancestors as needed.
    pub fn navigate_to_path(&mut self, target: &Path) {
        // Build path from root to target
        let mut ancestors: Vec<PathBuf> = Vec::new();
        let mut current = target.to_path_buf();

        while current != self.root_path && current.parent().is_some() {
            ancestors.push(current.clone());
            if let Some(parent) = current.parent() {
                current = parent.to_path_buf();
            } else {
                break;
            }
        }
        ancestors.reverse();

        // Expand each ancestor in order
        for ancestor in &ancestors {
            if let Some(idx) = self.entries.iter().position(|e| &e.path == ancestor) {
                if self.entries[idx].is_directory && !self.entries[idx].is_expanded {
                    self.entries[idx].is_expanded = true;
                    self.load_children_at(idx);
                }
            }
        }

        // Move cursor to target
        if let Some(idx) = self.entries.iter().position(|e| e.path == *target) {
            self.cursor_idx = idx;
            self.ensure_visible();
        }
    }

    // =========================================================================
    // Entry Loading
    // =========================================================================

    /// Load children of the entry at the given index.
    fn load_children_at(&mut self, parent_idx: usize) {
        let parent = &self.entries[parent_idx];
        if !parent.is_directory || !parent.is_expanded {
            return;
        }

        let parent_path = parent.path.clone();
        let child_depth = parent.depth + 1;

        let children = self.load_children_of(&parent_path, child_depth);

        // Insert children after parent
        let insert_pos = parent_idx + 1;
        for (i, child) in children.into_iter().enumerate() {
            self.entries.insert(insert_pos + i, child);
        }
    }

    /// Load immediate children of a directory.
    fn load_children_of(&self, parent: &Path, depth: usize) -> Vec<TreeEntry> {
        let mut dirs: Vec<TreeEntry> = Vec::new();
        let mut files: Vec<TreeEntry> = Vec::new();

        if let Ok(read_dir) = fs::read_dir(parent) {
            let mut fs_entries: Vec<_> = read_dir.flatten().collect();
            fs_entries.sort_by_key(|e| e.path());

            for entry in fs_entries {
                let path = entry.path();
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                // Skip hidden files/directories unless configured
                if !self.filter.include_hidden && name.starts_with('.') {
                    continue;
                }

                // Skip if path filter is active and this path is not in the allowed set
                if let Some(ref allowed) = self.active_path_filter {
                    if !allowed.contains(&path) {
                        continue;
                    }
                }

                if path.is_dir() {
                    let has_children = self.path_has_children(&path) || self.show_new_dir_entry;
                    let item_count = self.count_audio_files(&path);
                    let mut entry = TreeEntry::directory(path.clone(), name, depth, has_children, item_count);
                    entry.deploy_marker = self.deploy_marker_for(&path);
                    dirs.push(entry);
                } else if self.filter.include_files && self.is_audio_file(&path) {
                    files.push(TreeEntry::file(path, name, depth));
                }
            }
        }

        // Optionally prepend synthetic "[+ new directory]" entry
        let mut result = Vec::new();
        if self.show_new_dir_entry {
            result.push(TreeEntry::new_directory_prompt(parent.to_path_buf(), depth));
        }

        // Directories first, then files
        result.extend(dirs);
        result.extend(files);
        result
    }

    /// Check if a path has children (subdirs or, if filter allows, audio files).
    fn path_has_children(&self, path: &Path) -> bool {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let entry_path = entry.path();
                let name = entry_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                // Skip hidden
                if !self.filter.include_hidden && name.starts_with('.') {
                    continue;
                }

                if entry_path.is_dir() {
                    return true;
                }
                if self.filter.include_files && self.is_audio_file(&entry_path) {
                    return true;
                }
            }
        }
        false
    }

    /// Count audio files in a directory (non-recursive).
    fn count_audio_files(&self, path: &Path) -> usize {
        fs::read_dir(path)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| self.is_audio_file(&e.path()))
                    .count()
            })
            .unwrap_or(0)
    }

    /// Check if a path is an audio file.
    ///
    /// Excludes macOS resource fork files (`._*`) which appear on NFS/SMB mounts.
    fn is_audio_file(&self, path: &Path) -> bool {
        // Skip macOS resource fork (AppleDouble) files
        let is_resource_fork = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with("._"))
            .unwrap_or(false);

        if is_resource_fork {
            return false;
        }

        path.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| AUDIO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
                .unwrap_or(false)
    }

    // =========================================================================
    // Scrolling
    // =========================================================================

    /// Ensure cursor is visible in viewport.
    fn ensure_visible(&mut self) {
        if self.visible_height == 0 {
            return;
        }

        if self.cursor_idx < self.scroll_offset {
            self.scroll_offset = self.cursor_idx;
        } else if self.cursor_idx >= self.scroll_offset + self.visible_height {
            self.scroll_offset = self.cursor_idx - self.visible_height + 1;
        }
    }

    // =========================================================================
    // Accessors
    // =========================================================================

    /// Get all visible entries.
    pub fn entries(&self) -> &[TreeEntry] {
        &self.entries
    }

    /// Get current cursor index.
    pub fn cursor_idx(&self) -> usize {
        self.cursor_idx
    }

    /// Get entry under cursor.
    pub fn current_entry(&self) -> Option<&TreeEntry> {
        self.entries.get(self.cursor_idx)
    }

    /// Get scroll offset.
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Set visible height (called during render).
    pub fn set_visible_height(&mut self, height: usize) {
        self.visible_height = height;
    }

    /// Get root path.
    pub fn root_path(&self) -> &PathBuf {
        &self.root_path
    }

    /// Compute deploy marker for a directory path.
    fn deploy_marker_for(&self, path: &Path) -> DeployMarker {
        for src in &self.deploy_source_paths {
            if path == src.as_path() {
                return DeployMarker::SourceRoot;
            }
        }
        for src in &self.deploy_source_paths {
            if path.starts_with(src) {
                return DeployMarker::Inherited;
            }
        }
        DeployMarker::None
    }

}
