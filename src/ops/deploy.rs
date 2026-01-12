//! Deploy Module
//!
//! Part of MLA's toolkit: provides the "deploy" capability for library organization.
//!
//! NOTE: Core deployment logic implemented but UI integration pending.
//! See docs/FUTURE_FEATURES.md for planned integration.

#![allow(dead_code)]

use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::db::{Database, Track};

#[derive(Debug, Clone)]
pub struct DeploymentPlan {
    pub library_name: String,
    pub files_to_deploy: Vec<DeploymentAction>,
    pub files_already_deployed: Vec<Track>,
    pub lost_files: Vec<LostFileAction>,
}

#[derive(Debug, Clone)]
pub struct DeploymentAction {
    pub corpus_track: Track,
    pub target_path: PathBuf, // Relative to library root
}

#[derive(Debug, Clone)]
pub struct LostFileAction {
    pub current_path: PathBuf,
    pub target_path: PathBuf, // In stash directory
    pub inode: i64,
}

#[derive(Debug, Clone)]
pub struct DeploymentResult {
    pub files_deployed: usize,
    pub files_skipped: usize,
    pub lost_files_moved: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DryRunReport {
    pub library_name: String,
    pub files_already_deployed: usize,
    pub files_to_deploy: usize,
    pub bytes_to_deploy: u64,
    pub lost_files: usize,
    pub lost_files_bytes: u64,
}

// ============================================================================
// Full Deployment Status (unified with health detection)
// ============================================================================

/// Comprehensive deployment status for a library, including stale detection
#[derive(Debug, Clone)]
pub struct FullDeploymentStatus {
    pub library_name: String,
    /// Files deployed at correct path
    pub healthy: Vec<DeployedFile>,
    /// New files to deploy
    pub to_deploy: Vec<DeploymentAction>,
    /// Files deployed but at outdated path (tags changed)
    pub stale: Vec<StaleDeploymentAction>,
    /// Library files without corpus backing
    pub orphans: Vec<OrphanAction>,
    /// Multiple corpus files would deploy to same path
    pub conflicts: Vec<DeploymentConflict>,
}

/// A file that is deployed at the correct path
#[derive(Debug, Clone)]
pub struct DeployedFile {
    pub corpus_path: String,
    pub library_path: PathBuf,
    pub inode: i64,
}

/// A stale deployment that needs path correction
#[derive(Debug, Clone)]
pub struct StaleDeploymentAction {
    pub corpus_track: Track,
    pub current_library_path: PathBuf,
    pub expected_library_path: PathBuf,
}

/// An orphaned library file to be stashed
#[derive(Debug, Clone)]
pub struct OrphanAction {
    pub library_path: PathBuf,
    pub stash_target: PathBuf,
    pub inode: i64,
    pub file_size: u64,
}

/// Multiple corpus files conflict on the same deployment path
#[derive(Debug, Clone)]
pub struct DeploymentConflict {
    pub target_path: PathBuf,
    pub conflicting_tracks: Vec<Track>,
}

/// Sanitize a path component by replacing invalid characters
fn sanitize_path_component(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect()
}

/// Compute deployment path based on metadata tags
/// Returns: {album_artist}/{album}/{track}. {title}.{ext}
/// Or: {album_artist}/{title}.{ext} for singles
/// Or: [no album artist]/... if missing album_artist
pub fn compute_deployment_path(track: &Track) -> PathBuf {
    // Get extension from original path
    let ext = Path::new(&track.path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("unknown");

    // Determine album artist (prefer album_artist, fallback to artist)
    let album_artist = track
        .album_artist
        .as_deref()
        .or(track.artist.as_deref())
        .unwrap_or("[no album artist]");

    if let Some(album) = &track.album {
        // Full path: {album_artist}/{album}/{track}. {title}.{ext}
        let mut path = PathBuf::new();
        path.push(sanitize_path_component(album_artist));
        path.push(sanitize_path_component(album));

        let filename = if let Some(title) = &track.title {
            if let Some(track_num) = track.track_number {
                format!(
                    "{:02}. {}.{}",
                    track_num,
                    sanitize_path_component(title),
                    ext
                )
            } else {
                format!("{}.{}", sanitize_path_component(title), ext)
            }
        } else {
            // Fallback to original filename
            Path::new(&track.path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string()
        };
        path.push(filename);
        path
    } else {
        // Single: {album_artist}/{title}.{ext}
        let mut path = PathBuf::new();
        path.push(sanitize_path_component(album_artist));

        let filename = if let Some(title) = &track.title {
            format!("{}.{}", sanitize_path_component(title), ext)
        } else {
            Path::new(&track.path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string()
        };
        path.push(filename);
        path
    }
}

/// Create deployment plans for all configured deploy mappings
pub fn create_deployment_plan(config: &Config, db: &Database) -> Result<Vec<DeploymentPlan>> {
    let mut plans = Vec::new();

    for mapping in &config.deploy_mappings {
        for library_name in &mapping.library_names {
            // Compute library path from libraries_root
            let library_path = config.libraries_root.join(library_name);

            // Collect corpus tracks from ALL corpus paths in this mapping
            let mut all_corpus_tracks = Vec::new();
            for corpus_relative_path in &mapping.corpus_relative_paths {
                let corpus_path = config.corpus_root.join(corpus_relative_path);
                let tracks = db.get_tracks_by_corpus_path_prefix(&corpus_path.to_string_lossy())?;
                all_corpus_tracks.extend(tracks);
            }

            // Get library tracks (to check what's already deployed)
            let library_tracks = db.get_library_tracks_by_source(library_name)?;
            let deployed_inodes: HashSet<i64> = library_tracks.iter().map(|t| t.inode).collect();

            // Categorize corpus tracks
            let mut files_to_deploy = Vec::new();
            let mut files_already_deployed = Vec::new();

            for track in all_corpus_tracks {
                if deployed_inodes.contains(&track.inode) {
                    files_already_deployed.push(track);
                } else {
                    let target_path = compute_deployment_path(&track);
                    files_to_deploy.push(DeploymentAction {
                        corpus_track: track,
                        target_path,
                    });
                }
            }

            // Find lost files (in library but not in corpus)
            let corpus_inodes: HashSet<i64> = files_to_deploy
                .iter()
                .map(|a| a.corpus_track.inode)
                .chain(files_already_deployed.iter().map(|t| t.inode))
                .collect();

            let lost_files = library_tracks
                .iter()
                .filter(|t| !corpus_inodes.contains(&t.inode))
                .map(|t| {
                    // Compute lost file path relative to library
                    let relative = Path::new(&t.path)
                        .strip_prefix(&library_path)
                        .unwrap_or(Path::new(&t.path));

                    let target_path = PathBuf::from(library_name).join(relative);

                    LostFileAction {
                        current_path: PathBuf::from(&t.path),
                        target_path,
                        inode: t.inode,
                    }
                })
                .collect();

            plans.push(DeploymentPlan {
                library_name: library_name.clone(),
                files_to_deploy,
                files_already_deployed,
                lost_files,
            });
        }
    }

    Ok(plans)
}

/// Execute a deployment plan
pub fn execute_deployment(
    plan: &DeploymentPlan,
    library_root: &Path,
    stash_root: Option<&Path>,
    dry_run: bool,
) -> Result<DeploymentResult> {
    let mut result = DeploymentResult {
        files_deployed: 0,
        files_skipped: 0,
        lost_files_moved: 0,
        errors: Vec::new(),
    };

    // Phase 1: Move orphaned library files to stash (if configured)
    if let Some(stash_path) = stash_root {
        for lost_file in &plan.lost_files {
            let target = stash_path.join(&lost_file.target_path);

            if !dry_run {
                if let Err(e) = relocate_to_stash(&lost_file.current_path, &target) {
                    result.errors.push(format!(
                        "Failed to stash orphaned file {}: {}",
                        lost_file.current_path.display(),
                        e
                    ));
                    continue;
                }
            }
            result.lost_files_moved += 1;
        }
    }

    // Phase 2: Deploy new files
    for action in &plan.files_to_deploy {
        let target = library_root.join(&action.target_path);

        // Check if target already exists
        if target.exists() {
            // Check if same inode (already hard-linked)
            if let Ok(metadata) = std::fs::metadata(&target) {
                if metadata.ino() == action.corpus_track.inode as u64 {
                    result.files_skipped += 1;
                    continue;
                }
            }

            // Different file at same path - error
            result
                .errors
                .push(format!("Target path already exists: {}", target.display()));
            continue;
        }

        if !dry_run {
            // Create parent directories
            if let Some(parent) = target.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    result.errors.push(format!(
                        "Failed to create directory {}: {}",
                        parent.display(),
                        e
                    ));
                    continue;
                }
            }

            // Create hard link
            let source = Path::new(&action.corpus_track.path);
            if let Err(e) = std::fs::hard_link(source, &target) {
                result.errors.push(format!(
                    "Failed to hard link {} -> {}: {}",
                    source.display(),
                    target.display(),
                    e
                ));
                continue;
            }
        }

        result.files_deployed += 1;
    }

    Ok(result)
}

/// Relocate an orphaned file to the stash directory
fn relocate_to_stash(current_path: &Path, target_path: &Path) -> Result<()> {
    // Create parent directories
    if let Some(parent) = target_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Move file (rename if same filesystem, copy+delete otherwise)
    std::fs::rename(current_path, target_path).or_else(|_| {
        std::fs::copy(current_path, target_path)?;
        std::fs::remove_file(current_path)?;
        Ok(())
    })
}

/// Convert a deployment plan into pending change mutations
///
/// This allows deployment operations to be tracked, previewed, and committed
/// like any other corpus mutation. Each Deploy/Undeploy operation becomes
/// a PendingChange that can be executed through the standard change system.
///
/// A single corpus file can be deployed to multiple libraries by creating
/// multiple Deploy mutations with different target paths.
pub fn plan_deployment_as_mutations(
    plan: &DeploymentPlan,
    library_root: &Path,
    stash_root: Option<&Path>,
    session_id: &str,
) -> Vec<crate::db::PendingChange> {
    use crate::db::{ChangeStatus, ChangeType, PendingChange};

    let mut changes = Vec::new();

    // Files to deploy -> Deploy mutations
    for action in &plan.files_to_deploy {
        let target_path = library_root.join(&action.target_path);
        changes.push(PendingChange {
            id: None,
            session_id: session_id.to_string(),
            change_type: ChangeType::Deploy,
            source_path: action.corpus_track.path.clone(),
            target_path: Some(target_path.to_string_lossy().to_string()),
            metadata_changes: Some(serde_json::json!({
                "library": plan.library_name,
                "relative_path": action.target_path.to_string_lossy(),
            }).to_string()),
            created_at: None,
            status: ChangeStatus::Pending,
        });
    }

    // Lost files -> Undeploy mutations (move to stash)
    if let Some(stash_path) = stash_root {
        for lost in &plan.lost_files {
            let target_path = stash_path.join(&lost.target_path);
            changes.push(PendingChange {
                id: None,
                session_id: session_id.to_string(),
                change_type: ChangeType::Undeploy,
                source_path: lost.current_path.to_string_lossy().to_string(),
                target_path: Some(target_path.to_string_lossy().to_string()),
                metadata_changes: Some(serde_json::json!({
                    "library": plan.library_name,
                    "reason": "orphan_cleanup",
                    "original_inode": lost.inode,
                }).to_string()),
                created_at: None,
                status: ChangeStatus::Pending,
            });
        }
    }

    changes
}

/// Convert all deployment plans to mutations
pub fn plan_all_deployments_as_mutations(
    config: &Config,
    db: &Database,
    session_id: &str,
) -> Result<Vec<crate::db::PendingChange>> {
    let plans = create_deployment_plan(config, db)?;
    let mut all_changes = Vec::new();

    for plan in plans {
        let library_root = config.libraries_root.join(&plan.library_name);
        let stash_root = config.stash_dir.as_ref().map(|p| p.as_path());

        let changes = plan_deployment_as_mutations(&plan, &library_root, stash_root, session_id);
        all_changes.extend(changes);
    }

    Ok(all_changes)
}

/// Generate dry-run reports for all deployment plans
pub fn generate_dry_run_report(config: &Config, db: &Database) -> Result<Vec<DryRunReport>> {
    let plans = create_deployment_plan(config, db)?;

    let mut reports = Vec::new();
    for plan in plans {
        let bytes_to_deploy: u64 = plan
            .files_to_deploy
            .iter()
            .map(|a| a.corpus_track.file_size as u64)
            .sum();

        let lost_files_bytes: u64 = plan
            .lost_files
            .iter()
            .filter_map(|lf| std::fs::metadata(&lf.current_path).ok().map(|m| m.len()))
            .sum();

        reports.push(DryRunReport {
            library_name: plan.library_name,
            files_already_deployed: plan.files_already_deployed.len(),
            files_to_deploy: plan.files_to_deploy.len(),
            bytes_to_deploy,
            lost_files: plan.lost_files.len(),
            lost_files_bytes,
        });
    }

    Ok(reports)
}

// ============================================================================
// Full Deployment Status Computation
// ============================================================================

/// Audio file extensions
const AUDIO_EXTENSIONS: &[&str] = &["flac", "mp3", "ogg", "m4a", "opus", "wav", "aiff", "aif"];

/// Check if path has audio file extension
fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| AUDIO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Walk a library directory and collect all files with their inodes
fn walk_library_files(root: &Path) -> Vec<(PathBuf, i64)> {
    let mut files = Vec::new();

    if !root.exists() {
        return files;
    }

    walk_library_recursive(root, &mut files);
    files
}

fn walk_library_recursive(dir: &Path, files: &mut Vec<(PathBuf, i64)>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            walk_library_recursive(&path, files);
        } else if is_audio_file(&path) {
            if let Ok(metadata) = std::fs::metadata(&path) {
                files.push((path, metadata.ino() as i64));
            }
        }
    }
}

/// Get corpus tracks that should be deployed to a specific library
fn get_deployable_corpus_tracks(
    config: &Config,
    db: &Database,
    library_name: &str,
) -> Vec<Track> {
    let mut all_tracks = Vec::new();

    for mapping in &config.deploy_mappings {
        if mapping.library_names.contains(&library_name.to_string()) {
            for corpus_relative_path in &mapping.corpus_relative_paths {
                let corpus_path = config.corpus_root.join(corpus_relative_path);
                if let Ok(tracks) = db.get_tracks_by_corpus_path_prefix(&corpus_path.to_string_lossy())
                {
                    all_tracks.extend(tracks);
                }
            }
        }
    }

    all_tracks
}

/// Get list of all configured library names
fn get_configured_library_names(config: &Config) -> Vec<String> {
    let mut names = HashSet::new();
    for mapping in &config.deploy_mappings {
        for name in &mapping.library_names {
            names.insert(name.clone());
        }
    }
    names.into_iter().collect()
}

/// Compute comprehensive deployment status with stale and conflict detection
///
/// This combines deployment planning with health detection to provide:
/// - healthy: Files deployed at correct path
/// - to_deploy: New files to deploy
/// - stale: Files deployed but at wrong path (tags changed)
/// - orphans: Library files without corpus backing
/// - conflicts: Multiple corpus files that would deploy to same path
pub fn compute_full_deployment_status(
    config: &Config,
    db: &Database,
) -> Result<Vec<FullDeploymentStatus>> {
    let library_names = get_configured_library_names(config);
    let mut statuses = Vec::new();

    for library_name in library_names {
        let library_root = config.libraries_root.join(&library_name);
        let stash_root = config.stash_dir.as_ref();

        // Get all corpus tracks that should be deployed to this library
        let corpus_tracks = get_deployable_corpus_tracks(config, db, &library_name);

        // Get all files currently in the library
        let library_files = walk_library_files(&library_root);

        // Build inode -> library path map
        let library_inode_map: HashMap<i64, PathBuf> = library_files
            .iter()
            .map(|(path, inode)| (*inode, path.clone()))
            .collect();

        let library_inodes: HashSet<i64> = library_files.iter().map(|(_, inode)| *inode).collect();
        let corpus_inodes: HashSet<i64> = corpus_tracks.iter().map(|t| t.inode).collect();

        // Build target_path -> tracks map for conflict detection
        let mut target_path_map: HashMap<PathBuf, Vec<Track>> = HashMap::new();
        for track in &corpus_tracks {
            let target = compute_deployment_path(track);
            target_path_map
                .entry(target)
                .or_default()
                .push(track.clone());
        }

        // Identify conflicts (multiple tracks -> same path)
        let mut conflicts = Vec::new();
        let mut conflicting_inodes = HashSet::new();
        for (target_path, tracks) in &target_path_map {
            if tracks.len() > 1 {
                conflicts.push(DeploymentConflict {
                    target_path: target_path.clone(),
                    conflicting_tracks: tracks.clone(),
                });
                for track in tracks {
                    conflicting_inodes.insert(track.inode);
                }
            }
        }

        // Categorize corpus tracks
        let mut healthy = Vec::new();
        let mut to_deploy = Vec::new();
        let mut stale = Vec::new();

        for track in &corpus_tracks {
            // Skip tracks involved in conflicts
            if conflicting_inodes.contains(&track.inode) {
                continue;
            }

            let expected_path = library_root.join(compute_deployment_path(track));

            if let Some(library_path) = library_inode_map.get(&track.inode) {
                // File is deployed - check if at correct path
                if library_path == &expected_path {
                    healthy.push(DeployedFile {
                        corpus_path: track.path.clone(),
                        library_path: library_path.clone(),
                        inode: track.inode,
                    });
                } else {
                    // Stale deployment - file at wrong path due to tag changes
                    stale.push(StaleDeploymentAction {
                        corpus_track: track.clone(),
                        current_library_path: library_path.clone(),
                        expected_library_path: expected_path,
                    });
                }
            } else {
                // Not deployed
                to_deploy.push(DeploymentAction {
                    corpus_track: track.clone(),
                    target_path: compute_deployment_path(track),
                });
            }
        }

        // Find orphans (library files without corpus backing)
        let orphan_inodes: HashSet<i64> = library_inodes.difference(&corpus_inodes).copied().collect();
        let orphans: Vec<OrphanAction> = library_files
            .iter()
            .filter(|(_, inode)| orphan_inodes.contains(inode))
            .map(|(path, inode)| {
                let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

                // Compute stash target path: {stash}/orphans/{library_name}/{relative_path}
                let relative = path
                    .strip_prefix(&library_root)
                    .unwrap_or(path.as_path());
                let stash_target = match stash_root {
                    Some(stash) => stash.join("orphans").join(&library_name).join(relative),
                    None => PathBuf::from("stash").join("orphans").join(&library_name).join(relative),
                };

                OrphanAction {
                    library_path: path.clone(),
                    stash_target,
                    inode: *inode,
                    file_size,
                }
            })
            .collect();

        statuses.push(FullDeploymentStatus {
            library_name,
            healthy,
            to_deploy,
            stale,
            orphans,
            conflicts,
        });
    }

    Ok(statuses)
}

/// Convert full deployment status to pending change mutations
///
/// Generates:
/// - Deploy mutations for new files
/// - Redeploy mutations for stale path corrections
/// - Undeploy mutations for orphaned library files
///
/// Files with conflicts are skipped (they become health issues instead).
pub fn deployment_status_to_mutations(
    status: &FullDeploymentStatus,
    library_root: &Path,
    stash_root: Option<&Path>,
    session_id: &str,
) -> Vec<crate::db::PendingChange> {
    use crate::db::{ChangeStatus, ChangeType, PendingChange};

    let mut changes = Vec::new();

    // New files -> Deploy mutations
    for action in &status.to_deploy {
        let target_path = library_root.join(&action.target_path);
        changes.push(PendingChange {
            id: None,
            session_id: session_id.to_string(),
            change_type: ChangeType::Deploy,
            source_path: action.corpus_track.path.clone(),
            target_path: Some(target_path.to_string_lossy().to_string()),
            metadata_changes: Some(
                serde_json::json!({
                    "library": status.library_name,
                    "relative_path": action.target_path.to_string_lossy(),
                })
                .to_string(),
            ),
            created_at: None,
            status: ChangeStatus::Pending,
        });
    }

    // Stale files -> Redeploy mutations (relocate to correct path)
    for stale in &status.stale {
        changes.push(PendingChange {
            id: None,
            session_id: session_id.to_string(),
            change_type: ChangeType::Redeploy,
            source_path: stale.current_library_path.to_string_lossy().to_string(),
            target_path: Some(stale.expected_library_path.to_string_lossy().to_string()),
            metadata_changes: Some(
                serde_json::json!({
                    "library": status.library_name,
                    "corpus_path": stale.corpus_track.path,
                    "reason": "stale_path_correction",
                })
                .to_string(),
            ),
            created_at: None,
            status: ChangeStatus::Pending,
        });
    }

    // Orphans -> Undeploy mutations (move to stash)
    // Orphans are always stashed - no exclusion option
    if stash_root.is_some() {
        for orphan in &status.orphans {
            changes.push(PendingChange {
                id: None,
                session_id: session_id.to_string(),
                change_type: ChangeType::Undeploy,
                source_path: orphan.library_path.to_string_lossy().to_string(),
                target_path: Some(orphan.stash_target.to_string_lossy().to_string()),
                metadata_changes: Some(
                    serde_json::json!({
                        "library": status.library_name,
                        "reason": "orphan_cleanup",
                        "original_inode": orphan.inode,
                    })
                    .to_string(),
                ),
                created_at: None,
                status: ChangeStatus::Pending,
            });
        }
    }

    // Conflicts are NOT converted to mutations - they become health issues instead

    changes
}

/// Convert all deployment statuses to mutations
pub fn all_deployment_statuses_to_mutations(
    statuses: &[FullDeploymentStatus],
    config: &Config,
    session_id: &str,
) -> Vec<crate::db::PendingChange> {
    let mut all_changes = Vec::new();

    for status in statuses {
        let library_root = config.libraries_root.join(&status.library_name);
        let stash_root = config.stash_dir.as_ref().map(|p| p.as_path());

        let changes = deployment_status_to_mutations(status, &library_root, stash_root, session_id);
        all_changes.extend(changes);
    }

    all_changes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_path_component() {
        assert_eq!(sanitize_path_component("Hello/World"), "Hello_World");
        assert_eq!(sanitize_path_component("Track:01"), "Track_01");
        assert_eq!(sanitize_path_component("Normal Name"), "Normal Name");
    }

    #[test]
    fn test_compute_deployment_path_full() {
        let track = Track {
            id: None,
            path: "/corpus/test.flac".to_string(),
            source: "corpus".to_string(),
            inode: 123,
            file_size: 1000,
            file_type: "flac".to_string(),
            artist: Some("Artist Name".to_string()),
            album: Some("Album Name".to_string()),
            album_artist: Some("Album Artist".to_string()),
            title: Some("Track Title".to_string()),
            track_number: Some(1),
            duration_ms: Some(180000),
            bitrate_kbps: None,
            sample_rate: None,
            fingerprint: None,
            isrc: None,
        };

        let path = compute_deployment_path(&track);
        assert_eq!(
            path,
            PathBuf::from("Album Artist/Album Name/01. Track Title.flac")
        );
    }

    #[test]
    fn test_compute_deployment_path_single() {
        let track = Track {
            id: None,
            path: "/corpus/test.mp3".to_string(),
            source: "corpus".to_string(),
            inode: 123,
            file_size: 1000,
            file_type: "mp3".to_string(),
            artist: Some("Artist Name".to_string()),
            album: None,
            album_artist: None,
            title: Some("Single Track".to_string()),
            track_number: None,
            duration_ms: Some(180000),
            bitrate_kbps: None,
            sample_rate: None,
            fingerprint: None,
            isrc: None,
        };

        let path = compute_deployment_path(&track);
        assert_eq!(path, PathBuf::from("Artist Name/Single Track.mp3"));
    }

    #[test]
    fn test_compute_deployment_path_no_album_artist() {
        let track = Track {
            id: None,
            path: "/corpus/test.flac".to_string(),
            source: "corpus".to_string(),
            inode: 123,
            file_size: 1000,
            file_type: "flac".to_string(),
            artist: None,
            album: Some("Album".to_string()),
            album_artist: None,
            title: Some("Title".to_string()),
            track_number: Some(5),
            duration_ms: Some(180000),
            bitrate_kbps: None,
            sample_rate: None,
            fingerprint: None,
            isrc: None,
        };

        let path = compute_deployment_path(&track);
        assert_eq!(
            path,
            PathBuf::from("[no album artist]/Album/05. Title.flac")
        );
    }
}
