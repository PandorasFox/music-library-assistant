//! Corrupt File Resolution Modal
//!
//! Provides the interactive workflow for resolving corrupt corpus files.
//! Shows files with tag parse errors or waveform decode failures and allows
//! the operator to stash + drop them from the index.

pub mod preview;
pub mod types;

pub use preview::{CorruptFilePreviewAction, CorruptFilePreviewState};
pub use types::{CorruptFileModalData, stash_and_drop_mutations};
