//! Derivation-phase computations: First-level derivations.
//!
//! These computations run during the Derivation phase after Observation completes.
//! They derive first-level signals by comparing corpus observations (FileInCorpus)
//! against the index to produce: UnindexedFile, MissingFile, HealthyFile.
//!
//! ## Phase Boundary Enforcement
//!
//! The `Result` type's `spawn` field can ONLY contain `derivation::Computation`.
//! This is enforced at compile time - attempting to spawn an Observation or
//! Analysis computation from a Derivation executor will fail to compile.
//!
//! ## Computations
//!
//! - `ScheduleSecondLevelDerivations` - Orchestrator: spawns global corpus derivation
//! - `DeriveCorpusSignals` - Global inode comparison for corpus signals
//! - `UpdateCorpusFileSignals` - Lightweight per-file corpus signal update (post-mutation)
//! - `UpdateLibraryFileSignals` - Lightweight per-file library signal update (post-mutation)
//! - `WalkLibrary` - Enumerate library directories for scanning
//! - `ScanLibraryDirectory` - Scan library directory, store results in DB

mod executors;

pub use executors::*;

// Re-export types from mm-meta
pub use mm_meta::computations::derivation::{Computation, ObservedLibraryFile};

/// Extension trait for server-side execution dispatch on derivation computations.
pub trait DerivationExecute {
    fn execute(&self, ctx: &super::traits::ComputationContext) -> Result;
}

impl DerivationExecute for Computation {
    fn execute(&self, ctx: &super::traits::ComputationContext) -> Result {
        match self {
            Computation::ScheduleSecondLevelDerivations => {
                execute_schedule_second_level_derivations(ctx.read_db, ctx.witness)
            }
            Computation::DeriveCorpusSignals { observed_inodes } => execute_derive_corpus_signals(
                ctx.read_db,
                observed_inodes.clone(),
                ctx.witness,
            ),
            Computation::UpdateCorpusFileSignals { path } => {
                execute_update_corpus_file_signals(ctx.read_db, path, ctx.witness)
            }
            Computation::UpdateLibraryFileSignals { path } => {
                execute_update_library_file_signals(ctx.read_db, path, ctx.witness)
            }
            Computation::ReconcileLibraryFiles { observed_files } => {
                execute_reconcile_library_files(ctx.read_db, observed_files, ctx.witness)
            }
            Computation::UpdateDeploySignals {
                corpus_path,
                library_path,
            } => execute_update_deploy_signals(
                ctx.read_db,
                corpus_path,
                library_path,
                ctx.witness,
            ),
            Computation::StashAndReplaceSidecars { replacements } => {
                execute_stash_and_replace_sidecars(ctx.snapshot, replacements, ctx.witness)
            }
        }
    }
}

// ============================================================================
// Derivation Result
// ============================================================================

/// Result of executing a Derivation-phase computation.
///
/// The `spawn` field can ONLY contain `derivation::Computation`. This is the
/// compile-time enforcement mechanism for phase boundaries.
#[derive(Debug)]
pub struct Result {
    pub _computation: Computation,
    pub success: bool,
    pub error: Option<String>,
    /// Follow-up computations - ONLY Derivation computations allowed.
    pub spawn: Vec<Computation>,
}

impl Result {
    pub fn success(computation: Computation, spawn: Vec<Computation>) -> Self {
        Self {
            _computation: computation,
            success: true,
            error: None,
            spawn,
        }
    }

    pub fn failure(computation: Computation, error: String) -> Self {
        Self {
            _computation: computation,
            success: false,
            error: Some(error),
            spawn: Vec::new(),
        }
    }
}
