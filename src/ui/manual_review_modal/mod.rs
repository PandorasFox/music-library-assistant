//! Manual Review Modal
//!
//! Provides the interactive workflow for resolving groups of files that
//! require manual operator intervention. Supports three review kinds:
//! - RedundantDuplicate: equal-quality duplicates needing operator choice
//! - DeployConflict: multiple files mapping to the same deploy path
//! - MetadataDuplicate: files with identical tag signatures

pub mod preview;
pub mod types;

pub use preview::{ManualReviewAction, ManualReviewState, render};
pub use types::ReviewKind;
