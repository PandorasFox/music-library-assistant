//! Config Editor View
//!
//! Full-screen view for browsing and editing all Opinion groups.
//! Reachable via the lateral view ring (Tab/Shift-Tab).
//!
//! State, types, and build logic live in mm-ui for TUI/web parity.
//! This module provides TUI-specific rendering only.

pub mod render;

// Re-export shared types from mm-ui
pub use mm_ui::config_editor::{
    build, CollectionPosition, ConfigEditorAction, ConfigEditorState, ConfigField, ConfigGroup,
    ConfigValue, CycleDirection, EditorButton, EditorButtonCtx, EditorFocus, FieldSource,
};
