//! Awakening-phase computations: First-level derivations.
//!
//! These computations run during the "Awakening" phase after Asleep completes.
//! They derive first-level signals by comparing corpus observations (FileInCorpus)
//! against the index to produce: UnindexedFile, MissingFile, HealthyFile.
//!
//! ## Phase Boundary Enforcement
//!
//! The `Result` type's `spawn` field can ONLY contain `awakening::Computation`.
//! This is enforced at compile time - attempting to spawn an Asleep or
//! Awake computation from an Awakening executor will fail to compile.
//!
//! ## Computations
//!
//! - `ScheduleSecondLevelDerivations` - Orchestrator: spawns per-directory work
//! - `DeriveDirectorySignals` - Derive signals for a single directory
//! - `UpdateCorpusFileSignals` - Lightweight per-file corpus signal update (post-mutation)
//! - `UpdateLibraryFileSignals` - Lightweight per-file library signal update (post-mutation)
//! - `WalkLibrary` - Enumerate library directories for scanning
//! - `ScanLibraryDirectory` - Scan library directory, store results in DB

mod executors;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use executors::*;

// ============================================================================
// Awakening Computation Enum
// ============================================================================

/// A computation that runs during the Awakening phase.
///
/// These computations derive first-level signals from corpus observations.
/// They can only spawn other Awakening computations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Computation {
    /// Schedule second-level signal derivations.
    ///
    /// Orchestrator that spawns per-directory computations after eyeballing.
    ScheduleSecondLevelDerivations,

    /// Derive second-level signals for a single directory.
    ///
    /// Compares FileInCorpus signals against indexed tracks:
    /// - FileInCorpus without track → UnindexedFile
    /// - Track without FileInCorpus → MissingFile
    /// - Track with FileInCorpus → HealthyFile
    DeriveDirectorySignals { directory: PathBuf },

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
    /// Collects (path, inode) pairs and stores them in library_scan_state table.
    /// Deploy health derivation happens in Awake phase.
    ScanLibraryDirectory {
        directory: PathBuf,
        library_name: String,
        library_root: PathBuf,
        corpus_path_prefixes: Vec<PathBuf>,
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
            Computation::DeriveDirectorySignals { .. } => "Deriving signals",
            Computation::UpdateCorpusFileSignals { .. } => "Updating corpus file signals",
            Computation::UpdateLibraryFileSignals { .. } => "Updating library file signals",
            Computation::WalkLibrary { .. } => "Walking library",
            Computation::ScanLibraryDirectory { .. } => "Scanning library directory",
            Computation::UpdateDeploySignals { .. } => "Updating deploy signals",
        }
    }
}

// ============================================================================
// Awakening Result
// ============================================================================

/// Result of executing an Awakening-phase computation.
///
/// The `spawn` field can ONLY contain `awakening::Computation`. This is the
/// compile-time enforcement mechanism for phase boundaries.
#[derive(Debug)]
pub struct Result {
    pub computation: Computation,
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
    /// Follow-up computations - ONLY Awakening computations allowed.
    pub spawn: Vec<Computation>,
}

impl Result {
    pub fn success(computation: Computation, duration_ms: u64, spawn: Vec<Computation>) -> Self {
        Self {
            computation,
            success: true,
            error: None,
            duration_ms,
            spawn,
        }
    }

    pub fn failure(computation: Computation, duration_ms: u64, error: String) -> Self {
        Self {
            computation,
            success: false,
            error: Some(error),
            duration_ms,
            spawn: Vec::new(),
        }
    }
}
