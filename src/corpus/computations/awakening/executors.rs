//! Awakening-phase computation executors.
//!
//! These functions implement the actual logic for Awakening computations.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::logging::log_general;
use crate::corpus::computations::helpers::{
    clear_file_signal_if_present, ensure_file_signal_if_missing,
    ensure_file_signal_with_metadata_if_missing, enumerate_all_directories,
    get_configured_library_names, is_audio_file,
};
use crate::corpus::computations::types::ComputationWitness;
use crate::corpus::db::types::{CorpusFileSignalType, LibraryFileSignalType};
use crate::corpus::db::Database;
use crate::corpus::paths;
use crate::db_thread;

use super::{Computation, Result};

// ============================================================================
// Second-Level Signal Derivations
// ============================================================================

/// Schedule second-level signal derivations by spawning per-directory computations.
pub fn execute_schedule_second_level_derivations(
    read_only_db: &Database,
    start: Instant,
) -> Result {
    log_general("[COMPUTE] ScheduleSecondLevelDerivations: starting");

    // Get all directories that have:
    // 1. FileInCorpus signals (corpus directories with audio files)
    // 2. Indexed tracks (may be missing from corpus now)
    let corpus_dirs = read_only_db.get_distinct_corpus_directories().unwrap_or_default();
    log_general(format!(
        "[COMPUTE] Found {} directories with FileInCorpus signals",
        corpus_dirs.len()
    ));

    let index_dirs = read_only_db.get_distinct_track_directories().unwrap_or_default();
    log_general(format!(
        "[COMPUTE] Found {} directories with indexed tracks",
        index_dirs.len()
    ));

    // Union of all directories that need second-level signal derivation
    let all_dirs: HashSet<PathBuf> = corpus_dirs.into_iter()
        .chain(index_dirs.into_iter())
        .collect();

    log_general(format!(
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

/// Derive second-level signals for files in a single directory.
pub fn execute_derive_directory_signals(
    read_only_db: &Database,
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
    let corpus_signals = match read_only_db.get_signals_in_directory(directory, CorpusFileSignalType::FileInCorpus.into()) {
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
    let tracks = match read_only_db.get_tracks_by_corpus_path_prefix(&dir_str) {
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
        read_only_db.get_signals_in_directory(directory, CorpusFileSignalType::UnindexedFile.into())
    {
        for signal in existing_unindexed {
            if !corpus_paths.contains(&signal.issue_key) {
                sender.clear_file_signal(CorpusFileSignalType::UnindexedFile.into(), &signal.issue_key, witness);
            }
        }
    }

    // Process files in corpus
    for corpus_path in &corpus_paths {
        if indexed_paths.contains_key(corpus_path) {
            clear_file_signal_if_present(read_only_db, &sender, CorpusFileSignalType::UnindexedFile.into(), corpus_path, witness);
        } else {
            ensure_file_signal_if_missing(read_only_db, &sender, CorpusFileSignalType::UnindexedFile.into(), corpus_path, witness);
        }
    }

    // Process indexed tracks
    for (path, _track) in &indexed_paths {
        if corpus_paths.contains(path) {
            clear_file_signal_if_present(read_only_db, &sender, CorpusFileSignalType::MissingFile.into(), path, witness);
            ensure_file_signal_if_missing(read_only_db, &sender, CorpusFileSignalType::HealthyFile.into(), path, witness);
            // NOTE: Deploy conflicts are handled in bulk by DetectDeployConflicts in Awake phase
        } else {
            clear_file_signal_if_present(read_only_db, &sender, CorpusFileSignalType::HealthyFile.into(), path, witness);
            ensure_file_signal_if_missing(read_only_db, &sender, CorpusFileSignalType::MissingFile.into(), path, witness);
        }
    }

    Result::success(
        computation,
        start.elapsed().as_millis() as u64,
        Vec::new(), // No spawn - deploy conflicts handled in Awake phase
    )
}

/// Update corpus signals for a single file after a mutation.
///
/// Only valid for paths within the corpus directory.
pub fn execute_update_corpus_file_signals(
    read_only_db: &Database,
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

    // Convert absolute path to relative for DB queries and signal keys
    let resolver = paths::get_resolver();
    let relative_path = resolver
        .to_relative(path)
        .unwrap_or_else(|| path.to_path_buf());
    let path_str = relative_path.to_string_lossy().to_string();

    let file_exists = path.exists() && is_audio_file(path);
    let is_indexed = read_only_db.get_track_by_path(&path_str).ok().flatten().is_some();

    if file_exists {
        ensure_file_signal_if_missing(read_only_db, &sender, CorpusFileSignalType::FileInCorpus.into(), &path_str, witness);

        if is_indexed {
            clear_file_signal_if_present(read_only_db, &sender, CorpusFileSignalType::UnindexedFile.into(), &path_str, witness);
            clear_file_signal_if_present(read_only_db, &sender, CorpusFileSignalType::MissingFile.into(), &path_str, witness);
            ensure_file_signal_if_missing(read_only_db, &sender, CorpusFileSignalType::HealthyFile.into(), &path_str, witness);
        } else {
            clear_file_signal_if_present(read_only_db, &sender, CorpusFileSignalType::HealthyFile.into(), &path_str, witness);
            clear_file_signal_if_present(read_only_db, &sender, CorpusFileSignalType::MissingFile.into(), &path_str, witness);
            ensure_file_signal_if_missing(read_only_db, &sender, CorpusFileSignalType::UnindexedFile.into(), &path_str, witness);
        }
    } else {
        clear_file_signal_if_present(read_only_db, &sender, CorpusFileSignalType::FileInCorpus.into(), &path_str, witness);
        clear_file_signal_if_present(read_only_db, &sender, CorpusFileSignalType::UnindexedFile.into(), &path_str, witness);

        if is_indexed {
            clear_file_signal_if_present(read_only_db, &sender, CorpusFileSignalType::HealthyFile.into(), &path_str, witness);
            ensure_file_signal_if_missing(read_only_db, &sender, CorpusFileSignalType::MissingFile.into(), &path_str, witness);
        } else {
            clear_file_signal_if_present(read_only_db, &sender, CorpusFileSignalType::HealthyFile.into(), &path_str, witness);
            clear_file_signal_if_present(read_only_db, &sender, CorpusFileSignalType::MissingFile.into(), &path_str, witness);
        }
    }

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

/// Update library signals for a single file after a mutation.
///
/// Only valid for paths within library directories.
/// Handles LibraryLeftover signals when files are added/removed from libraries.
pub fn execute_update_library_file_signals(
    read_only_db: &Database,
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
        // These use compound keys, so we search and clear matching ones
        clear_library_signals_for_path(read_only_db, &sender, &path_str, witness);
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
    _read_only_db: &Database,
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

    // Clear previous scan state for this library before re-scanning
    // Routes through db_thread which has write access
    sender.clear_library_scan_state(library_name, witness);

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

/// Scan a single library directory and store results in library_scan_state table.
///
/// The actual deploy health derivation (comparing against corpus) happens in
/// the Awake phase via DeriveDeployHealthSignals.
pub fn execute_scan_library_directory(
    _read_only_db: &Database,
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

        for (file_path, inode) in &library_files {
            // Convert file_path to relative (relative to archive root)
            let relative_file_path = resolver
                .to_relative(file_path)
                .unwrap_or_else(|| file_path.clone());

            // Routes through db_thread which has write access
            sender.record_library_file(
                library_name,
                &relative_library_root,
                &relative_file_path,
                *inode,
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
    read_only_db: &Database,
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

    // Clear library-side signals for this path (any library name)
    // These use keys like "library_leftover:{name}:{path}" so we need to find and clear them
    clear_library_signals_for_path(read_only_db, &sender, &library_path_str, witness);

    // Clear DeployReady for corpus file
    clear_file_signal_if_present(
        read_only_db,
        &sender,
        LibraryFileSignalType::DeployReady.into(),
        &corpus_path_str,
        witness,
    );

    // Ensure DeployedHealthy with library_path in metadata (store relative path)
    let metadata = serde_json::json!({
        "library_path": library_path_str,
    });
    ensure_file_signal_with_metadata_if_missing(
        read_only_db,
        &sender,
        LibraryFileSignalType::DeployedHealthy.into(),
        &corpus_path_str,
        &metadata.to_string(),
        witness,
    );

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

/// Clear library-side signals (LibraryLeftover, LibraryStale) for a library path.
///
/// These signals use compound keys like "library_leftover:{name}:{path}",
/// so we query existing signals and clear matching ones.
fn clear_library_signals_for_path(
    read_only_db: &Database,
    sender: &db_thread::SignalWriteSender,
    library_path: &str,
    witness: &ComputationWitness,
) {
    // Check for LibraryLeftover signals matching this path
    if let Ok(signals) = read_only_db.get_signals(Some(LibraryFileSignalType::LibraryLeftover.into())) {
        for signal in signals {
            // Key format: "library_leftover:{name}:{path}"
            if signal.issue_key.ends_with(&format!(":{}", library_path)) {
                sender.clear_file_signal(LibraryFileSignalType::LibraryLeftover.into(), &signal.issue_key, witness);
            }
        }
    }

    // Check for LibraryStale signals matching this path
    if let Ok(signals) = read_only_db.get_signals(Some(LibraryFileSignalType::LibraryStale.into())) {
        for signal in signals {
            // Key format: "library_stale:{name}:{path}"
            if signal.issue_key.ends_with(&format!(":{}", library_path)) {
                sender.clear_file_signal(LibraryFileSignalType::LibraryStale.into(), &signal.issue_key, witness);
            }
        }
    }
}
