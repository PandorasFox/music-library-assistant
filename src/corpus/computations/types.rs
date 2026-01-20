//! Computation types and core definitions.
//!
//! This module defines the `Computation` enum (all computation variants),
//! `ComputationResult` (execution result), and `ComputationWitness` (proof
//! that code is executing within a computation context).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ============================================================================
// ComputationWitness - Proof of Computation Execution Context
// ============================================================================

/// Sealed module to prevent external construction of ComputationWitness.
mod sealed {
    /// Zero-sized proof that code is executing within a computation context.
    ///
    /// This witness is required by signal-altering database operations to ensure
    /// health signals are only modified through the computation system.
    ///
    /// Cannot be constructed outside of `execute_single` or the DB write thread.
    #[derive(Debug, Clone, Copy)]
    pub struct ComputationWitness(());

    impl ComputationWitness {
        /// Create a new witness. Only callable from within this crate's computation execution.
        pub(crate) fn new() -> Self {
            Self(())
        }

        /// Create a witness for the DB write thread.
        ///
        /// The DB thread executes signal operations that were enqueued from legitimate
        /// computation contexts (which required a witness at send time). This constructor
        /// allows the DB thread to obtain a witness for the actual DB call.
        pub(crate) fn new_for_db_thread() -> Self {
            Self(())
        }
    }
}

pub use sealed::ComputationWitness;

// ============================================================================
// Computation Enum
// ============================================================================

/// A computation operation that derives facts without altering corpus state.
///
/// Computations can be queued without a `DecisionWitness` because they only
/// emit signals - they don't modify files or index data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Computation {
    /// Verify tags on disk match database, recording mismatches as signals.
    ///
    /// This compares the actual file tags to what's stored in the index.
    /// Any mismatches are recorded as health signals for operator review.
    VerifyTags {
        track_id: i64,
        path: PathBuf,
    },

    // -------------------------------------------------------------------------
    // Eyeballing Computations
    // -------------------------------------------------------------------------

    /// Phase 1: Walk corpus directory tree to collect file state.
    ///
    /// Enumerates top-level directories under root and spawns per-directory scans.
    /// This provides granular progress feedback during startup.
    WalkCorpus {
        root: PathBuf,
        source: String,  // "corpus" or "legacy"
        /// If true, verify tags for ALL files (paranoid mode)
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
    ///
    /// Spawns: VerifyMtime for files with mtime mismatches (non-paranoid)
    ///         or VerifyTags for all files (paranoid mode)
    CompareInodes {
        source: String,
        /// Disk state: (inode, path, mtime_secs, mtime_nanos)
        disk_state: Vec<(i64, PathBuf, i64, i64)>,
        /// If true, verify tags for ALL files (paranoid mode)
        paranoid: bool,
    },

    /// Phase 3: Verify single file mtime.
    ///
    /// Checks if current mtime differs from expected (from scan_state).
    /// Spawns: VerifyTags if mtime mismatched.
    VerifyMtime {
        track_id: i64,
        path: PathBuf,
        expected_mtime_secs: i64,
        expected_mtime_nanos: i64,
    },

    // -------------------------------------------------------------------------
    // Second-Level Signal Computations
    // -------------------------------------------------------------------------

    /// Schedule second-level signal derivations.
    ///
    /// This is an orchestrator computation that spawns per-directory computations.
    /// Called during daemon Awakening phase after first-level eyeballing completes.
    ///
    /// Spawns: `DeriveDirectorySignals` for each directory that has either:
    /// - FileInCorpus signals (corpus directories with audio files)
    /// - Indexed tracks (may be missing from corpus now)
    ScheduleSecondLevelDerivations,

    /// Derive second-level signals for a single directory.
    ///
    /// Compares FileInCorpus signals against indexed tracks for this directory:
    /// - FileInCorpus without matching track → `UnindexedFile`
    /// - Track without matching FileInCorpus → `MissingFile`
    /// - Track with matching FileInCorpus (same inode/mtime) → `HealthyFile`
    /// - Track with different mtime → `CorpusFileModifiedOutOfBand`
    ///
    /// For files that become `HealthyFile`, spawns `CheckDeployConflicts`.
    DeriveDirectorySignals { directory: PathBuf },

    /// Check for deployment conflicts on a healthy track.
    ///
    /// Third-level computation triggered when a file is marked as healthy.
    /// Checks if this track has deployment conflicts with other tracks.
    CheckDeployConflicts { track_id: i64 },

    /// Update signals for a single file after mutation.
    ///
    /// Lightweight per-file computation that updates:
    /// - FileInCorpus: if file exists on disk
    /// - HealthyFile: if file exists in corpus AND is indexed
    /// - UnindexedFile: if file exists in corpus but NOT indexed
    /// - MissingFile: if file is indexed but NOT in corpus
    ///
    /// Does NOT spawn CheckDeployConflicts (handled in bulk by DetectDeployConflicts).
    UpdateFileSignals { path: PathBuf },

    // -------------------------------------------------------------------------
    // Library Health Computations
    // -------------------------------------------------------------------------

    /// Walk a library directory tree to collect file inodes.
    ///
    /// Spawns `ScanLibraryDirectory` for each subdirectory found.
    WalkLibrary {
        library_root: PathBuf,
        library_name: String,
        /// Corpus path prefixes that deploy to this library (from config.deploy_mappings)
        corpus_path_prefixes: Vec<PathBuf>,
    },

    /// Scan a single library directory and collect (path, inode) pairs.
    ///
    /// Results are accumulated in memory, then passed to DeriveLibraryHealthSignals.
    ScanLibraryDirectory {
        directory: PathBuf,
        library_name: String,
        library_root: PathBuf,
        corpus_path_prefixes: Vec<PathBuf>,
    },

    /// Derive library health signals for files in a single directory.
    ///
    /// Compares library inodes against corpus index:
    /// - Match by inode → check if path is correct (LibraryStale if wrong)
    /// - Library file without corpus backing → LibraryOrphan
    DeriveLibraryHealthSignals {
        library_name: String,
        library_root: PathBuf,
        corpus_path_prefixes: Vec<PathBuf>,
        /// Library files: (path, inode)
        library_files: Vec<(PathBuf, i64)>,
    },

    // -------------------------------------------------------------------------
    // Content Analysis Computations (Awakening Stage 2)
    // -------------------------------------------------------------------------

    /// Schedule all content analysis computations.
    ///
    /// This is an orchestrator that spawns all detection computations in parallel:
    /// - DetectFingerprintDuplicates
    /// - DetectDuplicateInodes
    /// - DetectMissingTags
    /// - DetectMetadataDuplicates
    /// - DetectTagCanonicalizations
    /// - VerifyOutOfBandChanges
    ScheduleContentAnalysis,

    /// Detect fingerprint duplicates across all tracks.
    ///
    /// Bulk SQL query: GROUP BY fingerprint HAVING COUNT > 1
    /// Creates/updates FingerprintDuplicate signals with fingerprint as issue_key.
    DetectFingerprintDuplicates,

    /// Detect duplicate inodes across all tracks.
    ///
    /// Bulk SQL query: GROUP BY inode HAVING COUNT > 1
    /// Creates DuplicateInode signals for each duplicate group.
    DetectDuplicateInodes,

    /// Detect tracks missing required tags.
    ///
    /// Checks each required tag (from config) and creates MissingTag signals.
    /// Issue key: "missing_tag:{tag_name}"
    DetectMissingTags,

    /// Detect metadata duplicates (exact match on artist/album/title).
    ///
    /// Case-insensitive tag names, CASE-SENSITIVE tag values.
    /// Creates MetadataDuplicate signals with metadata as issue_key.
    DetectMetadataDuplicates,

    /// Detect tag canonicalization opportunities.
    ///
    /// Calls existing detect_and_store_canonicalizations() which uses TagCloud.
    /// Stores to tag_canonicalization table (tag-level, not track-level).
    DetectTagCanonicalizations,

    /// Verify out-of-band changes for files with modified mtime.
    ///
    /// For each CorpusFileModifiedOutOfBand signal:
    /// - Read file tags and compare to database
    /// - If tags differ: create OutOfBandTagChange signal
    /// - If tags match: clear CorpusFileModifiedOutOfBand (file was touched but unchanged)
    VerifyOutOfBandChanges,

    /// Detect deployment conflicts across all healthy tracks (bulk).
    ///
    /// Groups healthy tracks by their computed deployment path. Tracks that would
    /// deploy to the same path are marked as DeployConflict signals.
    ///
    /// This is more efficient than per-track CheckDeployConflicts during bulk operations.
    DetectDeployConflicts,
}

impl Computation {
    /// Get the primary file path affected by this computation, if any.
    pub fn primary_path(&self) -> Option<&std::path::Path> {
        match self {
            Computation::VerifyTags { path, .. } => Some(path),
            Computation::WalkCorpus { root, .. } => Some(root),
            Computation::ScanCorpusDirectory { directory, .. } => Some(directory),
            Computation::CompareInodes { .. } => None,
            Computation::VerifyMtime { path, .. } => Some(path),
            Computation::ScheduleSecondLevelDerivations => None,
            Computation::DeriveDirectorySignals { directory } => Some(directory),
            Computation::CheckDeployConflicts { .. } => None,
            Computation::UpdateFileSignals { path } => Some(path),
            Computation::WalkLibrary { library_root, .. } => Some(library_root),
            Computation::ScanLibraryDirectory { directory, .. } => Some(directory),
            Computation::DeriveLibraryHealthSignals { .. } => None,
            // Content analysis computations
            Computation::ScheduleContentAnalysis => None,
            Computation::DetectFingerprintDuplicates => None,
            Computation::DetectDuplicateInodes => None,
            Computation::DetectMissingTags => None,
            Computation::DetectMetadataDuplicates => None,
            Computation::DetectTagCanonicalizations => None,
            Computation::VerifyOutOfBandChanges => None,
            Computation::DetectDeployConflicts => None,
        }
    }

    /// Get a human-readable label for this computation type.
    pub fn label(&self) -> &'static str {
        match self {
            Computation::VerifyTags { .. } => "Tag verification",
            Computation::WalkCorpus { paranoid: true, .. } => "Eyeballing (paranoid)",
            Computation::WalkCorpus { paranoid: false, .. } => "Eyeballing",
            Computation::ScanCorpusDirectory { paranoid: true, .. } => "Scanning directory (paranoid)",
            Computation::ScanCorpusDirectory { paranoid: false, .. } => "Scanning directory",
            Computation::CompareInodes { .. } => "Comparing inodes",
            Computation::VerifyMtime { .. } => "Verifying mtime",
            Computation::ScheduleSecondLevelDerivations => "Scheduling signal derivations",
            Computation::DeriveDirectorySignals { .. } => "Deriving signals",
            Computation::CheckDeployConflicts { .. } => "Checking deploy conflicts",
            Computation::UpdateFileSignals { .. } => "Updating file signals",
            Computation::WalkLibrary { .. } => "Walking library",
            Computation::ScanLibraryDirectory { .. } => "Scanning library directory",
            Computation::DeriveLibraryHealthSignals { .. } => "Deriving library health",
            // Content analysis computations
            Computation::ScheduleContentAnalysis => "Scheduling content analysis",
            Computation::DetectFingerprintDuplicates => "Detecting fingerprint duplicates",
            Computation::DetectDuplicateInodes => "Detecting duplicate inodes",
            Computation::DetectMissingTags => "Detecting missing tags",
            Computation::DetectMetadataDuplicates => "Detecting metadata duplicates",
            Computation::DetectTagCanonicalizations => "Detecting tag canonicalizations",
            Computation::VerifyOutOfBandChanges => "Verifying out-of-band changes",
            Computation::DetectDeployConflicts => "Detecting deploy conflicts",
        }
    }
}

// ============================================================================
// ComputationResult
// ============================================================================

/// Result of executing a single computation.
#[derive(Debug)]
pub struct ComputationResult {
    pub computation: Computation,
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
    /// Follow-up computations to queue (chaining mechanism).
    pub spawn: Vec<Computation>,
}

impl ComputationResult {
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
