//! Flows Module
//!
//! Workflow operations invoked by the UI at the librarian's request.
//! These modules handle the core actions: deploying, decision execution, and background tasks.

pub mod background;
pub mod changes;
pub mod daemon;
pub mod decisions;
pub mod dedup;
pub mod deploy;

// Re-export commonly used types
pub use background::{BackgroundTask, ProgressSender, TaskMessage, TaskProgress, TaskResult};
pub use daemon::{DaemonStatus, TaskDaemon};
pub use decisions::{DecisionType, PendingDecision};
