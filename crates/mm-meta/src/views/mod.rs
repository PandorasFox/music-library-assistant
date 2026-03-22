//! Inter-system aggregate types for UI views.
//!
//! These types span protocol boundaries. They are computed by database queries
//! but consumed by UI code and computations. They represent derived views of
//! corpus state, not raw database row types.

pub mod canonicity_compound;
pub mod cluster_deploy;
pub mod external_matches;
pub mod health_modals;
pub mod inode_detail;
pub mod review_match;
pub mod startup_organize;

use serde::{Deserialize, Serialize};

use crate::mutations;
use crate::paths::PathResolver;

// ============================================================================
// Insights View Data Types
// ============================================================================

/// Insights data for the bucketed Insights view.
/// Computed at cache refresh time, never in render.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InsightsData {
    pub bucket_corpus: CorpusFilesBucket,
    pub bucket_placeholder: PlaceholderBucket,
    pub bucket_other: OtherSignalsBucket,
}

/// Bucket 1: Corpus Files - file state overview
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CorpusFilesBucket {
    // OOB signals at top - highest priority within bucket
    pub oob_tag_sync: usize,
    pub oob_tag_conflict: usize,
    pub mtime_only_mismatch: usize,
    // Standard corpus file signals
    pub files_in_corpus: usize,
    pub files_indexed: usize,
    pub files_unindexed: usize,
    pub files_missing: usize,
    pub directories_missing: usize,
    pub files_relocated: usize,
    /// Files that are corrupt (unreadable tags or waveform decode failure)
    pub corrupt_files: usize,
    /// Lossless non-Vorbis files (WAV, AIFF, APE, WV) — remux candidates
    pub lossless_remux_candidates: usize,
    /// Corpus image files (sidecar art, etc.) with entries in image_info
    pub images_in_corpus: usize,
    /// Filetype breakdown for files_in_corpus
    pub file_type_breakdown: Vec<(String, usize)>,
    /// Directory-level aggregation for selected signal
    pub _directory_breakdown: DirectoryBreakdown,
}

/// Bucket 2: Tag Squash - duplicates, tag canonicity, album_artist, and compound tag issues
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TagSquashBucket {
    /// Directory overlap clusters (grouped fingerprint overlaps for bulk resolution)
    pub cross_source_overlap_count: usize,
    /// Release overlaps (multiple releases → same album directory)
    pub release_overlap_count: usize,
    /// Subpar duplicates (lower quality versions, easy stash candidates)
    pub subpar_duplicate_count: usize,
    /// Redundant duplicates (equal quality, requires operator choice)
    pub redundant_duplicate_count: usize,
    /// Tag canonicity issues grouped by tag name (e.g., "artist": 50 clusters)
    pub tag_canonicity: Vec<TagSquashEntry>,
    /// Inconsistent album_artist issues count
    pub inconsistent_album_artist_count: usize,
    /// Compound tag values grouped by tag name (e.g., "artist", "genre")
    pub compound_tags: Vec<CompoundTagEntry>,
    /// Missing album singles (tracks without ALBUM but with ARTIST+TITLE)
    pub missing_album_single_count: usize,
    /// Disc values extractable from ALBUM or TRACKNUMBER tags
    pub disc_extraction_count: usize,
    /// Files where filename-derived tags don't match embedded tags
    pub path_tag_mismatch_count: usize,
    /// Same MB recording on different releases
    pub same_recording_different_release_count: usize,
}

/// Entry for tag squash signals (grouped by tag name)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagSquashEntry {
    /// Tag name (e.g., "artist", "genre", "album")
    pub tag_name: String,
    /// Number of clusters needing resolution
    pub cluster_count: usize,
    /// Total tracks affected (for ordering - higher = more important)
    pub _total_tracks: usize,
}

/// Entry for compound tag signals (grouped by tag name)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompoundTagEntry {
    /// Tag name (e.g., "artist", "genre")
    pub tag_name: String,
    /// Number of safe splits (all parts exist in corpus)
    pub safe_count: usize,
    /// Number of splits needing review (some parts are new)
    pub review_count: usize,
}

// Aliases for compatibility
pub type PlaceholderBucket = TagSquashBucket;

// ============================================================================
// External Match Review Types
// ============================================================================

/// An external match entry ready for operator review.
///
/// Read-only view: track → MB recording URL with confidence score.
/// Actual tagging decisions come from the bin-packed release analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalMatchReviewEntry {
    pub path: String,
    /// AcoustID confidence score.
    pub confidence: f64,
    /// Recording MBID.
    pub recording_id: String,
}

// ============================================================================
// External Matches View Data Types
// ============================================================================

/// Confidence tier for bucketing external matches by AcoustID confidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConfidenceTier {
    /// confidence == 1.0
    Perfect,
    /// 0.99 ≤ confidence < 1.0
    VeryHigh,
    /// 0.95 ≤ confidence < 0.99
    High,
    /// 0.90 ≤ confidence < 0.95
    Medium,
    /// confidence < 0.90
    Low,
}

impl ConfidenceTier {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Perfect => "100%",
            Self::VeryHigh => "99%+",
            Self::High => "95%+",
            Self::Medium => "90%+",
            Self::Low => "< 90%",
        }
    }

    pub fn from_confidence(c: f64) -> Self {
        if c >= 1.0 {
            Self::Perfect
        } else if c >= 0.99 {
            Self::VeryHigh
        } else if c >= 0.95 {
            Self::High
        } else if c >= 0.90 {
            Self::Medium
        } else {
            Self::Low
        }
    }

    /// Ordered list of all tiers from highest to lowest confidence.
    pub const ALL: [ConfidenceTier; 5] = [
        ConfidenceTier::Perfect,
        ConfidenceTier::VeryHigh,
        ConfidenceTier::High,
        ConfidenceTier::Medium,
        ConfidenceTier::Low,
    ];
}

/// A confidence bucket grouping external matches by AcoustID confidence tier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidenceBucket {
    pub tier: ConfidenceTier,
    pub total: usize,
    /// Entries in this bucket (for launching the review modal).
    pub entries: Vec<ExternalMatchReviewEntry>,
}

/// Data for the External Matches lateral view (loaded via cache thread).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExternalMatchesData {
    /// MetadataOnly entries — fingerprint matches on files with no existing tags.
    pub untagged_entries: Vec<ExternalMatchReviewEntry>,
    /// ContentDiff entries bucketed by confidence tier.
    pub confidence_buckets: Vec<ConfidenceBucket>,

    // Release packing per-category counts
    /// Perfect releases (all tracks matched via AcoustID).
    pub packing_perfect_count: usize,
    /// Full-match releases (all tracks assigned, some via elimination).
    pub packing_full_match_count: usize,
    /// Single-track releases.
    pub packing_singles_count: usize,
    /// Incomplete releases (some but not all tracks assigned).
    pub packing_incomplete_count: usize,
    /// Low-confidence releases (poor AcoustID coverage + low album match).
    pub packing_low_confidence_count: usize,
    /// Packing knot components (conflict tangles requiring review).
    pub packing_knots_count: usize,
    /// Unsolved: had AcoustID match, was scored, lost conflict resolution.
    pub unsolved_conflict_count: usize,
    /// Unsolved: had AcoustID match, never optimally scored for any release.
    pub unsolved_no_release_count: usize,
    /// Unsolved: fingerprinted but no AcoustID match.
    pub unsolved_no_match_count: usize,
    /// Count of releases with VA override suggestions.
    pub va_override_count: usize,
    /// Count of pinned release conflict signals (invariant violations).
    pub pinned_conflict_count: usize,
    /// True if any pinned release in config lacks a matching packed_release signal.
    pub pinned_releases_stale: bool,
}

/// Bucket 3: Other signals (sorted by magnitude)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OtherSignalsBucket {
    /// Sorted descending by count
    pub entries: Vec<OtherSignalEntry>,
}

/// Entry for other signals bucket
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtherSignalEntry {
    pub signal_type: String,
    pub display_label: String,
    pub count: usize,
    /// For aggregate signals that track affected files/tracks
    pub affected_count: Option<usize>,
}

/// Directory breakdown for detail pane
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirectoryBreakdown {
    /// Sorted by count descending
    pub _entries: Vec<DirectoryBreakdownEntry>,
}

/// Single entry in directory breakdown
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryBreakdownEntry {
    pub _directory: String,
    pub _count: usize,
}

// ============================================================================
// Deploy Modal Data Types
// ============================================================================

/// A file with deploy info (for healthy/new files).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploySignalFile {
    /// Target library name (e.g., "music", "soundtracks")
    pub library_name: String,
    /// Path in the corpus
    pub corpus_path: String,
    /// Computed deploy path in library (relative to library root, no library prefix)
    pub deploy_path: String,
}

/// A stale library file (deployed path differs from expected).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaleSignalFile {
    /// Library this file belongs to (e.g., "music")
    pub library_name: String,
    /// Current path in library (wrong) — "{library_name}/path/..."
    pub library_path: String,
    /// Expected path (computed from current tags) — "{library_name}/path/..."
    pub expected_path: String,
}

/// A leftover file (in library but no corpus backing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeftoverSignalFile {
    /// Library this file belongs to (e.g., "music")
    pub library_name: String,
    /// Path in the library — "{library_name}/path/..."
    pub library_path: String,
}

impl LeftoverSignalFile {
    /// Produce a StashLeftovers mutation for this leftover file.
    pub fn to_mutation(&self, resolver: &PathResolver) -> mutations::Mutation {
        let path_rel = std::path::Path::new("libraries").join(&self.library_path);
        let path = resolver.resolve(&path_rel);
        mutations::Mutation::StashLeftovers(mutations::file_ops::StashLeftoversMutation { path })
    }
}

impl StaleSignalFile {
    /// Produce a LibraryMove mutation to correct this stale deployment.
    pub fn to_mutation(&self, resolver: &PathResolver) -> mutations::Mutation {
        let source_rel = std::path::Path::new("libraries").join(&self.library_path);
        let source = resolver.resolve(&source_rel);
        let dest_rel = std::path::Path::new("libraries").join(&self.expected_path);
        let destination = resolver.resolve(&dest_rel);
        mutations::Mutation::LibraryMove(mutations::file_ops::LibraryMoveMutation {
            source,
            destination,
        })
    }
}

impl DeploySignalFile {
    /// Produce a HardLink mutation to deploy this file.
    ///
    /// Returns `None` if `library_name` or `deploy_path` is empty (config gap).
    pub fn to_mutation(&self, resolver: &PathResolver) -> Option<mutations::Mutation> {
        if self.deploy_path.is_empty() || self.library_name.is_empty() {
            return None;
        }
        let source = resolver.resolve(std::path::Path::new(&self.corpus_path));
        let dest_rel = std::path::Path::new("libraries")
            .join(&self.library_name)
            .join(&self.deploy_path);
        let destination = resolver.resolve(&dest_rel);
        Some(mutations::Mutation::HardLink(
            mutations::file_ops::HardLinkMutation {
                source,
                destination,
            },
        ))
    }
}

/// A deploy conflict group (multiple corpus files → same library path).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictGroup {
    /// The library path they all would deploy to
    pub deploy_path: String,
    /// List of conflicting corpus files: (corpus_path, inode)
    pub conflicting_files: Vec<(String, i64)>,
}

/// A sidecar deploy conflict group (multiple corpus images → same library sidecar path).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarConflictGroup {
    /// The library-relative deploy path they all target (e.g. "Artist/Album/cover.jpg")
    pub deploy_path: String,
    /// Target library name
    pub library_name: String,
    /// List of conflicting corpus image files: (corpus_path, inode)
    pub conflicting_files: Vec<(String, i64)>,
}

// ============================================================================
// Subpar Duplicate Resolution Types
// ============================================================================

/// A subpar duplicate file (lower quality version identified by fingerprint analysis).
#[derive(Debug, Clone)]
pub struct SubparDuplicateEntry {
    /// Path in the corpus (this is the subpar file)
    pub corpus_path: String,
    /// Reason for being subpar (e.g., "SubparBitrate", "SubparFormat", "SubparSampleRate")
    pub reason: String,
    /// Path to the superior version
    pub superior_path: String,
    /// Fingerprint similarity score (0.0-100.0).
    pub similarity_score: f64,
}

// ============================================================================
// OOB Tag Resolution Types
// ============================================================================

/// A single tag mismatch entry between DB and disk.
///
/// Mirrors the signal-level `TagMismatchEntry` with UI-friendly field naming.
/// The signal computation already aggregates multi-value tags into display strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagMismatchEntry {
    pub field: String,
    pub db_value: Option<String>,
    pub disk_value: Option<String>,
}

/// Classification bucket for OOB signal files.
///
/// Determined by typed OOB signal classification:
/// - MtimeOnly: MtimeOnlyMismatchSignal (mtime changed, tags identical) - needs acknowledgement
/// - DbOnly: all mismatches have `disk_value IS NULL`
/// - DiskOnly: all mismatches have `db_value IS NULL`
/// - Conflict: both values present, or mixed null directions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConflictBucket {
    MtimeOnly,
    DbOnly,
    DiskOnly,
    Conflict,
}

impl ConflictBucket {
    pub fn label(&self) -> &'static str {
        match self {
            Self::MtimeOnly => "Mtime Only",
            Self::DbOnly => "DB Only",
            Self::DiskOnly => "Disk Only",
            Self::Conflict => "Conflicts",
        }
    }

    pub fn index(&self) -> usize {
        *self as usize
    }

    pub fn next(&self) -> Self {
        match self {
            Self::MtimeOnly => Self::DbOnly,
            Self::DbOnly => Self::DiskOnly,
            Self::DiskOnly => Self::Conflict,
            Self::Conflict => Self::MtimeOnly,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            Self::MtimeOnly => Self::Conflict,
            Self::DbOnly => Self::MtimeOnly,
            Self::DiskOnly => Self::DbOnly,
            Self::Conflict => Self::DiskOnly,
        }
    }

    pub const ALL: [ConflictBucket; 4] = [
        ConflictBucket::MtimeOnly,
        ConflictBucket::DbOnly,
        ConflictBucket::DiskOnly,
        ConflictBucket::Conflict,
    ];

    /// Whether this bucket supports bulk resolution buttons (tag sync/conflict).
    pub fn is_resolvable(&self) -> bool {
        matches!(self, Self::DbOnly | Self::DiskOnly | Self::Conflict)
    }

    /// Whether this bucket supports acknowledgement (mtime-only changes).
    pub fn is_acknowledgeable(&self) -> bool {
        matches!(self, Self::MtimeOnly)
    }
}

/// A file with an OOB tag signal, classified into a conflict bucket.
///
/// Carries the mismatch data from the signal so the UI never needs to
/// re-read tags from disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OobFile {
    pub inode: i64,
    pub path: String,
    pub bucket: ConflictBucket,
    /// Tag mismatches from the signal (empty for MtimeOnly bucket).
    pub mismatches: Vec<TagMismatchEntry>,
}

/// Deploy status for the Deploy view and titlebar indicator.
/// Computed at cache refresh time, never in render.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeployStatus {
    /// Whether there is actionable deploy work (deploy_ready, stale, or leftover signals exist).
    pub needs_action: bool,
    /// Per-library file counts: (library_name, file_count).
    pub library_file_counts: Vec<(String, usize)>,
}

// ============================================================================
// Edit History View Data Types
// ============================================================================

/// Summary of one edit session for the History list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditSessionSummary {
    pub session_id: String,
    pub earliest_at: String,
    pub edit_count: usize,
    pub inode_count: usize,
}

/// Single edit record within a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditRecord {
    pub id: i64,
    pub inode: i64,
    pub field_name: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub edited_at: String,
}

/// Full edit history row for export (includes session_id).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditHistoryExportRow {
    pub id: i64,
    pub inode: i64,
    pub field_name: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub edited_at: String,
    pub session_id: String,
}

/// Data payload for the History view cache refresh.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EditHistoryData {
    pub sessions: Vec<EditSessionSummary>,
}

/// A file with a MovedFile signal (same inode, different path).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovedFileInfo {
    pub inode: i64,
    pub old_path: String,
    pub new_path: String,
    /// Zone the file was previously indexed in.
    pub old_zone: String,
    /// Zone the file is now found in.
    pub new_zone: String,
}

impl MovedFileInfo {
    /// Build an UpdateFilePath mutation to acknowledge this move.
    pub fn to_update_mutation(&self) -> crate::mutations::Mutation {
        let cross_zone = if self.old_zone != self.new_zone {
            Some(self.new_zone.clone())
        } else {
            None
        };
        crate::mutations::Mutation::UpdateFilePath(
            crate::mutations::indexing::UpdateFilePathMutation {
                zone: self.old_zone.clone(),
                inode: self.inode,
                new_path: std::path::PathBuf::from(&self.new_path),
                new_zone: cross_zone,
            },
        )
    }
}
