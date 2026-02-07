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

use crate::corpus::computations::{Computation, awakening};
use crate::corpus::db::types::{CorpusFileSignalType, FileSignalType, LibraryFileSignalType};
use crate::corpus::tags::TagSet;
use crate::corpus::transcode::TranscodeTarget;

// ============================================================================
// TagOp - Incremental Tag Operations
// ============================================================================

/// An atomic tag operation on a specific track.
///
/// Expressed as (inode, tag_name, old_value, new_value) where:
/// - old=Some, new=Some → replace value (validate old exists)
/// - old=Some, new=None → drop value (validate old exists)
/// - old=None, new=Some → add value (idempotent)
/// - old=None, new=None → no-op
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TagOp {
    pub inode: i64,
    pub tag_name: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
}

impl TagOp {
    /// Create an operation to drop a tag value.
    /// Validates that the old value exists before dropping.
    pub fn drop_tag(inode: i64, tag_name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            inode,
            tag_name: tag_name.into(),
            old_value: Some(value.into()),
            new_value: None,
        }
    }

    /// Create an operation to add a tag value.
    /// Idempotent - won't fail if value already exists.
    pub fn add_tag(inode: i64, tag_name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            inode,
            tag_name: tag_name.into(),
            old_value: None,
            new_value: Some(value.into()),
        }
    }

    /// Create an operation to replace a tag value.
    /// Validates that the old value exists before replacing.
    pub fn replace_tag(
        inode: i64,
        tag_name: impl Into<String>,
        old: impl Into<String>,
        new: impl Into<String>,
    ) -> Self {
        Self {
            inode,
            tag_name: tag_name.into(),
            old_value: Some(old.into()),
            new_value: Some(new.into()),
        }
    }

    /// True if this is a no-op (old == new for replace, or both None).
    pub fn is_nop(&self) -> bool {
        match (&self.old_value, &self.new_value) {
            (Some(old), Some(new)) => old == new,
            (None, None) => true,
            _ => false,
        }
    }
}

// ============================================================================
// Pending Signal Types
// ============================================================================

/// Signal to be emitted post-execution.
///
/// Avoids race condition with async DB writes by carrying signal data from
/// execution time rather than querying DB after the write is sent.
///
/// When a mutation executes and sends a fire-and-forget write to db_thread,
/// querying the read-only connection immediately may not see the write yet.
/// By embedding the signal data in the MutationResult, we avoid this race.
#[derive(Debug, Clone)]
pub enum PendingSignal {
    /// File signal without metadata
    FileSignal {
        signal_type: CorpusFileSignalType,
        path: String,
    },
    /// File signal with JSON metadata
    FileSignalWithMetadata {
        signal_type: CorpusFileSignalType,
        path: String,
        metadata_json: String,
    },
}

// ============================================================================
// Post-Execution Behavior Types
// ============================================================================

/// Signal clearing scope after successful mutation.
///
/// Determines which signals are cleared for affected paths after a mutation executes.
/// This is enforced via exhaustive match in `Mutation::signal_clear_scope()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalClearScope {
    /// Clear only mutable signals (preserve CorruptFile, ShitFormat).
    /// Used for mutations that modify files but don't remove them.
    MutableOnly,
    /// Clear ALL signals including file-inherent ones.
    /// Used when the file is gone (stashed, dropped) or replaced entirely (transcode).
    All,
    /// No signal clearing needed.
    /// Used for DB-only operations with no file impact.
    None,
}

/// Specific signal to clear by type and key pattern (beyond path-based clearing).
///
/// Used for targeted signal clearing like LibraryStale, aggregate tag signals, etc.
/// where the signal key doesn't match the mutation's affected paths.
#[derive(Debug, Clone)]
pub struct SignalToClear {
    pub signal_type: FileSignalType,
    pub key_pattern: String,
}

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
    /// All tags extracted from the file.
    pub tags: TagSet,
}

impl ExtractedMetadata {
    /// Get a tag value by name.
    #[cfg(test)]
    pub fn get_tag(&self, name: &str) -> Option<&str> {
        self.tags.values_for(name).next()
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
    // Tag Operations (Incremental with Validation)
    // ========================================================================
    /// Apply a set of incremental tag operations to tracks.
    ///
    /// Operations are pre-coalesced by inode. At execution time:
    /// 1. Group ops by inode
    /// 2. For each inode: read current tags, validate expected old_values
    /// 3. If ANY validation fails for an inode → that inode's ops fail (others continue)
    /// 4. Apply tag ops directly via apply_index_tag_ops (INSERT/UPDATE/DELETE)
    /// 5. Spawn ApplyDbTagsToDisk for each modified inode
    ApplyTagOps {
        ops: Vec<TagOp>,
    },

    // ========================================================================
    // Indexing Operations
    // ========================================================================
    /// Index a file from path only - extracts metadata during execution.
    ///
    /// This is the worker-thread-safe way to index files. Metadata extraction
    /// happens on the worker thread, not the UI thread.
    IndexFileFromPath {
        path: PathBuf,
        source: String,
    },

    // ========================================================================
    // File Operations
    // ========================================================================
    /// Move a file from source to destination.
    ///
    /// NOTE: This mutation is fully plumbed but intentionally not yet utilized in UI.
    /// It will be used by the inbox intake flow for moving files from inbox to corpus.
    /// This is an exception to our "don't add unused code" policy - we need to stop
    /// adding infrastructure too early in general.
    Move {
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
        inode: i64,
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
    /// Update file path in files table (for relocated files).
    UpdateFilePath {
        source: String,
        inode: i64,
        new_path: PathBuf,
    },

    /// Drop file from index (for missing files or orphaned signals).
    DropFromIndex {
        path: PathBuf,
        /// Inode to also remove from files table (None for orphaned signals)
        inode: Option<i64>,
        source: Option<String>,
    },

    /// Drop a directory and all its contents from the index.
    ///
    /// Used when an entire directory is deleted externally and acknowledged.
    /// - Removes directory entry from files table
    /// - Removes all file entries under the directory from files table
    /// - Removes all audio_info entries for those files
    /// - Clears MissingDirectory signal for the directory
    /// - Clears MissingFile signals for files within
    DropDirectoryFromIndex {
        /// Relative path of the directory
        directory_path: PathBuf,
    },

    // ========================================================================
    // OOB Resolution Operations
    // ========================================================================
    /// Acknowledge mtime-only change - update file mtime, clear MtimeOnlyMismatch signal.
    ///
    /// Used when disk file mtime changed but tags are identical. Updates file mtime
    /// to match current disk mtime so file is considered synced.
    AcknowledgeMtimeOnly {
        /// Inodes with their absolute paths: (inode, abs_path)
        tracks: Vec<(i64, PathBuf)>,
    },

    /// Acknowledge inode change - update audio_info and files table, clear InodeChanged signal.
    ///
    /// Used when a file was replaced (same path, different inode). Updates the stored
    /// inode to match disk and refreshes file entry. Tag differences are handled separately
    /// through the OOB tag resolution flow.
    AcknowledgeInodeChanged {
        /// Inodes with their absolute paths: (inode, abs_path)
        tracks: Vec<(i64, PathBuf)>,
    },

    /// Apply DB tags to disk file (defer to db / reject disk changes).
    ///
    /// - Reads tags from database (source of truth)
    /// - Writes to disk via write_file_tags()
    /// - Updates file mtime after write
    /// - Clears needs_disk_flush flag
    /// - Clears OOB signals
    ///
    /// Used for:
    /// - OOB sync resolution (reject disk changes)
    /// - Spawned from ApplyTagOps (incremental tag edit step 2)
    ApplyDbTagsToDisk {
        inode: i64,
        path: PathBuf,
    },

    /// Assimilate disk tags into DB (defer to corpus / accept disk changes).
    ///
    /// - Reads tags from disk file
    /// - Writes to database, overwriting DB values
    /// - Updates file mtime to match disk
    /// - Clears OOB signals
    ///
    /// Used for OOB sync resolution (accept disk changes).
    AssimilateDiskTagsToDb {
        inode: i64,
        path: PathBuf,
    },

    // ========================================================================
    // Signal Emission Operations
    // ========================================================================
    /// Emit a CanonicalTag signal to whitelist a tag value.
    ///
    /// Used when operator confirms a compound-looking value is actually
    /// a single canonical entity (e.g., "Rinse & Repeat" is a band name,
    /// not a collaboration). The signal prevents future detection as
    /// a compound value.
    EmitCanonicalTag {
        tag_name: String,
        canonical_value: String,
    },

    // Note: VerifyTags has been moved to corpus::computations::Computation.
    // Computations don't alter state - they only emit signals.
}

impl Mutation {
    /// Human-readable label for this mutation (for logging/display).
    pub fn label(&self) -> &'static str {
        match self {
            Mutation::ApplyTagOps { .. } => "Tag edit",
            Mutation::ApplyDbTagsToDisk { .. } => "Tag sync (DB→disk)",
            Mutation::AssimilateDiskTagsToDb { .. } => "Tag sync (disk→DB)",
            Mutation::IndexFileFromPath { .. } => "Indexing",
            Mutation::UpdateFilePath { .. } => "Path update",
            Mutation::DropFromIndex { .. } => "Drop from index",
            Mutation::DropDirectoryFromIndex { .. } => "Drop directory from index",
            Mutation::AcknowledgeMtimeOnly { .. } => "Acknowledge mtime",
            Mutation::AcknowledgeInodeChanged { .. } => "Acknowledge inode",
            Mutation::Move { .. } | Mutation::MoveToStash { .. } => "File move",
            Mutation::HardLink { .. } => "Hard link",
            Mutation::LibraryMove { .. } => "Library move",
            Mutation::DbMigration { .. } => "Migration",
            Mutation::Transcode { .. } => "Transcode",
            Mutation::EmitCanonicalTag { .. } => "Mark canonical",
        }
    }

    /// Check if this mutation is database-only (no file system operations).
    #[cfg(test)]
    pub fn is_db_only(&self) -> bool {
        matches!(
            self,
            Mutation::ApplyTagOps { .. }
                | Mutation::DbMigration { .. }
                | Mutation::UpdateFilePath { .. }
                | Mutation::DropFromIndex { .. }
                | Mutation::AcknowledgeMtimeOnly { .. }
                | Mutation::AssimilateDiskTagsToDb { .. }
                | Mutation::EmitCanonicalTag { .. }
            // Note: ApplyDbTagsToDisk writes to disk, so NOT db-only
        )
    }

    /// Check if this mutation requires serial execution (cannot be parallelized).
    #[cfg(test)]
    pub fn requires_serial(&self) -> bool {
        matches!(self, Mutation::DbMigration { .. })
    }

    /// Get the inode affected by this mutation, if any.
    ///
    /// Used to trigger health signal refresh after mutations.
    #[cfg(test)]
    pub fn affected_inode(&self) -> Option<i64> {
        match self {
            Mutation::Transcode { inode, .. }
            | Mutation::ApplyDbTagsToDisk { inode, .. }
            | Mutation::AssimilateDiskTagsToDb { inode, .. } => Some(*inode),

            // These don't have a single inode directly (batch operations or no inode)
            Mutation::ApplyTagOps { .. }
            | Mutation::IndexFileFromPath { .. }
            | Mutation::UpdateFilePath { .. }
            | Mutation::DropFromIndex { .. }
            | Mutation::Move { .. }
            | Mutation::MoveToStash { .. }
            | Mutation::HardLink { .. }
            | Mutation::LibraryMove { .. }
            | Mutation::DbMigration { .. }
            | Mutation::AcknowledgeMtimeOnly { .. }
            | Mutation::AcknowledgeInodeChanged { .. }
            | Mutation::DropDirectoryFromIndex { .. }
            | Mutation::EmitCanonicalTag { .. } => None,
        }
    }

    /// Get directories affected by this mutation for signal recomputation.
    ///
    /// After a mutation completes, signals in these directories may need
    /// to be recomputed (e.g., UnindexedFile → HealthyFile after indexing).
    #[cfg(test)]
    pub fn affected_directories(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        match self {
            // DB-only tag operations don't change file presence
            Mutation::ApplyTagOps { .. } => {}

            // Single-track tag sync operations affect the file's directory
            Mutation::ApplyDbTagsToDisk { path, .. }
            | Mutation::AssimilateDiskTagsToDb { path, .. } => {
                if let Some(parent) = path.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }

            // Indexing operations affect the file's directory
            Mutation::IndexFileFromPath { path, .. } => {
                if let Some(parent) = path.parent() {
                    dirs.push(parent.to_path_buf());
                }
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
            Mutation::MoveToStash { path, .. } => {
                if let Some(parent) = path.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }

            // Deployment operations happen outside corpus, don't affect corpus signals
            Mutation::HardLink { .. } | Mutation::LibraryMove { .. } => {}

            // UpdateFilePath only has new_path (old path not tracked)
            Mutation::UpdateFilePath { new_path, .. } => {
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

            // Migration doesn't affect signals
            Mutation::DbMigration { .. } => {}

            // Batch OOB resolution: paths resolved at execution time, executors spawn follow-ups directly
            Mutation::AcknowledgeMtimeOnly { .. }
            | Mutation::AcknowledgeInodeChanged { .. } => {}

            // Directory drops: the directory itself is affected
            Mutation::DropDirectoryFromIndex { directory_path, .. } => {
                dirs.push(directory_path.clone());
            }

            // Signal emission: DB-only, no directories affected
            Mutation::EmitCanonicalTag { .. } => {}
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
            Mutation::IndexFileFromPath { path, .. } => vec![path.clone()],

            // Tag operations: single-track mutations with path
            Mutation::ApplyDbTagsToDisk { path, .. }
            | Mutation::AssimilateDiskTagsToDb { path, .. } => vec![path.clone()],

            // File operations: source and destination
            Mutation::Move { source, destination, .. }
            | Mutation::HardLink { source, destination }
            | Mutation::LibraryMove { source, destination } => {
                vec![source.clone(), destination.clone()]
            }

            Mutation::MoveToStash { path, .. } => {
                vec![path.clone()]
            }

            // Drop: the path being dropped
            Mutation::DropFromIndex { path, .. } => vec![path.clone()],

            // Directory drop: the directory path
            Mutation::DropDirectoryFromIndex { directory_path } => vec![directory_path.clone()],

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

            // Path update (for moved files) - return new path for signal clearing
            Mutation::UpdateFilePath { new_path, .. } => vec![new_path.clone()],

            // Operations without specific file paths that need signal updates
            Mutation::ApplyTagOps { .. }
            | Mutation::DbMigration { .. }
            | Mutation::EmitCanonicalTag { .. } => Vec::new(),
        }
    }

    // ========================================================================
    // Post-Execution Behavior Methods (Exhaustive Matches)
    // ========================================================================
    //
    // These methods define post-execution behavior for each mutation variant.
    // **Compile-time guarantee**: Adding a new variant forces you to specify
    // its behavior in each function (exhaustive match, no `_ =>` fallthrough).

    /// Signal clearing scope. Exhaustive: adding a variant requires specifying its scope.
    ///
    /// Determines whether to clear all signals (file is gone/replaced), only mutable
    /// signals (file modified but exists), or no signals (DB-only operation).
    pub fn signal_clear_scope(&self) -> SignalClearScope {
        match self {
            // Clear ALL signals (file is gone or replaced entirely)
            Mutation::MoveToStash { .. }
            | Mutation::DropFromIndex { .. }
            | Mutation::DropDirectoryFromIndex { .. }
            | Mutation::Transcode { .. } => SignalClearScope::All,

            // Clear mutable signals only (preserve CorruptFile, ShitFormat)
            Mutation::IndexFileFromPath { .. }
            | Mutation::Move { .. }
            | Mutation::HardLink { .. }
            | Mutation::LibraryMove { .. }
            | Mutation::ApplyDbTagsToDisk { .. }
            | Mutation::AssimilateDiskTagsToDb { .. }
            | Mutation::AcknowledgeMtimeOnly { .. }
            | Mutation::AcknowledgeInodeChanged { .. }
            | Mutation::UpdateFilePath { .. } => SignalClearScope::MutableOnly,

            // No signal clearing (DB-only or no file impact)
            Mutation::ApplyTagOps { .. }
            | Mutation::DbMigration { .. }
            | Mutation::EmitCanonicalTag { .. } => SignalClearScope::None,
        }
    }

    /// Paths to spawn signal update computations for.
    ///
    /// May differ from `affected_paths()` (e.g., Transcode only spawns for new path
    /// because source is stashed and would race with MissingFile emission).
    pub fn paths_for_signal_updates(&self) -> Vec<PathBuf> {
        match self {
            // Transcode: only spawn for NEW path (source is stashed, would race)
            Mutation::Transcode { source_path, target_format, .. } => {
                vec![source_path.with_extension(target_format.extension())]
            }

            // Most mutations: use affected_paths equivalent
            Mutation::IndexFileFromPath { path, .. }
            | Mutation::ApplyDbTagsToDisk { path, .. }
            | Mutation::AssimilateDiskTagsToDb { path, .. } => vec![path.clone()],

            Mutation::Move { source, destination, .. }
            | Mutation::HardLink { source, destination }
            | Mutation::LibraryMove { source, destination } => {
                vec![source.clone(), destination.clone()]
            }

            Mutation::AcknowledgeMtimeOnly { tracks }
            | Mutation::AcknowledgeInodeChanged { tracks } => {
                tracks.iter().map(|(_, path)| path.clone()).collect()
            }

            // No signal updates needed (file is gone or DB-only)
            // MoveToStash/DropFromIndex: file removed, signal updates would race with index drop
            Mutation::ApplyTagOps { .. }
            | Mutation::DbMigration { .. }
            | Mutation::UpdateFilePath { .. }
            | Mutation::MoveToStash { .. }
            | Mutation::DropFromIndex { .. }
            | Mutation::DropDirectoryFromIndex { .. }
            | Mutation::EmitCanonicalTag { .. } => Vec::new(),
        }
    }

    /// Additional computations to spawn (beyond path-based signal updates).
    ///
    /// Exhaustive match - every variant must be handled.
    pub fn additional_computations(&self) -> Vec<Computation> {
        match self {
            Mutation::HardLink { source, destination } => vec![
                Computation::Awakening(awakening::Computation::UpdateDeploySignals {
                    corpus_path: source.clone(),
                    library_path: destination.clone(),
                })
            ],

            // Explicit: all other variants spawn no additional computations
            Mutation::IndexFileFromPath { .. }
            | Mutation::MoveToStash { .. }
            | Mutation::DropFromIndex { .. }
            | Mutation::DropDirectoryFromIndex { .. }
            | Mutation::Transcode { .. }
            | Mutation::Move { .. }
            | Mutation::LibraryMove { .. }
            | Mutation::ApplyTagOps { .. }
            | Mutation::ApplyDbTagsToDisk { .. }
            | Mutation::AssimilateDiskTagsToDb { .. }
            | Mutation::AcknowledgeMtimeOnly { .. }
            | Mutation::AcknowledgeInodeChanged { .. }
            | Mutation::DbMigration { .. }
            | Mutation::UpdateFilePath { .. }
            | Mutation::EmitCanonicalTag { .. } => Vec::new(),
        }
    }

    /// Specific signals to clear by type+key (beyond path-based clearing).
    ///
    /// Exhaustive match - every variant must be handled.
    /// Used for targeted clearing like LibraryStale after moves, aggregate tag
    /// signals after tag edits, etc.
    pub fn specific_signals_to_clear(&self) -> Vec<SignalToClear> {
        match self {
            Mutation::LibraryMove { source, .. } => vec![
                SignalToClear {
                    signal_type: LibraryFileSignalType::LibraryStale.into(),
                    key_pattern: source.to_string_lossy().to_string(),
                }
            ],

            // TODO: Tag mutations may need to clear aggregate tag signals here
            // e.g., ApplyTagOps could clear MissingTag signals for affected tag types

            // Explicit: all other variants clear no specific signals
            Mutation::IndexFileFromPath { .. }
            | Mutation::MoveToStash { .. }
            | Mutation::DropFromIndex { .. }
            | Mutation::DropDirectoryFromIndex { .. }
            | Mutation::Transcode { .. }
            | Mutation::Move { .. }
            | Mutation::HardLink { .. }
            | Mutation::ApplyTagOps { .. }
            | Mutation::ApplyDbTagsToDisk { .. }
            | Mutation::AssimilateDiskTagsToDb { .. }
            | Mutation::AcknowledgeMtimeOnly { .. }
            | Mutation::AcknowledgeInodeChanged { .. }
            | Mutation::DbMigration { .. }
            | Mutation::UpdateFilePath { .. }
            | Mutation::EmitCanonicalTag { .. } => Vec::new(),
        }
    }
}

/// Result of executing a single mutation.
#[derive(Debug)]
pub struct MutationResult {
    pub _mutation: Mutation,
    pub success: bool,
    pub error: Option<String>,
    pub _duration_ms: u64,
    /// Follow-up mutations to queue (from spawn chaining).
    /// E.g., ApplyTagOps spawns ApplyDbTagsToDisk after DB write succeeds.
    pub spawn_mutations: Vec<crate::witch::SpawnedMutation>,
    /// Signals to emit post-execution.
    /// Avoids race with async DB writes by carrying signal data from execution time.
    pub pending_signals: Vec<PendingSignal>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mutation_labels() {
        let apply_tag_ops = Mutation::ApplyTagOps {
            ops: vec![TagOp::add_tag(1, "artist", "New")],
        };
        assert_eq!(apply_tag_ops.label(), "Tag edit");
        assert!(apply_tag_ops.is_db_only());

        let apply_tags = Mutation::ApplyDbTagsToDisk {
            inode: 1,
            path: PathBuf::from("/test/file.flac"),
        };
        assert_eq!(apply_tags.label(), "Tag sync (DB→disk)");
        assert!(!apply_tags.is_db_only());

        let file_move = Mutation::Move {
            source: PathBuf::from("/a"),
            destination: PathBuf::from("/b"),
        };
        assert_eq!(file_move.label(), "File move");

        let migration = Mutation::DbMigration {
            migration_id: 3,
            description: "test".to_string(),
        };
        assert_eq!(migration.label(), "Migration");
        assert!(migration.is_db_only());
        assert!(migration.requires_serial());
    }

}
