//! Awakening-phase computation executors.
//!
//! These functions implement the actual logic for Awakening computations.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::config::log_message;
use crate::corpus::computations::helpers::{
    clear_file_signal_if_present, ensure_file_signal_if_missing, enumerate_all_directories,
    get_configured_library_names, is_audio_file,
};
use crate::corpus::computations::types::ComputationWitness;
use crate::corpus::db::types::{FileSignalType, HealthIssueType};
use crate::corpus::db::Database;
use crate::db_thread;

use super::{Computation, Result};

// ============================================================================
// Second-Level Signal Derivations
// ============================================================================

/// Schedule second-level signal derivations by spawning per-directory computations.
pub fn execute_schedule_second_level_derivations(
    db: &Database,
    start: Instant,
) -> Result {
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

    Result::success(
        Computation::ScheduleSecondLevelDerivations,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

/// Derive second-level signals for files in a single directory.
pub fn execute_derive_directory_signals(
    db: &Database,
    directory: &Path,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::DeriveDirectorySignals {
        directory: directory.to_path_buf(),
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

    // Get FileInCorpus signals for this directory
    let corpus_signals = match db.get_signals_in_directory(directory, HealthIssueType::FileInCorpus) {
        Ok(s) => s,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to get FileInCorpus signals: {}", e),
            );
        }
    };

    // Get indexed tracks for this directory
    let dir_str = directory.to_string_lossy();
    let tracks = match db.get_tracks_by_corpus_path_prefix(&dir_str) {
        Ok(t) => t,
        Err(e) => {
            return Result::failure(
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

    // Prune stale UnindexedFile signals
    if let Ok(existing_unindexed) =
        db.get_signals_in_directory(directory, HealthIssueType::UnindexedFile)
    {
        for signal in existing_unindexed {
            if !corpus_paths.contains(&signal.issue_key) {
                sender.clear_file_signal(FileSignalType::UnindexedFile, &signal.issue_key, witness);
            }
        }
    }

    // Process files in corpus
    for corpus_path in &corpus_paths {
        if indexed_paths.contains_key(corpus_path) {
            clear_file_signal_if_present(db, &sender, FileSignalType::UnindexedFile, corpus_path, witness);
        } else {
            ensure_file_signal_if_missing(db, &sender, FileSignalType::UnindexedFile, corpus_path, witness);
        }
    }

    // Process indexed tracks
    for (path, _track) in &indexed_paths {
        if corpus_paths.contains(path) {
            clear_file_signal_if_present(db, &sender, FileSignalType::MissingFile, path, witness);
            ensure_file_signal_if_missing(db, &sender, FileSignalType::HealthyFile, path, witness);
            // NOTE: Deploy conflicts are handled in bulk by DetectDeployConflicts in Awake phase
        } else {
            clear_file_signal_if_present(db, &sender, FileSignalType::HealthyFile, path, witness);
            ensure_file_signal_if_missing(db, &sender, FileSignalType::MissingFile, path, witness);
        }
    }

    Result::success(
        computation,
        start.elapsed().as_millis() as u64,
        Vec::new(), // No spawn - deploy conflicts handled in Awake phase
    )
}

/// Update signals for a single file after a mutation.
pub fn execute_update_file_signals(
    db: &Database,
    path: &Path,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::UpdateFileSignals {
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

    let path_str = path.to_string_lossy().to_string();
    let file_exists = path.exists() && is_audio_file(path);
    let is_indexed = db.get_track_by_path(&path_str).ok().flatten().is_some();

    if file_exists {
        ensure_file_signal_if_missing(db, &sender, FileSignalType::FileInCorpus, &path_str, witness);

        if is_indexed {
            clear_file_signal_if_present(db, &sender, FileSignalType::UnindexedFile, &path_str, witness);
            clear_file_signal_if_present(db, &sender, FileSignalType::MissingFile, &path_str, witness);
            ensure_file_signal_if_missing(db, &sender, FileSignalType::HealthyFile, &path_str, witness);
        } else {
            clear_file_signal_if_present(db, &sender, FileSignalType::HealthyFile, &path_str, witness);
            clear_file_signal_if_present(db, &sender, FileSignalType::MissingFile, &path_str, witness);
            ensure_file_signal_if_missing(db, &sender, FileSignalType::UnindexedFile, &path_str, witness);
        }
    } else {
        clear_file_signal_if_present(db, &sender, FileSignalType::FileInCorpus, &path_str, witness);
        clear_file_signal_if_present(db, &sender, FileSignalType::UnindexedFile, &path_str, witness);

        if is_indexed {
            clear_file_signal_if_present(db, &sender, FileSignalType::HealthyFile, &path_str, witness);
            ensure_file_signal_if_missing(db, &sender, FileSignalType::MissingFile, &path_str, witness);
        } else {
            clear_file_signal_if_present(db, &sender, FileSignalType::HealthyFile, &path_str, witness);
            clear_file_signal_if_present(db, &sender, FileSignalType::MissingFile, &path_str, witness);
        }
    }

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Library Health Scanning
// ============================================================================

/// Walk a library directory tree and spawn per-directory scans.
pub fn execute_walk_library(
    db: &Database,
    library_root: &Path,
    library_name: &str,
    corpus_path_prefixes: &[PathBuf],
    start: Instant,
) -> Result {
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
        return Result::success(
            computation,
            start.elapsed().as_millis() as u64,
            Vec::new(),
        );
    }

    // Clear previous scan state for this library before re-scanning
    if let Err(e) = db.clear_library_scan_state(library_name) {
        let _ = log_message(&format!(
            "[COMPUTE] WalkLibrary '{}': failed to clear scan state: {}",
            library_name, e
        ));
    }

    let (directories, _symlink_count) = enumerate_all_directories(library_root);

    let _ = log_message(&format!(
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

/// Scan a single library directory and store results in library_scan_state table.
///
/// The actual deploy health derivation (comparing against corpus) happens in
/// the Awake phase via DeriveDeployHealthSignals.
pub fn execute_scan_library_directory(
    db: &Database,
    directory: &Path,
    library_name: &str,
    library_root: &Path,
    corpus_path_prefixes: &[PathBuf],
    start: Instant,
) -> Result {
    let computation = Computation::ScanLibraryDirectory {
        directory: directory.to_path_buf(),
        library_name: library_name.to_string(),
        library_root: library_root.to_path_buf(),
        corpus_path_prefixes: corpus_path_prefixes.to_vec(),
    };

    // Collect audio files in this directory (non-recursive)
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

    // Store library scan results in DB for Awake phase to process
    if !library_files.is_empty() {
        let scanned_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        for (file_path, inode) in &library_files {
            if let Err(e) = db.record_library_file(
                library_name,
                library_root,
                file_path,
                *inode,
                scanned_at,
            ) {
                let _ = log_message(&format!(
                    "[COMPUTE] ScanLibraryDirectory: failed to record file {:?}: {}",
                    file_path, e
                ));
            }
        }
    }

    // No spawn - DeriveDeployHealthSignals runs from ScheduleContentAnalysis in Awake phase
    // corpus_path_prefixes stored with entries for later use by DeriveDeployHealthSignals
    let _ = corpus_path_prefixes; // Suppress unused warning - needed for logging/future use

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
