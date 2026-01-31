//! Directory Overlap Cluster Resolution Types
//!
//! Data structures for the directory cluster resolution modal, including
//! cluster entries, resolution options, and button state.

use std::path::PathBuf;

use anyhow::Result;

use crate::corpus::db::types::AggregateSignalType;
use crate::corpus::db::ReadOnlyDb;
use crate::corpus::mutations::Mutation;
use crate::corpus::paths;

/// A directory within an overlap cluster.
#[derive(Debug, Clone)]
pub struct DirectoryGroupEntry {
    /// Path suffix identifying this directory (e.g., "bandcamp" or "indie/msx")
    pub path_suffix: String,
    /// Track IDs in this directory
    pub track_ids: Vec<i64>,
    /// Inodes for DropFromIndex cleanup
    pub inodes: Vec<i64>,
    /// Corpus paths for the tracks
    pub paths: Vec<String>,
    /// Format summary (e.g., "FLAC (3)" or "MP3 (2)")
    pub format_summary: String,
    /// Total file size in MB
    pub total_size_mb: f64,
}

/// A single directory overlap cluster ready for resolution.
#[derive(Debug, Clone)]
pub struct DirectoryClusterEntry {
    /// Cluster key (sorted path suffixes joined by |)
    pub cluster_key: String,
    /// Signal ID for this cluster
    pub signal_id: i64,
    /// Directories in this cluster
    pub directories: Vec<DirectoryGroupEntry>,
    /// Source fingerprint overlap keys
    pub fingerprint_overlap_keys: Vec<String>,
}

/// Resolution option for a directory cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClusterResolutionOption {
    /// Keep one directory, stash the other(s)
    KeepDirectory { keep_suffix: String },
    /// Auto-select based on quality (higher format class wins)
    AutoQuality { keep_format: String, stash_format: String },
    /// Skip - don't resolve this cluster
    Skip,
}

impl ClusterResolutionOption {
    pub fn label(&self) -> String {
        match self {
            Self::KeepDirectory { keep_suffix } => format!("Keep {}/", keep_suffix),
            Self::AutoQuality { keep_format, stash_format } => {
                format!("Keep {}, stash {}", keep_format, stash_format)
            }
            Self::Skip => "Skip (mark as variant)".to_string(),
        }
    }
}

/// Cached data for the directory cluster resolution modal.
///
/// Loaded once when the modal opens. All renders use this cached data.
#[derive(Debug, Clone, Default)]
pub struct DirectoryClusterModalData {
    /// Directory overlap clusters
    pub clusters: Vec<DirectoryClusterEntry>,
}

impl DirectoryClusterModalData {
    /// Load directory overlap clusters from the database.
    pub fn load(read_db: &ReadOnlyDb<'_>) -> Result<Self> {
        // Get all DirectoryOverlapCluster signals
        let signals = read_db
            .get_aggregate_signals(Some(AggregateSignalType::DirectoryOverlapCluster))
            .unwrap_or_default();

        if signals.is_empty() {
            return Ok(Self::default());
        }

        let mut clusters = Vec::new();

        for signal in signals {
            let signal_id = signal.id.unwrap_or(0);
            let cluster_key = signal.key.clone();

            // Parse metadata
            let metadata: serde_json::Value = signal
                .metadata_json
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();

            let fingerprint_overlap_keys: Vec<String> = metadata
                .get("fingerprint_overlap_keys")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let directories_json = metadata
                .get("directories")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            let mut directories = Vec::new();

            for dir_json in directories_json {
                let path_suffix = dir_json
                    .get("path_suffix")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let track_ids: Vec<i64> = dir_json
                    .get("track_ids")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect())
                    .unwrap_or_default();

                // Load track details for format/size info
                let mut inodes = Vec::new();
                let mut paths = Vec::new();
                let mut total_size: i64 = 0;
                let mut format_counts: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();

                for &track_id in &track_ids {
                    if let Ok(Some(track)) = read_db.get_track_by_id(track_id) {
                        inodes.push(track.inode);
                        paths.push(track.path.clone());
                        total_size += track.file_size;
                        *format_counts.entry(track.file_type.to_uppercase()).or_insert(0) += 1;
                    }
                }

                // Build format summary (e.g., "FLAC (3)" or "MP3 (2), FLAC (1)")
                let format_summary = if format_counts.len() == 1 {
                    let (fmt, count) = format_counts.iter().next().unwrap();
                    format!("{} ({})", fmt, count)
                } else {
                    let mut parts: Vec<String> = format_counts
                        .iter()
                        .map(|(fmt, count)| format!("{} ({})", fmt, count))
                        .collect();
                    parts.sort();
                    parts.join(", ")
                };

                let total_size_mb = total_size as f64 / (1024.0 * 1024.0);

                directories.push(DirectoryGroupEntry {
                    path_suffix,
                    track_ids,
                    inodes,
                    paths,
                    format_summary,
                    total_size_mb,
                });
            }

            // Skip clusters with fewer than 2 directories
            if directories.len() < 2 {
                continue;
            }

            clusters.push(DirectoryClusterEntry {
                cluster_key,
                signal_id,
                directories,
                fingerprint_overlap_keys,
            });
        }

        Ok(Self { clusters })
    }

    /// Total number of clusters.
    pub fn total_count(&self) -> usize {
        self.clusters.len()
    }

    /// Check if there are any clusters.
    pub fn has_clusters(&self) -> bool {
        !self.clusters.is_empty()
    }

    /// Generate mutations for a resolution option on a specific cluster.
    ///
    /// For KeepDirectory: MoveToStash + DropFromIndex for all directories EXCEPT the kept one.
    /// For AutoQuality: Same logic, but picks the keep directory by format quality.
    /// For Skip: No mutations.
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

        let keep_suffix = match option {
            ClusterResolutionOption::Skip => return mutations,
            ClusterResolutionOption::KeepDirectory { keep_suffix } => keep_suffix.clone(),
            ClusterResolutionOption::AutoQuality { keep_format, .. } => {
                // Find directory with the preferred format
                cluster
                    .directories
                    .iter()
                    .find(|d| d.format_summary.starts_with(keep_format))
                    .map(|d| d.path_suffix.clone())
                    .unwrap_or_default()
            }
        };

        // Stash all directories except the one we're keeping
        for dir in &cluster.directories {
            if dir.path_suffix == keep_suffix {
                continue;
            }

            for (idx, corpus_path) in dir.paths.iter().enumerate() {
                let abs_path = resolver.resolve(std::path::Path::new(corpus_path));

                // MoveToStash mutation
                mutations.push(Mutation::MoveToStash {
                    path: abs_path,
                    stash_name: "overlaps".to_string(),
                });

                // DropFromIndex mutation
                let track_id = dir.track_ids.get(idx).copied().unwrap_or(0);
                let inode = dir.inodes.get(idx).copied();

                mutations.push(Mutation::DropFromIndex {
                    track_id,
                    path: PathBuf::from(corpus_path),
                    inode,
                    source: Some("corpus".to_string()),
                });
            }
        }

        mutations
    }
}

/// Which action button is selected in the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectedButton {
    Confirm,
    #[default]
    Cancel,
}

impl SelectedButton {
    /// Move selection left.
    pub fn left(&mut self, has_options: bool) {
        *self = match *self {
            Self::Cancel => {
                if has_options {
                    Self::Confirm
                } else {
                    Self::Cancel
                }
            }
            Self::Confirm => Self::Confirm,
        };
    }

    /// Move selection right.
    pub fn right(&mut self, _has_options: bool) {
        *self = match *self {
            Self::Confirm => Self::Cancel,
            Self::Cancel => Self::Cancel,
        };
    }
}
