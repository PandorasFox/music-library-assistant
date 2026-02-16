//! Awakening-phase computation executors.
//!
//! These functions implement the actual logic for Awakening computations.

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::logging::log_general;
use crate::meta::computations::helpers::{
    drop_stale_corpus_signal, ensure_typed_signal,
    enumerate_all_directories, get_configured_library_names, is_audio_file,
};
use crate::meta::computations::types::ComputationWitness;
use crate::meta::signals::data::*;
use crate::meta::signals::store::CorpusSignalStore;
use crate::corpus::db::ReadOnlyDb;
use crate::corpus::paths;
use crate::db_thread;

use super::{Computation, Result};

// ============================================================================
// Second-Level Signal Derivations
// ============================================================================

/// Schedule second-level signal derivations by spawning per-directory computations.
pub fn execute_schedule_second_level_derivations(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    log_general("[COMPUTE] ScheduleSecondLevelDerivations: starting");

    // Get signal sender for missing directory signals
    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                Computation::ScheduleSecondLevelDerivations,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // ========================================================================
    // Detect Missing Directories
    // ========================================================================
    // Check indexed directories against disk to emit/clear MissingDirectory signals
    let indexed_dirs = read_only_db.get_indexed_corpus_directories().unwrap_or_default();
    let resolver = paths::get_resolver();

    let mut missing_dir_count = 0;
    let mut existing_dir_count = 0;

    for (indexed_dir, inode) in &indexed_dirs {
        // Paths in DB are root-relative (e.g., "corpus/physical/cd/...")
        // Use resolver.resolve() to get absolute path
        let abs_path = resolver.resolve(indexed_dir);
        let dir_str = indexed_dir.to_string_lossy().to_string();

        if abs_path.exists() && abs_path.is_dir() {
            // Directory exists - clear any stale MissingDirectory signal
            drop_stale_corpus_signal::<MissingDirectorySignal>(
                read_only_db,
                &sender,
                *inode,
                witness,
            );
            existing_dir_count += 1;
        } else {
            // Directory is missing - emit MissingDirectory signal
            ensure_typed_signal(
                read_only_db,
                &sender,
                TypedSignalWrite::MissingDirectory(MissingDirectorySignal {
                    inode: *inode,
                    path: dir_str.clone(),
                }),
                witness,
            );
            missing_dir_count += 1;
        }
    }

    log_general(format!(
        "[COMPUTE] Directory check: {} indexed, {} existing, {} missing",
        indexed_dirs.len(),
        existing_dir_count,
        missing_dir_count
    ));

    // ========================================================================
    // Schedule Global Corpus Signal Derivation
    // ========================================================================
    // Single global DeriveCorpusSignals replaces per-directory derivation
    log_general("[COMPUTE] ScheduleSecondLevelDerivations: spawning global DeriveCorpusSignals");

    let mut spawn: Vec<Computation> = vec![
        Computation::DeriveCorpusSignals,
        Computation::DeriveInboxSignals,
    ];

    // Also spawn library health computations for each configured library
    if let Ok(config) = crate::config::load_config() {
        let library_names = get_configured_library_names(&config);
        log_general(format!(
            "[COMPUTE] ScheduleSecondLevelDerivations: spawning {} library walks",
            library_names.len()
        ));

        for library_name in library_names {
            let library_root = config.libraries_dir().join(&library_name);
            let corpus_path_prefixes = config.get_corpus_paths_for_library(&library_name);
            spawn.push(Computation::WalkLibrary {
                library_root,
                library_name,
                corpus_path_prefixes,
            });
        }
    }

    Result::success(
        Computation::ScheduleSecondLevelDerivations,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

// ============================================================================
// Global Corpus Signal Derivation
// ============================================================================

/// Derive corpus signals via global inode set comparison.
///
/// Compares disk inodes (from FileInCorpus signals) against indexed inodes:
/// - disk_only = disk - indexed → UnindexedFile signals
/// - index_only = indexed - disk → MissingFile signals
/// - both = disk ∩ indexed → check OOB, emit HealthyFile
pub fn execute_derive_corpus_signals(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    log_general("[COMPUTE] DeriveCorpusSignals: starting global inode comparison");

    // Get signal sender for async writes
    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                Computation::DeriveCorpusSignals,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Get disk state: FileInCorpus signals (inode -> path)
    let disk_inodes = match read_only_db.get_file_in_corpus_inodes() {
        Ok(inodes) => inodes,
        Err(e) => {
            return Result::failure(
                Computation::DeriveCorpusSignals,
                start.elapsed().as_millis() as u64,
                format!("Failed to get FileInCorpus inodes: {}", e),
            );
        }
    };

    // Get indexed state: files table (inode -> path)
    let indexed_inodes = match read_only_db.get_all_corpus_inodes() {
        Ok(inodes) => inodes,
        Err(e) => {
            return Result::failure(
                Computation::DeriveCorpusSignals,
                start.elapsed().as_millis() as u64,
                format!("Failed to get indexed inodes: {}", e),
            );
        }
    };

    log_general(format!(
        "[COMPUTE] DeriveCorpusSignals: {} disk inodes, {} indexed inodes",
        disk_inodes.len(),
        indexed_inodes.len()
    ));

    // Compute set operations
    let disk_set: HashSet<i64> = disk_inodes.keys().copied().collect();
    let indexed_set: HashSet<i64> = indexed_inodes.keys().copied().collect();

    let disk_only: Vec<i64> = disk_set.difference(&indexed_set).copied().collect();
    let index_only: Vec<i64> = indexed_set.difference(&disk_set).copied().collect();
    let both: Vec<i64> = disk_set.intersection(&indexed_set).copied().collect();

    log_general(format!(
        "[COMPUTE] DeriveCorpusSignals: {} unindexed, {} missing, {} present",
        disk_only.len(),
        index_only.len(),
        both.len()
    ));

    // Emit UnindexedFile signals for files on disk but not indexed
    for inode in &disk_only {
        if let Some(path) = disk_inodes.get(inode) {
            ensure_typed_signal(
                read_only_db,
                &sender,
                TypedSignalWrite::UnindexedFile(UnindexedFileSignal {
                    inode: *inode,
                    path: path.to_string(),
                }),
                witness,
            );
        }
    }

    // Emit MissingFile signals for indexed files not on disk
    for inode in &index_only {
        if let Some(path) = indexed_inodes.get(inode) {
            ensure_typed_signal(
                read_only_db,
                &sender,
                TypedSignalWrite::MissingFile(MissingFileSignal {
                    inode: *inode,
                    path: path.to_string(),
                    replaced_by_inode: None,
                }),
                witness,
            );
            // Clear any stale HealthyFile signal
            drop_stale_corpus_signal::<HealthyFileSignal>(
                read_only_db,
                &sender,
                *inode,
                witness,
            );
        }
    }

    // Process files present in both disk and index
    for inode in &both {
        let path = disk_inodes.get(inode).or_else(|| indexed_inodes.get(inode));
        let path_str = path.map(|p| p.as_str()).unwrap_or("");

        // Clear any stale MissingFile/UnindexedFile signals
        drop_stale_corpus_signal::<MissingFileSignal>(
            read_only_db,
            &sender,
            *inode,
            witness,
        );
        drop_stale_corpus_signal::<UnindexedFileSignal>(
            read_only_db,
            &sender,
            *inode,
            witness,
        );

        // Check if file has any OOB signal - if so, don't mark as HealthyFile
        let has_oob_signal =
            read_only_db.corpus_signal_exists::<OutOfBandTagConflictSignal>(*inode) ||
            read_only_db.corpus_signal_exists::<OutOfBandTagSyncSignal>(*inode) ||
            read_only_db.corpus_signal_exists::<MtimeOnlyMismatchSignal>(*inode);

        if has_oob_signal {
            // File has OOB signal - NOT healthy
            drop_stale_corpus_signal::<HealthyFileSignal>(
                read_only_db,
                &sender,
                *inode,
                witness,
            );
        } else {
            ensure_typed_signal(
                read_only_db,
                &sender,
                TypedSignalWrite::HealthyFile(HealthyFileSignal {
                    inode: *inode,
                    path: path_str.to_string(),
                }),
                witness,
            );
        }
    }

    // ========================================================================
    // GC Backstop: Clear orphaned corpus signals for inodes no longer known
    // ========================================================================
    // Any inode that is neither on disk nor in the index has no reason to have
    // corpus signals. This catches signals that persisted due to mutations
    // returning empty affected_inodes() (now fixed) or any future bugs.
    let known_inodes: HashSet<i64> = disk_set.union(&indexed_set).copied().collect();
    let gc_total = gc_orphaned_corpus_signals(read_only_db, &sender, &known_inodes, witness);
    if gc_total > 0 {
        log_general(format!(
            "[COMPUTE] DeriveCorpusSignals: GC cleared {} orphaned signal(s)",
            gc_total
        ));
    }

    log_general("[COMPUTE] DeriveCorpusSignals: complete");

    Result::success(
        Computation::DeriveCorpusSignals,
        start.elapsed().as_millis() as u64,
        Vec::new(),
    )
}

// ============================================================================
// Global Inbox Signal Derivation
// ============================================================================

/// Derive inbox signals via global inode set comparison.
///
/// Compares disk inodes (from FileInInbox signals) against inbox-indexed inodes:
/// - disk_only (disk - indexed) → InboxUnindexed signals
/// - both (disk ∩ indexed) → InboxHealthy signals
/// Inbox files don't produce MissingFile — missing inbox files are simply gone.
pub fn execute_derive_inbox_signals(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    log_general("[COMPUTE] DeriveInboxSignals: starting inbox inode comparison");

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                Computation::DeriveInboxSignals,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Get disk state: FileInInbox signals (inode -> path)
    let disk_inodes = match read_only_db.get_file_in_inbox_inodes() {
        Ok(inodes) => inodes,
        Err(e) => {
            return Result::failure(
                Computation::DeriveInboxSignals,
                start.elapsed().as_millis() as u64,
                format!("Failed to get FileInInbox inodes: {}", e),
            );
        }
    };

    // No inbox files observed — cascade-drop any stale indexed inbox state, then GC
    if disk_inodes.is_empty() {
        log_general("[COMPUTE] DeriveInboxSignals: no inbox files on disk");

        // Drop inbox state for any inodes still indexed as inbox
        let indexed_inodes = read_only_db.get_all_inbox_inodes().unwrap_or_default();
        if !indexed_inodes.is_empty() {
            log_general(format!(
                "[COMPUTE] DeriveInboxSignals: dropping inbox state for {} gone file(s)",
                indexed_inodes.len()
            ));
            for inode in indexed_inodes.keys() {
                sender.drop_inbox_file_state(*inode, witness);
            }
        }

        let gc_total = gc_orphaned_inbox_signals(read_only_db, &sender, &HashSet::new(), witness);
        if gc_total > 0 {
            log_general(format!(
                "[COMPUTE] DeriveInboxSignals: GC cleared {} orphaned signal(s)",
                gc_total
            ));
        }
        return Result::success(
            Computation::DeriveInboxSignals,
            start.elapsed().as_millis() as u64,
            Vec::new(),
        );
    }

    // Get indexed state: files table WHERE zone='inbox' (inode -> path)
    let indexed_inodes = match read_only_db.get_all_inbox_inodes() {
        Ok(inodes) => inodes,
        Err(e) => {
            return Result::failure(
                Computation::DeriveInboxSignals,
                start.elapsed().as_millis() as u64,
                format!("Failed to get indexed inbox inodes: {}", e),
            );
        }
    };

    log_general(format!(
        "[COMPUTE] DeriveInboxSignals: {} disk inodes, {} indexed inodes",
        disk_inodes.len(),
        indexed_inodes.len()
    ));

    let disk_set: HashSet<i64> = disk_inodes.keys().copied().collect();
    let indexed_set: HashSet<i64> = indexed_inodes.keys().copied().collect();

    let disk_only: Vec<i64> = disk_set.difference(&indexed_set).copied().collect();
    let both: Vec<i64> = disk_set.intersection(&indexed_set).copied().collect();

    // Emit InboxUnindexed signals for files on disk but not indexed
    for inode in &disk_only {
        if let Some(path) = disk_inodes.get(inode) {
            ensure_typed_signal(
                read_only_db,
                &sender,
                TypedSignalWrite::InboxUnindexed(InboxUnindexedSignal {
                    inode: *inode,
                    path: path.to_string(),
                }),
                witness,
            );
        }
    }

    // Emit InboxHealthy for files present in both disk and index
    for inode in &both {
        let path = disk_inodes.get(inode).or_else(|| indexed_inodes.get(inode));
        let path_str = path.map(|p| p.as_str()).unwrap_or("");

        // Clear stale InboxUnindexed signal
        drop_stale_corpus_signal::<InboxUnindexedSignal>(
            read_only_db,
            &sender,
            *inode,
            witness,
        );

        ensure_typed_signal(
            read_only_db,
            &sender,
            TypedSignalWrite::InboxHealthy(InboxHealthySignal {
                inode: *inode,
                path: path_str.to_string(),
            }),
            witness,
        );
    }

    // ========================================================================
    // Cascade-drop inbox state for files gone from disk
    // ========================================================================
    let index_only: Vec<i64> = indexed_set.difference(&disk_set).copied().collect();
    if !index_only.is_empty() {
        log_general(format!(
            "[COMPUTE] DeriveInboxSignals: dropping inbox state for {} gone file(s)",
            index_only.len()
        ));
        for inode in &index_only {
            sender.drop_inbox_file_state(*inode, witness);
        }
    }

    log_general(format!(
        "[COMPUTE] DeriveInboxSignals: {} unindexed, {} healthy, {} gone",
        disk_only.len(),
        both.len(),
        index_only.len()
    ));

    // ========================================================================
    // GC Backstop: Clear orphaned inbox signals for inodes no longer known
    // ========================================================================
    // Disk presence is the sole authority — only disk inodes are "known"
    let known_inodes: HashSet<i64> = disk_set;
    let gc_total = gc_orphaned_inbox_signals(read_only_db, &sender, &known_inodes, witness);
    if gc_total > 0 {
        log_general(format!(
            "[COMPUTE] DeriveInboxSignals: GC cleared {} orphaned signal(s)",
            gc_total
        ));
    }

    log_general("[COMPUTE] DeriveInboxSignals: complete");

    Result::success(
        Computation::DeriveInboxSignals,
        start.elapsed().as_millis() as u64,
        Vec::new(),
    )
}

/// GC orphaned corpus signals whose inodes are not in the known universe.
///
/// Returns the total number of orphaned signals cleared.
fn gc_orphaned_corpus_signals(
    read_only_db: &ReadOnlyDb<'_>,
    sender: &db_thread::SignalWriteSender,
    known_inodes: &HashSet<i64>,
    witness: &ComputationWitness,
) -> usize {
    let mut total = 0;
    total += gc_signal_table::<UnindexedFileSignal>(read_only_db, sender, known_inodes, witness);
    total += gc_signal_table::<MissingFileSignal>(read_only_db, sender, known_inodes, witness);
    total += gc_signal_table::<MovedFileSignal>(read_only_db, sender, known_inodes, witness);
    total += gc_signal_table::<HealthyFileSignal>(read_only_db, sender, known_inodes, witness);
    total += gc_signal_table::<CorruptFileSignal>(read_only_db, sender, known_inodes, witness);
    total += gc_signal_table::<ShitFormatSignal>(read_only_db, sender, known_inodes, witness);
    total += gc_signal_table::<MtimeOnlyMismatchSignal>(read_only_db, sender, known_inodes, witness);
    total += gc_signal_table::<OutOfBandTagSyncSignal>(read_only_db, sender, known_inodes, witness);
    total += gc_signal_table::<OutOfBandTagConflictSignal>(read_only_db, sender, known_inodes, witness);
    total += gc_signal_table::<SubparDuplicateSignal>(read_only_db, sender, known_inodes, witness);
    total += gc_signal_table::<CompoundTagSignal>(read_only_db, sender, known_inodes, witness);
    total += gc_signal_table::<DeployReadySignal>(read_only_db, sender, known_inodes, witness);
    total += gc_signal_table::<DeployedHealthySignal>(read_only_db, sender, known_inodes, witness);
    total += gc_signal_table::<MissingDirectorySignal>(read_only_db, sender, known_inodes, witness);
    // FileInCorpus excluded: it IS the disk observation, always part of known_inodes
    total
}

/// GC orphaned inbox signals whose inodes are not in the known universe.
///
/// Returns the total number of orphaned signals cleared.
/// FileInInbox excluded: it IS the disk observation, same reason FileInCorpus is excluded.
fn gc_orphaned_inbox_signals(
    read_only_db: &ReadOnlyDb<'_>,
    sender: &db_thread::SignalWriteSender,
    known_inodes: &HashSet<i64>,
    witness: &ComputationWitness,
) -> usize {
    let mut total = 0;
    total += gc_signal_table::<InboxUnindexedSignal>(read_only_db, sender, known_inodes, witness);
    total += gc_signal_table::<InboxHealthySignal>(read_only_db, sender, known_inodes, witness);
    total += gc_signal_table::<InboxCorpusMatchSignal>(read_only_db, sender, known_inodes, witness);
    // FileInInbox excluded: it IS the disk observation, always part of known_inodes
    total
}

/// Clear signals from a single corpus signal table for inodes not in `known_inodes`.
fn gc_signal_table<S: CorpusSignalStore>(
    read_only_db: &ReadOnlyDb<'_>,
    sender: &db_thread::SignalWriteSender,
    known_inodes: &HashSet<i64>,
    witness: &ComputationWitness,
) -> usize {
    let signal_inodes = match read_only_db.corpus_signal_all_inodes::<S>() {
        Ok(inodes) => inodes,
        Err(_) => return 0,
    };
    let mut cleared = 0;
    for inode in signal_inodes {
        if !known_inodes.contains(&inode) {
            sender.clear_corpus_signal::<S>(inode, witness);
            cleared += 1;
        }
    }
    if cleared > 0 {
        log_general(format!(
            "[COMPUTE] GC: cleared {} orphan(s) from {}",
            cleared, S::TABLE_NAME
        ));
    }
    cleared
}

/// Update corpus signals for a single file after a mutation.
///
/// Only valid for paths within the corpus directory.
/// Uses inode-keyed signals consistent with the global observation system.
pub fn execute_update_corpus_file_signals(
    read_only_db: &ReadOnlyDb<'_>,
    path: &Path,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::UpdateCorpusFileSignals {
        path: path.to_path_buf(),
    };

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Convert absolute path to relative for DB queries
    let resolver = paths::get_resolver();
    let relative_path = resolver
        .to_relative(path)
        .unwrap_or_else(|| path.to_path_buf());
    let path_str = relative_path.to_string_lossy().to_string();

    let file_exists = path.exists() && is_audio_file(path);

    // Get inode from disk or database
    let disk_inode = if file_exists {
        std::fs::metadata(path)
            .ok()
            .map(|m| m.ino() as i64)
    } else {
        None
    };

    // Check if path is indexed (and get the indexed inode)
    let indexed_info = read_only_db.get_audio_file_by_path(&path_str).ok().flatten();
    let indexed_inode = indexed_info.as_ref().map(|af| af.inode());

    if file_exists {
        let inode = disk_inode.expect("file exists but no inode");

        // FileInCorpus: keyed by inode
        ensure_typed_signal(
            read_only_db,
            &sender,
            TypedSignalWrite::FileInCorpus(FileInCorpusSignal {
                inode,
                path: path_str.clone(),
            }),
            witness,
        );

        if indexed_info.is_some() {
            // File is indexed - clear unindexed/missing
            drop_stale_corpus_signal::<UnindexedFileSignal>(
                read_only_db,
                &sender,
                inode,
                witness,
            );
            drop_stale_corpus_signal::<MissingFileSignal>(
                read_only_db,
                &sender,
                inode,
                witness,
            );

            // Check if file has any OOB signal (using native inode column)
            let has_oob_signal =
                read_only_db.corpus_signal_exists::<OutOfBandTagConflictSignal>(inode) ||
                read_only_db.corpus_signal_exists::<OutOfBandTagSyncSignal>(inode) ||
                read_only_db.corpus_signal_exists::<MtimeOnlyMismatchSignal>(inode);

            if has_oob_signal {
                // File has OOB signal - NOT healthy
                drop_stale_corpus_signal::<HealthyFileSignal>(
                    read_only_db,
                    &sender,
                    inode,
                    witness,
                );
            } else {
                ensure_typed_signal(
                    read_only_db,
                    &sender,
                    TypedSignalWrite::HealthyFile(HealthyFileSignal {
                        inode,
                        path: path_str.clone(),
                    }),
                    witness,
                );
            }
        } else {
            // File not indexed - mark as unindexed
            drop_stale_corpus_signal::<HealthyFileSignal>(
                read_only_db,
                &sender,
                inode,
                witness,
            );
            drop_stale_corpus_signal::<MissingFileSignal>(
                read_only_db,
                &sender,
                inode,
                witness,
            );
            ensure_typed_signal(
                read_only_db,
                &sender,
                TypedSignalWrite::UnindexedFile(UnindexedFileSignal {
                    inode,
                    path: path_str.clone(),
                }),
                witness,
            );
        }
    } else if let Some(inode) = indexed_inode {
        // File doesn't exist but was indexed - clear disk signals, mark missing
        drop_stale_corpus_signal::<FileInCorpusSignal>(
            read_only_db,
            &sender,
            inode,
            witness,
        );
        drop_stale_corpus_signal::<UnindexedFileSignal>(
            read_only_db,
            &sender,
            inode,
            witness,
        );
        drop_stale_corpus_signal::<HealthyFileSignal>(
            read_only_db,
            &sender,
            inode,
            witness,
        );
        ensure_typed_signal(
            read_only_db,
            &sender,
            TypedSignalWrite::MissingFile(MissingFileSignal {
                inode,
                path: path_str.clone(),
                replaced_by_inode: None,
            }),
            witness,
        );
    }
    // If file doesn't exist and isn't indexed, there's nothing to do

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

/// Update library signals for a single file after a mutation.
///
/// Only valid for paths within library directories.
/// Handles LibraryLeftover signals when files are added/removed from libraries.
pub fn execute_update_library_file_signals(
    _read_only_db: &ReadOnlyDb<'_>,
    path: &Path,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::UpdateLibraryFileSignals {
        path: path.to_path_buf(),
    };

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Convert absolute path to relative for signal keys
    let resolver = paths::get_resolver();
    let relative_path = resolver
        .to_relative(path)
        .unwrap_or_else(|| path.to_path_buf());
    let path_str = relative_path.to_string_lossy().to_string();

    // For library files, we check if the file exists and clear any leftover/stale signals
    // The full library health is recomputed during the Awake phase
    if path.exists() && is_audio_file(path) {
        // File exists - clear any LibraryLeftover/LibraryStale for this path
        clear_library_signals_for_path(&sender, &path_str, witness);
    }
    // If file doesn't exist, LibraryLeftover signals will be created during
    // the next full library scan in the Awake phase

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Library Health Scanning
// ============================================================================

/// Walk a library directory tree and spawn per-directory scans.
pub fn execute_walk_library(
    _read_only_db: &ReadOnlyDb<'_>,
    library_root: &Path,
    library_name: &str,
    corpus_path_prefixes: &[PathBuf],
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::WalkLibrary {
        library_root: library_root.to_path_buf(),
        library_name: library_name.to_string(),
        corpus_path_prefixes: corpus_path_prefixes.to_vec(),
    };

    // Get signal sender for async writes (library scan state is written via db_thread)
    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    if !library_root.exists() {
        log_general(format!(
            "[COMPUTE] WalkLibrary: library root does not exist: {:?}",
            library_root
        ));
        return Result::success(
            computation,
            start.elapsed().as_millis() as u64,
            Vec::new(),
        );
    }

    // Clear previous files for this library before re-scanning
    // Routes through db_thread which has write access
    sender.clear_library_files(library_name, witness);

    let (directories, _symlink_count) = enumerate_all_directories(library_root);

    log_general(format!(
        "[COMPUTE] WalkLibrary '{}': found {} directories in {:?}",
        library_name,
        directories.len(),
        library_root
    ));

    let spawn: Vec<Computation> = directories
        .into_iter()
        .map(|directory| Computation::ScanLibraryDirectory {
            directory,
            library_name: library_name.to_string(),
            library_root: library_root.to_path_buf(),
            corpus_path_prefixes: corpus_path_prefixes.to_vec(),
        })
        .collect();

    Result::success(computation, start.elapsed().as_millis() as u64, spawn)
}

/// Scan a single library directory and store results in files table.
///
/// The actual deploy health derivation (comparing against corpus) happens in
/// the Awake phase via DeriveDeployHealthSignals.
pub fn execute_scan_library_directory(
    _read_only_db: &ReadOnlyDb<'_>,
    directory: &Path,
    library_name: &str,
    library_root: &Path,
    corpus_path_prefixes: &[PathBuf],
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::ScanLibraryDirectory {
        directory: directory.to_path_buf(),
        library_name: library_name.to_string(),
        library_root: library_root.to_path_buf(),
        corpus_path_prefixes: corpus_path_prefixes.to_vec(),
    };

    // Get signal sender for async writes (library scan state is written via db_thread)
    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Collect audio files in this directory (non-recursive)
    // Capture: path, inode, mtime, file_size for new files table schema
    struct LibraryFileInfo {
        path: PathBuf,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
        file_size: i64,
    }
    let mut library_files: Vec<LibraryFileInfo> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && is_audio_file(&path) {
                if let Ok(metadata) = std::fs::metadata(&path) {
                    let mtime = metadata.modified().ok().and_then(|t| {
                        t.duration_since(std::time::UNIX_EPOCH).ok()
                    });
                    let (mtime_secs, mtime_nanos) = mtime
                        .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
                        .unwrap_or((0, 0));
                    library_files.push(LibraryFileInfo {
                        path,
                        inode: metadata.ino() as i64,
                        mtime_secs,
                        mtime_nanos,
                        file_size: metadata.len() as i64,
                    });
                }
            }
        }
    }

    // Log per-directory scan results (only non-empty directories to avoid noise)
    if !library_files.is_empty() {
        log_general(format!(
            "[COMPUTE] ScanLibraryDirectory '{}': {} audio files in {:?}",
            library_name, library_files.len(), directory,
        ));
    }

    // Store library scan results via db_thread for Awake phase to process
    if !library_files.is_empty() {
        let resolver = paths::get_resolver();
        let scanned_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Convert library_root to relative (relative to archive root)
        let relative_library_root = resolver
            .to_relative(library_root)
            .unwrap_or_else(|| library_root.to_path_buf());

        for info in &library_files {
            // Convert file_path to relative (relative to archive root)
            let relative_file_path = resolver
                .to_relative(&info.path)
                .unwrap_or_else(|| info.path.clone());

            // Routes through db_thread which has write access
            sender.record_library_file(
                library_name,
                &relative_library_root,
                &relative_file_path,
                info.inode,
                info.mtime_secs,
                info.mtime_nanos,
                info.file_size,
                scanned_at,
                witness,
            );
        }
    }

    // No spawn - DeriveDeployHealthSignals runs from ScheduleContentAnalysis in Awake phase
    // corpus_path_prefixes stored with entries for later use by DeriveDeployHealthSignals
    let _ = corpus_path_prefixes; // Suppress unused warning - needed for logging/future use

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Deploy Signal Updates (Post-Mutation)
// ============================================================================

/// Update deploy signals after a HardLink mutation.
///
/// - Clears DeployReady for corpus_path
/// - Ensures DeployedHealthy for corpus_path (with library_path metadata)
/// - Clears any LibraryLeftover/LibraryStale for library_path
pub fn execute_update_deploy_signals(
    read_only_db: &ReadOnlyDb<'_>,
    corpus_path: &Path,
    library_path: &Path,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::UpdateDeploySignals {
        corpus_path: corpus_path.to_path_buf(),
        library_path: library_path.to_path_buf(),
    };

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Convert absolute paths to relative for DB queries and signal keys
    let resolver = paths::get_resolver();
    let relative_library_path = resolver
        .to_relative(library_path)
        .unwrap_or_else(|| library_path.to_path_buf());
    let relative_corpus_path = resolver
        .to_relative(corpus_path)
        .unwrap_or_else(|| corpus_path.to_path_buf());

    let library_path_str = relative_library_path.to_string_lossy().to_string();
    let corpus_path_str = relative_corpus_path.to_string_lossy().to_string();

    // Clear library-side signals for this path
    clear_library_signals_for_path(&sender, &library_path_str, witness);

    // Get the corpus file inode for signal keying
    let corpus_inode = match read_only_db.get_file_entry_by_path(&corpus_path_str, "corpus") {
        Ok(Some(entry)) => entry.inode,
        Ok(None) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Corpus file not in index: {}", corpus_path_str),
            );
        }
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to look up corpus file: {}", e),
            );
        }
    };

    // Clear DeployReady for corpus file (inode-keyed)
    drop_stale_corpus_signal::<DeployReadySignal>(
        read_only_db,
        &sender,
        corpus_inode,
        witness,
    );

    // Ensure DeployedHealthy with library_path (inode-keyed)
    if !read_only_db.corpus_signal_exists::<DeployedHealthySignal>(corpus_inode) {
        sender.write_typed_signal(
            TypedSignalWrite::DeployedHealthy(DeployedHealthySignal {
                inode: corpus_inode,
                path: corpus_path_str.clone(),
                library_path: library_path_str,
            }),
            witness,
        );
    }

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

/// Clear library-side signals (LibraryLeftover, LibraryStale) for a library path.
///
/// Constructs exact keys from the library path (O(1) instead of scanning all keys).
/// `library_path` is "{library_name}/relative/path" (e.g., "libraries/music/Artist/track.opus"
/// or "music/Artist/track.opus" depending on caller).
fn clear_library_signals_for_path(
    sender: &db_thread::SignalWriteSender,
    library_path: &str,
    witness: &ComputationWitness,
) {
    // Extract library_name from first path component
    // library_path may be "libraries/music/..." or "music/..." depending on caller
    let effective_path = library_path.strip_prefix("libraries/").unwrap_or(library_path);
    if let Some(library_name) = effective_path.split('/').next() {
        let leftover_key = LibraryLeftoverSignal::make_key(library_name, effective_path);
        sender.clear_aggregate_signal::<LibraryLeftoverSignal>(&leftover_key, witness);
        let stale_key = LibraryStaleSignal::make_key(library_name, effective_path);
        sender.clear_aggregate_signal::<LibraryStaleSignal>(&stale_key, witness);
    }
}
