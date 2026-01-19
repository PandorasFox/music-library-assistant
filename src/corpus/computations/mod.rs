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
    /// Collects (inode, path, mtime_secs, mtime_nanos) for all audio files.
    /// Spawns: CompareInodes with collected disk state.
    WalkCorpus {
        root: PathBuf,
        source: String,  // "corpus" or "legacy"
        /// If true, verify tags for ALL files (paranoid mode)
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
}

impl Computation {
    /// Get the primary file path affected by this computation, if any.
    pub fn primary_path(&self) -> Option<&std::path::Path> {
        match self {
            Computation::VerifyTags { path, .. } => Some(path),
            Computation::WalkCorpus { root, .. } => Some(root),
            Computation::CompareInodes { .. } => None,
            Computation::VerifyMtime { path, .. } => Some(path),
        }
    }

    /// Get a human-readable label for this computation type.
    pub fn label(&self) -> &'static str {
        match self {
            Computation::VerifyTags { .. } => "Tag verification",
            Computation::WalkCorpus { paranoid: true, .. } => "Eyeballing (paranoid)",
            Computation::WalkCorpus { paranoid: false, .. } => "Eyeballing",
            Computation::CompareInodes { .. } => "Comparing inodes",
            Computation::VerifyMtime { .. } => "Verifying mtime",
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

        Computation::CompareInodes { source, disk_state, paranoid } => {
            execute_compare_inodes(&db, source, disk_state, *paranoid, start)
        }

        Computation::VerifyMtime { track_id, path, expected_mtime_secs, expected_mtime_nanos } => {
            execute_verify_mtime(&db, *track_id, path, *expected_mtime_secs, *expected_mtime_nanos, start)
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

/// Phase 1: Walk corpus directory and collect file state.
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

    // Collect disk state: (inode, path, mtime_secs, mtime_nanos)
    let mut disk_state: Vec<(i64, PathBuf, i64, i64)> = Vec::new();
    walk_dir_recursive(root, &mut disk_state);

    // Log walk results for diagnostics
    let _ = log_message(&format!(
        "WalkCorpus: found {} audio files in {:?}",
        disk_state.len(),
        root
    ));

    // Spawn CompareInodes with collected state, passing through paranoid flag
    let spawn = vec![Computation::CompareInodes {
        source: source.to_string(),
        disk_state,
        paranoid,
    }];

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

/// Phase 2: Compare disk state to database index.
fn execute_compare_inodes(
    db: &Database,
    source: &str,
    disk_state: &[(i64, PathBuf, i64, i64)],
    paranoid: bool,
    start: Instant,
) -> ComputationResult {
    

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
            issue_type: HealthIssueType::MissingFromDisk,
            issue_key,
            severity: HealthIssueSeverity::ManualReview,
            discovered_at: None,
            resolved_at: None,
            resolution_type: None,
            resolution_session: None,
            metadata_json: Some(metadata.to_string()),
        };

        let _ = db.insert_health_issue(&issue);
    }
}

/// Create health issues for files missing from index.
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
        "MissingFromIndex: {} files across {} directories",
        total_files,
        by_directory.len()
    ));

    for (directory, files) in by_directory {
        let issue_key = format!("missing_from_index:{}", directory);

        // Check if issue already exists
        if db.get_health_issue_by_key(HealthIssueType::MissingFromIndex, &issue_key)
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
            issue_type: HealthIssueType::MissingFromIndex,
            issue_key,
            severity: HealthIssueSeverity::Informational,
            discovered_at: None,
            resolved_at: None,
            resolution_type: None,
            resolution_session: None,
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
