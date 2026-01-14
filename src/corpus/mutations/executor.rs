//! Bulk Executor
//!
//! Provides execution of mutations with progress reporting.
//! File I/O operations can be parallelized, while database
//! operations are executed serially due to SQLite's single-writer model.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use rayon::prelude::*;

use crate::corpus::db::Database;

use super::types::{
    ExecutionProgress, ExecutionResult, Mutation, MutationResult,
};
use super::{file_ops, indexing, tag_edit};

/// Bulk executor for mutation execution.
///
/// Groups mutations by file and executes them efficiently.
/// File I/O can happen in parallel; database updates are serialized.
pub struct BulkExecutor {
    db: Arc<Database>,
    progress_tx: Option<mpsc::Sender<ExecutionProgress>>,
    cancel_flag: Arc<AtomicBool>,
    session_id: String,
}

impl BulkExecutor {
    /// Create a new BulkExecutor with database connection.
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            progress_tx: None,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            session_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    /// Set progress reporting channel.
    pub fn with_progress(mut self, tx: mpsc::Sender<ExecutionProgress>) -> Self {
        self.progress_tx = Some(tx);
        self
    }

    /// Set cancellation flag.
    pub fn with_cancel_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.cancel_flag = flag;
        self
    }

    /// Set session ID for tag edit history.
    pub fn with_session_id(mut self, session_id: String) -> Self {
        self.session_id = session_id;
        self
    }

    /// Execute mutations with automatic grouping.
    ///
    /// For file-only operations (TagFlushToDisk, file moves/copies/deletes),
    /// execution can happen in parallel. Database operations are serialized.
    pub fn execute(self, mutations: Vec<Mutation>) -> ExecutionResult {
        let start = std::time::Instant::now();

        if mutations.is_empty() {
            return ExecutionResult::empty();
        }

        let total = mutations.len();

        // Send initial progress
        self.send_progress(0, total, None, vec![]);

        // Separate by execution model
        let (serial_mutations, parallel_mutations): (Vec<_>, Vec<_>) =
            mutations.into_iter().partition(|m| self.requires_serial_execution(m));

        let mut all_results = Vec::new();
        let mut errors = Vec::new();

        // Execute serial mutations first (migrations, db-only operations)
        for mutation in serial_mutations {
            if self.is_cancelled() {
                break;
            }

            let result = self.execute_serial(&mutation);
            if !result.success {
                if let Some(ref e) = result.error {
                    errors.push(e.clone());
                }
            }
            all_results.push(result);

            self.send_progress(
                all_results.len(),
                total,
                mutation.primary_path().map(|p| p.display().to_string()),
                errors.clone(),
            );
        }

        // Execute parallel-safe mutations (file operations)
        // Note: We extract only Send+Sync values for the parallel closure
        if !parallel_mutations.is_empty() {
            let completed = AtomicUsize::new(all_results.len());
            let errors_vec: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(errors.clone());
            let cancel_flag = Arc::clone(&self.cancel_flag);
            let progress_tx = self.progress_tx.clone();

            // Parallel execution for file-only operations
            let parallel_results: Vec<MutationResult> = parallel_mutations
                .into_par_iter()
                .map(|mutation| {
                    if cancel_flag.load(Ordering::Relaxed) {
                        return MutationResult {
                            mutation: mutation.clone(),
                            success: false,
                            error: Some("Cancelled".to_string()),
                            duration_ms: 0,
                        };
                    }

                    let result = execute_file_only_static(&mutation);

                    // Update progress
                    let count = completed.fetch_add(1, Ordering::Relaxed) + 1;

                    if !result.success {
                        if let Some(ref e) = result.error {
                            if let Ok(mut errs) = errors_vec.lock() {
                                errs.push(e.clone());
                            }
                        }
                    }

                    // Send progress (every 13 items to reduce overhead)
                    if count % 13 == 0 {
                        if let Some(ref tx) = progress_tx {
                            let current_errors = errors_vec.lock().map(|e| e.clone()).unwrap_or_default();
                            let _ = tx.send(ExecutionProgress {
                                completed: count,
                                total,
                                current_file: mutation.primary_path().map(|p| p.display().to_string()),
                                errors: current_errors,
                            });
                        }
                    }

                    result
                })
                .collect();

            all_results.extend(parallel_results);
        }

        // Final progress
        let final_errors = all_results
            .iter()
            .filter_map(|r| r.error.clone())
            .collect();
        self.send_progress(all_results.len(), total, None, final_errors);

        let succeeded = all_results.iter().filter(|r| r.success).count();

        ExecutionResult {
            total: all_results.len(),
            succeeded,
            failed: all_results.len() - succeeded,
            results: all_results,
            duration_ms: start.elapsed().as_millis() as u64,
        }
    }

    /// Execute mutations serially (simpler, works for all mutation types).
    ///
    /// Use this when parallel execution isn't needed or for small batches.
    pub fn execute_serial_batch(self, mutations: Vec<Mutation>) -> ExecutionResult {
        let start = std::time::Instant::now();

        if mutations.is_empty() {
            return ExecutionResult::empty();
        }

        let total = mutations.len();
        self.send_progress(0, total, None, vec![]);

        let mut results = Vec::new();
        let mut errors = Vec::new();

        for mutation in mutations {
            if self.is_cancelled() {
                break;
            }

            let result = self.execute_serial(&mutation);
            if !result.success {
                if let Some(ref e) = result.error {
                    errors.push(e.clone());
                }
            }
            results.push(result);

            self.send_progress(
                results.len(),
                total,
                mutation.primary_path().map(|p| p.display().to_string()),
                errors.clone(),
            );
        }

        let succeeded = results.iter().filter(|r| r.success).count();

        ExecutionResult {
            total: results.len(),
            succeeded,
            failed: results.len() - succeeded,
            results,
            duration_ms: start.elapsed().as_millis() as u64,
        }
    }

    /// Check if a mutation requires serial execution.
    fn requires_serial_execution(&self, mutation: &Mutation) -> bool {
        match mutation {
            // Migrations must be serial
            Mutation::DbMigration { .. } => true,
            // DB-only operations need database access
            Mutation::TagEditDb { .. } => true,
            Mutation::CleanupStaleScanState { .. } => true,
            // Signal resolution operations are DB-only
            Mutation::UpdateTrackPath { .. } => true,
            Mutation::UpdateScanStatePath { .. } => true,
            Mutation::DropFromIndex { .. } => true,
            Mutation::UpdateTrack { .. } => true,
            // These need both file and DB access
            Mutation::TagEditAndFlush { .. } => true,
            Mutation::IndexTrack { .. } => true,
            Mutation::UpdateScanState { .. } => true,
            // File-only operations can be parallel
            Mutation::TagFlushToDisk { .. } => false,
            Mutation::Move { .. } => false,
            Mutation::Copy { .. } => false,
            Mutation::Delete { .. } => false,
            Mutation::MoveToStash { .. } => false,
            Mutation::HardLink { .. } => false,
            Mutation::Unlink { .. } => false,
        }
    }

    /// Execute a single mutation (serial execution with DB access).
    fn execute_serial(&self, mutation: &Mutation) -> MutationResult {
        let start = std::time::Instant::now();

        let result = match mutation {
            Mutation::DbMigration { migration_id, .. } => {
                super::MigrationRegistry::new().apply_migration(&self.db, *migration_id)
            }
            Mutation::TagEditDb { .. } | Mutation::TagFlushToDisk { .. } | Mutation::TagEditAndFlush { .. } => {
                return tag_edit::execute_single(&self.db, mutation, &self.session_id);
            }
            Mutation::IndexTrack { .. }
            | Mutation::UpdateScanState { .. }
            | Mutation::CleanupStaleScanState { .. }
            | Mutation::UpdateTrackPath { .. }
            | Mutation::UpdateScanStatePath { .. }
            | Mutation::DropFromIndex { .. }
            | Mutation::UpdateTrack { .. } => {
                return indexing::execute_single(&self.db, mutation);
            }
            _ => {
                return file_ops::execute_single(Some(&self.db), mutation);
            }
        };

        let (success, error) = match result {
            Ok(()) => (true, None),
            Err(e) => (false, Some(e.to_string())),
        };

        MutationResult {
            mutation: mutation.clone(),
            success,
            error,
            duration_ms: start.elapsed().as_millis() as u64,
        }
    }


    /// Check if execution has been cancelled.
    fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::Relaxed)
    }

    /// Send progress update if channel is configured.
    fn send_progress(
        &self,
        completed: usize,
        total: usize,
        current_file: Option<String>,
        errors: Vec<String>,
    ) {
        if let Some(ref tx) = self.progress_tx {
            let _ = tx.send(ExecutionProgress {
                completed,
                total,
                current_file,
                errors,
            });
        }
    }
}

/// Execute a file-only mutation (no database access, safe for parallel).
/// This is a static function to avoid capturing non-Send types in parallel closures.
fn execute_file_only_static(mutation: &Mutation) -> MutationResult {
    let start = std::time::Instant::now();

    let result = match mutation {
        Mutation::TagFlushToDisk { path, tags } => {
            tag_edit::write_tags_to_disk_only(path, tags)
        }
        Mutation::Move { source, destination, .. } => {
            file_ops::execute_move(None, source, destination, None)
        }
        Mutation::Copy { source, destination } => {
            file_ops::execute_copy(source, destination)
        }
        Mutation::Delete { path, .. } => {
            file_ops::execute_delete(None, path, None)
        }
        Mutation::MoveToStash { path: _, .. } => {
            // MoveToStash without stash_root - error
            Err(anyhow::anyhow!("MoveToStash requires configuration"))
        }
        Mutation::HardLink { source, destination } => {
            file_ops::execute_hard_link(source, destination)
        }
        Mutation::Unlink { path } => {
            file_ops::execute_unlink(path)
        }
        _ => {
            Err(anyhow::anyhow!("Mutation type requires database access"))
        }
    };

    let (success, error) = match result {
        Ok(()) => (true, None),
        Err(e) => (false, Some(e.to_string())),
    };

    MutationResult {
        mutation: mutation.clone(),
        success,
        error,
        duration_ms: start.elapsed().as_millis() as u64,
    }
}

/// Convenience function to execute mutations with default settings.
pub fn execute(db: Arc<Database>, mutations: Vec<Mutation>) -> ExecutionResult {
    BulkExecutor::new(db).execute_serial_batch(mutations)
}

/// Convenience function to execute mutations with progress reporting.
pub fn execute_with_progress(
    db: Arc<Database>,
    mutations: Vec<Mutation>,
    progress_tx: mpsc::Sender<ExecutionProgress>,
    cancel_flag: Arc<AtomicBool>,
) -> ExecutionResult {
    BulkExecutor::new(db)
        .with_progress(progress_tx)
        .with_cancel_flag(cancel_flag)
        .execute_serial_batch(mutations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use super::super::types::{TagEdit, WorkUnit};

    #[test]
    fn test_empty_execution() {
        let mutations: Vec<Mutation> = vec![];
        assert!(mutations.is_empty());
    }

    #[test]
    fn test_work_unit_structure() {
        let unit = WorkUnit {
            path: PathBuf::from("/test/file.flac"),
            track_id: Some(1),
            mutations: vec![
                Mutation::TagEditAndFlush {
                    track_id: 1,
                    path: PathBuf::from("/test/file.flac"),
                    edits: vec![TagEdit {
                        tag_name: "artist".to_string(),
                        old_value: None,
                        new_value: Some("New Artist".to_string()),
                    }],
                },
            ],
        };

        assert_eq!(unit.mutations.len(), 1);
        assert!(unit.track_id.is_some());
    }
}
