//! Computation System - Phase-stratified background processing
//!
//! See [`docs/COMPUTATION_REFERENCE.md`](../../../docs/COMPUTATION_REFERENCE.md) for the
//! canonical reference of computation phases, signal matrices, and spawn relationships.
//!
//! **Any changes to computation behavior must be reflected in that document.**
//!
//! ## Overview
//!
//! Computations are derived facts computed from corpus/index state. Unlike Mutations,
//! they do not alter state - they only emit signals. This distinction is important
//! because:
//!
//! - **Mutations** require a `ConfirmationGesture` to stage (user-led decision context)
//! - **Computations** can be queued without a witness (config-driven, automatic)
//!
//! ## Phase Stratification
//!
//! Computations are organized into three phases with compile-time enforced boundaries:
//!
//! - **Observation** (`observation/`) - Corpus observation (WalkCorpus, ScanCorpusDirectory, etc.)
//! - **Derivation** (`derivation/`) - First-level derivations (DeriveDirectorySignals, etc.)
//! - **Analysis** (`analysis/`) - Full-corpus analysis (DetectFingerprintDuplicates, etc.)
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
//! - `observation/` - Observation phase computations
//! - `derivation/` - Derivation phase computations
//! - `analysis/` - Analysis phase computations

// Module declarations
mod types;
pub mod traits;
mod stats;
pub(crate) mod helpers;
pub mod observation;
pub mod derivation;
pub mod analysis;

// Public re-exports
pub use types::ComputationWitness;
pub use stats::{close_thread_local_connection, get_thread_stats, with_read_only_db, ThreadStats};

// Internal imports for execute functions
use std::collections::HashMap;
use crate::config;
use stats::{ensure_thread_id, record_task_stats};

// ============================================================================
// Shared Constants
// ============================================================================

/// Computation types that use per-inode spawning and need dirty tracking.
/// Other computations use bulk SQL queries and don't need this optimization.
///
/// Used by:
/// - Post-execution pipeline (dirty marking after mutations)
/// - Migration seeding (re-seed dirty inodes after schema changes)
pub const PER_INODE_COMPUTATIONS: &[&str] = &["compound_tag", "shit_format"];

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
    Observation(observation::Computation),
    Derivation(derivation::Computation),
    Analysis(analysis::Computation),
}

impl Computation {
    /// Get a human-readable label for this computation.
    pub fn label(&self) -> &'static str {
        match self {
            Computation::Observation(c) => c.label(),
            Computation::Derivation(c) => c.label(),
            Computation::Analysis(c) => c.label(),
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
    /// Observation computations to spawn
    pub spawn_observation: Vec<observation::Computation>,
    /// Derivation computations to spawn
    pub spawn_derivation: Vec<derivation::Computation>,
    /// Analysis computations to spawn
    pub spawn_analysis: Vec<analysis::Computation>,
    /// Corpus inodes observed on disk during this computation (inode → relative path).
    pub observed_corpus_inodes: HashMap<i64, String>,
    /// Inbox inodes observed on disk during this computation (inode → relative path).
    pub observed_inbox_inodes: HashMap<i64, String>,
    /// Library files observed on disk during ScanLibraryDirectory.
    pub observed_library_files: Vec<derivation::ObservedLibraryFile>,
}

impl ComputationResult {
    fn from_observation(result: observation::Result) -> Self {
        Self {
            success: result.success,
            error: result.error,
            duration_ms: result.duration_ms,
            spawn_observation: result.spawn,
            spawn_derivation: Vec::new(),
            spawn_analysis: Vec::new(),
            observed_corpus_inodes: result.observed_corpus_inodes,
            observed_inbox_inodes: result.observed_inbox_inodes,
            observed_library_files: Vec::new(),
        }
    }

    fn from_derivation(result: derivation::Result) -> Self {
        Self {
            success: result.success,
            error: result.error,
            duration_ms: result.duration_ms,
            spawn_observation: Vec::new(),
            spawn_derivation: result.spawn,
            spawn_analysis: Vec::new(),
            observed_corpus_inodes: HashMap::new(),
            observed_inbox_inodes: HashMap::new(),
            observed_library_files: result.observed_library_files,
        }
    }

    fn from_analysis(result: analysis::Result) -> Self {
        Self {
            success: result.success,
            error: result.error,
            duration_ms: result.duration_ms,
            spawn_observation: Vec::new(),
            spawn_derivation: Vec::new(),
            spawn_analysis: result.spawn,
            observed_corpus_inodes: HashMap::new(),
            observed_inbox_inodes: HashMap::new(),
            observed_library_files: Vec::new(),
        }
    }

    /// Get all spawned computations as unified Computation enums.
    pub fn all_spawned(&self) -> Vec<Computation> {
        let mut result = Vec::new();
        for c in &self.spawn_observation {
            result.push(Computation::Observation(c.clone()));
        }
        for c in &self.spawn_derivation {
            result.push(Computation::Derivation(c.clone()));
        }
        for c in &self.spawn_analysis {
            result.push(Computation::Analysis(c.clone()));
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

    // Use thread-local cached READ-ONLY connection
    // IMPORTANT: All writes must go through write_thread::signal_sender()
    let db_access_start = std::time::Instant::now();

    let result = with_read_only_db(|read_only_db| {
        let db_access_ms = db_access_start.elapsed().as_millis();

        let ctx = traits::ComputationContext {
            read_db: read_only_db,
            witness: &witness,
            start,
        };

        let compute_result = match computation {
            Computation::Observation(c) => ComputationResult::from_observation(c.execute(&ctx)),
            Computation::Derivation(c) => ComputationResult::from_derivation(c.execute(&ctx)),
            Computation::Analysis(c) => ComputationResult::from_analysis(c.execute(&ctx)),
        };

        // Log timing (only on first access when connection is opened, and only if timing instrumentation enabled)
        if db_access_ms > 1 && config::is_timing_enabled() {
            crate::logging::log_perf(format!(
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
            spawn_observation: Vec::new(),
            spawn_derivation: Vec::new(),
            spawn_analysis: Vec::new(),
            observed_corpus_inodes: HashMap::new(),
            observed_inbox_inodes: HashMap::new(),
            observed_library_files: Vec::new(),
        },
    };

    // Record task completion stats
    record_task_stats(computation.label(), final_result.duration_ms);

    final_result
}
