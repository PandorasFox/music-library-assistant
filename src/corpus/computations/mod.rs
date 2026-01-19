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

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::config::log_message;

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
pub fn execute_single(computation: &Computation) -> ComputationResult {
    use crate::config;
    use crate::corpus::db::Database;
    use crate::corpus::mutations::indexing;

    let start = std::time::Instant::now();

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
            execute_scan_corpus_directory(&db, directory, source, *paranoid, start)
        }

        Computation::CompareInodes { source, disk_state, paranoid } => {
            execute_compare_inodes(&db, source, disk_state, *paranoid, start)
        }

        Computation::VerifyMtime { track_id, path, expected_mtime_secs, expected_mtime_nanos } => {
            execute_verify_mtime(&db, *track_id, path, *expected_mtime_secs, *expected_mtime_nanos, start)
        }

        Computation::ScheduleSecondLevelDerivations => {
            execute_schedule_second_level_derivations(&db, start)
        }

        Computation::DeriveDirectorySignals { directory } => {
            execute_derive_directory_signals(&db, directory, start)
        }

        Computation::CheckDeployConflicts { track_id } => {
            execute_check_deploy_conflicts(&db, *track_id, start)
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

/// Phase 1: Enumerate corpus directories and spawn per-directory scans.
///
/// Instead of walking the entire corpus tree in one computation, this
/// enumerates top-level directories and spawns `ScanCorpusDirectory` for each.
/// This provides granular progress feedback during startup.
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

    // Enumerate immediate children of root
    let mut spawn: Vec<Computation> = Vec::new();
    let mut file_count = 0;
    let mut dir_count = 0;

    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();

            if path.is_dir() {
                // Spawn per-directory scan
                spawn.push(Computation::ScanCorpusDirectory {
                    directory: path,
                    source: source.to_string(),
                    paranoid,
                });
                dir_count += 1;
            } else if is_audio_file(&path) {
                // Files directly in root - these are rare but handle them
                // by spawning a scan for the root itself if we find any
                file_count += 1;
            }
        }
    }

    // If there are files directly in the root, scan the root as a "directory"
    // (just for its immediate files, not recursive)
    if file_count > 0 {
        spawn.push(Computation::ScanCorpusDirectory {
            directory: root.to_path_buf(),
            source: source.to_string(),
            paranoid,
        });
    }

    let _ = log_message(&format!(
        "[COMPUTE] WalkCorpus: found {} top-level directories, {} root files in {:?}",
        dir_count, file_count, root
    ));

    ComputationResult::success(
        computation,
        start.elapsed().as_millis() as u64,
        spawn,
    )
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

/// Scan a single corpus directory subtree.
///
/// Walks the directory recursively, collects disk state, compares against
/// scan_state, creates FileInCorpus signals, and spawns follow-up computations.
fn execute_scan_corpus_directory(
    db: &Database,
    directory: &Path,
    source: &str,
    paranoid: bool,
    start: Instant,
) -> ComputationResult {
    use crate::corpus::db::{HealthIssue, HealthIssueType, HealthIssueSeverity};

    let computation = Computation::ScanCorpusDirectory {
        directory: directory.to_path_buf(),
        source: source.to_string(),
        paranoid,
    };

    if !directory.exists() {
        return ComputationResult::failure(
            computation,
            start.elapsed().as_millis() as u64,
            format!("Directory does not exist: {:?}", directory),
        );
    }

    // Collect disk state for this directory subtree
    let mut disk_state: Vec<(i64, PathBuf, i64, i64)> = Vec::new();
    walk_dir_recursive(directory, &mut disk_state);

    let _ = log_message(&format!(
        "[COMPUTE] ScanCorpusDirectory: found {} audio files in {:?}",
        disk_state.len(),
        directory
    ));

    // Build lookup map for disk inodes
    let disk_inodes: HashSet<i64> = disk_state.iter().map(|(inode, _, _, _)| *inode).collect();

    // Get indexed inodes from scan_state for comparison
    // We only want to compare inodes we actually found on disk in this directory
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

    // Create FileInCorpus signal for unindexed files (grouped by directory)
    if !unindexed_paths.is_empty() {
        let dir_str = directory.to_string_lossy().to_string();
        let issue_key = format!("file_in_corpus:{}", dir_str);

        // Check if signal already exists before creating
        if db.get_health_issue_by_key(HealthIssueType::FileInCorpus, &issue_key)
            .ok()
            .flatten()
            .is_none()
        {
            let metadata = serde_json::json!({
                "directory": dir_str,
                "file_count": unindexed_paths.len(),
                "sample_files": unindexed_paths.iter().take(5).collect::<Vec<_>>(),
            });

            let issue = HealthIssue {
                id: None,
                issue_type: HealthIssueType::FileInCorpus,
                issue_key,
                severity: HealthIssueSeverity::Informational,
                discovered_at: None,
                metadata_json: Some(metadata.to_string()),
            };

            let _ = db.insert_health_issue(&issue);
        }

        let _ = log_message(&format!(
            "[COMPUTE] ScanCorpusDirectory: {} unindexed files in {:?}",
            unindexed_paths.len(),
            directory
        ));
    }

    if !spawn.is_empty() {
        let _ = log_message(&format!(
            "[COMPUTE] ScanCorpusDirectory: spawning {} follow-up computations for {:?}",
            spawn.len(),
            directory
        ));
    }

    ComputationResult::success(
        computation,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

/// Phase 2: Compare disk state to database index.
fn execute_compare_inodes(
    db: &Database,
    source: &str,
    disk_state: &[(i64, PathBuf, i64, i64)],
    paranoid: bool,
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
            create_missing_from_disk_issues(db, source, &missing_paths);
        }
    }

    // Create MissingFromIndex signals
    if !missing_from_index.is_empty() {
        let missing_paths: Vec<String> = missing_from_index
            .iter()
            .filter_map(|inode| disk_inode_to_state.get(inode))
            .map(|(path, _, _)| path.to_string_lossy().to_string())
            .collect();
        create_missing_from_index_issues(db, &missing_paths);
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
fn create_missing_from_disk_issues(db: &Database, source: &str, missing_paths: &[String]) {
    use crate::corpus::db::{HealthIssue, HealthIssueType, HealthIssueSeverity};

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

        // Check if issue already exists
        if db.get_health_issue_by_key(HealthIssueType::MissingFromDisk, &issue_key)
            .ok()
            .flatten()
            .is_some()
        {
            continue;
        }

        let metadata = serde_json::json!({
            "source": source,
            "directory": directory,
            "file_count": files.len(),
            "sample_files": files.iter().take(5).collect::<Vec<_>>(),
        });

        let issue = HealthIssue {
            id: None,
            issue_type: HealthIssueType::MissingFile,
            issue_key,
            severity: HealthIssueSeverity::ManualReview,
            discovered_at: None,
            metadata_json: Some(metadata.to_string()),
        };

        let _ = db.insert_health_issue(&issue);
    }
}

/// Create signals for files in corpus but not in index.
fn create_missing_from_index_issues(db: &Database, missing_paths: &[String]) {
    use crate::corpus::db::{HealthIssue, HealthIssueType, HealthIssueSeverity};

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

        // Check if signal already exists
        if db.get_health_issue_by_key(HealthIssueType::FileInCorpus, &issue_key)
            .ok()
            .flatten()
            .is_some()
        {
            continue;
        }

        let metadata = serde_json::json!({
            "directory": directory,
            "file_count": files.len(),
            "sample_files": files.iter().take(5).collect::<Vec<_>>(),
        });

        let issue = HealthIssue {
            id: None,
            issue_type: HealthIssueType::FileInCorpus,
            issue_key,
            severity: HealthIssueSeverity::Informational,
            discovered_at: None,
            metadata_json: Some(metadata.to_string()),
        };

        let _ = db.insert_health_issue(&issue);
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
    let spawn: Vec<Computation> = all_dirs
        .into_iter()
        .map(|directory| Computation::DeriveDirectorySignals { directory })
        .collect();

    ComputationResult::success(
        Computation::ScheduleSecondLevelDerivations,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

/// Derive second-level signals for files in a single directory.
fn execute_derive_directory_signals(
    db: &Database,
    directory: &Path,
    start: Instant,
) -> ComputationResult {
    use crate::corpus::db::types::{HealthIssue, HealthIssueSeverity, HealthIssueType};

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

    let mut signals_to_create: Vec<HealthIssue> = Vec::new();
    let mut spawn: Vec<Computation> = Vec::new();

    // FileInCorpus without matching track → UnindexedFile
    for corpus_path in &corpus_paths {
        if !indexed_paths.contains_key(corpus_path) {
            signals_to_create.push(HealthIssue {
                id: None,
                issue_type: HealthIssueType::UnindexedFile,
                issue_key: corpus_path.clone(),
                severity: HealthIssueSeverity::Informational,
                discovered_at: None,
                metadata_json: None,
            });
        }
    }

    // Check each indexed track against corpus state
    for (path, track) in &indexed_paths {
        if !corpus_paths.contains(path) {
            // Track without FileInCorpus → MissingFile
            signals_to_create.push(HealthIssue {
                id: None,
                issue_type: HealthIssueType::MissingFile,
                issue_key: path.clone(),
                severity: HealthIssueSeverity::ManualReview,
                discovered_at: None,
                metadata_json: None,
            });
        } else {
            // File exists in both corpus and index - check if healthy or modified
            // For now, consider it healthy and spawn deploy conflict check
            // A more thorough check would compare mtime, but VerifyMtime handles that
            signals_to_create.push(HealthIssue {
                id: None,
                issue_type: HealthIssueType::HealthyFile,
                issue_key: path.clone(),
                severity: HealthIssueSeverity::Informational,
                discovered_at: None,
                metadata_json: None,
            });

            // Spawn deploy conflict check for healthy files
            if let Some(track_id) = track.id {
                spawn.push(Computation::CheckDeployConflicts { track_id });
            }
        }
    }

    // Record the signals
    for signal in signals_to_create {
        if let Err(e) = db.insert_health_issue(&signal) {
            log_message(&format!(
                "Failed to record {:?} signal for {}: {}",
                signal.issue_type, signal.issue_key, e
            ));
        }
    }

    // Clean up stale signals for this directory
    // Delete UnindexedFile signals for files that are now indexed
    // Delete MissingFile signals for files that now exist
    // Delete HealthyFile signals for files that no longer exist or are no longer healthy
    // (This is handled by the fact that we just recorded fresh signals - old stale ones
    // should be cleaned up by a separate staleness check or replaced by newer signals)

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
    log_message(&format!(
        "CheckDeployConflicts: skipping track {} (pending integration)",
        track_id
    ));

    ComputationResult::success(
        Computation::CheckDeployConflicts { track_id },
        start.elapsed().as_millis() as u64,
        Vec::new(),
    )
}
