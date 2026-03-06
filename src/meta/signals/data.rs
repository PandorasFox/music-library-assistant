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

use std::hash::{Hash, Hasher};
use serde::{Deserialize, Serialize};

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
    ValueMismatch { mismatches: Vec<PathTagValueMismatch> },
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackingScoreBreakdown {
    /// AcoustID fingerprint confidence (0.0-1.0).
    pub acoustid_confidence: f64,
    /// Duration match quality (1.0 = exact, decays with mismatch).
    pub duration_match: f64,
    /// String similarity between corpus tags and MB metadata (0.0-1.0).
    pub tag_similarity: f64,
    /// Track number match bonus (1.0 if TRACKNUMBER matches position, 0.0 otherwise).
    pub track_number_match: f64,
    /// Directory cohesion bonus (fraction of sibling files mapping to same release).
    pub directory_cohesion: f64,
}

// ============================================================================
// Release Packing Gap Analysis Signals
// ============================================================================

/// Corpus inode with AcoustID recording matches but no release assignment
/// after global conflict resolution. (Corpus signal, inode PK)
#[derive(Debug, Clone)]
pub struct UnmatchedCorpusTrackSignal {
    pub inode: i64,
    pub path: String,
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

/// Near-miss: release with (n-1)/n tracks matched, all from same directory
/// containing n total audio files. The missing track is likely the unmatched file.
/// (Aggregate signal, key = `{release_id}:{directory}`)
#[derive(Debug, Clone)]
pub struct NearMissReleaseSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: NearMissReleaseData,
}

/// Bincode payload for NearMissRelease.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NearMissReleaseData {
    pub release_id: String,
    pub release_title: String,
    pub release_artist: String,
    pub directory: String,
    pub candidate_inode: i64,
    pub candidate_path: String,
    pub missing_medium_pos: u32,
    pub missing_track_pos: u32,
    pub missing_track_title: String,
    pub missing_recording_id: String,
    pub filled_count: u32,
    pub total_tracks: u32,
}

/// A group of inodes sharing the same compound tag value.
/// Used to aggregate compound split resolution by value rather than per-file.
#[derive(Debug, Clone)]
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
    pub key: String,        // "source_a|source_b" (sorted, same format as CrossSourceOverlap keys)
    pub source_a: String,
    pub source_b: String,
    pub created_at: String,
}

/// Operator-confirmed expected fingerprint overlap.
/// Suppresses RedundantDuplicate and SubparDuplicate signal emission for this group.
#[derive(Debug, Clone)]
pub struct ExpectedDuplicateSignal {
    pub key: String,        // fingerprint text (same key space as RedundantDuplicate)
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
#[derive(Debug, Clone)]
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
    pub key: String,       // "artist:beyonce"
    pub tag_name: String,  // "artist"
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
    pub original_value: String,   // "A01"
    pub cleaned_digits: String,   // "01"
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
// Typed Signal Write Envelope
// ============================================================================

/// Typed signal data for direct writes to per-signal tables.
///
/// Sent through the db_thread channel to avoid JSON serialization.
/// Each variant wraps the typed signal data struct and maps 1:1 to a table.
#[derive(Debug, Clone)]
pub enum TypedSignalWrite {
    // Corpus file signals (inode-keyed)
    FileInCorpus(FileInCorpusSignal),
    UnindexedFile(UnindexedFileSignal),
    HealthyFile(HealthyFileSignal),
    // Inbox file signals (inode-keyed)
    FileInInbox(FileInInboxSignal),
    InboxUnindexed(InboxUnindexedSignal),
    InboxHealthy(InboxHealthySignal),
    InboxCorpusMatch(InboxCorpusMatchSignal),
    CorruptFile(CorruptFileSignal),
    MtimeOnlyMismatch(MtimeOnlyMismatchSignal),
    MissingDirectory(MissingDirectorySignal),
    MissingFile(MissingFileSignal),
    MovedFile(MovedFileSignal),
    ShitFormat(ShitFormatSignal),
    DeployReady(DeployReadySignal),
    DeployedHealthy(DeployedHealthySignal),
    SidecarDeployReady(SidecarDeployReadySignal),
    OutOfBandTagSync(OutOfBandTagSyncSignal),
    OutOfBandTagConflict(OutOfBandTagConflictSignal),
    SubparDuplicate(SubparDuplicateSignal),
    CompoundTag(CompoundTagSignal),
    PathTagMismatch(PathTagMismatchSignal),
    ExternalMatch(ExternalMatchSignal),
    ReleasePacking(ReleasePackingSignal),
    UnmatchedCorpusTrack(UnmatchedCorpusTrackSignal),
    // Aggregate signals (semantic-keyed)
    CanonicalTag(CanonicalTagSignal),
    LibraryLeftover(LibraryLeftoverSignal),
    LibraryStale(LibraryStaleSignal),
    FingerprintOverlap(FingerprintOverlapSignal),
    MetadataDuplicate(MetadataDuplicateSignal),
    DuplicateInode(DuplicateInodeSignal),
    MissingTag(MissingTagSignal),
    DeployConflict(DeployConflictSignal),
    SidecarDeployConflict(SidecarDeployConflictSignal),
    TagCanonicity(TagCanonicitySignal),
    InconsistentAlbumArtist(InconsistentAlbumArtistSignal),
    CrossSourceOverlap(CrossSourceOverlapSignal),
    ReleaseOverlap(ReleaseOverlapSignal),
    RedundantDuplicate(RedundantDuplicateSignal),
    ExpectedOverlap(ExpectedOverlapSignal),
    ExpectedDuplicate(ExpectedDuplicateSignal),
    MissingAlbumSingle(MissingAlbumSingleSignal),
    ExpectedMissingTag(ExpectedMissingTagSignal),
    InboxTagCanonicity(InboxTagCanonicitySignal),
    InboxMissingTag(InboxMissingTagSignal),
    InboxCompoundTag(InboxCompoundTagSignal),
    DiscExtraction(DiscExtractionSignal),
    UnfilledReleaseSlot(UnfilledReleaseSlotSignal),
    NearMissRelease(NearMissReleaseSignal),
}

impl TypedSignalWrite {
    /// Insert this signal into its typed table.
    pub fn insert(self, conn: &rusqlite::Connection) -> rusqlite::Result<()> {
        use crate::meta::signals::store::{AggregateSignalStore, CorpusSignalStore};
        match self {
            Self::FileInCorpus(s) => s.insert(conn),
            Self::UnindexedFile(s) => s.insert(conn),
            Self::HealthyFile(s) => s.insert(conn),
            Self::FileInInbox(s) => s.insert(conn),
            Self::InboxUnindexed(s) => s.insert(conn),
            Self::InboxHealthy(s) => s.insert(conn),
            Self::InboxCorpusMatch(s) => s.insert(conn),
            Self::CorruptFile(s) => s.insert(conn),
            Self::MtimeOnlyMismatch(s) => s.insert(conn),
            Self::MissingDirectory(s) => s.insert(conn),
            Self::MissingFile(s) => s.insert(conn),
            Self::MovedFile(s) => s.insert(conn),
            Self::ShitFormat(s) => s.insert(conn),
            Self::DeployReady(s) => s.insert(conn),
            Self::DeployedHealthy(s) => s.insert(conn),
            Self::SidecarDeployReady(s) => s.insert(conn),
            Self::OutOfBandTagSync(s) => s.insert(conn),
            Self::OutOfBandTagConflict(s) => s.insert(conn),
            Self::SubparDuplicate(s) => s.insert(conn),
            Self::CompoundTag(s) => s.insert(conn),
            Self::PathTagMismatch(s) => s.insert(conn),
            Self::ExternalMatch(s) => s.insert(conn),
            Self::ReleasePacking(s) => s.insert(conn),
            Self::UnmatchedCorpusTrack(s) => s.insert(conn),
            Self::CanonicalTag(s) => s.insert(conn),
            Self::LibraryLeftover(s) => s.insert(conn),
            Self::LibraryStale(s) => s.insert(conn),
            Self::FingerprintOverlap(s) => s.insert(conn),
            Self::MetadataDuplicate(s) => s.insert(conn),
            Self::DuplicateInode(s) => s.insert(conn),
            Self::MissingTag(s) => s.insert(conn),
            Self::DeployConflict(s) => s.insert(conn),
            Self::SidecarDeployConflict(s) => s.insert(conn),
            Self::TagCanonicity(s) => s.insert(conn),
            Self::InconsistentAlbumArtist(s) => s.insert(conn),
            Self::CrossSourceOverlap(s) => s.insert(conn),
            Self::ReleaseOverlap(s) => s.insert(conn),
            Self::RedundantDuplicate(s) => s.insert(conn),
            Self::ExpectedOverlap(s) => s.insert(conn),
            Self::ExpectedDuplicate(s) => s.insert(conn),
            Self::MissingAlbumSingle(s) => s.insert(conn),
            Self::ExpectedMissingTag(s) => s.insert(conn),
            Self::InboxTagCanonicity(s) => s.insert(conn),
            Self::InboxMissingTag(s) => s.insert(conn),
            Self::InboxCompoundTag(s) => s.insert(conn),
            Self::DiscExtraction(s) => s.insert(conn),
            Self::UnfilledReleaseSlot(s) => s.insert(conn),
            Self::NearMissRelease(s) => s.insert(conn),
        }
    }

    /// Check if this signal already exists in its typed table.
    pub fn exists(&self, conn: &rusqlite::Connection) -> bool {
        use crate::meta::signals::store::{AggregateSignalStore, CorpusSignalStore};
        let result = match self {
            Self::FileInCorpus(s) => FileInCorpusSignal::exists(conn, s.inode),
            Self::UnindexedFile(s) => UnindexedFileSignal::exists(conn, s.inode),
            Self::HealthyFile(s) => HealthyFileSignal::exists(conn, s.inode),
            Self::FileInInbox(s) => FileInInboxSignal::exists(conn, s.inode),
            Self::InboxUnindexed(s) => InboxUnindexedSignal::exists(conn, s.inode),
            Self::InboxHealthy(s) => InboxHealthySignal::exists(conn, s.inode),
            Self::InboxCorpusMatch(s) => InboxCorpusMatchSignal::exists(conn, s.inode),
            Self::CorruptFile(s) => CorruptFileSignal::exists(conn, s.inode),
            Self::MtimeOnlyMismatch(s) => MtimeOnlyMismatchSignal::exists(conn, s.inode),
            Self::MissingDirectory(s) => MissingDirectorySignal::exists(conn, s.inode),
            Self::MissingFile(s) => MissingFileSignal::exists(conn, s.inode),
            Self::MovedFile(s) => MovedFileSignal::exists(conn, s.inode),
            Self::ShitFormat(s) => ShitFormatSignal::exists(conn, s.inode),
            Self::DeployReady(s) => DeployReadySignal::exists(conn, s.inode),
            Self::DeployedHealthy(s) => DeployedHealthySignal::exists(conn, s.inode),
            Self::SidecarDeployReady(s) => SidecarDeployReadySignal::exists(conn, s.inode),
            Self::OutOfBandTagSync(s) => OutOfBandTagSyncSignal::exists(conn, s.inode),
            Self::OutOfBandTagConflict(s) => OutOfBandTagConflictSignal::exists(conn, s.inode),
            Self::SubparDuplicate(s) => SubparDuplicateSignal::exists(conn, s.inode),
            Self::CompoundTag(s) => CompoundTagSignal::exists(conn, s.inode),
            Self::PathTagMismatch(s) => PathTagMismatchSignal::exists(conn, s.inode),
            Self::ExternalMatch(s) => ExternalMatchSignal::exists(conn, s.inode),
            Self::ReleasePacking(s) => ReleasePackingSignal::exists(conn, s.inode),
            Self::UnmatchedCorpusTrack(s) => UnmatchedCorpusTrackSignal::exists(conn, s.inode),
            Self::CanonicalTag(s) => CanonicalTagSignal::exists(conn, &s.key),
            Self::LibraryLeftover(s) => LibraryLeftoverSignal::exists(conn, &s.key),
            Self::LibraryStale(s) => LibraryStaleSignal::exists(conn, &s.key),
            Self::FingerprintOverlap(s) => FingerprintOverlapSignal::exists(conn, &s.key),
            Self::MetadataDuplicate(s) => MetadataDuplicateSignal::exists(conn, &s.key),
            Self::DuplicateInode(s) => DuplicateInodeSignal::exists(conn, &s.key),
            Self::MissingTag(s) => MissingTagSignal::exists(conn, &s.key),
            Self::DeployConflict(s) => DeployConflictSignal::exists(conn, &s.key),
            Self::SidecarDeployConflict(s) => SidecarDeployConflictSignal::exists(conn, &s.key),
            Self::TagCanonicity(s) => TagCanonicitySignal::exists(conn, &s.key),
            Self::InconsistentAlbumArtist(s) => InconsistentAlbumArtistSignal::exists(conn, &s.key),
            Self::CrossSourceOverlap(s) => CrossSourceOverlapSignal::exists(conn, &s.key),
            Self::ReleaseOverlap(s) => ReleaseOverlapSignal::exists(conn, &s.key),
            Self::RedundantDuplicate(s) => RedundantDuplicateSignal::exists(conn, &s.key),
            Self::ExpectedOverlap(s) => ExpectedOverlapSignal::exists(conn, &s.key),
            Self::ExpectedDuplicate(s) => ExpectedDuplicateSignal::exists(conn, &s.key),
            Self::MissingAlbumSingle(s) => MissingAlbumSingleSignal::exists(conn, &s.key),
            Self::ExpectedMissingTag(s) => ExpectedMissingTagSignal::exists(conn, s.inode),
            Self::InboxTagCanonicity(s) => InboxTagCanonicitySignal::exists(conn, &s.key),
            Self::InboxMissingTag(s) => InboxMissingTagSignal::exists(conn, &s.key),
            Self::InboxCompoundTag(s) => InboxCompoundTagSignal::exists(conn, s.inode),
            Self::DiscExtraction(s) => DiscExtractionSignal::exists(conn, &s.key),
            Self::UnfilledReleaseSlot(s) => UnfilledReleaseSlotSignal::exists(conn, &s.key),
            Self::NearMissRelease(s) => NearMissReleaseSignal::exists(conn, &s.key),
        };
        result.unwrap_or(false)
    }

    /// Compute a content hash for change detection.
    ///
    /// For BLOB variants: bincode-serializes the data and hashes the bytes.
    /// For scalar-only variants: hashes the non-PK fields.
    /// Includes the enum discriminant for type safety.
    pub fn content_hash(&self) -> u64 {
        let mut hasher = std::hash::DefaultHasher::new();
        // Hash the discriminant
        std::mem::discriminant(self).hash(&mut hasher);
        match self {
            // BLOB corpus signals — hash serialized data
            Self::InboxCorpusMatch(s) => {
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::OutOfBandTagSync(s) => {
                if let Ok(bytes) = bincode::serialize(&s.mismatches) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::OutOfBandTagConflict(s) => {
                if let Ok(bytes) = bincode::serialize(&s.mismatches) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::SubparDuplicate(s) => {
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::CompoundTag(s) => {
                if let Ok(bytes) = bincode::serialize(&s.compounds) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::PathTagMismatch(s) => {
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::ExternalMatch(s) => {
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::ReleasePacking(s) => {
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::UnmatchedCorpusTrack(s) => {
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
            // BLOB aggregate signals — hash serialized data
            Self::FingerprintOverlap(s) => {
                if let Ok(bytes) = bincode::serialize(&s.inodes) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::MetadataDuplicate(s) => {
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::DuplicateInode(s) => {
                if let Ok(bytes) = bincode::serialize(&s.inodes) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::MissingTag(s) => {
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::MissingAlbumSingle(s) => {
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::DeployConflict(s) => {
                if let Ok(bytes) = bincode::serialize(&s.inodes) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::SidecarDeployConflict(s) => {
                if let Ok(bytes) = bincode::serialize(&s.inodes) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::TagCanonicity(s) => {
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::InconsistentAlbumArtist(s) => {
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::CrossSourceOverlap(s) => {
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::ReleaseOverlap(s) => {
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::RedundantDuplicate(s) => {
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::InboxTagCanonicity(s) => {
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::InboxMissingTag(s) => {
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::InboxCompoundTag(s) => {
                if let Ok(bytes) = bincode::serialize(&s.compounds) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::DiscExtraction(s) => {
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
            // Scalar corpus signals — hash non-PK fields
            Self::FileInCorpus(s) => s.path.hash(&mut hasher),
            Self::UnindexedFile(s) => s.path.hash(&mut hasher),
            Self::HealthyFile(s) => s.path.hash(&mut hasher),
            Self::FileInInbox(s) => s.path.hash(&mut hasher),
            Self::InboxUnindexed(s) => s.path.hash(&mut hasher),
            Self::InboxHealthy(s) => s.path.hash(&mut hasher),
            Self::CorruptFile(s) => s.path.hash(&mut hasher),
            Self::MtimeOnlyMismatch(s) => s.path.hash(&mut hasher),
            Self::MissingDirectory(s) => s.path.hash(&mut hasher),
            Self::MissingFile(s) => {
                s.path.hash(&mut hasher);
                s.replaced_by_inode.hash(&mut hasher);
            }
            Self::MovedFile(s) => {
                s.path.hash(&mut hasher);
                s.old_path.hash(&mut hasher);
                s.old_zone.hash(&mut hasher);
                s.new_zone.hash(&mut hasher);
            }
            Self::ShitFormat(s) => {
                s.path.hash(&mut hasher);
                s.file_type.hash(&mut hasher);
            }
            Self::DeployReady(s) => {
                s.path.hash(&mut hasher);
                s.deploy_path.hash(&mut hasher);
            }
            Self::DeployedHealthy(s) => {
                s.path.hash(&mut hasher);
                s.library_path.hash(&mut hasher);
            }
            Self::SidecarDeployReady(s) => {
                s.path.hash(&mut hasher);
                s.deploy_path.hash(&mut hasher);
                s.library_name.hash(&mut hasher);
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::ExpectedMissingTag(_) => {} // inode-only, no extra fields
            // Scalar aggregate signals — hash non-PK fields
            Self::CanonicalTag(s) => {
                s.tag_name.hash(&mut hasher);
                s.canonical_value.hash(&mut hasher);
            }
            Self::LibraryLeftover(_) => {} // key-only, no extra fields
            Self::LibraryStale(s) => {
                s.library_path.hash(&mut hasher);
                s.expected_path.hash(&mut hasher);
                s.corpus_path.hash(&mut hasher);
                s.inode.hash(&mut hasher);
            }
            Self::ExpectedOverlap(s) => {
                s.source_a.hash(&mut hasher);
                s.source_b.hash(&mut hasher);
            }
            Self::ExpectedDuplicate(_) => {} // key + created_at only
            Self::UnfilledReleaseSlot(s) => {
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
            Self::NearMissRelease(s) => {
                if let Ok(bytes) = bincode::serialize(&s.data) {
                    bytes.hash(&mut hasher);
                }
            }
        }
        hasher.finish()
    }
}
