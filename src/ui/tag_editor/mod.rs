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
//! - `mutations`: Pure functions for change computation and mutation generation
//! - `field_editing`: Buffer operations and field manipulation
//! - `input`: Keyboard event handling
//! - `render`: All rendering methods
//! - `state`: UnifiedTagEditorState for transaction-based editing

mod field_editing;
mod input;
pub mod mutations;
mod render;
pub mod state;
pub mod types;

// Unified tag editor exports (transaction-based API)
pub use state::UnifiedTagEditorState;
pub use types::{GroupContext, TagEditorSource, UnifiedTagEditorAction};
