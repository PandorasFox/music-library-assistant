//! Startup Heartbeat Check
//!
//! Quick validation of corpus and libraries against index at app launch.
//! Detects files missing from disk, new files not yet indexed, and library health.
//! Also handles automatic scanning of new files and detection of moved files.
//!
//! ## Health Detection Coverage
//!
//! - **missing_from_disk**: Indexed files that no longer exist on disk
//! - **new_on_disk**: Audio files in corpus not yet indexed (now auto-scanned)
//! - **files_relocated**: Files moved to new paths (same inode, different path)
//! - **duplicate_inodes**: Multiple tracks sharing the same inode
//! - **library_health**: Per-library deployment status (healthy, pending, stale, orphans)

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use crate::config::{self, Config};
use crate::corpus::db::{Database, HealthIssue, HealthIssueType, HealthIssueSeverity, ScanStateEntry};
use crate::corpus::metadata;

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
    /// Number of files on disk not in index (before scanning)
    pub new_on_disk: usize,
    /// Number of new files successfully scanned and indexed
    pub files_scanned: usize,
    /// Number of files detected as relocated (same inode, different path)
    pub files_relocated: usize,
    /// Number of duplicate inode issues found
    pub duplicate_inodes: usize,
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
        // Note: new_on_disk may be > 0 if scanning failed for some files,
        // but files_scanned tells us how many succeeded
        self.missing_from_disk == 0
            && self.files_relocated == 0
            && self.duplicate_inodes == 0
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

/// Results from scanning a single source (corpus or legacy)
struct SourceScanResult {
    indexed_count: usize,
    disk_count: usize,
    missing_from_disk: usize,
    new_on_disk: usize,
    files_scanned: usize,
    files_relocated: usize,
}

/// Run the heartbeat check synchronously
fn run_heartbeat(config: &Config) -> HeartbeatResult {
    let start = Instant::now();

    // Get database path and open connection
    let db_path = match config::get_db_path() {
        Ok(p) => p,
        Err(_) => {
            return HeartbeatResult {
                indexed_count: 0,
                disk_count: 0,
                missing_from_disk: 0,
                new_on_disk: 0,
                files_scanned: 0,
                files_relocated: 0,
                duplicate_inodes: 0,
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
                files_scanned: 0,
                files_relocated: 0,
                duplicate_inodes: 0,
                pending_tag_flushes: 0,
                library_health: Vec::new(),
                deployment_conflicts: 0,
                duration: start.elapsed(),
            };
        }
    };

    // Process corpus
    let corpus_result = process_source(&db, "corpus", &config.corpus_root);

    // Process legacy library if configured
    let legacy_result = config.legacy_library.as_ref().map(|legacy_path| {
        process_source(&db, "legacy", legacy_path)
    });

    // Combine results
    let (indexed_count, disk_count, missing_from_disk, new_on_disk, files_scanned, files_relocated) =
        if let Some(legacy) = legacy_result {
            (
                corpus_result.indexed_count + legacy.indexed_count,
                corpus_result.disk_count + legacy.disk_count,
                corpus_result.missing_from_disk + legacy.missing_from_disk,
                corpus_result.new_on_disk + legacy.new_on_disk,
                corpus_result.files_scanned + legacy.files_scanned,
                corpus_result.files_relocated + legacy.files_relocated,
            )
        } else {
            (
                corpus_result.indexed_count,
                corpus_result.disk_count,
                corpus_result.missing_from_disk,
                corpus_result.new_on_disk,
                corpus_result.files_scanned,
                corpus_result.files_relocated,
            )
        };

    // Detect duplicate inodes in the corpus
    let duplicate_inodes = detect_duplicate_inodes(&db);

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
        indexed_count,
        disk_count,
        missing_from_disk,
        new_on_disk,
        files_scanned,
        files_relocated,
        duplicate_inodes,
        pending_tag_flushes,
        library_health,
        deployment_conflicts,
        duration: start.elapsed(),
    }
}

/// Process a single source (corpus or legacy) - detect changes, scan new files
fn process_source(db: &Database, source: &str, root: &Path) -> SourceScanResult {
    // Get indexed inodes from scan_state for this source
    let indexed_inodes = get_indexed_inodes(db, source);

    // Walk directory and collect audio file inodes and paths
    let (disk_inodes, disk_inode_paths) = walk_corpus_inodes_with_paths(root);

    // Calculate differences
    let missing_inodes: HashSet<i64> = indexed_inodes.difference(&disk_inodes).cloned().collect();
    let new_inodes: HashSet<i64> = disk_inodes.difference(&indexed_inodes).cloned().collect();

    // Detect file relocations: inodes that are both "missing" and "new" at different paths
    let (files_relocated, remaining_missing, remaining_new) = detect_file_relocations(
        db,
        source,
        &missing_inodes,
        &new_inodes,
        &disk_inode_paths,
    );

    let missing_from_disk = remaining_missing.len();
    let new_on_disk = remaining_new.len();

    // Log missing files for debugging
    if missing_from_disk > 0 {
        if let Ok(missing_paths) = db.get_scan_state_paths_for_inodes(source, &remaining_missing) {
            let _ = config::log_message(&format!(
                "Heartbeat: {} {} files missing from disk:",
                missing_from_disk, source
            ));
            for path in &missing_paths {
                let _ = config::log_message(&format!("  - {}", path));
            }
        }
    }

    // Scan new files that aren't relocations
    let files_scanned = if !remaining_new.is_empty() {
        scan_new_files(db, source, &remaining_new, &disk_inode_paths)
    } else {
        0
    };

    // Create health issues for files that couldn't be scanned
    let unscanned_count = new_on_disk.saturating_sub(files_scanned);
    if unscanned_count > 0 {
        // Filter to only unscanned inodes for health issue creation
        let scanned_paths: HashSet<String> = remaining_new.iter()
            .filter_map(|inode| disk_inode_paths.get(inode))
            .take(files_scanned)
            .cloned()
            .collect();

        let unscanned_inodes: HashSet<i64> = remaining_new.iter()
            .filter(|inode| {
                disk_inode_paths.get(inode)
                    .map(|p| !scanned_paths.contains(p))
                    .unwrap_or(true)
            })
            .cloned()
            .collect();

        if !unscanned_inodes.is_empty() {
            create_missing_from_index_issues(db, &unscanned_inodes, &disk_inode_paths);
        }
    }

    SourceScanResult {
        indexed_count: indexed_inodes.len(),
        disk_count: disk_inodes.len(),
        missing_from_disk,
        new_on_disk,
        files_scanned,
        files_relocated,
    }
}

/// Get all indexed inodes for a source from scan_state table
fn get_indexed_inodes(db: &Database, source: &str) -> HashSet<i64> {
    // Query scan_state for all inodes of this source
    // This is faster than querying tracks table
    db.get_all_scan_state_inodes(source).unwrap_or_default()
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

/// Detect files that have been relocated (same inode at different path).
/// An inode that appears in both "missing" (indexed but not on disk at expected path)
/// and "new" (on disk but not indexed) sets indicates a file move.
///
/// Returns (relocations_count, remaining_missing_inodes, remaining_new_inodes)
fn detect_file_relocations(
    db: &Database,
    source: &str,
    missing_inodes: &HashSet<i64>,
    new_inodes: &HashSet<i64>,
    disk_inode_paths: &HashMap<i64, String>,
) -> (usize, HashSet<i64>, HashSet<i64>) {
    // Find inodes that are in both sets - these are relocations
    let relocated_inodes: HashSet<i64> = missing_inodes
        .intersection(new_inodes)
        .cloned()
        .collect();

    if relocated_inodes.is_empty() {
        return (0, missing_inodes.clone(), new_inodes.clone());
    }

    let mut relocations_created = 0;

    for inode in &relocated_inodes {
        // Get old path from scan_state
        let old_path = db
            .get_scan_state_path_for_inode(source, *inode)
            .ok()
            .flatten();

        // Get new path from disk
        let new_path = disk_inode_paths.get(inode);

        if let (Some(old), Some(new)) = (old_path, new_path) {
            let issue_key = format!("relocated:{}:{}:{}", source, inode, new);

            // Check if issue already exists
            if db.get_health_issue_by_key(HealthIssueType::FileRelocated, &issue_key)
                .ok()
                .flatten()
                .is_some()
            {
                continue;
            }

            let metadata = serde_json::json!({
                "source": source,
                "inode": inode,
                "old_path": old,
                "new_path": new,
            });

            let issue = HealthIssue {
                id: None,
                issue_type: HealthIssueType::FileRelocated,
                issue_key,
                severity: HealthIssueSeverity::ManualReview,
                discovered_at: None,
                resolved_at: None,
                resolution_type: None,
                resolution_session: None,
                metadata_json: Some(metadata.to_string()),
            };

            if db.insert_health_issue(&issue).is_ok() {
                relocations_created += 1;
                let _ = config::log_message(&format!(
                    "Heartbeat: File relocated: {} -> {}",
                    old, new
                ));
            }
        }
    }

    // Remove relocated inodes from both sets
    let remaining_missing: HashSet<i64> = missing_inodes
        .difference(&relocated_inodes)
        .cloned()
        .collect();
    let remaining_new: HashSet<i64> = new_inodes
        .difference(&relocated_inodes)
        .cloned()
        .collect();

    (relocations_created, remaining_missing, remaining_new)
}

/// Scan new files and add them to the index.
/// Returns the count of files successfully scanned.
fn scan_new_files(
    db: &Database,
    source: &str,
    new_inodes: &HashSet<i64>,
    inode_paths: &HashMap<i64, String>,
) -> usize {
    let mut scanned_count = 0;

    for inode in new_inodes {
        let path = match inode_paths.get(inode) {
            Some(p) => p,
            None => continue,
        };

        let path_obj = Path::new(path);

        // Extract metadata from the file (returns full Track)
        let track = match metadata::extract_metadata(path_obj, source) {
            Ok(t) => t,
            Err(e) => {
                let _ = config::log_message(&format!(
                    "Heartbeat: Failed to extract metadata from {}: {}",
                    path, e
                ));
                continue;
            }
        };

        // Insert track
        if db.insert_track(&track).is_err() {
            continue;
        }

        // Get file metadata for mtime
        let file_meta = match std::fs::metadata(path_obj) {
            Ok(m) => m,
            Err(_) => {
                // Track inserted but can't update scan_state - still count it
                scanned_count += 1;
                continue;
            }
        };

        let mtime = file_meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
            .unwrap_or((0, 0));

        // Create scan_state entry
        let scan_entry = ScanStateEntry {
            source: source.to_string(),
            inode: *inode,
            path: path.clone(),
            mtime_secs: mtime.0,
            mtime_nanos: mtime.1,
            file_size: track.file_size,
        };

        if db.upsert_scan_state(&scan_entry).is_ok() {
            scanned_count += 1;
        }
    }

    if scanned_count > 0 {
        let _ = config::log_message(&format!(
            "Heartbeat: Scanned and indexed {} new files",
            scanned_count
        ));
    }

    scanned_count
}

/// Detect duplicate inodes in the corpus (multiple tracks with same inode).
/// This indicates either hard links or database inconsistency.
/// Returns the count of duplicate inode issues created.
///
/// TODO: Integrate DuplicateInode detection with corpus reorganization tools.
/// When reorganizing corpus, check for and prevent creating duplicate inodes.
/// This health signal indicates either hard links or database inconsistency.
fn detect_duplicate_inodes(db: &Database) -> usize {
    // Query for inodes that appear multiple times in tracks table
    let duplicate_groups = match db.get_duplicate_inodes_in_corpus() {
        Ok(groups) => groups,
        Err(_) => return 0,
    };

    let mut issues_created = 0;

    for (inode, paths) in duplicate_groups {
        if paths.len() < 2 {
            continue;
        }

        let issue_key = format!("duplicate_inode:{}", inode);

        // Check if issue already exists
        if db.get_health_issue_by_key(HealthIssueType::DuplicateInode, &issue_key)
            .ok()
            .flatten()
            .is_some()
        {
            continue;
        }

        let metadata = serde_json::json!({
            "inode": inode,
            "paths": paths,
            "note": "Multiple corpus files share the same inode - indicates hard links or database inconsistency"
        });

        let issue = HealthIssue {
            id: None,
            issue_type: HealthIssueType::DuplicateInode,
            issue_key,
            severity: HealthIssueSeverity::ManualReview,
            discovered_at: None,
            resolved_at: None,
            resolution_type: None,
            resolution_session: None,
            metadata_json: Some(metadata.to_string()),
        };

        if db.insert_health_issue(&issue).is_ok() {
            issues_created += 1;
            let _ = config::log_message(&format!(
                "Heartbeat: Duplicate inode {} found at {} paths",
                inode,
                paths.len()
            ));
        }
    }

    issues_created
}
