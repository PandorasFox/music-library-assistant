//! Album Tag Resolution Flow
//!
//! Canonicalization for album names with EP/edition detection.
//! Identifies albums that should be unified:
//! - Spelling/capitalization variants ("Abbey Road" vs "abbey road")
//! - Format variants ("Album" vs "Album EP" vs "Album (EP)")
//! - Edition variants ("Album" vs "Album (Deluxe Edition)")
//!
//! When format or edition variants are detected, users can flag for
//! metadata-duplicate review to handle potential fingerprint overlaps.

mod cluster_view;
pub mod coordinator;
mod review;
mod session;
mod types;

pub use cluster_view::AlbumClusterState;
pub use review::AlbumReviewState;
pub use session::AlbumCanonSession;
pub use types::{AlbumBucket, AlbumClusterAction, AlbumReviewAction, AlbumVariant};
