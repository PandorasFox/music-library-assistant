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
use crate::flows::deploy::compute_deployment_path;

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
pub fn check_library_health(
    config: &Config,
    db: &Database,
    library_name: &str,
) -> LibraryHealthResult {
    use std::time::Instant;
    let start = Instant::now();

    let library_root = config.libraries_root.join(library_name);

    // Get all corpus tracks that should be deployed to this library
    let corpus_tracks = get_deployable_corpus_tracks(config, db, library_name);

    // Get all files currently in the library
    let library_files = walk_library_files(&library_root);

    // Build inode -> library path map
    let library_inode_map: HashMap<i64, PathBuf> = library_files
        .iter()
        .map(|(path, inode)| (*inode, path.clone()))
        .collect();

    let library_inodes: HashSet<i64> = library_files.iter().map(|(_, inode)| *inode).collect();
    let corpus_inodes: HashSet<i64> = corpus_tracks.iter().map(|t| t.inode).collect();

    let mut healthy = 0;
    let mut not_deployed = 0;
    let mut stale = 0;
    let mut stale_files = Vec::new();

    // Check each corpus track
    for track in &corpus_tracks {
        if let Some(library_path) = library_inode_map.get(&track.inode) {
            // File is deployed - check if at correct path
            let expected_path = library_root.join(compute_deployment_path(track));

            if library_path == &expected_path {
                healthy += 1;
            } else {
                // Stale deployment - file moved due to tag changes
                stale += 1;
                stale_files.push(StaleDeployment {
                    corpus_path: track.path.clone(),
                    corpus_inode: track.inode,
                    current_library_path: library_path.clone(),
                    expected_library_path: expected_path,
                });
            }
        } else {
            // Not deployed
            not_deployed += 1;
        }
    }

    // Find orphans (library files without corpus backing)
    let orphan_inodes: HashSet<i64> = library_inodes.difference(&corpus_inodes).copied().collect();
    let orphan_files: Vec<OrphanFile> = library_files
        .iter()
        .filter(|(_, inode)| orphan_inodes.contains(inode))
        .map(|(path, inode)| {
            let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            OrphanFile {
                library_path: path.clone(),
                inode: *inode,
                file_size,
            }
        })
        .collect();

    LibraryHealthResult {
        library_name: library_name.to_string(),
        healthy,
        not_deployed,
        stale,
        orphans: orphan_files.len(),
        stale_files,
        orphan_files,
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
