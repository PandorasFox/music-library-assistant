//! Cross-Source Overlap Resolution Types
//!
//! Data structures for the cross-source overlap resolution modal, including
//! cluster entries, resolution options, and stash file preview.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::corpus::paths;
use crate::meta::mutations::file_ops::StashFromZoneMutation;
use crate::meta::mutations::indexing::DropFromIndexMutation;
use crate::meta::mutations::Mutation;
use crate::ui::manual_review_modal::types::FileMetaSummary;

/// A source directory within an overlap cluster.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DirectoryGroupEntry {
    /// Source directory path (e.g., "web/releases/bandcamp" or "web/releases/indie")
    pub path_suffix: String,
    /// Inodes for files in this source
    pub inodes: Vec<i64>,
    /// Corpus paths for the tracks
    pub paths: Vec<String>,
    /// Format summary (e.g., "FLAC (3)" or "MP3 (2)")
    pub format_summary: String,
    /// Whether this source can have duplicates stashed (from config, default: true).
    pub can_stash_dupes: bool,
}

/// A single cross-source overlap cluster ready for resolution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DirectoryClusterEntry {
    /// Cluster key (sorted source paths joined by |)
    pub cluster_key: String,
    /// Source directories in this cluster (usually 2)
    pub directories: Vec<DirectoryGroupEntry>,
    /// Number of overlapping track pairs
    pub overlap_count: usize,
}

/// Resolution option for a directory cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClusterResolutionOption {
    /// Stash a specific directory (keep the other(s))
    StashDirectory { stash_suffix: String },
    /// Auto-select based on quality (stash the lower-quality format)
    AutoQuality { stash_format: String },
    /// Edit tags for a specific directory (launch tag editor)
    EditTags {
        dir_suffix: String,
        inodes: Vec<i64>,
    },
    /// Mark this source pair overlap as expected (suppress future signals)
    MarkExpected,
}

/// A file that would be stashed by a resolution option.
#[derive(Debug, Clone)]
pub struct StashFileEntry {
    pub corpus_path: String,
    pub inode: i64,
}

/// Cached data for the cross-source overlap resolution modal.
///
/// Loaded once when the modal opens. All renders use this cached data.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DirectoryClusterModalData {
    /// Cross-source overlap clusters
    pub clusters: Vec<DirectoryClusterEntry>,
    /// Audio metadata cache keyed by inode (loaded at init time).
    pub file_meta_cache: HashMap<i64, FileMetaSummary>,
}

impl DirectoryClusterModalData {
    /// Total number of clusters.
    pub fn total_count(&self) -> usize {
        self.clusters.len()
    }

    /// Generate mutations for a resolution option on a specific cluster.
    ///
    /// Stashes the directory identified by the option (respecting `can_stash_dupes`).
    pub fn mutations_for_resolution(
        &self,
        cluster_index: usize,
        option: &ClusterResolutionOption,
    ) -> Vec<Mutation> {
        let resolver = paths::get_resolver();
        let mut mutations = Vec::new();

        let cluster = match self.clusters.get(cluster_index) {
            Some(c) => c,
            None => return mutations,
        };

        // MarkExpected and EditTags don't stash files
        if matches!(
            option,
            ClusterResolutionOption::MarkExpected | ClusterResolutionOption::EditTags { .. }
        ) {
            return mutations;
        }

        for dir in &cluster.directories {
            let should_stash = match option {
                ClusterResolutionOption::StashDirectory { stash_suffix } => {
                    dir.path_suffix == *stash_suffix
                }
                ClusterResolutionOption::AutoQuality { stash_format } => {
                    dir.format_summary.starts_with(stash_format.as_str())
                }
                ClusterResolutionOption::EditTags { .. } => false,
                ClusterResolutionOption::MarkExpected => false,
            };

            if !should_stash || !dir.can_stash_dupes {
                continue;
            }

            for (idx, corpus_path) in dir.paths.iter().enumerate() {
                let abs_path = resolver.resolve(std::path::Path::new(corpus_path));

                mutations.push(Mutation::StashFromZone(StashFromZoneMutation {
                    path: abs_path,
                    stash_name: "overlaps".to_string(),
                }));

                let inode = dir.inodes.get(idx).copied();

                mutations.push(Mutation::DropFromIndex(DropFromIndexMutation {
                    path: PathBuf::from(corpus_path),
                    inode,
                    zone: Some("corpus".to_string()),
                }));
            }
        }

        mutations
    }

    /// Collect files that would be stashed by a resolution option on a specific cluster.
    ///
    /// Mirrors the stash logic from `mutations_for_resolution()` but returns
    /// `StashFileEntry` values instead of mutations.
    pub fn stash_files_for_option(
        &self,
        cluster_index: usize,
        option: &ClusterResolutionOption,
    ) -> Vec<StashFileEntry> {
        let cluster = match self.clusters.get(cluster_index) {
            Some(c) => c,
            None => return Vec::new(),
        };

        // MarkExpected and EditTags have no files to stash
        if matches!(
            option,
            ClusterResolutionOption::MarkExpected | ClusterResolutionOption::EditTags { .. }
        ) {
            return Vec::new();
        }

        let mut entries = Vec::new();
        for dir in &cluster.directories {
            let should_stash = match option {
                ClusterResolutionOption::StashDirectory { stash_suffix } => {
                    dir.path_suffix == *stash_suffix
                }
                ClusterResolutionOption::AutoQuality { stash_format } => {
                    dir.format_summary.starts_with(stash_format.as_str())
                }
                ClusterResolutionOption::EditTags { .. } => false,
                ClusterResolutionOption::MarkExpected => false,
            };

            if !should_stash || !dir.can_stash_dupes {
                continue;
            }
            for (idx, corpus_path) in dir.paths.iter().enumerate() {
                let inode = dir.inodes.get(idx).copied().unwrap_or(0);
                entries.push(StashFileEntry {
                    corpus_path: corpus_path.clone(),
                    inode,
                });
            }
        }
        entries
    }
}
