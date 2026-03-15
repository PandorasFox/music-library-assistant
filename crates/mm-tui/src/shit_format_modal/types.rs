//! Shit Format Resolution Modal Types
//!
//! Button, action, and context types are defined in mm-ui. This module
//! re-exports them for use in mm-tui rendering and action handlers.

pub use mm_meta::views::cluster_deploy::{ShitFormatEntry, ShitFormatModalData};
pub use mm_ui::resolutions::shit_format::{
    ShitFormatAction, ShitFormatButton, ShitFormatButtonCtx, ShitFormatPreviewState,
};
