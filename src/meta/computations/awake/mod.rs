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
//! - `DetectFingerprintOverlaps` - Find tracks with identical fingerprints (internal)
//! - `DetectCrossSourceOverlaps` - Cluster fingerprint overlaps by source directory (UI-facing)
//! - `DetectDuplicateInodes` - Find tracks sharing the same inode
//! - `DetectMissingTags` - Find tracks missing required tags
//! - `DetectMetadataDuplicates` - Find tracks with identical tag sets
//! - `DetectTagCanonicalizations` - Find tag canonicalization opportunities
//!
//! Inbox:
//! - `DetectInboxCorpusMatches` - Find inbox files matching corpus by fingerprint
//! - `DetectInboxTagCanonicity` - Find inbox tag values differing from corpus canonical spellings
//!
//! Deploy Health:
//! - `DetectDeployConflicts` - Bulk detection of deploy path collisions
//! - `DeriveDeployHealthSignals` - Derive library health signals from scan data
//!
//! Note: OOB tag change classification is now handled in the Asleep phase by
//! `VerifyTags` which directly emits OutOfBandTagSync, OutOfBandTagConflict,
//! or MtimeOnlyMismatch signals.

mod schedule;
mod duplicates;
mod tags;
mod deploy;
mod formats;
mod album_art;
mod inbox_matches;
mod inbox_tags;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use schedule::*;
pub use duplicates::*;
pub use tags::*;
pub use deploy::*;
pub use formats::*;
pub use album_art::*;
pub use inbox_matches::*;
pub use inbox_tags::*;

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

    /// Detect fingerprint overlaps across all tracks.
    ///
    /// Bulk SQL query: GROUP BY fingerprint HAVING COUNT > 1
    /// Emits FingerprintOverlap signals (internal, not surfaced to UI).
    DetectFingerprintOverlaps,

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

    /// Detect compound tag values that should be split (orchestrator).
    ///
    /// Spawns DetectCompoundTagsForInode for each corpus inode, parallelizing
    /// the expensive regex work across worker threads.
    DetectCompoundTagValues,

    /// Detect compound tag values for a single inode.
    ///
    /// Checks tags for separator patterns and featuring patterns, emitting
    /// per-file CompoundTag signals for files with compound values.
    DetectCompoundTagsForInode { inode: i64 },

    /// Detect files with non-Vorbis container formats (MP3, M4A, WAV, etc).
    ///
    /// These files have poor metadata support or inefficient containers and
    /// should be transcoded to Opus (lossy) or FLAC (lossless).
    DetectShitFormats,

    /// Analyze fingerprint overlaps for similarity, variants, and quality.
    ///
    /// Reads FingerprintOverlap signals, clusters by duration, computes
    /// fingerprint similarity, detects variants via release metadata, ranks
    /// by quality, and emits SubparDuplicate signals for non-best tracks.
    AnalyzeFingerprintOverlaps,

    /// Detect cross-source fingerprint overlaps for bulk resolution.
    ///
    /// Reads FingerprintOverlap signals, classifies files by their configured
    /// source directory (from config `dir` stanzas), and emits CrossSourceOverlap
    /// signals for overlaps spanning different sources. Within-source overlaps
    /// are ignored (they're legitimate variants/releases).
    DetectCrossSourceOverlaps,

    /// Detect deployment conflicts (bulk).
    ///
    /// Groups healthy tracks by deployment path, flags conflicts.
    DetectDeployConflicts,

    /// Derive deploy health signals from library scan data.
    ///
    /// Compares library files (source='library') against corpus to identify leftovers/stale.
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

    /// Detect directories with sidecar album art embeddable into artless audio files.
    ///
    /// Scans directories for image files (cover.jpg, folder.png, etc.) and probes
    /// audio files for embedded pictures. Emits EmbeddableAlbumArt signals.
    DetectEmbeddableAlbumArt,

    /// Detect inbox files that match corpus files by fingerprint+duration.
    ///
    /// For each inbox file with a fingerprint, finds corpus files with similar
    /// fingerprints within duration tolerance. Emits InboxCorpusMatchSignal.
    DetectInboxCorpusMatches,

    /// Detect inbox tag values that differ from corpus canonical spellings.
    ///
    /// Compares inbox tag values against corpus vocabulary. Emits
    /// InboxTagCanonicitySignal for values whose normalized form matches
    /// corpus values but whose exact spelling differs.
    DetectInboxTagCanonicity,
}

impl Computation {
    /// Get a human-readable label for this computation.
    pub fn label(&self) -> &'static str {
        match self {
            Computation::ScheduleContentAnalysis => "Scheduling content analysis",
            Computation::DetectFingerprintOverlaps => "Detecting fingerprint overlaps",
            Computation::DetectDuplicateInodes => "Detecting duplicate inodes",
            Computation::DetectMissingTags => "Detecting missing tags",
            Computation::DetectMetadataDuplicates => "Detecting metadata duplicates",
            Computation::DetectTagCanonicalizations => "Detecting tag canonicalizations",
            Computation::DetectInconsistentAlbumArtist => "Detecting inconsistent album_artist",
            Computation::DetectCompoundTagValues => "Scheduling compound tag detection",
            Computation::DetectCompoundTagsForInode { .. } => "Detecting compound tags",
            Computation::DetectShitFormats => "Detecting shit format files",
            Computation::AnalyzeFingerprintOverlaps => "Analyzing fingerprint overlaps",
            Computation::DetectCrossSourceOverlaps => "Detecting cross-source overlaps",
            Computation::DetectDeployConflicts => "Detecting deploy conflicts",
            Computation::DeriveDeployHealthSignals { .. } => "Deriving deploy health",
            Computation::DeriveCorpusDeployStatus => "Deriving corpus deploy status",
            Computation::DetectEmbeddableAlbumArt => "Detecting embeddable album art",
            Computation::DetectInboxCorpusMatches => "Detecting inbox-corpus matches",
            Computation::DetectInboxTagCanonicity => "Detecting inbox tag canonicity",
        }
    }

    /// Execute this computation.
    pub fn execute(&self, ctx: &super::traits::ComputationContext) -> Result {
        match self {
            Computation::ScheduleContentAnalysis => {
                execute_schedule_content_analysis(ctx.read_db, ctx.start)
            }
            Computation::DetectFingerprintOverlaps => {
                execute_detect_fingerprint_overlaps(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::DetectDuplicateInodes => {
                execute_detect_duplicate_inodes(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::DetectMissingTags => {
                execute_detect_missing_tags(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::DetectMetadataDuplicates => {
                execute_detect_metadata_duplicates(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::DetectTagCanonicalizations => {
                execute_detect_tag_canonicalizations(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::DetectInconsistentAlbumArtist => {
                execute_detect_inconsistent_album_artist(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::DetectCompoundTagValues => {
                execute_detect_compound_tag_values(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::DetectCompoundTagsForInode { inode } => {
                execute_detect_compound_tags_for_inode(ctx.read_db, *inode, ctx.witness, ctx.start)
            }
            Computation::DetectShitFormats => {
                execute_detect_shit_formats(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::AnalyzeFingerprintOverlaps => {
                execute_analyze_fingerprint_overlaps(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::DetectCrossSourceOverlaps => {
                execute_detect_cross_source_overlaps(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::DetectDeployConflicts => {
                execute_detect_deploy_conflicts(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::DeriveDeployHealthSignals { library_name, library_root, corpus_path_prefixes } => {
                execute_derive_deploy_health_signals(ctx.read_db, library_name, library_root, corpus_path_prefixes, ctx.witness, ctx.start)
            }
            Computation::DeriveCorpusDeployStatus => {
                execute_derive_corpus_deploy_status(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::DetectEmbeddableAlbumArt => {
                execute_detect_embeddable_album_art(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::DetectInboxCorpusMatches => {
                execute_detect_inbox_corpus_matches(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::DetectInboxTagCanonicity => {
                execute_detect_inbox_tag_canonicity(ctx.read_db, ctx.witness, ctx.start)
            }
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
    pub _computation: Computation,
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
    /// Follow-up computations - ONLY Awake computations allowed.
    pub spawn: Vec<Computation>,
}

impl Result {
    pub fn success(computation: Computation, duration_ms: u64, spawn: Vec<Computation>) -> Self {
        Self {
            _computation: computation,
            success: true,
            error: None,
            duration_ms,
            spawn,
        }
    }

    pub fn failure(computation: Computation, duration_ms: u64, error: String) -> Self {
        Self {
            _computation: computation,
            success: false,
            error: Some(error),
            duration_ms,
            spawn: Vec::new(),
        }
    }
}
