//! Asleep-phase computation executors.
//!
//! These functions implement the actual logic for Asleep computations.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::config::log_message;
use crate::corpus::computations::helpers::{
    enumerate_all_directories, extract_mtime, is_audio_file,
    ensure_file_signal_if_missing,
};
use crate::corpus::computations::types::ComputationWitness;
use crate::corpus::db::types::FileSignalType;
use crate::corpus::db::Database;
use crate::db_thread;

use super::{Computation, Result};

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
        let _ = log_message(&format!(
            "[WARN] WalkCorpus: skipped {} directory symlinks in {:?}",
            symlink_count, root
        ));
    }

    let _ = log_message(&format!(
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

    // Process each file found on disk
    for (inode, path, disk_mtime_s, disk_mtime_ns) in &disk_state {
        let path_str = path.to_string_lossy().to_string();

        if let Some(entry) = indexed_by_inode.get(inode) {
            // File is indexed - check if mtime changed
            if entry.mtime_secs != *disk_mtime_s || entry.mtime_nanos != *disk_mtime_ns {
                let track = match read_only_db.get_track_by_path(&path_str) {
                    Ok(Some(t)) => t,
                    _ => continue,
                };
                let Some(track_id) = track.id else { continue };

                // Mtime mismatched - verify tags
                spawn.push(Computation::VerifyMtime {
                    track_id,
                    path: path.clone(),
                    expected_mtime_secs: entry.mtime_secs,
                    expected_mtime_nanos: entry.mtime_nanos,
                });
            }
        } else {
            // File not indexed - create a FileInCorpus signal for this file
            ensure_file_signal_if_missing(read_only_db, &sender, FileSignalType::FileInCorpus, &path_str, witness);
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
// Phase 2: Compare Inodes
// ============================================================================

/// Phase 2: Compare disk state to database index.
pub fn execute_compare_inodes(
    read_only_db: &Database,
    source: &str,
    disk_state: &[(i64, PathBuf, i64, i64)],
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let _ = log_message(&format!(
        "[COMPUTE] CompareInodes: comparing {} disk files for source '{}'",
        disk_state.len(),
        source
    ));

    let computation = Computation::CompareInodes {
        source: source.to_string(),
        disk_state: disk_state.to_vec(),
    };

    // Build disk inode set and lookup maps
    let disk_inodes: HashSet<i64> = disk_state.iter().map(|(inode, _, _, _)| *inode).collect();
    let disk_inode_to_state: HashMap<i64, (&PathBuf, i64, i64)> = disk_state
        .iter()
        .map(|(inode, path, mtime_s, mtime_ns)| (*inode, (path, *mtime_s, *mtime_ns)))
        .collect();

    // Get indexed inodes from scan_state
    let indexed_inodes = read_only_db.get_all_scan_state_inodes(source).unwrap_or_default();

    // Calculate differences
    let missing_from_disk: HashSet<i64> = indexed_inodes.difference(&disk_inodes).cloned().collect();
    let missing_from_index: HashSet<i64> = disk_inodes.difference(&indexed_inodes).cloned().collect();

    let _ = log_message(&format!(
        "[COMPUTE] CompareInodes: {} indexed, {} on disk, {} missing from disk, {} missing from index",
        indexed_inodes.len(),
        disk_inodes.len(),
        missing_from_disk.len(),
        missing_from_index.len()
    ));

    // Create MissingFile signals (indexed files not on disk)
    if !missing_from_disk.is_empty() {
        if let Ok(missing_paths) = read_only_db.get_scan_state_paths_for_inodes(source, &missing_from_disk) {
            create_missing_file_issues(read_only_db, &missing_paths, witness);
        }
    }

    // Create UnindexedFile signals (disk files not indexed)
    if !missing_from_index.is_empty() {
        let missing_paths: Vec<String> = missing_from_index
            .iter()
            .filter_map(|inode| disk_inode_to_state.get(inode))
            .map(|(path, _, _)| path.to_string_lossy().to_string())
            .collect();
        create_unindexed_file_issues(read_only_db, &missing_paths, witness);
    }

    // Spawn follow-up computations for mtime verification
    let mut spawn: Vec<Computation> = Vec::new();

    // Get scan_state entries for comparison
    let inode_vec: Vec<i64> = indexed_inodes.iter().copied().collect();
    let indexed_by_inode = read_only_db.get_scan_state_batch(source, &inode_vec).unwrap_or_default();

    for (inode, path, disk_mtime_s, disk_mtime_ns) in disk_state {
        // Skip files not in index (already reported as UnindexedFile)
        let Some(entry) = indexed_by_inode.get(inode) else {
            continue;
        };

        // Only verify if mtime mismatched
        if entry.mtime_secs != *disk_mtime_s || entry.mtime_nanos != *disk_mtime_ns {
            // Get track_id for this path
            let track = match read_only_db.get_track_by_path(&path.to_string_lossy()) {
                Ok(Some(t)) => t,
                _ => continue,
            };
            let Some(track_id) = track.id else { continue };

            spawn.push(Computation::VerifyMtime {
                track_id,
                path: path.clone(),
                expected_mtime_secs: entry.mtime_secs,
                expected_mtime_nanos: entry.mtime_nanos,
            });
        }
    }

    let _ = log_message(&format!(
        "[COMPUTE] CompareInodes: spawning {} mtime verifications",
        spawn.len()
    ));

    Result::success(
        computation,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

/// Create MissingFile signals for files in index but missing from disk.
fn create_missing_file_issues(
    read_only_db: &Database,
    missing_paths: &[String],
    witness: &ComputationWitness,
) {
    let Some(sender) = db_thread::signal_sender() else {
        return;
    };

    for path in missing_paths {
        ensure_file_signal_if_missing(read_only_db, &sender, FileSignalType::MissingFile, path, witness);
    }
}

/// Create UnindexedFile signals for files on disk but not in index.
fn create_unindexed_file_issues(
    read_only_db: &Database,
    missing_paths: &[String],
    witness: &ComputationWitness,
) {
    let Some(sender) = db_thread::signal_sender() else {
        return;
    };

    for path in missing_paths {
        ensure_file_signal_if_missing(read_only_db, &sender, FileSignalType::UnindexedFile, path, witness);
    }
}

// ============================================================================
// Phase 3: Verify Mtime
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
        Err(e) => Result::failure(
            computation,
            start.elapsed().as_millis() as u64,
            e.to_string(),
        ),
    }
}
