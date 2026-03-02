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

use crate::meta::computations::Computation;
use crate::meta::signals::data::TypedSignalWrite;
use crate::corpus::tags::{TagSet, PictureInfo};

use super::file_ops::{MoveMutation, StashFromZoneMutation, StashLeftoversMutation, HardLinkMutation, LibraryMoveMutation, InboxToCorpusMutation, InboxDirToCorpusMutation};
use super::indexing::{
    IndexFileFromPathMutation, UpdateFilePathMutation, DropFromIndexMutation,
    DropDirectoryFromIndexMutation, AcknowledgeMtimeOnlyMutation, ApplyDbTagsToDiskMutation,
    FlushTagsToDiskMutation, AssimilateDiskTagsToDbMutation, EmitCanonicalTagMutation,
    EmitExpectedOverlapMutation, EmitExpectedDuplicateMutation,
    EmitExpectedMissingTagMutation,
};
use super::tag_edit::ApplyTagOpsMutation;
use super::transcode::TranscodeMutation;
use super::album_art::{AppendAlbumArtMutation, EmbedAlbumArtMutation, UpgradeAlbumArtMutation};
use super::config_edit::ApplyConfigEditsMutation;
use super::dir_config_edit::{ApplyDirConfigEditMutation, ApplyBatchDirConfigEditsMutation};

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
// Diff Display Types
// ============================================================================

/// A single field-level diff for display in transaction review.
///
/// Rendered as red (old_value) → green (new_value) with a label.
#[derive(Debug, Clone)]
pub struct DiffEntry {
    pub label: String,
    pub old_value: String,
    pub new_value: String,
}

impl DiffEntry {
    pub fn new(label: impl Into<String>, old_value: impl ToString, new_value: impl ToString) -> Self {
        Self {
            label: label.into(),
            old_value: old_value.to_string(),
            new_value: new_value.to_string(),
        }
    }
}

/// Extract filename from a path for use as a diff label.
pub fn path_filename(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
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
    /// Detailed picture metadata (format, resolution, count). None if no pictures.
    pub pic_info: Option<PictureInfo>,
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

    /// Stash a corpus or inbox file (operator-driven eviction).
    StashFromZone(StashFromZoneMutation),

    /// Stash orphaned library files during deploy cleanup.
    StashLeftovers(StashLeftoversMutation),

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

    /// Move an inbox file into the corpus (zone change + tag migration).
    InboxToCorpus(InboxToCorpusMutation),

    /// Move an entire inbox directory into the corpus (preserves non-audio content).
    InboxDirToCorpus(InboxDirToCorpusMutation),

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

    /// Mark inodes as expected-missing-tag (suppress future MissingAlbumSingle signals).
    EmitExpectedMissingTag(EmitExpectedMissingTagMutation),

    // ========================================================================
    // Album Art Operations (struct-backed — see album_art.rs for trait impl)
    // ========================================================================
    /// Embed a sidecar image into an audio file.
    EmbedAlbumArt(EmbedAlbumArtMutation),

    /// Replace existing embedded art with a better sidecar image.
    UpgradeAlbumArt(UpgradeAlbumArtMutation),

    /// Append a sidecar image alongside existing embedded art.
    AppendAlbumArt(AppendAlbumArtMutation),

    // ========================================================================
    // Config Operations (struct-backed — see config_edit.rs for trait impl)
    // ========================================================================
    /// Apply edited config to disk (comment-preserving KDL modification).
    ApplyConfigEdits(ApplyConfigEditsMutation),

    /// Apply a source directory config edit to dirs.kdl.
    ApplyDirConfigEdit(ApplyDirConfigEditMutation),

    /// Batch-apply multiple source directory config edits to dirs.kdl atomically.
    /// Produced by coalescing individual ApplyDirConfigEdit mutations at commit time.
    ApplyBatchDirConfigEdits(ApplyBatchDirConfigEditsMutation),
}

impl Mutation {
    /// Get the inner struct as a trait object.
    ///
    /// Every variant wraps an inner `MutationExecutor` implementor.
    pub fn as_executor(&self) -> &dyn super::traits::MutationExecutor {
        match self {
            Mutation::ApplyTagOps(m) => m,
            Mutation::IndexFileFromPath(m) => m,
            Mutation::Move(m) => m,
            Mutation::StashFromZone(m) => m,
            Mutation::StashLeftovers(m) => m,
            Mutation::Transcode(m) => m,
            Mutation::HardLink(m) => m,
            Mutation::LibraryMove(m) => m,
            Mutation::InboxToCorpus(m) => m,
            Mutation::InboxDirToCorpus(m) => m,
            Mutation::UpdateFilePath(m) => m,
            Mutation::DropFromIndex(m) => m,
            Mutation::DropDirectoryFromIndex(m) => m,
            Mutation::AcknowledgeMtimeOnly(m) => m,
            Mutation::ApplyDbTagsToDisk(m) => m,
            Mutation::FlushTagsToDisk(m) => m,
            Mutation::AssimilateDiskTagsToDb(m) => m,
            Mutation::EmitCanonicalTag(m) => m,
            Mutation::EmitExpectedOverlap(m) => m,
            Mutation::EmitExpectedDuplicate(m) => m,
            Mutation::EmitExpectedMissingTag(m) => m,
            Mutation::EmbedAlbumArt(m) => m,
            Mutation::UpgradeAlbumArt(m) => m,
            Mutation::AppendAlbumArt(m) => m,
            Mutation::ApplyConfigEdits(m) => m,
            Mutation::ApplyDirConfigEdit(m) => m,
            Mutation::ApplyBatchDirConfigEdits(m) => m,
        }
    }

    /// Human-readable label for this mutation (for logging/display).
    pub fn label(&self) -> &'static str {
        self.as_executor().label()
    }

    /// Check if this mutation is database-only (no file system operations).
    #[cfg(test)]
    pub fn is_db_only(&self) -> bool {
        matches!(
            self,
            Mutation::ApplyTagOps(_)
                | Mutation::UpdateFilePath(_)
                | Mutation::DropFromIndex(_)
                | Mutation::AcknowledgeMtimeOnly(_)
                | Mutation::AssimilateDiskTagsToDb(_)
                | Mutation::EmitCanonicalTag(_)
                | Mutation::EmitExpectedOverlap(_)
                | Mutation::EmitExpectedDuplicate(_)
                | Mutation::EmitExpectedMissingTag(_)
            // Note: ApplyDbTagsToDisk writes to disk, so NOT db-only
            // Note: InboxToCorpus moves files + updates DB, so NOT db-only
            // Note: ApplyBatchDirConfigEdits writes to disk, so NOT db-only
        )
    }

    // ========================================================================
    // Post-Execution Behavior Methods (delegated to MutationExecutor trait)
    // ========================================================================

    /// Paths to spawn signal update computations for.
    pub fn paths_for_signal_updates(&self) -> Vec<PathBuf> {
        self.as_executor().paths_for_signal_updates()
    }

    /// Additional computations to spawn (beyond path-based signal updates).
    pub fn additional_computations(&self) -> Vec<Computation> {
        self.as_executor().additional_computations()
    }

    /// Specific signals to clear by type+key (beyond path-based clearing).
    pub fn specific_signals_to_clear(&self) -> Vec<SignalToClear> {
        self.as_executor().specific_signals_to_clear()
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
            zone: crate::db::types::Zone::Corpus,
        });
        assert_eq!(apply_tag_ops.label(), "Tag edit");
        assert!(apply_tag_ops.is_db_only());

        let apply_tags = Mutation::ApplyDbTagsToDisk(ApplyDbTagsToDiskMutation {
            inode: 1,
            path: PathBuf::from("/test/file.flac"),
            zone: crate::db::types::Zone::Corpus,
        });
        assert_eq!(apply_tags.label(), "Tag sync (DB→disk)");
        assert!(!apply_tags.is_db_only());

        let file_move = Mutation::Move(MoveMutation {
            source: PathBuf::from("/a"),
            destination: PathBuf::from("/b"),
        });
        assert_eq!(file_move.label(), "File move");

    }

}
