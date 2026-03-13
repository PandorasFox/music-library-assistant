//! Pure data types for signal BLOB payloads.
//!
//! These types are `Serialize`/`Deserialize` and contain no server-side
//! dependencies. Signal wrapper structs live in mm's `meta::signals::data`.

use serde::{Deserialize, Serialize};

// ============================================================================
// Shared Macros
// ============================================================================

/// Generate an `as_str`-style method mapping enum variants to `&'static str`.
macro_rules! impl_as_str {
    ($ty:ty, $method:ident, [$($variant:ident => $s:literal),+ $(,)?]) => {
        impl $ty {
            pub fn $method(&self) -> &'static str {
                match self { $(Self::$variant => $s,)+ }
            }
        }
    };
}

// Make macro available to submodules
pub(crate) use impl_as_str;

// ============================================================================
// Submodules
// ============================================================================

mod deploy;
mod inbox;
mod release_packing;

pub use deploy::*;
pub use inbox::*;
pub use release_packing::*;

// ============================================================================
// Tag Mismatch Data (OOB signals)
// ============================================================================

/// A single tag mismatch between disk and DB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagMismatchEntry {
    pub tag_name: String,
    pub disk_value: Option<String>,
    pub db_value: Option<String>,
}

// ============================================================================
// Subpar Duplicate Data
// ============================================================================

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

// ============================================================================
// Compound Tag Data
// ============================================================================

/// A single compound tag value detected in a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompoundTagEntry {
    pub tag_name: String,
    pub compound_value: String,
    pub split_parts: Vec<String>,
    pub separator: String,
    pub matching_parts: Vec<String>,
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

/// A group of inodes sharing the same compound tag value.
/// Used to aggregate compound split resolution by value rather than per-file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompoundGroup {
    pub tag_name: String,
    pub compound_value: String,
    pub inodes: Vec<i64>,
}

// ============================================================================
// Path-Tag Mismatch Data
// ============================================================================

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

// ============================================================================
// External Match Data
// ============================================================================

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
// Packing Score Breakdown
// ============================================================================

/// Breakdown of the composite packing score.
///
/// Each tag dimension (title, artist, album) is stored independently rather
/// than as a single composite, so weights can zero out dimensions that are
/// unreliable in certain scoring contexts (e.g., artist in elimination).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
// Aggregate Signal Data Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataDuplicateData {
    pub tag_signature: String,
    pub inodes: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingTagData {
    pub missing_tags: Vec<String>,
    pub inodes: Vec<i64>,
}

/// Bincode-serialized payload for TagCanonicity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagCanonicityData {
    /// (variant_value, count) pairs sorted by count DESC.
    pub variants: Vec<(String, usize)>,
    pub inodes: Vec<i64>,
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

/// A pair of tracks from different sources that share a fingerprint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossSourceTrackPair {
    pub fingerprint_key: String,
    pub source_a_inode: i64,
    pub source_a_path: String,
    pub source_b_inode: i64,
    pub source_b_path: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedundantDuplicateData {
    pub file_type: String, // shared format (e.g. "flac")
    pub inodes: Vec<i64>,
    pub paths: Vec<String>, // parallel to inodes
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
