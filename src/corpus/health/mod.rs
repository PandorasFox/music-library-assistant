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
mod canonicalization;
pub mod collision;
mod detection;
pub mod filter;
mod heartbeat;
pub mod library;
pub mod normalization;
pub mod tag_cloud;

pub use canonicalization::detect_and_store_canonicalizations;
pub use collision::{
    get_album_artist_collisions, get_album_collisions, get_artist_collisions,
    get_genre_collisions, TagCollision,
};
pub use detection::{
    cleanup_resolved_deployment_conflicts, detect_deployment_conflicts,
    detect_deployment_conflicts_for_library, detect_fingerprint_issues, detect_metadata_issues,
};
#[allow(unused_imports)]
pub use filter::{
    durations_within_tolerance, is_legitimate_rerelease, is_same_album_different_tracks,
};
pub use heartbeat::{spawn_heartbeat, HeartbeatResult};
#[allow(unused_imports)]
pub use library::{
    check_all_libraries_health, check_library_health, generate_orphan_cleanup_mutations,
    LibraryHealthResult, OrphanFile, StaleDeployment,
};
pub use tag_cloud::{spawn_tag_cloud_build, TagCloud};
