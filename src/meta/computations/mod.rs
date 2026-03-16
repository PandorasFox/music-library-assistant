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
//! - **Observation** (`observation/`) - Per-file verification (VerifyTags, VerifyAudio)
//! - **Derivation** (`derivation/`) - First-level derivations (DeriveCorpusSignals, DeriveInboxSignals, etc.)
//! - **Analysis** (`analysis/`) - Full-corpus analysis (DetectFingerprintOverlaps, etc.)
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

/// Extract config from the computation context's snapshot, or return early with `Result::failure`.
///
/// Returns `&MagicConfig` from `ctx.snapshot.config`. If config is not available
/// (pre-setup state), returns a failure result. Callers must have `Result` and
/// `Computation` in scope.
macro_rules! require_config {
    ($ctx:expr, $computation:expr) => {
        match $ctx.snapshot.config.as_deref() {
            Some(c) => c,
            None => {
                return Result::failure(
                    $computation,
                    "Config not available (pre-setup)".to_string(),
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
use std::collections::VecDeque;

// Import execution extension traits
use derivation::DerivationExecute;
use observation::ObservationExecute;

// Re-export PipelineStage from mm-meta
pub use mm_meta::computations::PipelineStage;

// ============================================================================
// Shared Constants
// ============================================================================

/// Computation types that use per-inode spawning and need dirty tracking.
/// Other computations use bulk SQL queries and don't need this optimization.
///
/// Used by:
/// - Post-execution pipeline (dirty marking after mutations)
/// - Migration seeding (re-seed dirty inodes after schema changes)
pub const PER_INODE_COMPUTATIONS: &[&str] = &["compound_tag", "lossless_remux", "sidecar_deploy"];

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
pub fn execute_single(
    computation: &Computation,
    snapshot: &crate::witch::types::HadesSnapshot,
) -> ComputationResult {
    // Create witness for signal operations
    let witness = ComputationWitness::new();

    // Use thread-local cached READ-ONLY connection
    // IMPORTANT: All writes must go through write_thread::signal_sender()
    let result = with_read_only_db(|read_only_db| {
        let ctx = traits::ComputationContext {
            read_db: read_only_db,
            witness: &witness,
            snapshot,
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
        },
    }
}
