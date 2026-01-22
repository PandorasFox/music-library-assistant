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
//! - `state`: UnifiedTagEditorState for transaction-based editing

pub mod state;
pub mod types;

// Unified tag editor exports (transaction-based API)
pub use state::UnifiedTagEditorState;
pub use types::{
    GroupContext, TagEditorSource,
    UnifiedTagEditorAction, UnifiedTagEditorModal,
};
