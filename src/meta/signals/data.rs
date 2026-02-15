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

/// Directory contains sidecar album art embeddable into artless audio files.
#[derive(Debug, Clone)]
pub struct EmbeddableAlbumArtSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: EmbeddableAlbumArtData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddableAlbumArtData {
    /// Absolute path to sidecar image file.
    pub image_path: String,
    /// Display name (e.g. "cover.jpg").
    pub image_filename: String,
    /// Inodes of audio files lacking embedded art.
    pub artless_inodes: Vec<i64>,
    /// Corpus-relative paths (parallel to artless_inodes).
    pub artless_paths: Vec<String>,
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
    CrossSourceOverlap(CrossSourceOverlapSignal),
    RedundantDuplicate(RedundantDuplicateSignal),
    EmbeddableAlbumArt(EmbeddableAlbumArtSignal),
    ExpectedOverlap(ExpectedOverlapSignal),
    ExpectedDuplicate(ExpectedDuplicateSignal),
    MissingAlbumSingle(MissingAlbumSingleSignal),
    ExpectedMissingTag(ExpectedMissingTagSignal),
}

impl TypedSignalWrite {
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
            Self::CrossSourceOverlap(s) => s.insert(conn),
            Self::RedundantDuplicate(s) => s.insert(conn),
            Self::EmbeddableAlbumArt(s) => s.insert(conn),
            Self::ExpectedOverlap(s) => s.insert(conn),
            Self::ExpectedDuplicate(s) => s.insert(conn),
            Self::MissingAlbumSingle(s) => s.insert(conn),
            Self::ExpectedMissingTag(s) => s.insert(conn),
        }
    }

    /// Check if this signal already exists in its typed table.
    pub fn exists(&self, conn: &rusqlite::Connection) -> bool {
        use crate::meta::signals::store::{AggregateSignalStore, CorpusSignalStore};
        let result = match self {
            Self::FileInCorpus(s) => FileInCorpusSignal::exists(conn, s.inode),
            Self::UnindexedFile(s) => UnindexedFileSignal::exists(conn, s.inode),
            Self::HealthyFile(s) => HealthyFileSignal::exists(conn, s.inode),
            Self::CorruptFile(s) => CorruptFileSignal::exists(conn, s.inode),
            Self::MtimeOnlyMismatch(s) => MtimeOnlyMismatchSignal::exists(conn, s.inode),
            Self::MissingDirectory(s) => MissingDirectorySignal::exists(conn, s.inode),
            Self::MissingFile(s) => MissingFileSignal::exists(conn, s.inode),
            Self::MovedFile(s) => MovedFileSignal::exists(conn, s.inode),
            Self::ShitFormat(s) => ShitFormatSignal::exists(conn, s.inode),
            Self::DeployReady(s) => DeployReadySignal::exists(conn, s.inode),
            Self::DeployedHealthy(s) => DeployedHealthySignal::exists(conn, s.inode),
            Self::OutOfBandTagSync(s) => OutOfBandTagSyncSignal::exists(conn, s.inode),
            Self::OutOfBandTagConflict(s) => OutOfBandTagConflictSignal::exists(conn, s.inode),
            Self::SubparDuplicate(s) => SubparDuplicateSignal::exists(conn, s.inode),
            Self::CompoundTag(s) => CompoundTagSignal::exists(conn, s.inode),
            Self::CanonicalTag(s) => CanonicalTagSignal::exists(conn, &s.key),
            Self::LibraryLeftover(s) => LibraryLeftoverSignal::exists(conn, &s.key),
            Self::LibraryStale(s) => LibraryStaleSignal::exists(conn, &s.key),
            Self::FingerprintOverlap(s) => FingerprintOverlapSignal::exists(conn, &s.key),
            Self::MetadataDuplicate(s) => MetadataDuplicateSignal::exists(conn, &s.key),
            Self::DuplicateInode(s) => DuplicateInodeSignal::exists(conn, &s.key),
            Self::MissingTag(s) => MissingTagSignal::exists(conn, &s.key),
            Self::DeployConflict(s) => DeployConflictSignal::exists(conn, &s.key),
            Self::TagCanonicity(s) => TagCanonicitySignal::exists(conn, &s.key),
            Self::InconsistentAlbumArtist(s) => InconsistentAlbumArtistSignal::exists(conn, &s.key),
            Self::CrossSourceOverlap(s) => CrossSourceOverlapSignal::exists(conn, &s.key),
            Self::RedundantDuplicate(s) => RedundantDuplicateSignal::exists(conn, &s.key),
            Self::EmbeddableAlbumArt(s) => EmbeddableAlbumArtSignal::exists(conn, &s.key),
            Self::ExpectedOverlap(s) => ExpectedOverlapSignal::exists(conn, &s.key),
            Self::ExpectedDuplicate(s) => ExpectedDuplicateSignal::exists(conn, &s.key),
            Self::MissingAlbumSingle(s) => MissingAlbumSingleSignal::exists(conn, &s.key),
            Self::ExpectedMissingTag(s) => ExpectedMissingTagSignal::exists(conn, s.inode),
        };
        result.unwrap_or(false)
    }
}
