//! Tree Browser Configuration
//!
//! Configuration types for the tree browser.

/// Configuration for the CorpusBrowser variant.
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

