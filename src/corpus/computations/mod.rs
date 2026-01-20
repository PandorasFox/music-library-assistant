//! Corpus Computations Module
//!
//! Computations are derived facts computed from corpus/index state. Unlike Mutations,
//! they do not alter state - they only emit signals. This distinction is important
//! because:
//!
//! - **Mutations** require a `DecisionWitness` to queue (user-led decision context)
//! - **Computations** can be queued without a witness (config-driven, automatic)
//!
//! ## Eyeballing Computation Chain
//!
//! The eyeballing system uses computation chaining:
//!
//! ```text
//! WalkCorpus ──► CompareInodes ──► VerifyMtime ──► VerifyTags
//!                    │                   │              │
//!                    ▼                   ▼              ▼
//!             MissingFromDisk     (spawn next)   OutOfBandTagChange
//!             MissingFromIndex
//! ```
//!
//! Each computation can spawn follow-up computations via `ComputationResult::spawn`.
//!
//! ## Architecture
//!
//! ```text
//! Eyeballing (automatic)
//!     │
//!     └──► TaskDaemon.queue_computation() ───► Computation Executor
//!                                                    │
//!                                                    ├──► Signals (health_issues table)
//!                                                    └──► Spawned Computations (chained)
//! ```
//!
//! Computations never bypass the witness system because they don't alter state.
//! They only observe and record findings.
//!
//! ## ComputationWitness
//!
//! Signal-altering operations (insert, delete, upsert) require a `ComputationWitness`
//! which can only be created inside computation execution. This ensures signals are
//! only modified through the computation system, not ad-hoc from UI or other code.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::config::log_message;

// ============================================================================
// ComputationWitness - Proof of Computation Execution Context
// ============================================================================

/// Sealed module to prevent external construction of ComputationWitness.
mod sealed {
    /// Zero-sized proof that code is executing within a computation context.
    ///
    /// This witness is required by signal-altering database operations to ensure
    /// health signals are only modified through the computation system.
    ///
    /// Cannot be constructed outside of `execute_single`.
    #[derive(Debug, Clone, Copy)]
    pub struct ComputationWitness(());

    impl ComputationWitness {
        /// Create a new witness. Only callable from within this crate's computation execution.
        pub(crate) fn new() -> Self {
            Self(())
        }
    }
}

pub use sealed::ComputationWitness;

/// A computation operation that derives facts without altering corpus state.
///
/// Computations can be queued without a `DecisionWitness` because they only
/// emit signals - they don't modify files or index data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Computation {
    /// Verify tags on disk match database, recording mismatches as signals.
    ///
    /// This compares the actual file tags to what's stored in the index.
    /// Any mismatches are recorded as health signals for operator review.
    VerifyTags {
        track_id: i64,
        path: PathBuf,
    },

    // -------------------------------------------------------------------------
    // Eyeballing Computations
    // -------------------------------------------------------------------------

    /// Phase 1: Walk corpus directory tree to collect file state.
    ///
    /// Enumerates top-level directories under root and spawns per-directory scans.
    /// This provides granular progress feedback during startup.
    WalkCorpus {
        root: PathBuf,
        source: String,  // "corpus" or "legacy"
        /// If true, verify tags for ALL files (paranoid mode)
        paranoid: bool,
    },

    /// Walk and scan a single directory subtree.
    ///
    /// Walks the directory recursively, collects disk state, and compares
    /// against scan_state. Creates FileInCorpus signals and spawns
    /// mtime/tag verification as needed.
    ScanCorpusDirectory {
        directory: PathBuf,
        source: String,
        paranoid: bool,
    },

    /// Phase 2: Compare disk state to database index.
    ///
    /// Creates signals for:
    /// - MissingFromDisk: indexed files not on disk
    /// - MissingFromIndex: disk files not indexed
    ///
    /// Spawns: VerifyMtime for files with mtime mismatches (non-paranoid)
    ///         or VerifyTags for all files (paranoid mode)
    CompareInodes {
        source: String,
        /// Disk state: (inode, path, mtime_secs, mtime_nanos)
        disk_state: Vec<(i64, PathBuf, i64, i64)>,
        /// If true, verify tags for ALL files (paranoid mode)
        paranoid: bool,
    },

    /// Phase 3: Verify single file mtime.
    ///
    /// Checks if current mtime differs from expected (from scan_state).
    /// Spawns: VerifyTags if mtime mismatched.
    VerifyMtime {
        track_id: i64,
        path: PathBuf,
        expected_mtime_secs: i64,
        expected_mtime_nanos: i64,
    },

    // -------------------------------------------------------------------------
    // Second-Level Signal Computations
    // -------------------------------------------------------------------------

    /// Schedule second-level signal derivations.
    ///
    /// This is an orchestrator computation that spawns per-directory computations.
    /// Called during daemon Awakening phase after first-level eyeballing completes.
    ///
    /// Spawns: `DeriveDirectorySignals` for each directory that has either:
    /// - FileInCorpus signals (corpus directories with audio files)
    /// - Indexed tracks (may be missing from corpus now)
    ScheduleSecondLevelDerivations,

    /// Derive second-level signals for a single directory.
    ///
    /// Compares FileInCorpus signals against indexed tracks for this directory:
    /// - FileInCorpus without matching track → `UnindexedFile`
    /// - Track without matching FileInCorpus → `MissingFile`
    /// - Track with matching FileInCorpus (same inode/mtime) → `HealthyFile`
    /// - Track with different mtime → `CorpusFileModifiedOutOfBand`
    ///
    /// For files that become `HealthyFile`, spawns `CheckDeployConflicts`.
    DeriveDirectorySignals { directory: PathBuf },

    /// Check for deployment conflicts on a healthy track.
    ///
    /// Third-level computation triggered when a file is marked as healthy.
    /// Checks if this track has deployment conflicts with other tracks.
    CheckDeployConflicts { track_id: i64 },

    // -------------------------------------------------------------------------
    // Library Health Computations
    // -------------------------------------------------------------------------

    /// Walk a library directory tree to collect file inodes.
    ///
    /// Spawns `ScanLibraryDirectory` for each subdirectory found.
    WalkLibrary {
        library_root: PathBuf,
        library_name: String,
    },

    /// Scan a single library directory and collect (path, inode) pairs.
    ///
    /// Results are accumulated in memory, then passed to DeriveLibraryHealthSignals.
    ScanLibraryDirectory {
        directory: PathBuf,
        library_name: String,
    },

    /// Derive library health signals after scanning all directories.
    ///
    /// Compares library inodes against corpus index:
    /// - Match by inode → check if path is correct (healthy vs stale)
    /// - Corpus track not in library → LibraryNotDeployed
    /// - Library file without corpus backing → LibraryOrphan
    DeriveLibraryHealthSignals {
        library_name: String,
        /// Library files: (path, inode)
        library_files: Vec<(PathBuf, i64)>,
    },
}

impl Computation {
    /// Get the primary file path affected by this computation, if any.
    pub fn primary_path(&self) -> Option<&std::path::Path> {
        match self {
            Computation::VerifyTags { path, .. } => Some(path),
            Computation::WalkCorpus { root, .. } => Some(root),
            Computation::ScanCorpusDirectory { directory, .. } => Some(directory),
            Computation::CompareInodes { .. } => None,
            Computation::VerifyMtime { path, .. } => Some(path),
            Computation::ScheduleSecondLevelDerivations => None,
            Computation::DeriveDirectorySignals { directory } => Some(directory),
            Computation::CheckDeployConflicts { .. } => None,
            Computation::WalkLibrary { library_root, .. } => Some(library_root),
            Computation::ScanLibraryDirectory { directory, .. } => Some(directory),
            Computation::DeriveLibraryHealthSignals { .. } => None,
        }
    }

    /// Get a human-readable label for this computation type.
    pub fn label(&self) -> &'static str {
        match self {
            Computation::VerifyTags { .. } => "Tag verification",
            Computation::WalkCorpus { paranoid: true, .. } => "Eyeballing (paranoid)",
            Computation::WalkCorpus { paranoid: false, .. } => "Eyeballing",
            Computation::ScanCorpusDirectory { paranoid: true, .. } => "Scanning directory (paranoid)",
            Computation::ScanCorpusDirectory { paranoid: false, .. } => "Scanning directory",
            Computation::CompareInodes { .. } => "Comparing inodes",
            Computation::VerifyMtime { .. } => "Verifying mtime",
            Computation::ScheduleSecondLevelDerivations => "Scheduling signal derivations",
            Computation::DeriveDirectorySignals { .. } => "Deriving signals",
            Computation::CheckDeployConflicts { .. } => "Checking deploy conflicts",
            Computation::WalkLibrary { .. } => "Walking library",
            Computation::ScanLibraryDirectory { .. } => "Scanning library directory",
            Computation::DeriveLibraryHealthSignals { .. } => "Deriving library health",
        }
    }
}

/// Result of executing a single computation.
#[derive(Debug)]
pub struct ComputationResult {
    pub computation: Computation,
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
    /// Follow-up computations to queue (chaining mechanism).
    pub spawn: Vec<Computation>,
}

impl ComputationResult {
    fn success(computation: Computation, duration_ms: u64, spawn: Vec<Computation>) -> Self {
        Self {
            computation,
            success: true,
            error: None,
            duration_ms,
            spawn,
        }
    }

    fn failure(computation: Computation, duration_ms: u64, error: String) -> Self {
        Self {
            computation,
            success: false,
            error: Some(error),
            duration_ms,
            spawn: Vec::new(),
        }
    }
}

/// Execute a single computation.
///
/// Opens DB connection as needed to record signals.
/// Creates a `ComputationWitness` for signal-altering operations.
pub fn execute_single(computation: &Computation) -> ComputationResult {
    use crate::config;
    use crate::corpus::db::Database;
    use crate::corpus::mutations::indexing;

    let start = std::time::Instant::now();

    // Create witness for signal operations - only valid within this execution context
    let witness = ComputationWitness::new();

    // Open database for signal recording
    let db = match config::get_db_path().and_then(|p| Database::open(&p).map_err(|e| e.into())) {
        Ok(db) => db,
        Err(e) => {
            return ComputationResult::failure(
                computation.clone(),
                start.elapsed().as_millis() as u64,
                format!("DB error: {}", e),
            );
        }
    };

    match computation {
        Computation::VerifyTags { track_id, path } => {
            // Delegate to the existing execute_verify_tags implementation
            match indexing::execute_verify_tags(&db, *track_id, path) {
                Ok(()) => ComputationResult::success(
                    computation.clone(),
                    start.elapsed().as_millis() as u64,
                    Vec::new(),
                ),
                Err(e) => ComputationResult::failure(
                    computation.clone(),
                    start.elapsed().as_millis() as u64,
                    e.to_string(),
                ),
            }
        }

        Computation::WalkCorpus { root, source, paranoid } => {
            execute_walk_corpus(&db, root, source, *paranoid, start)
        }

        Computation::ScanCorpusDirectory { directory, source, paranoid } => {
            execute_scan_corpus_directory(&db, directory, source, *paranoid, &witness, start)
        }

        Computation::CompareInodes { source, disk_state, paranoid } => {
            execute_compare_inodes(&db, source, disk_state, *paranoid, &witness, start)
        }

        Computation::VerifyMtime { track_id, path, expected_mtime_secs, expected_mtime_nanos } => {
            execute_verify_mtime(&db, *track_id, path, *expected_mtime_secs, *expected_mtime_nanos, start)
        }

        Computation::ScheduleSecondLevelDerivations => {
            execute_schedule_second_level_derivations(&db, start)
        }

        Computation::DeriveDirectorySignals { directory } => {
            execute_derive_directory_signals(&db, directory, &witness, start)
        }

        Computation::CheckDeployConflicts { track_id } => {
            execute_check_deploy_conflicts(&db, *track_id, start)
        }

        Computation::WalkLibrary { library_root, library_name } => {
            execute_walk_library(&db, library_root, library_name, start)
        }

        Computation::ScanLibraryDirectory { directory, library_name } => {
            execute_scan_library_directory(&db, directory, library_name, start)
        }

        Computation::DeriveLibraryHealthSignals { library_name, library_files } => {
            execute_derive_library_health_signals(&db, library_name, library_files, &witness, start)
        }
    }
}

// ============================================================================
// Eyeballing Computation Executors
// ============================================================================

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Instant;

use crate::config::AUDIO_EXTENSIONS;
use crate::corpus::db::Database;

/// Check if path has audio file extension.
fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| AUDIO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Phase 1: Enumerate ALL corpus directories and spawn per-directory scans.
///
/// Recursively walks the entire corpus tree to find all directories,
/// then spawns `ScanCorpusDirectory` for each. This provides granular
/// progress feedback during startup (one computation per directory).
fn execute_walk_corpus(
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
    let mut directories: Vec<PathBuf> = Vec::new();
    let mut symlink_count = 0;
    enumerate_directories_recursive(root, &mut directories, &mut symlink_count);

    // Always include root itself (for files directly in root)
    directories.push(root.to_path_buf());

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

/// Recursively enumerate all directories under a root path.
///
/// Skips symlinks and logs a warning count.
fn enumerate_directories_recursive(dir: &Path, directories: &mut Vec<PathBuf>, symlink_count: &mut usize) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Check for symlinks - we don't follow them
        if path.is_symlink() {
            if path.is_dir() {
                *symlink_count += 1;
            }
            continue;
        }

        if path.is_dir() {
            directories.push(path.clone());
            enumerate_directories_recursive(&path, directories, symlink_count);
        }
    }
}

/// Recursively walk directory and collect audio file state.
fn walk_dir_recursive(dir: &Path, disk_state: &mut Vec<(i64, PathBuf, i64, i64)>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            walk_dir_recursive(&path, disk_state);
        } else if is_audio_file(&path) {
            if let Ok(metadata) = std::fs::metadata(&path) {
                let inode = metadata.ino() as i64;
                let mtime = metadata.modified().ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
                    .unwrap_or((0, 0));

                disk_state.push((inode, path, mtime.0, mtime.1));
            }
        }
    }
}

/// Scan a single corpus directory (non-recursive).
///
/// Processes only audio files directly in the given directory, compares against
/// scan_state, creates FileInCorpus signals, and spawns follow-up computations.
fn execute_scan_corpus_directory(
    db: &Database,
    directory: &Path,
    source: &str,
    paranoid: bool,
    witness: &ComputationWitness,
    start: Instant,
) -> ComputationResult {
    use crate::corpus::db::types::HealthIssueType;

    let computation = Computation::ScanCorpusDirectory {
        directory: directory.to_path_buf(),
        source: source.to_string(),
        paranoid,
    };

    let dir_str = directory.to_string_lossy().to_string();
    let issue_key = format!("file_in_corpus:{}", dir_str);

    if !directory.exists() {
        // Directory doesn't exist - clear any existing FileInCorpus signal
        let _ = db.clear_signal(HealthIssueType::FileInCorpus, &issue_key, witness);
        return ComputationResult::failure(
            computation,
            start.elapsed().as_millis() as u64,
            format!("Directory does not exist: {:?}", directory),
        );
    }

    // Collect disk state for files DIRECTLY in this directory (not recursive)
    let disk_state = collect_directory_files(directory);

    // If no files in directory, clear any existing signal and return
    if disk_state.is_empty() {
        let _ = db.clear_signal(HealthIssueType::FileInCorpus, &issue_key, witness);
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
    let mut unindexed_paths: Vec<String> = Vec::new();

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
            // File not indexed - record for FileInCorpus signal
            unindexed_paths.push(path_str);
        }
    }

    // Manage FileInCorpus signal for this directory
    if !unindexed_paths.is_empty() {
        let metadata = serde_json::json!({
            "directory": dir_str,
            "file_count": unindexed_paths.len(),
            "sample_files": unindexed_paths.iter().take(5).collect::<Vec<_>>(),
        });

        let _ = db.ensure_signal(
            HealthIssueType::FileInCorpus,
            &issue_key,
            Some(&metadata.to_string()),
            witness,
        );
    } else {
        // All files are indexed - clear any existing FileInCorpus signal
        let _ = db.clear_signal(HealthIssueType::FileInCorpus, &issue_key, witness);
    }

    ComputationResult::success(
        computation,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

/// Collect audio files directly in a directory (non-recursive).
fn collect_directory_files(dir: &Path) -> Vec<(i64, PathBuf, i64, i64)> {
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
                let mtime = metadata.modified().ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
                    .unwrap_or((0, 0));

                disk_state.push((inode, path, mtime.0, mtime.1));
            }
        }
    }

    disk_state
}

/// Phase 2: Compare disk state to database index.
fn execute_compare_inodes(
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

/// Create health issues for files missing from disk.
fn create_missing_from_disk_issues(
    db: &Database,
    source: &str,
    missing_paths: &[String],
    witness: &ComputationWitness,
) {
    use crate::corpus::db::types::HealthIssueType;

    // Group by parent directory
    let mut by_directory: HashMap<String, Vec<String>> = HashMap::new();
    for path in missing_paths {
        let parent = std::path::Path::new(path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "[root]".to_string());
        by_directory.entry(parent).or_default().push(path.clone());
    }

    for (directory, files) in by_directory {
        let issue_key = format!("missing_from_disk:{}:{}", source, directory);

        let metadata = serde_json::json!({
            "source": source,
            "directory": directory,
            "file_count": files.len(),
            "sample_files": files.iter().take(5).collect::<Vec<_>>(),
        });

        let _ = db.ensure_signal(
            HealthIssueType::MissingFile,
            &issue_key,
            Some(&metadata.to_string()),
            witness,
        );
    }
}

/// Create signals for files in corpus but not in index.
fn create_missing_from_index_issues(
    db: &Database,
    missing_paths: &[String],
    witness: &ComputationWitness,
) {
    use crate::corpus::db::types::HealthIssueType;

    // Group by parent directory
    let mut by_directory: HashMap<String, Vec<String>> = HashMap::new();
    for path in missing_paths {
        let parent = std::path::Path::new(path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "[root]".to_string());
        by_directory.entry(parent).or_default().push(path.clone());
    }

    // Log signal creation for diagnostics
    let total_files: usize = by_directory.values().map(|v| v.len()).sum();
    let _ = log_message(&format!(
        "FileInCorpus: {} files across {} directories",
        total_files,
        by_directory.len()
    ));

    for (directory, files) in by_directory {
        let issue_key = format!("file_in_corpus:{}", directory);

        let metadata = serde_json::json!({
            "directory": directory,
            "file_count": files.len(),
            "sample_files": files.iter().take(5).collect::<Vec<_>>(),
        });

        let _ = db.ensure_signal(
            HealthIssueType::FileInCorpus,
            &issue_key,
            Some(&metadata.to_string()),
            witness,
        );
    }
}

/// Phase 3: Verify single file mtime.
fn execute_verify_mtime(
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
    let current_mtime = match std::fs::metadata(path) {
        Ok(m) => m.modified().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64)),
        Err(e) => {
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to read file metadata: {}", e),
            );
        }
    };

    let Some((current_secs, current_nanos)) = current_mtime else {
        return ComputationResult::failure(
            computation,
            start.elapsed().as_millis() as u64,
            "Failed to read file mtime".to_string(),
        );
    };

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

// ============================================================================
// Second-Level Signal Computation Executors
// ============================================================================

/// Schedule second-level signal derivations by spawning per-directory computations.
fn execute_schedule_second_level_derivations(
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
        let library_names = crate::corpus::health::library::get_configured_library_names(&config);
        let _ = log_message(&format!(
            "[COMPUTE] ScheduleSecondLevelDerivations: spawning {} library walks",
            library_names.len()
        ));

        for library_name in library_names {
            let library_root = config.libraries_root.join(&library_name);
            spawn.push(Computation::WalkLibrary {
                library_root,
                library_name,
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
fn execute_derive_directory_signals(
    db: &Database,
    directory: &Path,
    witness: &ComputationWitness,
    start: Instant,
) -> ComputationResult {
    use crate::corpus::db::types::HealthIssueType;

    let computation = Computation::DeriveDirectorySignals {
        directory: directory.to_path_buf(),
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

    // Get indexed tracks for this directory
    let tracks = match db.get_tracks_in_directory(directory) {
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

    let mut spawn: Vec<Computation> = Vec::new();

    // Process files in corpus (FileInCorpus signals)
    for corpus_path in &corpus_paths {
        if indexed_paths.contains_key(corpus_path) {
            // File is in both corpus and index → clear UnindexedFile if it exists
            let _ = db.clear_signal(HealthIssueType::UnindexedFile, corpus_path, witness);
        } else {
            // File in corpus but not indexed → ensure UnindexedFile signal
            let _ = db.ensure_signal(
                HealthIssueType::UnindexedFile,
                corpus_path,
                None,
                witness,
            );
        }
    }

    // Process indexed tracks
    for (path, track) in &indexed_paths {
        if corpus_paths.contains(path) {
            // File exists in both corpus and index → healthy
            // Clear MissingFile signal if it existed
            let _ = db.clear_signal(HealthIssueType::MissingFile, path, witness);

            // Ensure HealthyFile signal
            let _ = db.ensure_signal(
                HealthIssueType::HealthyFile,
                path,
                None,
                witness,
            );

            // Spawn deploy conflict check for healthy files
            if let Some(track_id) = track.id {
                spawn.push(Computation::CheckDeployConflicts { track_id });
            }
        } else {
            // Track without FileInCorpus → file is missing from disk
            // Clear HealthyFile signal if it existed
            let _ = db.clear_signal(HealthIssueType::HealthyFile, path, witness);

            // Ensure MissingFile signal
            let _ = db.ensure_signal(
                HealthIssueType::MissingFile,
                path,
                None,
                witness,
            );
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
fn execute_check_deploy_conflicts(
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

// ============================================================================
// Library Health Computation Executors
// ============================================================================

/// Walk a library directory tree and spawn per-directory scans.
fn execute_walk_library(
    _db: &Database,
    library_root: &Path,
    library_name: &str,
    start: Instant,
) -> ComputationResult {
    let computation = Computation::WalkLibrary {
        library_root: library_root.to_path_buf(),
        library_name: library_name.to_string(),
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
    let mut directories: Vec<PathBuf> = Vec::new();
    let mut symlink_count = 0;
    enumerate_directories_recursive(library_root, &mut directories, &mut symlink_count);

    // Include root itself
    directories.push(library_root.to_path_buf());

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
fn execute_scan_library_directory(
    _db: &Database,
    directory: &Path,
    library_name: &str,
    start: Instant,
) -> ComputationResult {
    let computation = Computation::ScanLibraryDirectory {
        directory: directory.to_path_buf(),
        library_name: library_name.to_string(),
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

    let file_count = library_files.len();

    // Spawn DeriveLibraryHealthSignals to process these files
    // Note: Each directory spawns its own derivation. A future optimization
    // could batch all directories and run one final derivation.
    let spawn = if library_files.is_empty() {
        Vec::new()
    } else {
        vec![Computation::DeriveLibraryHealthSignals {
            library_name: library_name.to_string(),
            library_files,
        }]
    };

    let _ = log_message(&format!(
        "[COMPUTE] ScanLibraryDirectory '{}': found {} audio files in {:?}",
        library_name, file_count, directory
    ));

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, spawn)
}

/// Derive library health signals by comparing library inodes against corpus.
fn execute_derive_library_health_signals(
    db: &Database,
    library_name: &str,
    library_files: &[(PathBuf, i64)],
    witness: &ComputationWitness,
    start: Instant,
) -> ComputationResult {
    use crate::corpus::db::types::{HealthIssue, HealthIssueType};

    let computation = Computation::DeriveLibraryHealthSignals {
        library_name: library_name.to_string(),
        library_files: library_files.to_vec(),
    };

    let _ = log_message(&format!(
        "[COMPUTE] DeriveLibraryHealthSignals '{}': processing {} files",
        library_name,
        library_files.len()
    ));

    // Build inode -> library_path map
    let library_inodes: HashMap<i64, &PathBuf> = library_files
        .iter()
        .map(|(path, inode)| (*inode, path))
        .collect();

    // Get all corpus track inodes
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

        if let Some(corpus_path) = corpus_inodes.get(library_inode) {
            // Inode match found - this file is deployed from corpus
            // Clear any orphan signal that might have existed
            let _ = db.clear_signal(HealthIssueType::LibraryOrphan, &orphan_key, witness);

            // For now, just count as healthy. Stale detection requires
            // comparing expected deployment path vs actual, which needs
            // the deploy path computation (currently disabled).
            healthy_count += 1;

            // TODO: When corpus::deploy is re-enabled, check if library_path
            // matches the expected deployment path. If not, create LibraryStale signal.
            let _ = corpus_path; // Suppress unused warning
        } else {
            // No corpus match - this is an orphan
            orphan_count += 1;

            let metadata = serde_json::json!({
                "library_name": library_name,
                "library_path": library_path.display().to_string(),
                "inode": library_inode,
            });

            let _ = db.ensure_signal(
                HealthIssueType::LibraryOrphan,
                &orphan_key,
                Some(&metadata.to_string()),
                witness,
            );
        }
    }

    // TODO: Check for LibraryNotDeployed (corpus tracks that should be in library but aren't)
    // This requires knowing which corpus tracks are deployable to this library,
    // which depends on the deploy_mappings config. For now, skip this check.

    let _ = log_message(&format!(
        "[COMPUTE] DeriveLibraryHealthSignals '{}': {} healthy, {} stale, {} orphan",
        library_name, healthy_count, stale_count, orphan_count
    ));

    // Replace LibraryHealthSummary signal with fresh data
    let summary_key = format!("library_health:{}", library_name);
    let summary_metadata = serde_json::json!({
        "library_name": library_name,
        "healthy_count": healthy_count,
        "stale_count": stale_count,
        "orphan_count": orphan_count,
        "total_files": library_files.len(),
    });

    let summary_issue = HealthIssue {
        id: None,
        issue_type: HealthIssueType::LibraryHealthSummary,
        issue_key: summary_key,
        discovered_at: None,
        metadata_json: Some(summary_metadata.to_string()),
    };
    let _ = db.replace_signal(&summary_issue, witness);

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
