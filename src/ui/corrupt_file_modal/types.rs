//! Corrupt File Resolution Modal Types
//!
//! Data structures for the corrupt file resolution modal, including
//! file entries and button state.

use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use anyhow::Result;

use crate::corpus::db::ReadOnlyDb;
use crate::meta::mutations::Mutation;
use crate::meta::mutations::file_ops::StashFromZoneMutation;
use crate::meta::mutations::indexing::DropFromIndexMutation;
use crate::corpus::paths;

/// A corrupt corpus file (tag parse error or waveform decode failure).
#[derive(Debug, Clone)]
pub struct CorruptFileEntry {
    /// Corpus path (relative)
    pub corpus_path: String,
    /// Inode (for DropFromIndex cleanup)
    pub inode: i64,
}

/// Cached data for the corrupt file resolution modal.
///
/// Loaded once when the modal opens. All renders use this cached data.
#[derive(Debug, Clone, Default)]
pub struct CorruptFileModalData {
    /// Files with CorruptFile signals
    pub files: Vec<CorruptFileEntry>,
}

impl CorruptFileModalData {
    /// Load corrupt files from the database.
    ///
    /// Corrupt files exist on disk but failed indexing (tag parse error or audio decode failure).
    /// They may not have audio_info entries since indexing failed.
    /// We get the inode from the filesystem directly since the file exists on disk.
    pub fn load(read_db: &ReadOnlyDb<'_>) -> Result<Self> {
        // Get all CorruptFile signals (issue_key = corpus path)
        let corrupt_paths = read_db.get_corrupt_file_paths()?;

        if corrupt_paths.is_empty() {
            return Ok(Self::default());
        }

        let resolver = paths::get_resolver();
        let mut files = Vec::new();

        for corpus_path in corrupt_paths {
            // Resolve to absolute path
            let abs_path = resolver.resolve(std::path::Path::new(&corpus_path));

            // Get inode from filesystem (corrupt files exist on disk)
            let inode = if let Ok(metadata) = std::fs::metadata(&abs_path) {
                metadata.ino() as i64
            } else {
                // File doesn't exist on disk anymore - skip it
                // The signal will be cleared on next scan
                continue;
            };

            files.push(CorruptFileEntry {
                corpus_path,
                inode,
            });
        }

        Ok(Self { files })
    }

    /// Total number of corrupt files.
    pub fn total_count(&self) -> usize {
        self.files.len()
    }

    /// Check if there are any corrupt files.
    pub fn has_files(&self) -> bool {
        !self.files.is_empty()
    }

    /// Generate StashFromZone + DropFromIndex mutations for all files.
    ///
    /// For each corrupt file:
    /// 1. StashFromZone to stash/corrupt/
    /// 2. DropFromIndex to remove from database
    pub fn stash_and_drop_mutations(&self) -> Vec<Mutation> {
        let resolver = paths::get_resolver();
        let mut mutations = Vec::new();

        for file in &self.files {
            // Resolve relative path to absolute for filesystem operations
            let abs_path = resolver.resolve(std::path::Path::new(&file.corpus_path));

            // StashFromZone mutation
            mutations.push(Mutation::StashFromZone(StashFromZoneMutation {
                path: abs_path.clone(),
                stash_name: "corrupt".to_string(),
            }));

            // DropFromIndex mutation
            mutations.push(Mutation::DropFromIndex(DropFromIndexMutation {
                path: PathBuf::from(&file.corpus_path),
                inode: Some(file.inode),
                zone: Some("corpus".to_string()),
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
