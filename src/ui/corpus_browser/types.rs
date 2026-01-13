//! Types for the corpus browser component.

use std::path::PathBuf;

/// A single entry in the flattened tree view (can be directory or file).
#[derive(Debug, Clone)]
pub struct BrowserEntry {
    /// Full path to this entry
    pub path: PathBuf,
    /// Display name (basename)
    pub name: String,
    /// Indentation level (0 = root)
    pub depth: usize,
    /// Whether this is a directory (true) or file (false)
    pub is_directory: bool,
    /// Whether children are currently visible (for directories)
    pub is_expanded: bool,
    /// Whether this directory has children (subdirs or files)
    pub has_children: bool,
    /// Number of audio files in this directory (non-recursive, for directories only)
    pub item_count: usize,
}

/// Metadata about an audio file for preview pane.
#[derive(Debug, Clone, Default)]
pub struct FileMetadata {
    /// Bitrate in kbps
    pub bitrate_kbps: Option<u32>,
    /// Duration in milliseconds
    pub duration_ms: Option<u64>,
    /// Sample rate in Hz
    pub sample_rate: Option<u32>,
    /// File size in bytes
    pub file_size: u64,
    /// File format/extension
    pub file_type: String,
    /// Tag key-value pairs
    pub tags: Vec<(String, String)>,
}

/// Configuration for corpus browser behavior.
#[derive(Debug, Clone)]
pub struct CorpusBrowserConfig {
    /// Header text displayed at top
    pub title: String,
    /// Show files in tree (not just directories)
    pub show_files: bool,
}

impl Default for CorpusBrowserConfig {
    fn default() -> Self {
        Self {
            title: "Corpus Browser".to_string(),
            show_files: true,
        }
    }
}

/// Actions returned from key handling.
#[derive(Debug, Clone)]
pub enum CorpusBrowserAction {
    /// No action needed
    None,
    /// User pressed Enter on a directory - edit all tracks in subtree
    EditDirectory(PathBuf),
    /// User pressed Enter on a file - edit single file
    EditFile(PathBuf),
    /// User pressed Esc - cancel and return to previous mode
    Cancel,
}
