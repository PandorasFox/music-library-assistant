//! Deduplication Operations Module
//!
//! Provides fingerprint-based deduplication for surgical duplicate removal.
//! This module re-exports from the top-level `deduplication` module.
//!
//! ## Module Organization
//!
//! Currently, the implementation lives in `src/deduplication.rs`. This module
//! provides the canonical path (`ops::dedup`) for accessing dedup functionality.
//!
//! ### Future Structure
//!
//! The plan is to split into submodules:
//! - `clustering.rs` - DirectorySetCluster, compute_directory_set_clusters
//! - `changes.rs` - ClusterDecision, generate_cluster_changes
//! - `sleuthing.rs` - find_duplicates_between_directories
//! - `session.rs` - DeduplicationSession, ConflictSet, SessionStats
//!
//! ## Types
//!
//! - `ConflictSet` - Tracks sharing a fingerprint across directories
//! - `DirectorySetCluster` - Aggregated duplicates by exact directory set
//! - `ClusterDecision` - Decision state for a cluster
//! - `DeduplicationSession` - Session state for the workflow
//! - `SessionStats` - Statistics from dedup session
//! - `AutoIgnoreState` - Tracks directories to auto-skip

// Re-export everything from the top-level deduplication module
// Note: These re-exports provide the canonical path for dedup types.
// Callers should prefer ops::dedup over top-level deduplication.
#[allow(unused_imports)]
pub use crate::deduplication::{
    // Session and state types
    AutoIgnoreState,
    ClusterDecision,
    ConflictSet,
    DeduplicationSession,
    DirectoryCluster,
    DirectorySetCluster,
    SessionStats,
    // Functions
    compute_directory_set_clusters,
    find_duplicates_between_directories,
    find_fingerprint_duplicates,
    generate_cluster_changes,
};
