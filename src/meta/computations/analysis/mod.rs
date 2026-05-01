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
//! - `Detect{Artist,AlbumArtist,Album,Genre}TagCanonicalizations` - Find tag canonicalization opportunities (per tag type, run in parallel)
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

    /// Detect artist tag canonicalization opportunities.
    ///
    /// Finds similar `artist` tag values that could be unified.
    /// Reconciles only `TagCanonicitySignal` rows keyed `artist:*`.
    DetectArtistTagCanonicalizations,

    /// Detect albumartist tag canonicalization opportunities.
    ///
    /// Finds similar `albumartist` tag values that could be unified.
    /// Reconciles only `TagCanonicitySignal` rows keyed `albumartist:*`.
    DetectAlbumArtistTagCanonicalizations,

    /// Detect album tag canonicalization opportunities.
    ///
    /// Finds similar `album` tag values that could be unified, gated by
    /// disjoint release-id checks to avoid false positives across distinct releases.
    /// Reconciles only `TagCanonicitySignal` rows keyed `album:*`.
    DetectAlbumTagCanonicalizations,

    /// Detect genre tag canonicalization opportunities.
    ///
    /// Finds similar `genre` tag values that could be unified.
    /// Reconciles only `TagCanonicitySignal` rows keyed `genre:*`.
    DetectGenreTagCanonicalizations,

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

    /// Detect lossless files in non-Vorbis containers (WAV, AIFF, APE, WV).
    ///
    /// These files can be losslessly remuxed to FLAC.
    DetectLosslessRemux,

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
    /// Compares library files (zone='library') against corpus to identify leftovers/stale.
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

    /// Detect files with fully-applied MusicBrainz release tags.
    ///
    /// Checks for presence of both configured track + release MB tags.
    /// Emits MusicBrainzTagged per-file signals; clears when tags removed.
    DetectMusicBrainzTagged,

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
    /// `incremental=false` (operator-initiated full repack): truncates the
    /// intermediate tables, identifies all candidate releases, writes the full
    /// session manifest, spawns N ScoreReleaseCandidates (Stage 2), and defers
    /// ComputeReleaseMappings (Stage 3) and EmitUnmatchedSignals (Stage 4) as
    /// barrier-separated phases.
    ///
    /// `incremental=true` (live-ingestion path): re-scores only the releases
    /// reachable from inodes flagged dirty for `release_packing` (plus pinned
    /// releases). Manifest/candidates/scores rows for affected releases are
    /// deleted-and-rewritten; all other scoring data and existing
    /// PackedRelease / ReleasePacking signals stay intact, and mapping/MIS is
    /// **not** run. The next operator-initiated full repack reconciles the
    /// global packing decision against the updated scoring data.
    PackReleases { incremental: bool },

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
    ComputeReleaseMappings { incremental: bool },

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
    EmitUnmatchedSignals { incremental: bool },

    /// Gateway for incremental pinned release packing.
    ///
    /// Checks whether MB cache has the release data and scoring exists.
    /// Routes to fast path (scores exist), warm path (MB cached, needs scoring),
    /// or cold path (needs fetch first).
    ResolvePinForDir {
        release_id: String,
        dir_path: std::path::PathBuf,
    },

    /// Commit a pinned release: emit packing signals and invalidate displaced releases.
    ///
    /// Runs after scoring confirms full coverage. Emits PackedRelease + ReleasePacking
    /// signals. Invalidates all releases whose inodes are claimed by the pin.
    /// If coverage is incomplete, emits PinnedReleasePackFailure instead.
    CommitPinnedRelease {
        release_id: String,
        dir_path: std::path::PathBuf,
    },

    /// Clean up packing signals for an unpinned release.
    ///
    /// Removes PackedRelease, ReleasePacking, and related signals for the
    /// given release. Leaves the directory bare for the next full repack.
    InvalidatePinnedRelease {
        release_id: String,
        dir_path: std::path::PathBuf,
    },

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

    /// Register images observed by the FS watcher into the `files` table.
    ///
    /// Fast path: only writes path/inode/mtime/size — no image decoding.
    /// Spawns `AnalyzeImageMetadata` as a follow-up for the slow decode pass.
    /// This must complete before derivation runs so images aren't seen as ghosts.
    IndexObservedImages {
        images: Vec<crate::witch::fs_thread::ObservedImage>,
    },

    /// Extract format, dimensions, and role from image files on disk.
    ///
    /// Slow path spawned by `IndexObservedImages` — opens each file to read
    /// dimensions and classify role (cover_front, cover_back, other).
    /// Writes to `image_info` table. Does NOT gate derivation or startup.
    AnalyzeImageMetadata {
        images: Vec<crate::witch::fs_thread::ObservedImage>,
    },
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
            Computation::DetectArtistTagCanonicalizations => "Detecting artist canonicalizations",
            Computation::DetectAlbumArtistTagCanonicalizations => "Detecting albumartist canonicalizations",
            Computation::DetectAlbumTagCanonicalizations => "Detecting album canonicalizations",
            Computation::DetectGenreTagCanonicalizations => "Detecting genre canonicalizations",
            Computation::DetectInconsistentAlbumArtist => "Detecting inconsistent album_artist",
            Computation::DetectCompoundTagValues => "Scheduling compound tag detection",
            Computation::DetectCompoundTagsForInode { .. } => "Detecting compound tags",
            Computation::DetectLosslessRemux => "Detecting lossless remux candidates",
            Computation::AnalyzeFingerprintOverlaps => "Analyzing fingerprint overlaps",
            Computation::DetectCrossSourceOverlaps => "Detecting cross-source overlaps",
            Computation::DetectDeployConflicts => "Detecting deploy conflicts",
            Computation::DetectReleaseOverlaps => "Detecting release overlaps",
            Computation::DeriveDeployHealthSignals { .. } => "Deriving deploy health",
            Computation::DeriveCorpusDeployStatus => "Deriving corpus deploy status",
            Computation::DetectMusicBrainzTagged => "Detecting MusicBrainz-tagged files",
            Computation::DetectDiscExtractions => "Detecting disc extractions",
            Computation::DetectPathTagMismatches => "Detecting path-tag mismatches",
            Computation::PackReleases { .. } => "Packing releases",
            Computation::ScoreReleaseCandidates { .. } => "Scoring release candidates",
            Computation::ComputeReleaseMappings { .. } => "Classifying release proposals",
            Computation::MapPerfectReleases { .. } => "Mapping perfect releases",
            Computation::MapFullMatchReleases { .. } => "Mapping full-match releases",
            Computation::MapIncompleteReleases { .. } => "Mapping incomplete releases",
            Computation::MapSingleReleases { .. } => "Mapping single-track releases",
            Computation::ResolvePackingComponent { .. } => "Resolving packing component",
            Computation::EmitUnmatchedSignals { .. } => "Emitting unmatched signals",
            Computation::ResolvePinForDir { .. } => "Resolving pinned release",
            Computation::CommitPinnedRelease { .. } => "Committing pinned release",
            Computation::InvalidatePinnedRelease { .. } => "Invalidating unpinned release",
            Computation::DeriveExternalMatches => "Deriving external match signals",
            Computation::SeedCompoundTagDirtyInodes { .. } => "Seeding compound tag dirty inodes",
            Computation::IndexObservedImages { .. } => "Registering observed images",
            Computation::AnalyzeImageMetadata { .. } => "Analyzing image metadata",
        }
    }

    /// Execute this computation.
    pub fn execute(&self, ctx: &super::traits::ComputationContext) -> Result {
        match self {
            Computation::ScheduleContentAnalysis { ref scope } => {
                execute_schedule_content_analysis(ctx, scope)
            }
            Computation::DetectFingerprintOverlaps => {
                execute_detect_fingerprint_overlaps(ctx)
            }
            Computation::DetectDuplicateInodes => {
                execute_detect_duplicate_inodes(ctx)
            }
            Computation::DetectMissingTags => {
                execute_detect_missing_tags(ctx)
            }
            Computation::DetectMetadataDuplicates => {
                execute_detect_metadata_duplicates(ctx)
            }
            Computation::DetectArtistTagCanonicalizations => {
                execute_detect_artist_canonicalizations(ctx)
            }
            Computation::DetectAlbumArtistTagCanonicalizations => {
                execute_detect_album_artist_canonicalizations(ctx)
            }
            Computation::DetectAlbumTagCanonicalizations => {
                execute_detect_album_canonicalizations(ctx)
            }
            Computation::DetectGenreTagCanonicalizations => {
                execute_detect_genre_canonicalizations(ctx)
            }
            Computation::DetectInconsistentAlbumArtist => {
                execute_detect_inconsistent_album_artist(ctx)
            }
            Computation::DetectCompoundTagValues => {
                execute_detect_compound_tag_values(ctx)
            }
            Computation::DetectCompoundTagsForInode { inode } => {
                execute_detect_compound_tags_for_inode(ctx, *inode)
            }
            Computation::DetectLosslessRemux => {
                execute_detect_lossless_remux(ctx)
            }
            Computation::AnalyzeFingerprintOverlaps => {
                execute_analyze_fingerprint_overlaps(ctx)
            }
            Computation::DetectCrossSourceOverlaps => {
                execute_detect_cross_source_overlaps(ctx)
            }
            Computation::DetectDeployConflicts => {
                execute_detect_deploy_conflicts(ctx)
            }
            Computation::DetectReleaseOverlaps => {
                execute_detect_release_overlaps(ctx)
            }
            Computation::DeriveDeployHealthSignals {
                library_name,
                library_root,
                corpus_path_prefixes,
            } => execute_derive_deploy_health_signals(
                ctx,
                library_name,
                library_root,
                corpus_path_prefixes,
            ),
            Computation::DeriveCorpusDeployStatus => {
                execute_derive_corpus_deploy_status(ctx)
            }
            Computation::DetectMusicBrainzTagged => {
                execute_detect_musicbrainz_tagged(ctx)
            }
            Computation::DetectDiscExtractions => {
                execute_detect_disc_extractions(ctx)
            }
            Computation::DetectPathTagMismatches => {
                execute_detect_path_tag_mismatches(ctx)
            }
            Computation::PackReleases { incremental } => {
                execute_pack_releases(ctx, *incremental)
            }
            Computation::ScoreReleaseCandidates { ref release_id } => {
                execute_score_release_candidates(ctx, release_id)
            }
            Computation::ComputeReleaseMappings { incremental } => {
                execute_compute_release_mappings(ctx, *incremental)
            }
            Computation::MapPerfectReleases { ref state } => {
                execute_map_perfect_releases(ctx, state)
            }
            Computation::MapFullMatchReleases { ref state } => {
                execute_map_full_match_releases(ctx, state)
            }
            Computation::MapIncompleteReleases { ref state } => {
                execute_map_incomplete_releases(ctx, state)
            }
            Computation::MapSingleReleases { ref state } => {
                execute_map_single_releases(ctx, state)
            }
            Computation::ResolvePackingComponent { ref data } => {
                execute_resolve_packing_component(ctx, data)
            }
            Computation::EmitUnmatchedSignals { incremental } => {
                execute_emit_unmatched_signals(ctx, *incremental)
            }
            Computation::ResolvePinForDir { ref release_id, ref dir_path } => {
                release_packing::pinned::execute_resolve_pin_for_dir(ctx, release_id, dir_path)
            }
            Computation::CommitPinnedRelease { ref release_id, ref dir_path } => {
                release_packing::pinned::execute_commit_pinned_release(ctx, release_id, dir_path)
            }
            Computation::InvalidatePinnedRelease { ref release_id, ref dir_path } => {
                release_packing::pinned::execute_invalidate_pinned_release(ctx, release_id, dir_path)
            }
            Computation::DeriveExternalMatches => {
                execute_derive_external_matches(ctx)
            }
            Computation::SeedCompoundTagDirtyInodes { ref new_separators } => {
                execute_seed_compound_tag_dirty_inodes(ctx, new_separators)
            }
            Computation::IndexObservedImages { ref images } => {
                execute_index_observed_images(ctx, images)
            }
            Computation::AnalyzeImageMetadata { ref images } => {
                image_index::execute_analyze_image_metadata(ctx, images)
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
    /// Fetch requests — MB entities to fetch before continuing with `then` computations.
    pub fetch_requests: Vec<super::FetchRequest>,
}

impl Result {
    pub fn success(computation: Computation, spawn: Vec<Computation>) -> Self {
        Self {
            _computation: computation,
            success: true,
            error: None,
            spawn,
            deferred_phases: VecDeque::new(),
            fetch_requests: Vec::new(),
        }
    }

    pub fn failure(computation: Computation, error: String) -> Self {
        Self {
            _computation: computation,
            success: false,
            error: Some(error),
            spawn: Vec::new(),
            deferred_phases: VecDeque::new(),
            fetch_requests: Vec::new(),
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
            fetch_requests: Vec::new(),
        }
    }

    /// Create a result that requests MB entities be fetched before continuing.
    pub fn needs_fetch(
        computation: Computation,
        fetch_requests: Vec<super::FetchRequest>,
    ) -> Self {
        Self {
            _computation: computation,
            success: true,
            error: None,
            spawn: Vec::new(),
            deferred_phases: VecDeque::new(),
            fetch_requests,
        }
    }
}
