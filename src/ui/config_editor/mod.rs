//! Config Editor View
//!
//! Full-screen view for browsing and editing all Opinion groups.
//! Reachable via the lateral view ring (Tab/Shift-Tab).

pub mod build;
pub mod render;
pub mod state;
pub mod types;

pub use state::{ConfigEditorAction, ConfigEditorState};
