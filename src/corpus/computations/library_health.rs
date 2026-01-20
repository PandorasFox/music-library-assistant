//! Library health computations and second-level signal derivations.
//!
//! This module handles:
//! 1. Second-level signal derivation (after eyeballing): comparing FileInCorpus
//!    signals against indexed tracks to derive HealthyFile, UnindexedFile, MissingFile
//! 2. Library health scanning: walking library directories and checking for
//!    orphaned or stale deployments
//!
//! Second-level signals bridge the gap between first-level eyeballing (which only
//! knows what's on disk) and the index (which knows what was previously indexed).

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::config::log_message;
use crate::corpus::db::types::{FileSignalType, HealthIssueType};
use crate::corpus::db::Database;

use super::helpers::{
    clear_file_signal_if_present, ensure_file_signal_if_missing, enumerate_all_directories,
    get_configured_library_names, get_signal_sender_or_fail, is_audio_file,
};
use super::types::{Computation, ComputationResult, ComputationWitness};

// ============================================================================
// Second-Level Signal Derivations
// ============================================================================

/// Schedule second-level signal derivations by spawning per-directory computations.
pub(super) fn execute_schedule_second_level_derivations(
    db: &Database,
    start: Instant,
) -> ComputationResult {
    let _ = log_message("[COMPUTE] ScheduleSecondLevelDerivations: starting");

    // Get all directories that have:
    // 1. FileInCorpus signals (corpus directories with audio files)
    // 2. Indexed tracks (may be missing from corpus now)
    let corpus_dirs = db.get_distinct_corpus_directories().unwrap_or_default();
    let _ = log_message(&format!(
        "[COMPUTE] Found {} directories with FileInCorpus signals",
        corpus_dirs.len()
    ));

    let index_dirs = db.get_distinct_track_directories().unwrap_or_default();
    let _ = log_message(&format!(
        "[COMPUTE] Found {} directories with indexed tracks",
        index_dirs.len()
    ));

    // Union of all directories that need second-level signal derivation
    let all_dirs: HashSet<PathBuf> = corpus_dirs.into_iter()
        .chain(index_dirs.into_iter())
        .collect();

    let _ = log_message(&format!(
        "[COMPUTE] ScheduleSecondLevelDerivations: spawning {} directory computations",
        all_dirs.len()
    ));

    // Spawn a DeriveDirectorySignals computation for each directory
    let mut spawn: Vec<Computation> = all_dirs
        .into_iter()
        .map(|directory| Computation::DeriveDirectorySignals { directory })
        .collect();

    // Also spawn library health computations for each configured library
    if let Ok(config) = crate::config::load_config() {
        let library_names = get_configured_library_names(&config);
        let _ = log_message(&format!(
            "[COMPUTE] ScheduleSecondLevelDerivations: spawning {} library walks",
            library_names.len()
        ));

        for library_name in library_names {
            let library_root = config.libraries_root.join(&library_name);
            let corpus_path_prefixes = config.get_corpus_paths_for_library(&library_name);
            spawn.push(Computation::WalkLibrary {
                library_root,
                library_name,
                corpus_path_prefixes,
            });
        }
    }

    ComputationResult::success(
        Computation::ScheduleSecondLevelDerivations,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

/// Derive second-level signals for files in a single directory.
///
/// Computes the expected state for each file and ensures the correct signals exist
/// while clearing signals that no longer apply.
pub(super) fn execute_derive_directory_signals(
    db: &Database,
    directory: &Path,
    witness: &ComputationWitness,
    start: Instant,
) -> ComputationResult {
    let computation = Computation::DeriveDirectorySignals {
        directory: directory.to_path_buf(),
    };

    // Get signal sender for async writes
    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    // Get FileInCorpus signals for this directory
    let corpus_signals = match db.get_signals_in_directory(directory, HealthIssueType::FileInCorpus) {
        Ok(s) => s,
        Err(e) => {
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to get FileInCorpus signals: {}", e),
            );
        }
    };

    // Get indexed tracks for this directory (regardless of fingerprint status)
    let dir_str = directory.to_string_lossy();
    let tracks = match db.get_tracks_by_corpus_path_prefix(&dir_str) {
        Ok(t) => t,
        Err(e) => {
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to get tracks: {}", e),
            );
        }
    };

    // Build lookup sets
    let corpus_paths: HashSet<String> = corpus_signals
        .iter()
        .map(|s| s.issue_key.clone())
        .collect();

    let indexed_paths: HashMap<String, &crate::corpus::db::types::Track> = tracks
        .iter()
        .map(|t| (t.path.clone(), t))
        .collect();

    // Prune stale UnindexedFile signals: those without a matching FileInCorpus.
    // This handles files that were deleted/converted (e.g., WMA→FLAC) - the old
    // UnindexedFile signal should be removed since the file no longer exists.
    // NOTE: No freshness check needed here - we're iterating signals we know exist.
    if let Ok(existing_unindexed) =
        db.get_signals_in_directory(directory, HealthIssueType::UnindexedFile)
    {
        for signal in existing_unindexed {
            if !corpus_paths.contains(&signal.issue_key) {
                // No FileInCorpus for this path → file was deleted → prune stale signal
                // Direct call since we already know the signal exists from DB query
                sender.clear_file_signal(FileSignalType::UnindexedFile, &signal.issue_key, witness);
            }
        }
    }

    let spawn: Vec<Computation> = Vec::new();

    // Process files in corpus (FileInCorpus signals)
    for corpus_path in &corpus_paths {
        if indexed_paths.contains_key(corpus_path) {
            // File is in both corpus and index → clear UnindexedFile if it exists
            clear_file_signal_if_present(db, &sender, FileSignalType::UnindexedFile, corpus_path, witness);
        } else {
            // File in corpus but not indexed → ensure UnindexedFile signal
            ensure_file_signal_if_missing(db, &sender, FileSignalType::UnindexedFile, corpus_path, witness);
        }
    }

    // Process indexed tracks
    for (path, _track) in &indexed_paths {
        if corpus_paths.contains(path) {
            // File exists in both corpus and index → healthy
            // Clear MissingFile signal if it existed
            clear_file_signal_if_present(db, &sender, FileSignalType::MissingFile, path, witness);

            // Ensure HealthyFile signal
            ensure_file_signal_if_missing(db, &sender, FileSignalType::HealthyFile, path, witness);

            // NOTE: CheckDeployConflicts is now handled in bulk by DetectDeployConflicts
            // during ScheduleContentAnalysis, not per-file here.
        } else {
            // Track without FileInCorpus → file is missing from disk
            // Clear HealthyFile signal if it existed
            clear_file_signal_if_present(db, &sender, FileSignalType::HealthyFile, path, witness);

            // Ensure MissingFile signal
            ensure_file_signal_if_missing(db, &sender, FileSignalType::MissingFile, path, witness);
        }
    }

    // Clean up stale signals for paths no longer tracked
    // UnindexedFile signals for paths that are now indexed (handled above)
    // HealthyFile signals for paths no longer in corpus or index need cleanup
    // This requires knowing all paths that WERE tracked - for now we handle the common cases above

    ComputationResult::success(
        computation,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

/// Check for deployment conflicts on a healthy track.
///
/// TODO: Integrate with existing deploy conflict detection logic in corpus/health/detection.rs.
/// For now, this is a no-op placeholder that allows the computation chain to complete.
pub(super) fn execute_check_deploy_conflicts(
    _db: &Database,
    track_id: i64,
    start: Instant,
) -> ComputationResult {
    // Deploy conflict detection is handled by the existing library-level detection
    // in corpus/health/detection.rs. Per-track conflict checking would require
    // refactoring that logic to work at the track level.
    //
    // For now, this computation is a no-op - actual deploy conflicts are still
    // detected during the dedicated deploy conflict detection pass.
    let _ = log_message(&format!(
        "CheckDeployConflicts: skipping track {} (pending integration)",
        track_id
    ));

    ComputationResult::success(
        Computation::CheckDeployConflicts { track_id },
        start.elapsed().as_millis() as u64,
        Vec::new(),
    )
}

/// Update signals for a single file after a mutation.
///
/// This is a lightweight computation that:
/// 1. Checks if file exists on disk (FileInCorpus)
/// 2. Checks if file is indexed (track exists)
/// 3. Updates exactly the signals that apply to THIS file
///
/// Does NOT spawn CheckDeployConflicts - that is handled in bulk by
/// DetectDeployConflicts during ScheduleContentAnalysis.
pub(super) fn execute_update_file_signals(
    db: &Database,
    path: &Path,
    witness: &ComputationWitness,
    start: Instant,
) -> ComputationResult {
    let computation = Computation::UpdateFileSignals {
        path: path.to_path_buf(),
    };

    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    let path_str = path.to_string_lossy().to_string();
    let file_exists = path.exists() && is_audio_file(path);
    let is_indexed = db.get_track_by_path(&path_str).ok().flatten().is_some();

    if file_exists {
        // File exists on disk
        ensure_file_signal_if_missing(db, &sender, FileSignalType::FileInCorpus, &path_str, witness);

        if is_indexed {
            // Healthy: exists + indexed
            clear_file_signal_if_present(db, &sender, FileSignalType::UnindexedFile, &path_str, witness);
            clear_file_signal_if_present(db, &sender, FileSignalType::MissingFile, &path_str, witness);
            ensure_file_signal_if_missing(db, &sender, FileSignalType::HealthyFile, &path_str, witness);
        } else {
            // Unindexed: exists but not indexed
            clear_file_signal_if_present(db, &sender, FileSignalType::HealthyFile, &path_str, witness);
            clear_file_signal_if_present(db, &sender, FileSignalType::MissingFile, &path_str, witness);
            ensure_file_signal_if_missing(db, &sender, FileSignalType::UnindexedFile, &path_str, witness);
        }
    } else {
        // File does not exist on disk
        clear_file_signal_if_present(db, &sender, FileSignalType::FileInCorpus, &path_str, witness);
        clear_file_signal_if_present(db, &sender, FileSignalType::UnindexedFile, &path_str, witness);

        if is_indexed {
            // Missing: indexed but not on disk
            clear_file_signal_if_present(db, &sender, FileSignalType::HealthyFile, &path_str, witness);
            ensure_file_signal_if_missing(db, &sender, FileSignalType::MissingFile, &path_str, witness);
        } else {
            // Gone: not indexed, not on disk - clear all
            clear_file_signal_if_present(db, &sender, FileSignalType::HealthyFile, &path_str, witness);
            clear_file_signal_if_present(db, &sender, FileSignalType::MissingFile, &path_str, witness);
        }
    }

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Library Health Scanning
// ============================================================================

/// Walk a library directory tree and spawn per-directory scans.
pub(super) fn execute_walk_library(
    _db: &Database,
    library_root: &Path,
    library_name: &str,
    corpus_path_prefixes: &[PathBuf],
    start: Instant,
) -> ComputationResult {
    let computation = Computation::WalkLibrary {
        library_root: library_root.to_path_buf(),
        library_name: library_name.to_string(),
        corpus_path_prefixes: corpus_path_prefixes.to_vec(),
    };

    if !library_root.exists() {
        let _ = log_message(&format!(
            "[COMPUTE] WalkLibrary: library root does not exist: {:?}",
            library_root
        ));
        return ComputationResult::success(
            computation,
            start.elapsed().as_millis() as u64,
            Vec::new(), // No directories to scan
        );
    }

    // Enumerate all directories recursively
    let (directories, _symlink_count) = enumerate_all_directories(library_root);

    let _ = log_message(&format!(
        "[COMPUTE] WalkLibrary '{}': found {} directories in {:?}",
        library_name,
        directories.len(),
        library_root
    ));

    // Spawn ScanLibraryDirectory for each directory
    let spawn: Vec<Computation> = directories
        .into_iter()
        .map(|directory| Computation::ScanLibraryDirectory {
            directory,
            library_name: library_name.to_string(),
            library_root: library_root.to_path_buf(),
            corpus_path_prefixes: corpus_path_prefixes.to_vec(),
        })
        .collect();

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, spawn)
}

/// Scan a single library directory and collect (path, inode) pairs.
///
/// Since we need to aggregate files before deriving signals, this stores
/// files in a temporary table or accumulates them in memory. For simplicity,
/// we'll collect files and spawn DeriveLibraryHealthSignals when the last
/// directory scan completes.
///
/// Note: This design accumulates files across multiple ScanLibraryDirectory
/// calls by tracking completion in a separate mechanism. For now, each
/// scan immediately spawns a DeriveLibraryHealthSignals with its files.
/// A more efficient implementation would batch directories.
pub(super) fn execute_scan_library_directory(
    _db: &Database,
    directory: &Path,
    library_name: &str,
    library_root: &Path,
    corpus_path_prefixes: &[PathBuf],
    start: Instant,
) -> ComputationResult {
    let computation = Computation::ScanLibraryDirectory {
        directory: directory.to_path_buf(),
        library_name: library_name.to_string(),
        library_root: library_root.to_path_buf(),
        corpus_path_prefixes: corpus_path_prefixes.to_vec(),
    };

    // Collect audio files in this directory (non-recursive, only immediate children)
    let mut library_files: Vec<(PathBuf, i64)> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && is_audio_file(&path) {
                if let Ok(metadata) = std::fs::metadata(&path) {
                    library_files.push((path, metadata.ino() as i64));
                }
            }
        }
    }

    let _file_count = library_files.len();

    // Spawn DeriveLibraryHealthSignals to process these files
    // Note: Each directory spawns its own derivation. A future optimization
    // could batch all directories and run one final derivation.
    let spawn = if library_files.is_empty() {
        Vec::new()
    } else {
        vec![Computation::DeriveLibraryHealthSignals {
            library_name: library_name.to_string(),
            library_root: library_root.to_path_buf(),
            corpus_path_prefixes: corpus_path_prefixes.to_vec(),
            library_files,
        }]
    };

    // Verbose logging disabled - too noisy for directory-level computations
    // let _ = log_message(&format!(
    //     "[COMPUTE] ScanLibraryDirectory '{}': found {} audio files in {:?}",
    //     library_name, file_count, directory
    // ));

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, spawn)
}

/// Derive library health signals by comparing library inodes against corpus.
pub(super) fn execute_derive_library_health_signals(
    db: &Database,
    library_name: &str,
    library_root: &Path,
    corpus_path_prefixes: &[PathBuf],
    library_files: &[(PathBuf, i64)],
    witness: &ComputationWitness,
    start: Instant,
) -> ComputationResult {
    use crate::corpus::deploy::compute_deployment_path_with_tags;

    let computation = Computation::DeriveLibraryHealthSignals {
        library_name: library_name.to_string(),
        library_root: library_root.to_path_buf(),
        corpus_path_prefixes: corpus_path_prefixes.to_vec(),
        library_files: library_files.to_vec(),
    };

    // Get signal sender for async writes
    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    // Get all corpus track inodes (inode -> corpus_path)
    let corpus_inodes = db.get_all_track_inodes().unwrap_or_default();

    let mut healthy_count: usize = 0;
    let mut stale_count: usize = 0;
    let mut orphan_count: usize = 0;

    // Check each library file against corpus
    for (library_path, library_inode) in library_files {
        let orphan_key = format!(
            "library_orphan:{}:{}",
            library_name,
            library_path.display()
        );
        let stale_key = format!(
            "library_stale:{}:{}",
            library_name,
            library_path.display()
        );

        if let Some(corpus_path) = corpus_inodes.get(library_inode) {
            // Inode match found - this file is deployed from corpus
            // Clear any orphan signal that might have existed
            clear_file_signal_if_present(db, &sender, FileSignalType::LibraryOrphan, &orphan_key, witness);

            // Check if deployed at correct path (stale detection)
            let is_stale = if let Ok(Some(track)) = db.get_track_by_path(corpus_path) {
                if let Some(track_id) = track.id {
                    // Get tags and compute expected deployment path
                    let tags = db.get_track_tags(track_id).unwrap_or_default();
                    let tag_map: std::collections::HashMap<String, String> = tags
                        .into_iter()
                        .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
                        .collect();

                    let expected_relative = compute_deployment_path_with_tags(&track, &tag_map);
                    let expected_path = library_root.join(&expected_relative);

                    // Compare paths (normalize for comparison)
                    library_path != &expected_path
                } else {
                    false
                }
            } else {
                false
            };

            if is_stale {
                stale_count += 1;
                ensure_file_signal_if_missing(db, &sender, FileSignalType::LibraryStale, &stale_key, witness);
            } else {
                healthy_count += 1;
                clear_file_signal_if_present(db, &sender, FileSignalType::LibraryStale, &stale_key, witness);
            }
        } else {
            // No corpus match - this is an orphan
            orphan_count += 1;
            ensure_file_signal_if_missing(db, &sender, FileSignalType::LibraryOrphan, &orphan_key, witness);
            // Clear any stale signal (orphans aren't stale, they're orphans)
            clear_file_signal_if_present(db, &sender, FileSignalType::LibraryStale, &stale_key, witness);
        }
    }

    // Library health is now just per-file signals (LibraryOrphan, LibraryStale).
    // Aggregate counts are computed at UI time via queries, not stored as signals.
    // This avoids the N×M explosion when running per-directory computations.

    let _ = log_message(&format!(
        "[COMPUTE] DeriveLibraryHealthSignals '{}': {} files, {} healthy, {} stale, {} orphan",
        library_name,
        library_files.len(),
        healthy_count,
        stale_count,
        orphan_count,
    ));

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
