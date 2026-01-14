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
//! - `directory_state`: DirectoryTagEditorState for bulk directory editing
//! - `directory_input`: Input handling for directory editor
//! - `directory_render`: Rendering for directory editor

mod directory_input;
pub mod directory_render;
pub mod directory_state;
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

// Directory tag editor exports
pub use directory_state::DirectoryTagEditorState;
#[allow(unused_imports)]
pub use types::{
    AggregatedTagField, AggregatedValue, DirectoryTagEditorAction, DirectoryTagEditorFocus,
    DirectoryTagEditorModal, GatheringMessage, VariousConfirmState,
};
