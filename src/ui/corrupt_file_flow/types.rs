//! Corrupt File Resolution Modal Types
//!
//! Data structures for the corrupt file resolution modal, including
//! file entries and button state.

use std::path::PathBuf;

use anyhow::Result;

use crate::corpus::db::ReadOnlyDb;
use crate::corpus::mutations::Mutation;
use crate::corpus::paths;

/// A corrupt corpus file (tag parse error or waveform decode failure).
#[derive(Debug, Clone)]
pub struct CorruptFileEntry {
    /// Corpus path (relative)
    pub corpus_path: String,
    /// Track ID from tracks table
    pub track_id: i64,
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
    pub fn load(read_db: &ReadOnlyDb<'_>) -> Result<Self> {
        // Get all CorruptFile signals (issue_key = corpus path)
        let corrupt_paths = read_db.get_corrupt_file_paths()?;

        if corrupt_paths.is_empty() {
            return Ok(Self::default());
        }

        // Get track info for each path
        let mut files = Vec::new();
        for corpus_path in corrupt_paths {
            // Get track info for this path
            let track = match read_db.get_track_by_path(&corpus_path)? {
                Some(t) => t,
                None => continue, // Signal refers to non-existent track, skip
            };

            let track_id = track.id.unwrap_or(0);
            let inode = track.inode;

            files.push(CorruptFileEntry {
                corpus_path,
                track_id,
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

    /// Generate MoveToStash + DropFromIndex mutations for all files.
    ///
    /// For each corrupt file:
    /// 1. MoveToStash to stash/corrupt/
    /// 2. DropFromIndex to remove from database
    pub fn stash_and_drop_mutations(&self) -> Vec<Mutation> {
        let resolver = paths::get_resolver();
        let mut mutations = Vec::new();

        for file in &self.files {
            // Resolve relative path to absolute for filesystem operations
            let abs_path = resolver.resolve(std::path::Path::new(&file.corpus_path));

            // MoveToStash mutation
            mutations.push(Mutation::MoveToStash {
                path: abs_path.clone(),
                stash_name: "corrupt".to_string(),
            });

            // DropFromIndex mutation
            mutations.push(Mutation::DropFromIndex {
                track_id: file.track_id,
                path: PathBuf::from(&file.corpus_path),
                inode: Some(file.inode),
                source: Some("corpus".to_string()),
            });
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
