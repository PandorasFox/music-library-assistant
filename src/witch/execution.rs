//! Task execution for mutations, computations, and maintenance tasks.
//!
//! This module is part of the Witch subsystem. See `witch/mod.rs` for overview.
//!
//! ## DB Access Patterns
//!
//! - **Mutations**: Use thread-local read-only connection (`with_read_only_db`).
//!   All writes go through `write_thread::signal_sender()`.
//! - **Computations**: Use thread-local read-only connection (same pattern).
//! - **Maintenance**: Both variants route through db_thread's write connection:
//!   - Migration: Uses `write_thread::execute_migration()` (schema changes on write connection).
//!   - Vacuum: Uses `write_thread::execute_vacuum()` (needs exclusive write connection).
//!
//! ## Post-Execution Pipeline
//!
//! After a mutation executes successfully, `apply_post_execution()` runs a 6-phase
//! pipeline. Each mutation struct implements `MutationExecutor` (in `meta/mutations/traits.rs`)
//! which defines its post-execution behavior:
//!
//! 1. **Signal clearing** - Clear corpus signals by inode (scope from `signal_clear_scope()`)
//! 1c. **Dirty inode marking** - Mark affected inodes dirty for per-inode computations (when scope includes TAGS)
//! 2. **File-inherent signals** - Emit CorruptFile/ShitFormat via pending_signals
//! 3. **Signal update spawning** - Spawn UpdateFileSignals (from `paths_for_signal_updates()`)
//! 4. **Additional computations** - Spawn extra computations (from `additional_computations()`)
//! 5. **Specific signal clearing** - Clear signals by type+key (from `specific_signals_to_clear()`)

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::time::Instant;

use crate::config;
use crate::meta::computations::{Computation, derivation, with_read_only_db};
use crate::meta::recomputation::RecomputationScope;
use crate::meta::signals::data::TypedSignalWrite;
use crate::meta::mutations::{Mutation, PendingSignal};
use crate::corpus::paths;
use crate::db::write_thread;

use crate::meta::maintenance::DbMaintenanceTask;

use super::types::{MutationExecutionWitness, Task, TaskResult};

// ============================================================================
// Task Execution
// ============================================================================

/// Execute a single task (mutation, computation, or maintenance). Opens DB connection as needed.
pub(super) fn execute_task(task: Task, label: String, queue_time: Instant) -> TaskResult {
    let queue_wait_ms = queue_time.elapsed().as_millis() as u64;

    match task {
        Task::Mutation(mutation) => execute_mutation(mutation, label, queue_wait_ms),
        Task::Computation(computation) => execute_computation(computation, label, queue_wait_ms),
        Task::Maintenance(task) => execute_maintenance(task, label, queue_wait_ms),
    }
}

/// Execute a single mutation using thread-local read-only DB connection.
///
/// All writes go through `write_thread::signal_sender()`. Read operations use
/// the same thread-local cached connection as computations.
pub(super) fn execute_mutation(mutation: Mutation, label: String, queue_wait_ms: u64) -> TaskResult {
    use crate::meta::mutations::traits::MutationContext;

    let start = Instant::now();

    crate::logging::log_mutation(format!(
        "[EXECUTION] execute_mutation START: {} (label={:?})",
        mutation.label(), label
    ));

    // Create execution witness - proves we're inside the Witch's execution context
    let witness = MutationExecutionWitness::new();

    // Load config for stash_root access (needed by file_ops and transcode)
    let loaded_config = config::load_config().ok();
    let stash_root = loaded_config.as_ref().map(|c| c.stash_dir());

    let session_id: &str = &label;

    // Execute mutation via MutationExecutor trait dispatch.
    // All writes go through write_thread::signal_sender() (fire-and-forget).
    let result = with_read_only_db(|read_db| {
        let executor = mutation.as_executor();
        let ctx = MutationContext {
            read_db,
            witness: &witness,
            stash_root: stash_root.as_deref(),
            session_id,
        };
        let r = executor.execute(&ctx);
        (r.success, r.error, r.spawn_mutations, r.pending_signals, r.discovered_inodes)
    });

    // Handle DB access failure
    let (success, error, spawn_mutations, pending_signals, discovered_inodes) = match result {
        Ok((s, e, sm, ps, di)) => (s, e, sm, ps, di),
        Err(db_err) => {
            crate::logging::log_error(format!(
                "[EXECUTION] DB access FAILED: {}",
                db_err
            ));
            return TaskResult {
                success: false,
                error: Some(format!("DB access failed: {}", db_err)),
                label,
                spawn: Vec::new(),
                spawn_mutations: Vec::new(),
                duration_ms: start.elapsed().as_millis() as u64,
                queue_wait_ms,
                thread_stats: None,
                config_update: None,
                recomputation_scope: RecomputationScope::EMPTY,
                observed_corpus_inodes: HashMap::new(),
                observed_inbox_inodes: HashMap::new(),
                observed_library_files: Vec::new(),
            };
        }
    };

    let duration_ms = start.elapsed().as_millis() as u64;

    crate::logging::log_mutation(format!(
        "[EXECUTION] execute_mutation END: success={}, error={:?}, duration={}ms",
        success, error, duration_ms
    ));
    // Mirror failed mutations to errors.log for central error diagnosis
    if !success {
        if let Some(ref err) = error {
            crate::logging::log_error(format!(
                "[EXECUTION] Mutation failed (label={:?}): {}", label, err
            ));
        }

        // Emit CorruptFile signal for failed IndexFileFromPath mutations.
        // These files failed to index (corrupt metadata/audio), so they should
        // be flagged for stashing rather than remaining as mere UnindexedFile signals.
        if let Mutation::IndexFileFromPath(ref m) = &mutation {
            let path = &m.path;
            if let Some(sender) = write_thread::signal_sender() {
                // Get inode from filesystem (file exists but failed to parse)
                if let Ok(metadata) = std::fs::metadata(path) {
                    let inode = metadata.ino() as i64;
                    let resolver = paths::get_resolver();
                    if let Some(rel) = resolver.to_relative(path) {
                        let rel_str = rel.to_string_lossy();
                        sender.write_typed_signal(
                            TypedSignalWrite::CorruptFile(
                                crate::meta::signals::data::CorruptFileSignal {
                                    inode,
                                    path: rel_str.to_string(),
                                },
                            ),
                            &witness,
                        );
                        crate::logging::log_general(format!(
                            "[EXECUTION] Emitted CorruptFile signal for failed indexing: {}",
                            rel_str
                        ));
                    }
                }
            }
        }
    }

    // Apply structured post-execution pipeline
    let spawn = apply_post_execution(&mutation, success, &pending_signals, &discovered_inodes, &witness);

    // Extract config update for config-modifying mutations.
    // Both ApplyConfigEdits and ApplyDirConfigEdit carry the new Config in-band.
    let config_update = if success {
        match &mutation {
            Mutation::ApplyConfigEdits(ref m) => Some(m.new_config.clone()),
            Mutation::ApplyDirConfigEdit(ref m) => Some(m.new_config.clone()),
            Mutation::ApplyBatchDirConfigEdits(ref m) => Some(m.new_config.clone()),
            _ => None,
        }
    } else {
        None
    };

    // Extract recomputation scope from executor (EMPTY on failure)
    let recomputation_scope = if success {
        mutation.as_executor().recomputation_scope()
    } else {
        RecomputationScope::EMPTY
    };

    TaskResult {
        success,
        error,
        label,
        spawn,
        spawn_mutations,
        duration_ms,
        queue_wait_ms,
        thread_stats: None, // Mutations don't use thread-local stats
        config_update,
        recomputation_scope,
        observed_corpus_inodes: HashMap::new(),
        observed_inbox_inodes: HashMap::new(),
        observed_library_files: Vec::new(),
    }
}

/// Execute a single computation. Uses thread-local DB connection.
pub(super) fn execute_computation(computation: Computation, label: String, queue_wait_ms: u64) -> TaskResult {
    use crate::meta::computations;

    let result = computations::execute_single(&computation);

    // Capture thread stats after execution
    let thread_stats = Some(computations::get_thread_stats());

    // Collect all spawned computations (already wrapped in unified Computation enum)
    let spawn = result.all_spawned();

    TaskResult {
        success: result.success,
        error: result.error,
        label,
        spawn,
        spawn_mutations: Vec::new(), // Computations don't spawn mutations
        duration_ms: result.duration_ms,
        queue_wait_ms,
        thread_stats,
        config_update: None,
        recomputation_scope: RecomputationScope::EMPTY,
        observed_corpus_inodes: result.observed_corpus_inodes,
        observed_inbox_inodes: result.observed_inbox_inodes,
        observed_library_files: result.observed_library_files,
    }
}

/// Execute a database maintenance task (migration or vacuum).
pub(super) fn execute_maintenance(task: DbMaintenanceTask, label: String, queue_wait_ms: u64) -> TaskResult {
    let start = Instant::now();

    let (success, error) = match task {
        DbMaintenanceTask::SchemaReconciliation => {
            crate::logging::log_mutation(format!(
                "[EXECUTION] execute_maintenance SchemaReconciliation START (label={:?})",
                label
            ));

            // Route reconciliation through db_thread which owns the write connection
            match write_thread::execute_reconciliation() {
                Ok(()) => (true, None),
                Err(e) => (false, Some(e)),
            }
        }

        DbMaintenanceTask::Vacuum => {
            crate::logging::log_general(format!(
                "[EXECUTION] execute_maintenance Vacuum START (label={:?})",
                label
            ));

            match crate::db::write_thread::execute_vacuum() {
                Ok(()) => (true, None),
                Err(e) => (false, Some(e)),
            }
        }
    };

    let duration_ms = start.elapsed().as_millis() as u64;

    crate::logging::log_general(format!(
        "[EXECUTION] execute_maintenance END: success={}, error={:?}, duration={}ms",
        success, error, duration_ms
    ));

    TaskResult {
        success,
        error,
        label,
        spawn: Vec::new(),
        spawn_mutations: Vec::new(),
        duration_ms,
        queue_wait_ms,
        thread_stats: None,
        config_update: None,
        recomputation_scope: RecomputationScope::EMPTY,
        observed_corpus_inodes: HashMap::new(),
        observed_inbox_inodes: HashMap::new(),
        observed_library_files: Vec::new(),
    }
}

// ============================================================================
// Post-Execution Pipeline
// ============================================================================

/// Apply post-execution hooks for a mutation.
///
/// This is a structured pipeline that replaces the ad-hoc conditionals
/// that previously handled signal clearing, emission, and computation spawning.
///
/// Each phase uses exhaustive match methods on `Mutation` to ensure compile-time
/// enforcement when new variants are added.
///
/// ## Phases
///
/// 1. **Inode signal clearing** - Clear corpus signals by inode based on `signal_clear_scope()`
/// 1c. **Dirty inode marking** - Mark affected inodes dirty for per-inode computations (TAGS scope)
/// 2. **File-inherent signals** - Emit CorruptFile/ShitFormat via pending_signals
/// 3. **Signal update spawning** - Spawn UpdateFileSignals for `paths_for_signal_updates()`
/// 4. **Additional computations** - Spawn extra computations from `additional_computations()`
/// 5. **Specific signal clearing** - Clear aggregate signals by type+key from `specific_signals_to_clear()`
fn apply_post_execution(
    mutation: &Mutation,
    success: bool,
    pending_signals: &[PendingSignal],
    discovered_inodes: &[i64],
    witness: &MutationExecutionWitness,
) -> Vec<Computation> {
    use crate::meta::mutations::SignalClearScope;

    if !success {
        return Vec::new();
    }

    let resolver = paths::get_resolver();
    let mut spawned = Vec::new();

    // Phase 1: Inode-based corpus signal clearing
    // Combine pre-known inodes (from trait) with inodes discovered at execution time.
    // Signal clearing scope determines which signals are cleared:
    //   - All: clear everything (file gone/replaced)
    //   - MutableOnly: preserve CorruptFile/ShitFormat (file still exists)
    //   - None: skip clearing (DB-only operations)
    {
        let executor = mutation.as_executor();
        let scope = executor.signal_clear_scope();
        if scope != SignalClearScope::None {
            let pre_known = executor.affected_inodes();
            let all_inodes: Vec<i64> = pre_known
                .into_iter()
                .chain(discovered_inodes.iter().copied())
                .collect();

            if !all_inodes.is_empty() {
                if let Some(sender) = write_thread::signal_sender() {
                    for inode in &all_inodes {
                        match scope {
                            SignalClearScope::All => {
                                sender.clear_all_corpus_signals(*inode, witness);
                            }
                            SignalClearScope::MutableOnly => {
                                sender.clear_mutable_corpus_signals(*inode, witness);
                            }
                            SignalClearScope::None => unreachable!(),
                        }
                    }
                }
            }
        }
    }

    // Phase 1c: Mark affected inodes dirty for per-inode computations
    // Uses the same inode collection as Phase 1 signal clearing. Only runs when
    // the mutation's recomputation scope includes TAGS — dirty inodes exist for
    // tag-dependent computations only.
    {
        let executor = mutation.as_executor();
        let scope = executor.recomputation_scope();
        if scope.contains(RecomputationScope::TAGS) {
            let pre_known = executor.affected_inodes();
            let all_inodes: Vec<i64> = pre_known
                .into_iter()
                .chain(discovered_inodes.iter().copied())
                .collect();

            if !all_inodes.is_empty() {
                if let Some(sender) = write_thread::signal_sender() {
                    for computation_type in crate::meta::computations::PER_INODE_COMPUTATIONS {
                        sender.mark_dirty_inodes(all_inodes.clone(), computation_type, witness);
                    }
                }
            }
        }
    }

    // Phase 1b: Drop files table entry for stash mutations
    // When stashing a file, we must also remove it from the files table (not just signals).
    // Otherwise DeriveDirectorySignals will emit MissingFile for the stashed path.
    let stash_path: Option<&std::path::Path> = match mutation {
        Mutation::StashFromZone(ref m) => Some(&m.path),
        Mutation::StashLeftovers(ref m) => Some(&m.path),
        _ => None,
    };
    if let Some(path) = stash_path {
        if let Some(sender) = write_thread::signal_sender() {
            let rel_path = if path.is_absolute() {
                resolver.to_relative(path)
            } else {
                Some(path.to_path_buf())
            };
            if let Some(rel) = rel_path {
                sender.drop_from_index(&rel.to_string_lossy(), witness);
            }
        }
    }

    // Phase 2: File-inherent signal emission (CorruptFile, ShitFormat)
    // For IndexFileFromPath, use pending_signals (avoids race with async DB writes).
    // For other mutations, use the checks_* methods.
    emit_file_inherent_signals(mutation, pending_signals, witness);

    // Phase 3: Spawn signal update computations
    // Path-aware: corpus paths → UpdateCorpusFileSignals, library paths → UpdateLibraryFileSignals
    for path in mutation.paths_for_signal_updates() {
        let rel = if path.is_absolute() {
            match resolver.to_relative(&path) {
                Some(r) => r,
                None => continue,
            }
        } else {
            path
        };
        let abs = resolver.resolve(&rel);
        if paths::is_corpus_path(&rel) {
            spawned.push(Computation::Derivation(
                derivation::Computation::UpdateCorpusFileSignals { path: abs }
            ));
        } else if paths::is_library_path(&rel) {
            spawned.push(Computation::Derivation(
                derivation::Computation::UpdateLibraryFileSignals { path: abs }
            ));
        }
    }

    // Phase 4: Additional computations (beyond path-based signal updates)
    // e.g., HardLink spawns UpdateDeploySignals
    spawned.extend(mutation.additional_computations());

    // Phase 5: Specific signal clearing (exact key)
    // e.g., LibraryMove clears LibraryStale for the old path
    let signals_to_clear = mutation.specific_signals_to_clear();
    if !signals_to_clear.is_empty() {
        if let Some(sender) = write_thread::signal_sender() {
            for spec in &signals_to_clear {
                sender.clear_aggregate_signal_fn(spec.clear_by_key_fn, &spec.key, spec.label, witness);
            }
        }
    }

    spawned
}

/// Emit pending signals carried from mutation execution.
///
/// These signals were determined at execution time, before the async DB write,
/// avoiding the race condition where a post-execution DB read might not see the write.
fn emit_pending_signals(
    pending_signals: &[PendingSignal],
    sender: &write_thread::SignalWriteSender,
    witness: &MutationExecutionWitness,
) {
    for signal in pending_signals {
        sender.write_typed_signal(signal.clone(), witness);
    }
}

/// Emit CorruptFile/ShitFormat signals based on mutation type.
///
/// For IndexFileFromPath, uses pending_signals (determined at execution time)
/// to avoid race conditions with async DB writes.
fn emit_file_inherent_signals(
    mutation: &Mutation,
    pending_signals: &[PendingSignal],
    witness: &MutationExecutionWitness,
) {
    // Only IndexFileFromPath emits file-inherent signals
    if let Mutation::IndexFileFromPath(_) = mutation {
        if let Some(sender) = write_thread::signal_sender() {
            emit_pending_signals(pending_signals, sender, witness);
        }
    }
}

