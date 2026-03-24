//! Config Editor View
//!
//! Shared state, types, and build logic for the config editor.
//! Rendering is backend-specific (mm-tui for terminal, mm-web for HTML).

pub mod build;
pub mod state;
pub mod types;

pub use state::{
    CollectionPosition, ConfigEditorAction, ConfigEditorState, CycleDirection, EditorButton,
    EditorButtonCtx, EditorFocus,
};
pub use types::{ConfigField, ConfigGroup, ConfigValue, FieldSource};
