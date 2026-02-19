//! Tree Entry Type
//!
//! Unified entry type for the tree browser, supporting both files and directories.

use std::path::PathBuf;

/// Deploy marker for a tree entry, distinguishing source roots from inherited dirs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployMarker {
    /// Not under any configured deployment source.
    None,
    /// Exact source directory root — C opens config panel here.
    SourceRoot,
    /// Under a source directory (inherited deployment config).
    Inherited,
}

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
    /// Deploy marker: None, SourceRoot, or Inherited
    pub deploy_marker: DeployMarker,
    /// Whether this is a synthetic UI-only entry (e.g., "[+ new directory]")
    pub is_synthetic: bool,
    /// Whether this entry should be visually dimmed (non-primary zone dirs)
    pub is_dimmed: bool,
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
            deploy_marker: DeployMarker::None,
            is_synthetic: false,
            is_dimmed: false,
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
            deploy_marker: DeployMarker::None,
            is_synthetic: false,
            is_dimmed: false,
        }
    }

    /// Create a synthetic "[+ new directory]" entry.
    pub fn new_directory_prompt(parent_path: PathBuf, depth: usize) -> Self {
        Self {
            path: parent_path,
            name: "[+ new directory]".to_string(),
            depth,
            is_directory: true,
            is_expanded: false,
            has_children: false,
            item_count: 0,
            deploy_marker: DeployMarker::None,
            is_synthetic: true,
            is_dimmed: false,
        }
    }
}
