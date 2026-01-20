//! Eyeballing computations - corpus scanning and file verification.
//!
//! The eyeballing system walks the corpus directory tree, comparing disk state
//! against the database index. It identifies:
//! - Files present on disk but not indexed (UnindexedFile)
//! - Files indexed but missing from disk (MissingFile)
//! - Files with modified timestamps (CorpusFileModifiedOutOfBand)
//! - Tag mismatches between disk and index (OutOfBandTagChange)

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::config::log_message;
use crate::corpus::db::types::FileSignalType;
use crate::corpus::db::Database;
use crate::db_thread;

use super::helpers::{
    enumerate_all_directories, extract_mtime, get_signal_sender_or_fail,
    is_audio_file, ensure_file_signal_if_missing,
};
use super::types::{Computation, ComputationResult, ComputationWitness};

// ============================================================================
// Phase 1: Walk Corpus
// ============================================================================

/// Phase 1: Enumerate ALL corpus directories and spawn per-directory scans.
///
/// Recursively walks the entire corpus tree to find all directories,
/// then spawns `ScanCorpusDirectory` for each. This provides granular
/// progress feedback during startup (one computation per directory).
pub(super) fn execute_walk_corpus(
    _db: &Database,
    root: &Path,
    source: &str,
    paranoid: bool,
    start: Instant,
) -> ComputationResult {
    let computation = Computation::WalkCorpus {
        root: root.to_path_buf(),
        source: source.to_string(),
        paranoid,
    };

    if !root.exists() {
        return ComputationResult::failure(
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
            paranoid,
        })
        .collect();

    ComputationResult::success(
        computation,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

// ============================================================================
// Scan Single Directory
// ============================================================================

/// Scan a single corpus directory (non-recursive).
///
/// Processes only audio files directly in the given directory, compares against
/// scan_state, creates FileInCorpus signals (one per file), and spawns follow-up
/// computations.
pub(super) fn execute_scan_corpus_directory(
    db: &Database,
    directory: &Path,
    source: &str,
    paranoid: bool,
    witness: &ComputationWitness,
    start: Instant,
) -> ComputationResult {
    let computation = Computation::ScanCorpusDirectory {
        directory: directory.to_path_buf(),
        source: source.to_string(),
        paranoid,
    };

    // Get signal sender for async writes
    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    if !directory.exists() {
        return ComputationResult::failure(
            computation,
            start.elapsed().as_millis() as u64,
            format!("Directory does not exist: {:?}", directory),
        );
    }

    // Collect disk state for files DIRECTLY in this directory (not recursive)
    let disk_state = collect_directory_files(directory);

    // If no files in directory, nothing to do
    if disk_state.is_empty() {
        return ComputationResult::success(
            computation,
            start.elapsed().as_millis() as u64,
            Vec::new(),
        );
    }

    // Build lookup map for disk inodes
    let disk_inodes: HashSet<i64> = disk_state.iter().map(|(inode, _, _, _)| *inode).collect();

    // Get indexed inodes from scan_state for comparison
    let inode_vec: Vec<i64> = disk_inodes.iter().copied().collect();
    let indexed_by_inode = db.get_scan_state_batch(source, &inode_vec).unwrap_or_default();

    let mut spawn: Vec<Computation> = Vec::new();

    // Process each file found on disk
    for (inode, path, disk_mtime_s, disk_mtime_ns) in &disk_state {
        let path_str = path.to_string_lossy().to_string();

        if let Some(entry) = indexed_by_inode.get(inode) {
            // File is indexed - check if we need to verify mtime or tags
            let track = match db.get_track_by_path(&path_str) {
                Ok(Some(t)) => t,
                _ => continue,
            };
            let Some(track_id) = track.id else { continue };

            if paranoid {
                // Paranoid mode: verify tags for ALL indexed files
                spawn.push(Computation::VerifyTags {
                    track_id,
                    path: path.clone(),
                });
            } else if entry.mtime_secs != *disk_mtime_s || entry.mtime_nanos != *disk_mtime_ns {
                // Non-paranoid: only verify if mtime mismatched
                spawn.push(Computation::VerifyMtime {
                    track_id,
                    path: path.clone(),
                    expected_mtime_secs: entry.mtime_secs,
                    expected_mtime_nanos: entry.mtime_nanos,
                });
            }
        } else {
            // File not indexed - create a FileInCorpus signal for this file
            ensure_file_signal_if_missing(db, &sender, FileSignalType::FileInCorpus, &path_str, witness);
        }
    }

    ComputationResult::success(
        computation,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

/// Collect audio files directly in a directory (non-recursive).
pub(super) fn collect_directory_files(dir: &Path) -> Vec<(i64, PathBuf, i64, i64)> {
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
///
/// Creates signals for:
/// - MissingFromDisk: indexed files not on disk
/// - MissingFromIndex: disk files not indexed
///
/// Spawns: VerifyMtime for files with mtime mismatches (non-paranoid)
///         or VerifyTags for all files (paranoid mode)
pub(super) fn execute_compare_inodes(
    db: &Database,
    source: &str,
    disk_state: &[(i64, PathBuf, i64, i64)],
    paranoid: bool,
    witness: &ComputationWitness,
    start: Instant,
) -> ComputationResult {
    let _ = log_message(&format!(
        "[COMPUTE] CompareInodes: comparing {} disk files for source '{}'",
        disk_state.len(),
        source
    ));

    let computation = Computation::CompareInodes {
        source: source.to_string(),
        disk_state: disk_state.to_vec(),
        paranoid,
    };

    // Build disk inode set and lookup maps
    let disk_inodes: HashSet<i64> = disk_state.iter().map(|(inode, _, _, _)| *inode).collect();
    let disk_inode_to_state: HashMap<i64, (&PathBuf, i64, i64)> = disk_state
        .iter()
        .map(|(inode, path, mtime_s, mtime_ns)| (*inode, (path, *mtime_s, *mtime_ns)))
        .collect();

    // Get indexed inodes from scan_state
    let indexed_inodes = db.get_all_scan_state_inodes(source).unwrap_or_default();

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

    // Create MissingFromDisk signals
    if !missing_from_disk.is_empty() {
        if let Ok(missing_paths) = db.get_scan_state_paths_for_inodes(source, &missing_from_disk) {
            create_missing_from_disk_issues(db, source, &missing_paths, witness);
        }
    }

    // Create MissingFromIndex signals
    if !missing_from_index.is_empty() {
        let missing_paths: Vec<String> = missing_from_index
            .iter()
            .filter_map(|inode| disk_inode_to_state.get(inode))
            .map(|(path, _, _)| path.to_string_lossy().to_string())
            .collect();
        create_missing_from_index_issues(db, &missing_paths, witness);
    }

    // Spawn follow-up computations for mtime verification
    let mut spawn: Vec<Computation> = Vec::new();

    // Get scan_state entries for comparison (already returns HashMap<i64, ScanStateEntry>)
    let inode_vec: Vec<i64> = indexed_inodes.iter().copied().collect();
    let indexed_by_inode = db.get_scan_state_batch(source, &inode_vec).unwrap_or_default();

    for (inode, path, disk_mtime_s, disk_mtime_ns) in disk_state {
        // Skip files not in index (already reported as MissingFromIndex)
        let Some(entry) = indexed_by_inode.get(inode) else {
            continue;
        };

        // Get track_id for this path
        let track = match db.get_track_by_path(&path.to_string_lossy()) {
            Ok(Some(t)) => t,
            _ => continue,
        };
        let Some(track_id) = track.id else { continue };

        if paranoid {
            // Paranoid mode: verify tags for ALL indexed files
            spawn.push(Computation::VerifyTags {
                track_id,
                path: path.clone(),
            });
        } else {
            // Non-paranoid: only verify if mtime mismatched
            if entry.mtime_secs != *disk_mtime_s || entry.mtime_nanos != *disk_mtime_ns {
                spawn.push(Computation::VerifyMtime {
                    track_id,
                    path: path.clone(),
                    expected_mtime_secs: entry.mtime_secs,
                    expected_mtime_nanos: entry.mtime_nanos,
                });
            }
        }
    }

    let _ = log_message(&format!(
        "[COMPUTE] CompareInodes: spawning {} follow-up computations ({})",
        spawn.len(),
        if paranoid { "paranoid tag verify" } else { "mtime verify" }
    ));

    ComputationResult::success(
        computation,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

/// Create MissingFile signals for files in index but missing from disk.
///
/// Creates one signal per file with path as key (no metadata).
fn create_missing_from_disk_issues(
    db: &Database,
    _source: &str,
    missing_paths: &[String],
    witness: &ComputationWitness,
) {
    let Some(sender) = db_thread::signal_sender() else {
        return;
    };

    // Create one signal per missing file (with freshness check)
    for path in missing_paths {
        ensure_file_signal_if_missing(db, &sender, FileSignalType::MissingFile, path, witness);
    }
}

/// Create UnindexedFile signals for files on disk but not in index.
///
/// Called from CompareInodes when it identifies files missing from the index.
fn create_missing_from_index_issues(
    db: &Database,
    missing_paths: &[String],
    witness: &ComputationWitness,
) {
    let Some(sender) = db_thread::signal_sender() else {
        return;
    };

    // File exists on disk but is not indexed = UnindexedFile (with freshness check)
    for path in missing_paths {
        ensure_file_signal_if_missing(db, &sender, FileSignalType::UnindexedFile, path, witness);
    }
}

// ============================================================================
// Phase 3: Verify Mtime
// ============================================================================

/// Phase 3: Verify single file mtime.
///
/// Checks if current mtime differs from expected (from scan_state).
/// Spawns: VerifyTags if mtime mismatched.
pub(super) fn execute_verify_mtime(
    _db: &Database,
    track_id: i64,
    path: &Path,
    expected_mtime_secs: i64,
    expected_mtime_nanos: i64,
    start: Instant,
) -> ComputationResult {
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
            return ComputationResult::failure(
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

    ComputationResult::success(
        computation,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}
