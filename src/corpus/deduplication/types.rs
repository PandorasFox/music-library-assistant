//! Core deduplication types.
//!
//! Data structures used across the deduplication system.

use std::collections::HashMap;

use crate::corpus::db::{PendingChange, Track};

use super::{AutoIgnoreState, ConflictSet};

/// A cluster of duplicates sharing EXACTLY this set of directories.
/// Uses non-transitive grouping - duplicates in {A, B} are kept separate from
/// those in {A, B, C}.
#[derive(Debug, Clone)]
pub struct DirectorySetCluster {
    /// Sorted directory names (e.g., ["Tracks-DaB", "Tracks-trans"])
    pub directory_set: Vec<String>,
    /// Fingerprints that exist in EXACTLY these directories
    pub fingerprints: Vec<String>,
    /// Total file count across all directories
    pub file_count: usize,
    /// Number of directories (cluster magnitude)
    pub magnitude: usize,
    /// Tracks grouped by directory name (aggregated across all fingerprints)
    pub tracks_by_dir: HashMap<String, Vec<Track>>,
}

/// Decision state for a single cluster.
#[derive(Debug, Clone)]
pub struct ClusterDecision {
    /// The cluster this decision is for
    pub cluster: DirectorySetCluster,
    /// Directory chosen as keeper (None = not decided yet)
    pub keeper_dir: Option<String>,
    /// Generated pending changes (populated after decision)
    pub pending_changes: Vec<PendingChange>,
}

/// Session state for the deduplication workflow.
#[derive(Debug, Clone)]
pub struct DeduplicationSession {
    /// Change session ID for tracking in database
    pub session_id: String,
    /// Clusters sorted by magnitude desc, then file_count desc
    pub clusters: Vec<DirectorySetCluster>,
    /// Current cluster index being reviewed
    pub current_index: usize,
    /// Completed decisions
    pub decisions: Vec<ClusterDecision>,
    /// Whether bulk phase is complete (individual conflicts remain)
    pub bulk_phase_complete: bool,
    /// Common divergence root path (e.g., "web/rips/spotify/")
    pub divergence_root: String,
    /// Original conflict sets (needed for change generation)
    pub conflict_sets: Vec<ConflictSet>,
    /// Auto-ignore state
    pub auto_ignore: AutoIgnoreState,
}

impl DeduplicationSession {
    /// Get the current cluster being reviewed, if any.
    pub fn current_cluster(&self) -> Option<&DirectorySetCluster> {
        self.clusters.get(self.current_index)
    }

    /// Check if all clusters have been processed.
    pub fn is_complete(&self) -> bool {
        self.current_index >= self.clusters.len()
    }

    /// Get total pending changes across all decisions.
    pub fn total_pending_changes(&self) -> usize {
        self.decisions.iter().map(|d| d.pending_changes.len()).sum()
    }

    /// Get count of files that will be kept.
    pub fn files_to_keep(&self) -> usize {
        let mut kept = 0;
        for decision in &self.decisions {
            if let Some(keeper) = &decision.keeper_dir {
                // Use cluster.tracks_by_dir directly if available (directory-set mode)
                if !decision.cluster.tracks_by_dir.is_empty() {
                    if let Some(tracks) = decision.cluster.tracks_by_dir.get(keeper) {
                        kept += tracks.len();
                    }
                } else {
                    // Legacy mode: search through conflict_sets
                    for cs in &self.conflict_sets {
                        let mut cs_dirs: Vec<_> = cs.conflict_dirs.clone();
                        cs_dirs.sort();
                        if cs_dirs == decision.cluster.directory_set {
                            if let Some(tracks) = cs.tracks_by_dir.get(keeper) {
                                kept += tracks.len();
                            }
                        }
                    }
                }
            }
        }
        kept
    }

    /// Get statistics by keeper directory.
    pub fn keeper_stats(&self) -> HashMap<String, usize> {
        let mut stats: HashMap<String, usize> = HashMap::new();
        for decision in &self.decisions {
            if let Some(keeper) = &decision.keeper_dir {
                if !decision.cluster.tracks_by_dir.is_empty() {
                    if let Some(tracks) = decision.cluster.tracks_by_dir.get(keeper) {
                        *stats.entry(keeper.clone()).or_default() += tracks.len();
                    }
                } else {
                    for cs in &self.conflict_sets {
                        let mut cs_dirs: Vec<_> = cs.conflict_dirs.clone();
                        cs_dirs.sort();
                        if cs_dirs == decision.cluster.directory_set {
                            if let Some(tracks) = cs.tracks_by_dir.get(keeper) {
                                *stats.entry(keeper.clone()).or_default() += tracks.len();
                            }
                        }
                    }
                }
            }
        }
        stats
    }
}
