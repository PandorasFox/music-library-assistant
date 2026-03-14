//! Core types for the mutations system.
//!
//! Data types (TagOp, DiffEntry, Mutation enum, etc.) are re-exported from mm-meta.
//! Server-specific types (PendingSignal, SignalToClear, MutationResult) live here.

use std::path::PathBuf;

use crate::meta::computations::Computation;
use crate::meta::signals::registry::TypedSignalWrite;

// Re-export data types from mm-meta (struct re-exports come from sub-modules)
pub use mm_meta::mutations::{
    ExtractedMetadata, Mutation, MutationExecutionStage, MutationOrigin,
    SignalClearScope, TagOp,
};

// ============================================================================
// Server-only types
// ============================================================================

/// Signal to be emitted post-execution.
pub type PendingSignal = TypedSignalWrite;

/// Specific aggregate signal to clear by exact key.
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

/// Result of executing a single mutation.
#[derive(Debug)]
pub struct MutationResult {
    pub _mutation: Mutation,
    pub success: bool,
    pub error: Option<String>,
    pub _duration_ms: u64,
    /// Follow-up mutations to queue (from spawn chaining).
    pub spawn_mutations: Vec<crate::witch::SpawnedMutation>,
    /// Signals to emit post-execution.
    pub pending_signals: Vec<PendingSignal>,
    /// Inodes discovered during execution.
    pub discovered_inodes: Vec<i64>,
}

impl MutationResult {
    /// Construct from a `Result<(), _>` with timing.
    pub fn from_unit_result(
        mutation: Mutation,
        result: anyhow::Result<()>,
        start: std::time::Instant,
    ) -> Self {
        let (success, error) = match result {
            Ok(()) => (true, None),
            Err(e) => (false, Some(format!("{:#}", e))),
        };
        Self {
            _mutation: mutation,
            success,
            error,
            _duration_ms: start.elapsed().as_millis() as u64,
            spawn_mutations: Vec::new(),
            pending_signals: Vec::new(),
            discovered_inodes: Vec::new(),
        }
    }
}

// ============================================================================
// MutationDispatch - Extension trait for server-side execution dispatch
// ============================================================================

/// Extension trait adding execution dispatch to mm-meta's Mutation enum.
///
/// Since Mutation is defined in mm-meta but MutationExecutor is defined in mm,
/// we can't add inherent methods — the orphan rule prevents it. This trait
/// bridges the gap.
pub trait MutationDispatch {
    /// Get the inner struct as a trait object.
    fn as_executor(&self) -> &dyn super::traits::MutationExecutor;

    /// Paths to spawn signal update computations for.
    fn paths_for_signal_updates(&self) -> Vec<PathBuf>;

    /// Additional computations to spawn (beyond path-based signal updates).
    fn additional_computations(&self) -> Vec<Computation>;

    /// Specific signals to clear by type+key (beyond path-based clearing).
    fn specific_signals_to_clear(&self) -> Vec<SignalToClear>;
}

impl MutationDispatch for Mutation {
    fn as_executor(&self) -> &dyn super::traits::MutationExecutor {
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
            Mutation::DropExternalMatch(m) => m,
            Mutation::ApplyConfigEdits(m) => m.as_ref(),
            Mutation::ApplyDirConfigEdit(m) => m.as_ref(),
            Mutation::ApplyBatchDirConfigEdits(m) => m.as_ref(),
            Mutation::ExportEditHistory(m) => m,
            Mutation::ClearEditHistory(m) => m,
        }
    }

    fn paths_for_signal_updates(&self) -> Vec<PathBuf> {
        self.as_executor().paths_for_signal_updates()
    }

    fn additional_computations(&self) -> Vec<Computation> {
        self.as_executor().additional_computations()
    }

    fn specific_signals_to_clear(&self) -> Vec<SignalToClear> {
        self.as_executor().specific_signals_to_clear()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::mutations::file_ops::MoveMutation;
    use crate::meta::mutations::indexing::ApplyDbTagsToDiskMutation;
    use crate::meta::mutations::tag_edit::ApplyTagOpsMutation;
    use std::path::PathBuf;

    #[test]
    fn test_mutation_labels() {
        let apply_tag_ops = Mutation::ApplyTagOps(ApplyTagOpsMutation {
            ops: vec![TagOp::add_tag(1, "artist", "New")],
            zone: crate::db::types::Zone::Corpus,
        });
        assert_eq!(apply_tag_ops.label(), "Tag edit");

        let apply_tags = Mutation::ApplyDbTagsToDisk(ApplyDbTagsToDiskMutation {
            inode: 1,
            path: PathBuf::from("/test/file.flac"),
            zone: crate::db::types::Zone::Corpus,
        });
        assert_eq!(apply_tags.label(), "Tag sync (DB→disk)");

        let file_move = Mutation::Move(MoveMutation {
            source: PathBuf::from("/a"),
            destination: PathBuf::from("/b"),
        });
        assert_eq!(file_move.label(), "File move");
    }
}
