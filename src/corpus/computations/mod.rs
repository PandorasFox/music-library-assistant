//! Computation System - Declarative background processing
//!
//! Computations are derived facts computed from corpus/index state. Unlike Mutations,
//! they do not alter state - they only emit signals. This distinction is important
//! because:
//!
//! - **Mutations** require a `DecisionWitness` to queue (user-led decision context)
//! - **Computations** can be queued without a witness (config-driven, automatic)
//!
//! ## Module Organization
//!
//! - `types.rs` - Computation enum, ComputationResult, witness types
//! - `stats.rs` - Thread-local performance tracking
//! - `helpers.rs` - Shared utility functions
//! - `eyeballing.rs` - Corpus scanning and file verification
//! - `library_health.rs` - Library deployment health checks and second-level signals
//! - `content_analysis.rs` - Duplicate detection, tag analysis
//!
//! ## Adding New Computations
//!
//! 1. Add variant to `Computation` enum in `types.rs`
//! 2. Add executor function in appropriate module
//! 3. Add dispatch case in `execute_single()` in this file
//!
//! ## Eyeballing Computation Chain
//!
//! The eyeballing system uses computation chaining:
//!
//! ```text
//! WalkCorpus ──► CompareInodes ──► VerifyMtime ──► VerifyTags
//!                    │                   │              │
//!                    ▼                   ▼              ▼
//!             MissingFromDisk     (spawn next)   OutOfBandTagChange
//!             MissingFromIndex
//! ```
//!
//! Each computation can spawn follow-up computations via `ComputationResult::spawn`.
//!
//! ## Architecture
//!
//! ```text
//! Eyeballing (automatic)
//!     │
//!     └──► TaskDaemon.queue_computation() ───► Computation Executor
//!                                                    │
//!                                                    ├──► Signals (health_issues table)
//!                                                    └──► Spawned Computations (chained)
//! ```
//!
//! Computations never bypass the witness system because they don't alter state.
//! They only observe and record findings.

// Module declarations
mod types;
mod stats;
mod helpers;
mod eyeballing;
mod library_health;
mod content_analysis;

// Public re-exports
pub use types::{Computation, ComputationResult, ComputationWitness};
pub use stats::{get_thread_stats, ThreadStats};

// Internal imports for execute_single
use crate::config;
use stats::{ensure_thread_id, record_task_stats, with_thread_db};

/// Execute a single computation.
///
/// Uses thread-local cached read-only connection for queries.
/// Writes go through the DB thread via SignalWriteSender.
/// Creates a `ComputationWitness` for signal-altering operations.
pub fn execute_single(computation: &Computation) -> ComputationResult {
    use crate::corpus::mutations::indexing;

    // Ensure this thread has an ID assigned for stats tracking
    let _thread_id = ensure_thread_id();

    let start = std::time::Instant::now();

    // Create witness for signal operations - only valid within this execution context
    let witness = ComputationWitness::new();

    // Use thread-local cached connection - first access per thread opens, subsequent reuses
    let db_access_start = std::time::Instant::now();

    let result = with_thread_db(|db| {
        let db_access_ms = db_access_start.elapsed().as_millis();

        let compute_result = match computation {
            Computation::VerifyTags { track_id, path } => {
                // Delegate to the existing execute_verify_tags implementation
                match indexing::execute_verify_tags(db, *track_id, path) {
                    Ok(()) => ComputationResult::success(
                        computation.clone(),
                        start.elapsed().as_millis() as u64,
                        Vec::new(),
                    ),
                    Err(e) => ComputationResult::failure(
                        computation.clone(),
                        start.elapsed().as_millis() as u64,
                        e.to_string(),
                    ),
                }
            }

            Computation::WalkCorpus { root, source, paranoid } => {
                eyeballing::execute_walk_corpus(db, root, source, *paranoid, start)
            }

            Computation::ScanCorpusDirectory { directory, source, paranoid } => {
                eyeballing::execute_scan_corpus_directory(db, directory, source, *paranoid, &witness, start)
            }

            Computation::CompareInodes { source, disk_state, paranoid } => {
                eyeballing::execute_compare_inodes(db, source, disk_state, *paranoid, &witness, start)
            }

            Computation::VerifyMtime { track_id, path, expected_mtime_secs, expected_mtime_nanos } => {
                eyeballing::execute_verify_mtime(db, *track_id, path, *expected_mtime_secs, *expected_mtime_nanos, start)
            }

            Computation::ScheduleSecondLevelDerivations => {
                library_health::execute_schedule_second_level_derivations(db, start)
            }

            Computation::DeriveDirectorySignals { directory } => {
                library_health::execute_derive_directory_signals(db, directory, &witness, start)
            }

            Computation::CheckDeployConflicts { track_id } => {
                library_health::execute_check_deploy_conflicts(db, *track_id, start)
            }

            Computation::UpdateFileSignals { path } => {
                library_health::execute_update_file_signals(db, path, &witness, start)
            }

            Computation::WalkLibrary { library_root, library_name, corpus_path_prefixes } => {
                library_health::execute_walk_library(db, library_root, library_name, corpus_path_prefixes, start)
            }

            Computation::ScanLibraryDirectory { directory, library_name, library_root, corpus_path_prefixes } => {
                library_health::execute_scan_library_directory(db, directory, library_name, library_root, corpus_path_prefixes, start)
            }

            Computation::DeriveLibraryHealthSignals { library_name, library_root, corpus_path_prefixes, library_files } => {
                library_health::execute_derive_library_health_signals(db, library_name, library_root, corpus_path_prefixes, library_files, &witness, start)
            }

            // Content analysis computations
            Computation::ScheduleContentAnalysis => {
                content_analysis::execute_schedule_content_analysis(start)
            }

            Computation::DetectFingerprintDuplicates => {
                content_analysis::execute_detect_fingerprint_duplicates(db, &witness, start)
            }

            Computation::DetectDuplicateInodes => {
                content_analysis::execute_detect_duplicate_inodes(db, &witness, start)
            }

            Computation::DetectMissingTags => {
                content_analysis::execute_detect_missing_tags(db, &witness, start)
            }

            Computation::DetectMetadataDuplicates => {
                content_analysis::execute_detect_metadata_duplicates(db, &witness, start)
            }

            Computation::DetectTagCanonicalizations => {
                content_analysis::execute_detect_tag_canonicalizations(db, start)
            }

            Computation::VerifyOutOfBandChanges => {
                content_analysis::execute_verify_out_of_band_changes(db, &witness, start)
            }

            Computation::DetectDeployConflicts => {
                content_analysis::execute_detect_deploy_conflicts(db, &witness, start)
            }
        };

        // Log timing (only on first access when connection is opened)
        if db_access_ms > 1 {
            let _ = config::log_message(&format!(
                "[PERF] {} db_access={}ms (thread-local init)",
                computation.label(),
                db_access_ms
            ));
        }

        compute_result
    });

    // Handle db access failure and record stats
    let final_result = match result {
        Ok(r) => r,
        Err(e) => ComputationResult::failure(
            computation.clone(),
            start.elapsed().as_millis() as u64,
            format!("DB access failed: {}", e),
        ),
    };

    // Record task completion stats
    record_task_stats(computation.label(), final_result.duration_ms);

    final_result
}
