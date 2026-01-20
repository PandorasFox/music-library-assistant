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
//! - `WalkCorpus` - Enumerate directories, spawn per-directory scans
//! - `ScanCorpusDirectory` - Scan single directory, emit FileInCorpus signals
//! - `CompareInodes` - Compare disk vs index inodes (no longer used, vestigial)
//! - `VerifyMtime` - Check file modification time
//! - `VerifyTags` - Compare disk tags to indexed tags

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
    /// Phase 1: Walk corpus directory tree to collect file state.
    ///
    /// Enumerates top-level directories under root and spawns per-directory scans.
    WalkCorpus {
        root: PathBuf,
        source: String,
        paranoid: bool,
    },

    /// Walk and scan a single directory subtree.
    ///
    /// Walks the directory recursively, collects disk state, and compares
    /// against scan_state. Creates FileInCorpus signals and spawns
    /// mtime/tag verification as needed.
    ScanCorpusDirectory {
        directory: PathBuf,
        source: String,
        paranoid: bool,
    },

    /// Phase 2: Compare disk state to database index.
    ///
    /// Creates signals for:
    /// - MissingFromDisk: indexed files not on disk
    /// - MissingFromIndex: disk files not indexed
    CompareInodes {
        source: String,
        disk_state: Vec<(i64, PathBuf, i64, i64)>,
        paranoid: bool,
    },

    /// Phase 3: Verify single file mtime.
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
}

impl Computation {
    /// Get a human-readable label for this computation.
    pub fn label(&self) -> &'static str {
        match self {
            Computation::WalkCorpus { paranoid: true, .. } => "Eyeballing (paranoid)",
            Computation::WalkCorpus { paranoid: false, .. } => "Eyeballing",
            Computation::ScanCorpusDirectory { paranoid: true, .. } => "Scanning directory (paranoid)",
            Computation::ScanCorpusDirectory { paranoid: false, .. } => "Scanning directory",
            Computation::CompareInodes { .. } => "Comparing inodes",
            Computation::VerifyMtime { .. } => "Verifying mtime",
            Computation::VerifyTags { .. } => "Tag verification",
        }
    }

    /// Get the primary file path affected by this computation, if any.
    pub fn primary_path(&self) -> Option<&std::path::Path> {
        match self {
            Computation::WalkCorpus { root, .. } => Some(root),
            Computation::ScanCorpusDirectory { directory, .. } => Some(directory),
            Computation::CompareInodes { .. } => None,
            Computation::VerifyMtime { path, .. } => Some(path),
            Computation::VerifyTags { path, .. } => Some(path),
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
