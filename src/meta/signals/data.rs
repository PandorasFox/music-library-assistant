//! Typed signal data structs.
//!
//! One struct per signal type, replacing untyped `metadata_json` blobs.
//!
//! ## Conventions
//!
//! - **Top-level signal structs** map 1:1 to SQL table columns. They are NOT
//!   `Serialize`/`Deserialize` — serialization happens at the column level.
//! - **Inner `*Data` structs** (for collection data stored as bincode BLOBs)
//!   derive `Serialize, Deserialize`. These are the BLOB payload types.
//! - Corpus file signals are keyed by `inode INTEGER PRIMARY KEY`.
//! - Aggregate signals are keyed by `key TEXT PRIMARY KEY`.

use serde::{Deserialize, Serialize};
use std::hash::Hash;

// ============================================================================
// Corpus File Signals (inode-keyed)
// ============================================================================

// --- Simple signals (all flat columns, no BLOB needed) ---

/// File exists in corpus directory.
/// Emitted during corpus walk (Observation phase).
#[derive(Debug, Clone)]
pub struct FileInCorpusSignal {
    pub inode: i64,
    pub path: String,
    /// Observation generation for stale signal cleanup.
    pub generation: u8,
}

/// File in corpus but not in index (needs indexing).
#[derive(Debug, Clone)]
pub struct UnindexedFileSignal {
    pub inode: i64,
    pub path: String,
}

/// File in corpus + index with matching state (healthy).
#[derive(Debug, Clone)]
pub struct HealthyFileSignal {
    pub inode: i64,
    pub path: String,
}

// ============================================================================
// Inbox file signals (inode-keyed)
// ============================================================================

/// File exists in inbox directory.
/// Emitted during inbox walk (Observation phase).
#[derive(Debug, Clone)]
pub struct FileInInboxSignal {
    pub inode: i64,
    pub path: String,
    /// Observation generation for stale signal cleanup.
    pub generation: u8,
}

/// File in inbox but not in index (needs indexing).
#[derive(Debug, Clone)]
pub struct InboxUnindexedSignal {
    pub inode: i64,
    pub path: String,
}

/// File in inbox + index with matching state (healthy).
#[derive(Debug, Clone)]
pub struct InboxHealthySignal {
    pub inode: i64,
    pub path: String,
}

/// Quality classification of an inbox file relative to its corpus matches.
///
/// Computed at signal emission time by comparing quality tiers (format class,
/// bitrate, sample rate). Stored both in the bincode BLOB and as a SQL column
/// for efficient query filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CorpusMatchQuality {
    /// Inbox file is better quality than all corpus matches — should be organized in.
    Better,
    /// Same quality tier as best corpus match — safe to stash.
    Equivalent,
    /// Inbox file is lower quality — safe to stash.
    Subpar,
}

impl CorpusMatchQuality {
    /// SQL column value for this classification.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Better => "better",
            Self::Equivalent => "equivalent",
            Self::Subpar => "subpar",
        }
    }
}

/// Inbox file has fingerprint+duration match against corpus file(s).
/// Likely a duplicate — operator can stash the inbox copy.
#[derive(Debug, Clone)]
pub struct InboxCorpusMatchSignal {
    pub inode: i64,
    pub path: String,
    /// Pre-computed quality classification relative to best corpus match.
    pub classification: CorpusMatchQuality,
    /// Serialized as bincode BLOB.
    pub data: InboxCorpusMatchData,
}

/// Match details for an inbox file that overlaps with corpus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxCorpusMatchData {
    /// Corpus inodes that match this inbox file.
    pub corpus_matches: Vec<InboxCorpusMatch>,
    /// Pre-computed quality classification (also stored as SQL column).
    pub classification: CorpusMatchQuality,
}

/// A single corpus file matching an inbox file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxCorpusMatch {
    pub corpus_inode: i64,
    pub corpus_path: String,
    pub similarity: f64,
}

// ============================================================================
// Corpus health signals (inode-keyed)
// ============================================================================

/// File is corrupt (unreadable tags or waveform decode failure).
#[derive(Debug, Clone)]
pub struct CorruptFileSignal {
    pub inode: i64,
    pub path: String,
}

/// File mtime changed but tags are identical (requires operator acknowledgement).
#[derive(Debug, Clone)]
pub struct MtimeOnlyMismatchSignal {
    pub inode: i64,
    pub path: String,
}

/// Directory in index but no longer exists in corpus.
#[derive(Debug, Clone)]
pub struct MissingDirectorySignal {
    pub inode: i64,
    pub path: String,
}

/// Operator-suppressed missing tag (file genuinely shouldn't have the tag).
#[derive(Debug, Clone)]
pub struct ExpectedMissingTagSignal {
    pub inode: i64,
}

// --- Signals with extra flat columns ---

/// File in index but no longer exists in corpus.
#[derive(Debug, Clone)]
pub struct MissingFileSignal {
    pub inode: i64,
    pub path: String,
    /// If the file was replaced at the same path, the new inode.
    pub replaced_by_inode: Option<i64>,
}

/// File was moved (same inode, different path than indexed).
///
/// When `old_zone` and `new_zone` differ, this is a cross-zone move
/// (e.g. inbox→corpus). Same-zone moves leave both as the same value.
#[derive(Debug, Clone)]
pub struct MovedFileSignal {
    pub inode: i64,
    pub path: String,
    pub old_path: String,
    /// Zone the file was previously indexed in.
    pub old_zone: String,
    /// Zone the file is now found in.
    pub new_zone: String,
}

/// File is in a non-Vorbis container format.
#[derive(Debug, Clone)]
pub struct ShitFormatSignal {
    pub inode: i64,
    pub path: String,
    pub file_type: String,
}

/// Healthy corpus file ready for deployment (not yet in any library).
#[derive(Debug, Clone)]
pub struct DeployReadySignal {
    pub inode: i64,
    pub path: String,
    pub deploy_path: String,
}

/// Healthy corpus file deployed at correct library path.
#[derive(Debug, Clone)]
pub struct DeployedHealthySignal {
    pub inode: i64,
    pub path: String,
    pub library_path: String,
}

/// Corpus sidecar image file ready for deployment to a library.
///
/// Emitted by `DeriveCorpusDeployStatus` for image files in corpus directories
/// that have deployed or deploy-ready audio, where the corresponding library
/// directory does not yet contain the image.
#[derive(Debug, Clone)]
pub struct SidecarDeployReadySignal {
    pub inode: i64,
    /// Corpus image path
    pub path: String,
    /// Target deploy path (relative, no library prefix): "Artist/Album/cover.jpg"
    pub deploy_path: String,
    /// Target library name
    pub library_name: String,
    /// Image metadata stored as BLOB
    pub data: SidecarDeployReadyData,
}

/// Serializable metadata for a sidecar deploy-ready signal.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SidecarDeployReadyData {
    pub role: String,
    pub format: String,
    pub width: u32,
    pub height: u32,
}

// --- Signals with bincode BLOB data ---

/// Tags on disk have extras in one direction only (syncable without conflict).
#[derive(Debug, Clone)]
pub struct OutOfBandTagSyncSignal {
    pub inode: i64,
    pub path: String,
    /// Serialized as bincode BLOB.
    pub mismatches: Vec<TagMismatchEntry>,
}

/// Tags on disk conflict with indexed tags (value differences or mixed directions).
#[derive(Debug, Clone)]
pub struct OutOfBandTagConflictSignal {
    pub inode: i64,
    pub path: String,
    /// Serialized as bincode BLOB.
    pub mismatches: Vec<TagMismatchEntry>,
}

/// A single tag mismatch between disk and DB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagMismatchEntry {
    pub tag_name: String,
    pub disk_value: Option<String>,
    pub db_value: Option<String>,
}

/// Track is a subpar duplicate (lower quality version of another track).
#[derive(Debug, Clone)]
pub struct SubparDuplicateSignal {
    pub inode: i64,
    pub path: String,
    /// Serialized as bincode BLOB.
    pub data: SubparDuplicateData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubparDuplicateData {
    pub reason: String,
    pub superior_inode: i64,
    pub superior_path: String,
    pub dupe_group_fingerprint: String,
    /// Fingerprint similarity score (0.0-100.0) between subpar and superior.
    #[serde(default)]
    pub similarity_score: f64,
}

/// Per-file compound tag detection results.
#[derive(Debug, Clone)]
pub struct CompoundTagSignal {
    pub inode: i64,
    pub path: String,
    /// Serialized as bincode BLOB.
    pub compounds: Vec<CompoundTagEntry>,
}

/// A single compound tag value detected in a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompoundTagEntry {
    pub tag_name: String,
    pub compound_value: String,
    pub split_parts: Vec<String>,
    pub separator: String,
    pub matching_parts: Vec<String>,
}

/// File's path disagrees with its tags according to a configured path-tag schema.
#[derive(Debug, Clone)]
pub struct PathTagMismatchSignal {
    pub inode: i64,
    pub path: String,
    /// Serialized as bincode BLOB.
    pub data: PathTagMismatchData,
}

/// Bincode-serialized payload for PathTagMismatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathTagMismatchData {
    pub source_dir: String,
    pub schema_template: String,
    pub mismatch_kind: PathMismatchKind,
}

/// The kind of path-tag mismatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PathMismatchKind {
    /// Path doesn't match the schema's expected structure at all.
    StructureMismatch { description: String },
    /// Path matches the structure but extracted values differ from DB tags.
    ValueMismatch {
        mismatches: Vec<PathTagValueMismatch>,
    },
}

/// A single tag value mismatch between path-extracted value and DB value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathTagValueMismatch {
    pub tag_name: String,
    pub path_value: String,
    /// None = tag missing from DB entirely.
    pub db_value: Option<String>,
}

/// External match from AcoustID/MusicBrainz compared against corpus tags.
/// Inode-keyed, one signal per corpus file with external match data.
#[derive(Debug, Clone)]
pub struct ExternalMatchSignal {
    pub inode: i64,
    pub path: String,
    /// Serialized as bincode BLOB.
    pub data: ExternalMatchData,
}

/// Bincode-serialized payload for ExternalMatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalMatchData {
    /// ExternalSource key (e.g., 1 for AcoustID).
    pub source: u8,
    /// MusicBrainz recording MBID (best match).
    pub recording_id: String,
    /// AcoustID confidence score.
    pub confidence: f64,
    /// Overall classification of the match.
    pub classification: MatchClassification,
    /// Per-tag differences (non-empty for ContentDiff/MetadataOnly).
    pub diffs: Vec<ExternalTagDiff>,
    /// How many recordings matched this fingerprint.
    pub total_candidates: usize,
    /// MusicBrainz release MBID (best matching release).
    pub release_id: Option<String>,
    /// MusicBrainz release group MBID.
    pub release_group_id: Option<String>,
}

/// Classification of how well external metadata matches corpus tags.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchClassification {
    /// All compared tags identical (raw string equality).
    ExactMatch,
    /// One or more tags differ.
    ContentDiff,
    /// External has metadata for tags absent in corpus.
    MetadataOnly,
}

/// A single tag difference between external and corpus data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalTagDiff {
    /// Tag name (e.g., "TITLE", "ARTIST", "ALBUM").
    pub tag_name: String,
    /// Value from external source.
    pub external_value: String,
    /// Value from corpus (None if tag absent).
    pub corpus_value: Option<String>,
}

// ============================================================================
// Release Packing (inode-keyed)
// ============================================================================

/// Release bin-packing result for a corpus file.
/// Inode-keyed, one signal per corpus file that was successfully packed into a release.
#[derive(Debug, Clone)]
pub struct ReleasePackingSignal {
    pub inode: i64,
    pub path: String,
    /// Serialized as bincode BLOB.
    pub data: ReleasePackingData,
}

/// How a corpus file was matched to a release track slot.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MatchMethod {
    /// Matched via AcoustID fingerprint lookup.
    AcoustId,
    /// Matched by elimination: all siblings assigned to same release,
    /// remaining files fill remaining slots.
    Elimination,
}

/// Bincode-serialized payload for ReleasePacking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleasePackingData {
    /// Best-matching MusicBrainz release MBID.
    pub release_id: String,
    /// Release title (denormalized for display without cache lookup).
    pub release_title: String,
    /// Release artist credit (joined string).
    pub release_artist: String,
    /// Track position within the medium (1-indexed).
    pub track_position: u32,
    /// Medium position (disc number, 1-indexed).
    pub medium_position: u32,
    /// Medium format (e.g. "CD", "Digital Media", "12\" Vinyl").
    pub medium_format: Option<String>,
    /// Track number string from the release (e.g. "A1" for vinyl, "3" for CD).
    pub track_number: String,
    /// Recording MBID at this track position.
    pub recording_id: String,
    /// Track title from the release tracklist.
    pub track_title: String,
    /// Composite confidence score (0.0-1.0).
    pub score: f64,
    /// Score breakdown for debugging/display.
    pub score_breakdown: PackingScoreBreakdown,
    /// Number of alternative releases considered for this inode.
    pub alternatives_count: u16,
    /// Release coverage: fraction of this release's tracks that are matched.
    pub release_coverage: f32,
    /// How this file was matched to its track slot.
    pub match_method: MatchMethod,
}

/// Breakdown of the composite packing score.
///
/// Each tag dimension (title, artist, album) is stored independently rather
/// than as a single composite, so weights can zero out dimensions that are
/// unreliable in certain scoring contexts (e.g., artist in elimination).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackingScoreBreakdown {
    /// AcoustID fingerprint confidence (0.0-1.0).
    pub acoustid_confidence: f64,
    /// Duration match quality (1.0 = exact, decays with mismatch).
    pub duration_match: f64,
    /// Title similarity (max of track title and recording title, 0.0-1.0).
    pub title_match: f64,
    /// Artist similarity (corpus ARTIST vs release artist, 0.0-1.0).
    pub artist_match: f64,
    /// Album similarity (corpus ALBUM vs release title, 0.0-1.0).
    pub album_match: f64,
    /// Track number match bonus (1.0 if TRACKNUMBER matches position, 0.0 otherwise).
    pub track_number_match: f64,
}

// ============================================================================
// Release Packing Gap Analysis Signals
// ============================================================================

/// Classification of unsolved corpus tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsolvedCategory {
    /// Had AcoustID match, was scored for releases, lost MIS conflict resolution.
    Conflict,
    /// Had AcoustID match but was never optimally scored for any release.
    NoRelease,
    /// Fingerprinted but no AcoustID recording match at all.
    NoMatch,
}

impl UnsolvedCategory {
    /// String value stored in the `category` column.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Conflict => "conflict",
            Self::NoRelease => "no_release",
            Self::NoMatch => "no_match",
        }
    }

}

/// Fingerprinted corpus inode not assigned to any release after packing.
/// (Corpus signal, inode PK)
#[derive(Debug, Clone)]
pub struct UnmatchedCorpusTrackSignal {
    pub inode: i64,
    pub path: String,
    pub category: UnsolvedCategory,
    /// Serialized as bincode BLOB.
    pub data: UnmatchedCorpusTrackData,
}

/// Bincode payload for UnmatchedCorpusTrack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnmatchedCorpusTrackData {
    /// Recording MBIDs this inode matched via AcoustID.
    pub recording_ids: Vec<String>,
    /// Release MBIDs where this inode was a candidate but lost conflict resolution.
    pub considered_release_ids: Vec<String>,
}

/// Release track position with no matching corpus file after global assignment.
/// (Aggregate signal, key = `{release_id}:{medium_pos}:{track_pos}`)
#[derive(Debug, Clone)]
pub struct UnfilledReleaseSlotSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: UnfilledReleaseSlotData,
}

/// Bincode payload for UnfilledReleaseSlot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnfilledReleaseSlotData {
    pub release_id: String,
    pub release_title: String,
    pub release_artist: String,
    pub medium_pos: u32,
    pub track_pos: u32,
    pub track_title: String,
    pub recording_id: String,
    pub filled_count: u32,
    pub total_tracks: u32,
}

/// Per-release aggregate packing result.
/// Key = `{category_prefix}:{release_id}` for SQL-level filtering.
/// (Aggregate signal, key = `full_match:{release_id}` | `single:{release_id}` | `incomplete:{release_id}`)
#[derive(Debug, Clone)]
pub struct PackedReleaseSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: PackedReleaseData,
}

/// Category of a packed release in the optimal solution.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PackedReleaseCategory {
    /// Every track matched via AcoustID fingerprint (highest confidence).
    Perfect,
    /// All slots filled, but some tracks matched via elimination/scoring.
    FullMatch,
    /// Single-track release.
    Single,
    /// Some but not all slots filled (includes former near-misses).
    Incomplete,
    /// FullMatch/Incomplete with poor AcoustID coverage and low album match —
    /// likely mispack from elimination filling slots on wrong release.
    LowConfidence,
}

impl PackedReleaseCategory {
    /// Key prefix for SQL LIKE filtering.
    pub fn key_prefix(&self) -> &'static str {
        match self {
            Self::Perfect => "perfect",
            Self::FullMatch => "full_match",
            Self::Single => "single",
            Self::Incomplete => "incomplete",
            Self::LowConfidence => "low_confidence",
        }
    }
}

/// Bincode payload for PackedRelease.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackedReleaseData {
    pub release_id: String,
    pub release_title: String,
    pub release_artist: String,
    pub category: PackedReleaseCategory,
    pub assigned_count: u32,
    pub total_tracks: u32,
    /// For LowConfidence: the AcoustID ratio that triggered the downgrade.
    pub low_confidence_acoustid_ratio: Option<f64>,
    /// For LowConfidence: the avg album_match that triggered the downgrade.
    pub low_confidence_avg_album_match: Option<f64>,
}

impl PackedReleaseData {
    /// Deserialize from bincode, handling blobs written before the
    /// low_confidence metric fields were added.
    pub fn deserialize_compat(blob: &[u8]) -> Result<Self, bincode::Error> {
        match bincode::deserialize::<Self>(blob) {
            Ok(data) => Ok(data),
            Err(_) => {
                #[derive(Deserialize)]
                struct Legacy {
                    release_id: String,
                    release_title: String,
                    release_artist: String,
                    category: PackedReleaseCategory,
                    assigned_count: u32,
                    total_tracks: u32,
                }
                let legacy: Legacy = bincode::deserialize(blob)?;
                Ok(Self {
                    release_id: legacy.release_id,
                    release_title: legacy.release_title,
                    release_artist: legacy.release_artist,
                    category: legacy.category,
                    assigned_count: legacy.assigned_count,
                    total_tracks: legacy.total_tracks,
                    low_confidence_acoustid_ratio: None,
                    low_confidence_avg_album_match: None,
                })
            }
        }
    }
}

// ============================================================================
// Packing Knot signal (aggregate, key-keyed)
// ============================================================================

/// Knot component from MIS conflict resolution.
/// Key = `{tier}:{knot_id}` (e.g., `full_match:3`).
#[derive(Debug, Clone)]
pub struct PackingKnotSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: PackingKnotData,
}

/// Bincode payload for PackingKnot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackingKnotData {
    pub tier: String,
    pub knot_id: usize,
    pub classification: KnotClassification,
    pub ratio: f64,
    pub contested_inodes: Vec<i64>,
    pub proposals: Vec<KnotProposalEntry>,
}

/// How a knot component was classified for extraction.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum KnotClassification {
    /// proposals/inodes ratio exceeded threshold.
    ByRatio,
    /// Component size exceeded limit.
    BySize,
}

/// A release proposal within a knot component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnotProposalEntry {
    pub release_id: String,
    pub release_title: String,
    pub release_artist: String,
    pub total_tracks: i32,
    pub total_score: f64,
    /// Whether this proposal was selected by greedy resolution.
    pub selected: bool,
    pub assignments: Vec<KnotAssignment>,
}

/// A single track assignment within a knot proposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnotAssignment {
    pub inode: i64,
    pub recording_id: String,
    pub medium_pos: i32,
    pub track_pos: i32,
    pub track_title: String,
    pub score: f64,
    pub score_breakdown: PackingScoreBreakdown,
    pub match_method: i32,
}

// ============================================================================
// Alternative Release Packing signal (aggregate, key-keyed)
// ============================================================================

/// Alternative release with identical inode signature to a winning release.
/// Key = `{winner_release_id}:{alt_release_id}`.
#[derive(Debug, Clone)]
pub struct AlternativeReleasePackingSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: AlternativeReleasePackingData,
}

/// Bincode payload for AlternativeReleasePacking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlternativeReleasePackingData {
    pub winner_release_id: String,
    pub winner_release_title: String,
    pub alternative_release_id: String,
    pub alternative_release_title: String,
    pub alternative_release_artist: String,
    pub alternative_score: f64,
    pub winner_score: f64,
    pub inode_count: u32,
}

// ============================================================================
// Various Artists Override signal (aggregate, key-keyed)
// ============================================================================

/// Suggested non-VA artist override for a winning release with "Various Artists".
/// Key = winner `release_id`.
#[derive(Debug, Clone)]
pub struct VariousArtistsOverrideSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: VariousArtistsOverrideData,
}

/// Bincode payload for VariousArtistsOverride.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariousArtistsOverrideData {
    pub release_id: String,
    pub release_title: String,
    pub suggested_artist: String,
    pub source: VariousArtistsOverrideSource,
}

/// How the VA override artist name was discovered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VariousArtistsOverrideSource {
    /// From an exact alternative (same inode signature, different release).
    ExactAlternative,
    /// From a competing proposal in the same component that overlaps the winner's inodes.
    CompetingProposal,
}

// ============================================================================
// Pinned Release Conflict signal (aggregate, key-keyed)
// ============================================================================

/// Invariant violation: a release is pinned by more directories than it has media.
/// Key = release_id. This is a hard stop — the release is skipped entirely.
#[derive(Debug, Clone)]
pub struct PinnedReleaseConflictSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: PinnedReleaseConflictData,
}

/// Bincode payload for PinnedReleaseConflict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinnedReleaseConflictData {
    pub release_id: String,
    pub release_title: String,
    pub release_artist: String,
    pub media_count: i32,
    pub directories: Vec<String>,
    pub reason: String,
}

/// A group of inodes sharing the same compound tag value.
/// Used to aggregate compound split resolution by value rather than per-file.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CompoundGroup {
    pub tag_name: String,
    pub compound_value: String,
    pub inodes: Vec<i64>,
}

impl CompoundTagEntry {
    /// Whether this compound split is "safe" (can be auto-applied without review).
    ///
    /// Safe if:
    /// - All split parts already exist as standalone values in the corpus, OR
    /// - Tag is "artist" and the separator is semicolon-based (artist names
    ///   delimited by semicolons are an unambiguous multi-value encoding).
    pub fn is_safe(&self) -> bool {
        if self.split_parts.is_empty() {
            return false;
        }
        // All parts already known in corpus
        if self.split_parts.len() == self.matching_parts.len() {
            return true;
        }
        // Artist semicolon splits are always safe
        self.tag_name.eq_ignore_ascii_case("artist") && self.separator.contains(';')
    }
}

// ============================================================================
// Aggregate Signals (semantic-keyed)
// ============================================================================

// --- Simple aggregate signals (flat columns only) ---

/// Operator-confirmed canonical tag value (whitelist — skip split detection).
#[derive(Debug, Clone)]
pub struct CanonicalTagSignal {
    pub key: String,
    pub tag_name: String,
    pub canonical_value: String,
    pub created_at: String,
}

/// Operator-confirmed expected overlap between two source directories.
/// Suppresses CrossSourceOverlap signal emission for this source pair.
#[derive(Debug, Clone)]
pub struct ExpectedOverlapSignal {
    pub key: String, // "source_a|source_b" (sorted, same format as CrossSourceOverlap keys)
    pub source_a: String,
    pub source_b: String,
    pub created_at: String,
}

/// Operator-confirmed expected fingerprint overlap.
/// Suppresses RedundantDuplicate and SubparDuplicate signal emission for this group.
#[derive(Debug, Clone)]
pub struct ExpectedDuplicateSignal {
    pub key: String, // fingerprint text (same key space as RedundantDuplicate)
    pub created_at: String,
}

/// Library file without corpus backing.
#[derive(Debug, Clone)]
pub struct LibraryLeftoverSignal {
    pub key: String,
}

impl LibraryLeftoverSignal {
    const KEY_PREFIX: &'static str = "library_leftover";

    /// Construct the canonical key for a library file.
    /// `library_name`: e.g. "music"
    /// `library_path`: e.g. "music/Artist/Album/track.opus" (library-name-prefixed)
    pub fn make_key(library_name: &str, library_path: &str) -> String {
        format!("{}:{}:{}", Self::KEY_PREFIX, library_name, library_path)
    }

    /// Key prefix for bulk-clearing all signals for a specific library.
    pub fn key_prefix_for_library(library_name: &str) -> String {
        format!("{}:{}:", Self::KEY_PREFIX, library_name)
    }

    /// Parse a key into (library_name, library_path). Returns None if malformed.
    pub fn parse_key(key: &str) -> Option<(&str, &str)> {
        let after = key.strip_prefix("library_leftover:")?;
        let idx = after.find(':')?;
        Some((&after[..idx], &after[idx + 1..]))
    }
}

// --- Aggregate signals with extra flat columns ---

/// Library file at wrong path (tags changed since deploy).
#[derive(Debug, Clone)]
pub struct LibraryStaleSignal {
    pub key: String,
    pub library_path: String,
    pub expected_path: String,
    pub corpus_path: String,
    pub inode: i64,
}

impl LibraryStaleSignal {
    const KEY_PREFIX: &'static str = "library_stale";

    /// Construct the canonical key for a stale library file.
    /// `library_name`: e.g. "music"
    /// `library_path`: e.g. "music/Artist/Album/track.opus" (library-name-prefixed)
    pub fn make_key(library_name: &str, library_path: &str) -> String {
        format!("{}:{}:{}", Self::KEY_PREFIX, library_name, library_path)
    }

    /// Key prefix for bulk-clearing all signals for a specific library.
    pub fn key_prefix_for_library(library_name: &str) -> String {
        format!("{}:{}:", Self::KEY_PREFIX, library_name)
    }
}

// --- Aggregate signals with bincode BLOB data ---

/// Multiple files with same fingerprint (internal overlap detection).
#[derive(Debug, Clone)]
pub struct FingerprintOverlapSignal {
    pub key: String,
    pub inodes: Vec<i64>,
}

/// Multiple files with same artist/album/title metadata.
#[derive(Debug, Clone)]
pub struct MetadataDuplicateSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: MetadataDuplicateData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataDuplicateData {
    pub tag_signature: String,
    pub inodes: Vec<i64>,
}

/// Multiple index entries sharing the same inode.
#[derive(Debug, Clone)]
pub struct DuplicateInodeSignal {
    pub key: String,
    pub inode: i64,
    pub inodes: Vec<i64>,
}

/// Tracks missing a required tag.
#[derive(Debug, Clone)]
pub struct MissingTagSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: MissingTagData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingTagData {
    pub missing_tags: Vec<String>,
    pub inodes: Vec<i64>,
}

/// Multiple corpus files deploy to the same library path.
#[derive(Debug, Clone)]
pub struct DeployConflictSignal {
    pub key: String,
    pub deploy_path: String,
    pub inodes: Vec<i64>,
}

/// Multiple corpus image files deploy to the same sidecar library path.
#[derive(Debug, Clone)]
pub struct SidecarDeployConflictSignal {
    /// Key: "library_name/deploy_path" (e.g. "music/Artist/Album/cover.jpg")
    pub key: String,
    /// The library-relative deploy path (e.g. "Artist/Album/cover.jpg")
    pub deploy_path: String,
    /// Target library name
    pub library_name: String,
    /// All corpus image inodes that would deploy to this path
    pub inodes: Vec<i64>,
}

/// Tag value collision needing canonicalization.
#[derive(Debug, Clone)]
pub struct TagCanonicitySignal {
    pub key: String,
    pub tag_name: String,
    /// Serialized as bincode BLOB.
    pub data: TagCanonicityData,
}

/// Bincode-serialized payload for TagCanonicity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagCanonicityData {
    /// (variant_value, count) pairs sorted by count DESC.
    pub variants: Vec<(String, usize)>,
    pub inodes: Vec<i64>,
}

/// Album has tracks with different artists + missing/inconsistent album_artist.
#[derive(Debug, Clone)]
pub struct InconsistentAlbumArtistSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: InconsistentAlbumArtistData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InconsistentAlbumArtistData {
    pub album: String,
    /// (artist_value, count) pairs.
    pub artist_variants: Vec<(String, usize)>,
    /// (album_artist_value, count) pairs.
    pub album_artist_variants: Vec<(String, usize)>,
    pub inodes: Vec<i64>,
}

/// Cross-source fingerprint overlap cluster.
#[derive(Debug, Clone)]
pub struct CrossSourceOverlapSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: CrossSourceOverlapData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossSourceOverlapData {
    pub source_a: String,
    pub source_b: String,
    pub source_a_can_stash: bool,
    pub source_b_can_stash: bool,
    pub overlap_count: usize,
    pub fingerprint_count: usize,
    pub fingerprint_keys: Vec<String>,
    pub track_pairs: Vec<CrossSourceTrackPair>,
}

/// Release overlap: multiple releases target the same album directory.
///
/// Keyed by album directory string (e.g., "Artist/Album").
/// Detects when files from different releases (different source+release-dir
/// combinations) would deploy into the same library directory.
#[derive(Debug, Clone)]
pub struct ReleaseOverlapSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: ReleaseOverlapData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseOverlapData {
    /// All (source_dir, release_dir) pairs contributing to this album directory.
    pub releases: Vec<ReleaseOverlapEntry>,
    /// Total file count across all releases.
    pub file_count: usize,
}

/// One release contributing files to an overlapping album directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseOverlapEntry {
    pub source_dir: String,
    pub release_dir: String,
    pub can_stash: bool,
    pub inodes: Vec<i64>,
    pub corpus_paths: Vec<String>,
}

/// Group of files with identical fingerprints and equivalent quality.
/// Neither file is subpar — requires operator choice.
#[derive(Debug, Clone)]
pub struct RedundantDuplicateSignal {
    pub key: String, // fingerprint text (same key space as FingerprintOverlapSignal)
    pub data: RedundantDuplicateData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedundantDuplicateData {
    pub file_type: String, // shared format (e.g. "flac")
    pub inodes: Vec<i64>,
    pub paths: Vec<String>, // parallel to inodes
}

/// Tracks missing an ALBUM tag but having ARTIST and TITLE (album-less singles).
#[derive(Debug, Clone, serde::Serialize)]
pub struct MissingAlbumSingleSignal {
    pub key: String, // lowercased artist name for dedup
    /// Serialized as bincode BLOB.
    pub data: MissingAlbumSingleData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingAlbumSingleData {
    pub artist: String, // display-cased artist name
    pub tracks: Vec<SingleTrackInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SingleTrackInfo {
    pub inode: i64,
    pub title: String,
    pub path: String,
}

/// Inbox files missing required tags.
/// Aggregate signal keyed by album/directory, reuses MissingTagData.
#[derive(Debug, Clone)]
pub struct InboxMissingTagSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: MissingTagData,
}

/// Per-inbox-file compound tag detection results.
/// Reuses Vec<CompoundTagEntry> from corpus compound tag detection.
#[derive(Debug, Clone)]
pub struct InboxCompoundTagSignal {
    pub inode: i64,
    pub path: String,
    /// Serialized as bincode BLOB.
    pub compounds: Vec<CompoundTagEntry>,
}

/// Inbox tag values that differ from corpus canonical spellings.
/// Aggregate signal keyed by "{tag_name}:{normalized_key}".
#[derive(Debug, Clone)]
pub struct InboxTagCanonicitySignal {
    pub key: String,      // "artist:beyonce"
    pub tag_name: String, // "artist"
    /// Serialized as bincode BLOB.
    pub data: InboxTagCanonicityData,
}

/// Bincode-serialized payload for InboxTagCanonicity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxTagCanonicityData {
    /// Inbox variants not matching any corpus value: (value, count)
    pub inbox_variants: Vec<(String, usize)>,
    /// All inbox inodes affected
    pub inbox_inodes: Vec<i64>,
    /// Corpus variants for this normalized key: (value, count) sorted DESC
    pub corpus_variants: Vec<(String, usize)>,
}

/// Disc value extractable from an existing tag (ALBUM or TRACKNUMBER).
/// Aggregate signal keyed by source-specific prefix + grouping key.
#[derive(Debug, Clone)]
pub struct DiscExtractionSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: DiscExtractionData,
}

/// Bincode-serialized payload for DiscExtraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscExtractionData {
    pub source: DiscExtractionSource,
    pub inodes: Vec<i64>,
}

/// Where the disc value was found.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DiscExtractionSource {
    /// From ALBUM tag: "Album, Disc 2". All files share same cleaned album.
    Album {
        original_album: String,
        cleaned_album: String,
        disc_number: String,
    },
    /// From TRACKNUMBER: "A01". Each file has different cleaned digits.
    TrackNumber {
        disc_prefix: String,
        /// Release context for display (album name)
        album: String,
        /// Release context for display (album artist)
        album_artist: String,
        per_file: Vec<TrackNumberExtraction>,
    },
}

/// Per-file data for track number disc extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackNumberExtraction {
    pub inode: i64,
    pub original_value: String, // "A01"
    pub cleaned_digits: String, // "01"
}

/// A pair of tracks from different sources that share a fingerprint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossSourceTrackPair {
    pub fingerprint_key: String,
    pub source_a_inode: i64,
    pub source_a_path: String,
    pub source_b_inode: i64,
    pub source_b_path: String,
}

// ============================================================================
// Deploy Lifecycle Classification
// ============================================================================

/// Deploy lifecycle phases for corpus and library files (audio and sidecar).
///
/// Used on both sides of the deploy pipeline:
/// - **Library side** (`DeriveDeployHealthSignals`): classifies each library
///   file as Healthy, Stale, or Leftover.
/// - **Corpus side** (`DeriveCorpusDeployStatus`): classifies each healthy
///   corpus file as Ready, Healthy (deployed correctly), or Stale (deployed
///   at wrong path). Conflict/overlap filters then suppress Ready signals.
///
/// Exhaustive match on this enum at classification sites ensures
/// both audio and sidecar codepaths handle all phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployLifecyclePhase {
    /// Corpus file ready for deployment (not yet in any library).
    Ready,
    /// File correctly deployed at expected library path.
    Healthy,
    /// File deployed but at wrong path (tags changed since deploy).
    Stale,
    /// Library file with no corpus backing (orphan).
    Leftover,
}

// ============================================================================
// SignalContentHash implementations
// ============================================================================

use super::registry::SignalContentHash;

/// Helper macro to implement SignalContentHash by hashing specified fields.
macro_rules! impl_content_hash {
    // Hash scalar fields (or empty for key/inode-only signals)
    ($ty:ty => [$($field:ident),* $(,)?]) => {
        impl SignalContentHash for $ty {
            fn content_hash_fields(&self, _hasher: &mut std::hash::DefaultHasher) {
                $(self.$field.hash(_hasher);)*
            }
        }
    };
    // Hash a blob field (bincode serialize)
    ($ty:ty => blob($blob:ident)) => {
        impl SignalContentHash for $ty {
            fn content_hash_fields(&self, hasher: &mut std::hash::DefaultHasher) {
                if let Ok(bytes) = bincode::serialize(&self.$blob) {
                    bytes.hash(hasher);
                }
            }
        }
    };
    // Hash scalar fields + blob field
    ($ty:ty => [$($field:ident),+] + blob($blob:ident)) => {
        impl SignalContentHash for $ty {
            fn content_hash_fields(&self, hasher: &mut std::hash::DefaultHasher) {
                $(self.$field.hash(hasher);)*
                if let Ok(bytes) = bincode::serialize(&self.$blob) {
                    bytes.hash(hasher);
                }
            }
        }
    };
    // No fields to hash (inode-only or key-only)
    ($ty:ty => []) => {
        impl SignalContentHash for $ty {
            fn content_hash_fields(&self, _: &mut std::hash::DefaultHasher) {}
        }
    };
}

// Corpus signals — scalar only
impl_content_hash!(FileInCorpusSignal => [path]);
impl_content_hash!(UnindexedFileSignal => [path]);
impl_content_hash!(HealthyFileSignal => [path]);
impl_content_hash!(FileInInboxSignal => [path]);
impl_content_hash!(InboxUnindexedSignal => [path]);
impl_content_hash!(InboxHealthySignal => [path]);
impl_content_hash!(CorruptFileSignal => [path]);
impl_content_hash!(MtimeOnlyMismatchSignal => [path]);
impl_content_hash!(MissingDirectorySignal => [path]);
impl_content_hash!(MissingFileSignal => [path, replaced_by_inode]);
impl_content_hash!(MovedFileSignal => [path, old_path, old_zone, new_zone]);
impl_content_hash!(ShitFormatSignal => [path, file_type]);
impl_content_hash!(DeployReadySignal => [path, deploy_path]);
impl_content_hash!(DeployedHealthySignal => [path, library_path]);
impl_content_hash!(ExpectedMissingTagSignal => []);

// Corpus signals — blob
impl_content_hash!(InboxCorpusMatchSignal => blob(data));
impl_content_hash!(OutOfBandTagSyncSignal => blob(mismatches));
impl_content_hash!(OutOfBandTagConflictSignal => blob(mismatches));
impl_content_hash!(SubparDuplicateSignal => blob(data));
impl_content_hash!(CompoundTagSignal => blob(compounds));
impl_content_hash!(PathTagMismatchSignal => blob(data));
impl_content_hash!(ExternalMatchSignal => blob(data));
impl_content_hash!(ReleasePackingSignal => blob(data));
impl_content_hash!(UnmatchedCorpusTrackSignal => blob(data));
impl_content_hash!(InboxCompoundTagSignal => blob(compounds));

// Corpus signals — scalar + blob
impl_content_hash!(SidecarDeployReadySignal => [path, deploy_path, library_name] + blob(data));

// Aggregate signals — scalar only
impl_content_hash!(CanonicalTagSignal => [tag_name, canonical_value]);
impl_content_hash!(LibraryLeftoverSignal => []);
impl_content_hash!(LibraryStaleSignal => [library_path, expected_path, corpus_path, inode]);
impl_content_hash!(ExpectedOverlapSignal => [source_a, source_b]);
impl_content_hash!(ExpectedDuplicateSignal => []);

// Aggregate signals — blob
impl_content_hash!(FingerprintOverlapSignal => blob(inodes));
impl_content_hash!(MetadataDuplicateSignal => blob(data));
impl_content_hash!(DuplicateInodeSignal => blob(inodes));
impl_content_hash!(MissingTagSignal => blob(data));
impl_content_hash!(MissingAlbumSingleSignal => blob(data));
impl_content_hash!(DeployConflictSignal => blob(inodes));
impl_content_hash!(SidecarDeployConflictSignal => blob(inodes));
impl_content_hash!(TagCanonicitySignal => blob(data));
impl_content_hash!(InconsistentAlbumArtistSignal => blob(data));
impl_content_hash!(CrossSourceOverlapSignal => blob(data));
impl_content_hash!(ReleaseOverlapSignal => blob(data));
impl_content_hash!(RedundantDuplicateSignal => blob(data));
impl_content_hash!(InboxTagCanonicitySignal => blob(data));
impl_content_hash!(InboxMissingTagSignal => blob(data));
impl_content_hash!(DiscExtractionSignal => blob(data));
impl_content_hash!(UnfilledReleaseSlotSignal => blob(data));
impl_content_hash!(PackedReleaseSignal => blob(data));
impl_content_hash!(PackingKnotSignal => blob(data));
impl_content_hash!(AlternativeReleasePackingSignal => blob(data));
impl_content_hash!(VariousArtistsOverrideSignal => blob(data));
impl_content_hash!(PinnedReleaseConflictSignal => blob(data));

// TypedSignalWrite is now generated by signal_registry! in registry.rs
