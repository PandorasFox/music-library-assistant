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

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use executors::*;

// ============================================================================
// Observation Computation Enum
// ============================================================================

/// A computation that runs during the Observation phase.
///
/// These computations verify individual files against indexed state.
/// They can only spawn other Observation computations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Computation {
    /// Verify tags on disk match database.
    ///
    /// Compares the actual file tags to what's stored in the index.
    /// Pending-write aware: distinguishes MM-initiated writes from external changes.
    VerifyTags {
        inode: i64,
        path: PathBuf,
        /// Watcher-provided disk mtime (avoids stat round-trip).
        mtime_secs: i64,
        mtime_nanos: i64,
        /// Watcher-provided file size.
        file_size: i64,
        /// Watcher-provided disk tags (avoids re-reading file).
        disk_tags: crate::corpus::tags::TagSet,
    },

    /// Verify audio stream integrity by decoding the entire file.
    ///
    /// Catches truncated files, corrupt streams, and other audio-level issues
    /// that tag verification wouldn't detect. Emits CorruptFile if decode fails.
    VerifyAudio { inode: i64, path: PathBuf },
}

impl Computation {
    /// Get a human-readable label for this computation.
    pub fn label(&self) -> &'static str {
        match self {
            Computation::VerifyTags { .. } => "Tag verification",
            Computation::VerifyAudio { .. } => "Audio verification",
        }
    }

    /// Execute this computation.
    pub fn execute(&self, ctx: &super::traits::ComputationContext) -> Result {
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
