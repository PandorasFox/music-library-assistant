//! OOB Tag Sync Resolution Modal
//!
//! Displays files with purely one-directional tag mismatches (extras only on
//! disk, or extras only in DB) and allows bulk acceptance in either direction.

pub mod render;
pub mod types;

pub use render::render;
pub use types::{OobSyncAction, OobSyncState};
