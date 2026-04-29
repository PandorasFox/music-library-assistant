//! Derivation-phase computation types.
//!
//! First-level signal derivations. Execution logic stays in mm.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use super::types::ObservedInodeMeta;

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

/// A sidecar image that the CAA fetch wants to replace with a better version.
///
/// Carries the downloaded bytes from the scheduler thread to a computation
/// that handles the full lifecycle: stash old → write new → clean up DB state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarReplacement {
    /// Absolute path of the existing sidecar to stash.
    pub existing_path: PathBuf,
    /// Absolute path where the new sidecar should be written.
    /// May differ from `existing_path` if the format changed (e.g. jpg → png).
    pub new_path: PathBuf,
    /// The downloaded image bytes.
    pub new_bytes: Vec<u8>,
}

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

    /// Derive corpus signals via global inode set comparison.
    ///
    /// Single-pass global comparison of observed disk inodes vs indexed inodes:
    /// - disk_only (disk - indexed) → UnindexedFile signals
    /// - index_only (indexed - disk) → MissingFile signals
    /// - both (disk ∩ indexed) → check OOB, emit HealthyFile or spawn verification
    DeriveCorpusSignals {
        /// Corpus inodes observed on disk (inode → enriched metadata).
        /// Populated by the FS watcher's initial scan and steady-state events.
        observed_inodes: HashMap<i64, ObservedInodeMeta>,
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

    /// Reconcile observed library files against the DB.
    ///
    /// Performs set reconciliation: new files are upserted, stale files are deleted,
    /// unchanged files are skipped. Replaces the old DELETE-all + INSERT-all pattern.
    ReconcileLibraryFiles {
        observed_files: Vec<ObservedLibraryFile>,
    },

    /// Apply a single watcher-observed library file create-or-change to the DB.
    ///
    /// Per-event surface for `WatcherMessage::FileCreated` and `FileChanged` on
    /// the library zone. Lets the DB stay in sync with the watcher's authoritative
    /// in-memory map without waiting for the next `ReconcileLibraryFiles` tick —
    /// closes the steady-state divergence gap that drove the perpetual-leftover
    /// loop.
    WatcherUpsertLibraryFile {
        file: ObservedLibraryFile,
    },

    /// Apply a single watcher-observed library file removal to the DB.
    ///
    /// Per-event surface for `WatcherMessage::FileRemoved` on the library zone.
    /// Symmetric to `WatcherUpsertLibraryFile`: drops the path row immediately
    /// rather than waiting for the next reconcile.
    WatcherDeleteLibraryFile {
        stored_path: String,
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

    /// Stash inferior sidecar images and write their CAA replacements.
    ///
    /// Handles the full lifecycle for sidecar replacement: stash old file to
    /// the cover-art stash, write new bytes, clear signals and drop old inode
    /// from the index. This is a computation (not a mutation) because sidecar
    /// images don't participate in the tag/audio/fingerprint signal graph.
    ///
    /// NOTE: The write of the new sidecar is not atomic w.r.t. disk-full — if
    /// the FS fills up between stash and write, the old file is safely stashed
    /// but the new one may not land. Acceptable for now.
    StashAndReplaceSidecars {
        replacements: Vec<SidecarReplacement>,
    },
}

impl Computation {
    /// Get a human-readable label for this computation.
    pub fn label(&self) -> &'static str {
        match self {
            Computation::ScheduleSecondLevelDerivations => "Scheduling signal derivations",
            Computation::DeriveCorpusSignals { .. } => "Deriving corpus signals",
            Computation::UpdateCorpusFileSignals { .. } => "Updating corpus file signals",
            Computation::UpdateLibraryFileSignals { .. } => "Updating library file signals",
            Computation::ReconcileLibraryFiles { .. } => "Reconciling library files",
            Computation::WatcherUpsertLibraryFile { .. } => "Recording library file upsert",
            Computation::WatcherDeleteLibraryFile { .. } => "Recording library file removal",
            Computation::UpdateDeploySignals { .. } => "Updating deploy signals",
            Computation::StashAndReplaceSidecars { .. } => "Stashing and replacing sidecar images",
        }
    }
}
