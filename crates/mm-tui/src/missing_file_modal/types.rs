//! Missing File Resolution Modal Types
//!
//! Button, action, and context types are defined in mm-ui. This module
//! re-exports them for use in mm-tui rendering and action handlers.

pub use mm_meta::views::health_modals::*;
pub use mm_ui::resolutions::missing_file::{
    MissingFileAction, MissingFileButton, MissingFileButtonCtx, MissingFilePreviewState,
};
