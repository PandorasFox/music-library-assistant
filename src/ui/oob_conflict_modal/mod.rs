//! OOB Tag Bucketed Resolution Modal
//!
//! Four-bucket tabbed inspector for all OOB tag signals:
//! - No Changes: signal exists but tags match (dismiss)
//! - DB Only: tags in DB not on disk (resolvable)
//! - Disk Only: tags on disk not in DB (resolvable)
//! - Conflicts: both directions differ (inspection only)
//!
//! Resolvable buckets offer bulk "Apply DB → Files" or
//! "Assimilate Files → DB" resolution via TransactionReview.

pub mod render;
pub mod types;

pub use render::render;
pub use types::{OobConflictAction, OobConflictState};
