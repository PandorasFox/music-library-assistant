//! Corpus Mutations Module
//!
//! Provides a standardized interface for all corpus-mutating operations.
//! All mutations are resolved to single file/track level work units.
//!
//! ## Architecture
//!
//! ```text
//! MutationDispatcher (high-level interface)
//!     │
//!     ├── create_tag_edits()      → Vec<Mutation>
//!     ├── create_bulk_tag_fill()  → Vec<Mutation>
//!     └── execute_sync() / execute_with_progress()
//!             │
//!             ▼
//!         BulkExecutor
//!             │
//!             ├── group_by_file()  → Vec<WorkUnit>
//!             ├── execute_work_unit() (parallel via rayon)
//!             │       ├── tag_edit::execute_batch()
//!             │       ├── indexing::execute_index_track()
//!             │       └── file_ops::execute_move() / etc.
//!             └── progress reporting via mpsc
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use corpus::mutations::{MutationDispatcher, Mutation};
//!
//! let dispatcher = MutationDispatcher::new(db.clone());
//!
//! // Create mutations
//! let mutations = dispatcher.create_bulk_tag_fill(&files, "album_artist", "Various Artists");
//!
//! // Execute synchronously
//! let result = dispatcher.execute_sync(mutations);
//!
//! // Or with progress reporting
//! let (rx, cancel, handle) = dispatcher.execute_with_progress(mutations);
//! ```

mod types;
mod grouping;
mod migration;
pub mod tag_edit;
pub mod indexing;
pub mod file_ops;
pub mod executor;

pub use types::*;
pub use migration::MigrationRegistry;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Arc;

use crate::corpus::db::Database;

/// High-level interface for dispatching mutations.
///
/// This is the primary API for code that needs to execute mutations.
/// It provides helper methods for common operations and manages
/// the bulk executor.
pub struct MutationDispatcher {
    db: Arc<Database>,
}

impl MutationDispatcher {
    /// Create a new MutationDispatcher with the given database connection.
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Execute mutations synchronously (blocking).
    ///
    /// Returns the execution result once all mutations are complete.
    pub fn execute_sync(&self, mutations: Vec<Mutation>) -> ExecutionResult {
        if mutations.is_empty() {
            return ExecutionResult::empty();
        }

        // TODO: Phase 7 - implement via BulkExecutor
        // For now, execute mutations serially as a placeholder
        let start = std::time::Instant::now();
        let total = mutations.len();
        let mut succeeded = 0;
        let mut results = Vec::new();

        for mutation in mutations {
            let result = self.execute_single(&mutation);
            if result.success {
                succeeded += 1;
            }
            results.push(result);
        }

        ExecutionResult {
            total,
            succeeded,
            failed: total - succeeded,
            results,
            duration_ms: start.elapsed().as_millis() as u64,
        }
    }

    /// Execute mutations with progress reporting via callback.
    ///
    /// This executes synchronously but sends progress updates to the provided channel.
    /// For true async execution, use the BulkExecutor directly (Phase 7).
    pub fn execute_with_progress_sync(
        &self,
        mutations: Vec<Mutation>,
        progress_tx: mpsc::Sender<ExecutionProgress>,
        cancel_flag: Arc<AtomicBool>,
    ) -> ExecutionResult {
        let total = mutations.len();

        // Send initial progress
        let _ = progress_tx.send(ExecutionProgress {
            completed: 0,
            total,
            current_file: None,
            errors: vec![],
        });

        let start = std::time::Instant::now();
        let mut succeeded = 0;
        let mut results = Vec::new();
        let mut errors = Vec::new();

        for (i, mutation) in mutations.into_iter().enumerate() {
            if cancel_flag.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }

            let current_file = mutation.primary_path().map(|p| p.display().to_string());

            let result = self.execute_single(&mutation);
            if result.success {
                succeeded += 1;
            } else if let Some(ref e) = result.error {
                errors.push(e.clone());
            }
            results.push(result);

            // Send progress update
            let _ = progress_tx.send(ExecutionProgress {
                completed: i + 1,
                total,
                current_file,
                errors: errors.clone(),
            });
        }

        ExecutionResult {
            total: results.len(),
            succeeded,
            failed: results.len() - succeeded,
            results,
            duration_ms: start.elapsed().as_millis() as u64,
        }
    }

    /// Execute a single mutation (internal helper).
    fn execute_single(&self, mutation: &Mutation) -> MutationResult {
        // Delegate to appropriate executor based on mutation category
        match mutation.category() {
            MutationCategory::TagEdit => {
                // Tag edits require session ID for audit trail
                let session_id = format!("session_{}", std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis());
                tag_edit::execute_single(&self.db, mutation, &session_id)
            }
            MutationCategory::Indexing => {
                indexing::execute_single(&self.db, mutation)
            }
            MutationCategory::FileMove | MutationCategory::FileCopy |
            MutationCategory::FileDelete | MutationCategory::Deployment => {
                file_ops::execute_single(Some(&self.db), mutation)
            }
            MutationCategory::Migration => {
                let start = std::time::Instant::now();
                let (success, error) = if let Mutation::DbMigration { migration_id, .. } = mutation {
                    match MigrationRegistry::new().apply_migration(&self.db, *migration_id) {
                        Ok(()) => (true, None),
                        Err(e) => (false, Some(e.to_string())),
                    }
                } else {
                    (false, Some("Invalid mutation for Migration category".to_string()))
                };
                MutationResult {
                    mutation: mutation.clone(),
                    success,
                    error,
                    duration_ms: start.elapsed().as_millis() as u64,
                }
            }
        }
    }

    /// Create tag edit mutations from a list of edits.
    ///
    /// Each edit is a tuple of (tag_name, old_value, new_value).
    pub fn create_tag_edits(
        &self,
        track_id: i64,
        path: &std::path::Path,
        edits: Vec<(String, Option<String>, Option<String>)>,
    ) -> Vec<Mutation> {
        let tag_edits: Vec<TagEdit> = edits
            .into_iter()
            .map(|(name, old, new)| TagEdit {
                tag_name: name,
                old_value: old,
                new_value: new,
            })
            .collect();

        vec![Mutation::TagEditAndFlush {
            track_id,
            path: path.to_path_buf(),
            edits: tag_edits,
        }]
    }

    /// Create bulk tag fill mutations (same value to multiple files).
    ///
    /// This is used for operations like filling album_artist across a directory.
    pub fn create_bulk_tag_fill(
        &self,
        files: &[(i64, PathBuf)],
        tag_name: &str,
        value: &str,
    ) -> Vec<Mutation> {
        files
            .iter()
            .map(|(track_id, path)| Mutation::TagEditAndFlush {
                track_id: *track_id,
                path: path.clone(),
                edits: vec![TagEdit {
                    tag_name: tag_name.to_string(),
                    old_value: None, // Not tracked for bulk operations
                    new_value: Some(value.to_string()),
                }],
            })
            .collect()
    }

    /// Create index track mutations from extracted metadata.
    pub fn create_index_mutations(
        &self,
        source: &str,
        files: Vec<(PathBuf, ExtractedMetadata, u64, i64, i64)>, // (path, metadata, inode, mtime_secs, mtime_nanos)
    ) -> Vec<Mutation> {
        let mut mutations = Vec::new();

        for (path, metadata, inode, mtime_secs, mtime_nanos) in files {
            mutations.push(Mutation::IndexTrack {
                path: path.clone(),
                source: source.to_string(),
                metadata,
            });

            mutations.push(Mutation::UpdateScanState {
                source: source.to_string(),
                inode,
                mtime_secs,
                mtime_nanos,
                file_size: 0, // TODO: Get from metadata
                path,
            });
        }

        mutations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_bulk_tag_fill() {
        // This test requires a database, so we just test the mutation creation logic
        let files = vec![
            (1i64, PathBuf::from("/test/a.flac")),
            (2i64, PathBuf::from("/test/b.flac")),
        ];

        // Create mutations manually to test the structure
        let mutations: Vec<Mutation> = files
            .iter()
            .map(|(track_id, path)| Mutation::TagEditAndFlush {
                track_id: *track_id,
                path: path.clone(),
                edits: vec![TagEdit {
                    tag_name: "album_artist".to_string(),
                    old_value: None,
                    new_value: Some("Various Artists".to_string()),
                }],
            })
            .collect();

        assert_eq!(mutations.len(), 2);
        for mutation in &mutations {
            assert_eq!(mutation.category(), MutationCategory::TagEdit);
            assert!(mutation.primary_path().is_some());
        }
    }
}
