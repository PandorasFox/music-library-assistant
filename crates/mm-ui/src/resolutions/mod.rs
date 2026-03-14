//! Resolution modal types: button enums, data wrappers, and action types.
//!
//! Each submodule defines the backend-agnostic types for one resolution route:
//! - A `ResolutionData` wrapper around the mm-meta wire type
//! - A `ModalButtons` enum for the operator's choices
//! - An action enum produced by button confirmation
//! - A concrete state type alias: `ResolutionState<Data, Button>`
//!
//! Rendering stays in the client crates (mm-tui, mm-web).

pub mod corrupt_file;
pub mod external_match;
pub mod inbox_corpus_match;
pub mod missing_directory;
pub mod missing_file;
pub mod moved_file;
pub mod oob_sync;
pub mod shit_format;
pub mod subpar_duplicate;
