//! Cluster and deploy modal data types.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::mutations;
use crate::mutations::transcode::TranscodeMutation;
use crate::paths::PathResolver;
use crate::transcode::TranscodeTarget;
use crate::views::{
    ConflictGroup, DeploySignalFile, LeftoverSignalFile, SidecarConflictGroup, StaleSignalFile,
};

// Cross-reference: FileMetaSummary is defined in the review_match view module.
use super::review_match::FileMetaSummary;

// ============================================================================
// Directory Cluster Modal Types
// ============================================================================

/// A source directory within an overlap cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryClusterEntry {
    /// Cluster key (sorted source paths joined by |)
    pub cluster_key: String,
    /// Source directories in this cluster (usually 2)
    pub directories: Vec<DirectoryGroupEntry>,
    /// Number of overlapping track pairs
    pub overlap_count: usize,
}

/// Cached data for the cross-source overlap resolution modal.
///
/// Loaded once when the modal opens. All renders use this cached data.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
}

// ============================================================================
// Lossless Remux Modal Types
// ============================================================================

/// A lossless non-Vorbis file that can be remuxed to FLAC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemuxCandidateEntry {
    /// Corpus path (relative)
    pub corpus_path: String,
    /// Inode of the file
    pub inode: i64,
    /// File type (wav, aiff, ape, wv)
    pub file_type: String,
}

/// Cached data for the lossless remux resolution modal.
///
/// Loaded once when the modal opens. All renders use this cached data.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LosslessRemuxModalData {
    /// Lossless files (WAV, AIFF, APE, WV) to remux to FLAC
    pub files: Vec<RemuxCandidateEntry>,
    /// File counts by type (for display breakdown)
    pub file_counts: HashMap<String, i64>,
}

impl LosslessRemuxModalData {
    /// Construct from pre-loaded data (used by modal_loaders).
    pub fn new_from_loaded(
        files: Vec<RemuxCandidateEntry>,
        file_counts: HashMap<String, i64>,
    ) -> Self {
        Self { files, file_counts }
    }

    /// Total number of remux candidate files.
    pub fn total_count(&self) -> usize {
        self.files.len()
    }

    /// Check if there are any files to remux.
    pub fn has_files(&self) -> bool {
        !self.files.is_empty()
    }

    /// Get file type breakdown as sorted vec.
    pub fn format_breakdown(&self) -> Vec<(&str, i64)> {
        let mut breakdown: Vec<(&str, i64)> = self
            .file_counts
            .iter()
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        breakdown.sort_by(|a, b| b.1.cmp(&a.1));
        breakdown
    }

    /// Generate Transcode mutations for all files (remux to FLAC).
    pub fn mutations(&self, resolver: &PathResolver) -> Vec<mutations::Mutation> {
        self.files
            .iter()
            .map(|file| {
                let abs_path = resolver.resolve(std::path::Path::new(&file.corpus_path));

                mutations::Mutation::Transcode(TranscodeMutation {
                    inode: file.inode,
                    source_path: abs_path,
                    target_format: TranscodeTarget::Flac,
                    stash_name: "originals".to_string(),
                })
            })
            .collect()
    }
}

// ============================================================================
// Deploy Modal Types
// ============================================================================

/// Directory aggregate for grouped file display.
/// Sorted by count descending.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryAggregate {
    /// Directory path
    pub directory: String,
    /// Number of audio files in this directory
    pub count: usize,
    /// Number of sidecar images in this directory
    pub sidecar_count: usize,
}

/// Per-library breakdown of deploy operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibrarySummary {
    pub library_name: String,
    pub healthy_count: usize,
    pub new_count: usize,
    pub leftover_count: usize,
    pub stale_count: usize,
    /// Leftovers whose library_path matches a pending new deployment destination
    pub replaced_count: usize,
}

/// A sidecar image file to deploy alongside audio files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarDeployEntry {
    /// Relative corpus path to the image file
    pub corpus_image_path: String,
    /// Target library name
    pub library_name: String,
    /// Album directory within the library (e.g. "Artist/Album")
    pub library_album_dir: String,
    /// Original filename (e.g. "cover.jpg")
    pub filename: String,
    /// Image format (e.g. "jpeg", "png")
    pub format: String,
    /// Image width in pixels
    pub width: u32,
    /// Image height in pixels
    pub height: u32,
    /// Image role (e.g. "cover_front", "cover_back", "other")
    pub role: String,
}

impl SidecarDeployEntry {
    /// Produce a HardLink mutation to deploy this sidecar image.
    pub fn to_mutation(&self, resolver: &PathResolver) -> mutations::Mutation {
        let source = resolver.resolve(std::path::Path::new(&self.corpus_image_path));
        let dest_rel = std::path::Path::new("libraries")
            .join(&self.library_name)
            .join(&self.library_album_dir)
            .join(&self.filename);
        let destination = resolver.resolve(&dest_rel);
        mutations::Mutation::HardLink(mutations::file_ops::HardLinkMutation {
            source,
            destination,
        })
    }
}

/// Result of converting deploy modal data into staged mutations.
pub struct DeployMutationSet {
    /// Mutations for DecisionKey::Deploy (leftovers -> stale -> new, in execution order)
    pub deploy: Vec<mutations::Mutation>,
    /// Mutations for DecisionKey::DeploySidecars
    pub sidecars: Vec<mutations::Mutation>,
    /// Files skipped due to missing library_name (config gap)
    pub skipped: usize,
}

/// Cached data for the deploy modal.
///
/// Loaded once when the modal opens, contains all signal lists.
/// This prevents database queries during render.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeployModalData {
    /// Healthy files: deployed at correct library path
    pub healthy: Vec<DeploySignalFile>,
    /// New files: ready to deploy (not yet in library)
    pub new: Vec<DeploySignalFile>,
    /// New files aggregated by directory (sorted by count desc)
    pub new_by_dir: Vec<DirectoryAggregate>,
    /// Conflict groups: multiple corpus files -> same library path
    pub conflicts: Vec<ConflictGroup>,
    /// Leftover files: in library but no corpus backing
    pub leftover: Vec<LeftoverSignalFile>,
    /// Leftover files aggregated by directory (sorted by count desc)
    pub leftover_by_dir: Vec<DirectoryAggregate>,
    /// Stale files: deployed at wrong path (tags changed)
    pub stale: Vec<StaleSignalFile>,
    /// Per-library breakdown (only populated when > 1 library)
    pub per_library: Vec<LibrarySummary>,
    /// Total leftovers that will be replaced by new deployments
    pub replaced_count: usize,
    /// Sidecar images to deploy alongside audio files
    pub sidecars: Vec<SidecarDeployEntry>,
    /// Sidecar image conflicts (multiple corpus images -> same library path)
    pub sidecar_conflicts: Vec<SidecarConflictGroup>,
}

impl DeployModalData {
    /// Get counts for each tab (for display in tab bar).
    pub fn tab_counts(&self) -> [usize; 5] {
        [
            self.healthy.len(),
            self.new.len() + self.sidecars.len(),
            self.conflicts.len() + self.sidecar_conflicts.len(),
            self.leftover.len(),
            self.stale.len(),
        ]
    }

    /// Total operations that will be performed (excluding healthy).
    /// Conflict winners now appear in `new` via the computation layer.
    pub fn total_operations(&self) -> usize {
        self.new.len() + self.stale.len() + self.leftover.len() + self.sidecars.len()
    }

    /// Convert all deploy modal data into staged mutations.
    ///
    /// Ordering within `deploy`: leftovers first -> stale -> new
    /// (stash orphans before moving/linking into those paths).
    pub fn to_mutations(&self, resolver: &PathResolver) -> DeployMutationSet {
        let mut deploy = Vec::new();
        let mut skipped = 0usize;

        // 1. Leftovers: stash orphan files to clear destination paths
        for file in &self.leftover {
            deploy.push(file.to_mutation(resolver));
        }

        // 2. Stale: move existing deployments to correct paths
        for file in &self.stale {
            deploy.push(file.to_mutation(resolver));
        }

        // 3. New: deploy hard links (destinations now clear)
        for file in &self.new {
            match file.to_mutation(resolver) {
                Some(m) => deploy.push(m),
                None => skipped += 1,
            }
        }

        // 4. Sidecars: separate decision
        let sidecars: Vec<_> = self
            .sidecars
            .iter()
            .map(|s| s.to_mutation(resolver))
            .collect();

        DeployMutationSet {
            deploy,
            sidecars,
            skipped,
        }
    }
}
