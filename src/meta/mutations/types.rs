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

use crate::meta::computations::Computation;
use crate::meta::signals::data::TypedSignalWrite;
use crate::corpus::tags::TagSet;

use super::file_ops::{MoveMutation, MoveToStashMutation, HardLinkMutation, LibraryMoveMutation};
use super::indexing::{
    IndexFileFromPathMutation, UpdateFilePathMutation, DropFromIndexMutation,
    DropDirectoryFromIndexMutation, AcknowledgeMtimeOnlyMutation, ApplyDbTagsToDiskMutation,
    FlushTagsToDiskMutation, AssimilateDiskTagsToDbMutation, EmitCanonicalTagMutation,
    EmitExpectedOverlapMutation, EmitExpectedDuplicateMutation,
};
use super::tag_edit::ApplyTagOpsMutation;
use super::transcode::TranscodeMutation;
use super::album_art::EmbedAlbumArtMutation;

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
/// Signal data carried from mutation execution time for post-execution emission.
///
/// Avoids race condition with async DB writes by carrying signal data from
/// execution time rather than querying DB after the write is sent.
pub type PendingSignal = TypedSignalWrite;

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

/// Specific aggregate signal to clear by exact key.
///
/// Used for targeted signal clearing like LibraryStale after library moves,
/// where the signal key doesn't match the mutation's affected inodes.
/// Function pointers are resolved at construction time via `SignalToClear::exact::<S>()`.
#[derive(Clone)]
pub struct SignalToClear {
    pub clear_by_key_fn: fn(&rusqlite::Connection, &str) -> rusqlite::Result<()>,
    pub key: String,
    pub label: &'static str,
}

impl SignalToClear {
    pub fn exact<S: crate::meta::signals::store::AggregateSignalStore>(key: String) -> Self {
        Self {
            clear_by_key_fn: S::clear_by_key,
            key,
            label: S::TABLE_NAME,
        }
    }
}

impl std::fmt::Debug for SignalToClear {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignalToClear")
            .field("label", &self.label)
            .field("key", &self.key)
            .finish()
    }
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
    /// Whether the file has embedded pictures (album art).
    pub has_pictures: bool,
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
    // Tag Operations (struct-backed — see tag_edit.rs for trait impl)
    // ========================================================================
    /// Apply a set of incremental tag operations to tracks.
    ApplyTagOps(ApplyTagOpsMutation),

    // ========================================================================
    // Indexing Operations (struct-backed — see indexing.rs for trait impls)
    // ========================================================================
    /// Index a file from path only - extracts metadata during execution.
    IndexFileFromPath(IndexFileFromPathMutation),

    // ========================================================================
    // File Operations (struct-backed — see file_ops.rs for trait impls)
    // ========================================================================
    /// Move a file from source to destination.
    Move(MoveMutation),

    /// Move a file to the stash directory.
    MoveToStash(MoveToStashMutation),

    // ========================================================================
    // Transcode Operations (struct-backed — see transcode.rs for trait impl)
    // ========================================================================
    /// Transcode a file to a different container/codec format.
    Transcode(TranscodeMutation),

    // ========================================================================
    // Deployment Operations (struct-backed — see file_ops.rs for trait impls)
    // ========================================================================
    /// Create a hard link from source to destination.
    HardLink(HardLinkMutation),

    /// Move a file within a library (e.g., stale file to correct location).
    LibraryMove(LibraryMoveMutation),

    // ========================================================================
    // Database Migration Operations
    // ========================================================================
    /// Apply a database schema migration.
    DbMigration {
        migration_id: u32,
        description: String,
    },

    // ========================================================================
    // Signal Resolution Operations (struct-backed — see indexing.rs for trait impls)
    // ========================================================================
    /// Update file path in files table (for relocated files).
    UpdateFilePath(UpdateFilePathMutation),

    /// Drop file from index (for missing files or orphaned signals).
    DropFromIndex(DropFromIndexMutation),

    /// Drop a directory and all its contents from the index.
    DropDirectoryFromIndex(DropDirectoryFromIndexMutation),

    // ========================================================================
    // OOB Resolution Operations (struct-backed — see indexing.rs for trait impls)
    // ========================================================================
    /// Acknowledge mtime-only change - update file mtime, clear MtimeOnlyMismatch signal.
    AcknowledgeMtimeOnly(AcknowledgeMtimeOnlyMutation),

    /// Apply DB tags to disk file (defer to db / reject disk changes).
    ApplyDbTagsToDisk(ApplyDbTagsToDiskMutation),

    /// Flush carried tags to disk (no DB read — avoids race with async DB writes).
    FlushTagsToDisk(FlushTagsToDiskMutation),

    /// Assimilate disk tags into DB (defer to corpus / accept disk changes).
    AssimilateDiskTagsToDb(AssimilateDiskTagsToDbMutation),

    // ========================================================================
    // Signal Emission Operations (struct-backed — see indexing.rs for trait impl)
    // ========================================================================
    /// Emit a CanonicalTag signal to whitelist a tag value.
    EmitCanonicalTag(EmitCanonicalTagMutation),

    /// Mark a source pair overlap as expected (suppress future CrossSourceOverlap signals).
    EmitExpectedOverlap(EmitExpectedOverlapMutation),

    /// Mark a fingerprint overlap group as expected (suppress future duplicate signals).
    EmitExpectedDuplicate(EmitExpectedDuplicateMutation),

    // ========================================================================
    // Album Art Operations (struct-backed — see album_art.rs for trait impl)
    // ========================================================================
    /// Embed a sidecar image into an audio file.
    EmbedAlbumArt(EmbedAlbumArtMutation),
}

impl Mutation {
    /// Get the inner struct as a trait object.
    ///
    /// Returns `None` for `DbMigration` which uses a separate execution path.
    /// All other variants return their inner `MutationExecutor` implementor.
    pub fn as_executor(&self) -> Option<&dyn super::traits::MutationExecutor> {
        match self {
            Mutation::ApplyTagOps(m) => Some(m),
            Mutation::IndexFileFromPath(m) => Some(m),
            Mutation::Move(m) => Some(m),
            Mutation::MoveToStash(m) => Some(m),
            Mutation::Transcode(m) => Some(m),
            Mutation::HardLink(m) => Some(m),
            Mutation::LibraryMove(m) => Some(m),
            Mutation::UpdateFilePath(m) => Some(m),
            Mutation::DropFromIndex(m) => Some(m),
            Mutation::DropDirectoryFromIndex(m) => Some(m),
            Mutation::AcknowledgeMtimeOnly(m) => Some(m),
            Mutation::ApplyDbTagsToDisk(m) => Some(m),
            Mutation::FlushTagsToDisk(m) => Some(m),
            Mutation::AssimilateDiskTagsToDb(m) => Some(m),
            Mutation::EmitCanonicalTag(m) => Some(m),
            Mutation::EmitExpectedOverlap(m) => Some(m),
            Mutation::EmitExpectedDuplicate(m) => Some(m),
            Mutation::EmbedAlbumArt(m) => Some(m),
            Mutation::DbMigration { .. } => None,
        }
    }

    /// Human-readable label for this mutation (for logging/display).
    pub fn label(&self) -> &'static str {
        match self.as_executor() {
            Some(e) => e.label(),
            None => "Migration", // DbMigration
        }
    }

    /// Check if this mutation is database-only (no file system operations).
    #[cfg(test)]
    pub fn is_db_only(&self) -> bool {
        matches!(
            self,
            Mutation::ApplyTagOps(_)
                | Mutation::DbMigration { .. }
                | Mutation::UpdateFilePath(_)
                | Mutation::DropFromIndex(_)
                | Mutation::AcknowledgeMtimeOnly(_)
                | Mutation::AssimilateDiskTagsToDb(_)
                | Mutation::EmitCanonicalTag(_)
                | Mutation::EmitExpectedOverlap(_)
                | Mutation::EmitExpectedDuplicate(_)
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
            Mutation::Transcode(ref m) => Some(m.inode),
            Mutation::ApplyDbTagsToDisk(ref m) => Some(m.inode),
            Mutation::FlushTagsToDisk(ref m) => Some(m.inode),
            Mutation::AssimilateDiskTagsToDb(ref m) => Some(m.inode),
            Mutation::EmbedAlbumArt(ref m) => Some(m.inode),

            // These don't have a single inode directly (batch operations or no inode)
            Mutation::ApplyTagOps(_)
            | Mutation::IndexFileFromPath(_)
            | Mutation::UpdateFilePath(_)
            | Mutation::DropFromIndex(_)
            | Mutation::Move(_)
            | Mutation::MoveToStash(_)
            | Mutation::HardLink(_)
            | Mutation::LibraryMove(_)
            | Mutation::DbMigration { .. }
            | Mutation::AcknowledgeMtimeOnly(_)
            | Mutation::DropDirectoryFromIndex(_)
            | Mutation::EmitCanonicalTag(_)
            | Mutation::EmitExpectedOverlap(_)
            | Mutation::EmitExpectedDuplicate(_) => None,
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
            Mutation::ApplyTagOps(_) => {}

            // Single-track tag sync operations affect the file's directory
            Mutation::ApplyDbTagsToDisk(m) => {
                if let Some(parent) = m.path.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }
            Mutation::FlushTagsToDisk(m) => {
                if let Some(parent) = m.path.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }
            Mutation::AssimilateDiskTagsToDb(m) => {
                if let Some(parent) = m.path.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }

            // Indexing operations affect the file's directory
            Mutation::IndexFileFromPath(m) => {
                if let Some(parent) = m.path.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }

            // File operations affect source and destination directories
            Mutation::Move(m) => {
                if let Some(parent) = m.source.parent() {
                    dirs.push(parent.to_path_buf());
                }
                if let Some(parent) = m.destination.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }
            Mutation::MoveToStash(m) => {
                if let Some(parent) = m.path.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }

            // Deployment operations happen outside corpus, don't affect corpus signals
            Mutation::HardLink(_) | Mutation::LibraryMove(_) => {}

            // UpdateFilePath only has new_path (old path not tracked)
            Mutation::UpdateFilePath(m) => {
                if let Some(parent) = m.new_path.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }

            // Index drops affect the file's directory
            Mutation::DropFromIndex(m) => {
                if let Some(parent) = m.path.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }

            // Transcode: affects the source file's directory (output is same dir, new extension)
            Mutation::Transcode(ref m) => {
                if let Some(parent) = m.source_path.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }

            // Migration doesn't affect signals
            Mutation::DbMigration { .. } => {}

            // Batch OOB resolution: paths resolved at execution time, executors spawn follow-ups directly
            Mutation::AcknowledgeMtimeOnly(_) => {}

            // Directory drops: the directory itself is affected
            Mutation::DropDirectoryFromIndex(m) => {
                dirs.push(m.directory_path.clone());
            }

            // Signal emission: DB-only, no directories affected
            Mutation::EmitCanonicalTag(_) => {}
            Mutation::EmitExpectedOverlap(_) => {}
            Mutation::EmitExpectedDuplicate(_) => {}

            // Album art embedding: affects the audio file's directory
            Mutation::EmbedAlbumArt(m) => {
                if let Some(parent) = m.audio_path.parent() {
                    dirs.push(parent.to_path_buf());
                }
            }
        }

        // Deduplicate directories
        dirs.sort();
        dirs.dedup();
        dirs
    }

    // ========================================================================
    // Post-Execution Behavior Methods (delegated to MutationExecutor trait)
    // ========================================================================

    /// Paths to spawn signal update computations for.
    pub fn paths_for_signal_updates(&self) -> Vec<PathBuf> {
        match self.as_executor() {
            Some(e) => e.paths_for_signal_updates(),
            None => Vec::new(), // DbMigration
        }
    }

    /// Additional computations to spawn (beyond path-based signal updates).
    pub fn additional_computations(&self) -> Vec<Computation> {
        match self.as_executor() {
            Some(e) => e.additional_computations(),
            None => Vec::new(), // DbMigration
        }
    }

    /// Specific signals to clear by type+key (beyond path-based clearing).
    pub fn specific_signals_to_clear(&self) -> Vec<SignalToClear> {
        match self.as_executor() {
            Some(e) => e.specific_signals_to_clear(),
            None => Vec::new(), // DbMigration
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
    /// Inodes discovered during execution (e.g., IndexFileFromPath discovers
    /// the inode during metadata extraction). Combined with the trait's
    /// `affected_inodes()` for post-execution signal clearing + dirty marking.
    pub discovered_inodes: Vec<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mutation_labels() {
        let apply_tag_ops = Mutation::ApplyTagOps(ApplyTagOpsMutation {
            ops: vec![TagOp::add_tag(1, "artist", "New")],
        });
        assert_eq!(apply_tag_ops.label(), "Tag edit");
        assert!(apply_tag_ops.is_db_only());

        let apply_tags = Mutation::ApplyDbTagsToDisk(ApplyDbTagsToDiskMutation {
            inode: 1,
            path: PathBuf::from("/test/file.flac"),
        });
        assert_eq!(apply_tags.label(), "Tag sync (DB→disk)");
        assert!(!apply_tags.is_db_only());

        let file_move = Mutation::Move(MoveMutation {
            source: PathBuf::from("/a"),
            destination: PathBuf::from("/b"),
        });
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
