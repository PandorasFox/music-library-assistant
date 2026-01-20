//! Computation System - Phase-stratified background processing
//!
//! Computations are derived facts computed from corpus/index state. Unlike Mutations,
//! they do not alter state - they only emit signals. This distinction is important
//! because:
//!
//! - **Mutations** require a `DecisionWitness` to queue (user-led decision context)
//! - **Computations** can be queued without a witness (config-driven, automatic)
//!
//! ## Phase Stratification
//!
//! Computations are organized into three phases with compile-time enforced boundaries:
//!
//! - **Asleep** (`asleep/`) - Corpus observation (WalkCorpus, ScanCorpusDirectory, etc.)
//! - **Awakening** (`awakening/`) - First-level derivations (DeriveDirectorySignals, etc.)
//! - **Awake** (`awake/`) - Full-corpus analysis (DetectFingerprintDuplicates, etc.)
//!
//! Each phase has its own `Computation` enum and `Result` type. The `Result::spawn`
//! field can ONLY contain computations from the same phase - this is enforced at
//! compile time, preventing cross-phase spawning errors.
//!
//! ## Module Organization
//!
//! - `types.rs` - ComputationWitness (shared across phases)
//! - `stats.rs` - Thread-local performance tracking
//! - `helpers.rs` - Shared utility functions
//! - `asleep/` - Asleep phase computations
//! - `awakening/` - Awakening phase computations
//! - `awake/` - Awake phase computations

// Module declarations
mod types;
mod stats;
mod helpers;
pub mod asleep;
pub mod awakening;
pub mod awake;

// Public re-exports
pub use types::ComputationWitness;
pub use stats::{get_thread_stats, ThreadStats};

// Internal imports for execute functions
use crate::config;
use stats::{ensure_thread_id, record_task_stats, with_thread_db};

// ============================================================================
// Unified Computation Enum (for daemon's queue)
// ============================================================================

/// Unified computation enum that wraps phase-specific computations.
///
/// This is used by the daemon for queuing - the daemon doesn't care about
/// phase boundaries, it just executes computations. The phase boundaries
/// are enforced at the point where computations SPAWN other computations.
#[derive(Debug, Clone)]
pub enum Computation {
    Asleep(asleep::Computation),
    Awakening(awakening::Computation),
    Awake(awake::Computation),
}

impl Computation {
    /// Get a human-readable label for this computation.
    pub fn label(&self) -> &'static str {
        match self {
            Computation::Asleep(c) => c.label(),
            Computation::Awakening(c) => c.label(),
            Computation::Awake(c) => c.label(),
        }
    }

    /// Get the primary file path affected by this computation, if any.
    pub fn primary_path(&self) -> Option<&std::path::Path> {
        match self {
            Computation::Asleep(c) => c.primary_path(),
            Computation::Awakening(c) => c.primary_path(),
            Computation::Awake(c) => c.primary_path(),
        }
    }
}

// ============================================================================
// Unified Computation Result (for daemon's result handling)
// ============================================================================

/// Unified result that wraps phase-specific results.
///
/// The `spawn_*` fields contain phase-specific spawned computations.
/// The daemon converts these to the unified `Computation` type when queuing.
#[derive(Debug)]
pub struct ComputationResult {
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
    pub label: &'static str,
    /// Asleep computations to spawn
    pub spawn_asleep: Vec<asleep::Computation>,
    /// Awakening computations to spawn
    pub spawn_awakening: Vec<awakening::Computation>,
    /// Awake computations to spawn
    pub spawn_awake: Vec<awake::Computation>,
}

impl ComputationResult {
    fn from_asleep(result: asleep::Result) -> Self {
        Self {
            success: result.success,
            error: result.error,
            duration_ms: result.duration_ms,
            label: result.computation.label(),
            spawn_asleep: result.spawn,
            spawn_awakening: Vec::new(),
            spawn_awake: Vec::new(),
        }
    }

    fn from_awakening(result: awakening::Result) -> Self {
        Self {
            success: result.success,
            error: result.error,
            duration_ms: result.duration_ms,
            label: result.computation.label(),
            spawn_asleep: Vec::new(),
            spawn_awakening: result.spawn,
            spawn_awake: Vec::new(),
        }
    }

    fn from_awake(result: awake::Result) -> Self {
        Self {
            success: result.success,
            error: result.error,
            duration_ms: result.duration_ms,
            label: result.computation.label(),
            spawn_asleep: Vec::new(),
            spawn_awakening: Vec::new(),
            spawn_awake: result.spawn,
        }
    }

    /// Get all spawned computations as unified Computation enums.
    pub fn all_spawned(&self) -> Vec<Computation> {
        let mut result = Vec::new();
        for c in &self.spawn_asleep {
            result.push(Computation::Asleep(c.clone()));
        }
        for c in &self.spawn_awakening {
            result.push(Computation::Awakening(c.clone()));
        }
        for c in &self.spawn_awake {
            result.push(Computation::Awake(c.clone()));
        }
        result
    }
}

// ============================================================================
// Execution
// ============================================================================

/// Execute a single computation.
///
/// Dispatches to the appropriate phase-specific executor based on the
/// computation variant. Uses thread-local cached read-only connection.
pub fn execute_single(computation: &Computation) -> ComputationResult {
    // Ensure this thread has an ID assigned for stats tracking
    let _thread_id = ensure_thread_id();

    let start = std::time::Instant::now();

    // Create witness for signal operations
    let witness = ComputationWitness::new();

    // Use thread-local cached connection
    let db_access_start = std::time::Instant::now();

    let result = with_thread_db(|db| {
        let db_access_ms = db_access_start.elapsed().as_millis();

        let compute_result = match computation {
            // Asleep phase computations
            Computation::Asleep(c) => {
                let result = match c {
                    asleep::Computation::WalkCorpus { root, source, paranoid } => {
                        asleep::execute_walk_corpus(db, root, source, *paranoid, start)
                    }
                    asleep::Computation::ScanCorpusDirectory { directory, source, paranoid } => {
                        asleep::execute_scan_corpus_directory(db, directory, source, *paranoid, &witness, start)
                    }
                    asleep::Computation::CompareInodes { source, disk_state, paranoid } => {
                        asleep::execute_compare_inodes(db, source, disk_state, *paranoid, &witness, start)
                    }
                    asleep::Computation::VerifyMtime { track_id, path, expected_mtime_secs, expected_mtime_nanos } => {
                        asleep::execute_verify_mtime(db, *track_id, path, *expected_mtime_secs, *expected_mtime_nanos, start)
                    }
                    asleep::Computation::VerifyTags { track_id, path } => {
                        asleep::execute_verify_tags(db, *track_id, path, start)
                    }
                };
                ComputationResult::from_asleep(result)
            }

            // Awakening phase computations
            Computation::Awakening(c) => {
                let result = match c {
                    awakening::Computation::ScheduleSecondLevelDerivations => {
                        awakening::execute_schedule_second_level_derivations(db, start)
                    }
                    awakening::Computation::DeriveDirectorySignals { directory } => {
                        awakening::execute_derive_directory_signals(db, directory, &witness, start)
                    }
                    awakening::Computation::UpdateFileSignals { path } => {
                        awakening::execute_update_file_signals(db, path, &witness, start)
                    }
                    awakening::Computation::WalkLibrary { library_root, library_name, corpus_path_prefixes } => {
                        awakening::execute_walk_library(db, library_root, library_name, corpus_path_prefixes, start)
                    }
                    awakening::Computation::ScanLibraryDirectory { directory, library_name, library_root, corpus_path_prefixes } => {
                        awakening::execute_scan_library_directory(db, directory, library_name, library_root, corpus_path_prefixes, start)
                    }
                };
                ComputationResult::from_awakening(result)
            }

            // Awake phase computations
            Computation::Awake(c) => {
                let result = match c {
                    awake::Computation::ScheduleContentAnalysis => {
                        awake::execute_schedule_content_analysis(db, start)
                    }
                    awake::Computation::DetectFingerprintDuplicates => {
                        awake::execute_detect_fingerprint_duplicates(db, &witness, start)
                    }
                    awake::Computation::DetectDuplicateInodes => {
                        awake::execute_detect_duplicate_inodes(db, &witness, start)
                    }
                    awake::Computation::DetectMissingTags => {
                        awake::execute_detect_missing_tags(db, &witness, start)
                    }
                    awake::Computation::DetectMetadataDuplicates => {
                        awake::execute_detect_metadata_duplicates(db, &witness, start)
                    }
                    awake::Computation::DetectTagCanonicalizations => {
                        awake::execute_detect_tag_canonicalizations(db, start)
                    }
                    awake::Computation::VerifyOutOfBandChanges => {
                        awake::execute_verify_out_of_band_changes(db, &witness, start)
                    }
                    awake::Computation::DetectDeployConflicts => {
                        awake::execute_detect_deploy_conflicts(db, &witness, start)
                    }
                    awake::Computation::CheckDeployConflicts { track_id } => {
                        awake::execute_check_deploy_conflicts(db, *track_id, start)
                    }
                    awake::Computation::DeriveDeployHealthSignals { library_name, library_root, corpus_path_prefixes } => {
                        awake::execute_derive_deploy_health_signals(db, library_name, library_root, corpus_path_prefixes, &witness, start)
                    }
                };
                ComputationResult::from_awake(result)
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

    // Handle db access failure
    let final_result = match result {
        Ok(r) => r,
        Err(e) => ComputationResult {
            success: false,
            error: Some(format!("DB access failed: {}", e)),
            duration_ms: start.elapsed().as_millis() as u64,
            label: computation.label(),
            spawn_asleep: Vec::new(),
            spawn_awakening: Vec::new(),
            spawn_awake: Vec::new(),
        },
    };

    // Record task completion stats
    record_task_stats(computation.label(), final_result.duration_ms);

    final_result
}
