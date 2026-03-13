//! Missing Directory Resolution Modal
//!
//! Provides the interactive workflow for acknowledging and dropping missing directories
//! from the index. When a directory is deleted externally, this modal allows the operator
//! to acknowledge the deletion and remove the directory and its contents from the index.

pub mod preview;

pub use mm_meta::views::health_modals::MissingDirectoryModalData;
pub use preview::{MissingDirectoryPreviewAction, MissingDirectoryPreviewState};
