//! Directory Overlap Cluster Resolution Modal
//!
//! Provides the interactive workflow for resolving directory-level overlaps.
//! Aggregates fingerprint overlap signals by directory, allowing the operator
//! to choose which directory's version to keep vs stash.

pub mod preview;
pub mod render_v3;
pub mod types;

pub use preview::{DirectoryClusterPreviewAction, DirectoryClusterPreviewState};
pub use types::DirectoryClusterModalData;
