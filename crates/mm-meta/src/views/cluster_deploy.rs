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
// Shit Format Modal Types
// ============================================================================

/// Default Opus bitrate in kbps.
const DEFAULT_OPUS_BITRATE: u32 = 128;

/// Minimum Opus bitrate.
const MIN_OPUS_BITRATE: u32 = 32;

/// Maximum Opus bitrate.
const MAX_OPUS_BITRATE: u32 = 512;

/// Bitrate adjustment step.
const BITRATE_STEP: u32 = 8;

/// Lossless formats that can be remuxed to FLAC without quality loss.
const LOSSLESS_FORMATS: &[&str] = &["wav", "aiff", "aif", "ape", "wv"];

/// Lossy formats that need transcoding to Opus.
const LOSSY_FORMATS: &[&str] = &["mp3", "m4a", "aac", "wma"];

/// A file with ShitFormat signal (non-Vorbis container).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShitFormatEntry {
    /// Corpus path (relative)
    pub corpus_path: String,
    /// Inode of the file
    pub inode: i64,
    /// File type (mp3, m4a, etc.)
    pub file_type: String,
}

impl ShitFormatEntry {
    /// Check if this file is a lossless format.
    pub fn is_lossless(&self) -> bool {
        LOSSLESS_FORMATS.contains(&self.file_type.to_lowercase().as_str())
    }

    /// Check if this file is a lossy format.
    pub fn is_lossy(&self) -> bool {
        LOSSY_FORMATS.contains(&self.file_type.to_lowercase().as_str())
    }
}

/// Cached data for the shit format resolution modal.
///
/// Loaded once when the modal opens. All renders use this cached data.
/// Separates files into lossless (remux) and lossy (transcode) categories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShitFormatModalData {
    /// Lossless files (WAV, AIFF, APE, WV) - remux to FLAC
    pub lossless_files: Vec<ShitFormatEntry>,
    /// Lossy files (MP3, M4A, AAC, WMA) - transcode to Opus or capture to FLAC
    pub lossy_files: Vec<ShitFormatEntry>,
    /// File counts by type (for display breakdown)
    pub file_counts: HashMap<String, i64>,
    /// Opus bitrate in kbps (user-adjustable, for lossy only)
    pub opus_bitrate_kbps: u32,
    /// When true, lossy files are captured to FLAC instead of transcoded to Opus
    pub lossy_to_flac: bool,
}

impl Default for ShitFormatModalData {
    fn default() -> Self {
        Self {
            lossless_files: Vec::new(),
            lossy_files: Vec::new(),
            file_counts: HashMap::new(),
            opus_bitrate_kbps: DEFAULT_OPUS_BITRATE,
            lossy_to_flac: false,
        }
    }
}

impl ShitFormatModalData {
    /// Construct from pre-loaded data (used by modal_loaders).
    pub fn new_from_loaded(
        lossless_files: Vec<ShitFormatEntry>,
        lossy_files: Vec<ShitFormatEntry>,
        file_counts: HashMap<String, i64>,
    ) -> Self {
        Self {
            lossless_files,
            lossy_files,
            file_counts,
            opus_bitrate_kbps: DEFAULT_OPUS_BITRATE,
            lossy_to_flac: false,
        }
    }

    /// Total number of shit format files.
    pub fn total_count(&self) -> usize {
        self.lossless_files.len() + self.lossy_files.len()
    }

    /// Check if there are lossless files.
    pub fn has_lossless(&self) -> bool {
        !self.lossless_files.is_empty()
    }

    /// Check if there are lossy files.
    pub fn has_lossy(&self) -> bool {
        !self.lossy_files.is_empty()
    }

    /// Get lossless file type breakdown as sorted vec.
    pub fn lossless_breakdown(&self) -> Vec<(&str, i64)> {
        let mut breakdown: Vec<(&str, i64)> = self
            .file_counts
            .iter()
            .filter(|(k, _)| LOSSLESS_FORMATS.contains(&k.to_lowercase().as_str()))
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        breakdown.sort_by(|a, b| b.1.cmp(&a.1));
        breakdown
    }

    /// Get lossy file type breakdown as sorted vec.
    pub fn lossy_breakdown(&self) -> Vec<(&str, i64)> {
        let mut breakdown: Vec<(&str, i64)> = self
            .file_counts
            .iter()
            .filter(|(k, _)| LOSSY_FORMATS.contains(&k.to_lowercase().as_str()))
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        breakdown.sort_by(|a, b| b.1.cmp(&a.1));
        breakdown
    }

    /// Adjust bitrate (within bounds).
    pub fn adjust_bitrate(&mut self, delta: i32) {
        let new_bitrate = (self.opus_bitrate_kbps as i32 + delta)
            .max(MIN_OPUS_BITRATE as i32)
            .min(MAX_OPUS_BITRATE as i32);
        self.opus_bitrate_kbps = new_bitrate as u32;
    }

    /// Decrease bitrate by one step.
    pub fn decrease_bitrate(&mut self) {
        self.adjust_bitrate(-(BITRATE_STEP as i32));
    }

    /// Increase bitrate by one step.
    pub fn increase_bitrate(&mut self) {
        self.adjust_bitrate(BITRATE_STEP as i32);
    }

    /// Generate Transcode mutations for lossless files only (remux to FLAC).
    pub fn lossless_mutations(&self, resolver: &PathResolver) -> Vec<mutations::Mutation> {
        self.lossless_files
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

    /// Generate Transcode mutations for lossy files only.
    ///
    /// When `lossy_to_flac` is true, captures to FLAC (lossless waveform capture).
    /// Otherwise transcodes to Opus at the configured bitrate.
    pub fn lossy_mutations(&self, resolver: &PathResolver) -> Vec<mutations::Mutation> {
        let target = if self.lossy_to_flac {
            TranscodeTarget::FlacLossyCapture
        } else {
            TranscodeTarget::Opus {
                bitrate_kbps: self.opus_bitrate_kbps,
            }
        };

        self.lossy_files
            .iter()
            .map(|file| {
                let abs_path = resolver.resolve(std::path::Path::new(&file.corpus_path));

                mutations::Mutation::Transcode(TranscodeMutation {
                    inode: file.inode,
                    source_path: abs_path,
                    target_format: target,
                    stash_name: "originals".to_string(),
                })
            })
            .collect()
    }

    /// Generate Transcode mutations for all files.
    pub fn all_mutations(&self, resolver: &PathResolver) -> Vec<mutations::Mutation> {
        let mut mutations = self.lossless_mutations(resolver);
        mutations.extend(self.lossy_mutations(resolver));
        mutations
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
