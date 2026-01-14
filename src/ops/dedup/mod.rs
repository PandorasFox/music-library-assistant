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
