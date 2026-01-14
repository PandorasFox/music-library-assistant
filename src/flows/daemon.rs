//! Task Daemon - Simple parallel task queue with status polling.

use std::collections::VecDeque;

use parallel_worker::{CancelableWorker, WorkerInit, WorkerMethods};

use crate::corpus::mutations::Mutation;

/// Status returned from poll().
#[derive(Debug, Clone, Default)]
pub struct DaemonStatus {
    pub pending: usize,
    pub completed: usize,
    pub failed: usize,
    pub total_processed: usize,
    pub recent_errors: Vec<String>,
}

/// Result of executing a single task.
#[derive(Debug)]
pub struct TaskResult {
    pub success: bool,
    pub error: Option<String>,
}

/// Simple parallel task queue.
pub struct TaskDaemon {
    worker: CancelableWorker<Mutation, TaskResult>,
    total_processed: usize,
    total_failed: usize,
    recent_errors: VecDeque<String>,
}

impl TaskDaemon {
    pub fn new() -> Self {
        let worker = CancelableWorker::new(|mutation: Mutation, _state| {
            Some(execute_mutation(mutation))
        });

        Self {
            worker,
            total_processed: 0,
            total_failed: 0,
            recent_errors: VecDeque::with_capacity(5),
        }
    }

    /// Queue a single mutation.
    pub fn queue(&mut self, mutation: Mutation) {
        self.worker.add_task(mutation);
    }

    /// Queue multiple mutations.
    pub fn queue_all(&mut self, mutations: impl IntoIterator<Item = Mutation>) {
        self.worker.add_tasks(mutations);
    }

    /// Poll for completed tasks. Call each UI tick.
    pub fn poll(&mut self) -> DaemonStatus {
        let mut completed = 0;
        let mut failed = 0;

        // Drain completed results
        while let Some(result) = self.worker.get() {
            self.total_processed += 1;
            if result.success {
                completed += 1;
            } else {
                failed += 1;
                self.total_failed += 1;
                if let Some(err) = result.error {
                    if self.recent_errors.len() >= 5 {
                        self.recent_errors.pop_front();
                    }
                    self.recent_errors.push_back(err);
                }
            }
        }

        DaemonStatus {
            pending: self.worker.pending_tasks(),
            completed,
            failed,
            total_processed: self.total_processed,
            recent_errors: self.recent_errors.iter().cloned().collect(),
        }
    }

    /// Check if there's pending work.
    pub fn has_pending(&self) -> bool {
        self.worker.pending_tasks() > 0
    }

    /// Cancel all pending tasks.
    pub fn cancel(&mut self) {
        self.worker.cancel_tasks();
    }
}

impl Default for TaskDaemon {
    fn default() -> Self {
        Self::new()
    }
}

/// Execute a single mutation. Opens DB connection as needed.
fn execute_mutation(mutation: Mutation) -> TaskResult {
    use crate::config;
    use crate::corpus::db::Database;
    use crate::corpus::mutations::{file_ops, tag_edit, indexing, MutationCategory};

    // Open database
    let db = match config::get_db_path().and_then(|p| Database::open(&p).map_err(|e| e.into())) {
        Ok(db) => db,
        Err(e) => return TaskResult {
            success: false,
            error: Some(format!("DB error: {}", e)),
        },
    };

    let session_id = "daemon";

    let (success, error) = match mutation.category() {
        MutationCategory::TagEdit => {
            let r = tag_edit::execute_single(&db, &mutation, session_id);
            (r.success, r.error)
        }
        MutationCategory::FileMove | MutationCategory::FileCopy |
        MutationCategory::FileDelete | MutationCategory::Deployment => {
            let r = file_ops::execute_single(Some(&db), &mutation);
            (r.success, r.error)
        }
        MutationCategory::Indexing => {
            let r = indexing::execute_single(&db, &mutation);
            (r.success, r.error)
        }
        MutationCategory::Migration => {
            (false, Some("Migrations not supported in daemon".to_string()))
        }
    };

    TaskResult { success, error }
}
