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
// Shared Macros (must be defined before `mod` declarations)
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

/// Generate `make_key` and `key_prefix_for_library` for library-namespaced signal keys.
macro_rules! impl_library_keyed {
    ($ty:ty, $prefix:literal) => {
        impl $ty {
            const KEY_PREFIX: &'static str = $prefix;

            pub fn make_key(library_name: &str, library_path: &str) -> String {
                format!("{}:{}:{}", Self::KEY_PREFIX, library_name, library_path)
            }

            pub fn key_prefix_for_library(library_name: &str) -> String {
                format!("{}:{}:", Self::KEY_PREFIX, library_name)
            }
        }
    };
}

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
// SignalContentHash implementations
// ============================================================================

use super::registry::SignalContentHash;

// Corpus signals — scalar only
impl_content_hash!(FileInCorpusSignal => [path]);
impl_content_hash!(UnindexedFileSignal => [path]);
impl_content_hash!(HealthyFileSignal => [path]);
impl_content_hash!(CorruptFileSignal => [path]);
impl_content_hash!(MtimeOnlyMismatchSignal => [path]);
impl_content_hash!(MissingDirectorySignal => [path]);
impl_content_hash!(MissingFileSignal => [path, replaced_by_inode]);
impl_content_hash!(MovedFileSignal => [path, old_path, old_zone, new_zone]);
impl_content_hash!(ShitFormatSignal => [path, file_type]);
impl_content_hash!(ExpectedMissingTagSignal => []);

// Corpus signals — blob
impl_content_hash!(OutOfBandTagSyncSignal => blob(mismatches));
impl_content_hash!(OutOfBandTagConflictSignal => blob(mismatches));
impl_content_hash!(SubparDuplicateSignal => blob(data));
impl_content_hash!(CompoundTagSignal => blob(compounds));
impl_content_hash!(PathTagMismatchSignal => blob(data));
impl_content_hash!(ExternalMatchSignal => blob(data));

// Aggregate signals — scalar only
impl_content_hash!(CanonicalTagSignal => [tag_name, canonical_value]);
impl_content_hash!(ExpectedOverlapSignal => [source_a, source_b]);
impl_content_hash!(ExpectedDuplicateSignal => []);

// Aggregate signals — blob
impl_content_hash!(FingerprintOverlapSignal => blob(inodes));
impl_content_hash!(MetadataDuplicateSignal => blob(data));
impl_content_hash!(DuplicateInodeSignal => blob(inodes));
impl_content_hash!(MissingTagSignal => blob(data));
impl_content_hash!(MissingAlbumSingleSignal => blob(data));
impl_content_hash!(TagCanonicitySignal => blob(data));
impl_content_hash!(InconsistentAlbumArtistSignal => blob(data));
impl_content_hash!(CrossSourceOverlapSignal => blob(data));
impl_content_hash!(ReleaseOverlapSignal => blob(data));
impl_content_hash!(RedundantDuplicateSignal => blob(data));
impl_content_hash!(DiscExtractionSignal => blob(data));

// TypedSignalWrite is now generated by signal_registry! in registry.rs
