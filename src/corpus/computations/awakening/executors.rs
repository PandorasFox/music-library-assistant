//! Awakening-phase computation executors.
//!
//! These functions implement the actual logic for Awakening computations.

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::logging::log_general;
use crate::corpus::computations::helpers::{
    drop_stale_file_signal, drop_stale_inode_signal,
    ensure_file_signal_if_missing, ensure_file_signal_with_metadata_if_missing,
    ensure_inode_signal_if_missing,
    enumerate_all_directories, get_configured_library_names, is_audio_file,
};
use crate::corpus::computations::types::ComputationWitness;
use crate::corpus::db::types::{CorpusFileSignalType, LibraryFileSignalType};
use crate::corpus::db::ReadOnlyDb;
use crate::corpus::paths;
use crate::db_thread;

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
    let sender = match db_thread::signal_sender() {
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

    for indexed_dir in &indexed_dirs {
        // Paths in DB are root-relative (e.g., "corpus/physical/cd/...")
        // Use resolver.resolve() to get absolute path
        let abs_path = resolver.resolve(indexed_dir);
        let dir_str = indexed_dir.to_string_lossy().to_string();

        if abs_path.exists() && abs_path.is_dir() {
            // Directory exists - clear any stale MissingDirectory signal
            drop_stale_file_signal(
                read_only_db,
                &sender,
                CorpusFileSignalType::MissingDirectory.into(),
                &dir_str,
                witness,
            );
            existing_dir_count += 1;
        } else {
            // Directory is missing - emit MissingDirectory signal
            ensure_file_signal_if_missing(
                read_only_db,
                &sender,
                CorpusFileSignalType::MissingDirectory.into(),
                &dir_str,
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
    // Schedule Global Corpus Signal Derivation
    // ========================================================================
    // Single global DeriveCorpusSignals replaces per-directory derivation
    log_general("[COMPUTE] ScheduleSecondLevelDerivations: spawning global DeriveCorpusSignals");

    let mut spawn: Vec<Computation> = vec![Computation::DeriveCorpusSignals];

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
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    log_general("[COMPUTE] DeriveCorpusSignals: starting global inode comparison");

    // Get signal sender for async writes
    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                Computation::DeriveCorpusSignals,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Get disk state: FileInCorpus signals (inode -> path)
    let disk_inodes = match read_only_db.get_file_in_corpus_inodes() {
        Ok(inodes) => inodes,
        Err(e) => {
            return Result::failure(
                Computation::DeriveCorpusSignals,
                start.elapsed().as_millis() as u64,
                format!("Failed to get FileInCorpus inodes: {}", e),
            );
        }
    };

    // Get indexed state: files table (inode -> path)
    let indexed_inodes = match read_only_db.get_all_corpus_inodes() {
        Ok(inodes) => inodes,
        Err(e) => {
            return Result::failure(
                Computation::DeriveCorpusSignals,
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
            ensure_inode_signal_if_missing(
                read_only_db,
                &sender,
                CorpusFileSignalType::UnindexedFile.into(),
                *inode,
                path,
                witness,
            );
        }
    }

    // Emit MissingFile signals for indexed files not on disk
    for inode in &index_only {
        if let Some(path) = indexed_inodes.get(inode) {
            ensure_inode_signal_if_missing(
                read_only_db,
                &sender,
                CorpusFileSignalType::MissingFile.into(),
                *inode,
                path,
                witness,
            );
            // Clear any stale HealthyFile signal
            drop_stale_inode_signal(
                read_only_db,
                &sender,
                CorpusFileSignalType::HealthyFile.into(),
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
        drop_stale_inode_signal(
            read_only_db,
            &sender,
            CorpusFileSignalType::MissingFile.into(),
            *inode,
            witness,
        );
        drop_stale_inode_signal(
            read_only_db,
            &sender,
            CorpusFileSignalType::UnindexedFile.into(),
            *inode,
            witness,
        );

        // Check if file has any OOB signal - if so, don't mark as HealthyFile
        let inode_key = inode.to_string();
        let has_oob_signal =
            read_only_db.file_signal_exists(CorpusFileSignalType::OutOfBandTagConflict.into(), &inode_key) ||
            read_only_db.file_signal_exists(CorpusFileSignalType::OutOfBandTagSync.into(), &inode_key) ||
            read_only_db.file_signal_exists(CorpusFileSignalType::MtimeOnlyMismatch.into(), &inode_key);

        if has_oob_signal {
            // File has OOB signal - NOT healthy
            drop_stale_inode_signal(
                read_only_db,
                &sender,
                CorpusFileSignalType::HealthyFile.into(),
                *inode,
                witness,
            );
        } else {
            ensure_inode_signal_if_missing(
                read_only_db,
                &sender,
                CorpusFileSignalType::HealthyFile.into(),
                *inode,
                path_str,
                witness,
            );
        }
    }

    log_general("[COMPUTE] DeriveCorpusSignals: complete");

    Result::success(
        Computation::DeriveCorpusSignals,
        start.elapsed().as_millis() as u64,
        Vec::new(),
    )
}

// ============================================================================
// Per-Directory Signal Derivation (Deprecated)
// ============================================================================

/// Derive second-level signals for files in a single directory.
///
/// DEPRECATED: Use DeriveCorpusSignals for global inode comparison.
pub fn execute_derive_directory_signals(
    read_only_db: &ReadOnlyDb<'_>,
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

    // Get indexed audio files for this directory
    let dir_str = directory.to_string_lossy();
    let audio_files = match read_only_db.get_audio_files_by_path_prefix(&dir_str) {
        Ok(af) => af,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to get audio files: {}", e),
            );
        }
    };

    // Build lookup sets
    let corpus_paths: HashSet<String> = corpus_signals
        .iter()
        .map(|s| s.issue_key.clone())
        .collect();

    let indexed_paths: HashSet<String> = audio_files
        .iter()
        .map(|af| af.path().to_string())
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
        if indexed_paths.contains(corpus_path) {
            drop_stale_file_signal(read_only_db, &sender, CorpusFileSignalType::UnindexedFile.into(), corpus_path, witness);
        } else {
            ensure_file_signal_if_missing(read_only_db, &sender, CorpusFileSignalType::UnindexedFile.into(), corpus_path, witness);
        }
    }

    // Process indexed audio files
    for path in &indexed_paths {
        if corpus_paths.contains(path) {
            drop_stale_file_signal(read_only_db, &sender, CorpusFileSignalType::MissingFile.into(), path, witness);

            // Check if file has any OOB signal - if so, don't mark as HealthyFile
            let has_oob_signal =
                read_only_db.file_signal_exists(CorpusFileSignalType::OutOfBandTagConflict.into(), path) ||
                read_only_db.file_signal_exists(CorpusFileSignalType::OutOfBandTagSync.into(), path) ||
                read_only_db.file_signal_exists(CorpusFileSignalType::MtimeOnlyMismatch.into(), path);

            if has_oob_signal {
                // File has OOB signal - NOT healthy
                drop_stale_file_signal(read_only_db, &sender, CorpusFileSignalType::HealthyFile.into(), path, witness);
            } else {
                ensure_file_signal_if_missing(read_only_db, &sender, CorpusFileSignalType::HealthyFile.into(), path, witness);
            }
            // NOTE: Deploy conflicts are handled in bulk by DetectDeployConflicts in Awake phase
        } else {
            drop_stale_file_signal(read_only_db, &sender, CorpusFileSignalType::HealthyFile.into(), path, witness);
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

        // FileInCorpus: keyed by inode, path in metadata
        ensure_inode_signal_if_missing(
            read_only_db,
            &sender,
            CorpusFileSignalType::FileInCorpus.into(),
            inode,
            &path_str,
            witness,
        );

        if indexed_info.is_some() {
            // File is indexed - clear unindexed/missing
            drop_stale_inode_signal(
                read_only_db,
                &sender,
                CorpusFileSignalType::UnindexedFile.into(),
                inode,
                witness,
            );
            drop_stale_inode_signal(
                read_only_db,
                &sender,
                CorpusFileSignalType::MissingFile.into(),
                inode,
                witness,
            );

            // Check if file has any OOB signal (keyed by inode)
            let inode_key = inode.to_string();
            let has_oob_signal =
                read_only_db.file_signal_exists(CorpusFileSignalType::OutOfBandTagConflict.into(), &inode_key) ||
                read_only_db.file_signal_exists(CorpusFileSignalType::OutOfBandTagSync.into(), &inode_key) ||
                read_only_db.file_signal_exists(CorpusFileSignalType::MtimeOnlyMismatch.into(), &inode_key);

            if has_oob_signal {
                // File has OOB signal - NOT healthy
                drop_stale_inode_signal(
                    read_only_db,
                    &sender,
                    CorpusFileSignalType::HealthyFile.into(),
                    inode,
                    witness,
                );
            } else {
                ensure_inode_signal_if_missing(
                    read_only_db,
                    &sender,
                    CorpusFileSignalType::HealthyFile.into(),
                    inode,
                    &path_str,
                    witness,
                );
            }
        } else {
            // File not indexed - mark as unindexed
            drop_stale_inode_signal(
                read_only_db,
                &sender,
                CorpusFileSignalType::HealthyFile.into(),
                inode,
                witness,
            );
            drop_stale_inode_signal(
                read_only_db,
                &sender,
                CorpusFileSignalType::MissingFile.into(),
                inode,
                witness,
            );
            ensure_inode_signal_if_missing(
                read_only_db,
                &sender,
                CorpusFileSignalType::UnindexedFile.into(),
                inode,
                &path_str,
                witness,
            );
        }
    } else if let Some(inode) = indexed_inode {
        // File doesn't exist but was indexed - clear disk signals, mark missing
        drop_stale_inode_signal(
            read_only_db,
            &sender,
            CorpusFileSignalType::FileInCorpus.into(),
            inode,
            witness,
        );
        drop_stale_inode_signal(
            read_only_db,
            &sender,
            CorpusFileSignalType::UnindexedFile.into(),
            inode,
            witness,
        );
        drop_stale_inode_signal(
            read_only_db,
            &sender,
            CorpusFileSignalType::HealthyFile.into(),
            inode,
            witness,
        );
        ensure_inode_signal_if_missing(
            read_only_db,
            &sender,
            CorpusFileSignalType::MissingFile.into(),
            inode,
            &path_str,
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
    read_only_db: &ReadOnlyDb<'_>,
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
    _read_only_db: &ReadOnlyDb<'_>,
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

    // Clear previous files for this library before re-scanning
    // Routes through db_thread which has write access
    sender.clear_library_files(library_name, witness);

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

/// Scan a single library directory and store results in files table.
///
/// The actual deploy health derivation (comparing against corpus) happens in
/// the Awake phase via DeriveDeployHealthSignals.
pub fn execute_scan_library_directory(
    _read_only_db: &ReadOnlyDb<'_>,
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
    // Capture: path, inode, mtime, file_size for new files table schema
    struct LibraryFileInfo {
        path: PathBuf,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
        file_size: i64,
    }
    let mut library_files: Vec<LibraryFileInfo> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && is_audio_file(&path) {
                if let Ok(metadata) = std::fs::metadata(&path) {
                    let mtime = metadata.modified().ok().and_then(|t| {
                        t.duration_since(std::time::UNIX_EPOCH).ok()
                    });
                    let (mtime_secs, mtime_nanos) = mtime
                        .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
                        .unwrap_or((0, 0));
                    library_files.push(LibraryFileInfo {
                        path,
                        inode: metadata.ino() as i64,
                        mtime_secs,
                        mtime_nanos,
                        file_size: metadata.len() as i64,
                    });
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

        for info in &library_files {
            // Convert file_path to relative (relative to archive root)
            let relative_file_path = resolver
                .to_relative(&info.path)
                .unwrap_or_else(|| info.path.clone());

            // Routes through db_thread which has write access
            sender.record_library_file(
                library_name,
                &relative_library_root,
                &relative_file_path,
                info.inode,
                info.mtime_secs,
                info.mtime_nanos,
                info.file_size,
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
    drop_stale_file_signal(
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
    read_only_db: &ReadOnlyDb<'_>,
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
