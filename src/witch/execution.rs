//! Task execution for mutations, computations, and migrations.
//!
//! This module is part of the Witch subsystem. See `witch/mod.rs` for overview.
//!
//! ## DB Access Patterns
//!
//! - **Mutations**: Use thread-local read-only connection (`with_read_only_db`).
//!   All writes go through `db_thread::signal_sender()`.
//! - **Computations**: Use thread-local read-only connection (same pattern).
//! - **Migrations**: Open write connection via `Database::open()` (requires schema changes).
//!
//! ## Post-Execution Pipeline
//!
//! After a mutation executes successfully, `apply_post_execution()` runs a 5-phase
//! pipeline defined by exhaustive match methods on `Mutation`:
//!
//! 1. **Signal clearing** - Clear signals for affected paths (scope from `signal_clear_scope()`)
//! 2. **File-inherent signals** - Emit CorruptFile/ShitFormat (from `checks_*` methods)
//! 3. **Signal update spawning** - Spawn UpdateFileSignals (from `paths_for_signal_updates()`)
//! 4. **Additional computations** - Spawn extra computations (from `additional_computations()`)
//! 5. **Specific signal clearing** - Clear signals by type+key (from `specific_signals_to_clear()`)

use std::os::unix::fs::MetadataExt;
use std::time::Instant;

use crate::config;
use crate::corpus::computations::{Computation, awakening, with_read_only_db};
use crate::corpus::db::types::CorpusFileSignalType;
use crate::corpus::db::{Database, ReadOnlyDb};
use crate::corpus::mutations::{Mutation, PendingSignal, SignalClearScope, SignalToClear};
use crate::corpus::paths;
use crate::db_thread;

use super::types::{Migration, MigrationWitness, MutationExecutionWitness, SpawnedMutation, Task, TaskResult};

// ============================================================================
// Database Opening Helper (Migrations Only)
// ============================================================================

/// Open a write-capable database connection for migrations.
///
/// Migrations require write access because they may alter the database schema.
/// Regular mutations and computations use thread-local read-only connections
/// via `with_read_only_db()` and route writes through `db_thread::signal_sender()`.
fn open_db_for_migration(label: String, start: Instant, queue_wait_ms: u64) -> Result<Database, TaskResult> {
    match config::get_db_path().and_then(|p| Database::open(&p).map_err(|e| e.into())) {
        Ok(db) => Ok(db),
        Err(e) => {
            crate::logging::log_error(format!(
                "[EXECUTION] DB open FAILED: {:#}",
                e
            ));
            Err(TaskResult {
                success: false,
                error: Some(format!("DB error: {:#}", e)),
                label,
                spawn: Vec::new(),
                spawn_mutations: Vec::new(),
                duration_ms: start.elapsed().as_millis() as u64,
                queue_wait_ms,
                thread_stats: None,
            })
        }
    }
}

// ============================================================================
// Task Execution
// ============================================================================

/// Execute a single task (mutation, computation, or migration). Opens DB connection as needed.
pub(super) fn execute_task(task: Task, label: String, queue_time: Instant) -> TaskResult {
    let queue_wait_ms = queue_time.elapsed().as_millis() as u64;

    match task {
        Task::Mutation(mutation) => execute_mutation(mutation, label, queue_wait_ms),
        Task::Computation(computation) => execute_computation(computation, label, queue_wait_ms),
        Task::Migration(migration) => execute_migration(migration, label, queue_wait_ms),
    }
}

/// Execute a single mutation using thread-local read-only DB connection.
///
/// All writes go through `db_thread::signal_sender()`. Read operations use
/// the same thread-local cached connection as computations.
pub(super) fn execute_mutation(mutation: Mutation, label: String, queue_wait_ms: u64) -> TaskResult {
    use crate::corpus::mutations::{file_ops, tag_edit, indexing};

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

    let session_id = "witch";

    // Execute mutation using thread-local read-only DB connection.
    // All writes go through db_thread::signal_sender() (fire-and-forget).
    // Result tuple: (success, error, spawn_mutations, pending_signals)
    //
    // Route directly by variant to the appropriate executor module.
    let result = with_read_only_db(|read_db| {
        match &mutation {
            // Tag edit: incremental operations with validation, spawns disk sync
            Mutation::ApplyTagOps { .. } => {
                let r = tag_edit::execute_single(read_db, &mutation, session_id, &witness);
                (r.success, r.error, r.spawn_mutations, r.pending_signals)
            }

            // Indexing operations (including OOB tag sync which does disk I/O)
            Mutation::IndexFileFromPath { .. }
            | Mutation::UpdateFilePath { .. }
            | Mutation::DropFromIndex { .. }
            | Mutation::DropDirectoryFromIndex { .. }
            | Mutation::AcknowledgeMtimeOnly { .. }
            | Mutation::ApplyDbTagsToDisk { .. }
            | Mutation::AssimilateDiskTagsToDb { .. }
            | Mutation::EmitCanonicalTag { .. } => {
                let r = indexing::execute_single(read_db, &mutation, &witness);
                (r.success, r.error, r.spawn_mutations, r.pending_signals)
            }

            // File operations
            Mutation::Move { .. }
            | Mutation::MoveToStash { .. }
            | Mutation::HardLink { .. }
            | Mutation::LibraryMove { .. } => {
                let r = file_ops::execute_single(&mutation, stash_root.as_deref(), &witness);
                (r.success, r.error, r.spawn_mutations, r.pending_signals)
            }

            // Transcode
            Mutation::Transcode { .. } => {
                let r = crate::corpus::mutations::transcode::execute_single(
                    read_db, &mutation, stash_root.as_deref(), &witness,
                );
                (r.success, r.error, r.spawn_mutations, r.pending_signals)
            }

            // Migrations are handled separately (require write DB connection)
            Mutation::DbMigration { .. } => {
                (false, Some("Migrations not supported in Witch executor".to_string()), Vec::<SpawnedMutation>::new(), Vec::new())
            }
        }
    });

    // Handle DB access failure
    let (success, error, spawn_mutations, pending_signals) = match result {
        Ok((s, e, sm, ps)) => (s, e, sm, ps),
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
        if let Mutation::IndexFileFromPath { path, .. } = &mutation {
            if let Some(sender) = db_thread::signal_sender() {
                // Get inode from filesystem (file exists but failed to parse)
                if let Ok(metadata) = std::fs::metadata(path) {
                    let inode = metadata.ino() as i64;
                    let resolver = paths::get_resolver();
                    if let Some(rel) = resolver.to_relative(path) {
                        let rel_str = rel.to_string_lossy();
                        sender.ensure_corpus_signal(
                            CorpusFileSignalType::CorruptFile,
                            inode,
                            &rel_str,
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
    let spawn = apply_post_execution(&mutation, success, &pending_signals, &witness);

    TaskResult {
        success,
        error,
        label,
        spawn,
        spawn_mutations,
        duration_ms,
        queue_wait_ms,
        thread_stats: None, // Mutations don't use thread-local stats
    }
}

/// Execute a single computation. Uses thread-local DB connection.
pub(super) fn execute_computation(computation: Computation, label: String, queue_wait_ms: u64) -> TaskResult {
    use crate::corpus::computations;

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
    }
}

/// Execute a single migration. Opens DB connection and runs the migration.
pub(super) fn execute_migration(migration: Migration, label: String, queue_wait_ms: u64) -> TaskResult {
    use crate::corpus::mutations::MigrationRegistry;

    let start = Instant::now();

    // Create migration witness - proves we're inside the Witch's execution context
    let witness = MigrationWitness::new();

    // Open write-capable database (migrations require schema changes)
    let db = match open_db_for_migration(label.clone(), start, queue_wait_ms) {
        Ok(db) => db,
        Err(result) => return result,
    };

    // Apply the migration (includes schema version update in same transaction)
    let registry = MigrationRegistry::new();
    let result = registry.apply_migration(&db, migration.to_version, &witness);

    let (success, error) = match &result {
        Ok(()) => (true, None),
        Err(e) => (false, Some(format!("{:#}", e))),
    };

    TaskResult {
        success,
        error,
        label,
        spawn: Vec::new(),
        spawn_mutations: Vec::new(), // Migrations don't spawn mutations
        duration_ms: start.elapsed().as_millis() as u64,
        queue_wait_ms,
        thread_stats: None, // Migrations don't use thread-local stats
    }
}

// ============================================================================
// Post-Execution Pipeline
// ============================================================================

/// Apply post-execution hooks for a mutation.
///
/// This is a structured 5-phase pipeline that replaces the ad-hoc conditionals
/// that previously handled signal clearing, emission, and computation spawning.
///
/// Each phase uses exhaustive match methods on `Mutation` to ensure compile-time
/// enforcement when new variants are added.
///
/// ## Phases
///
/// 1. **Signal clearing** - Clear signals for affected paths based on `signal_clear_scope()`
/// 2. **File-inherent signals** - Emit CorruptFile/ShitFormat via pending_signals or `checks_*` methods
/// 3. **Signal update spawning** - Spawn UpdateFileSignals for `paths_for_signal_updates()`
/// 4. **Additional computations** - Spawn extra computations from `additional_computations()`
/// 5. **Specific signal clearing** - Clear signals by type+key from `specific_signals_to_clear()`
fn apply_post_execution(
    mutation: &Mutation,
    success: bool,
    pending_signals: &[PendingSignal],
    witness: &MutationExecutionWitness,
) -> Vec<Computation> {
    if !success {
        return Vec::new();
    }

    let resolver = paths::get_resolver();
    let mut spawned = Vec::new();

    // Phase 1: Signal clearing (MUST happen before emission)
    // Clears stale signals for affected paths; scope determined by mutation type.
    let clear_scope = mutation.signal_clear_scope();
    if clear_scope != SignalClearScope::None {
        if let Some(sender) = db_thread::signal_sender() {
            for path in mutation.affected_paths() {
                let signal_key = if path.is_absolute() {
                    resolver.to_relative(&path)
                } else {
                    Some(path)
                };
                if let Some(key) = signal_key {
                    let key_str = key.to_string_lossy();
                    match clear_scope {
                        SignalClearScope::All => sender.clear_signals_for_path(&key_str, witness),
                        SignalClearScope::MutableOnly => sender.clear_mutable_signals_for_path(&key_str, witness),
                        SignalClearScope::None => {} // Already handled above
                    }
                }
            }
        }
    }

    // Phase 1b: Drop files table entry for MoveToStash
    // When stashing a file, we must also remove it from the files table (not just signals).
    // Otherwise DeriveDirectorySignals will emit MissingFile for the stashed path.
    if let Mutation::MoveToStash { path, .. } = mutation {
        if let Some(sender) = db_thread::signal_sender() {
            let rel_path = if path.is_absolute() {
                resolver.to_relative(path)
            } else {
                Some(path.clone())
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
            spawned.push(Computation::Awakening(
                awakening::Computation::UpdateCorpusFileSignals { path: abs }
            ));
        } else if paths::is_library_path(&rel) {
            spawned.push(Computation::Awakening(
                awakening::Computation::UpdateLibraryFileSignals { path: abs }
            ));
        }
    }

    // Phase 4: Additional computations (beyond path-based signal updates)
    // e.g., HardLink spawns UpdateDeploySignals
    spawned.extend(mutation.additional_computations());

    // Phase 5: Specific signal clearing (by type+key pattern)
    // e.g., LibraryMove clears LibraryStale for the old path
    let signals_to_clear = mutation.specific_signals_to_clear();
    if !signals_to_clear.is_empty() {
        let _ = with_read_only_db(|read_db| {
            if let Some(sender) = db_thread::signal_sender() {
                for spec in &signals_to_clear {
                    clear_signals_by_pattern(read_db, &sender, spec, witness);
                }
            }
        });
    }

    spawned
}

/// Clear signals matching a type+key pattern.
///
/// Used for targeted clearing like LibraryStale signals, where the key format
/// is compound (e.g., "{name}:{path}") and doesn't match simple path-based clearing.
fn clear_signals_by_pattern(
    db: &ReadOnlyDb<'_>,
    sender: &db_thread::SignalWriteSender,
    spec: &SignalToClear,
    witness: &MutationExecutionWitness,
) {
    // Query existing aggregate signals of this type and clear those matching the pattern
    if let Ok(signals) = db.get_aggregate_signals(Some(spec.signal_type)) {
        for signal in signals {
            // Match if key ends with the pattern (compound key format)
            // or if key equals the pattern exactly
            if signal.key.ends_with(&format!(":{}", spec.key_pattern))
               || signal.key == spec.key_pattern
            {
                sender.clear_aggregate_signal(spec.signal_type, &signal.key, witness);
            }
        }
    }
}

/// Emit pending signals carried from mutation execution.
///
/// These signals were determined at execution time, before the async DB write,
/// avoiding the race condition where a post-execution DB read might not see the write.
fn emit_pending_signals(
    pending_signals: &[PendingSignal],
    sender: &db_thread::SignalWriteSender,
    witness: &MutationExecutionWitness,
) {
    for signal in pending_signals {
        match signal {
            PendingSignal::CorpusSignal { signal_type, inode, path } => {
                sender.ensure_corpus_signal(*signal_type, *inode, path, witness);
            }
            PendingSignal::CorpusSignalWithMetadata { signal_type, inode, path, metadata_json } => {
                let extra_metadata: serde_json::Value = serde_json::from_str(metadata_json)
                    .unwrap_or_else(|_| serde_json::json!({}));
                sender.ensure_corpus_signal_with_metadata(
                    *signal_type,
                    *inode,
                    path,
                    extra_metadata,
                    witness,
                );
            }
        }
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
    if let Mutation::IndexFileFromPath { .. } = mutation {
        if let Some(sender) = db_thread::signal_sender() {
            emit_pending_signals(pending_signals, &sender, witness);
        }
    }
}

