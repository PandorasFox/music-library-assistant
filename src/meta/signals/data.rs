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

// ============================================================================
// Corpus File Signals (inode-keyed)
// ============================================================================

// --- Simple signals (all flat columns, no BLOB needed) ---

/// File exists in corpus directory.
/// Emitted during corpus walk (Asleep phase).
#[derive(Debug, Clone)]
pub struct FileInCorpusSignal {
    pub inode: i64,
    pub path: String,
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
#[derive(Debug, Clone)]
pub struct MovedFileSignal {
    pub inode: i64,
    pub path: String,
    pub old_path: String,
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
    pub quality_score: i32,
    pub superior_quality_score: i32,
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

/// Library file without corpus backing.
#[derive(Debug, Clone)]
pub struct LibraryLeftoverSignal {
    pub key: String,
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

/// Tag value contains separator characters needing to be split.
#[derive(Debug, Clone)]
pub struct CompoundTagValueSignal {
    pub key: String,
    pub tag_name: String,
    /// Serialized as bincode BLOB.
    pub data: CompoundTagValueData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompoundTagValueData {
    pub compound_value: String,
    pub split_parts: Vec<String>,
    pub separator: String,
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
    CorruptFile(CorruptFileSignal),
    MtimeOnlyMismatch(MtimeOnlyMismatchSignal),
    MissingDirectory(MissingDirectorySignal),
    MissingFile(MissingFileSignal),
    MovedFile(MovedFileSignal),
    ShitFormat(ShitFormatSignal),
    DeployReady(DeployReadySignal),
    DeployedHealthy(DeployedHealthySignal),
    OutOfBandTagSync(OutOfBandTagSyncSignal),
    OutOfBandTagConflict(OutOfBandTagConflictSignal),
    SubparDuplicate(SubparDuplicateSignal),
    CompoundTag(CompoundTagSignal),
    // Aggregate signals (semantic-keyed)
    CanonicalTag(CanonicalTagSignal),
    LibraryLeftover(LibraryLeftoverSignal),
    LibraryStale(LibraryStaleSignal),
    FingerprintOverlap(FingerprintOverlapSignal),
    MetadataDuplicate(MetadataDuplicateSignal),
    DuplicateInode(DuplicateInodeSignal),
    MissingTag(MissingTagSignal),
    DeployConflict(DeployConflictSignal),
    TagCanonicity(TagCanonicitySignal),
    InconsistentAlbumArtist(InconsistentAlbumArtistSignal),
    CompoundTagValue(CompoundTagValueSignal),
    CrossSourceOverlap(CrossSourceOverlapSignal),
}

impl TypedSignalWrite {
    /// Construct a TypedSignalWrite for simple corpus signals (inode + path only).
    ///
    /// Panics if the signal type requires extra fields (e.g., ShitFormat, MovedFile).
    /// Those must be constructed directly with their full data.
    pub fn simple_corpus(signal_type: crate::meta::signals::types::CorpusFileSignalType, inode: i64, path: String) -> Self {
        use crate::meta::signals::types::CorpusFileSignalType;
        match signal_type {
            CorpusFileSignalType::FileInCorpus => Self::FileInCorpus(FileInCorpusSignal { inode, path }),
            CorpusFileSignalType::UnindexedFile => Self::UnindexedFile(UnindexedFileSignal { inode, path }),
            CorpusFileSignalType::HealthyFile => Self::HealthyFile(HealthyFileSignal { inode, path }),
            CorpusFileSignalType::MissingFile => Self::MissingFile(MissingFileSignal { inode, path, replaced_by_inode: None }),
            CorpusFileSignalType::MissingDirectory => Self::MissingDirectory(MissingDirectorySignal { inode, path }),
            CorpusFileSignalType::CorruptFile => Self::CorruptFile(CorruptFileSignal { inode, path }),
            CorpusFileSignalType::MtimeOnlyMismatch => Self::MtimeOnlyMismatch(MtimeOnlyMismatchSignal { inode, path }),
            // These signal types require extra data — cannot be constructed from (inode, path) alone
            CorpusFileSignalType::MovedFile
            | CorpusFileSignalType::ShitFormat
            | CorpusFileSignalType::SubparDuplicate
            | CorpusFileSignalType::OutOfBandTagSync
            | CorpusFileSignalType::OutOfBandTagConflict
            | CorpusFileSignalType::CompoundTag
            | CorpusFileSignalType::DeployReady
            | CorpusFileSignalType::DeployedHealthy => {
                panic!("TypedSignalWrite::simple_corpus called with signal type {:?} that requires extra data", signal_type)
            }
        }
    }

    /// Insert this signal into its typed table.
    pub fn insert(self, conn: &rusqlite::Connection) -> rusqlite::Result<()> {
        use crate::meta::signals::store::{AggregateSignalStore, CorpusSignalStore};
        match self {
            Self::FileInCorpus(s) => s.insert(conn),
            Self::UnindexedFile(s) => s.insert(conn),
            Self::HealthyFile(s) => s.insert(conn),
            Self::CorruptFile(s) => s.insert(conn),
            Self::MtimeOnlyMismatch(s) => s.insert(conn),
            Self::MissingDirectory(s) => s.insert(conn),
            Self::MissingFile(s) => s.insert(conn),
            Self::MovedFile(s) => s.insert(conn),
            Self::ShitFormat(s) => s.insert(conn),
            Self::DeployReady(s) => s.insert(conn),
            Self::DeployedHealthy(s) => s.insert(conn),
            Self::OutOfBandTagSync(s) => s.insert(conn),
            Self::OutOfBandTagConflict(s) => s.insert(conn),
            Self::SubparDuplicate(s) => s.insert(conn),
            Self::CompoundTag(s) => s.insert(conn),
            Self::CanonicalTag(s) => s.insert(conn),
            Self::LibraryLeftover(s) => s.insert(conn),
            Self::LibraryStale(s) => s.insert(conn),
            Self::FingerprintOverlap(s) => s.insert(conn),
            Self::MetadataDuplicate(s) => s.insert(conn),
            Self::DuplicateInode(s) => s.insert(conn),
            Self::MissingTag(s) => s.insert(conn),
            Self::DeployConflict(s) => s.insert(conn),
            Self::TagCanonicity(s) => s.insert(conn),
            Self::InconsistentAlbumArtist(s) => s.insert(conn),
            Self::CompoundTagValue(s) => s.insert(conn),
            Self::CrossSourceOverlap(s) => s.insert(conn),
        }
    }
}
