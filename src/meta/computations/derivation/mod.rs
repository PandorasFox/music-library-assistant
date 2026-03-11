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
//! - `ScheduleSecondLevelDerivations` - Orchestrator: spawns global corpus/inbox derivation
//! - `DeriveCorpusSignals` - Global inode comparison for corpus signals
//! - `DeriveInboxSignals` - Global inode comparison for inbox signals
//! - `UpdateCorpusFileSignals` - Lightweight per-file corpus signal update (post-mutation)
//! - `UpdateLibraryFileSignals` - Lightweight per-file library signal update (post-mutation)
//! - `WalkLibrary` - Enumerate library directories for scanning
//! - `ScanLibraryDirectory` - Scan library directory, store results in DB

mod executors;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

pub use executors::*;

// ============================================================================
// Library File Observation
// ============================================================================

/// A library file observed on disk during ScanLibraryDirectory.
///
/// Accumulated by the Witch and passed to ReconcileLibraryFiles for
/// set reconciliation against the DB, replacing the old pattern of
/// unconditional DELETE + INSERT OR REPLACE.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservedLibraryFile {
    /// Stored path: "library_name/relative/path"
    pub stored_path: String,
    pub inode: i64,
    pub mtime_secs: i64,
    pub mtime_nanos: i64,
    pub file_size: i64,
}

// ============================================================================
// Derivation Computation Enum
// ============================================================================

/// A computation that runs during the Derivation phase.
///
/// These computations derive first-level signals from corpus observations.
/// They can only spawn other Derivation computations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Computation {
    /// Schedule second-level signal derivations.
    ///
    /// Orchestrator that spawns global corpus derivation and library walks.
    ScheduleSecondLevelDerivations,

    /// Derive inbox signals via global inode set comparison.
    ///
    /// Compares observed disk inodes vs inbox-indexed inodes:
    /// - disk_only (disk - indexed) → InboxUnindexed signals
    /// - both (disk ∩ indexed) → InboxHealthy signals
    DeriveInboxSignals {
        /// Inbox inodes observed on disk (inode → relative path).
        /// Populated by the FS watcher's initial scan and steady-state events.
        observed_inodes: HashMap<i64, String>,
    },

    /// Derive corpus signals via global inode set comparison.
    ///
    /// Single-pass global comparison of observed disk inodes vs indexed inodes:
    /// - disk_only (disk - indexed) → UnindexedFile signals
    /// - index_only (indexed - disk) → MissingFile signals
    /// - both (disk ∩ indexed) → check OOB, emit HealthyFile or spawn verification
    DeriveCorpusSignals {
        /// Corpus inodes observed on disk (inode → relative path).
        /// Populated by the FS watcher's initial scan and steady-state events.
        observed_inodes: HashMap<i64, String>,
    },

    /// Update corpus signals for a single file after mutation.
    ///
    /// Lightweight per-file computation that ensures corpus signals reflect
    /// the current state of a file after a mutation modifies it.
    /// Only valid for paths within the corpus directory.
    UpdateCorpusFileSignals { path: PathBuf },

    /// Update library signals for a single file after mutation.
    ///
    /// Ensures library signals (LibraryLeftover, etc.) are updated
    /// when a library file is modified or removed.
    /// Only valid for paths within library directories.
    UpdateLibraryFileSignals { path: PathBuf },

    /// Walk a library directory tree.
    ///
    /// Spawns ScanLibraryDirectory for each subdirectory found.
    WalkLibrary {
        library_root: PathBuf,
        library_name: String,
        corpus_path_prefixes: Vec<PathBuf>,
    },

    /// Scan a single library directory.
    ///
    /// Collects (path, inode) pairs and stores them in files table (zone='library').
    /// Deploy health derivation happens in Analysis phase.
    ScanLibraryDirectory {
        directory: PathBuf,
        library_name: String,
        library_root: PathBuf,
        corpus_path_prefixes: Vec<PathBuf>,
    },

    /// Reconcile observed library files against the DB.
    ///
    /// Performs set reconciliation: new files are upserted, stale files are deleted,
    /// unchanged files are skipped. Replaces the old DELETE-all + INSERT-all pattern.
    ReconcileLibraryFiles {
        observed_files: Vec<ObservedLibraryFile>,
    },

    /// Update deploy signals after a HardLink mutation.
    ///
    /// Clears DeployReady, ensures DeployedHealthy, clears library leftovers.
    UpdateDeploySignals {
        /// Corpus path (source of HardLink)
        corpus_path: PathBuf,
        /// Library path (destination of HardLink)
        library_path: PathBuf,
    },
}

impl Computation {
    /// Get a human-readable label for this computation.
    pub fn label(&self) -> &'static str {
        match self {
            Computation::ScheduleSecondLevelDerivations => "Scheduling signal derivations",
            Computation::DeriveInboxSignals { .. } => "Deriving inbox signals",
            Computation::DeriveCorpusSignals { .. } => "Deriving corpus signals",
            Computation::UpdateCorpusFileSignals { .. } => "Updating corpus file signals",
            Computation::UpdateLibraryFileSignals { .. } => "Updating library file signals",
            Computation::WalkLibrary { .. } => "Walking library",
            Computation::ScanLibraryDirectory { .. } => "Scanning library directory",
            Computation::ReconcileLibraryFiles { .. } => "Reconciling library files",
            Computation::UpdateDeploySignals { .. } => "Updating deploy signals",
        }
    }

    /// Execute this computation.
    pub fn execute(&self, ctx: &super::traits::ComputationContext) -> Result {
        match self {
            Computation::ScheduleSecondLevelDerivations => {
                execute_schedule_second_level_derivations(ctx.read_db, ctx.witness)
            }
            Computation::DeriveInboxSignals { observed_inodes } => execute_derive_inbox_signals(
                ctx.read_db,
                observed_inodes.clone(),
                ctx.witness,
            ),
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
            Computation::WalkLibrary {
                library_root,
                library_name,
                corpus_path_prefixes,
            } => execute_walk_library(
                ctx.read_db,
                library_root,
                library_name,
                corpus_path_prefixes,
                ctx.witness,
            ),
            Computation::ScanLibraryDirectory {
                directory,
                library_name,
                library_root,
                corpus_path_prefixes,
            } => execute_scan_library_directory(
                ctx.read_db,
                directory,
                library_name,
                library_root,
                corpus_path_prefixes,
                ctx.witness,
            ),
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
    /// Library files observed on disk during ScanLibraryDirectory.
    /// Accumulated by the Witch and consumed by ReconcileLibraryFiles.
    pub observed_library_files: Vec<ObservedLibraryFile>,
}

impl Result {
    pub fn success(computation: Computation, spawn: Vec<Computation>) -> Self {
        Self {
            _computation: computation,
            success: true,
            error: None,
            spawn,
            observed_library_files: Vec::new(),
        }
    }

    pub fn success_with_library_files(
        computation: Computation,
        spawn: Vec<Computation>,
        observed_library_files: Vec<ObservedLibraryFile>,
    ) -> Self {
        Self {
            _computation: computation,
            success: true,
            error: None,
            spawn,
            observed_library_files,
        }
    }

    pub fn failure(computation: Computation, error: String) -> Self {
        Self {
            _computation: computation,
            success: false,
            error: Some(error),
            spawn: Vec::new(),
            observed_library_files: Vec::new(),
        }
    }
}
