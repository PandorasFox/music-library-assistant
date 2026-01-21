//! Task execution for mutations, computations, and migrations.
//!
//! This module is part of the Witch subsystem. See `witch/mod.rs` for overview.

use std::time::Instant;

use crate::config;
use crate::corpus::computations::{Computation, awakening};
use crate::corpus::db::Database;
use crate::corpus::mutations::Mutation;

use super::types::{Migration, MigrationWitness, MutationExecutionWitness, Task, TaskResult};

// ============================================================================
// Database Opening Helper
// ============================================================================

/// Open database for task execution, returning error TaskResult if it fails.
pub(super) fn open_db_for_task(label: String, start: Instant, queue_wait_ms: u64) -> Result<Database, TaskResult> {
    match config::get_db_path().and_then(|p| Database::open(&p).map_err(|e| e.into())) {
        Ok(db) => Ok(db),
        Err(e) => {
            let _ = config::log_message(&format!(
                "[EXECUTION] DB open FAILED: {}",
                e
            ));
            Err(TaskResult {
                success: false,
                error: Some(format!("DB error: {}", e)),
                label,
                spawn: Vec::new(),
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

/// Execute a single mutation. Opens DB connection as needed.
pub(super) fn execute_mutation(mutation: Mutation, label: String, queue_wait_ms: u64) -> TaskResult {
    use crate::corpus::mutations::{file_ops, tag_edit, indexing, MutationCategory};

    let start = Instant::now();

    let _ = config::log_message(&format!(
        "[EXECUTION] execute_mutation START: {:?} (label={:?})",
        mutation.category(), label
    ));

    // Create execution witness - proves we're inside the Witch's execution context
    let witness = MutationExecutionWitness::new();

    // Open database
    let db = match open_db_for_task(label.clone(), start, queue_wait_ms) {
        Ok(db) => db,
        Err(result) => return result,
    };

    let session_id = "witch";

    let (success, error) = match mutation.category() {
        MutationCategory::TagEdit => {
            let _ = config::log_message(&format!(
                "[EXECUTION] TagEdit mutation: {:?}",
                mutation
            ));
            let r = tag_edit::execute_single(&db, &mutation, session_id, &witness);
            (r.success, r.error)
        }
        MutationCategory::FileMove | MutationCategory::FileCopy |
        MutationCategory::Deployment => {
            let r = file_ops::execute_single(Some(&db), &mutation, &witness);
            (r.success, r.error)
        }
        MutationCategory::Indexing => {
            let r = indexing::execute_single(&db, &mutation, &witness);
            (r.success, r.error)
        }
        MutationCategory::Migration => {
            (false, Some("Migrations not supported in Witch executor".to_string()))
        }
    };

    let duration_ms = start.elapsed().as_millis() as u64;

    let _ = config::log_message(&format!(
        "[EXECUTION] execute_mutation END: success={}, error={:?}, duration={}ms",
        success, error, duration_ms
    ));

    // Queue per-file signal updates for affected paths
    // This ensures signals like UnindexedFile → HealthyFile are updated
    // Mutations can ONLY spawn awakening-phase computations (phase boundary enforcement)
    //
    // Path-aware spawning: corpus paths → UpdateCorpusFileSignals,
    // library paths → UpdateLibraryFileSignals, others → skip
    let mut spawn: Vec<Computation> = if success {
        let config = config::load_config().ok();
        mutation
            .affected_paths()
            .into_iter()
            .filter_map(|path| {
                if let Some(ref cfg) = config {
                    if path.starts_with(&cfg.corpus_root) {
                        Some(Computation::Awakening(awakening::Computation::UpdateCorpusFileSignals { path }))
                    } else if path.starts_with(&cfg.libraries_root) {
                        Some(Computation::Awakening(awakening::Computation::UpdateLibraryFileSignals { path }))
                    } else {
                        // Path outside corpus/library - skip
                        None
                    }
                } else {
                    // No config - skip to be safe
                    None
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    // For deploy mutations, also spawn deploy signal updates
    if success {
        match &mutation {
            Mutation::HardLink { source, destination } => {
                spawn.push(Computation::Awakening(awakening::Computation::UpdateDeploySignals {
                    corpus_path: source.clone(),
                    library_path: destination.clone(),
                }));
            }
            Mutation::LibraryMove { source, .. } => {
                // Clear LibraryStale signals for the old (source) path
                clear_library_stale_signals(&db, source, &witness);
            }
            _ => {}
        }
    }

    TaskResult {
        success,
        error,
        label,
        spawn,
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

    // Open database
    let db = match open_db_for_task(label.clone(), start, queue_wait_ms) {
        Ok(db) => db,
        Err(result) => return result,
    };

    // Apply the specific migration
    let registry = MigrationRegistry::new();
    let result = registry.apply_migration(&db, migration.to_version, &witness);

    let (success, error) = match result {
        Ok(()) => (true, None),
        Err(e) => (false, Some(e.to_string())),
    };

    TaskResult {
        success,
        error,
        label,
        spawn: Vec::new(),
        duration_ms: start.elapsed().as_millis() as u64,
        queue_wait_ms,
        thread_stats: None, // Migrations don't use thread-local stats
    }
}

/// Clear LibraryStale signals for a library path after a LibraryMove.
///
/// LibraryStale signals use compound keys like "library_stale:{name}:{path}",
/// so we query existing signals and clear matching ones.
fn clear_library_stale_signals(
    db: &Database,
    library_path: &std::path::Path,
    witness: &MutationExecutionWitness,
) {
    use crate::corpus::db::types::{LibraryFileSignalType, HealthIssueType};
    use crate::db_thread;

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => return,
    };

    let library_path_str = library_path.to_string_lossy();

    // Query LibraryStale signals and clear those matching this path
    if let Ok(signals) = db.get_health_signals(Some(HealthIssueType::LibraryStale)) {
        for signal in signals {
            // Key format: "library_stale:{name}:{path}"
            if signal.issue_key.ends_with(&format!(":{}", library_path_str)) {
                sender.clear_file_signal_for_mutation(
                    LibraryFileSignalType::LibraryStale.into(),
                    &signal.issue_key,
                    witness,
                );
            }
        }
    }
}
