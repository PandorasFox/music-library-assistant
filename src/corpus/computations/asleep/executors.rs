//! Asleep-phase computation executors.
//!
//! These functions implement the actual logic for Asleep computations.

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::logging::log_general;
use crate::corpus::computations::helpers::{
    enumerate_all_directories, extract_mtime, is_audio_file,
    ensure_file_signal_if_missing, ensure_file_signal_with_metadata_if_missing,
    drop_stale_file_signal,
};
use crate::corpus::computations::types::ComputationWitness;
use crate::corpus::db::types::{CorpusFileSignalType, FileSource};
use crate::corpus::db::Database;
use crate::corpus::paths;
use crate::db_thread;

use super::{Computation, Result};

// ============================================================================
// Phase 0: Clear Existing Observation State
// ============================================================================

/// Phase 0: Clear all FileInCorpus signals before a fresh corpus scan.
///
/// This ensures deleted files don't retain stale signals that would cause them
/// to appear as "healthy" instead of "missing" in DeriveDirectorySignals.
pub fn execute_clear_existing_observation_state(
    _read_only_db: &Database,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    log_general("[COMPUTE] ClearExistingObservationState: clearing FileInCorpus signals");

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                Computation::ClearExistingObservationState,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Clear all FileInCorpus signals - they'll be rebuilt during the corpus walk
    sender.clear_signals_by_type(CorpusFileSignalType::FileInCorpus.into(), witness);

    log_general("[COMPUTE] ClearExistingObservationState: complete");

    Result::success(
        Computation::ClearExistingObservationState,
        start.elapsed().as_millis() as u64,
        Vec::new(), // No spawn - WalkCorpus is queued separately
    )
}

// ============================================================================
// Phase 1: Walk Corpus
// ============================================================================

/// Phase 1: Enumerate ALL corpus directories and spawn per-directory scans.
pub fn execute_walk_corpus(
    _read_only_db: &Database,
    root: &Path,
    source: &str,
    force_check: bool,
    start: Instant,
) -> Result {
    let computation = Computation::WalkCorpus {
        root: root.to_path_buf(),
        source: source.to_string(),
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
        directories.len(), root,
        if force_check { " (force_check=true)" } else { "" }
    ));

    // Spawn ScanCorpusDirectory for each directory
    let spawn: Vec<Computation> = directories
        .into_iter()
        .map(|directory| Computation::ScanCorpusDirectory {
            directory,
            source: source.to_string(),
            force_check,
        })
        .collect();

    Result::success(
        computation,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

// ============================================================================
// Scan Single Directory
// ============================================================================

/// Scan a single corpus directory (non-recursive).
pub fn execute_scan_corpus_directory(
    read_only_db: &Database,
    directory: &Path,
    source: &str,
    force_check: bool,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::ScanCorpusDirectory {
        directory: directory.to_path_buf(),
        source: source.to_string(),
        force_check,
    };

    // Get signal sender for async writes
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

    if !directory.exists() {
        return Result::failure(
            computation,
            start.elapsed().as_millis() as u64,
            format!("Directory does not exist: {:?}", directory),
        );
    }

    // Collect disk state for files DIRECTLY in this directory (not recursive)
    let disk_state = collect_directory_files(directory);

    // If no files in directory, nothing to do
    if disk_state.is_empty() {
        return Result::success(
            computation,
            start.elapsed().as_millis() as u64,
            Vec::new(),
        );
    }

    // Build lookup map for disk inodes
    let disk_inodes: HashSet<i64> = disk_state.iter().map(|(inode, _, _, _)| *inode).collect();

    // Get indexed inodes from files table for comparison (mtime info)
    let file_source = FileSource::from_str(source).unwrap_or(FileSource::Corpus);
    let inode_vec: Vec<i64> = disk_inodes.iter().copied().collect();
    let indexed_by_inode = read_only_db.get_file_mtime_batch(file_source, &inode_vec).unwrap_or_default();

    let mut spawn: Vec<Computation> = Vec::new();
    let resolver = paths::get_resolver();

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

        // Create FileInCorpus signal for every file on disk (with relative key)
        // (ClearExistingObservationState cleared all stale signals at start of observation)
        ensure_file_signal_if_missing(read_only_db, &sender, CorpusFileSignalType::FileInCorpus.into(), &relative_path_str, witness);

        // Check if file is indexed and needs verification
        // indexed_by_inode returns HashMap<inode, (mtime_secs, mtime_nanos)>
        if let Some((db_mtime_secs, db_mtime_nanos)) = indexed_by_inode.get(inode) {
            // File is indexed by inode
            let mtime_changed = *db_mtime_secs != *disk_mtime_s || *db_mtime_nanos != *disk_mtime_ns;

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
            // Inode not in files table - check if path is indexed with different inode
            // This detects file replacement (same path, new inode)
            if let Ok(Some(audio_file)) = read_only_db.get_audio_file_by_path(&relative_path_str) {
                if audio_file.inode() != *inode {
                    // Inode changed! File was replaced.
                    let old_inode = audio_file.inode();

                    // Emit InodeChanged signal with old/new inode metadata
                    let metadata = serde_json::json!({
                        "old_inode": old_inode,
                        "new_inode": inode,
                    });
                    ensure_file_signal_with_metadata_if_missing(
                        read_only_db,
                        &sender,
                        CorpusFileSignalType::InodeChanged.into(),
                        &relative_path_str,
                        &metadata.to_string(),
                        witness,
                    );

                    // Spawn VerifyTags to check for tag differences
                    // (tags may differ between old and new file)
                    spawn.push(Computation::VerifyTags {
                        inode: *inode,
                        path: path.clone(),
                    });
                }
            }
        }
    }

    Result::success(
        computation,
        start.elapsed().as_millis() as u64,
        spawn,
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

// ============================================================================
// Verify Mtime
// ============================================================================

/// Phase 3: Verify single file mtime.
pub fn execute_verify_mtime(
    _read_only_db: &Database,
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

    Result::success(
        computation,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

/// Check if a file's disk mtime differs from what's stored in files table.
///
/// Uses the portable API (extract_mtime) for consistency with ScanCorpusDirectory.
/// Returns true if mtime differs or if we can't determine (fail-safe to emit signal).
fn check_mtime_differs(read_only_db: &Database, inode: i64, path: &Path) -> bool {
    // Get mtime info from files table for this inode
    let mtime_info = match read_only_db.get_file_mtime_batch(FileSource::Corpus, &[inode]) {
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
    read_only_db: &Database,
    inode: i64,
    path: &Path,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::corpus::mutations::indexing;

    let computation = Computation::VerifyTags {
        inode,
        path: path.to_path_buf(),
    };

    // Route mismatch writes through db_thread (read-only connection can't write directly)
    let sender = match crate::db_thread::signal_sender().cloned() {
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

    match indexing::execute_verify_tags(read_only_db, inode, path, &sender, witness) {
        Ok(verify_result) => {
            // Full classification based on TagVerifyResult
            if verify_result.is_clean() {
                // Tags match exactly - but we need to check if mtime actually differs
                // (in force_check mode, VerifyTags runs even when mtime matches)
                let mtime_actually_differs = check_mtime_differs(read_only_db, inode, path);

                if mtime_actually_differs {
                    // Mtime changed but tags are identical - requires operator acknowledgement
                    log_general(format!(
                        "[COMPUTE] VerifyTags: mtime-only change for inode {} ({})",
                        inode, path.display()
                    ));
                    ensure_file_signal_if_missing(
                        read_only_db,
                        &sender,
                        CorpusFileSignalType::MtimeOnlyMismatch.into(),
                        &rel_str,
                        witness,
                    );
                    // Clear mutually exclusive signals
                    drop_stale_file_signal(
                        read_only_db,
                        &sender,
                        CorpusFileSignalType::OutOfBandTagConflict.into(),
                        &rel_str,
                        witness,
                    );
                    drop_stale_file_signal(
                        read_only_db,
                        &sender,
                        CorpusFileSignalType::OutOfBandTagSync.into(),
                        &rel_str,
                        witness,
                    );
                } else {
                    // Tags match AND mtime matches - file is healthy, clear all OOB signals
                    log_general(format!(
                        "[COMPUTE] VerifyTags: file healthy for inode {} ({})",
                        inode, path.display()
                    ));
                    drop_stale_file_signal(
                        read_only_db,
                        &sender,
                        CorpusFileSignalType::MtimeOnlyMismatch.into(),
                        &rel_str,
                        witness,
                    );
                    drop_stale_file_signal(
                        read_only_db,
                        &sender,
                        CorpusFileSignalType::OutOfBandTagConflict.into(),
                        &rel_str,
                        witness,
                    );
                    drop_stale_file_signal(
                        read_only_db,
                        &sender,
                        CorpusFileSignalType::OutOfBandTagSync.into(),
                        &rel_str,
                        witness,
                    );
                }
            } else if verify_result.is_conflict() {
                // Value conflicts or mixed-direction extras - requires operator decision
                log_general(format!(
                    "[COMPUTE] VerifyTags: tag conflict for inode {} ({})",
                    inode, path.display()
                ));
                ensure_file_signal_if_missing(
                    read_only_db,
                    &sender,
                    CorpusFileSignalType::OutOfBandTagConflict.into(),
                    &rel_str,
                    witness,
                );
                // Clear mutually exclusive signals
                drop_stale_file_signal(
                    read_only_db,
                    &sender,
                    CorpusFileSignalType::OutOfBandTagSync.into(),
                    &rel_str,
                    witness,
                );
                drop_stale_file_signal(
                    read_only_db,
                    &sender,
                    CorpusFileSignalType::MtimeOnlyMismatch.into(),
                    &rel_str,
                    witness,
                );
            } else {
                // One-direction extras only - can be synced without conflict
                log_general(format!(
                    "[COMPUTE] VerifyTags: syncable tag diff for inode {} ({})",
                    inode, path.display()
                ));
                ensure_file_signal_if_missing(
                    read_only_db,
                    &sender,
                    CorpusFileSignalType::OutOfBandTagSync.into(),
                    &rel_str,
                    witness,
                );
                // Clear mutually exclusive signals
                drop_stale_file_signal(
                    read_only_db,
                    &sender,
                    CorpusFileSignalType::OutOfBandTagConflict.into(),
                    &rel_str,
                    witness,
                );
                drop_stale_file_signal(
                    read_only_db,
                    &sender,
                    CorpusFileSignalType::MtimeOnlyMismatch.into(),
                    &rel_str,
                    witness,
                );
            }

            Result::success(
                computation,
                start.elapsed().as_millis() as u64,
                Vec::new(),
            )
        }
        Err(e) => {
            // Emit CorruptFile signal so the issue is tracked in the DB (actionable)
            log_general(format!(
                "[COMPUTE] VerifyTags: tag parse error for inode {} ({}): {}",
                inode, path.display(), e
            ));
            ensure_file_signal_if_missing(
                read_only_db,
                &sender,
                CorpusFileSignalType::CorruptFile.into(),
                &rel_str,
                witness,
            );
            // Clear OOB signals on parse error - we can't classify what we can't read
            drop_stale_file_signal(
                read_only_db,
                &sender,
                CorpusFileSignalType::OutOfBandTagConflict.into(),
                &rel_str,
                witness,
            );
            drop_stale_file_signal(
                read_only_db,
                &sender,
                CorpusFileSignalType::OutOfBandTagSync.into(),
                &rel_str,
                witness,
            );
            drop_stale_file_signal(
                read_only_db,
                &sender,
                CorpusFileSignalType::MtimeOnlyMismatch.into(),
                &rel_str,
                witness,
            );
            // Return success so computation continues processing other files
            Result::success(
                computation,
                start.elapsed().as_millis() as u64,
                Vec::new(),
            )
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
    read_only_db: &Database,
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
    let sender = match crate::db_thread::signal_sender().cloned() {
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
            // Audio is valid - clear any stale CorruptFile signal
            drop_stale_file_signal(
                read_only_db,
                &sender,
                CorpusFileSignalType::CorruptFile.into(),
                &rel_str,
                witness,
            );
            Result::success(
                computation,
                start.elapsed().as_millis() as u64,
                Vec::new(),
            )
        }
        Err(e) => {
            // Audio verification failed - file is corrupt
            log_general(format!(
                "[COMPUTE] VerifyAudio: corruption detected for inode {} ({}): {}",
                inode, path.display(), e
            ));
            ensure_file_signal_if_missing(
                read_only_db,
                &sender,
                CorpusFileSignalType::CorruptFile.into(),
                &rel_str,
                witness,
            );
            // Return success so computation continues processing other files
            // (the signal emission handles the error state)
            Result::success(
                computation,
                start.elapsed().as_millis() as u64,
                Vec::new(),
            )
        }
    }
}
