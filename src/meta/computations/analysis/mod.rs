//! Analysis-phase computations: Full-corpus analysis.
//!
//! These computations run during the Analysis phase after Derivation completes.
//! They perform full-corpus-scope analysis: duplicate detection, tag analysis,
//! deploy health derivation. These require complete corpus awareness.
//!
//! ## Phase Boundary Enforcement
//!
//! The `Result` type's `spawn` field can ONLY contain `analysis::Computation`.
//! This is enforced at compile time - attempting to spawn an Observation or
//! Derivation computation from an Analysis executor will fail to compile.
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
//! Note: OOB tag change classification is now handled in the Observation phase by
//! `VerifyTags` which directly emits OutOfBandTagSync, OutOfBandTagConflict,
//! or MtimeOnlyMismatch signals.

mod schedule;
mod duplicates;
mod tags;
mod deploy;
mod formats;
pub(crate) mod album_art;
mod album_art_info;
mod image_index;
mod inbox_matches;
mod inbox_tags;
mod path_schema;
mod external_matches;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::meta::recomputation::RecomputationScope;

pub use schedule::*;
pub use duplicates::*;
pub use tags::*;
pub use deploy::*;
pub use formats::*;
pub use album_art::*;
pub use album_art_info::*;
pub use image_index::*;
pub use inbox_matches::*;
pub use inbox_tags::*;
pub use path_schema::*;
pub use external_matches::*;

// ============================================================================
// Analysis Computation Enum
// ============================================================================

/// A computation that runs during the Analysis phase.
///
/// These computations perform full-corpus-scope analysis. They can only
/// spawn other Analysis computations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Computation {
    /// Schedule all content analysis computations.
    ///
    /// Orchestrator that spawns detection computations filtered by scope.
    /// None = run all (startup). Some(scope) = filter to dirty domains.
    ScheduleContentAnalysis {
        scope: Option<RecomputationScope>,
    },

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

    /// Detect release overlaps at the album-directory level.
    ///
    /// Groups corpus files by their computed album directory (parent of deploy path),
    /// then partitions by (source_dir, release_dir). Emits ReleaseOverlapSignal when
    /// multiple releases target the same album directory.
    DetectReleaseOverlaps,

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

    /// Detect inbox files missing required tags.
    ///
    /// Simplified version of DetectMissingTags for inbox zone.
    /// No ExpectedMissingTag suppression, no MissingAlbumSingle routing.
    DetectInboxMissingTags,

    /// Detect compound tag values in inbox files.
    ///
    /// Single-pass (no orchestrator) since inbox is small.
    /// Checks collaboration keywords + per-tag separators, enriches
    /// matching_parts against corpus vocabulary.
    DetectInboxCompoundTags,

    /// Detect disc values extractable from ALBUM or TRACKNUMBER tags.
    ///
    /// Pass 1: Scans ALBUM tags for patterns like "Album Name, Disc 2".
    /// Pass 2: Scans TRACKNUMBER tags for letter prefixes like "A01".
    /// Emits DiscExtraction aggregate signals.
    DetectDiscExtractions,

    /// Detect path-tag mismatches against configured schemas.
    ///
    /// For source dirs with path-schema configs, checks whether each file's
    /// path structure agrees with its DB tags. Emits PathTagMismatch signals.
    DetectPathTagMismatches,

    /// Derive external match signals from AcoustID lookup results.
    ///
    /// Compares AcoustID recording metadata against corpus tags, emitting
    /// ExternalMatch signals with classification (ExactMatch, ContentDiff,
    /// MetadataOnly) and per-tag diffs.
    DeriveExternalMatches,

    /// Backfill picture metadata (format, resolution, count) for existing files.
    ///
    /// Single-pass dirty-inode computation: extracts picture info via lofty and
    /// writes pic_format/pic_width/pic_height/pic_count to audio_info.
    BackfillAlbumArtInfo,

    /// Seed dirty inodes for compound tag recomputation after config change.
    ///
    /// Carries the new (tag_name, separator) pairs from a tag_splitting config
    /// change. Queries corpus_tags for inodes whose values contain each new
    /// separator and marks them dirty for compound_tag detection.
    SeedCompoundTagDirtyInodes {
        new_separators: Vec<(String, String)>,
    },

    /// Index image files discovered by the scanner.
    ///
    /// Dirty-inode computation: reads image dimensions and determines role
    /// from filename, writing metadata to the `image_info` table.
    IndexImageFile,
}

impl Computation {
    /// Get a human-readable label for this computation.
    pub fn label(&self) -> &'static str {
        match self {
            Computation::ScheduleContentAnalysis { .. } => "Scheduling content analysis",
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
            Computation::DetectReleaseOverlaps => "Detecting release overlaps",
            Computation::DeriveDeployHealthSignals { .. } => "Deriving deploy health",
            Computation::DeriveCorpusDeployStatus => "Deriving corpus deploy status",
            Computation::DetectEmbeddableAlbumArt => "Detecting embeddable album art",
            Computation::DetectInboxCorpusMatches => "Detecting inbox-corpus matches",
            Computation::DetectInboxTagCanonicity => "Detecting inbox tag canonicity",
            Computation::DetectInboxMissingTags => "Detecting inbox missing tags",
            Computation::DetectInboxCompoundTags => "Detecting inbox compound tags",
            Computation::DetectDiscExtractions => "Detecting disc extractions",
            Computation::DetectPathTagMismatches => "Detecting path-tag mismatches",
            Computation::DeriveExternalMatches => "Deriving external match signals",
            Computation::BackfillAlbumArtInfo => "Backfilling album art metadata",
            Computation::SeedCompoundTagDirtyInodes { .. } => "Seeding compound tag dirty inodes",
            Computation::IndexImageFile => "Indexing image files",
        }
    }

    /// Execute this computation.
    pub fn execute(&self, ctx: &super::traits::ComputationContext) -> Result {
        match self {
            Computation::ScheduleContentAnalysis { ref scope } => {
                execute_schedule_content_analysis(ctx.read_db, ctx.start, scope)
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
            Computation::DetectReleaseOverlaps => {
                execute_detect_release_overlaps(ctx.read_db, ctx.witness, ctx.start)
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
            Computation::DetectInboxMissingTags => {
                execute_detect_inbox_missing_tags(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::DetectInboxCompoundTags => {
                execute_detect_inbox_compound_tags(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::DetectDiscExtractions => {
                execute_detect_disc_extractions(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::DetectPathTagMismatches => {
                execute_detect_path_tag_mismatches(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::DeriveExternalMatches => {
                execute_derive_external_matches(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::BackfillAlbumArtInfo => {
                execute_backfill_album_art_info(ctx.read_db, ctx.witness, ctx.start)
            }
            Computation::SeedCompoundTagDirtyInodes { ref new_separators } => {
                execute_seed_compound_tag_dirty_inodes(ctx.read_db, new_separators, ctx.witness, ctx.start)
            }
            Computation::IndexImageFile => {
                execute_index_image_file(ctx.read_db, ctx.witness, ctx.start)
            }
        }
    }
}

// ============================================================================
// Analysis Result
// ============================================================================

/// Result of executing an Analysis-phase computation.
///
/// The `spawn` field can ONLY contain `analysis::Computation`. This is the
/// compile-time enforcement mechanism for phase boundaries.
#[derive(Debug)]
pub struct Result {
    pub _computation: Computation,
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
    /// Follow-up computations - ONLY Analysis computations allowed.
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
