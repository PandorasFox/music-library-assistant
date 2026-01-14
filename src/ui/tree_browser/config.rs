//! Tree Browser Configuration
//!
//! Configuration types for the tree browser and its variants.

use std::path::PathBuf;

/// Core configuration shared by all browser variants.
#[derive(Debug, Clone)]
pub struct TreeBrowserConfig {
    /// Header/title text
    pub title: String,
    /// Root path to browse
    pub root_path: PathBuf,
    /// Whether this browser is part of the lateral view ring (Tab/Shift-Tab cycling)
    pub in_lateral_ring: bool,
}

impl TreeBrowserConfig {
    /// Create a new config with the given root path.
    pub fn new(root_path: PathBuf, title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            root_path,
            in_lateral_ring: false,
        }
    }

    /// Set whether this browser participates in lateral view cycling.
    pub fn with_lateral_ring(mut self, enabled: bool) -> Self {
        self.in_lateral_ring = enabled;
        self
    }
}

/// Configuration specific to the CorpusBrowser variant.
#[derive(Debug, Clone)]
pub struct CorpusBrowserConfig {
    /// Show audio files in tree (not just directories)
    pub show_files: bool,
}

impl Default for CorpusBrowserConfig {
    fn default() -> Self {
        Self { show_files: true }
    }
}

/// Configuration specific to the DirectorySelector variant.
#[derive(Debug, Clone)]
pub struct DirectorySelectorConfig {
    /// Allow multiple directory selections
    pub multi_select: bool,
    /// Show audio file counts per directory
    pub show_item_counts: bool,
}

impl Default for DirectorySelectorConfig {
    fn default() -> Self {
        Self {
            multi_select: true,
            show_item_counts: true,
        }
    }
}

impl DirectorySelectorConfig {
    /// Create a config preset for fingerprint-based deduplication (sleuthing).
    pub fn for_sleuthing() -> Self {
        Self {
            multi_select: true,
            show_item_counts: true,
        }
    }
}
