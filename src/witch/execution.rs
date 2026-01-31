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

use std::time::Instant;

use crate::config;
use crate::corpus::computations::{Computation, awakening, with_read_only_db};
use crate::corpus::db::types::CorpusFileSignalType;
use crate::corpus::db::Database;
use crate::corpus::mutations::Mutation;
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
    // Result tuple: (success, error, spawn_mutations)
    //
    // Route directly by variant to the appropriate executor module.
    let result = with_read_only_db(|read_db| {
        match &mutation {
            // Tag edit: DB write that spawns disk sync
            Mutation::SetTrackTagsDb { .. } => {
                let r = tag_edit::execute_single(read_db, &mutation, session_id, &witness);
                (r.success, r.error, r.spawn_mutations)
            }

            // Indexing operations (including OOB tag sync which does disk I/O)
            Mutation::IndexTrack { .. }
            | Mutation::IndexFileFromPath { .. }
            | Mutation::UpdateScanState { .. }
            | Mutation::CleanupStaleScanState { .. }
            | Mutation::UpdateTrackPath { .. }
            | Mutation::UpdateScanStatePath { .. }
            | Mutation::DropFromIndex { .. }
            | Mutation::UpdateTrack { .. }
            | Mutation::AcknowledgeMtimeOnly { .. }
            | Mutation::AcknowledgeInodeChanged { .. }
            | Mutation::ApplyDbTagsToDisk { .. }
            | Mutation::AssimilateDiskTagsToDb { .. } => {
                let r = indexing::execute_single(read_db, &mutation, &witness);
                (r.success, r.error, r.spawn_mutations)
            }

            // File operations
            Mutation::Move { .. }
            | Mutation::Copy { .. }
            | Mutation::MoveToStash { .. }
            | Mutation::HardLink { .. }
            | Mutation::LibraryMove { .. } => {
                let r = file_ops::execute_single(&mutation, stash_root.as_deref(), &witness);
                (r.success, r.error, r.spawn_mutations)
            }

            // Transcode
            Mutation::Transcode { .. } => {
                let r = crate::corpus::mutations::transcode::execute_single(
                    read_db, &mutation, stash_root.as_deref(), &witness,
                );
                (r.success, r.error, r.spawn_mutations)
            }

            // Migrations are handled separately (require write DB connection)
            Mutation::DbMigration { .. } => {
                (false, Some("Migrations not supported in Witch executor".to_string()), Vec::<SpawnedMutation>::new())
            }
        }
    });

    // Handle DB access failure
    let (success, error, spawn_mutations) = match result {
        Ok((s, e, sm)) => (s, e, sm),
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
    }

    // TODO: The post-execution hooks below (signal emission, signal clearing, path wiping,
    // spawn collection) have grown ad-hoc. Refactor into a structured post-execution
    // pipeline, e.g. a `PostExecutionContext` that collects side-effects from both
    // success and failure paths, rather than interleaving conditionals.

    // Wipe per-file signals for affected paths FIRST, before emitting new signals.
    // This prevents stale signals from persisting when mutations change corpus truth
    // (e.g. MissingFile signals surviving after a DropFromIndex removes the track).
    // The spawned signal update computations will re-derive any still-valid signals.
    // Uses signal_sender for the writes (fire-and-forget).
    //
    // IMPORTANT: This must happen BEFORE CorruptFile/ShitFormat emission below,
    // otherwise those signals get immediately wiped.
    //
    // TODO: Refactor signal clearing to use type-level signal categories instead of
    // mutation-type matching. See SignalWriteOp TODO for the broader refactoring plan.
    if success {
        if let Some(sender) = db_thread::signal_sender() {
            let resolver = crate::corpus::paths::get_resolver();

            // Determine if this mutation should clear ALL signals (including file-inherent)
            // or only mutable signals (preserving CorruptFile, ShitFormat).
            let clear_all = matches!(
                &mutation,
                Mutation::MoveToStash { .. }
                | Mutation::DropFromIndex { .. }
                | Mutation::Transcode { .. }
            );

            for path in mutation.affected_paths() {
                let signal_key = if path.is_absolute() {
                    resolver.to_relative(&path)
                } else {
                    Some(path)  // already root-relative
                };
                if let Some(key) = signal_key {
                    if clear_all {
                        sender.clear_signals_for_path(&key.to_string_lossy(), &witness);
                    } else {
                        sender.clear_mutable_signals_for_path(&key.to_string_lossy(), &witness);
                    }
                }
            }
        }
    }

    // Emit CorruptFile for indexing mutations where fingerprint extraction failed
    // (waveform decode failure indicates corrupt audio data)
    // Also emit ShitFormat for non-Vorbis container formats (MP3, M4A, AAC, WMA, etc.)
    if success {
        /// File types that should trigger ShitFormat signal (non-Vorbis containers)
        /// Includes lossy formats with poor metadata and lossless needing remux
        const SHIT_FORMAT_TYPES: &[&str] = &["mp3", "m4a", "aac", "wma", "wav", "aiff", "aif", "ape", "wv"];

        let resolver = crate::corpus::paths::get_resolver();
        match &mutation {
            Mutation::IndexTrack { path, metadata, .. } => {
                if let Some(rel) = resolver.to_relative(path) {
                    if let Some(sender) = db_thread::signal_sender() {
                        let rel_str = rel.to_string_lossy();

                        // CorruptFile if fingerprint extraction failed
                        if metadata.fingerprint.is_none() {
                            sender.ensure_file_signal(
                                CorpusFileSignalType::CorruptFile.into(),
                                &rel_str,
                                &witness,
                            );
                        }

                        // ShitFormat if non-Vorbis container
                        let file_type_lower = metadata.file_type.to_lowercase();
                        if SHIT_FORMAT_TYPES.contains(&file_type_lower.as_str()) {
                            let metadata_json = serde_json::json!({
                                "file_type": metadata.file_type
                            }).to_string();
                            sender.ensure_file_signal_with_metadata(
                                CorpusFileSignalType::ShitFormat.into(),
                                &rel_str,
                                Some(&metadata_json),
                                &witness,
                            );
                        }
                    }
                }
            }
            Mutation::IndexFileFromPath { path, .. } => {
                if let Some(rel) = resolver.to_relative(path) {
                    let rel_str = rel.to_string_lossy();
                    // Check fingerprint and file_type via read-only DB
                    let _ = with_read_only_db(|read_db| {
                        if let Ok(Some(track)) = read_db.get_track_by_path(&rel_str) {
                            if let Some(sender) = db_thread::signal_sender() {
                                // CorruptFile if fingerprint extraction failed
                                if track.fingerprint.is_none() {
                                    sender.ensure_file_signal(
                                        CorpusFileSignalType::CorruptFile.into(),
                                        &rel_str,
                                        &witness,
                                    );
                                }

                                // ShitFormat if non-Vorbis container
                                let file_type_lower = track.file_type.to_lowercase();
                                if SHIT_FORMAT_TYPES.contains(&file_type_lower.as_str()) {
                                    let metadata_json = serde_json::json!({
                                        "file_type": track.file_type
                                    }).to_string();
                                    sender.ensure_file_signal_with_metadata(
                                        CorpusFileSignalType::ShitFormat.into(),
                                        &rel_str,
                                        Some(&metadata_json),
                                        &witness,
                                    );
                                }
                            }
                        }
                    });
                }
            }
            _ => {}
        }
    }

    // Queue per-file signal updates for affected paths
    // This ensures signals like UnindexedFile → HealthyFile are updated
    // Mutations can ONLY spawn awakening-phase computations (phase boundary enforcement)
    //
    // Path-aware spawning: corpus paths → UpdateCorpusFileSignals,
    // library paths → UpdateLibraryFileSignals, others → skip
    //
    // For Transcode: only spawn for the NEW path, not the source path.
    // The source path no longer exists (it's stashed), and spawning signal updates
    // for it causes a race where MissingFile is emitted before db_thread processes
    // the track path update.
    let mut spawn: Vec<Computation> = if success {
        let resolver = crate::corpus::paths::get_resolver();

        // Get paths to spawn signal updates for
        let paths_to_update: Vec<std::path::PathBuf> = match &mutation {
            Mutation::Transcode { source_path, target_format, .. } => {
                // Only spawn for the new path (target), not the source (which is stashed)
                let new_path = source_path.with_extension(target_format.extension());
                vec![new_path]
            }
            _ => mutation.affected_paths(),
        };

        paths_to_update
            .into_iter()
            .filter_map(|path| {
                let rel = if path.is_absolute() {
                    resolver.to_relative(&path)?
                } else {
                    path
                };
                let abs = resolver.resolve(&rel);
                if crate::corpus::paths::is_corpus_path(&rel) {
                    Some(Computation::Awakening(awakening::Computation::UpdateCorpusFileSignals { path: abs }))
                } else if crate::corpus::paths::is_library_path(&rel) {
                    Some(Computation::Awakening(awakening::Computation::UpdateLibraryFileSignals { path: abs }))
                } else {
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
                // Uses with_read_only_db for reads, signal_sender for writes
                let _ = with_read_only_db(|read_db| {
                    clear_library_stale_signals(read_db, source, &witness);
                });
            }
            _ => {}
        }
    }

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

    // Apply the specific migration
    let registry = MigrationRegistry::new();
    let result = registry.apply_migration(&db, migration.to_version, &witness);

    let (success, error) = match result {
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

/// Clear LibraryStale signals for a library path after a LibraryMove.
///
/// LibraryStale signals use compound keys like "library_stale:{name}:{path}",
/// so we query existing signals and clear matching ones.
fn clear_library_stale_signals(
    db: &Database,
    library_path: &std::path::Path,
    witness: &MutationExecutionWitness,
) {
    use crate::corpus::db::types::LibraryFileSignalType;
    use crate::db_thread;

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => return,
    };

    let library_path_str = library_path.to_string_lossy();

    // Query LibraryStale signals and clear those matching this path
    if let Ok(signals) = db.get_signals(Some(LibraryFileSignalType::LibraryStale.into())) {
        for signal in signals {
            // Key format: "library_stale:{name}:{path}"
            if signal.issue_key.ends_with(&format!(":{}", library_path_str)) {
                sender.clear_file_signal(
                    LibraryFileSignalType::LibraryStale.into(),
                    &signal.issue_key,
                    witness,
                );
            }
        }
    }
}

