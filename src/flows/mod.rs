//! Flows Module
//!
//! Workflow operations invoked by the UI at the librarian's request.
//! These modules handle the core actions: deploying, decision execution, and background tasks.

pub mod background;
pub mod changes;
pub mod decisions;
pub mod deploy;

// Re-export commonly used types
pub use background::{BackgroundTask, ProgressSender, TaskMessage, TaskProgress, TaskResult};
pub use decisions::{DecisionType, PendingDecision};

// Re-export daemon types from crate root (daemon module moved to src/daemon.rs)
pub use crate::daemon::{CompletedSession, DaemonStatus, TaskDaemon, TaskLabel, DaemonState, confirm_decision, DecisionWitness};
