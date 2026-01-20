//! Corpus Health System
//!
//! Health is a continuously maintained invariant - not computed on-demand.
//! This module provides detection, filtering, and tracking of corpus health issues.
//!
//! Flow:
//! ```text
//! SCAN/INDEX → detect issues → store in DB
//!      ↓
//! MUTATION (tag edit, move, delete) → update affected issues
//!      ↓
//! REPORTS → read from DB (fast, cached)
//!      ↓
//! TRIAGE UI → resolve issues → update DB
//! ```
//!
//! ## Health Issue Categories
//!
//! - **Fingerprint Duplicates**: Exact fingerprint match across files
//! - **Legitimate Re-releases**: Same fingerprint, different album metadata
//! - **Metadata Collisions**: Same artist/album/title, different fingerprint
//! - **Tag Canonicalization**: Tag value variants needing unification (artist, genre, album, etc.)
//! - **Quality Variants**: Same content, different quality (FLAC vs MP3)
//!
//! ## Library Health
//!
//! - **Healthy**: Deployed files at correct paths
//! - **Not Deployed**: Corpus files missing from library
//! - **Stale Deployments**: Tag changes caused incorrect library paths
//! - **Orphans**: Library files without corpus backing
//! - **Deployment Conflicts**: Multiple corpus files would deploy to same path

pub mod album_normalization;
pub mod canonicalization;
pub mod collision;
mod detection;
pub mod filter;
pub mod insights;
pub mod library;
pub mod normalization;
pub mod tag_cloud;

pub use detection::{cleanup_resolved_deployment_conflicts, refresh_health_for_track};
pub use canonicalization::detect_and_store_canonicalizations;

// TODO: Canonicalization detection should be reimplemented with direct SQL queries
// against the read-only db accessor on the task daemon, replacing the TagCloud approach.
// See: canonicalization.rs, tag_cloud.rs, collision.rs
