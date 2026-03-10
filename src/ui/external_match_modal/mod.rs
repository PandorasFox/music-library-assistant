//! External Match Browser Modal
//!
//! Read-only browser: track → MB recording URL + confidence.
//! Shows cached MB recording data inline when available.
//! Uses StandardList with wizard system for recording detail (Z key).

pub mod render;
pub mod types;

pub use render::render;
pub use types::{ExternalMatchReviewAction, ExternalMatchReviewState};
