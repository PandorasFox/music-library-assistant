//! Deploy Module
//!
//! Part of MLA's toolkit: provides the "deploy" capability for library organization.
//!
//! NOTE: Core deployment logic implemented but UI integration pending.
//! See docs/FUTURE_FEATURES.md for planned integration.

#![allow(dead_code)]

use anyhow::Result;
use std::collections::HashSet;
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
    pub target_path: PathBuf, // In lost-files directory
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
    lost_files_root: Option<&Path>,
    dry_run: bool,
) -> Result<DeploymentResult> {
    let mut result = DeploymentResult {
        files_deployed: 0,
        files_skipped: 0,
        lost_files_moved: 0,
        errors: Vec::new(),
    };

    // Phase 1: Move lost files (if configured)
    if let Some(lost_root) = lost_files_root {
        for lost_file in &plan.lost_files {
            let target = lost_root.join(&lost_file.target_path);

            if !dry_run {
                if let Err(e) = relocate_lost_file(&lost_file.current_path, &target) {
                    result.errors.push(format!(
                        "Failed to move lost file {}: {}",
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

/// Relocate a lost file to the lost files directory
fn relocate_lost_file(current_path: &Path, target_path: &Path) -> Result<()> {
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
