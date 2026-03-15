//! Tag Editor UI Module
//!
//! TUI wrapper around mm-ui's `TagEditorState` + `FieldFormState`.
//!
//! ## Module Structure
//!
//! - `types`: TUI-specific type definitions (actions, modals, source context)
//! - `mutations`: TUI query orchestration (load_tag_sets_batch)
//! - `input`: Input handling (delegates field editing to FieldFormState)
//! - `render`: ratatui rendering
//! - `state`: UnifiedTagEditorState wrapping mm-ui's TagEditorState

mod input;
pub mod mutations;
mod render;
pub mod state;
pub mod types;

// Unified tag editor exports
pub use state::UnifiedTagEditorState;
pub use types::{GroupContext, TagEditorSource, UnifiedTagEditorAction};
