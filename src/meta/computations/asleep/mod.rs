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
//! - `VerifyMtime` - Check file modification time
//! - `VerifyTags` - Compare disk tags to indexed tags
//! - `VerifyAudio` - Deep audio integrity check (decodes entire file)

mod executors;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
        zone: String,
        /// When true, bypass mtime optimization and verify all indexed files.
        force_check: bool,
    },

    /// Walk and scan a single directory subtree.
    ///
    /// Walks the directory recursively, collects disk state, and compares
    /// against files table. Creates FileInCorpus signals and spawns
    /// mtime/tag verification as needed.
    ScanCorpusDirectory {
        directory: PathBuf,
        zone: String,
        /// When true, bypass mtime optimization and verify all indexed files.
        force_check: bool,
    },

    /// Verify single file mtime.
    ///
    /// Checks if current mtime differs from expected (from files table).
    /// Spawns VerifyTags if mtime mismatched.
    VerifyMtime {
        inode: i64,
        path: PathBuf,
        expected_mtime_secs: i64,
        expected_mtime_nanos: i64,
    },

    /// Verify tags on disk match database.
    ///
    /// Compares the actual file tags to what's stored in the index.
    VerifyTags {
        inode: i64,
        path: PathBuf,
    },

    /// Verify audio stream integrity by decoding the entire file.
    ///
    /// Catches truncated files, corrupt streams, and other audio-level issues
    /// that tag verification wouldn't detect. Emits CorruptFile if decode fails.
    VerifyAudio {
        inode: i64,
        path: PathBuf,
    },
}

impl Computation {
    /// Get a human-readable label for this computation.
    pub fn label(&self) -> &'static str {
        match self {
            Computation::WalkCorpus { .. } => "Observing",
            Computation::ScanCorpusDirectory { .. } => "Scanning directory",
            Computation::VerifyMtime { .. } => "Verifying mtime",
            Computation::VerifyTags { .. } => "Tag verification",
            Computation::VerifyAudio { .. } => "Audio verification",
        }
    }

    /// Execute this computation.
    pub fn execute(&self, ctx: &super::traits::ComputationContext) -> Result {
        match self {
            Computation::WalkCorpus { root, zone, force_check } => {
                execute_walk_corpus(ctx.read_db, root, zone, *force_check, ctx.start)
            }
            Computation::ScanCorpusDirectory { directory, zone, force_check } => {
                execute_scan_corpus_directory(ctx.read_db, directory, zone, *force_check, ctx.witness, ctx.start)
            }
            Computation::VerifyMtime { inode, path, expected_mtime_secs, expected_mtime_nanos } => {
                execute_verify_mtime(ctx.read_db, *inode, path, *expected_mtime_secs, *expected_mtime_nanos, ctx.start)
            }
            Computation::VerifyTags { inode, path } => {
                execute_verify_tags(ctx.read_db, *inode, path, ctx.witness, ctx.start)
            }
            Computation::VerifyAudio { inode, path } => {
                execute_verify_audio(ctx.read_db, *inode, path, ctx.witness, ctx.start)
            }
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
    pub _computation: Computation,
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
    /// Follow-up computations - ONLY Asleep computations allowed.
    pub spawn: Vec<Computation>,
    /// Corpus inodes observed on disk during this computation (inode → relative path).
    pub observed_corpus_inodes: HashMap<i64, String>,
    /// Inbox inodes observed on disk during this computation (inode → relative path).
    pub observed_inbox_inodes: HashMap<i64, String>,
}

impl Result {
    pub fn success(computation: Computation, duration_ms: u64, spawn: Vec<Computation>) -> Self {
        Self {
            _computation: computation,
            success: true,
            error: None,
            duration_ms,
            spawn,
            observed_corpus_inodes: HashMap::new(),
            observed_inbox_inodes: HashMap::new(),
        }
    }

    pub fn success_with_observations(
        computation: Computation,
        duration_ms: u64,
        spawn: Vec<Computation>,
        observed_corpus_inodes: HashMap<i64, String>,
        observed_inbox_inodes: HashMap<i64, String>,
    ) -> Self {
        Self {
            _computation: computation,
            success: true,
            error: None,
            duration_ms,
            spawn,
            observed_corpus_inodes,
            observed_inbox_inodes,
        }
    }

    pub fn failure(computation: Computation, duration_ms: u64, error: String) -> Self {
        Self {
            _computation: computation,
            success: false,
            error: Some(error),
            duration_ms,
            spawn: Vec::new(),
            observed_corpus_inodes: HashMap::new(),
            observed_inbox_inodes: HashMap::new(),
        }
    }
}
