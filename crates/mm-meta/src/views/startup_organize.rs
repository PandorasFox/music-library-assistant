//! Startup intake modal data types.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ============================================================================
// Intake Confirmation Types
// ============================================================================

/// Where the intake confirmation was triggered from.
///
/// Replaces the old `zone: String` for post-action routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntakeSource {
    /// Triggered at startup after eyeballing completes
    Startup,
    /// Triggered from Health Insights "Index unindexed" action
    Health,
}

/// A directory group for display purposes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryGroup {
    /// Display path (relative to corpus root)
    pub display_path: String,
    /// Filenames within this directory
    pub filenames: Vec<String>,
}

/// File entry with path for indexing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnindexedFileEntry {
    /// Absolute path for indexing
    pub abs_path: PathBuf,
}

/// State for the intake confirmation modal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntakeConfirmationState {
    /// Number of unindexed files detected
    pub file_count: usize,
    /// Total bytes to read (sum of file sizes)
    pub total_bytes: u64,
    /// Files to index, keyed by inode
    pub files: Vec<UnindexedFileEntry>,
    /// Where this intake was triggered from (for post-action routing)
    pub source: IntakeSource,
    /// Number of directories containing unindexed files
    pub _directory_count: usize,
    /// Files grouped by directory for display
    pub grouped_files: Vec<DirectoryGroup>,
    /// Scroll offset for file list
    pub scroll_offset: usize,
}

impl IntakeConfirmationState {
    /// Create IndexFileFromPath mutations for all unindexed files.
    ///
    /// These mutations contain the absolute path - metadata extraction happens
    /// on the worker thread, not the UI thread.
    pub fn create_index_mutations(&self) -> Vec<crate::mutations::Mutation> {
        self.files
            .iter()
            .map(|entry| {
                crate::mutations::Mutation::IndexFileFromPath(
                    crate::mutations::indexing::IndexFileFromPathMutation {
                        path: entry.abs_path.clone(),
                        zone: "corpus".to_string(),
                    },
                )
            })
            .collect()
    }
}
