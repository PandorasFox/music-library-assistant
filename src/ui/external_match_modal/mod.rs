//! External Match Browser Modal
//!
//! Read-only browser: track → MB recording URL + confidence.
//! Shows cached MB recording data inline when available.

pub mod types;
pub mod render;

pub use types::{ExternalMatchReviewState, ExternalMatchReviewAction};
pub use render::render;
