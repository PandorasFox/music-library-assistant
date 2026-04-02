//! Soft mutation types — library-zone filesystem operations dispatchable
//! without operator confirmation or transactions.
//!
//! Soft mutations are the automated counterpart to transaction-staged deploy
//! mutations (HardLink, LibraryMove, StashLeftovers). They share the same
//! underlying filesystem operations but are queued directly by the Witch
//! when auto-deploy is enabled, bypassing the decision/transaction system.
//!
//! Execution logic lives in the main crate (`src/meta/soft_mutations.rs`);
//! this module defines only the data types.

use std::path::PathBuf;

/// Execution phase for soft mutations.
///
/// `Ord` derives from declaration order: Cleanup < Deploy.
/// Soft mutations are bucketed by phase and executed with drain barriers,
/// ensuring cleanup (stash/move) completes before new deploys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SoftMutationPhase {
    /// Phase 1: clear library paths (stash leftovers, fix stale).
    Cleanup,
    /// Phase 2: deploy new hard links into cleared paths.
    Deploy,
}

/// A library-zone filesystem operation that can execute without a transaction.
#[derive(Debug, Clone)]
pub enum SoftMutation {
    /// Hard-link a corpus file into the library.
    DeployLink {
        source: PathBuf,
        destination: PathBuf,
    },
    /// Move a library file to its correct path (fix stale deployment).
    DeployMove {
        source: PathBuf,
        destination: PathBuf,
    },
    /// Stash an orphan library file (no corpus backing).
    StashLibrary {
        path: PathBuf,
    },
}

impl SoftMutation {
    /// Which execution phase this soft mutation belongs to.
    pub fn phase(&self) -> SoftMutationPhase {
        match self {
            SoftMutation::DeployLink { .. } => SoftMutationPhase::Deploy,
            SoftMutation::DeployMove { .. } => SoftMutationPhase::Cleanup,
            SoftMutation::StashLibrary { .. } => SoftMutationPhase::Cleanup,
        }
    }

    /// Human-readable label for logging.
    pub fn label(&self) -> &'static str {
        match self {
            SoftMutation::DeployLink { .. } => "Deploy link",
            SoftMutation::DeployMove { .. } => "Deploy move",
            SoftMutation::StashLibrary { .. } => "Stash library",
        }
    }
}
