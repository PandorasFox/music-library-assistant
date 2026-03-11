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
//! - **Observation** (`observation/`) - Corpus observation (ScanCorpusDirectory, VerifyMtime, etc.)
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

/// Acquire the DB write thread's signal sender, or return early with `Result::failure`.
///
/// Expands to a `let sender = ...;` binding. The caller's module must have
/// `Result` and `Computation` in scope (each phase re-exports its own).
macro_rules! require_sender {
    ($computation:expr) => {
        match $crate::db::write_thread::signal_sender() {
            Some(s) => s.clone(),
            None => {
                return Result::failure(
                    $computation,
                    "DB thread not initialized".to_string(),
                );
            }
        }
    };
}

// Module declarations
pub mod analysis;
pub mod derivation;
pub(crate) mod helpers;
pub mod observation;
mod stats;
pub mod traits;
mod types;

// Public re-exports
pub use stats::{close_thread_local_connection, with_read_only_db};
pub use types::ComputationWitness;

// Internal imports for execute functions
use std::collections::{HashMap, VecDeque};

// ============================================================================
// Pipeline Stages (multi-phase computation barriers)
// ============================================================================

/// Named stages for multi-phase computation pipelines.
///
/// Orchestrator computations (e.g., PackReleases) return `deferred_phases`
/// containing barrier-separated follow-up stages. Each stage runs only after
/// all prior work drains (in-flight tasks + db write queue).
///
/// Actual execution order is determined by VecDeque insertion order at the
/// orchestrator, not by variant declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PipelineStage {
    /// Global conflict resolution across scored entities.
    Resolve,
    /// Dependent analysis: spawn computations that read signals written by the
    /// parent computation. General-purpose "write signals → barrier → analyze"
    /// pattern (e.g., DetectFingerprintOverlaps → AnalyzeFingerprintOverlaps).
    DependentAnalysis,
}

impl PipelineStage {
    pub fn label(&self) -> &'static str {
        match self {
            PipelineStage::Resolve => "Resolving",
            PipelineStage::DependentAnalysis => "Dependent analysis",
        }
    }
}

// ============================================================================
// Shared Constants
// ============================================================================

/// Computation types that use per-inode spawning and need dirty tracking.
/// Other computations use bulk SQL queries and don't need this optimization.
///
/// Used by:
/// - Post-execution pipeline (dirty marking after mutations)
/// - Migration seeding (re-seed dirty inodes after schema changes)
pub const PER_INODE_COMPUTATIONS: &[&str] = &["compound_tag", "shit_format", "sidecar_deploy"];

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
    /// Observation computations to spawn
    pub spawn_observation: Vec<observation::Computation>,
    /// Derivation computations to spawn
    pub spawn_derivation: Vec<derivation::Computation>,
    /// Analysis computations to spawn
    pub spawn_analysis: Vec<analysis::Computation>,
    /// Barrier-separated follow-up phases. Each phase runs only after all
    /// prior work drains (in-flight tasks + db write queue empty).
    pub deferred_phases: VecDeque<(PipelineStage, Vec<Computation>)>,
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
            spawn_observation: result.spawn,
            spawn_derivation: Vec::new(),
            spawn_analysis: Vec::new(),
            deferred_phases: VecDeque::new(),
            observed_corpus_inodes: result.observed_corpus_inodes,
            observed_inbox_inodes: result.observed_inbox_inodes,
            observed_library_files: Vec::new(),
        }
    }

    fn from_derivation(result: derivation::Result) -> Self {
        Self {
            success: result.success,
            error: result.error,
            spawn_observation: Vec::new(),
            spawn_derivation: result.spawn,
            spawn_analysis: Vec::new(),
            deferred_phases: VecDeque::new(),
            observed_corpus_inodes: HashMap::new(),
            observed_inbox_inodes: HashMap::new(),
            observed_library_files: result.observed_library_files,
        }
    }

    fn from_analysis(result: analysis::Result) -> Self {
        Self {
            success: result.success,
            error: result.error,
            spawn_observation: Vec::new(),
            spawn_derivation: Vec::new(),
            spawn_analysis: result.spawn,
            deferred_phases: result.deferred_phases,
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
    // Create witness for signal operations
    let witness = ComputationWitness::new();

    // Use thread-local cached READ-ONLY connection
    // IMPORTANT: All writes must go through write_thread::signal_sender()
    let result = with_read_only_db(|read_only_db| {
        let ctx = traits::ComputationContext {
            read_db: read_only_db,
            witness: &witness,
        };

        match computation {
            Computation::Observation(c) => ComputationResult::from_observation(c.execute(&ctx)),
            Computation::Derivation(c) => ComputationResult::from_derivation(c.execute(&ctx)),
            Computation::Analysis(c) => ComputationResult::from_analysis(c.execute(&ctx)),
        }
    });

    // Handle db access failure
    match result {
        Ok(r) => r,
        Err(e) => ComputationResult {
            success: false,
            error: Some(format!("DB access failed: {}", e)),
            spawn_observation: Vec::new(),
            spawn_derivation: Vec::new(),
            spawn_analysis: Vec::new(),
            deferred_phases: VecDeque::new(),
            observed_corpus_inodes: HashMap::new(),
            observed_inbox_inodes: HashMap::new(),
            observed_library_files: Vec::new(),
        },
    }
}
