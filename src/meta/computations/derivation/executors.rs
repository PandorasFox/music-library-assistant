//! Derivation-phase computation executors.
//!
//! These functions implement the actual logic for Derivation computations.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::logging::log_general;
use crate::meta::computations::helpers::{
    drop_stale_corpus_signal, ensure_typed_signal,
    enumerate_all_directories, get_configured_library_names, is_audio_file, is_image_file,
};
use crate::meta::computations::types::ComputationWitness;
use crate::meta::signals::data::*;
use crate::meta::signals::store::CorpusSignalStore;
use crate::db::ReadOnlyDb;
use crate::corpus::paths;
use crate::db::write_thread;

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
    let sender = match write_thread::signal_sender() {
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
    // Schedule Library Walks
    // ========================================================================
    // DeriveCorpusSignals and DeriveInboxSignals are now queued directly by
    // the Witch with accumulated observation data. This computation handles
    // directory checks (above) and library walks (below).

    let mut spawn: Vec<Computation> = Vec::new();

    // Spawn library health computations for each configured library
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
    observed_inodes: HashMap<i64, String>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    log_general("[COMPUTE] DeriveCorpusSignals: starting global inode comparison");

    // Get signal sender for async writes
    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                Computation::DeriveCorpusSignals {
                    observed_inodes: HashMap::new(),
                },
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Reconcile FileInCorpus signals against observed disk state:
    // - Observed but no signal → write new FileInCorpus
    // - Signal exists but not observed → stale, clear it
    // - Both → already up to date
    let existing_fic = read_only_db.get_file_in_corpus_inodes().unwrap_or_default();
    let mut fic_new = 0usize;
    let mut fic_stale = 0usize;

    for (inode, path) in &observed_inodes {
        if !existing_fic.contains_key(inode) {
            // New file on disk with no FileInCorpus signal — write it
            sender.write_typed_signal(
                TypedSignalWrite::FileInCorpus(FileInCorpusSignal {
                    inode: *inode,
                    path: path.clone(),
                    generation: 0,
                }),
                witness,
            );
            fic_new += 1;
        }
    }

    for inode in existing_fic.keys() {
        if !observed_inodes.contains_key(inode) {
            // Stale FileInCorpus signal — file no longer on disk
            sender.clear_corpus_signal::<FileInCorpusSignal>(*inode, witness);
            fic_stale += 1;
        }
    }

    if fic_new > 0 || fic_stale > 0 {
        log_general(format!(
            "[COMPUTE] DeriveCorpusSignals: FileInCorpus reconciled: {} new, {} stale cleared",
            fic_new, fic_stale
        ));
    }

    // Use observed inodes as the definitive disk state
    let disk_inodes = observed_inodes;

    // Get indexed state: files table (inode -> path)
    let indexed_inodes = match read_only_db.get_all_corpus_inodes() {
        Ok(inodes) => inodes,
        Err(e) => {
            return Result::failure(
                Computation::DeriveCorpusSignals {
                    observed_inodes: HashMap::new(),
                },
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
        Computation::DeriveCorpusSignals {
            observed_inodes: HashMap::new(),
        },
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
    observed_inodes: HashMap<i64, String>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    log_general("[COMPUTE] DeriveInboxSignals: starting inbox inode comparison");

    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                Computation::DeriveInboxSignals {
                    observed_inodes: HashMap::new(),
                },
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Reconcile FileInInbox signals against observed disk state:
    // - Observed but no signal → write new FileInInbox
    // - Signal exists but not observed → stale, clear it
    // - Both → already up to date
    let existing_fii = read_only_db.get_file_in_inbox_inodes().unwrap_or_default();
    let mut fii_new = 0usize;
    let mut fii_stale = 0usize;

    for (inode, path) in &observed_inodes {
        if !existing_fii.contains_key(inode) {
            sender.write_typed_signal(
                TypedSignalWrite::FileInInbox(FileInInboxSignal {
                    inode: *inode,
                    path: path.clone(),
                    generation: 0,
                }),
                witness,
            );
            fii_new += 1;
        }
    }

    for inode in existing_fii.keys() {
        if !observed_inodes.contains_key(inode) {
            sender.clear_corpus_signal::<FileInInboxSignal>(*inode, witness);
            fii_stale += 1;
        }
    }

    if fii_new > 0 || fii_stale > 0 {
        log_general(format!(
            "[COMPUTE] DeriveInboxSignals: FileInInbox reconciled: {} new, {} stale cleared",
            fii_new, fii_stale
        ));
    }

    // Use observed inodes as the definitive disk state
    let disk_inodes = observed_inodes;

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
            Computation::DeriveInboxSignals {
                observed_inodes: HashMap::new(),
            },
            start.elapsed().as_millis() as u64,
            Vec::new(),
        );
    }

    // Get indexed state: files table WHERE zone='inbox' (inode -> path)
    let indexed_inodes = match read_only_db.get_all_inbox_inodes() {
        Ok(inodes) => inodes,
        Err(e) => {
            return Result::failure(
                Computation::DeriveInboxSignals {
                    observed_inodes: HashMap::new(),
                },
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
        Computation::DeriveInboxSignals {
            observed_inodes: HashMap::new(),
        },
        start.elapsed().as_millis() as u64,
        Vec::new(),
    )
}

/// GC orphaned corpus signals whose inodes are not in the known universe.
///
/// Returns the total number of orphaned signals cleared.
fn gc_orphaned_corpus_signals(
    read_only_db: &ReadOnlyDb<'_>,
    sender: &write_thread::SignalWriteSender,
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
    total += gc_signal_table::<SidecarDeployReadySignal>(read_only_db, sender, known_inodes, witness);
    total += gc_signal_table::<MissingDirectorySignal>(read_only_db, sender, known_inodes, witness);
    total += gc_signal_table::<ExternalMatchSignal>(read_only_db, sender, known_inodes, witness);
    // FileInCorpus excluded: it IS the disk observation, always part of known_inodes
    total
}

/// GC orphaned inbox signals whose inodes are not in the known universe.
///
/// Returns the total number of orphaned signals cleared.
/// FileInInbox excluded: it IS the disk observation, same reason FileInCorpus is excluded.
fn gc_orphaned_inbox_signals(
    read_only_db: &ReadOnlyDb<'_>,
    sender: &write_thread::SignalWriteSender,
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
    sender: &write_thread::SignalWriteSender,
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

    let sender = match write_thread::signal_sender() {
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

        // FileInCorpus: keyed by inode (use current observation generation)
        ensure_typed_signal(
            read_only_db,
            &sender,
            TypedSignalWrite::FileInCorpus(FileInCorpusSignal {
                inode,
                path: path_str.clone(),
                generation: crate::meta::computations::helpers::current_observation_generation(),
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

    let sender = match write_thread::signal_sender() {
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
    // The full library health is recomputed during the Analysis phase
    if path.exists() && is_audio_file(path) {
        // File exists - clear any LibraryLeftover/LibraryStale for this path
        clear_library_signals_for_path(&sender, &path_str, witness);
    }
    // If file doesn't exist, LibraryLeftover signals will be created during
    // the next full library scan in the Analysis phase

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Library Health Scanning
// ============================================================================

/// Walk a library directory tree and spawn per-directory scans.
///
/// No longer clears library files from the DB — reconciliation is deferred to
/// ReconcileLibraryFiles after all ScanLibraryDirectory results are accumulated.
pub fn execute_walk_library(
    _read_only_db: &ReadOnlyDb<'_>,
    library_root: &Path,
    library_name: &str,
    corpus_path_prefixes: &[PathBuf],
    _witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::WalkLibrary {
        library_root: library_root.to_path_buf(),
        library_name: library_name.to_string(),
        corpus_path_prefixes: corpus_path_prefixes.to_vec(),
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

/// Scan a single library directory and return observed files.
///
/// Instead of writing directly to the DB, returns observed library files via
/// the Result. The Witch accumulates these and passes them to
/// ReconcileLibraryFiles for set reconciliation.
pub fn execute_scan_library_directory(
    _read_only_db: &ReadOnlyDb<'_>,
    directory: &Path,
    library_name: &str,
    library_root: &Path,
    corpus_path_prefixes: &[PathBuf],
    _witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::ScanLibraryDirectory {
        directory: directory.to_path_buf(),
        library_name: library_name.to_string(),
        library_root: library_root.to_path_buf(),
        corpus_path_prefixes: corpus_path_prefixes.to_vec(),
    };

    // Collect audio files in this directory (non-recursive)
    let mut observed_files: Vec<super::ObservedLibraryFile> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && (is_audio_file(&path) || is_image_file(&path)) {
                if let Ok(metadata) = std::fs::metadata(&path) {
                    let mtime = metadata.modified().ok().and_then(|t| {
                        t.duration_since(std::time::UNIX_EPOCH).ok()
                    });
                    let (mtime_secs, mtime_nanos) = mtime
                        .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
                        .unwrap_or((0, 0));

                    // Compute stored_path: strip library_root prefix, prepend library_name
                    let library_relative = path.strip_prefix(library_root).unwrap_or(&path);
                    let stored_path = format!("{}/{}", library_name, library_relative.display());

                    observed_files.push(super::ObservedLibraryFile {
                        stored_path,
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
    if !observed_files.is_empty() {
        log_general(format!(
            "[COMPUTE] ScanLibraryDirectory '{}': {} files in {:?}",
            library_name, observed_files.len(), directory,
        ));
    }

    let _ = corpus_path_prefixes; // Suppress unused warning - needed for logging/future use

    Result::success_with_library_files(
        computation,
        start.elapsed().as_millis() as u64,
        Vec::new(),
        observed_files,
    )
}

// ============================================================================
// Library File Reconciliation
// ============================================================================

/// Reconcile observed library files against the DB.
///
/// Performs set reconciliation:
/// - Stale (in DB, not observed) → delete
/// - New (observed, not in DB) → upsert
/// - Changed (both, data differs) → upsert
/// - Unchanged (both, data matches) → skip
pub fn execute_reconcile_library_files(
    read_only_db: &ReadOnlyDb<'_>,
    observed_files: &[super::ObservedLibraryFile],
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::ReconcileLibraryFiles {
        observed_files: observed_files.to_vec(),
    };

    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Get existing library files from DB
    let existing = read_only_db.get_library_file_metadata().unwrap_or_default();

    // Build observed map: stored_path → ObservedLibraryFile
    let mut observed_map: std::collections::HashMap<&str, &super::ObservedLibraryFile> =
        std::collections::HashMap::with_capacity(observed_files.len());
    for file in observed_files {
        observed_map.insert(&file.stored_path, file);
    }

    let mut new_count = 0usize;
    let mut updated_count = 0usize;
    let mut stale_count = 0usize;
    let mut unchanged_count = 0usize;

    // Check observed files against DB
    for file in observed_files {
        if let Some(&(db_inode, db_mtime_s, db_mtime_ns, db_size)) = existing.get(&file.stored_path) {
            // Exists in DB - check if data matches
            if db_inode == file.inode
                && db_mtime_s == file.mtime_secs
                && db_mtime_ns == file.mtime_nanos
                && db_size == file.file_size
            {
                unchanged_count += 1;
            } else {
                // Data changed - upsert
                sender.upsert_library_file(
                    &file.stored_path,
                    file.inode,
                    file.mtime_secs,
                    file.mtime_nanos,
                    file.file_size,
                    witness,
                );
                updated_count += 1;
            }
        } else {
            // New file - upsert
            sender.upsert_library_file(
                &file.stored_path,
                file.inode,
                file.mtime_secs,
                file.mtime_nanos,
                file.file_size,
                witness,
            );
            new_count += 1;
        }
    }

    // Check for stale files (in DB but not observed)
    for db_path in existing.keys() {
        if !observed_map.contains_key(db_path.as_str()) {
            sender.delete_library_file(db_path, witness);
            stale_count += 1;
        }
    }

    log_general(format!(
        "[COMPUTE] ReconcileLibraryFiles: {} new, {} updated, {} stale, {} unchanged",
        new_count, updated_count, stale_count, unchanged_count
    ));

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

    let sender = match write_thread::signal_sender() {
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

    log_general(format!(
        "[COMPUTE] UpdateDeploySignals: corpus={} library={}",
        corpus_path_str, library_path_str,
    ));

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
    let had_deploy_ready = read_only_db.corpus_signal_exists::<DeployReadySignal>(corpus_inode);
    drop_stale_corpus_signal::<DeployReadySignal>(
        read_only_db,
        &sender,
        corpus_inode,
        witness,
    );

    // Clear SidecarDeployReady for this inode (no-op if inode is audio, clears if sidecar)
    drop_stale_corpus_signal::<SidecarDeployReadySignal>(
        read_only_db,
        &sender,
        corpus_inode,
        witness,
    );

    // Ensure DeployedHealthy with library_path (inode-keyed)
    let had_deployed_healthy = read_only_db.corpus_signal_exists::<DeployedHealthySignal>(corpus_inode);
    if !had_deployed_healthy {
        sender.write_typed_signal(
            TypedSignalWrite::DeployedHealthy(DeployedHealthySignal {
                inode: corpus_inode,
                path: corpus_path_str.clone(),
                library_path: library_path_str.clone(),
            }),
            witness,
        );
    }

    log_general(format!(
        "[COMPUTE] UpdateDeploySignals: inode={} DeployReady {} DeployedHealthy {}",
        corpus_inode,
        if had_deploy_ready { "CLEARED" } else { "absent" },
        if had_deployed_healthy { "already existed" } else { "WRITTEN" },
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

/// Clear library-side signals (LibraryLeftover, LibraryStale) for a library path.
///
/// Constructs exact keys from the library path (O(1) instead of scanning all keys).
/// `library_path` is "{library_name}/relative/path" (e.g., "libraries/music/Artist/track.opus"
/// or "music/Artist/track.opus" depending on caller).
fn clear_library_signals_for_path(
    sender: &write_thread::SignalWriteSender,
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
