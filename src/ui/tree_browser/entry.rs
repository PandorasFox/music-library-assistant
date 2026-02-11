//! Tree Entry Type
//!
//! Unified entry type for the tree browser, supporting both files and directories.

use std::path::PathBuf;

/// A single entry in the flattened tree view.
///
/// Represents either a directory or an audio file. The tree is stored as a
/// flat vector with depth tracking for visual indentation.
#[derive(Debug, Clone)]
pub struct TreeEntry {
    /// Full path to this entry
    pub path: PathBuf,
    /// Display name (filename without path)
    pub name: String,
    /// Depth in tree (0 = root level, 1 = first child, etc.)
    pub depth: usize,
    /// Whether this is a directory (false = audio file)
    pub is_directory: bool,
    /// Whether children are currently visible (directories only)
    pub is_expanded: bool,
    /// Whether this entry has expandable children
    pub has_children: bool,
    /// Count of audio files in this directory (non-recursive, directories only)
    pub item_count: usize,
    /// Whether this directory is under a configured deployment source
    pub configured_for_deploy: bool,
}

impl TreeEntry {
    /// Create a new directory entry.
    pub fn directory(
        path: PathBuf,
        name: String,
        depth: usize,
        has_children: bool,
        item_count: usize,
    ) -> Self {
        Self {
            path,
            name,
            depth,
            is_directory: true,
            is_expanded: false,
            has_children,
            item_count,
            configured_for_deploy: false,
        }
    }

    /// Create a new file entry.
    pub fn file(path: PathBuf, name: String, depth: usize) -> Self {
        Self {
            path,
            name,
            depth,
            is_directory: false,
            is_expanded: false,
            has_children: false,
            item_count: 0,
            configured_for_deploy: false,
        }
    }
}
