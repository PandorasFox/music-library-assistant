//! OOB Tag Sync Resolution Flow
//!
//! Displays files with purely one-directional tag mismatches (extras only on
//! disk, or extras only in DB) and allows bulk acceptance in either direction.

pub mod types;
pub mod render;

pub use types::{OobSyncState, OobSyncAction};
pub use render::render;
