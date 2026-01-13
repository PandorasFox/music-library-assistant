//! Operations Module
//!
//! Workflow operations invoked by the UI at the librarian's request.
//! These modules handle the core actions: scanning, reporting, deploying, and change execution.

pub mod changes;
pub mod dedup;
pub mod deploy;
pub mod operation;
pub mod progress;
pub mod reports;
pub mod scanner;

// Re-export progress types for convenience
pub use progress::ScanMessage;
