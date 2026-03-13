//! Core mutation data types.
//!
//! All corpus-mutating operations are represented as variants of the `Mutation` enum.
//! This provides a unified, serializable representation of changes.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::tags::{PictureInfo, TagSet};

use super::config_edit::ApplyConfigEditsMutation;
use super::dir_config_edit::{ApplyBatchDirConfigEditsMutation, ApplyDirConfigEditMutation};
use super::file_ops::{
    HardLinkMutation, InboxDirToCorpusMutation, InboxToCorpusMutation, LibraryMoveMutation,
    MoveMutation, StashFromZoneMutation, StashLeftoversMutation,
};
use super::indexing::{
    AcknowledgeMtimeOnlyMutation, ApplyDbTagsToDiskMutation, AssimilateDiskTagsToDbMutation,
    DropDirectoryFromIndexMutation, DropExternalMatchMutation, DropFromIndexMutation,
    EmitCanonicalTagMutation, EmitExpectedDuplicateMutation, EmitExpectedMissingTagMutation,
    EmitExpectedOverlapMutation, FlushTagsToDiskMutation, IndexFileFromPathMutation,
    UpdateFilePathMutation,
};
use super::jettison::JettisonEditHistoryMutation;
use super::tag_edit::ApplyTagOpsMutation;
use super::transcode::TranscodeMutation;

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
    pub fn new(
        label: impl Into<String>,
        old_value: impl ToString,
        new_value: impl ToString,
    ) -> Self {
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
// Post-Execution Behavior Types
// ============================================================================

/// Signal clearing scope after successful mutation.
///
/// Determines which signals are cleared for affected paths after a mutation executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalClearScope {
    /// Clear only mutable signals (preserve CorruptFile, ShitFormat).
    MutableOnly,
    /// Clear ALL signals including file-inherent ones.
    All,
    /// No signal clearing needed.
    None,
}

// ============================================================================
// Execution Staging
// ============================================================================

/// The execution phase a mutation belongs to within a staged transaction.
///
/// When a transaction is confirmed, mutations are bucketed by stage and
/// executed in order with drain barriers between each phase.
///
/// `Ord` derives from declaration order, giving natural phase ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MutationExecutionStage {
    /// Phase 0: app-level config writes
    Config,
    /// Phase 1: database record mutations
    DB,
    /// Phase 2: flush state to filesystem
    DiskFlush,
    /// Phase 3: arrange library copies
    DiskDeploy,
}

/// How a mutation is scheduled for execution.
///
/// DEPRECATED: Use `MutationOrigin` + `MutationExecutionStage` separately.
/// Kept temporarily for re-export compatibility during transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationStaging {
    /// Directly queueable in a transaction phase.
    Staged(MutationExecutionStage),
    /// Only spawned by parent mutations during execution (never in transactions).
    ChainEmitted,
}

/// Can this mutation appear in operator-staged transactions?
///
/// Orthogonal to `MutationExecutionStage` — every mutation has an execution
/// stage regardless of origin. Origin determines whether it can be directly
/// queued in a transaction or is only spawned during execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationOrigin {
    /// Directly queueable in a transaction.
    Staged,
    /// Only spawned during execution of another mutation.
    ChainEmitted,
}

// ============================================================================
// Extracted Metadata
// ============================================================================

/// Extracted metadata from an audio file, ready for indexing.
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
    pub fn get_tag(&self, name: &str) -> Option<&str> {
        self.tags.values_for(name).next()
    }
}

// ============================================================================
// Mutation Enum
// ============================================================================

/// Single atomic mutation operation.
///
/// Each variant represents one logical change to the corpus. Mutations are:
/// - Serializable for persistence/logging
/// - Grouped by file path for efficient execution
/// - Categorized for batching same-type operations
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Mutation {
    /// Apply a set of incremental tag operations to tracks.
    ApplyTagOps(ApplyTagOpsMutation),
    /// Index a file from path only - extracts metadata during execution.
    IndexFileFromPath(IndexFileFromPathMutation),
    /// Move a file from source to destination.
    Move(MoveMutation),
    /// Stash a corpus or inbox file (operator-driven eviction).
    StashFromZone(StashFromZoneMutation),
    /// Stash orphaned library files during deploy cleanup.
    StashLeftovers(StashLeftoversMutation),
    /// Transcode a file to a different container/codec format.
    Transcode(TranscodeMutation),
    /// Create a hard link from source to destination.
    HardLink(HardLinkMutation),
    /// Move a file within a library.
    LibraryMove(LibraryMoveMutation),
    /// Move an inbox file into the corpus.
    InboxToCorpus(InboxToCorpusMutation),
    /// Move an entire inbox directory into the corpus.
    InboxDirToCorpus(InboxDirToCorpusMutation),
    /// Update file path in files table.
    UpdateFilePath(UpdateFilePathMutation),
    /// Drop file from index.
    DropFromIndex(DropFromIndexMutation),
    /// Drop a directory and all its contents from the index.
    DropDirectoryFromIndex(DropDirectoryFromIndexMutation),
    /// Acknowledge mtime-only change.
    AcknowledgeMtimeOnly(AcknowledgeMtimeOnlyMutation),
    /// Apply DB tags to disk file.
    ApplyDbTagsToDisk(ApplyDbTagsToDiskMutation),
    /// Flush carried tags to disk.
    FlushTagsToDisk(FlushTagsToDiskMutation),
    /// Assimilate disk tags into DB.
    AssimilateDiskTagsToDb(AssimilateDiskTagsToDbMutation),
    /// Emit a CanonicalTag signal.
    EmitCanonicalTag(EmitCanonicalTagMutation),
    /// Mark a source pair overlap as expected.
    EmitExpectedOverlap(EmitExpectedOverlapMutation),
    /// Mark a fingerprint overlap group as expected.
    EmitExpectedDuplicate(EmitExpectedDuplicateMutation),
    /// Mark inodes as expected-missing-tag.
    EmitExpectedMissingTag(EmitExpectedMissingTagMutation),
    /// Drop external match data for an inode.
    DropExternalMatch(DropExternalMatchMutation),
    /// Apply edited config to disk.
    ApplyConfigEdits(Box<ApplyConfigEditsMutation>),
    /// Apply a source directory config edit.
    ApplyDirConfigEdit(Box<ApplyDirConfigEditMutation>),
    /// Batch-apply multiple source directory config edits atomically.
    ApplyBatchDirConfigEdits(Box<ApplyBatchDirConfigEditsMutation>),
    /// Jettison (delete) tag edit history from the database.
    JettisonEditHistory(JettisonEditHistoryMutation),
}

/// Fieldless mirror of `Mutation` for compile-time-enforced mapping tables.
///
/// Used by `DecisionKey::allowed_mutation_kinds()` to declare which mutation
/// types are valid for each decision type. The exhaustive match in
/// `Mutation::kind()` ensures the compiler forces an update when variants
/// are added to either enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MutationKind {
    ApplyTagOps,
    IndexFileFromPath,
    Move,
    StashFromZone,
    StashLeftovers,
    Transcode,
    HardLink,
    LibraryMove,
    InboxToCorpus,
    InboxDirToCorpus,
    UpdateFilePath,
    DropFromIndex,
    DropDirectoryFromIndex,
    AcknowledgeMtimeOnly,
    ApplyDbTagsToDisk,
    FlushTagsToDisk,
    AssimilateDiskTagsToDb,
    EmitCanonicalTag,
    EmitExpectedOverlap,
    EmitExpectedDuplicate,
    EmitExpectedMissingTag,
    DropExternalMatch,
    ApplyConfigEdits,
    ApplyDirConfigEdit,
    ApplyBatchDirConfigEdits,
    JettisonEditHistory,
}

impl Mutation {
    /// Fieldless discriminant for this mutation.
    pub fn kind(&self) -> MutationKind {
        match self {
            Mutation::ApplyTagOps(_) => MutationKind::ApplyTagOps,
            Mutation::IndexFileFromPath(_) => MutationKind::IndexFileFromPath,
            Mutation::Move(_) => MutationKind::Move,
            Mutation::StashFromZone(_) => MutationKind::StashFromZone,
            Mutation::StashLeftovers(_) => MutationKind::StashLeftovers,
            Mutation::Transcode(_) => MutationKind::Transcode,
            Mutation::HardLink(_) => MutationKind::HardLink,
            Mutation::LibraryMove(_) => MutationKind::LibraryMove,
            Mutation::InboxToCorpus(_) => MutationKind::InboxToCorpus,
            Mutation::InboxDirToCorpus(_) => MutationKind::InboxDirToCorpus,
            Mutation::UpdateFilePath(_) => MutationKind::UpdateFilePath,
            Mutation::DropFromIndex(_) => MutationKind::DropFromIndex,
            Mutation::DropDirectoryFromIndex(_) => MutationKind::DropDirectoryFromIndex,
            Mutation::AcknowledgeMtimeOnly(_) => MutationKind::AcknowledgeMtimeOnly,
            Mutation::ApplyDbTagsToDisk(_) => MutationKind::ApplyDbTagsToDisk,
            Mutation::FlushTagsToDisk(_) => MutationKind::FlushTagsToDisk,
            Mutation::AssimilateDiskTagsToDb(_) => MutationKind::AssimilateDiskTagsToDb,
            Mutation::EmitCanonicalTag(_) => MutationKind::EmitCanonicalTag,
            Mutation::EmitExpectedOverlap(_) => MutationKind::EmitExpectedOverlap,
            Mutation::EmitExpectedDuplicate(_) => MutationKind::EmitExpectedDuplicate,
            Mutation::EmitExpectedMissingTag(_) => MutationKind::EmitExpectedMissingTag,
            Mutation::DropExternalMatch(_) => MutationKind::DropExternalMatch,
            Mutation::ApplyConfigEdits(_) => MutationKind::ApplyConfigEdits,
            Mutation::ApplyDirConfigEdit(_) => MutationKind::ApplyDirConfigEdit,
            Mutation::ApplyBatchDirConfigEdits(_) => MutationKind::ApplyBatchDirConfigEdits,
            Mutation::JettisonEditHistory(_) => MutationKind::JettisonEditHistory,
        }
    }

    /// Human-readable label for this mutation (for logging/display).
    pub fn label(&self) -> &'static str {
        match self {
            Mutation::ApplyTagOps(_) => "Tag edit",
            Mutation::IndexFileFromPath(_) => "Indexing",
            Mutation::Move(_) => "File move",
            Mutation::StashFromZone(_) => "Stash from zone",
            Mutation::StashLeftovers(_) => "Stash leftovers",
            Mutation::Transcode(_) => "Transcode",
            Mutation::HardLink(_) => "Hard link",
            Mutation::LibraryMove(_) => "Library move",
            Mutation::InboxToCorpus(_) => "Inbox to corpus",
            Mutation::InboxDirToCorpus(_) => "Inbox dir to corpus",
            Mutation::UpdateFilePath(_) => "Update path",
            Mutation::DropFromIndex(_) => "Drop from index",
            Mutation::DropDirectoryFromIndex(_) => "Drop directory",
            Mutation::AcknowledgeMtimeOnly(_) => "Acknowledge mtime",
            Mutation::ApplyDbTagsToDisk(_) => "Tag sync (DB→disk)",
            Mutation::FlushTagsToDisk(_) => "Tag flush to disk",
            Mutation::AssimilateDiskTagsToDb(_) => "Tag sync (disk→DB)",
            Mutation::EmitCanonicalTag(_) => "Whitelist tag",
            Mutation::EmitExpectedOverlap(_) => "Expect overlap",
            Mutation::EmitExpectedDuplicate(_) => "Expect duplicate",
            Mutation::EmitExpectedMissingTag(_) => "Expect missing tag",
            Mutation::DropExternalMatch(_) => "Drop external match",
            Mutation::ApplyConfigEdits(_) => "Config update",
            Mutation::ApplyDirConfigEdit(_) => "Dir config update",
            Mutation::ApplyBatchDirConfigEdits(_) => "Dir config batch update",
            Mutation::JettisonEditHistory(_) => "Jettison edit history",
        }
    }

    /// Diff entries for transaction review display.
    ///
    /// Returns field-level diffs (label, old_value, new_value) for rendering
    /// red→green change visualization in the TUI.
    ///
    /// TODO: move per-struct diff_entries implementations from MutationExecutor
    /// impls in mm to inherent methods here.
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        Vec::new()
    }
}
