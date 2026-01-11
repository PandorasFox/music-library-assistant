//! Tag Editor UI Module
//!
//! Provides a multi-track metadata editing workflow with:
//! - 3-column layout: track list, tag editor, search panel
//! - Support for duplicate resolution workflow
//! - Fill-to-all functionality for shared fields
//! - Change preview before saving
//!
//! ## Module Structure
//!
//! - `types`: Core type definitions (TagField, TagChange, etc.)
//! - `state`: TagEditorState and conversion functions
//! - `input`: Keyboard input handling
//! - `render`: UI rendering functions

mod input;
mod render;
pub mod state;
pub mod types;

// Re-export public types
// Some types are not yet used externally but are part of the public API
#[allow(unused_imports)]
pub use render::{render_change_preview_modal, render_save_confirmation_modal};
pub use state::TagEditorState;
#[allow(unused_imports)]
pub use types::{
    DuplicateGroupInfo, DuplicateGroupType, FieldEditState, GroupedChange, TagChange,
    TagEditorAction, TagEditorModal, TagField,
};
