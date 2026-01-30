//! Asleep-phase computations: Corpus observation.
//!
//! These computations run during the "Asleep" (eye Closed) phase at startup.
//! They observe the corpus filesystem state without making any inferences
//! about index correctness. Their purpose is pure file discovery.
//!
//! ## Phase Boundary Enforcement
//!
//! The `Result` type's `spawn` field can ONLY contain `asleep::Computation`.
//! This is enforced at compile time - attempting to spawn an Awakening or
//! Awake computation from an Asleep executor will fail to compile.
//!
//! ## Computations
//!
//! - `ClearExistingObservationState` - Clear stale FileInCorpus signals before fresh scan
//! - `WalkCorpus` - Enumerate directories, spawn per-directory scans
//! - `ScanCorpusDirectory` - Scan single directory, emit FileInCorpus signals
//! - `VerifyMtime` - Check file modification time
//! - `VerifyTags` - Compare disk tags to indexed tags
//! - `VerifyAudio` - Deep audio integrity check (decodes entire file)

mod executors;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use executors::*;

// ============================================================================
// Asleep Computation Enum
// ============================================================================

/// A computation that runs during the Asleep (eye Closed) phase.
///
/// These computations observe the corpus filesystem state. They can only
/// spawn other Asleep computations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Computation {
    /// Phase 0: Clear existing observation state before fresh scan.
    ///
    /// Clears all FileInCorpus signals so they can be rebuilt from scratch
    /// during the corpus walk. This ensures deleted files don't retain stale
    /// signals that would cause them to appear as "healthy" instead of "missing".
    ClearExistingObservationState,

    /// Phase 1: Walk corpus directory tree to collect file state.
    ///
    /// Enumerates top-level directories under root and spawns per-directory scans.
    WalkCorpus {
        root: PathBuf,
        source: String,
        /// When true, bypass mtime optimization and verify all indexed files.
        force_check: bool,
    },

    /// Walk and scan a single directory subtree.
    ///
    /// Walks the directory recursively, collects disk state, and compares
    /// against scan_state. Creates FileInCorpus signals and spawns
    /// mtime/tag verification as needed.
    ScanCorpusDirectory {
        directory: PathBuf,
        source: String,
        /// When true, bypass mtime optimization and verify all indexed files.
        force_check: bool,
    },

    /// Verify single file mtime.
    ///
    /// Checks if current mtime differs from expected (from scan_state).
    /// Spawns VerifyTags if mtime mismatched.
    VerifyMtime {
        track_id: i64,
        path: PathBuf,
        expected_mtime_secs: i64,
        expected_mtime_nanos: i64,
    },

    /// Verify tags on disk match database.
    ///
    /// Compares the actual file tags to what's stored in the index.
    VerifyTags {
        track_id: i64,
        path: PathBuf,
    },

    /// Verify audio stream integrity by decoding the entire file.
    ///
    /// Catches truncated files, corrupt streams, and other audio-level issues
    /// that tag verification wouldn't detect. Emits CorruptFile if decode fails.
    VerifyAudio {
        track_id: i64,
        path: PathBuf,
    },
}

impl Computation {
    /// Get a human-readable label for this computation.
    pub fn label(&self) -> &'static str {
        match self {
            Computation::ClearExistingObservationState => "Clearing observation state",
            Computation::WalkCorpus { .. } => "Observing",
            Computation::ScanCorpusDirectory { .. } => "Scanning directory",
            Computation::VerifyMtime { .. } => "Verifying mtime",
            Computation::VerifyTags { .. } => "Tag verification",
            Computation::VerifyAudio { .. } => "Audio verification",
        }
    }

    /// Get the primary file path affected by this computation, if any.
    pub fn primary_path(&self) -> Option<&std::path::Path> {
        match self {
            Computation::ClearExistingObservationState => None,
            Computation::WalkCorpus { root, .. } => Some(root),
            Computation::ScanCorpusDirectory { directory, .. } => Some(directory),
            Computation::VerifyMtime { path, .. } => Some(path),
            Computation::VerifyTags { path, .. } => Some(path),
            Computation::VerifyAudio { path, .. } => Some(path),
        }
    }
}

// ============================================================================
// Asleep Result
// ============================================================================

/// Result of executing an Asleep-phase computation.
///
/// The `spawn` field can ONLY contain `asleep::Computation`. This is the
/// compile-time enforcement mechanism for phase boundaries.
#[derive(Debug)]
pub struct Result {
    pub computation: Computation,
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
    /// Follow-up computations - ONLY Asleep computations allowed.
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
