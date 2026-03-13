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

// ============================================================================
// Directory Grouping
// ============================================================================

/// Group organizable files into InboxDirectory structs based on granularity.
pub fn group_into_directories(
    files: &[(i64, String)],
    inbox_dir: &std::path::Path,
    granularity: crate::config::InboxOrganizeGranularity,
    resolver: &crate::paths::PathResolver,
) -> Vec<InboxDirectory> {
    use std::collections::BTreeMap;
    use crate::config::InboxOrganizeGranularity;

    // Convert relative paths to absolute, group by parent directory
    let mut dir_groups: BTreeMap<PathBuf, Vec<InboxOrganizeFile>> = BTreeMap::new();

    for (inode, rel_path) in files {
        let abs_path = resolver.resolve(std::path::Path::new(rel_path));

        let parent = match abs_path.parent() {
            Some(p) => p.to_path_buf(),
            None => continue,
        };
        let filename = abs_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        dir_groups
            .entry(parent)
            .or_default()
            .push(InboxOrganizeFile {
                inode: *inode,
                path: abs_path,
                filename,
            });
    }

    match granularity {
        InboxOrganizeGranularity::Leaf => {
            // Use directories as-is (deepest dirs containing files)
            dir_groups
                .into_iter()
                .map(|(dir_path, files)| {
                    let dir_name = dir_path
                        .strip_prefix(inbox_dir)
                        .unwrap_or(&dir_path)
                        .to_string_lossy()
                        .to_string();
                    InboxDirectory {
                        dir_name,
                        dir_path,
                        files,
                    }
                })
                .collect()
        }
        InboxOrganizeGranularity::TopLevel => {
            // Group by top-level child of inbox/
            let mut top_groups: BTreeMap<PathBuf, Vec<InboxOrganizeFile>> = BTreeMap::new();

            for (dir_path, files) in dir_groups {
                // Find the top-level directory under inbox/
                let rel = dir_path.strip_prefix(inbox_dir).unwrap_or(&dir_path);
                let top_component = rel
                    .components()
                    .next()
                    .map(|c| inbox_dir.join(c.as_os_str()))
                    .unwrap_or_else(|| dir_path.clone());

                top_groups.entry(top_component).or_default().extend(files);
            }

            top_groups
                .into_iter()
                .map(|(dir_path, files)| {
                    let dir_name = dir_path
                        .strip_prefix(inbox_dir)
                        .unwrap_or(&dir_path)
                        .to_string_lossy()
                        .to_string();
                    InboxDirectory {
                        dir_name,
                        dir_path,
                        files,
                    }
                })
                .collect()
        }
    }
}
