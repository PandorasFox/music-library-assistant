//! Subpar Duplicate Resolution Flow UI Module
//!
//! Provides the interactive workflow for resolving subpar corpus duplicates.
//! Shows files identified as lower-quality versions by fingerprint analysis
//! and allows the operator to stash + drop them from the index.

pub mod preview;
pub mod types;

pub use preview::{SubparDuplicatePreviewAction, SubparDuplicatePreviewState};
pub use types::SubparDuplicateModalData;
