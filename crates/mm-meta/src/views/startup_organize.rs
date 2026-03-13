//! Startup and inbox organize modal data types.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::db_types::Zone;

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
    /// Triggered from Inbox view or inbox lateral navigation
    Inbox,
}

/// A directory group for display purposes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryGroup {
    /// Display path (relative to corpus root)
    pub display_path: String,
    /// Filenames within this directory
    pub filenames: Vec<String>,
    /// Zone this directory belongs to
    pub zone: Zone,
}

/// File entry with path for indexing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnindexedFileEntry {
    /// Absolute path for indexing
    pub abs_path: PathBuf,
    /// Zone this file belongs to (for mutation creation)
    pub zone: Zone,
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
    /// Whether this state contains files from multiple zones (corpus + inbox)
    pub multi_zone: bool,
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
                        zone: entry.zone.as_str().to_string(),
                    },
                )
            })
            .collect()
    }
}

// ============================================================================
// Inbox Organize Types
// ============================================================================

/// A directory of inbox files to organize.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxDirectory {
    /// Display name for this directory group
    pub dir_name: String,
    /// Absolute path to the inbox directory
    pub dir_path: PathBuf,
    /// Files within this directory
    pub files: Vec<InboxOrganizeFile>,
}

/// A single organizable inbox file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxOrganizeFile {
    pub inode: i64,
    /// Absolute path to the file
    pub path: PathBuf,
    /// Just the filename (for display)
    pub filename: String,
}
