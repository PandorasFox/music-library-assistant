//! Startup Heartbeat Check
//!
//! Quick validation of corpus and libraries against index at app launch.
//! Detects files missing from disk, new files not yet indexed, and library health.
//!
//! ## Health Detection Coverage
//!
//! - **missing_from_disk**: Indexed files that no longer exist on disk
//! - **new_on_disk**: Audio files in corpus not yet indexed
//! - **library_health**: Per-library deployment status (healthy, pending, stale, orphans)
//! - **health_issues**: Creates health issue entries for unindexed files

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::config::{self, Config};
use crate::corpus::db::Database;

use super::library::{check_all_libraries_health, LibraryHealthResult};

/// Result of the startup heartbeat check
#[derive(Debug, Clone)]
pub struct HeartbeatResult {
    /// Number of files in the index
    #[allow(dead_code)]
    pub indexed_count: usize,
    /// Number of audio files found on disk
    #[allow(dead_code)]
    pub disk_count: usize,
    /// Number of indexed files missing from disk
    pub missing_from_disk: usize,
    /// Number of files on disk not in index
    pub new_on_disk: usize,
    /// Number of tracks with pending tag flushes (DB differs from disk)
    pub pending_tag_flushes: usize,
    /// Health results for each configured library
    pub library_health: Vec<LibraryHealthResult>,
    /// Number of deployment conflicts detected (multiple tracks -> same path)
    pub deployment_conflicts: usize,
    /// How long the check took
    pub duration: Duration,
}

impl HeartbeatResult {
    /// Returns true if the corpus is in sync with the index
    pub fn is_corpus_healthy(&self) -> bool {
        self.missing_from_disk == 0 && self.new_on_disk == 0
    }

    /// Returns true if all libraries are healthy
    pub fn are_libraries_healthy(&self) -> bool {
        self.library_health.iter().all(|l| l.is_healthy())
    }

    /// Returns true if there are no pending tag flushes
    pub fn are_tags_synced(&self) -> bool {
        self.pending_tag_flushes == 0
    }

    /// Returns true if there are no deployment conflicts
    pub fn are_deployments_conflict_free(&self) -> bool {
        self.deployment_conflicts == 0
    }

    /// Returns true if everything is healthy
    pub fn is_healthy(&self) -> bool {
        self.is_corpus_healthy()
            && self.are_libraries_healthy()
            && self.are_tags_synced()
            && self.are_deployments_conflict_free()
    }

    /// Total count of library issues across all libraries
    pub fn total_library_issues(&self) -> usize {
        let per_library: usize = self
            .library_health
            .iter()
            .map(|l| l.not_deployed + l.stale + l.orphans)
            .sum();
        per_library + self.deployment_conflicts
    }
}

use crate::config::AUDIO_EXTENSIONS;

/// Spawn heartbeat check in a background thread
pub fn spawn_heartbeat(config: &Config) -> mpsc::Receiver<HeartbeatResult> {
    let (tx, rx) = mpsc::channel();
    let config_clone = config.clone();

    std::thread::spawn(move || {
        let result = run_heartbeat(&config_clone);
        let _ = tx.send(result);
    });

    rx
}

/// Run the heartbeat check synchronously
fn run_heartbeat(config: &Config) -> HeartbeatResult {
    let start = Instant::now();
    let corpus_root = &config.corpus_root;

    // Get database path and open connection
    let db_path = match config::get_db_path() {
        Ok(p) => p,
        Err(_) => {
            return HeartbeatResult {
                indexed_count: 0,
                disk_count: 0,
                missing_from_disk: 0,
                new_on_disk: 0,
                pending_tag_flushes: 0,
                library_health: Vec::new(),
                deployment_conflicts: 0,
                duration: start.elapsed(),
            };
        }
    };

    let db = match Database::open(&db_path) {
        Ok(d) => d,
        Err(_) => {
            return HeartbeatResult {
                indexed_count: 0,
                disk_count: 0,
                missing_from_disk: 0,
                new_on_disk: 0,
                pending_tag_flushes: 0,
                library_health: Vec::new(),
                deployment_conflicts: 0,
                duration: start.elapsed(),
            };
        }
    };

    // Get indexed inodes from scan_state for corpus
    let indexed_inodes = get_indexed_inodes(&db);

    // Walk corpus directory and collect audio file inodes and paths
    let (disk_inodes, disk_inode_paths) = walk_corpus_inodes_with_paths(corpus_root);

    // Calculate differences
    let missing_inodes: HashSet<i64> = indexed_inodes.difference(&disk_inodes).cloned().collect();
    let missing_from_disk = missing_inodes.len();

    // Find new files (not in index)
    let new_inodes: HashSet<i64> = disk_inodes.difference(&indexed_inodes).cloned().collect();
    let new_on_disk = new_inodes.len();

    // Log missing files for debugging
    if missing_from_disk > 0 {
        if let Ok(missing_paths) = db.get_scan_state_paths_for_inodes("corpus", &missing_inodes) {
            let _ = config::log_message(&format!(
                "Heartbeat: {} files missing from disk:",
                missing_from_disk
            ));
            for path in &missing_paths {
                let _ = config::log_message(&format!("  - {}", path));
            }
        }
    }

    // Create health issues for unindexed files (grouped by directory)
    if new_on_disk > 0 {
        create_missing_from_index_issues(&db, &new_inodes, &disk_inode_paths);
    }

    // Check library health
    let library_health = check_all_libraries_health(config, &db);

    // Check for pending tag flushes (DB differs from disk)
    let pending_tag_flushes = db.get_tag_mismatch_count().unwrap_or(0);

    // Detect deployment conflicts (multiple corpus files -> same library path)
    // This also cleans up any previously detected conflicts that are now resolved
    let _ = super::detection::cleanup_resolved_deployment_conflicts(config, &db);
    let deployment_conflicts = super::detection::detect_deployment_conflicts(config, &db)
        .unwrap_or(0);

    HeartbeatResult {
        indexed_count: indexed_inodes.len(),
        disk_count: disk_inodes.len(),
        missing_from_disk,
        new_on_disk,
        pending_tag_flushes,
        library_health,
        deployment_conflicts,
        duration: start.elapsed(),
    }
}

/// Get all indexed inodes for corpus from scan_state table
fn get_indexed_inodes(db: &Database) -> HashSet<i64> {
    // Query scan_state for all corpus inodes
    // This is faster than querying tracks table
    db.get_all_scan_state_inodes("corpus").unwrap_or_default()
}

/// Walk corpus directory and collect inodes of audio files along with their paths
fn walk_corpus_inodes_with_paths(root: &Path) -> (HashSet<i64>, std::collections::HashMap<i64, String>) {
    use std::collections::HashMap;

    let mut inodes = HashSet::new();
    let mut inode_paths: HashMap<i64, String> = HashMap::new();

    if !root.exists() {
        return (inodes, inode_paths);
    }

    walk_dir_recursive_with_paths(root, &mut inodes, &mut inode_paths);
    (inodes, inode_paths)
}

/// Recursively walk directory and collect audio file inodes and paths
fn walk_dir_recursive_with_paths(
    dir: &Path,
    inodes: &mut HashSet<i64>,
    inode_paths: &mut std::collections::HashMap<i64, String>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            walk_dir_recursive_with_paths(&path, inodes, inode_paths);
        } else if is_audio_file(&path) {
            if let Ok(metadata) = std::fs::metadata(&path) {
                let inode = metadata.ino() as i64;
                inodes.insert(inode);
                inode_paths.insert(inode, path.to_string_lossy().to_string());
            }
        }
    }
}

/// Create health issues for files missing from the index, grouped by directory
fn create_missing_from_index_issues(
    db: &Database,
    new_inodes: &HashSet<i64>,
    inode_paths: &std::collections::HashMap<i64, String>,
) {
    use crate::corpus::db::{HealthIssue, HealthIssueType, HealthIssueSeverity};
    use std::collections::HashMap;

    // Group new files by parent directory
    let mut by_directory: HashMap<String, Vec<String>> = HashMap::new();

    for inode in new_inodes {
        if let Some(path) = inode_paths.get(inode) {
            let parent = std::path::Path::new(path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "[root]".to_string());
            by_directory.entry(parent).or_default().push(path.clone());
        }
    }

    // Create/update health issue for each directory
    for (directory, files) in by_directory {
        let issue_key = format!("missing_from_index:{}", directory);

        // Check if issue already exists
        if db.get_health_issue_by_key(HealthIssueType::MissingFromIndex, &issue_key)
            .ok()
            .flatten()
            .is_some()
        {
            // Issue already exists, skip (will be cleared when files are scanned)
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

/// Check if path has audio file extension
fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| AUDIO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}
