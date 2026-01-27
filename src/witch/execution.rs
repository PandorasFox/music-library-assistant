//! Task execution for mutations, computations, and migrations.
//!
//! This module is part of the Witch subsystem. See `witch/mod.rs` for overview.

use std::time::Instant;

use crate::config;
use crate::corpus::computations::{Computation, awakening};
use crate::corpus::db::types::{AggregateSignalType, CorpusFileSignalType};
use crate::corpus::db::Database;
use crate::corpus::health::normalization::{
    normalize_album, normalize_album_artist, normalize_artist, normalize_genre,
};
use crate::corpus::mutations::{Mutation, TagEdit};
use crate::db_thread;

use super::types::{Migration, MigrationWitness, MutationExecutionWitness, Task, TaskResult};

// ============================================================================
// Database Opening Helper
// ============================================================================

/// Open database for task execution, returning error TaskResult if it fails.
pub(super) fn open_db_for_task(label: String, start: Instant, queue_wait_ms: u64) -> Result<Database, TaskResult> {
    match config::get_db_path().and_then(|p| Database::open(&p).map_err(|e| e.into())) {
        Ok(db) => Ok(db),
        Err(e) => {
            crate::logging::log_error(format!(
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

    crate::logging::log_mutation(format!(
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

    // Load config for stash_root access (needed by file_ops and transcode)
    let loaded_config = config::load_config().ok();
    let stash_root = loaded_config.as_ref().and_then(|c| c.stash_dir.clone());

    let (success, error) = match mutation.category() {
        MutationCategory::TagEdit => {
            crate::logging::log_mutation(format!(
                "[EXECUTION] TagEdit mutation: {:?}",
                mutation
            ));
            let r = tag_edit::execute_single(&db, &mutation, session_id, &witness);
            (r.success, r.error)
        }
        MutationCategory::FileMove | MutationCategory::FileCopy |
        MutationCategory::Deployment => {
            let r = file_ops::execute_single(&mutation, stash_root.as_deref(), &witness);
            (r.success, r.error)
        }
        MutationCategory::Indexing => {
            let r = indexing::execute_single(&db, &mutation, &witness);
            (r.success, r.error)
        }
        MutationCategory::Migration => {
            (false, Some("Migrations not supported in Witch executor".to_string()))
        }
        MutationCategory::Transcode => {
            let r = crate::corpus::mutations::transcode::execute_single(
                &db, &mutation, stash_root.as_deref(), &witness,
            );
            (r.success, r.error)
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

    // Clear affected TagCanonicity and InconsistentAlbumArtist signals after successful tag edits
    // The next awake-phase computations will recreate any still-valid signals
    if success {
        if let Mutation::TagEditAndFlush { track_id, edits, .. } = &mutation {
            clear_affected_canonicity_signals(&db, *track_id, edits, &witness);
        }
    }

    // Emit WaveformReadError for indexing mutations where fingerprint extraction failed
    if success {
        let resolver = crate::corpus::paths::get_resolver();
        match &mutation {
            Mutation::IndexTrack { path, source, metadata } if metadata.fingerprint.is_none() => {
                if let Some(rel) = resolver.to_relative(path, source) {
                    if let Some(sender) = db_thread::signal_sender() {
                        sender.ensure_file_signal(
                            CorpusFileSignalType::WaveformReadError.into(),
                            &rel.to_string_lossy(),
                            &witness,
                        );
                    }
                }
            }
            Mutation::IndexFileFromPath { path, source } => {
                if let Some(rel) = resolver.to_relative(path, source) {
                    let rel_str = rel.to_string_lossy();
                    if let Ok(Some(track)) = db.get_track_by_path(&rel_str) {
                        if track.fingerprint.is_none() {
                            if let Some(sender) = db_thread::signal_sender() {
                                sender.ensure_file_signal(
                                    CorpusFileSignalType::WaveformReadError.into(),
                                    &rel_str,
                                    &witness,
                                );
                            }
                        }
                    }
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
    let mut spawn: Vec<Computation> = if success {
        mutation
            .affected_paths()
            .into_iter()
            .filter_map(|path| {
                if let Some(ref cfg) = loaded_config {
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

/// Clear TagCanonicity and InconsistentAlbumArtist signals affected by tag edits.
///
/// For each edited tag (artist, album_artist, album, genre), compute the normalized
/// key from the OLD value and clear the corresponding signal. The next awake-phase
/// computations will recreate any still-valid signals.
///
/// For album_artist edits, also clears InconsistentAlbumArtist signals for the track's album.
fn clear_affected_canonicity_signals(
    db: &Database,
    track_id: i64,
    edits: &[TagEdit],
    witness: &MutationExecutionWitness,
) {
    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => return,
    };

    // Get the track's album and artist for context-aware signal clearing
    let track_album = db.get_track_tag_value(track_id, "album").ok().flatten().unwrap_or_default();
    let track_artist = db.get_track_tag_value(track_id, "artist").ok().flatten().unwrap_or_default();

    for edit in edits {
        // Clear TagCanonicity signals for affected tag types
        let normalized_key = match edit.tag_name.as_str() {
            "artist" => edit.old_value.as_ref().map(|v| normalize_artist(v)),
            "album_artist" => edit.old_value.as_ref().map(|v| normalize_album_artist(v)),
            "album" => {
                // Album canonicity uses "{artist}::{album}" format
                edit.old_value.as_ref().map(|v| {
                    let norm_artist = normalize_artist(&track_artist);
                    let norm_album = normalize_album(v);
                    format!("{} :: {}", norm_artist, norm_album)
                })
            }
            "genre" => edit.old_value.as_ref().map(|v| normalize_genre(v)),
            _ => None, // Other tag types don't have canonicity signals
        };

        if let Some(normalized) = normalized_key {
            let signal_key = format!("{}:{}", edit.tag_name, normalized);
            sender.clear_aggregate_signal(
                AggregateSignalType::TagCanonicity,
                &signal_key,
                witness,
            );
        }

        // For album_artist edits, also clear InconsistentAlbumArtist signals
        if edit.tag_name == "album_artist" && !track_album.is_empty() {
            let normalized_album = normalize_album(&track_album);
            sender.clear_aggregate_signal(
                AggregateSignalType::InconsistentAlbumArtist,
                &normalized_album,
                witness,
            );
        }
    }
}
