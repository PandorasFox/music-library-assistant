//! Corrupt File Resolution Modal
//!
//! Provides the interactive workflow for resolving corrupt corpus files.
//! Shows files with tag parse errors or waveform decode failures and allows
//! the operator to stash + drop them from the index.
//!
//! Data wrapper, buttons, and actions live in `mm_ui::resolutions::corrupt_file`.
//! This module provides the ratatui-specific `ModalFrame` impl.

mod preview;
pub mod types;

pub use types::{
    CorruptFileAction, CorruptFileData, CorruptFileModalData, CorruptFileState,
};
