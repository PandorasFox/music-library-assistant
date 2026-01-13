//! Deduplication Operations Module
//!
//! Provides fingerprint-based deduplication for surgical duplicate removal.
//! This module re-exports from the corpus::deduplication module.
//!
//! ## Types
//!
//! - `ConflictSet` - Tracks sharing a fingerprint across directories
//! - `DirectorySetCluster` - Aggregated duplicates by exact directory set
//! - `ClusterDecision` - Decision state for a cluster
//! - `DeduplicationSession` - Session state for the workflow
//! - `AutoIgnoreState` - Tracks directories to auto-skip

// Re-export from corpus::deduplication module
pub use crate::corpus::deduplication::{
    AutoIgnoreState,
    ClusterDecision,
    ConflictSet,
    DeduplicationSession,
    DirectorySetCluster,
    compute_directory_set_clusters,
    find_duplicates_between_directories,
    find_fingerprint_duplicates,
    generate_cluster_changes,
};
