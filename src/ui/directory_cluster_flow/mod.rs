//! Directory Overlap Cluster Resolution Flow UI Module
//!
//! Provides the interactive workflow for resolving directory-level overlaps.
//! Aggregates fingerprint overlap signals by directory, allowing the operator
//! to choose which directory's version to keep vs stash.

pub mod preview;
pub mod types;

pub use preview::{DirectoryClusterPreviewAction, DirectoryClusterPreviewState};
pub use types::DirectoryClusterModalData;
