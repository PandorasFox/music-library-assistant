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
// Legacy types - kept for backwards compatibility during transition
#[allow(unused_imports)]
pub use render::{render_change_preview_modal, render_save_confirmation_modal};
pub use state::TagEditorState;
#[allow(unused_imports)]
pub use types::{
    AggregatedTagField, AggregatedValue, DuplicateGroupInfo, DuplicateGroupType, FieldEditState,
    GatheringMessage, GroupedChange, TagChange, TagEditorAction, TagEditorModal, TagField,
    VariousConfirmState,
};

// Unified tag editor exports (new transaction-based API)
pub use state::UnifiedTagEditorState;
#[allow(unused_imports)]
pub use types::{
    CoalescedTagField, FileEntry, GatheringState, GroupContext, TagEditorButton, TagEditorSource,
    UnifiedTagEditorAction, UnifiedTagEditorFocus, UnifiedTagEditorModal,
};
