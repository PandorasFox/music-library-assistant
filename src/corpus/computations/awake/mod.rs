//! Awake-phase computations: Full-corpus analysis.
//!
//! These computations run during the "Awake" phase after Awakening completes.
//! They perform full-corpus-scope analysis: duplicate detection, tag analysis,
//! deploy health derivation. These require complete corpus awareness.
//!
//! ## Phase Boundary Enforcement
//!
//! The `Result` type's `spawn` field can ONLY contain `awake::Computation`.
//! This is enforced at compile time - attempting to spawn an Asleep or
//! Awakening computation from an Awake executor will fail to compile.
//!
//! ## Computations
//!
//! Content Analysis:
//! - `ScheduleContentAnalysis` - Orchestrator: spawns all detection computations
//! - `DetectFingerprintDuplicates` - Find tracks with identical fingerprints
//! - `DetectDuplicateInodes` - Find tracks sharing the same inode
//! - `DetectMissingTags` - Find tracks missing required tags
//! - `DetectMetadataDuplicates` - Find tracks with identical tag sets
//! - `DetectTagCanonicalizations` - Find tag canonicalization opportunities
//!
//! Deploy Health:
//! - `DetectDeployConflicts` - Bulk detection of deploy path collisions
//! - `DeriveDeployHealthSignals` - Derive library health signals from scan data
//!
//! Note: OOB tag change classification is now handled in the Asleep phase by
//! `VerifyTags` which directly emits OutOfBandTagSync, OutOfBandTagConflict,
//! or MtimeOnlyMismatch signals.

mod executors;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use executors::*;

// ============================================================================
// Awake Computation Enum
// ============================================================================

/// A computation that runs during the Awake phase.
///
/// These computations perform full-corpus-scope analysis. They can only
/// spawn other Awake computations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Computation {
    /// Schedule all content analysis computations.
    ///
    /// Orchestrator that spawns all detection computations in parallel.
    ScheduleContentAnalysis,

    /// Detect fingerprint duplicates across all tracks.
    ///
    /// Bulk SQL query: GROUP BY fingerprint HAVING COUNT > 1
    DetectFingerprintDuplicates,

    /// Detect duplicate inodes across all tracks.
    ///
    /// Bulk SQL query: GROUP BY inode HAVING COUNT > 1
    DetectDuplicateInodes,

    /// Detect tracks missing required tags.
    ///
    /// Groups by album/directory, lists which tags are missing.
    DetectMissingTags,

    /// Detect metadata duplicates (exact match on all tags).
    ///
    /// Groups tracks by their full tag signature.
    DetectMetadataDuplicates,

    /// Detect tag canonicalization opportunities.
    ///
    /// Finds similar tag values that could be unified.
    DetectTagCanonicalizations,

    /// Detect albums with inconsistent album_artist tags.
    ///
    /// Finds albums where tracks have different artists but missing/inconsistent album_artist.
    DetectInconsistentAlbumArtist,

    /// Detect compound tag values that should be split.
    ///
    /// Finds tag values containing separator characters (e.g., "Rock; Metal" in genre).
    DetectCompoundTagValues,

    /// Detect files with non-Vorbis container formats (MP3, M4A, WAV, etc).
    ///
    /// These files have poor metadata support or inefficient containers and
    /// should be transcoded to Opus (lossy) or FLAC (lossless).
    DetectShitFormats,

    /// Detect deployment conflicts (bulk).
    ///
    /// Groups healthy tracks by deployment path, flags conflicts.
    DetectDeployConflicts,

    /// Derive deploy health signals from library scan data.
    ///
    /// Compares library_scan_state against corpus to identify leftovers/stale.
    /// Renamed from DeriveLibraryHealthSignals.
    DeriveDeployHealthSignals {
        library_name: String,
        library_root: PathBuf,
        corpus_path_prefixes: Vec<PathBuf>,
    },

    /// Derive corpus-side deployment status signals.
    ///
    /// For each HealthyFile, emits DeployReady (not in any library) or
    /// DeployedHealthy (correctly deployed). Runs after DeriveDeployHealthSignals.
    DeriveCorpusDeployStatus,
}

impl Computation {
    /// Get a human-readable label for this computation.
    pub fn label(&self) -> &'static str {
        match self {
            Computation::ScheduleContentAnalysis => "Scheduling content analysis",
            Computation::DetectFingerprintDuplicates => "Detecting fingerprint duplicates",
            Computation::DetectDuplicateInodes => "Detecting duplicate inodes",
            Computation::DetectMissingTags => "Detecting missing tags",
            Computation::DetectMetadataDuplicates => "Detecting metadata duplicates",
            Computation::DetectTagCanonicalizations => "Detecting tag canonicalizations",
            Computation::DetectInconsistentAlbumArtist => "Detecting inconsistent album_artist",
            Computation::DetectCompoundTagValues => "Detecting compound tag values",
            Computation::DetectShitFormats => "Detecting shit format files",
            Computation::DetectDeployConflicts => "Detecting deploy conflicts",
            Computation::DeriveDeployHealthSignals { .. } => "Deriving deploy health",
            Computation::DeriveCorpusDeployStatus => "Deriving corpus deploy status",
        }
    }
}

// ============================================================================
// Awake Result
// ============================================================================

/// Result of executing an Awake-phase computation.
///
/// The `spawn` field can ONLY contain `awake::Computation`. This is the
/// compile-time enforcement mechanism for phase boundaries.
#[derive(Debug)]
pub struct Result {
    pub computation: Computation,
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
    /// Follow-up computations - ONLY Awake computations allowed.
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
