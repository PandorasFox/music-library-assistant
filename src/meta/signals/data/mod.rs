//! Typed signal data structs.
//!
//! One struct per signal type, replacing untyped `metadata_json` blobs.
//!
//! ## Conventions
//!
//! - **Top-level signal structs** map 1:1 to SQL table columns. They are NOT
//!   `Serialize`/`Deserialize` — serialization happens at the column level.
//! - **Inner `*Data` structs** (for collection data stored as bincode BLOBs)
//!   are re-exported from mm-meta. These are the BLOB payload types.
//! - Corpus file signals are keyed by `inode INTEGER PRIMARY KEY`.
//! - Aggregate signals are keyed by `key TEXT PRIMARY KEY`.

use std::hash::Hash;

// Re-export all pure data types from mm-meta
pub use mm_meta::signals::data::{
    // Core data types
    TagMismatchEntry, SubparDuplicateData, CompoundTagEntry, CompoundGroup,
    PathTagMismatchData, PathMismatchKind, PathTagValueMismatch,
    ExternalMatchData, MatchClassification, ExternalTagDiff,
    PackingScoreBreakdown,
    MetadataDuplicateData, MissingTagData, TagCanonicityData,
    InconsistentAlbumArtistData,
    CrossSourceOverlapData, CrossSourceTrackPair,
    ReleaseOverlapData, ReleaseOverlapEntry,
    RedundantDuplicateData,
    CrossReleaseRecordingData, CrossReleaseEntry,
    MissingAlbumSingleData, SingleTrackInfo,
    DiscExtractionData, DiscExtractionSource, TrackNumberExtraction,
    // Deploy data types
    SidecarDeployReadyData, DeployLifecyclePhase,
    // Inbox data types
    CorpusMatchQuality, InboxCorpusMatchData, InboxCorpusMatch, InboxTagCanonicityData,
    // Release packing data types
    MatchMethod, ReleasePackingData, UnsolvedCategory,
    UnmatchedCorpusTrackData, UnfilledReleaseSlotData,
    PackedReleaseCategory, PackedReleaseData,
    PackingKnotData, KnotClassification, KnotProposalEntry, KnotAssignment,
    AlternativeReleasePackingData,
    VariousArtistsOverrideData, VariousArtistsOverrideSource,
    PinnedReleaseConflictData,
};

// ============================================================================
// Shared Macros (must be defined before `mod` declarations)
// ============================================================================

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

/// Track is a subpar duplicate (lower quality version of another track).
#[derive(Debug, Clone)]
pub struct SubparDuplicateSignal {
    pub inode: i64,
    pub path: String,
    /// Serialized as bincode BLOB.
    pub data: SubparDuplicateData,
}

/// Per-file compound tag detection results.
#[derive(Debug, Clone)]
pub struct CompoundTagSignal {
    pub inode: i64,
    pub path: String,
    /// Serialized as bincode BLOB.
    pub compounds: Vec<CompoundTagEntry>,
}

/// File's path disagrees with its tags according to a configured path-tag schema.
#[derive(Debug, Clone)]
pub struct PathTagMismatchSignal {
    pub inode: i64,
    pub path: String,
    /// Serialized as bincode BLOB.
    pub data: PathTagMismatchData,
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

/// Tag value collision needing canonicalization.
#[derive(Debug, Clone)]
pub struct TagCanonicitySignal {
    pub key: String,
    pub tag_name: String,
    /// Serialized as bincode BLOB.
    pub data: TagCanonicityData,
}

/// Album has tracks with different artists + missing/inconsistent album_artist.
#[derive(Debug, Clone)]
pub struct InconsistentAlbumArtistSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: InconsistentAlbumArtistData,
}

/// Cross-source fingerprint overlap cluster.
#[derive(Debug, Clone)]
pub struct CrossSourceOverlapSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: CrossSourceOverlapData,
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

/// Group of files with identical fingerprints and equivalent quality.
/// Neither file is subpar — requires operator choice.
#[derive(Debug, Clone)]
pub struct RedundantDuplicateSignal {
    pub key: String, // fingerprint text (same key space as FingerprintOverlapSignal)
    pub data: RedundantDuplicateData,
}

/// Tracks missing an ALBUM tag but having ARTIST and TITLE (album-less singles).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MissingAlbumSingleSignal {
    pub key: String, // lowercased artist name for dedup
    /// Serialized as bincode BLOB.
    pub data: MissingAlbumSingleData,
}

/// Disc value extractable from an existing tag (ALBUM or TRACKNUMBER).
/// Aggregate signal keyed by source-specific prefix + grouping key.
#[derive(Debug, Clone)]
pub struct DiscExtractionSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: DiscExtractionData,
}

/// Same MusicBrainz recording appearing on different releases.
/// Aggregate signal keyed by MB recording MBID.
#[derive(Debug, Clone)]
pub struct CrossReleaseRecordingSignal {
    pub key: String, // MB recording MBID
    /// Serialized as bincode BLOB.
    pub data: CrossReleaseRecordingData,
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
impl_content_hash!(CrossReleaseRecordingSignal => blob(data));

// TypedSignalWrite is now generated by signal_registry! in registry.rs
