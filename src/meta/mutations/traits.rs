//! Execution traits for mutations.
//!
//! Each mutation struct implements `MutationExecutor`, carrying its own
//! execution logic, label, signal clearing scope, and post-execution behavior.
//! This replaces the centralized match dispatch in `witch/execution.rs` and
//! the exhaustive match methods on the `Mutation` enum in `types.rs`.

use std::path::Path;

use crate::corpus::db::ReadOnlyDb;
use crate::meta::computations::Computation;
use crate::meta::recomputation::RecomputationScope;
use crate::witch::MutationExecutionWitness;

use super::types::{MutationResult, SignalClearScope, SignalToClear};

/// Context provided to mutation executors at execution time.
///
/// Contains everything a mutation needs to execute: read-only DB access,
/// execution witness, optional stash root, and session ID.
pub struct MutationContext<'a> {
    pub read_db: &'a ReadOnlyDb<'a>,
    pub witness: &'a MutationExecutionWitness,
    /// Stash root directory for file operations and transcode.
    // TODO: eventually replace with a single read-only ARC for the active
    // config blob (which will contain stash root etc) — will cleanly contain
    // "all config" that mutations might want access to, not just stash root.
    pub stash_root: Option<&'a Path>,
    pub session_id: &'a str,
}

/// Trait implemented by each mutation struct.
///
/// Defines execution logic and all post-execution behavior for a mutation.
/// Adding a new mutation = create struct + impl this trait (one file).
pub trait MutationExecutor: std::fmt::Debug + Send + Sync {
    /// Human-readable label for logging/display.
    fn label(&self) -> &'static str;

    /// Execute this mutation.
    fn execute(&self, ctx: &MutationContext) -> MutationResult;

    /// Signal clearing scope after successful execution.
    fn signal_clear_scope(&self) -> SignalClearScope;

    /// Inodes whose corpus signals should be cleared post-execution.
    ///
    /// For mutations that know their inode pre-execution (ApplyDbTagsToDisk,
    /// Transcode, etc.), return it here. For mutations where the inode is
    /// discovered during execution (IndexFileFromPath), return empty and
    /// carry the inode in `MutationResult.discovered_inodes` instead.
    fn affected_inodes(&self) -> Vec<i64>;

    /// Additional computations to spawn post-execution.
    ///
    /// Used for cross-domain side effects like HardLink -> UpdateDeploySignals.
    /// Signal recomputation for affected inodes is handled automatically by
    /// the dirty_inodes system, NOT by this method.
    fn additional_computations(&self) -> Vec<Computation> {
        Vec::new()
    }

    /// Specific aggregate signals to clear by type+key pattern.
    ///
    /// For targeted clearing like LibraryStale after library moves,
    /// where the signal uses a semantic key (not an inode).
    fn specific_signals_to_clear(&self) -> Vec<SignalToClear> {
        Vec::new()
    }

    /// Paths to spawn signal update computations for.
    ///
    /// Transitional: used during migration from path-based to inode-based
    /// post-execution pipeline. Returns paths that need UpdateCorpusFileSignals
    /// or UpdateLibraryFileSignals computations spawned.
    fn paths_for_signal_updates(&self) -> Vec<std::path::PathBuf> {
        Vec::new()
    }

    /// Diff entries for transaction review display.
    ///
    /// Returns field-level diffs (label, old_value, new_value) for rendering
    /// red→green change visualization. Default empty — mutations opt in.
    fn diff_entries(&self) -> Vec<super::types::DiffEntry> {
        Vec::new()
    }

    /// Which domains this mutation dirties, for selective content analysis.
    ///
    /// The Witch accumulates scopes from completed mutations and passes
    /// the result to `ScheduleContentAnalysis`, which only spawns
    /// computations touching the flagged domains.
    ///
    /// Mutations with `EMPTY` scope (e.g., AcknowledgeMtimeOnly, operational
    /// config changes) skip re-awakening entirely.
    ///
    /// Default: conservative — assumes all domains are dirty.
    fn recomputation_scope(&self) -> RecomputationScope {
        RecomputationScope::TAGS
            | RecomputationScope::FILES
            | RecomputationScope::DEPLOY
            | RecomputationScope::INBOX
    }
}
