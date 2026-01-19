//! Tag Editor UI Module
//!
//! Provides multi-track metadata editing workflows with:
//! - 3-column layout: context list, tag editor, action panel
//! - Support for single-file and bulk editing contexts
//! - Transaction-based save with review before commit
//! - Fill-to-all functionality for shared fields
//! - Change preview before saving
//!
//! ## Module Structure
//!
//! - `types`: Core type definitions (TagField, TagChange, etc.)
//! - `state`: TagEditorState (legacy) and UnifiedTagEditorState (new)
//! - `input`: Keyboard input handling
//! - `render`: UI rendering functions

mod input;
mod render;
pub mod state;
pub mod types;

// Re-export public types
pub use render::{render_change_preview_modal, render_save_confirmation_modal};
pub use state::TagEditorState;
pub use types::{
    DuplicateGroupInfo, TagEditorAction, TagEditorModal,
};

// Unified tag editor exports (new transaction-based API)
pub use state::UnifiedTagEditorState;
pub use types::{
    GroupContext, TagEditorSource,
    TransactionReviewButton, UnifiedTagEditorAction, UnifiedTagEditorModal,
};
