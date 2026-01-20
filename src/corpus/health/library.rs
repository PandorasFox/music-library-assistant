//! Library Health Tracking
//!
//! Tracks deployment health states for each configured library.
//! Detects healthy deployments, not-deployed files, stale deployments
//! (where tags changed since deploy), and orphaned library files.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::Config;
use crate::corpus::db::Database;
// TODO: Re-enable when corpus::deploy is available
// use crate::corpus::deploy::compute_deployment_path;

/// Deployment status for a single file
#[derive(Debug, Clone)]
pub enum DeploymentStatus {
    /// Hard link exists at expected path with correct inode
    Healthy,
    /// Not deployed (corpus file exists, no library link)
    NotDeployed,
    /// Deployed but tags changed - deployment path outdated
    StaleDeployment {
        current_path: PathBuf,
        expected_path: PathBuf,
    },
    /// Library file exists but no corpus backing (orphan)
    Orphan,
}

/// Information about a stale deployment
#[derive(Debug, Clone)]
pub struct StaleDeployment {
    pub corpus_path: String,
    pub corpus_inode: i64,
    pub current_library_path: PathBuf,
    pub expected_library_path: PathBuf,
}

/// Information about an orphaned library file
#[derive(Debug, Clone)]
pub struct OrphanFile {
    pub library_path: PathBuf,
    pub inode: i64,
    pub file_size: u64,
}

/// Health result for a single library
#[derive(Debug, Clone)]
pub struct LibraryHealthResult {
    pub library_name: String,
    pub healthy: usize,
    pub not_deployed: usize,
    pub stale: usize,
    pub orphans: usize,
    pub stale_files: Vec<StaleDeployment>,
    pub orphan_files: Vec<OrphanFile>,
    pub duration: Duration,
}

impl LibraryHealthResult {
    /// Returns true if the library is fully healthy
    pub fn is_healthy(&self) -> bool {
        self.not_deployed == 0 && self.stale == 0 && self.orphans == 0
    }

    /// Returns a short summary string
    pub fn summary(&self) -> String {
        if self.is_healthy() {
            format!("{}: {} deployed", self.library_name, self.healthy)
        } else {
            format!(
                "{}: {} ok, {} pending, {} stale, {} orphan",
                self.library_name, self.healthy, self.not_deployed, self.stale, self.orphans
            )
        }
    }
}

/// Check health of a single library
///
/// TODO: Requires compute_deployment_path from corpus::deploy which is disabled.
/// Currently returns a stub result indicating the library is not checked.
pub fn check_library_health(
    _config: &Config,
    _db: &Database,
    library_name: &str,
) -> LibraryHealthResult {
    use std::time::Instant;
    let start = Instant::now();

    // TODO: Re-enable when corpus::deploy is available
    // This function requires compute_deployment_path to determine expected paths.
    // Without it, we cannot accurately compute stale deployments.

    LibraryHealthResult {
        library_name: library_name.to_string(),
        healthy: 0,
        not_deployed: 0,
        stale: 0,
        orphans: 0,
        stale_files: vec![],
        orphan_files: vec![],
        duration: start.elapsed(),
    }
}

/// Check health of all configured libraries
pub fn check_all_libraries_health(config: &Config, db: &Database) -> Vec<LibraryHealthResult> {
    let library_names = get_configured_library_names(config);
    library_names
        .iter()
        .map(|name| check_library_health(config, db, name))
        .collect()
}

/// Get corpus tracks that should be deployed to a specific library
pub fn get_deployable_corpus_tracks(
    config: &Config,
    db: &Database,
    library_name: &str,
) -> Vec<crate::corpus::db::Track> {
    let mut all_tracks = Vec::new();

    for corpus_path in config.get_corpus_paths_for_library(library_name) {
        if let Ok(tracks) = db.get_tracks_by_corpus_path_prefix(&corpus_path.to_string_lossy()) {
            all_tracks.extend(tracks);
        }
    }

    all_tracks
}

/// Walk a library directory and collect all files with their inodes
pub fn walk_library_files(root: &Path) -> Vec<(PathBuf, i64)> {
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

/// Get list of all configured library names
pub fn get_configured_library_names(config: &Config) -> Vec<String> {
    let mut names = HashSet::new();
    for mapping in &config.deploy_mappings {
        for name in &mapping.library_names {
            names.insert(name.clone());
        }
    }
    names.into_iter().collect()
}

use crate::config::AUDIO_EXTENSIONS;

/// Check if path has audio file extension
pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| AUDIO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

// TODO: generate_orphan_cleanup_decisions removed - needs reimplementation with Mutation system

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_library_health_result_summary() {
        let result = LibraryHealthResult {
            library_name: "main".to_string(),
            healthy: 100,
            not_deployed: 0,
            stale: 0,
            orphans: 0,
            stale_files: vec![],
            orphan_files: vec![],
            duration: Duration::from_millis(50),
        };

        assert!(result.is_healthy());
        assert_eq!(result.summary(), "main: 100 deployed");
    }

    #[test]
    fn test_library_health_result_unhealthy() {
        let result = LibraryHealthResult {
            library_name: "portable".to_string(),
            healthy: 50,
            not_deployed: 10,
            stale: 5,
            orphans: 2,
            stale_files: vec![],
            orphan_files: vec![],
            duration: Duration::from_millis(50),
        };

        assert!(!result.is_healthy());
        assert_eq!(
            result.summary(),
            "portable: 50 ok, 10 pending, 5 stale, 2 orphan"
        );
    }
}
