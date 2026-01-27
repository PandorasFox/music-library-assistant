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
    ensure_file_signal_if_missing,
};
use crate::corpus::computations::types::ComputationWitness;
use crate::corpus::db::types::CorpusFileSignalType;
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
    start: Instant,
) -> Result {
    let computation = Computation::WalkCorpus {
        root: root.to_path_buf(),
        source: source.to_string(),
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
        "[COMPUTE] WalkCorpus: found {} directories in {:?}",
        directories.len(), root
    ));

    // Spawn ScanCorpusDirectory for each directory
    let spawn: Vec<Computation> = directories
        .into_iter()
        .map(|directory| Computation::ScanCorpusDirectory {
            directory,
            source: source.to_string(),
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
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::ScanCorpusDirectory {
        directory: directory.to_path_buf(),
        source: source.to_string(),
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

    // Get indexed inodes from scan_state for comparison
    let inode_vec: Vec<i64> = disk_inodes.iter().copied().collect();
    let indexed_by_inode = read_only_db.get_scan_state_batch(source, &inode_vec).unwrap_or_default();

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

        // Check if file is indexed and needs mtime verification
        if let Some(entry) = indexed_by_inode.get(inode) {
            // File is indexed - check if mtime changed
            if entry.mtime_secs != *disk_mtime_s || entry.mtime_nanos != *disk_mtime_ns {
                let track = match read_only_db.get_track_by_path(&relative_path_str) {
                    Ok(Some(t)) => t,
                    _ => continue,
                };
                let Some(track_id) = track.id else { continue };

                // Mtime mismatched - verify tags
                // Note: path in Computation is still absolute for filesystem operations
                spawn.push(Computation::VerifyMtime {
                    track_id,
                    path: path.clone(),
                    expected_mtime_secs: entry.mtime_secs,
                    expected_mtime_nanos: entry.mtime_nanos,
                });
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
    track_id: i64,
    path: &Path,
    expected_mtime_secs: i64,
    expected_mtime_nanos: i64,
    start: Instant,
) -> Result {
    let computation = Computation::VerifyMtime {
        track_id,
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
            track_id,
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

// ============================================================================
// Verify Tags
// ============================================================================

/// Verify tags on disk match database.
///
/// Delegates to the existing verify_tags implementation in indexing module.
pub fn execute_verify_tags(
    read_only_db: &Database,
    track_id: i64,
    path: &Path,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::corpus::mutations::indexing;

    let computation = Computation::VerifyTags {
        track_id,
        path: path.to_path_buf(),
    };

    match indexing::execute_verify_tags(read_only_db, track_id, path) {
        Ok(()) => Result::success(
            computation,
            start.elapsed().as_millis() as u64,
            Vec::new(),
        ),
        Err(e) => {
            // Emit TagParseError signal so the issue is tracked in the DB
            log_general(format!(
                "[COMPUTE] VerifyTags: tag parse error for track {} ({}): {}",
                track_id, path.display(), e
            ));
            if let Some(sender) = crate::db_thread::signal_sender() {
                let resolver = crate::corpus::paths::get_resolver();
                if let Some(rel) = resolver.to_relative(path) {
                    let rel_str = rel.to_string_lossy();
                    ensure_file_signal_if_missing(
                        read_only_db,
                        &sender,
                        CorpusFileSignalType::TagParseError.into(),
                        &rel_str,
                        witness,
                    );
                }
            }
            // Return success so computation continues processing other files
            Result::success(
                computation,
                start.elapsed().as_millis() as u64,
                Vec::new(),
            )
        }
    }
}
