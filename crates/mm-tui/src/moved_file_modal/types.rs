//! Types for moved file acknowledgement modal.
//!
//! Data, button, action, and state types are defined in mm-ui. This module
//! re-exports them for use in mm-tui rendering and action handlers.

pub use mm_ui::resolutions::moved_file::{
    MovedFileAction, MovedFileData, MovedFileState,
};
