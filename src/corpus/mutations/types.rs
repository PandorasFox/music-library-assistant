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
    pub fingerprint: Option<String>,
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

    /// Delete a file.
    Delete {
        path: PathBuf,
        track_id: Option<i64>,
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

    /// Remove a deployed link.
    Unlink { path: PathBuf },

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
    FileDelete,
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

            Mutation::Delete { .. } => MutationCategory::FileDelete,

            Mutation::HardLink { .. } | Mutation::Unlink { .. } => MutationCategory::Deployment,

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
            | Mutation::Delete { path, .. }
            | Mutation::MoveToStash { path, .. }
            | Mutation::HardLink { source: path, .. }
            | Mutation::Unlink { path, .. } => Some(path),

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
            | Mutation::Delete { .. }
            | Mutation::MoveToStash { .. }
            | Mutation::HardLink { .. }
            | Mutation::Unlink { .. }
            | Mutation::DbMigration { .. } => None,
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

/// Work unit for bulk execution - groups mutations by file.
#[derive(Debug)]
pub struct WorkUnit {
    pub path: PathBuf,
    pub track_id: Option<i64>,
    pub mutations: Vec<Mutation>,
}

/// Progress update during bulk execution.
#[derive(Debug, Clone)]
pub struct ExecutionProgress {
    pub completed: usize,
    pub total: usize,
    pub current_file: Option<String>,
    pub errors: Vec<String>,
}

/// Final result of bulk execution.
#[derive(Debug)]
pub struct ExecutionResult {
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub results: Vec<MutationResult>,
    pub duration_ms: u64,
}

impl ExecutionResult {
    /// Create an empty result.
    pub fn empty() -> Self {
        Self {
            total: 0,
            succeeded: 0,
            failed: 0,
            results: Vec::new(),
            duration_ms: 0,
        }
    }

    /// Check if all mutations succeeded.
    pub fn all_succeeded(&self) -> bool {
        self.failed == 0
    }
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
