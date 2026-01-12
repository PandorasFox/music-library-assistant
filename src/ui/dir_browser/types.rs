//! Types for the directory browser component.

use std::path::PathBuf;

/// A single entry in the flattened tree view.
#[derive(Debug, Clone)]
pub struct DirEntry {
    /// Full path to this directory
    pub path: PathBuf,
    /// Display name (directory basename)
    pub name: String,
    /// Indentation level (0 = root)
    pub depth: usize,
    /// Whether children are currently visible
    pub is_expanded: bool,
    /// Whether this directory has subdirectories
    pub has_children: bool,
    /// Number of audio files in this directory (non-recursive)
    pub item_count: usize,
}

/// Configuration for browser behavior.
#[derive(Debug, Clone)]
pub struct DirBrowserConfig {
    /// Header text displayed at top
    pub title: String,
    /// Allow multiple directory selections (default: true)
    pub multi_select: bool,
    /// Show item counts next to directories (default: true)
    pub show_item_counts: bool,
}

impl Default for DirBrowserConfig {
    fn default() -> Self {
        Self {
            title: "Select directories".to_string(),
            multi_select: true,
            show_item_counts: true,
        }
    }
}

impl DirBrowserConfig {
    /// Create config for sleuthing operation
    pub fn for_sleuthing() -> Self {
        Self {
            title: "Select directories for fingerprint deduplication".to_string(),
            multi_select: true,
            show_item_counts: true,
        }
    }
}

/// Actions returned from key handling.
/// The browser is generic - the caller interprets these actions.
#[derive(Debug, Clone)]
pub enum DirBrowserAction {
    /// No action needed
    None,
    /// User pressed Enter - return selected paths to caller
    Proceed(Vec<PathBuf>),
    /// User pressed Esc - cancel and return to previous mode
    Cancel,
}
