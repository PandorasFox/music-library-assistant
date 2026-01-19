//! Flows Module
//!
//! Workflow operations invoked by the UI at the librarian's request.
//! Primary module: deploy (library deployment planning and execution).

pub mod deploy;

// Re-export daemon types from crate root (daemon module at src/daemon.rs)
pub use crate::daemon::{
    DaemonStatus, TaskDaemon,
};

// =============================================================================
// Vestigial Stubs (to be removed during UI refactor)
// =============================================================================
//
// These types exist only to allow the codebase to compile while UI refactoring
// is in progress. All code using these types is marked for removal/rewrite.

use serde::{Deserialize, Serialize};

/// VESTIGIAL: Decision type for old decision system.
///
/// To be replaced with `corpus::mutations::Mutation` variants.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum DecisionType {
    Move,
    Delete,
    DropIndex,
    TagEdit,
    Deploy,
    Undeploy,
    Redeploy,
}

/// VESTIGIAL: Pending decision for old decision system.
///
/// To be replaced with `corpus::mutations::Mutation` queued via TaskDaemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingDecision {
    pub decision_type: DecisionType,
    pub source_path: String,
    pub target_path: Option<String>,
    pub metadata: Option<String>,
}

impl PendingDecision {
    pub fn new(decision_type: DecisionType) -> Self {
        Self {
            decision_type,
            source_path: String::new(),
            target_path: None,
            metadata: None,
        }
    }

    pub fn source(mut self, path: impl Into<String>) -> Self {
        self.source_path = path.into();
        self
    }

    pub fn target(mut self, path: impl Into<String>) -> Self {
        self.target_path = Some(path.into());
        self
    }

    pub fn with_metadata(mut self, json: serde_json::Value) -> Self {
        self.metadata = Some(json.to_string());
        self
    }

    pub fn tag_edit(path: impl Into<String>, changes: serde_json::Value) -> Self {
        Self::new(DecisionType::TagEdit)
            .source(path)
            .with_metadata(changes)
    }

    pub fn drop_index(path: impl Into<String>) -> Self {
        Self::new(DecisionType::DropIndex).source(path)
    }
}

// =============================================================================
// Background Task Stubs (vestigial)
// =============================================================================

pub mod background {
    //! VESTIGIAL: Old background task system.
    //!
    //! To be replaced with TaskDaemon computations.

    use std::time::{Duration, Instant};

    /// VESTIGIAL: Task progress stub.
    #[derive(Debug, Clone)]
    pub struct TaskProgress {
        pub completed: usize,
        pub total: usize,
        pub skipped: usize,
        pub errors: usize,
        pub bytes: Option<(u64, u64)>,
        pub current_item: Option<String>,
        pub start_time: Instant,
    }

    impl TaskProgress {
        pub fn new(total: usize) -> Self {
            Self {
                completed: 0,
                total,
                skipped: 0,
                errors: 0,
                bytes: None,
                current_item: None,
                start_time: Instant::now(),
            }
        }

        pub fn percentage(&self) -> u8 {
            if self.total == 0 {
                return 0;
            }
            let done = self.completed + self.skipped;
            ((done as f64 / self.total as f64) * 100.0).clamp(0.0, 100.0) as u8
        }
    }

    /// VESTIGIAL: Task message stub.
    #[derive(Debug, Clone)]
    pub enum TaskMessage {
        Progress(TaskProgress),
        Complete(TaskResult),
        Error(String),
        Cancelled,
    }

    /// VESTIGIAL: Task result stub.
    #[derive(Debug, Clone)]
    pub struct TaskResult {
        pub succeeded: usize,
        pub skipped: usize,
        pub failed: usize,
        pub duration: Duration,
        pub bytes_processed: Option<u64>,
        pub errors: Vec<String>,
    }

    /// VESTIGIAL: Background task stub.
    #[derive(Debug)]
    pub struct BackgroundTask {
        pub id: String,
        pub label: String,
        pub progress: TaskProgress,
    }

    /// VESTIGIAL: Poll tasks stub (no-op).
    pub fn poll_tasks(_tasks: &mut Vec<BackgroundTask>) -> Vec<(String, String, TaskResult)> {
        Vec::new()
    }

    /// VESTIGIAL: Log task summary stub (no-op).
    pub fn log_task_summary(_label: &str, _result: &TaskResult) {
        // No-op - operations log removed
    }
}

// =============================================================================
// Changes Stub (vestigial)
// =============================================================================

pub mod changes {
    //! VESTIGIAL: Old decision execution system.
    //!
    //! To be replaced with TaskDaemon mutation execution.

    use super::PendingDecision;
    use crate::corpus::db::Database;
    use anyhow::Result;

    /// VESTIGIAL: Execution report stub.
    #[derive(Debug, Default)]
    pub struct ExecutionReport {
        pub succeeded: usize,
        pub failed: usize,
        pub skipped: usize,
        pub errors: Vec<String>,
    }

    /// VESTIGIAL: Execute decisions stub.
    ///
    /// Always returns an error - this system has been removed.
    /// All code paths using this are marked for refactoring.
    pub fn execute_decisions(
        _db: &Database,
        _decisions: &[PendingDecision],
        _dry_run: bool,
    ) -> Result<ExecutionReport> {
        anyhow::bail!(
            "execute_decisions is vestigial - use TaskDaemon.queue_mutations() instead"
        )
    }
}
