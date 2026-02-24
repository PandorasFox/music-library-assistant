//! External Match Review Modal
//!
//! Displays corpus files with AcoustID match metadata for operator review.
//! Accept applies external tag values, Dismiss skips the file.

pub mod types;
pub mod render;

pub use types::{ExternalMatchReviewState, ExternalMatchReviewAction};
pub use render::render;
