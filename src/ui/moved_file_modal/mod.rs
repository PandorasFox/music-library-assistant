//! Moved file acknowledgement modal.
//!
//! Handles acknowledging files that have been moved to a new path.
//! When acknowledged, queues UpdateFilePath mutations to update the
//! stored paths in the database.

mod render;
mod types;

pub use render::render;
pub use types::{MovedFileAction, MovedFileState};
