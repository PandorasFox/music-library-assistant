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

mod deploy;
mod duplicates;
mod external_matches;
mod formats;
pub(crate) mod image_index;
mod inbox_matches;
mod inbox_tags;
mod path_schema;
mod release_packing;
mod schedule;
mod tags;

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;

use crate::meta::recomputation::RecomputationScope;

pub use deploy::*;
pub use duplicates::*;
pub use external_matches::*;
pub use formats::*;
pub use image_index::*;
pub use inbox_matches::*;
pub use inbox_tags::*;
pub use path_schema::*;
pub use release_packing::*;
pub use schedule::*;
pub use tags::*;

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
    ScheduleContentAnalysis { scope: Option<RecomputationScope> },

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

    /// Pack corpus files into MusicBrainz releases (Stage 1 — orchestrator).
    ///
    /// Loads external matches, identifies releases, writes session manifest,
    /// spawns N ScoreReleaseCandidates (Stage 2), defers ComputeReleaseMappings
    /// (Stage 3) and EmitUnmatchedSignals (Stage 4) as barrier-separated phases.
    ///
    /// Manual trigger only (expensive), not part of ScheduleContentAnalysis.
    PackReleases,

    /// Score candidates for a single MusicBrainz release (Stage 2).
    ///
    /// Loads the release tracklist, finds matching corpus inodes, scores each
    /// (inode, track_slot) pairing, solves optimal per-release assignment via
    /// Hungarian algorithm. Then runs per-release elimination: finds unassigned
    /// audio in directories where this release has AcoustID picks and matches
    /// them to unfilled slots via composite scoring (duration/title/tracknumber).
    /// Writes all results to release_packing_scores table with match_method.
    ScoreReleaseCandidates { release_id: String },

    /// Classify release proposals into quality tiers (Stage 3a — orchestrator).
    ///
    /// Loads optimal scores and manifest, classifies proposals into tiers
    /// (Perfect, FullMatch, Incomplete, Single), packages state, and defers
    /// MIS rounds as separate computations for visibility.
    ComputeReleaseMappings,

    /// MIS on Perfect proposals (Stage 3b).
    ///
    /// All slots filled, 1:1 directory↔release mapping, no leftover files.
    /// Multi-medium releases supported via per-medium directory mapping.
    MapPerfectReleases {
        state: release_packing::SharedMappingState,
    },

    /// MIS on FullMatch proposals (Stage 3c).
    ///
    /// All slots filled, but cross-directory or directory has extra files.
    MapFullMatchReleases {
        state: release_packing::SharedMappingState,
    },

    /// MIS on Incomplete proposals (Stage 3c½/3d).
    ///
    /// Partial slot coverage (includes former NearMiss tier).
    /// Proposals enter with unclaimed portion of inode set.
    MapIncompleteReleases {
        state: release_packing::SharedMappingState,
    },

    /// Per-inode-best for single-track releases (Stage 3d/3e).
    ///
    /// Assigns single-track releases by picking the best-scoring release
    /// per unclaimed inode. No MIS needed (no multi-inode conflicts).
    MapSingleReleases {
        state: release_packing::SharedMappingState,
    },

    /// Resolve one connected component of the packing conflict graph.
    ///
    /// Self-contained MIS solver: builds local conflict graph from the carried
    /// proposals, solves via bitmask (≤25) or BnB (>25), emits PackedRelease +
    /// ReleasePacking signals per selected proposal. Spawned in parallel by
    /// tier orchestrators (MapPerfect/FullMatch/Incomplete/SingleReleases).
    ResolvePackingComponent {
        data: release_packing::SharedComponentData,
    },

    /// Emit unmatched signals after release packing (Stage 4).
    ///
    /// Identifies unmatched corpus tracks and unfilled release slots.
    /// PackedRelease, ReleasePacking, and PackingKnot signals are emitted
    /// per-component within each tier's MIS computation.
    EmitUnmatchedSignals,

    /// Derive external match signals from AcoustID lookup results.
    ///
    /// Compares AcoustID recording metadata against corpus tags, emitting
    /// ExternalMatch signals with classification (ExactMatch, ContentDiff,
    /// MetadataOnly) and per-tag diffs.
    DeriveExternalMatches,

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
            Computation::DetectInboxCorpusMatches => "Detecting inbox-corpus matches",
            Computation::DetectInboxTagCanonicity => "Detecting inbox tag canonicity",
            Computation::DetectInboxMissingTags => "Detecting inbox missing tags",
            Computation::DetectInboxCompoundTags => "Detecting inbox compound tags",
            Computation::DetectDiscExtractions => "Detecting disc extractions",
            Computation::DetectPathTagMismatches => "Detecting path-tag mismatches",
            Computation::PackReleases => "Packing releases",
            Computation::ScoreReleaseCandidates { .. } => "Scoring release candidates",
            Computation::ComputeReleaseMappings => "Classifying release proposals",
            Computation::MapPerfectReleases { .. } => "Mapping perfect releases",
            Computation::MapFullMatchReleases { .. } => "Mapping full-match releases",
            Computation::MapIncompleteReleases { .. } => "Mapping incomplete releases",
            Computation::MapSingleReleases { .. } => "Mapping single-track releases",
            Computation::ResolvePackingComponent { .. } => "Resolving packing component",
            Computation::EmitUnmatchedSignals => "Emitting unmatched signals",
            Computation::DeriveExternalMatches => "Deriving external match signals",
            Computation::SeedCompoundTagDirtyInodes { .. } => "Seeding compound tag dirty inodes",
            Computation::IndexImageFile => "Indexing image files",
        }
    }

    /// Execute this computation.
    pub fn execute(&self, ctx: &super::traits::ComputationContext) -> Result {
        match self {
            Computation::ScheduleContentAnalysis { ref scope } => {
                execute_schedule_content_analysis(ctx.read_db, scope)
            }
            Computation::DetectFingerprintOverlaps => {
                execute_detect_fingerprint_overlaps(ctx.read_db, ctx.witness)
            }
            Computation::DetectDuplicateInodes => {
                execute_detect_duplicate_inodes(ctx.read_db, ctx.witness)
            }
            Computation::DetectMissingTags => {
                execute_detect_missing_tags(ctx.read_db, ctx.witness)
            }
            Computation::DetectMetadataDuplicates => {
                execute_detect_metadata_duplicates(ctx.read_db, ctx.witness)
            }
            Computation::DetectTagCanonicalizations => {
                execute_detect_tag_canonicalizations(ctx.read_db, ctx.witness)
            }
            Computation::DetectInconsistentAlbumArtist => {
                execute_detect_inconsistent_album_artist(ctx.read_db, ctx.witness)
            }
            Computation::DetectCompoundTagValues => {
                execute_detect_compound_tag_values(ctx.read_db, ctx.witness)
            }
            Computation::DetectCompoundTagsForInode { inode } => {
                execute_detect_compound_tags_for_inode(ctx.read_db, *inode, ctx.witness)
            }
            Computation::DetectShitFormats => {
                execute_detect_shit_formats(ctx.read_db, ctx.witness)
            }
            Computation::AnalyzeFingerprintOverlaps => {
                execute_analyze_fingerprint_overlaps(ctx.read_db, ctx.witness)
            }
            Computation::DetectCrossSourceOverlaps => {
                execute_detect_cross_source_overlaps(ctx.read_db, ctx.witness)
            }
            Computation::DetectDeployConflicts => {
                execute_detect_deploy_conflicts(ctx.read_db, ctx.witness)
            }
            Computation::DetectReleaseOverlaps => {
                execute_detect_release_overlaps(ctx.read_db, ctx.witness)
            }
            Computation::DeriveDeployHealthSignals {
                library_name,
                library_root,
                corpus_path_prefixes,
            } => execute_derive_deploy_health_signals(
                ctx.read_db,
                library_name,
                library_root,
                corpus_path_prefixes,
                ctx.witness,
            ),
            Computation::DeriveCorpusDeployStatus => {
                execute_derive_corpus_deploy_status(ctx.read_db, ctx.witness)
            }
            Computation::DetectInboxCorpusMatches => {
                execute_detect_inbox_corpus_matches(ctx.read_db, ctx.witness)
            }
            Computation::DetectInboxTagCanonicity => {
                execute_detect_inbox_tag_canonicity(ctx.read_db, ctx.witness)
            }
            Computation::DetectInboxMissingTags => {
                execute_detect_inbox_missing_tags(ctx.read_db, ctx.witness)
            }
            Computation::DetectInboxCompoundTags => {
                execute_detect_inbox_compound_tags(ctx.read_db, ctx.witness)
            }
            Computation::DetectDiscExtractions => {
                execute_detect_disc_extractions(ctx.read_db, ctx.witness)
            }
            Computation::DetectPathTagMismatches => {
                execute_detect_path_tag_mismatches(ctx.read_db, ctx.witness)
            }
            Computation::PackReleases => execute_pack_releases(ctx.read_db, ctx.witness),
            Computation::ScoreReleaseCandidates { ref release_id } => {
                execute_score_release_candidates(ctx.read_db, release_id, ctx.witness)
            }
            Computation::ComputeReleaseMappings => {
                execute_compute_release_mappings(ctx.read_db, ctx.witness)
            }
            Computation::MapPerfectReleases { ref state } => {
                execute_map_perfect_releases(state, ctx.read_db, ctx.witness)
            }
            Computation::MapFullMatchReleases { ref state } => {
                execute_map_full_match_releases(state, ctx.read_db, ctx.witness)
            }
            Computation::MapIncompleteReleases { ref state } => {
                execute_map_incomplete_releases(state, ctx.read_db, ctx.witness)
            }
            Computation::MapSingleReleases { ref state } => {
                execute_map_single_releases(state, ctx.read_db, ctx.witness)
            }
            Computation::ResolvePackingComponent { ref data } => {
                execute_resolve_packing_component(data, ctx.read_db, ctx.witness)
            }
            Computation::EmitUnmatchedSignals => {
                execute_emit_unmatched_signals(ctx.read_db, ctx.witness)
            }
            Computation::DeriveExternalMatches => {
                execute_derive_external_matches(ctx.read_db, ctx.witness)
            }
            Computation::SeedCompoundTagDirtyInodes { ref new_separators } => {
                execute_seed_compound_tag_dirty_inodes(
                    ctx.read_db,
                    new_separators,
                    ctx.witness,
                )
            }
            Computation::IndexImageFile => {
                execute_index_image_file(ctx.read_db, ctx.witness)
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
    /// Follow-up computations - ONLY Analysis computations allowed.
    pub spawn: Vec<Computation>,
    /// Barrier-separated follow-up phases. Each phase runs only after all
    /// prior work drains. Only used by pipeline orchestrators (e.g., PackReleases).
    pub deferred_phases: VecDeque<(super::PipelineStage, Vec<super::Computation>)>,
}

impl Result {
    pub fn success(computation: Computation, spawn: Vec<Computation>) -> Self {
        Self {
            _computation: computation,
            success: true,
            error: None,
            spawn,
            deferred_phases: VecDeque::new(),
        }
    }

    pub fn failure(computation: Computation, error: String) -> Self {
        Self {
            _computation: computation,
            success: false,
            error: Some(error),
            spawn: Vec::new(),
            deferred_phases: VecDeque::new(),
        }
    }

    /// Create a pipeline orchestrator result with barrier-separated follow-up phases.
    ///
    /// `spawn` computations are queued immediately (same as normal).
    /// `deferred_phases` are queued one-at-a-time after all prior work drains.
    pub fn pipeline(
        computation: Computation,
        spawn: Vec<Computation>,
        deferred_phases: Vec<(super::PipelineStage, Vec<super::Computation>)>,
    ) -> Self {
        Self {
            _computation: computation,
            success: true,
            error: None,
            spawn,
            deferred_phases: VecDeque::from(deferred_phases),
        }
    }
}
