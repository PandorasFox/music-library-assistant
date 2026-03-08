//! Observation-phase computation executors.
//!
//! These functions implement the actual logic for Observation computations.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::corpus::paths;
use crate::db::types::Zone;
use crate::db::write_thread;
use crate::db::ReadOnlyDb;
use crate::logging::log_general;
use crate::meta::computations::helpers::{
    drop_stale_corpus_signal, ensure_typed_signal, enumerate_all_directories, extract_mtime,
    is_audio_file, is_image_file,
};
use crate::meta::computations::types::ComputationWitness;
use crate::meta::signals::data::*;

use super::{Computation, Result};

// ============================================================================
// Phase 1: Walk Corpus
// ============================================================================

/// Phase 1: Enumerate ALL corpus directories and spawn per-directory scans.
pub fn execute_walk_corpus(
    _read_only_db: &ReadOnlyDb<'_>,
    root: &Path,
    zone: &str,
    force_check: bool,
    start: Instant,
) -> Result {
    let computation = Computation::WalkCorpus {
        root: root.to_path_buf(),
        zone: zone.to_string(),
        force_check,
    };

    if !root.exists() {
        return Result::failure(
            computation,
            start.elapsed().as_millis() as u64,
            format!("Root directory does not exist: {:?}", root),
        );
    }

    // Recursively enumerate ALL directories
    let (directories, symlink_count) = enumerate_all_directories(root);

    if symlink_count > 0 {
        log_general(format!(
            "[WARN] WalkCorpus: skipped {} directory symlinks in {:?}",
            symlink_count, root
        ));
    }

    log_general(format!(
        "[COMPUTE] WalkCorpus: found {} directories in {:?}{}",
        directories.len(),
        root,
        if force_check {
            " (force_check=true)"
        } else {
            ""
        }
    ));

    // Spawn ScanCorpusDirectory for each directory
    let spawn: Vec<Computation> = directories
        .into_iter()
        .map(|directory| Computation::ScanCorpusDirectory {
            directory,
            zone: zone.to_string(),
            force_check,
        })
        .collect();

    Result::success(computation, start.elapsed().as_millis() as u64, spawn)
}

// ============================================================================
// Scan Single Directory
// ============================================================================

/// Scan a single corpus directory (non-recursive).
pub fn execute_scan_corpus_directory(
    read_only_db: &ReadOnlyDb<'_>,
    directory: &Path,
    zone: &str,
    force_check: bool,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::ScanCorpusDirectory {
        directory: directory.to_path_buf(),
        zone: zone.to_string(),
        force_check,
    };

    // Get signal sender for async writes
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

    if !directory.exists() {
        return Result::failure(
            computation,
            start.elapsed().as_millis() as u64,
            format!("Directory does not exist: {:?}", directory),
        );
    }

    let resolver = paths::get_resolver();
    let file_zone = Zone::from_str(zone).unwrap_or(Zone::Corpus);

    // Index this directory in the files table
    index_directory(read_only_db, &sender, directory, zone, resolver, witness);

    // Collect disk state for files DIRECTLY in this directory (not recursive)
    let disk_state = collect_directory_files(directory);

    // If no files in directory, nothing to do
    if disk_state.is_empty() {
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    // Build lookup map for disk inodes
    let disk_inodes: HashSet<i64> = disk_state.iter().map(|(inode, _, _, _)| *inode).collect();

    // Get indexed inodes from files table for comparison (mtime info)
    let inode_vec: Vec<i64> = disk_inodes.iter().copied().collect();
    let indexed_by_inode = read_only_db
        .get_file_mtime_batch(file_zone, &inode_vec)
        .unwrap_or_default();

    // Get indexed paths for move detection (same inode, different path)
    let indexed_paths = read_only_db
        .get_file_paths_batch(file_zone, &inode_vec)
        .unwrap_or_default();

    let mut spawn: Vec<Computation> = Vec::new();
    let mut observed_corpus_inodes: HashMap<i64, String> = HashMap::new();
    let mut observed_inbox_inodes: HashMap<i64, String> = HashMap::new();

    // Process each file found on disk
    for (inode, path, disk_mtime_s, disk_mtime_ns) in &disk_state {
        // Convert absolute path to relative for DB queries and signal keys
        let relative_path = match resolver.to_relative(path) {
            Some(rel) => rel,
            None => {
                // Path doesn't match expected root - skip
                continue;
            }
        };
        let relative_path_str = relative_path.to_string_lossy().to_string();

        // Track this inode as observed on disk (accumulated by the Witch for
        // reconciliation in DeriveCorpusSignals / DeriveInboxSignals). Also
        // write the DB signal if it doesn't already exist (existence check
        // avoids redundant writes).
        match file_zone {
            Zone::Corpus => {
                observed_corpus_inodes.insert(*inode, relative_path_str.clone());
                ensure_typed_signal(
                    read_only_db,
                    &sender,
                    TypedSignalWrite::FileInCorpus(FileInCorpusSignal {
                        inode: *inode,
                        path: relative_path_str.clone(),
                        generation: 0,
                    }),
                    witness,
                );
            }
            Zone::Inbox => {
                observed_inbox_inodes.insert(*inode, relative_path_str.clone());
                ensure_typed_signal(
                    read_only_db,
                    &sender,
                    TypedSignalWrite::FileInInbox(FileInInboxSignal {
                        inode: *inode,
                        path: relative_path_str.clone(),
                        generation: 0,
                    }),
                    witness,
                );
            }
            _ => {}
        }

        // Check if file is indexed and needs verification
        // indexed_by_inode returns HashMap<inode, (mtime_secs, mtime_nanos)>
        if let Some((db_mtime_secs, db_mtime_nanos)) = indexed_by_inode.get(inode) {
            // File is indexed by inode in this zone - check if path changed (same-zone move)
            if let Some(db_path) = indexed_paths.get(inode) {
                if db_path != &relative_path_str {
                    // Same inode but different path - file was moved within zone
                    let zone_str = file_zone.as_str().to_string();
                    if !read_only_db.corpus_signal_exists::<MovedFileSignal>(*inode) {
                        sender.write_typed_signal(
                            TypedSignalWrite::MovedFile(MovedFileSignal {
                                inode: *inode,
                                path: relative_path_str.clone(),
                                old_path: db_path.to_string(),
                                old_zone: zone_str.clone(),
                                new_zone: zone_str,
                            }),
                            witness,
                        );
                    }
                }
            }

            let mtime_changed =
                *db_mtime_secs != *disk_mtime_s || *db_mtime_nanos != *disk_mtime_ns;

            if force_check {
                // Force-check mode: skip mtime optimization, verify all indexed files directly
                // Verify tags (metadata)
                spawn.push(Computation::VerifyTags {
                    inode: *inode,
                    path: path.clone(),
                });

                // Also verify audio integrity (decode entire file)
                // This catches truncated/corrupt files that tag verification misses
                spawn.push(Computation::VerifyAudio {
                    inode: *inode,
                    path: path.clone(),
                });
            } else if mtime_changed {
                // Normal mode: only verify if mtime changed
                // Mtime mismatched - verify tags via VerifyMtime
                // Note: path in Computation is still absolute for filesystem operations
                spawn.push(Computation::VerifyMtime {
                    inode: *inode,
                    path: path.clone(),
                    expected_mtime_secs: *db_mtime_secs,
                    expected_mtime_nanos: *db_mtime_nanos,
                });
            }
        } else {
            // Inode not indexed in this zone — check for cross-zone move first.
            // If the inode is indexed in a different zone, it was moved between zones
            // (e.g. inbox→corpus or corpus→inbox).
            if let Ok(Some((other_zone_str, old_path))) =
                read_only_db.get_file_zone_and_path_by_inode(*inode)
            {
                let other_zone = Zone::from_str(&other_zone_str).unwrap_or(Zone::Corpus);
                if other_zone != file_zone {
                    // Cross-zone move detected
                    if !read_only_db.corpus_signal_exists::<MovedFileSignal>(*inode) {
                        sender.write_typed_signal(
                            TypedSignalWrite::MovedFile(MovedFileSignal {
                                inode: *inode,
                                path: relative_path_str.clone(),
                                old_path,
                                old_zone: other_zone_str,
                                new_zone: file_zone.as_str().to_string(),
                            }),
                            witness,
                        );
                    }
                }
            } else if let Ok(Some(audio_file)) =
                read_only_db.get_audio_file_by_path(&relative_path_str)
            {
                // Not indexed anywhere in corpus/inbox — check if path is indexed
                // with a different inode. This detects file replacement (same path, new inode).
                if audio_file.inode() != *inode {
                    // Inode changed! File was replaced.
                    // Instead of emitting InodeChanged (obsolete), we emit:
                    // 1. MissingFile for the old inode (it no longer exists on disk)
                    // 2. The new file will be picked up as UnindexedFile during awakening
                    let old_inode = audio_file.inode();

                    // Emit MissingFile for the old inode (file at that inode is gone)
                    if !read_only_db.corpus_signal_exists::<MissingFileSignal>(old_inode) {
                        sender.write_typed_signal(
                            TypedSignalWrite::MissingFile(MissingFileSignal {
                                inode: old_inode,
                                path: relative_path_str.clone(),
                                replaced_by_inode: Some(*inode),
                            }),
                            witness,
                        );
                    }

                    // Spawn VerifyTags on the new inode to check for tag differences
                    spawn.push(Computation::VerifyTags {
                        inode: *inode,
                        path: path.clone(),
                    });
                }
            }
        }
    }

    // =====================================================================
    // Image file discovery (corpus zone only)
    // =====================================================================
    // Register sidecar image files in the files table and mark them dirty
    // for IndexImageFile. Do NOT emit FileInCorpusSignal — that would
    // contaminate the audio indexing pipeline.
    if file_zone == Zone::Corpus {
        let image_state = collect_directory_image_files(directory);

        if !image_state.is_empty() {
            // Build mtime + image_info existence lookups for freshness gating
            let img_inodes: Vec<i64> = image_state
                .iter()
                .map(|(inode, _, _, _, _)| *inode)
                .collect();
            let img_mtimes = read_only_db
                .get_file_mtime_batch(file_zone, &img_inodes)
                .unwrap_or_default();
            let img_info_exists = read_only_db
                .get_image_info_exists_batch(&img_inodes)
                .unwrap_or_default();

            for (img_inode, img_path, img_mtime_s, img_mtime_ns, img_file_size) in &image_state {
                let img_relative = match resolver.to_relative(img_path) {
                    Some(rel) => rel,
                    None => continue,
                };
                let img_relative_str = img_relative.to_string_lossy().to_string();

                // Track as observed so MissingFileSignal is not emitted for it
                observed_corpus_inodes.insert(*img_inode, img_relative_str.clone());

                // Register in files table (zone='corpus', is_dir=0) — always, keeps path/mtime fresh
                sender.index_image_file(
                    &img_relative_str,
                    zone,
                    *img_inode,
                    *img_mtime_s,
                    *img_mtime_ns,
                    *img_file_size,
                    witness,
                );

                // Freshness gate: skip dirty marking if mtime matches AND image_info exists
                let mtime_matches = img_mtimes
                    .get(img_inode)
                    .is_some_and(|(db_s, db_ns)| *db_s == *img_mtime_s && *db_ns == *img_mtime_ns);
                let has_image_info = img_info_exists.contains(img_inode);

                if mtime_matches && has_image_info {
                    continue;
                }

                // Mark dirty for IndexImageFile computation
                sender.mark_dirty_inodes(vec![*img_inode], "index_image_file", witness);

                // Mark dirty for sidecar deploy recomputation
                sender.mark_dirty_inodes(vec![*img_inode], "sidecar_deploy", witness);
            }
        }
    }

    Result::success_with_observations(
        computation,
        start.elapsed().as_millis() as u64,
        spawn,
        observed_corpus_inodes,
        observed_inbox_inodes,
    )
}

/// Collect audio files directly in a directory (non-recursive).
pub fn collect_directory_files(dir: &Path) -> Vec<(i64, PathBuf, i64, i64)> {
    let mut disk_state = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return disk_state,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Skip directories and symlinks
        if path.is_dir() || path.is_symlink() {
            continue;
        }

        if is_audio_file(&path) {
            if let Ok(metadata) = std::fs::metadata(&path) {
                let inode = metadata.ino() as i64;
                let mtime = extract_mtime(&metadata);

                disk_state.push((inode, path, mtime.0, mtime.1));
            }
        }
    }

    disk_state
}

/// Collect image files directly in a directory (non-recursive).
///
/// Parallel to `collect_directory_files` but for image files.
pub fn collect_directory_image_files(dir: &Path) -> Vec<(i64, PathBuf, i64, i64, i64)> {
    let mut disk_state = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return disk_state,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() || path.is_symlink() {
            continue;
        }

        if is_image_file(&path) {
            if let Ok(metadata) = std::fs::metadata(&path) {
                let inode = metadata.ino() as i64;
                let mtime = extract_mtime(&metadata);
                let file_size = metadata.len() as i64;

                disk_state.push((inode, path, mtime.0, mtime.1, file_size));
            }
        }
    }

    disk_state
}

/// Index the current directory in the files table.
///
/// This is called during corpus scanning to track directory entries.
/// Uses a read guard to skip writes when the directory entry already
/// matches (same zone, inode, mtime), eliminating ~D NOP writes per cycle.
fn index_directory(
    read_only_db: &ReadOnlyDb<'_>,
    sender: &write_thread::SignalWriteSender,
    directory: &Path,
    zone: &str,
    resolver: &crate::corpus::paths::PathResolver,
    witness: &ComputationWitness,
) {
    // Get directory metadata
    let dir_metadata = match std::fs::metadata(directory) {
        Ok(m) => m,
        Err(_) => return,
    };

    let dir_inode = dir_metadata.ino() as i64;
    let (mtime_secs, mtime_nanos) = extract_mtime(&dir_metadata);

    // Read guard: skip write if entry already matches
    if read_only_db.directory_entry_fresh(zone, dir_inode, mtime_secs, mtime_nanos) {
        return;
    }

    // Convert to relative path for DB storage
    let relative_dir = match resolver.to_relative(directory) {
        Some(rel) => rel,
        None => return,
    };
    let relative_dir_str = relative_dir.to_string_lossy().to_string();

    sender.index_directory(
        &relative_dir_str,
        zone,
        dir_inode,
        mtime_secs,
        mtime_nanos,
        witness,
    );
}

// ============================================================================
// Verify Mtime
// ============================================================================

/// Phase 3: Verify single file mtime.
pub fn execute_verify_mtime(
    _read_only_db: &ReadOnlyDb<'_>,
    inode: i64,
    path: &Path,
    expected_mtime_secs: i64,
    expected_mtime_nanos: i64,
    start: Instant,
) -> Result {
    let computation = Computation::VerifyMtime {
        inode,
        path: path.to_path_buf(),
        expected_mtime_secs,
        expected_mtime_nanos,
    };

    // Read current mtime from disk
    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to read file metadata: {}", e),
            );
        }
    };

    let (current_secs, current_nanos) = extract_mtime(&metadata);

    // If mtime differs from expected, spawn VerifyTags to check actual content
    let spawn = if current_secs != expected_mtime_secs || current_nanos != expected_mtime_nanos {
        vec![Computation::VerifyTags {
            inode,
            path: path.to_path_buf(),
        }]
    } else {
        Vec::new()
    };

    Result::success(computation, start.elapsed().as_millis() as u64, spawn)
}

/// Check if a file's disk mtime differs from what's stored in files table.
///
/// Uses the portable API (extract_mtime) for consistency with ScanCorpusDirectory.
/// Returns true if mtime differs or if we can't determine (fail-safe to emit signal).
fn check_mtime_differs(read_only_db: &ReadOnlyDb<'_>, inode: i64, path: &Path) -> bool {
    // Get mtime info from files table for this inode
    let mtime_info = match read_only_db.get_file_mtime_batch(Zone::Corpus, &[inode]) {
        Ok(map) => match map.get(&inode) {
            Some((db_secs, db_nanos)) => (*db_secs, *db_nanos),
            None => return true, // Not in files table, assume differs
        },
        Err(_) => return true, // Query failed, assume differs
    };

    // Get current disk mtime using portable API (same as ScanCorpusDirectory)
    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return true, // Can't read file, assume differs
    };
    let (disk_secs, disk_nanos) = extract_mtime(&metadata);

    // Compare
    mtime_info.0 != disk_secs || mtime_info.1 != disk_nanos
}

// ============================================================================
// Verify Tags
// ============================================================================

/// Verify tags on disk match database and classify OOB changes.
///
/// Performs full classification of out-of-band changes directly:
/// - `MtimeOnlyMismatch`: mtime changed but tags are identical (requires operator acknowledgement)
/// - `OutOfBandTagSync`: one-direction tag extras only (syncable without conflict)
/// - `OutOfBandTagConflict`: value conflicts or mixed-direction extras (requires operator decision)
///
/// These three signal types are mutually exclusive - emitting one clears the others.
pub fn execute_verify_tags(
    read_only_db: &ReadOnlyDb<'_>,
    inode: i64,
    path: &Path,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::meta::mutations::indexing;

    let computation = Computation::VerifyTags {
        inode,
        path: path.to_path_buf(),
    };

    // Route mismatch writes through db_thread (read-only connection can't write directly)
    let sender = match crate::db::write_thread::signal_sender().cloned() {
        Some(s) => s,
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Wait for pending DB writes to drain before reading audio_info.
    // This prevents a race where VerifyTags runs before IndexFileFromPath's
    // fire-and-forget write is processed, causing spurious "Audio file not found" errors.
    crate::db::write_thread::wait_for_queue_drain();

    // Get relative path for signal keys
    let resolver = crate::corpus::paths::get_resolver();
    let rel_path = match resolver.to_relative(path) {
        Some(rel) => rel,
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Path {} does not match any configured root", path.display()),
            );
        }
    };
    let rel_str = rel_path.to_string_lossy().to_string();

    match indexing::execute_verify_tags(read_only_db, inode, path) {
        Ok(verify_result) => {
            // Full classification based on TagVerifyResult
            // All signals are now keyed by inode with path in metadata
            if verify_result.is_clean() {
                // Tags match exactly - but we need to check if mtime actually differs
                // (in force_check mode, VerifyTags runs even when mtime matches)
                let mtime_actually_differs = check_mtime_differs(read_only_db, inode, path);

                if mtime_actually_differs {
                    // Mtime changed but tags are identical - requires operator acknowledgement
                    log_general(format!(
                        "[COMPUTE] VerifyTags: mtime-only change for inode {} ({})",
                        inode,
                        path.display()
                    ));
                    ensure_typed_signal(
                        read_only_db,
                        &sender,
                        TypedSignalWrite::MtimeOnlyMismatch(MtimeOnlyMismatchSignal {
                            inode,
                            path: rel_str.clone(),
                        }),
                        witness,
                    );
                    // Clear mutually exclusive signals
                    drop_stale_corpus_signal::<OutOfBandTagConflictSignal>(
                        read_only_db,
                        &sender,
                        inode,
                        witness,
                    );
                    drop_stale_corpus_signal::<OutOfBandTagSyncSignal>(
                        read_only_db,
                        &sender,
                        inode,
                        witness,
                    );
                } else {
                    // Tags match AND mtime matches - file is healthy, clear all OOB signals
                    log_general(format!(
                        "[COMPUTE] VerifyTags: file healthy for inode {} ({})",
                        inode,
                        path.display()
                    ));
                    drop_stale_corpus_signal::<MtimeOnlyMismatchSignal>(
                        read_only_db,
                        &sender,
                        inode,
                        witness,
                    );
                    drop_stale_corpus_signal::<OutOfBandTagConflictSignal>(
                        read_only_db,
                        &sender,
                        inode,
                        witness,
                    );
                    drop_stale_corpus_signal::<OutOfBandTagSyncSignal>(
                        read_only_db,
                        &sender,
                        inode,
                        witness,
                    );
                }
            } else if verify_result.is_conflict() {
                // Value conflicts or mixed-direction extras - requires operator decision
                log_general(format!(
                    "[COMPUTE] VerifyTags: tag conflict for inode {} ({})",
                    inode,
                    path.display()
                ));
                // Clear all OOB signals first (including target type to refresh metadata)
                drop_stale_corpus_signal::<OutOfBandTagConflictSignal>(
                    read_only_db,
                    &sender,
                    inode,
                    witness,
                );
                drop_stale_corpus_signal::<OutOfBandTagSyncSignal>(
                    read_only_db,
                    &sender,
                    inode,
                    witness,
                );
                drop_stale_corpus_signal::<MtimeOnlyMismatchSignal>(
                    read_only_db,
                    &sender,
                    inode,
                    witness,
                );
                // Create fresh signal with mismatch metadata (keyed by inode)
                let mismatches: Vec<TagMismatchEntry> = verify_result
                    .mismatches
                    .into_iter()
                    .map(|m| TagMismatchEntry {
                        tag_name: m.field,
                        disk_value: m.disk_value,
                        db_value: m.db_value,
                    })
                    .collect();
                sender.write_typed_signal(
                    TypedSignalWrite::OutOfBandTagConflict(OutOfBandTagConflictSignal {
                        inode,
                        path: rel_str.clone(),
                        mismatches,
                    }),
                    witness,
                );
            } else {
                // One-direction extras only - can be synced without conflict
                log_general(format!(
                    "[COMPUTE] VerifyTags: syncable tag diff for inode {} ({})",
                    inode,
                    path.display()
                ));
                // Clear all OOB signals first (including target type to refresh metadata)
                drop_stale_corpus_signal::<OutOfBandTagSyncSignal>(
                    read_only_db,
                    &sender,
                    inode,
                    witness,
                );
                drop_stale_corpus_signal::<OutOfBandTagConflictSignal>(
                    read_only_db,
                    &sender,
                    inode,
                    witness,
                );
                drop_stale_corpus_signal::<MtimeOnlyMismatchSignal>(
                    read_only_db,
                    &sender,
                    inode,
                    witness,
                );
                // Create fresh signal with mismatch metadata (keyed by inode)
                let mismatches: Vec<TagMismatchEntry> = verify_result
                    .mismatches
                    .into_iter()
                    .map(|m| TagMismatchEntry {
                        tag_name: m.field,
                        disk_value: m.disk_value,
                        db_value: m.db_value,
                    })
                    .collect();
                sender.write_typed_signal(
                    TypedSignalWrite::OutOfBandTagSync(OutOfBandTagSyncSignal {
                        inode,
                        path: rel_str.clone(),
                        mismatches,
                    }),
                    witness,
                );
            }

            Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
        }
        Err(e) => {
            // Emit CorruptFile signal so the issue is tracked in the DB (actionable)
            // Log to both general and errors so we can diagnose why this file is flagged
            let err_msg = format!(
                "[COMPUTE] VerifyTags FAILED for inode {} ({}): {:#}",
                inode,
                path.display(),
                e
            );
            log_general(&err_msg);
            crate::logging::log_error(&err_msg);
            ensure_typed_signal(
                read_only_db,
                &sender,
                TypedSignalWrite::CorruptFile(CorruptFileSignal {
                    inode,
                    path: rel_str.clone(),
                }),
                witness,
            );
            // Clear OOB signals on parse error - we can't classify what we can't read
            drop_stale_corpus_signal::<OutOfBandTagConflictSignal>(
                read_only_db,
                &sender,
                inode,
                witness,
            );
            drop_stale_corpus_signal::<OutOfBandTagSyncSignal>(
                read_only_db,
                &sender,
                inode,
                witness,
            );
            drop_stale_corpus_signal::<MtimeOnlyMismatchSignal>(
                read_only_db,
                &sender,
                inode,
                witness,
            );
            // Return success so computation continues processing other files
            Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
        }
    }
}

// ============================================================================
// Verify Audio (Deep Integrity Check)
// ============================================================================

/// Verify audio file integrity by decoding the entire stream.
///
/// Catches truncated files, corrupt audio data, and other issues that
/// tag verification wouldn't detect. Emits CorruptFile if decode fails.
pub fn execute_verify_audio(
    read_only_db: &ReadOnlyDb<'_>,
    inode: i64,
    path: &Path,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::VerifyAudio {
        inode,
        path: path.to_path_buf(),
    };

    // Get signal sender for async writes
    let sender = match crate::db::write_thread::signal_sender().cloned() {
        Some(s) => s,
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Get relative path for signal keys
    let resolver = crate::corpus::paths::get_resolver();
    let rel_path = match resolver.to_relative(path) {
        Some(rel) => rel,
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Path {} does not match any configured root", path.display()),
            );
        }
    };
    let rel_str = rel_path.to_string_lossy().to_string();

    // Verify audio integrity by decoding the entire file
    match crate::corpus::metadata::verify_audio_integrity(path) {
        Ok(()) => {
            // Audio is valid - clear any stale CorruptFile signal (keyed by inode)
            drop_stale_corpus_signal::<CorruptFileSignal>(read_only_db, &sender, inode, witness);
            Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
        }
        Err(e) => {
            // Audio verification failed - file is corrupt
            // Log to both general and errors so we can diagnose why this file is flagged
            let err_msg = format!(
                "[COMPUTE] VerifyAudio FAILED for inode {} ({}): {:#}",
                inode,
                path.display(),
                e
            );
            log_general(&err_msg);
            crate::logging::log_error(&err_msg);
            ensure_typed_signal(
                read_only_db,
                &sender,
                TypedSignalWrite::CorruptFile(CorruptFileSignal {
                    inode,
                    path: rel_str.clone(),
                }),
                witness,
            );
            // Return success so computation continues processing other files
            // (the signal emission handles the error state)
            Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
        }
    }
}
