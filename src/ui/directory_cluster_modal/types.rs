//! Cross-Source Overlap Resolution Types
//!
//! Data structures for the cross-source overlap resolution modal, including
//! cluster entries, resolution options, and stash file preview.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;

use crate::corpus::paths;
use crate::db::types::Zone;
use crate::db::ReadOnlyDb;
use crate::meta::mutations::file_ops::StashFromZoneMutation;
use crate::meta::mutations::indexing::DropFromIndexMutation;
use crate::meta::mutations::Mutation;
use crate::ui::manual_review_modal::types::FileMetaSummary;

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
    /// Whether this source can have duplicates stashed (from config, default: true).
    pub can_stash_dupes: bool,
}

/// A single cross-source overlap cluster ready for resolution.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone, Default)]
pub struct DirectoryClusterModalData {
    /// Cross-source overlap clusters
    pub clusters: Vec<DirectoryClusterEntry>,
    /// Audio metadata cache keyed by inode (loaded at init time).
    pub file_meta_cache: HashMap<i64, FileMetaSummary>,
}

impl DirectoryClusterModalData {
    /// Load cross-source overlap clusters from the database.
    pub fn load(read_db: &ReadOnlyDb<'_>) -> Result<Self> {
        // Get all CrossSourceOverlap signals (typed, no JSON parsing needed)
        let signals = read_db
            .get_cross_source_overlap_signals()
            .unwrap_or_default();

        if signals.is_empty() {
            return Ok(Self::default());
        }

        let mut clusters = Vec::new();

        for signal in signals {
            let cluster_key = signal.key;
            let data = signal.data;

            let source_a = data.source_a;
            let source_b = data.source_b;
            let source_a_can_stash = data.source_a_can_stash;
            let source_b_can_stash = data.source_b_can_stash;
            let overlap_count = data.overlap_count;

            // Collect all inodes for each source from typed track pairs
            let source_a_inodes: Vec<i64> = data
                .track_pairs
                .iter()
                .map(|tp| tp.source_a_inode)
                .collect();
            let source_b_inodes: Vec<i64> = data
                .track_pairs
                .iter()
                .map(|tp| tp.source_b_inode)
                .collect();

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
                let mut format_counts: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();

                for &inode in &unique_inodes {
                    if let Ok(Some(audio_file)) =
                        read_db.get_audio_file_by_inode(inode, Zone::Corpus)
                    {
                        paths.push(audio_file.path().to_string());
                        *format_counts
                            .entry(audio_file.audio.file_type.to_uppercase())
                            .or_insert(0) += 1;
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

                directories.push(DirectoryGroupEntry {
                    path_suffix: source_path.clone(),
                    inodes: unique_inodes,
                    paths,
                    format_summary,
                    can_stash_dupes: can_stash,
                });
            }

            // Skip clusters with fewer than 2 sources (shouldn't happen)
            if directories.len() < 2 {
                continue;
            }

            clusters.push(DirectoryClusterEntry {
                cluster_key,
                directories,
                overlap_count,
            });
        }

        // Enrich with audio metadata for all unique inodes across all clusters
        let mut file_meta_cache = HashMap::new();
        for cluster in &clusters {
            for dir in &cluster.directories {
                for &inode in &dir.inodes {
                    if file_meta_cache.contains_key(&inode) {
                        continue;
                    }
                    let audio_info = read_db.get_audio_info(inode).ok().flatten();
                    let tags = read_db.get_corpus_tags(inode).ok().unwrap_or_default();
                    let has_pictures = read_db.get_has_pictures(inode).unwrap_or(false);

                    if let Some(info) = audio_info {
                        let file_size = read_db
                            .get_audio_file_by_inode(inode, Zone::Corpus)
                            .ok()
                            .flatten()
                            .map(|af| af.entry.file_size)
                            .unwrap_or(0);

                        file_meta_cache.insert(
                            inode,
                            FileMetaSummary {
                                file_type: info.file_type,
                                duration_ms: info.duration_ms,
                                bitrate_kbps: info.bitrate_kbps,
                                sample_rate: info.sample_rate,
                                file_size,
                                has_pictures,
                                tags: tags
                                    .into_iter()
                                    .map(|t| (t.tag_name, t.tag_value))
                                    .collect(),
                            },
                        );
                    }
                }
            }
        }

        Ok(Self {
            clusters,
            file_meta_cache,
        })
    }

    /// Load release overlap clusters from the database.
    ///
    /// Converts `ReleaseOverlapSignal` entries into `DirectoryClusterEntry` +
    /// `DirectoryGroupEntry`, reusing the exact same types as cross-source overlaps.
    pub fn load_release_overlaps(read_db: &ReadOnlyDb<'_>) -> Result<Self> {
        let signals = read_db.get_release_overlap_signals().unwrap_or_default();

        if signals.is_empty() {
            return Ok(Self::default());
        }

        let mut clusters = Vec::new();

        for signal in signals {
            let cluster_key = signal.key;
            let data = signal.data;

            let mut directories = Vec::new();

            for entry in &data.releases {
                // Use "source_dir/release_dir" as the path suffix
                let path_suffix = if entry.release_dir.is_empty() {
                    entry.source_dir.clone()
                } else {
                    format!("{}/{}", entry.source_dir, entry.release_dir)
                };

                // Deduplicate inodes
                let unique_inodes: Vec<i64> = {
                    let mut seen = std::collections::HashSet::new();
                    entry
                        .inodes
                        .iter()
                        .copied()
                        .filter(|i| seen.insert(*i))
                        .collect()
                };

                let mut paths = Vec::new();
                let mut format_counts: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();

                for &inode in &unique_inodes {
                    if let Ok(Some(audio_file)) =
                        read_db.get_audio_file_by_inode(inode, Zone::Corpus)
                    {
                        paths.push(audio_file.path().to_string());
                        *format_counts
                            .entry(audio_file.audio.file_type.to_uppercase())
                            .or_insert(0) += 1;
                    }
                }

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

                directories.push(DirectoryGroupEntry {
                    path_suffix,
                    inodes: unique_inodes,
                    paths,
                    format_summary,
                    can_stash_dupes: entry.can_stash,
                });
            }

            if directories.len() < 2 {
                continue;
            }

            clusters.push(DirectoryClusterEntry {
                cluster_key,
                directories,
                overlap_count: data.file_count,
            });
        }

        // Enrich with audio metadata
        let mut file_meta_cache = HashMap::new();
        for cluster in &clusters {
            for dir in &cluster.directories {
                for &inode in &dir.inodes {
                    if file_meta_cache.contains_key(&inode) {
                        continue;
                    }
                    let audio_info = read_db.get_audio_info(inode).ok().flatten();
                    let tags = read_db.get_corpus_tags(inode).ok().unwrap_or_default();
                    let has_pictures = read_db.get_has_pictures(inode).unwrap_or(false);

                    if let Some(info) = audio_info {
                        let file_size = read_db
                            .get_audio_file_by_inode(inode, Zone::Corpus)
                            .ok()
                            .flatten()
                            .map(|af| af.entry.file_size)
                            .unwrap_or(0);

                        file_meta_cache.insert(
                            inode,
                            FileMetaSummary {
                                file_type: info.file_type,
                                duration_ms: info.duration_ms,
                                bitrate_kbps: info.bitrate_kbps,
                                sample_rate: info.sample_rate,
                                file_size,
                                has_pictures,
                                tags: tags
                                    .into_iter()
                                    .map(|t| (t.tag_name, t.tag_value))
                                    .collect(),
                            },
                        );
                    }
                }
            }
        }

        Ok(Self {
            clusters,
            file_meta_cache,
        })
    }

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
