//! Core types for the mutations system.
//!
//! All corpus-mutating operations are represented as variants of the `Mutation` enum.
//! This provides a unified, serializable representation of changes that can be:
//! - Queued for bulk execution
//! - Grouped by file for efficient batching
//! - Tracked for progress reporting
//! - Cancelled mid-execution

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Extracted metadata from an audio file, ready for indexing.
/// This is a Clone + Serialize version of the data from metadata::extract_metadata().
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractedMetadata {
    pub inode: i64,
    pub file_size: i64,
    pub file_type: String,
    pub duration_ms: Option<i64>,
    pub bitrate_kbps: Option<i32>,
    pub sample_rate: Option<i32>,
    /// Chromaprint acoustic fingerprint as raw u32 values.
    pub fingerprint: Option<Vec<u32>>,
    /// All tags extracted from the file: (tag_name, tag_value)
    pub tags: Vec<(String, String)>,
}

impl ExtractedMetadata {
    /// Create ExtractedMetadata from a Track and tags.
    ///
    /// Tags must be provided separately since they're stored in track_tags table.
    pub fn from_track_with_tags(track: &crate::corpus::db::Track, tags: Vec<(String, String)>) -> Self {
        Self {
            inode: track.inode,
            file_size: track.file_size,
            file_type: track.file_type.clone(),
            duration_ms: track.duration_ms,
            bitrate_kbps: track.bitrate_kbps,
            sample_rate: track.sample_rate,
            fingerprint: track.fingerprint.clone(),
            tags,
        }
    }

    /// Create ExtractedMetadata from a Track (without tags).
    pub fn from_track(track: &crate::corpus::db::Track) -> Self {
        Self::from_track_with_tags(track, Vec::new())
    }

    /// Get a tag value by name.
    pub fn get_tag(&self, name: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A single tag edit operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagEdit {
    pub tag_name: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
}

/// Single atomic mutation operation.
///
/// Each variant represents one logical change to the corpus. Mutations are:
/// - Serializable for persistence/logging
/// - Grouped by file path for efficient execution
/// - Categorized for batching same-type operations
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Mutation {
    // ========================================================================
    // Tag Operations
    // ========================================================================
    /// Edit tags in database only (no disk write).
    TagEditDb {
        track_id: i64,
        tag_name: String,
        old_value: Option<String>,
        new_value: Option<String>,
    },

    /// Flush tags to disk only (file write, no database).
    TagFlushToDisk {
        path: PathBuf,
        /// All tags to write to the file.
        tags: Vec<(String, String)>,
    },

    /// Combined tag edit: update database and write to disk in one operation.
    TagEditAndFlush {
        track_id: i64,
        path: PathBuf,
        edits: Vec<TagEdit>,
    },

    // ========================================================================
    // Indexing Operations
    // ========================================================================
    /// Index a track into the database from extracted metadata.
    IndexTrack {
        path: PathBuf,
        source: String,
        metadata: ExtractedMetadata,
    },

    /// Index a file from path only - extracts metadata during execution.
    ///
    /// This is the worker-thread-safe way to index files. Metadata extraction
    /// happens on the worker thread, not the UI thread.
    IndexFileFromPath {
        path: PathBuf,
        source: String,
    },

    /// Update scan state entry for incremental scanning.
    UpdateScanState {
        source: String,
        inode: u64,
        mtime_secs: i64,
        mtime_nanos: i64,
        file_size: u64,
        path: PathBuf,
    },

    /// Cleanup stale scan state entries (files that no longer exist).
    CleanupStaleScanState {
        source: String,
        valid_inodes: Vec<u64>,
    },

    // ========================================================================
    // File Operations
    // ========================================================================
    /// Move a file from source to destination.
    Move {
        source: PathBuf,
        destination: PathBuf,
        track_id: Option<i64>,
    },

    /// Copy a file from source to destination.
    Copy {
        source: PathBuf,
        destination: PathBuf,
    },

    /// Move a file to the stash directory.
    MoveToStash {
        path: PathBuf,
        track_id: Option<i64>,
        stash_name: String,
    },

    // ========================================================================
    // Deployment Operations
    // ========================================================================
    /// Create a hard link from source to destination.
    HardLink {
        source: PathBuf,
        destination: PathBuf,
    },

    /// Move a file within a library (e.g., stale file to correct location).
    ///
    /// Unlike corpus Move, this operates only on library paths and triggers
    /// library signal updates (clears LibraryStale for old path).
    LibraryMove {
        source: PathBuf,
        destination: PathBuf,
    },

    // ========================================================================
    // Database Migration Operations
    // ========================================================================
    /// Apply a database schema migration.
    DbMigration {
        migration_id: u32,
        description: String,
    },

    // ========================================================================
    // Signal Resolution Operations
    // ========================================================================
    /// Update track path in database (for relocated files).
    UpdateTrackPath {
        track_id: i64,
        old_path: PathBuf,
        new_path: PathBuf,
    },

    /// Update scan state path (for relocated files).
    UpdateScanStatePath {
        source: String,
        inode: i64,
        new_path: PathBuf,
    },

    /// Drop track from index (for missing files).
    DropFromIndex {
        track_id: i64,
        path: PathBuf,
        /// Also remove scan_state entry for this inode
        inode: Option<i64>,
        source: Option<String>,
    },

    /// Full track metadata update (for out-of-band file changes).
    UpdateTrack {
        track_id: i64,
        path: PathBuf,
        metadata: ExtractedMetadata,
    },
    // Note: VerifyTags has been moved to corpus::computations::Computation.
    // Computations don't alter state - they only emit signals.
}

/// Category for batching same-type operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MutationCategory {
    TagEdit,
    Indexing,
    FileMove,
    FileCopy,
    Deployment,
    Migration,
}

impl Mutation {
    /// Get the category of this mutation for batching.
    pub fn category(&self) -> MutationCategory {
        match self {
            Mutation::TagEditDb { .. }
            | Mutation::TagFlushToDisk { .. }
            | Mutation::TagEditAndFlush { .. } => MutationCategory::TagEdit,

            Mutation::IndexTrack { .. }
            | Mutation::IndexFileFromPath { .. }
            | Mutation::UpdateScanState { .. }
            | Mutation::CleanupStaleScanState { .. }
            | Mutation::UpdateTrackPath { .. }
            | Mutation::UpdateScanStatePath { .. }
            | Mutation::DropFromIndex { .. }
            | Mutation::UpdateTrack { .. } => MutationCategory::Indexing,

            Mutation::Move { .. } | Mutation::MoveToStash { .. } => MutationCategory::FileMove,

            Mutation::Copy { .. } => MutationCategory::FileCopy,

            Mutation::HardLink { .. } | Mutation::LibraryMove { .. } => MutationCategory::Deployment,

            Mutation::DbMigration { .. } => MutationCategory::Migration,
        }
    }

    /// Get the primary file path affected by this mutation, if any.
    pub fn primary_path(&self) -> Option<&Path> {
        match self {
            Mutation::TagFlushToDisk { path, .. }
            | Mutation::TagEditAndFlush { path, .. }
            | Mutation::IndexTrack { path, .. }
            | Mutation::IndexFileFromPath { path, .. }
            | Mutation::UpdateScanState { path, .. }
            | Mutation::Move { source: path, .. }
            | Mutation::Copy { source: path, .. }
            | Mutation::MoveToStash { path, .. }
            | Mutation::HardLink { source: path, .. }
            | Mutation::LibraryMove { source: path, .. } => Some(path),

            Mutation::TagEditDb { .. }
            | Mutation::CleanupStaleScanState { .. }
            | Mutation::DbMigration { .. }
            | Mutation::UpdateScanStatePath { .. } => None,

            Mutation::UpdateTrackPath { new_path: path, .. }
            | Mutation::DropFromIndex { path, .. }
            | Mutation::UpdateTrack { path, .. } => Some(path),
        }
    }

    /// Check if this mutation is database-only (no file system operations).
    pub fn is_db_only(&self) -> bool {
        matches!(
            self,
            Mutation::TagEditDb { .. }
                | Mutation::CleanupStaleScanState { .. }
                | Mutation::DbMigration { .. }
                | Mutation::UpdateTrackPath { .. }
                | Mutation::UpdateScanStatePath { .. }
                | Mutation::DropFromIndex { .. }
                | Mutation::UpdateTrack { .. }
        )
    }

    /// Check if this mutation requires serial execution (cannot be parallelized).
    pub fn requires_serial(&self) -> bool {
        matches!(self, Mutation::DbMigration { .. })
    }

    /// Get the track ID affected by this mutation, if any.
    ///
    /// Used to trigger health signal refresh after mutations.
    pub fn affected_track_id(&self) -> Option<i64> {
        match self {
            Mutation::TagEditDb { track_id, .. }
            | Mutation::TagEditAndFlush { track_id, .. }
            | Mutation::UpdateTrack { track_id, .. } => Some(*track_id),

            // These don't have a track_id directly
            Mutation::TagFlushToDisk { .. }
            | Mutation::IndexTrack { .. }
            | Mutation::IndexFileFromPath { .. }
            | Mutation::UpdateScanState { .. }
            | Mutation::CleanupStaleScanState { .. }
            | Mutation::UpdateTrackPath { .. }
            | Mutation::UpdateScanStatePath { .. }
            | Mutation::DropFromIndex { .. }
            | Mutation::Move { .. }
            | Mutation::Copy { .. }
            | Mutation::MoveToStash { .. }
            | Mutation::HardLink { .. }
            | Mutation::LibraryMove { .. }
            | Mutation::DbMigration { .. } => None,
        }
    }

    /// Get directories affected by this mutation for signal recomputation.
    ///
    /// After a mutation completes, signals in these directories may need
    /// to be recomputed (e.g., UnindexedFile → HealthyFile after indexing).
    pub fn affected_directories(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        match self {
            // Tag operations don't change file presence
            Mutation::TagEditDb { .. } => {}
            Mutation::TagFlushToDisk { path, .. } => {
                if let Some(parent) = path.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }
            Mutation::TagEditAndFlush { path, .. } => {
                if let Some(parent) = path.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }

            // Indexing operations affect the file's directory
            Mutation::IndexTrack { path, .. }
            | Mutation::IndexFileFromPath { path, .. }
            | Mutation::UpdateScanState { path, .. } => {
                if let Some(parent) = path.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }
            Mutation::CleanupStaleScanState { .. } => {
                // Affects multiple paths, but we don't track which ones
                // Signal recomputation will happen naturally on next eyeball
            }

            // File operations affect source and destination directories
            Mutation::Move { source, destination, .. } => {
                if let Some(parent) = source.parent() {
                    dirs.push(parent.to_path_buf());
                }
                if let Some(parent) = destination.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }
            Mutation::Copy { source, destination } => {
                if let Some(parent) = source.parent() {
                    dirs.push(parent.to_path_buf());
                }
                if let Some(parent) = destination.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }
            Mutation::MoveToStash { path, .. } => {
                if let Some(parent) = path.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }

            // Deployment operations happen outside corpus, don't affect corpus signals
            Mutation::HardLink { .. } | Mutation::LibraryMove { .. } => {}

            // Path updates affect both old and new directories
            Mutation::UpdateTrackPath { old_path, new_path, .. } => {
                if let Some(parent) = old_path.parent() {
                    dirs.push(parent.to_path_buf());
                }
                if let Some(parent) = new_path.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }
            // UpdateScanStatePath only has new_path (old path not tracked)
            Mutation::UpdateScanStatePath { new_path, .. } => {
                if let Some(parent) = new_path.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }

            // Index drops affect the file's directory
            Mutation::DropFromIndex { path, .. } => {
                if let Some(parent) = path.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }

            // Track updates don't change file presence
            Mutation::UpdateTrack { .. } => {}

            // Migration doesn't affect signals
            Mutation::DbMigration { .. } => {}
        }

        // Deduplicate directories
        dirs.sort();
        dirs.dedup();
        dirs
    }

    /// Get specific file paths affected by this mutation for signal updates.
    ///
    /// Unlike `affected_directories()` which returns parent directories,
    /// this returns the actual file paths that need signal updates.
    /// Used to spawn per-file `UpdateFileSignals` computations.
    pub fn affected_paths(&self) -> Vec<PathBuf> {
        match self {
            // Indexing: the file being indexed
            Mutation::IndexTrack { path, .. }
            | Mutation::IndexFileFromPath { path, .. }
            | Mutation::UpdateScanState { path, .. } => vec![path.clone()],

            // Tag operations: the file being modified
            Mutation::TagFlushToDisk { path, .. }
            | Mutation::TagEditAndFlush { path, .. } => vec![path.clone()],

            // File operations: source and destination
            Mutation::Move { source, destination, .. }
            | Mutation::Copy { source, destination }
            | Mutation::HardLink { source, destination }
            | Mutation::LibraryMove { source, destination } => {
                vec![source.clone(), destination.clone()]
            }

            Mutation::MoveToStash { path, .. } => {
                vec![path.clone()]
            }

            // Path updates: both old and new paths
            Mutation::UpdateTrackPath {
                old_path, new_path, ..
            } => vec![old_path.clone(), new_path.clone()],

            // Drop: the path being dropped
            Mutation::DropFromIndex { path, .. } => vec![path.clone()],

            // Track update: the path being updated
            Mutation::UpdateTrack { path, .. } => vec![path.clone()],

            // Operations without specific file paths that need signal updates
            Mutation::TagEditDb { .. }
            | Mutation::CleanupStaleScanState { .. }
            | Mutation::DbMigration { .. }
            | Mutation::UpdateScanStatePath { .. } => Vec::new(),
        }
    }
}

/// Result of executing a single mutation.
#[derive(Debug)]
pub struct MutationResult {
    pub mutation: Mutation,
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mutation_category() {
        let tag_edit = Mutation::TagEditDb {
            track_id: 1,
            tag_name: "artist".to_string(),
            old_value: Some("Old".to_string()),
            new_value: Some("New".to_string()),
        };
        assert_eq!(tag_edit.category(), MutationCategory::TagEdit);

        let file_move = Mutation::Move {
            source: PathBuf::from("/a"),
            destination: PathBuf::from("/b"),
            track_id: Some(1),
        };
        assert_eq!(file_move.category(), MutationCategory::FileMove);

        let migration = Mutation::DbMigration {
            migration_id: 3,
            description: "test".to_string(),
        };
        assert_eq!(migration.category(), MutationCategory::Migration);
        assert!(migration.is_db_only());
        assert!(migration.requires_serial());
    }

    #[test]
    fn test_mutation_primary_path() {
        let flush = Mutation::TagFlushToDisk {
            path: PathBuf::from("/test/file.flac"),
            tags: vec![],
        };
        assert_eq!(
            flush.primary_path(),
            Some(Path::new("/test/file.flac"))
        );

        let db_edit = Mutation::TagEditDb {
            track_id: 1,
            tag_name: "artist".to_string(),
            old_value: None,
            new_value: Some("Artist".to_string()),
        };
        assert_eq!(db_edit.primary_path(), None);
    }
}
