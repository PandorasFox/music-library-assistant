//! Core types for the mutations system.
//!
//! All corpus-mutating operations are represented as variants of the `Mutation` enum.
//! This provides a unified, serializable representation of changes that can be:
//! - Queued for bulk execution
//! - Grouped by file for efficient batching
//! - Tracked for progress reporting
//! - Cancelled mid-execution

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::corpus::transcode::TranscodeTarget;

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

/// Single atomic mutation operation.
///
/// Each variant represents one logical change to the corpus. Mutations are:
/// - Serializable for persistence/logging
/// - Grouped by file path for efficient execution
/// - Categorized for batching same-type operations
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Mutation {
    // ========================================================================
    // Tag Operations (DB-First Pattern with Spawn Chaining)
    // ========================================================================
    /// Set track tags in database only (DB-first pattern, step 1).
    ///
    /// - Writes tags to DB via set_track_tags() (full replacement)
    /// - Sets needs_disk_flush = TRUE
    /// - **Spawns** ApplyDbTagsToDisk to sync to disk (step 2)
    ///
    /// If interrupted between DB write and disk write, needs_disk_flush=TRUE
    /// enables recovery via OOB sync flow.
    SetTrackTagsDb {
        track_id: i64,
        /// Complete set of tags to store (replaces all existing tags).
        tags: Vec<(String, String)>,
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
    },

    /// Copy a file from source to destination.
    Copy {
        source: PathBuf,
        destination: PathBuf,
    },

    /// Move a file to the stash directory.
    MoveToStash {
        path: PathBuf,
        stash_name: String,
    },

    // ========================================================================
    // Transcode Operations
    // ========================================================================
    /// Transcode a file to a different container/codec format.
    ///
    /// On success: creates new file at same path with different extension,
    /// stashes original under stash_name, and updates the track record.
    Transcode {
        track_id: i64,
        source_path: PathBuf,
        target_format: TranscodeTarget,
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

    // ========================================================================
    // OOB Resolution Operations
    // ========================================================================
    /// Acknowledge mtime-only change - update scan_state, clear MtimeOnlyMismatch signal.
    ///
    /// Used when disk file mtime changed but tags are identical. Updates scan_state
    /// to match current disk mtime so file is considered synced.
    AcknowledgeMtimeOnly {
        /// Track IDs with their absolute paths: (track_id, abs_path)
        tracks: Vec<(i64, PathBuf)>,
    },

    /// Acknowledge inode change - update track.inode and scan_state, clear InodeChanged signal.
    ///
    /// Used when a file was replaced (same path, different inode). Updates the stored
    /// inode to match disk and refreshes scan_state. Tag differences are handled separately
    /// through the OOB tag resolution flow.
    AcknowledgeInodeChanged {
        /// Track IDs with their absolute paths: (track_id, abs_path)
        tracks: Vec<(i64, PathBuf)>,
    },

    /// Apply DB tags to disk file (defer to db / reject disk changes).
    ///
    /// - Reads tags from database (source of truth)
    /// - Writes to disk via write_file_tags()
    /// - Updates mtime in scan_state after write
    /// - Clears needs_disk_flush flag
    /// - Clears OOB signals and tag_mismatches
    ///
    /// Used for:
    /// - OOB sync resolution (reject disk changes)
    /// - Spawned from SetTrackTagsDb (DB-first pattern step 2)
    ApplyDbTagsToDisk {
        track_id: i64,
        path: PathBuf,
    },

    /// Assimilate disk tags into DB (defer to corpus / accept disk changes).
    ///
    /// - Reads tags from disk file
    /// - Writes to database, overwriting DB values
    /// - Updates mtime in scan_state to match disk
    /// - Clears OOB signals and tag_mismatches
    ///
    /// Used for OOB sync resolution (accept disk changes).
    AssimilateDiskTagsToDb {
        track_id: i64,
        path: PathBuf,
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
    Transcode,
}

impl Mutation {
    /// Get the category of this mutation for batching.
    pub fn category(&self) -> MutationCategory {
        match self {
            // Tag sync operations (write to disk)
            Mutation::ApplyDbTagsToDisk { .. }
            | Mutation::AssimilateDiskTagsToDb { .. } => MutationCategory::TagEdit,

            // SetTrackTagsDb is DB-only and fast, categorized with indexing
            Mutation::SetTrackTagsDb { .. }
            | Mutation::IndexTrack { .. }
            | Mutation::IndexFileFromPath { .. }
            | Mutation::UpdateScanState { .. }
            | Mutation::CleanupStaleScanState { .. }
            | Mutation::UpdateTrackPath { .. }
            | Mutation::UpdateScanStatePath { .. }
            | Mutation::DropFromIndex { .. }
            | Mutation::UpdateTrack { .. }
            | Mutation::AcknowledgeMtimeOnly { .. }
            | Mutation::AcknowledgeInodeChanged { .. } => MutationCategory::Indexing,

            Mutation::Move { .. } | Mutation::MoveToStash { .. } => MutationCategory::FileMove,

            Mutation::Copy { .. } => MutationCategory::FileCopy,

            Mutation::HardLink { .. } | Mutation::LibraryMove { .. } => MutationCategory::Deployment,

            Mutation::DbMigration { .. } => MutationCategory::Migration,

            Mutation::Transcode { .. } => MutationCategory::Transcode,
        }
    }

    /// Check if this mutation is database-only (no file system operations).
    pub fn is_db_only(&self) -> bool {
        matches!(
            self,
            Mutation::SetTrackTagsDb { .. }
                | Mutation::CleanupStaleScanState { .. }
                | Mutation::DbMigration { .. }
                | Mutation::UpdateTrackPath { .. }
                | Mutation::UpdateScanStatePath { .. }
                | Mutation::DropFromIndex { .. }
                | Mutation::UpdateTrack { .. }
                | Mutation::AcknowledgeMtimeOnly { .. }
                | Mutation::AssimilateDiskTagsToDb { .. }
            // Note: ApplyDbTagsToDisk writes to disk, so NOT db-only
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
            Mutation::SetTrackTagsDb { track_id, .. }
            | Mutation::UpdateTrack { track_id, .. }
            | Mutation::Transcode { track_id, .. }
            | Mutation::ApplyDbTagsToDisk { track_id, .. }
            | Mutation::AssimilateDiskTagsToDb { track_id, .. } => Some(*track_id),

            // These don't have a single track_id directly (batch operations or no track)
            Mutation::IndexTrack { .. }
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
            | Mutation::DbMigration { .. }
            | Mutation::AcknowledgeMtimeOnly { .. }
            | Mutation::AcknowledgeInodeChanged { .. } => None,
        }
    }

    /// Get directories affected by this mutation for signal recomputation.
    ///
    /// After a mutation completes, signals in these directories may need
    /// to be recomputed (e.g., UnindexedFile → HealthyFile after indexing).
    pub fn affected_directories(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        match self {
            // DB-only tag operations don't change file presence
            Mutation::SetTrackTagsDb { .. } => {}

            // Single-track tag sync operations affect the file's directory
            Mutation::ApplyDbTagsToDisk { path, .. }
            | Mutation::AssimilateDiskTagsToDb { path, .. } => {
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

            // Transcode: affects the source file's directory (output is same dir, new extension)
            Mutation::Transcode { source_path, .. } => {
                if let Some(parent) = source_path.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }

            // Track updates don't change file presence
            Mutation::UpdateTrack { .. } => {}

            // Migration doesn't affect signals
            Mutation::DbMigration { .. } => {}

            // Batch OOB resolution: paths resolved at execution time, executors spawn follow-ups directly
            Mutation::AcknowledgeMtimeOnly { .. }
            | Mutation::AcknowledgeInodeChanged { .. } => {}
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

            // Tag operations: single-track mutations with path
            Mutation::ApplyDbTagsToDisk { path, .. }
            | Mutation::AssimilateDiskTagsToDb { path, .. } => vec![path.clone()],

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

            // Transcode: source path and new output path
            Mutation::Transcode { source_path, target_format, .. } => {
                let mut paths = vec![source_path.clone()];
                let new_path = source_path.with_extension(target_format.extension());
                paths.push(new_path);
                paths
            }

            // Batch OOB resolution: paths are stored in the mutation
            Mutation::AcknowledgeMtimeOnly { tracks }
            | Mutation::AcknowledgeInodeChanged { tracks } => {
                tracks.iter().map(|(_, path)| path.clone()).collect()
            }

            // Operations without specific file paths that need signal updates
            Mutation::SetTrackTagsDb { .. }
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
    /// Follow-up mutations to queue (from spawn chaining).
    /// E.g., SetTrackTagsDb spawns ApplyDbTagsToDisk after DB write succeeds.
    pub spawn_mutations: Vec<Mutation>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mutation_category() {
        let set_tags = Mutation::SetTrackTagsDb {
            track_id: 1,
            tags: vec![("artist".to_string(), "New".to_string())],
        };
        assert_eq!(set_tags.category(), MutationCategory::Indexing);
        assert!(set_tags.is_db_only());

        let apply_tags = Mutation::ApplyDbTagsToDisk {
            track_id: 1,
            path: PathBuf::from("/test/file.flac"),
        };
        assert_eq!(apply_tags.category(), MutationCategory::TagEdit);
        assert!(!apply_tags.is_db_only());

        let file_move = Mutation::Move {
            source: PathBuf::from("/a"),
            destination: PathBuf::from("/b"),
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

}
