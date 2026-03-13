//! Missing File Resolution Modal
//!
//! Provides the interactive workflow for resolving missing corpus files.
//! Shows categorized files (restorable vs non-restorable) and allows the
//! operator to restore from library or drop from index.

pub mod preview;
pub mod types;

pub use preview::{MissingFilePreviewAction, MissingFilePreviewState};
pub use types::{MissingFileModalData, restore_mutations};
