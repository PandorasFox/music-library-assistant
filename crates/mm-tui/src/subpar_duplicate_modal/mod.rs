//! Subpar Duplicate Resolution Modal
//!
//! Data wrapper, buttons, and actions live in `mm_ui::resolutions::subpar_duplicate`.
//! This module provides the ratatui-specific `ModalFrame` impl.

mod preview;
pub mod types;

pub use types::{
    SubparDuplicateAction, SubparDuplicateData, SubparDuplicateModalData, SubparDuplicateState,
};
