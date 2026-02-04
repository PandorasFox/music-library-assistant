//! Cross-Source Overlap Resolution Types
//!
//! Data structures for the cross-source overlap resolution modal, including
//! cluster entries, resolution options, and button state.

use std::path::PathBuf;

use anyhow::Result;

use crate::corpus::db::types::{AggregateSignalType, FileSource};
use crate::corpus::db::ReadOnlyDb;
use crate::corpus::mutations::Mutation;
use crate::corpus::paths;

/// A source directory within an overlap cluster.
#[derive(Debug, Clone)]
pub struct DirectoryGroupEntry {
    /// Source directory path (e.g., "web/releases/bandcamp" or "web/releases/indie")
    pub path_suffix: String,
    /// Inodes for files in this source
    pub inodes: Vec<i64>,
    /// Corpus paths for the tracks
    pub paths: Vec<String>,
    /// Format summary (e.g., "FLAC (3)" or "MP3 (2)")
    pub format_summary: String,
    /// Total file size in MB
    pub total_size_mb: f64,
    /// Whether this source can have duplicates stashed (from config, default: true)
    pub can_stash_dupes: bool,
}

/// A single cross-source overlap cluster ready for resolution.
#[derive(Debug, Clone)]
pub struct DirectoryClusterEntry {
    /// Cluster key (sorted source paths joined by |)
    pub cluster_key: String,
    /// Signal ID for this cluster
    pub signal_id: i64,
    /// Source directories in this cluster (usually 2)
    pub directories: Vec<DirectoryGroupEntry>,
    /// Source fingerprint overlap keys
    pub fingerprint_overlap_keys: Vec<String>,
    /// Number of overlapping track pairs
    pub overlap_count: usize,
}

/// Resolution option for a directory cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClusterResolutionOption {
    /// Keep one directory, stash the other(s)
    KeepDirectory { keep_suffix: String },
    /// Auto-select based on quality (higher format class wins)
    AutoQuality { keep_format: String, stash_format: String },
}

impl ClusterResolutionOption {
    pub fn label(&self) -> String {
        match self {
            Self::KeepDirectory { keep_suffix } => format!("Keep {}/", keep_suffix),
            Self::AutoQuality { keep_format, stash_format } => {
                format!("Keep {}, stash {}", keep_format, stash_format)
            }
        }
    }
}

/// Cached data for the cross-source overlap resolution modal.
///
/// Loaded once when the modal opens. All renders use this cached data.
#[derive(Debug, Clone, Default)]
pub struct DirectoryClusterModalData {
    /// Cross-source overlap clusters
    pub clusters: Vec<DirectoryClusterEntry>,
}

impl DirectoryClusterModalData {
    /// Load cross-source overlap clusters from the database.
    pub fn load(read_db: &ReadOnlyDb<'_>) -> Result<Self> {
        // Get all CrossSourceOverlap signals
        let signals = read_db
            .get_aggregate_signals(Some(AggregateSignalType::CrossSourceOverlap))
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

            let source_a = metadata
                .get("source_a")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let source_b = metadata
                .get("source_b")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let source_a_can_stash = metadata
                .get("source_a_can_stash")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            let source_b_can_stash = metadata
                .get("source_b_can_stash")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            let overlap_count = metadata
                .get("overlap_count")
                .and_then(|v| v.as_i64())
                .unwrap_or(0) as usize;

            let fingerprint_keys: Vec<String> = metadata
                .get("fingerprint_keys")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            // track_pairs: Vec<[source_a_inode, source_b_inode]>
            let track_pairs: Vec<(i64, i64)> = metadata
                .get("track_pairs")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|pair| {
                            let arr = pair.as_array()?;
                            let a = arr.first()?.as_i64()?;
                            let b = arr.get(1)?.as_i64()?;
                            Some((a, b))
                        })
                        .collect()
                })
                .unwrap_or_default();

            // Collect all inodes for each source
            let source_a_inodes: Vec<i64> = track_pairs.iter().map(|(a, _)| *a).collect();
            let source_b_inodes: Vec<i64> = track_pairs.iter().map(|(_, b)| *b).collect();

            // Build directory entries for each source
            let mut directories = Vec::new();

            for (source_path, inodes, can_stash) in [
                (&source_a, &source_a_inodes, source_a_can_stash),
                (&source_b, &source_b_inodes, source_b_can_stash),
            ] {
                // Deduplicate inodes
                let unique_inodes: Vec<i64> = {
                    let mut seen = std::collections::HashSet::new();
                    inodes.iter().copied().filter(|i| seen.insert(*i)).collect()
                };

                let mut paths = Vec::new();
                let mut total_size: i64 = 0;
                let mut format_counts: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();

                for &inode in &unique_inodes {
                    if let Ok(Some(audio_file)) = read_db.get_audio_file_by_inode(inode, FileSource::Corpus) {
                        paths.push(audio_file.path().to_string());
                        total_size += audio_file.entry.file_size;
                        *format_counts.entry(audio_file.audio.file_type.to_uppercase()).or_insert(0) += 1;
                    }
                }

                // Build format summary
                let format_summary = if format_counts.len() == 1 {
                    let (fmt, count) = format_counts.iter().next().unwrap();
                    format!("{} ({})", fmt, count)
                } else if format_counts.is_empty() {
                    "unknown".to_string()
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
                    path_suffix: source_path.clone(),
                    inodes: unique_inodes,
                    paths,
                    format_summary,
                    total_size_mb,
                    can_stash_dupes: can_stash,
                });
            }

            // Skip clusters with fewer than 2 sources (shouldn't happen)
            if directories.len() < 2 {
                continue;
            }

            clusters.push(DirectoryClusterEntry {
                cluster_key,
                signal_id,
                directories,
                fingerprint_overlap_keys: fingerprint_keys,
                overlap_count,
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
                let inode = dir.inodes.get(idx).copied();

                mutations.push(Mutation::DropFromIndex {
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
