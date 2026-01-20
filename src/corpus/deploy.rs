//! Deployment Module (DISABLED)
//!
//! TODO: This module has been relocated from flows/deploy.rs and needs to be
//! updated to use the new transaction API and library health signals before
//! re-enabling. The old PendingDecision/DecisionType patterns have been removed.
//!
//! When re-enabling:
//! - Replace *_to_decisions() functions with *_to_mutations() returning Vec<Mutation>
//! - Integrate with DeriveLibraryHealthSignals computations
//! - Wire into the DecisionWitness transaction flow
//!
//! ## Original Module Documentation
//!
//! Deploy Module
//!
//! Part of MLA's toolkit: provides the "deploy" capability for library organization.
//!
//! NOTE: Core deployment logic implemented but UI integration pending.
//! See docs/FUTURE_FEATURES.md for planned integration.

/*
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::corpus::db::{Database, Track};
use crate::corpus::health::library::{
    get_configured_library_names, get_deployable_corpus_tracks, walk_library_files,
};

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
/// Compute deployment path from track and tags.
///
/// Tags should be provided as a HashMap with lowercase keys.
/// Required tags: album_artist (or artist), album (optional), title (optional), track_number (optional)
pub fn compute_deployment_path_with_tags(track: &Track, tags: &HashMap<String, String>) -> PathBuf {
    // Get extension from original path
    let ext = Path::new(&track.path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("unknown");

    // Determine album artist (prefer album_artist, fallback to artist)
    let album_artist = tags
        .get("album_artist")
        .or_else(|| tags.get("artist"))
        .map(|s| s.as_str())
        .unwrap_or("[no album artist]");

    if let Some(album) = tags.get("album") {
        // Full path: {album_artist}/{album}/{track}. {title}.{ext}
        let mut path = PathBuf::new();
        path.push(sanitize_path_component(album_artist));
        path.push(sanitize_path_component(album));

        let filename = if let Some(title) = tags.get("title") {
            if let Some(track_num_str) = tags.get("track_number") {
                if let Ok(track_num) = track_num_str.parse::<i32>() {
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

        let filename = if let Some(title) = tags.get("title") {
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

/// Compute deployment path (convenience wrapper that uses empty tags).
///
/// Note: This will produce fallback paths. For proper deployment paths,
/// use compute_deployment_path_with_tags with tags loaded from track_tags table.
pub fn compute_deployment_path(track: &Track) -> PathBuf {
    compute_deployment_path_with_tags(track, &HashMap::new())
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

// VESTIGIAL: Functions below used PendingDecision/DecisionType which have been removed.
// These need to be rewritten to return Vec<Mutation> directly.

// pub fn plan_deployment_as_decisions(...) -> Vec<PendingDecision>
// pub fn plan_all_deployments_as_decisions(...) -> Result<Vec<PendingDecision>>
// pub fn deployment_status_to_decisions(...) -> Vec<PendingDecision>
// pub fn all_deployment_statuses_to_decisions(...) -> Vec<PendingDecision>

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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_track(path: &str) -> Track {
        Track {
            id: None,
            path: path.to_string(),
            source: "corpus".to_string(),
            inode: 123,
            file_size: 1000,
            file_type: "flac".to_string(),
            duration_ms: Some(180000),
            bitrate_kbps: None,
            sample_rate: None,
            fingerprint: None,
        }
    }

    #[test]
    fn test_sanitize_path_component() {
        assert_eq!(sanitize_path_component("Hello/World"), "Hello_World");
        assert_eq!(sanitize_path_component("Track:01"), "Track_01");
        assert_eq!(sanitize_path_component("Normal Name"), "Normal Name");
    }

    #[test]
    fn test_compute_deployment_path_full() {
        let track = make_track("/corpus/test.flac");
        let tags: HashMap<String, String> = [
            ("artist".to_string(), "Artist Name".to_string()),
            ("album".to_string(), "Album Name".to_string()),
            ("album_artist".to_string(), "Album Artist".to_string()),
            ("title".to_string(), "Track Title".to_string()),
            ("track_number".to_string(), "1".to_string()),
        ].into_iter().collect();

        let path = compute_deployment_path_with_tags(&track, &tags);
        assert_eq!(
            path,
            PathBuf::from("Album Artist/Album Name/01. Track Title.flac")
        );
    }

    #[test]
    fn test_compute_deployment_path_single() {
        let mut track = make_track("/corpus/test.mp3");
        track.file_type = "mp3".to_string();
        let tags: HashMap<String, String> = [
            ("artist".to_string(), "Artist Name".to_string()),
            ("title".to_string(), "Single Track".to_string()),
        ].into_iter().collect();

        let path = compute_deployment_path_with_tags(&track, &tags);
        assert_eq!(path, PathBuf::from("Artist Name/Single Track.mp3"));
    }

    #[test]
    fn test_compute_deployment_path_no_album_artist() {
        let track = make_track("/corpus/test.flac");
        let tags: HashMap<String, String> = [
            ("album".to_string(), "Album".to_string()),
            ("title".to_string(), "Title".to_string()),
            ("track_number".to_string(), "5".to_string()),
        ].into_iter().collect();

        let path = compute_deployment_path_with_tags(&track, &tags);
        assert_eq!(
            path,
            PathBuf::from("[no album artist]/Album/05. Title.flac")
        );
    }
}
*/
