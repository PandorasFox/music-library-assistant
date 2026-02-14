//! Subpar Duplicate Resolution Modal Types
//!
//! Data structures for the subpar duplicate resolution modal, including
//! file entries and button state.

use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use anyhow::Result;

use crate::corpus::db::ReadOnlyDb;
use crate::meta::mutations::Mutation;
use crate::meta::mutations::file_ops::MoveToStashMutation;
use crate::meta::mutations::indexing::DropFromIndexMutation;
use crate::corpus::paths;

/// A subpar duplicate file ready for stashing.
#[derive(Debug, Clone)]
pub struct SubparFileEntry {
    /// Corpus path (the subpar file)
    pub corpus_path: String,
    /// Inode (for DropFromIndex cleanup)
    pub inode: i64,
    /// Reason for being subpar (human-readable)
    pub reason: String,
    /// Path to the superior version
    pub superior_path: String,
}

/// Cached data for the subpar duplicate resolution modal.
///
/// Loaded once when the modal opens. All renders use this cached data.
#[derive(Debug, Clone, Default)]
pub struct SubparDuplicateModalData {
    /// Files with SubparDuplicate signals
    pub files: Vec<SubparFileEntry>,
}

impl SubparDuplicateModalData {
    /// Load subpar duplicate files from the database.
    ///
    /// Subpar duplicate files exist on disk but are lower quality versions of other files.
    /// We try to get the inode from the database first, then fall back to filesystem lookup.
    pub fn load(read_db: &ReadOnlyDb<'_>) -> Result<Self> {
        // Get all SubparDuplicate signals with metadata
        let subpar_entries = read_db.get_subpar_duplicate_files()?;

        if subpar_entries.is_empty() {
            return Ok(Self::default());
        }

        let resolver = paths::get_resolver();
        let mut files = Vec::new();

        for entry in subpar_entries {
            // Try to get inode from database first
            let inode = if let Some(af) = read_db.get_audio_file_by_path(&entry.corpus_path)? {
                af.inode()
            } else if let Some(fe) = read_db.get_file_entry_by_path(&entry.corpus_path, "corpus")? {
                fe.inode
            } else {
                // Fall back to filesystem lookup (subpar files should exist on disk)
                let abs_path = resolver.resolve(std::path::Path::new(&entry.corpus_path));
                if let Ok(metadata) = std::fs::metadata(&abs_path) {
                    metadata.ino() as i64
                } else {
                    // File doesn't exist on disk - skip it
                    continue;
                }
            };

            // Convert reason to human-readable
            let reason = match entry.reason.as_str() {
                "SubparBitrate" => "Lower bitrate".to_string(),
                "SubparFormat" => "Worse format".to_string(),
                "SubparSampleRate" => "Lower sample rate".to_string(),
                other => other.to_string(),
            };

            files.push(SubparFileEntry {
                corpus_path: entry.corpus_path,
                inode,
                reason,
                superior_path: entry.superior_path,
            });
        }

        Ok(Self { files })
    }

    /// Total number of subpar files.
    pub fn total_count(&self) -> usize {
        self.files.len()
    }

    /// Check if there are any subpar files.
    pub fn has_files(&self) -> bool {
        !self.files.is_empty()
    }

    /// Generate MoveToStash + DropFromIndex mutations for all files.
    ///
    /// For each subpar file:
    /// 1. MoveToStash to stash/subpar/
    /// 2. DropFromIndex to remove from database
    pub fn stash_and_drop_mutations(&self) -> Vec<Mutation> {
        let resolver = paths::get_resolver();
        let mut mutations = Vec::new();

        for file in &self.files {
            // Resolve relative path to absolute for filesystem operations
            let abs_path = resolver.resolve(std::path::Path::new(&file.corpus_path));

            // MoveToStash mutation
            mutations.push(Mutation::MoveToStash(MoveToStashMutation {
                path: abs_path.clone(),
                stash_name: "subpar".to_string(),
            }));

            // DropFromIndex mutation
            mutations.push(Mutation::DropFromIndex(DropFromIndexMutation {
                path: PathBuf::from(&file.corpus_path),
                inode: Some(file.inode),
                source: Some("corpus".to_string()),
            }));
        }

        mutations
    }
}

/// Which action button is selected in the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectedButton {
    StashAll,
    #[default]
    Cancel,
}

impl SelectedButton {
    /// Move selection left.
    pub fn left(&mut self, has_files: bool) {
        *self = match *self {
            Self::Cancel => {
                if has_files {
                    Self::StashAll
                } else {
                    Self::Cancel
                }
            }
            Self::StashAll => Self::StashAll,
        };
    }

    /// Move selection right.
    pub fn right(&mut self, _has_files: bool) {
        *self = match *self {
            Self::StashAll => Self::Cancel,
            Self::Cancel => Self::Cancel,
        };
    }
}
