//! Corrupt File Resolution Modal
//!
//! Provides the interactive workflow for resolving corrupt corpus files.
//! Shows files with tag parse errors or waveform decode failures and allows
//! the operator to stash + drop them from the index.
//!
//! Uses the generic `ResolutionState` — only the data wrapper, button enum,
//! and rendering are modal-specific.

mod preview;
pub mod types;

pub use types::{
    CorruptFileData, CorruptFileModalData, CorruptFilePreviewAction, CorruptFilePreviewState,
    stash_and_drop_mutations,
};
