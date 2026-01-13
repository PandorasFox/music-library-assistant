//! Corpus Browser Module
//!
//! A two-pane browser for viewing and editing corpus files:
//! - Left pane (2/3): Directory/file tree with expand/collapse
//! - Right pane (1/3): Metadata preview for selected file
//!
//! Features:
//! - Navigate with arrow keys (or h/j/k/l)
//! - Expand/collapse directories with Left/Right
//! - Enter on directory: Open tag editor for all tracks in subtree
//! - Enter on file: Open tag editor for single file
//! - Preview shows technical info (bitrate, duration, sample rate)
//!   and tag key/values for files

mod input;
mod render;
mod state;
mod types;

pub use state::CorpusBrowserState;
pub use types::{CorpusBrowserAction, CorpusBrowserConfig};
