//! Derivation-phase computation executors.
//!
//! These functions implement the actual logic for Derivation computations.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::corpus::paths;
use crate::db::write_thread;
use crate::db::ReadOnlyDb;
use crate::logging::log_general;
use crate::meta::computations::helpers::{
    drop_stale_corpus_signal, ensure_typed_signal, is_audio_file, is_image_file,
};
use crate::meta::computations::types::ComputationWitness;
use crate::meta::signals::data::*;
use crate::meta::signals::registry::TypedSignalWrite;
use crate::meta::signals::store::CorpusSignalStore;

use crate::db::types::Zone;
use crate::zones::{CorpusZone, DeriveZoneSignals};

use super::{Computation, Result};

// ============================================================================
// Shared Helpers
// ============================================================================

/// Convert an absolute path to a zone-relative string for DB storage.
fn to_zone_relative_str(resolver: &paths::PathResolver, path: &Path, zone: Zone) -> String {
    resolver
        .to_zone_relative(path, zone)
        .unwrap_or_else(|| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

/// Check if an inode has any out-of-band signal (tag conflict, tag sync, or mtime mismatch).
fn inode_has_oob_signal(read_only_db: &ReadOnlyDb<'_>, inode: i64) -> bool {
    read_only_db.corpus_signal_exists::<OutOfBandTagConflictSignal>(inode)
        || read_only_db.corpus_signal_exists::<OutOfBandTagSyncSignal>(inode)
        || read_only_db.corpus_signal_exists::<MtimeOnlyMismatchSignal>(inode)
}

/// Reconcile HealthyFile signal for an inode based on OOB signal presence.
///
/// If the inode has any OOB signal, clears any stale HealthyFile.
/// Otherwise, ensures a HealthyFile signal exists with the given path.
fn reconcile_healthy_file_signal(
    read_only_db: &ReadOnlyDb<'_>,
    sender: &write_thread::SignalWriteSender,
    inode: i64,
    path: &str,
    witness: &ComputationWitness,
) {
    if inode_has_oob_signal(read_only_db, inode) {
        drop_stale_corpus_signal::<HealthyFileSignal>(read_only_db, sender, inode, witness);
    } else {
        ensure_typed_signal(
            read_only_db,
            sender,
            TypedSignalWrite::HealthyFile(HealthyFileSignal {
                inode,
                path: path.to_string(),
            }),
            witness,
        );
    }
}

/// GC orphaned signals from multiple signal tables in one call.
macro_rules! gc_signal_tables {
    ($read_only_db:expr, $sender:expr, $known_inodes:expr, $witness:expr, [ $($signal:ty),+ $(,)? ]) => {{
        let mut total = 0usize;
        $(
            total += gc_signal_table::<$signal>($read_only_db, $sender, $known_inodes, $witness);
        )+
        total
    }};
}

// ============================================================================
// Second-Level Signal Derivations
// ============================================================================

/// Detect missing indexed directories and schedule library walks.
pub fn execute_schedule_second_level_derivations(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
) -> Result {
    log_general("[COMPUTE] ScheduleSecondLevelDerivations: starting");

    let sender = require_sender!(Computation::ScheduleSecondLevelDerivations);

    // ========================================================================
    // Detect Missing Directories
    // ========================================================================
    // Check indexed directories against disk to emit/clear MissingDirectory signals
    let indexed_dirs = read_only_db
        .get_indexed_corpus_directories()
        .unwrap_or_default();
    let resolver = paths::get_resolver();

    let mut missing_dir_count = 0;
    let mut existing_dir_count = 0;

    for (indexed_dir, inode) in &indexed_dirs {
        // Paths in DB are zone-relative (e.g., "physical/cd/...")
        // Use resolver.resolve_for_zone() to get absolute path
        let abs_path = resolver.resolve_for_zone(Zone::Corpus, indexed_dir);
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

    // Library walks are no longer needed — the watcher observes the library
    // zone directly and the Witch queues ReconcileLibraryFiles from watcher data.

    Result::success(
        Computation::ScheduleSecondLevelDerivations,
        Vec::new(),
    )
}

// ============================================================================
// Zone-Generic Signal Derivation
// ============================================================================

/// Derive zone signals via global inode set comparison.
///
/// Compares disk inodes (from file-presence signals) against indexed inodes:
/// - disk_only = disk - indexed → unindexed signals
/// - index_only = indexed - disk → zone-specific gone handling
/// - both = disk ∩ indexed → zone-specific present handling
///
/// Zone-specific behavior is encoded in the `DeriveZoneSignals` trait.
fn derive_zone_signals<Z: DeriveZoneSignals>(
    read_only_db: &ReadOnlyDb<'_>,
    observed_inodes: HashMap<i64, crate::witch::ObservedInodeMeta>,
    sender: &write_thread::SignalWriteSender,
    witness: &ComputationWitness,
    computation: Computation,
) -> Result {
    let zone = Z::ZONE_STR;
    log_general(format!(
        "[COMPUTE] Derive({zone}): starting global inode comparison"
    ));

    // ========================================================================
    // Reconcile file-presence signals against observed disk state
    // ========================================================================
    let existing = read_only_db
        .get_file_presence_inodes::<Z>()
        .unwrap_or_default();
    let mut fp_new = 0usize;
    let mut fp_stale = 0usize;

    for (inode, meta) in &observed_inodes {
        if !existing.contains_key(inode) {
            sender.write_typed_signal(
                Z::file_presence_signal(*inode, meta.path.clone(), 0),
                witness,
            );
            fp_new += 1;
        }
    }

    for inode in existing.keys() {
        if !observed_inodes.contains_key(inode) {
            sender.clear_corpus_signal::<Z::FilePresenceSignal>(*inode, witness);
            fp_stale += 1;
        }
    }

    if fp_new > 0 || fp_stale > 0 {
        log_general(format!(
            "[COMPUTE] Derive({zone}): file-presence reconciled: {fp_new} new, {fp_stale} stale cleared"
        ));
    }

    // ========================================================================
    // Get indexed state and compute set operations
    // ========================================================================
    let disk_inodes = observed_inodes;

    let indexed_inodes = match read_only_db.get_all_inodes::<Z>() {
        Ok(inodes) => inodes,
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to get indexed {zone} inodes: {e}"),
            );
        }
    };

    log_general(format!(
        "[COMPUTE] Derive({zone}): {} disk inodes, {} indexed inodes",
        disk_inodes.len(),
        indexed_inodes.len()
    ));

    let disk_set: HashSet<i64> = disk_inodes.keys().copied().collect();
    let indexed_set: HashSet<i64> = indexed_inodes.keys().copied().collect();

    let disk_only: Vec<i64> = disk_set.difference(&indexed_set).copied().collect();
    let index_only: Vec<i64> = indexed_set.difference(&disk_set).copied().collect();
    let both: Vec<i64> = disk_set.intersection(&indexed_set).copied().collect();

    // ========================================================================
    // Emit unindexed signals for audio files on disk but not indexed.
    // Image files in disk_only are expected — they get indexed separately
    // by IndexObservedImages, not through the audio indexing pipeline.
    //
    // Cross-zone move detection: if the inode is indexed in a different zone,
    // emit MovedFileSignal to inform the operator.
    // ========================================================================
    let mut moved = 0usize;
    for inode in &disk_only {
        if let Some(meta) = disk_inodes.get(inode) {
            if is_image_file(Path::new(&meta.path)) {
                continue;
            }

            // Check if this inode is indexed in a different zone (cross-zone move)
            if let Ok(Some((old_zone_str, old_path))) =
                read_only_db.get_file_zone_and_path_by_inode(*inode)
            {
                if old_zone_str != zone {
                    ensure_typed_signal(
                        read_only_db,
                        sender,
                        TypedSignalWrite::MovedFile(MovedFileSignal {
                            inode: *inode,
                            path: meta.path.clone(),
                            old_path,
                            old_zone: old_zone_str,
                            new_zone: zone.to_string(),
                        }),
                        witness,
                    );
                    moved += 1;
                }
            }

            ensure_typed_signal(
                read_only_db,
                sender,
                Z::unindexed_signal(*inode, meta.path.clone()),
                witness,
            );
        }
    }

    // ========================================================================
    // Handle indexed files no longer on disk (zone-specific)
    // ========================================================================
    for inode in &index_only {
        if let Some(path) = indexed_inodes.get(inode) {
            Z::on_file_gone(*inode, path, read_only_db, sender, witness);
        }
    }

    // ========================================================================
    // Reconcile files present on both disk and index (zone-specific)
    //
    // Same-zone move detection: if disk path differs from indexed path,
    // the file was renamed/moved within the zone.
    // ========================================================================
    for inode in &both {
        let disk_path = disk_inodes.get(inode).map(|m| m.path.as_str()).unwrap_or("");
        let indexed_path = indexed_inodes.get(inode).map(|p| p.as_str()).unwrap_or("");

        if !disk_path.is_empty() && !indexed_path.is_empty() && disk_path != indexed_path {
            ensure_typed_signal(
                read_only_db,
                sender,
                TypedSignalWrite::MovedFile(MovedFileSignal {
                    inode: *inode,
                    path: disk_path.to_string(),
                    old_path: indexed_path.to_string(),
                    old_zone: zone.to_string(),
                    new_zone: zone.to_string(),
                }),
                witness,
            );
            moved += 1;
        }

        Z::on_file_present(*inode, disk_path, read_only_db, sender, witness);
    }

    if moved > 0 {
        log_general(format!(
            "[COMPUTE] Derive({zone}): detected {moved} moved file(s)"
        ));
    }

    log_general(format!(
        "[COMPUTE] Derive({zone}): {} unindexed, {} gone, {} present",
        disk_only.len(),
        index_only.len(),
        both.len()
    ));

    // ========================================================================
    // GC Backstop: Clear orphaned signals for inodes no longer known
    // ========================================================================
    let known_inodes = Z::known_inodes_for_gc(&disk_set, &indexed_set);
    let gc_total = Z::gc_orphaned_signals(read_only_db, sender, &known_inodes, witness);
    if gc_total > 0 {
        log_general(format!(
            "[COMPUTE] Derive({zone}): GC cleared {gc_total} orphaned signal(s)"
        ));
    }

    log_general(format!("[COMPUTE] Derive({zone}): complete"));
    Result::success(computation, Vec::new())
}

/// Derive corpus signals via global inode set comparison.
pub fn execute_derive_corpus_signals(
    read_only_db: &ReadOnlyDb<'_>,
    observed_inodes: HashMap<i64, crate::witch::ObservedInodeMeta>,
    witness: &ComputationWitness,
) -> Result {
    let sender = require_sender!(Computation::DeriveCorpusSignals {
        observed_inodes: HashMap::new(),
    });
    derive_zone_signals::<CorpusZone>(
        read_only_db,
        observed_inodes,
        &sender,
        witness,
        Computation::DeriveCorpusSignals {
            observed_inodes: HashMap::new(),
        },
    )
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
            cleared,
            S::TABLE_NAME
        ));
    }
    cleared
}

// ============================================================================
// DeriveZoneSignals Implementations
// ============================================================================

impl DeriveZoneSignals for CorpusZone {
    fn on_file_gone(
        inode: i64,
        path: &str,
        read_only_db: &ReadOnlyDb<'_>,
        sender: &write_thread::SignalWriteSender,
        witness: &ComputationWitness,
    ) {
        ensure_typed_signal(
            read_only_db,
            sender,
            TypedSignalWrite::MissingFile(MissingFileSignal {
                inode,
                path: path.to_string(),
                replaced_by_inode: None,
            }),
            witness,
        );
        drop_stale_corpus_signal::<HealthyFileSignal>(read_only_db, sender, inode, witness);
    }

    fn should_mark_healthy(inode: i64, read_only_db: &ReadOnlyDb<'_>) -> bool {
        !inode_has_oob_signal(read_only_db, inode)
    }

    fn gc_orphaned_signals(
        read_only_db: &ReadOnlyDb<'_>,
        sender: &write_thread::SignalWriteSender,
        known_inodes: &HashSet<i64>,
        witness: &ComputationWitness,
    ) -> usize {
        // FileInCorpus excluded: it IS the disk observation, always part of known_inodes
        gc_signal_tables!(read_only_db, sender, known_inodes, witness, [
            UnindexedFileSignal,
            MissingFileSignal,
            MovedFileSignal,
            HealthyFileSignal,
            CorruptFileSignal,
            LosslessRemuxSignal,
            MtimeOnlyMismatchSignal,
            OutOfBandTagSyncSignal,
            OutOfBandTagConflictSignal,
            SubparDuplicateSignal,
            CompoundTagSignal,
            DeployReadySignal,
            DeployedHealthySignal,
            SidecarDeployReadySignal,
            MissingDirectorySignal,
            ExternalMatchSignal,
            ReleasePackingSignal,
            UnmatchedCorpusTrackSignal,
        ])
    }

    fn on_file_present(
        inode: i64,
        path: &str,
        read_only_db: &ReadOnlyDb<'_>,
        sender: &write_thread::SignalWriteSender,
        witness: &ComputationWitness,
    ) {
        drop_stale_corpus_signal::<MissingFileSignal>(read_only_db, sender, inode, witness);
        drop_stale_corpus_signal::<UnindexedFileSignal>(read_only_db, sender, inode, witness);
        reconcile_healthy_file_signal(read_only_db, sender, inode, path, witness);
    }

    fn known_inodes_for_gc(disk_set: &HashSet<i64>, indexed_set: &HashSet<i64>) -> HashSet<i64> {
        disk_set.union(indexed_set).copied().collect()
    }
}

/// Update corpus signals for a single file after a mutation.
///
/// Only valid for paths within the corpus directory.
/// Uses inode-keyed signals consistent with the global observation system.
pub fn execute_update_corpus_file_signals(
    read_only_db: &ReadOnlyDb<'_>,
    path: &Path,
    witness: &ComputationWitness,
) -> Result {
    let computation = Computation::UpdateCorpusFileSignals {
        path: path.to_path_buf(),
    };

    let sender = require_sender!(computation);

    // Convert absolute path to zone-relative for DB queries
    let resolver = paths::get_resolver();
    let path_str = to_zone_relative_str(resolver, path, Zone::Corpus);

    let file_exists = path.exists() && is_audio_file(path);

    // Get inode from disk or database
    let disk_inode = if file_exists {
        std::fs::metadata(path).ok().map(|m| m.ino() as i64)
    } else {
        None
    };

    // Check if path is indexed (and get the indexed inode)
    let indexed_info = read_only_db
        .get_audio_file_by_path(&path_str)
        .ok()
        .flatten();
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
            drop_stale_corpus_signal::<UnindexedFileSignal>(read_only_db, &sender, inode, witness);
            drop_stale_corpus_signal::<MissingFileSignal>(read_only_db, &sender, inode, witness);

            reconcile_healthy_file_signal(read_only_db, &sender, inode, &path_str, witness);
        } else {
            // File not indexed - mark as unindexed
            drop_stale_corpus_signal::<HealthyFileSignal>(read_only_db, &sender, inode, witness);
            drop_stale_corpus_signal::<MissingFileSignal>(read_only_db, &sender, inode, witness);
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
        drop_stale_corpus_signal::<FileInCorpusSignal>(read_only_db, &sender, inode, witness);
        drop_stale_corpus_signal::<UnindexedFileSignal>(read_only_db, &sender, inode, witness);
        drop_stale_corpus_signal::<HealthyFileSignal>(read_only_db, &sender, inode, witness);
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

    Result::success(computation, Vec::new())
}

/// Update library signals for a single file after a mutation.
///
/// Only valid for paths within library directories.
/// Handles LibraryLeftover signals when files are added/removed from libraries.
pub fn execute_update_library_file_signals(
    _read_only_db: &ReadOnlyDb<'_>,
    path: &Path,
    witness: &ComputationWitness,
) -> Result {
    let computation = Computation::UpdateLibraryFileSignals {
        path: path.to_path_buf(),
    };

    let sender = require_sender!(computation);

    // Convert absolute path to zone-relative for signal keys
    let resolver = paths::get_resolver();
    let path_str = to_zone_relative_str(resolver, path, Zone::Library);

    // For library files, we check if the file exists and clear any leftover/stale signals
    // The full library health is recomputed during the Analysis phase
    if path.exists() && is_audio_file(path) {
        // File exists - clear any LibraryLeftover/LibraryStale for this path
        clear_library_signals_for_path(&sender, &path_str, witness);
    }
    // If file doesn't exist, LibraryLeftover signals will be created during
    // the next full library scan in the Analysis phase

    Result::success(computation, Vec::new())
}

// ============================================================================
// Library Health Scanning
// ============================================================================

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
) -> Result {
    let computation = Computation::ReconcileLibraryFiles {
        observed_files: observed_files.to_vec(),
    };

    let sender = require_sender!(computation);

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
        if let Some(&(db_inode, db_mtime_s, db_mtime_ns, db_size)) = existing.get(&file.stored_path)
        {
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

    Result::success(computation, Vec::new())
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
) -> Result {
    let computation = Computation::UpdateDeploySignals {
        corpus_path: corpus_path.to_path_buf(),
        library_path: library_path.to_path_buf(),
    };

    let sender = require_sender!(computation);

    // Convert absolute paths to zone-relative for DB queries and signal keys
    let resolver = paths::get_resolver();
    let library_path_str = to_zone_relative_str(resolver, library_path, Zone::Library);
    let corpus_path_str = to_zone_relative_str(resolver, corpus_path, Zone::Corpus);

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
                format!("Corpus file not in index: {}", corpus_path_str),
            );
        }
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to look up corpus file: {}", e),
            );
        }
    };

    // Clear DeployReady for corpus file (inode-keyed)
    let had_deploy_ready = read_only_db.corpus_signal_exists::<DeployReadySignal>(corpus_inode);
    drop_stale_corpus_signal::<DeployReadySignal>(read_only_db, &sender, corpus_inode, witness);

    // Clear SidecarDeployReady for this inode (no-op if inode is audio, clears if sidecar)
    drop_stale_corpus_signal::<SidecarDeployReadySignal>(
        read_only_db,
        &sender,
        corpus_inode,
        witness,
    );

    // Ensure DeployedHealthy with library_path (inode-keyed)
    let had_deployed_healthy =
        read_only_db.corpus_signal_exists::<DeployedHealthySignal>(corpus_inode);
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
        if had_deploy_ready {
            "CLEARED"
        } else {
            "absent"
        },
        if had_deployed_healthy {
            "already existed"
        } else {
            "WRITTEN"
        },
    ));

    Result::success(computation, Vec::new())
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
    let effective_path = library_path
        .strip_prefix("libraries/")
        .unwrap_or(library_path);
    if let Some(library_name) = effective_path.split('/').next() {
        let leftover_key = LibraryLeftoverSignal::make_key(library_name, effective_path);
        sender.clear_aggregate_signal::<LibraryLeftoverSignal>(&leftover_key, witness);
        let stale_key = LibraryStaleSignal::make_key(library_name, effective_path);
        sender.clear_aggregate_signal::<LibraryStaleSignal>(&stale_key, witness);
    }
}
