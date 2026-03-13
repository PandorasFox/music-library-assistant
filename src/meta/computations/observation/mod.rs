//! Observation-phase computations: Per-file verification.
//!
//! These computations run during steady-state (watcher-triggered) or startup
//! (force-check). They verify individual files against indexed state.
//!
//! ## Phase Boundary Enforcement
//!
//! The `Result` type's `spawn` field can ONLY contain `observation::Computation`.
//! This is enforced at compile time - attempting to spawn a Derivation or
//! Analysis computation from an Observation executor will fail to compile.
//!
//! ## Computations
//!
//! - `VerifyTags` - Compare disk tags to indexed tags
//! - `VerifyAudio` - Deep audio integrity check (decodes entire file)

mod executors;

pub use executors::*;

// Re-export Computation enum from mm-meta
pub use mm_meta::computations::observation::Computation;

/// Extension trait for server-side execution dispatch on observation computations.
pub trait ObservationExecute {
    fn execute(&self, ctx: &super::traits::ComputationContext) -> Result;
}

impl ObservationExecute for Computation {
    fn execute(&self, ctx: &super::traits::ComputationContext) -> Result {
        match self {
            Computation::VerifyTags { inode, path, mtime_secs, mtime_nanos, file_size: _, disk_tags } => {
                execute_verify_tags(ctx.read_db, *inode, path, *mtime_secs, *mtime_nanos, disk_tags, ctx.witness)
            }
            Computation::VerifyAudio { inode, path } => {
                execute_verify_audio(ctx.read_db, *inode, path, ctx.witness)
            }
        }
    }
}

// ============================================================================
// Observation Result
// ============================================================================

/// Result of executing an Observation-phase computation.
///
/// The `spawn` field can ONLY contain `observation::Computation`. This is the
/// compile-time enforcement mechanism for phase boundaries.
#[derive(Debug)]
pub struct Result {
    pub _computation: Computation,
    pub success: bool,
    pub error: Option<String>,
    /// Follow-up computations - ONLY Observation computations allowed.
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
