//! Core database types for track metadata and scan state.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Universal audio file representation.
/// Stored in the `tracks` table.
///
/// Contains ONLY file and audio waveform metadata.
/// Tag metadata (artist, title, album, etc.) is stored in `track_tags` table.
#[derive(Debug, Clone)]
pub struct Track {
    pub id: Option<i64>,
    pub path: String,
    pub source: String, // corpus/library name/legacy
    pub inode: i64,
    pub file_size: i64,
    pub file_type: String,             // flac, mp3, ogg, etc.
    pub duration_ms: Option<i64>,
    pub bitrate_kbps: Option<i32>,
    pub sample_rate: Option<i32>,
    /// Chromaprint acoustic fingerprint as raw u32 values.
    /// Stored in DB as BLOB (little-endian bytes).
    pub fingerprint: Option<Vec<u32>>,
    /// True when DB tags have changed but disk hasn't been updated yet.
    /// Used for recovery if interrupted between DB write and disk flush.
    pub needs_disk_flush: bool,
}

/// A single tag associated with a track.
/// Stored in the `track_tags` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackTag {
    pub track_id: i64,
    pub tag_name: String,
    pub tag_value: String,
}

/// Entry in the scan_state table for incremental scanning.
/// Tracks inode + mtime to detect file changes.
#[derive(Debug, Clone)]
pub struct ScanStateEntry {
    pub _source: String,
    pub inode: i64,
    pub _path: String,
    pub mtime_secs: i64,
    pub mtime_nanos: i64, // SQLite INTEGER is i64; cast to u32 at comparison time
    pub _file_size: i64,
}

/// Deployment statistics for corpus health tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentStats {
    pub total_corpus_files: usize,
    pub deployed_files: usize,
    pub deployment_percentage: f64,
    pub last_updated: String,
}

// ============================================================================
// Health Issue Types
// ============================================================================

/// Type of health signal detected in the corpus.
///
/// See [`docs/SIGNAL_REFERENCE.md`](../../../../docs/SIGNAL_REFERENCE.md) for the
/// canonical reference of what emits and clears each signal type.
///
/// **Any changes to signal semantics must be reflected in that document.**
///
/// Signals are organized into levels:
/// - **First-level**: Computed directly from corpus + index state (WalkCorpus, etc.)
/// - **Second-level**: Derived from comparing first-level signals
/// - **Third-level**: Triggered when files become healthy (e.g., deploy conflicts)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignalType {
    // =========================================================================
    // First-level signals (computed from corpus + index state)
    // =========================================================================
    /// File exists in corpus directory (discovered during WalkCorpus)
    FileInCorpus,

    // =========================================================================
    // Second-level signals (derived from first-level signals)
    // =========================================================================
    /// File in corpus but not in index (needs indexing)
    UnindexedFile,
    /// File in corpus + index with matching inode/mtime (healthy state)
    HealthyFile,
    /// File in index but mtime differs from disk (modified outside MLA)
    CorpusFileModifiedOutOfBand,
    /// File in index with different path but same inode (file was moved)
    MovedFile,
    /// File in index but no longer exists in corpus
    MissingFile,

    // =========================================================================
    // Third-level signals (computed for healthy files)
    // =========================================================================
    /// Multiple corpus files would deploy to the same library path
    DeployConflict,

    // =========================================================================
    // Library deployment health signals
    // =========================================================================
    /// Library file exists but deployed at wrong path (tags changed since deploy)
    /// issue_key: "library_stale:{library_name}:{library_path}"
    LibraryStale,
    /// Library file exists without corpus backing (leftover)
    /// issue_key: "library_leftover:{library_name}:{library_path}"
    LibraryLeftover,

    // =========================================================================
    // Content-level signals (tag and fingerprint analysis)
    // =========================================================================
    /// Same fingerprint across multiple files (internal overlap detection)
    FingerprintOverlap,
    /// Same metadata (artist/album/title) across multiple files
    MetadataDuplicate,
    /// Missing required tags (e.g., album_artist)
    MissingTag,
    /// Tags on disk have extras in one direction only (syncable)
    OutOfBandTagSync,
    /// Tags on disk conflict with indexed tags (value differences or mixed directions)
    OutOfBandTagConflict,
    /// File mtime changed but tags are identical (requires operator acknowledgement)
    MtimeOnlyMismatch,
    /// Multiple corpus entries share the same inode (hard links or DB inconsistency)
    DuplicateInode,
    /// File at path was replaced (different inode than indexed)
    /// issue_key: relative path, metadata: {"old_inode": i64, "new_inode": i64}
    InodeChanged,
    /// Tag value collision needing canonicalization
    TagCanonicity,
    /// Album has tracks with different artists + missing/inconsistent album_artist
    InconsistentAlbumArtist,
    /// Tag value contains separators that should be split into multiple values
    CompoundTagValue,

    // =========================================================================
    // Error signals (discovery-time parse/read failures)
    // =========================================================================
    /// File's tags could not be parsed (corrupt/unsupported tag format)
    TagParseError,
    /// File's audio waveform could not be decoded for fingerprinting
    WaveformReadError,
    /// File is corrupt (unreadable tags or waveform decode failure)
    /// Actionable signal with resolution flow (stash + drop)
    CorruptFile,
    /// File is in a non-Vorbis container format (MP3, M4A, AAC, WMA, or lossless needing remux)
    /// Actionable signal with resolution flow (transcode to Opus/FLAC)
    ShitFormat,
    /// Track is an subpar duplicate (lower quality version of another track)
    /// Actionable signal with resolution flow (stash subpar copy)
    SubparDuplicate,
}

impl SignalType {
    pub fn as_str(&self) -> &'static str {
        match self {
            // First-level signals
            Self::FileInCorpus => "file_in_corpus",

            // Second-level signals
            Self::UnindexedFile => "unindexed_file",
            Self::HealthyFile => "healthy_file",
            Self::CorpusFileModifiedOutOfBand => "corpus_file_modified_oob",
            Self::MovedFile => "moved_file",
            Self::MissingFile => "missing_file",

            // Third-level signals
            Self::DeployConflict => "deploy_conflict",

            // Library deployment health signals
            Self::LibraryStale => "library_stale",
            Self::LibraryLeftover => "library_leftover",

            // Content-level signals
            Self::FingerprintOverlap => "fingerprint_dup",
            Self::MetadataDuplicate => "metadata_dup",
            Self::MissingTag => "missing_tag",
            Self::OutOfBandTagSync => "oob_tag_sync",
            Self::OutOfBandTagConflict => "oob_tag_conflict",
            Self::MtimeOnlyMismatch => "mtime_only_mismatch",
            Self::DuplicateInode => "duplicate_inode",
            Self::InodeChanged => "inode_changed",
            Self::TagCanonicity => "tag_canonicity",
            Self::InconsistentAlbumArtist => "inconsistent_album_artist",
            Self::CompoundTagValue => "compound_tag_value",

            // Error signals
            Self::TagParseError => "tag_parse_error",
            Self::WaveformReadError => "waveform_read_error",
            Self::CorruptFile => "corrupt_file",
            Self::ShitFormat => "shit_format",
            Self::SubparDuplicate => "subpar_duplicate",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            // First-level signals
            "file_in_corpus" => Some(Self::FileInCorpus),

            // Second-level signals
            "unindexed_file" => Some(Self::UnindexedFile),
            "healthy_file" => Some(Self::HealthyFile),
            "corpus_file_modified_oob" => Some(Self::CorpusFileModifiedOutOfBand),
            "moved_file" => Some(Self::MovedFile),
            "missing_file" => Some(Self::MissingFile),

            // Third-level signals
            "deploy_conflict" => Some(Self::DeployConflict),

            // Library deployment health signals
            "library_stale" => Some(Self::LibraryStale),
            "library_leftover" => Some(Self::LibraryLeftover),

            // Content-level signals
            "fingerprint_dup" => Some(Self::FingerprintOverlap),
            "metadata_dup" => Some(Self::MetadataDuplicate),
            "missing_tag" => Some(Self::MissingTag),
            "oob_tag_sync" => Some(Self::OutOfBandTagSync),
            "oob_tag_conflict" => Some(Self::OutOfBandTagConflict),
            "mtime_only_mismatch" => Some(Self::MtimeOnlyMismatch),
            // Legacy: treat old "oob_tag" as conflict (conservative)
            "oob_tag" => Some(Self::OutOfBandTagConflict),
            "duplicate_inode" => Some(Self::DuplicateInode),
            "inode_changed" => Some(Self::InodeChanged),
            "tag_canonicity" => Some(Self::TagCanonicity),
            "inconsistent_album_artist" => Some(Self::InconsistentAlbumArtist),
            "compound_tag_value" => Some(Self::CompoundTagValue),
            "tag_parse_error" => Some(Self::TagParseError),
            "waveform_read_error" => Some(Self::WaveformReadError),
            "corrupt_file" => Some(Self::CorruptFile),
            "shit_format" => Some(Self::ShitFormat),
            "subpar_duplicate" => Some(Self::SubparDuplicate),

            // Legacy DB values → map to new types
            "missing_from_disk" => Some(Self::MissingFile),
            "missing_from_index" => Some(Self::FileInCorpus),
            "file_relocated" => Some(Self::MovedFile),
            "oob_file_change" => Some(Self::CorpusFileModifiedOutOfBand),

            _ => None,
        }
    }
}

impl From<CorpusFileSignalType> for SignalType {
    fn from(t: CorpusFileSignalType) -> Self {
        match t {
            CorpusFileSignalType::FileInCorpus => Self::FileInCorpus,
            CorpusFileSignalType::UnindexedFile => Self::UnindexedFile,
            CorpusFileSignalType::HealthyFile => Self::HealthyFile,
            CorpusFileSignalType::MissingFile => Self::MissingFile,
            CorpusFileSignalType::CorpusFileModifiedOutOfBand => Self::CorpusFileModifiedOutOfBand,
            CorpusFileSignalType::MovedFile => Self::MovedFile,
            CorpusFileSignalType::OutOfBandTagSync => Self::OutOfBandTagSync,
            CorpusFileSignalType::OutOfBandTagConflict => Self::OutOfBandTagConflict,
            CorpusFileSignalType::MtimeOnlyMismatch => Self::MtimeOnlyMismatch,
            CorpusFileSignalType::TagParseError => Self::TagParseError,
            CorpusFileSignalType::WaveformReadError => Self::WaveformReadError,
            CorpusFileSignalType::InodeChanged => Self::InodeChanged,
            CorpusFileSignalType::CorruptFile => Self::CorruptFile,
            CorpusFileSignalType::ShitFormat => Self::ShitFormat,
            CorpusFileSignalType::SubparDuplicate => Self::SubparDuplicate,
        }
    }
}

impl From<LibraryFileSignalType> for SignalType {
    fn from(t: LibraryFileSignalType) -> Self {
        match t {
            LibraryFileSignalType::LibraryLeftover => Self::LibraryLeftover,
            LibraryFileSignalType::LibraryStale => Self::LibraryStale,
            // DeployReady/DeployedHealthy don't have SignalType equivalents
            // They're library-specific internal states
            LibraryFileSignalType::DeployReady | LibraryFileSignalType::DeployedHealthy => {
                panic!("DeployReady/DeployedHealthy cannot be converted to SignalType")
            }
        }
    }
}


// ============================================================================
// Signal Types (Type-Safe Signal System)
// ============================================================================

/// File-based health signal.
///
/// The path uniquely identifies the signal. No metadata needed.
#[derive(Debug, Clone)]
pub struct FileSignal {
    pub id: Option<i64>,
    pub signal_type: FileSignalType,
    pub path: String,
    pub discovered_at: Option<String>,
}

/// Types of corpus file signals.
///
/// These signals track the state of files in the corpus directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CorpusFileSignalType {
    /// File exists in corpus directory
    FileInCorpus,
    /// File in corpus but not in index
    UnindexedFile,
    /// File in corpus + index, healthy state
    HealthyFile,
    /// File in index but missing from corpus
    MissingFile,
    /// File mtime differs from indexed mtime
    CorpusFileModifiedOutOfBand,
    /// File moved (same inode, different path)
    MovedFile,
    /// Tags on disk have extras in one direction only (syncable)
    OutOfBandTagSync,
    /// Tags on disk conflict with indexed tags (value differences or mixed directions)
    OutOfBandTagConflict,
    /// File mtime changed but tags are identical (requires operator acknowledgement)
    MtimeOnlyMismatch,
    /// File's tags could not be parsed
    TagParseError,
    /// File's audio waveform could not be decoded for fingerprinting
    WaveformReadError,
    /// File at path was replaced (different inode than indexed)
    InodeChanged,
    /// File is corrupt (unreadable tags or waveform decode failure)
    CorruptFile,
    /// File is in a non-Vorbis container format (MP3, M4A, AAC, WMA, or lossless needing remux)
    ShitFormat,
    /// Track is an subpar duplicate (lower quality version of another track)
    SubparDuplicate,
}

impl CorpusFileSignalType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FileInCorpus => "file_in_corpus",
            Self::UnindexedFile => "unindexed_file",
            Self::HealthyFile => "healthy_file",
            Self::MissingFile => "missing_file",
            Self::CorpusFileModifiedOutOfBand => "corpus_file_modified_oob",
            Self::MovedFile => "moved_file",
            Self::OutOfBandTagSync => "oob_tag_sync",
            Self::OutOfBandTagConflict => "oob_tag_conflict",
            Self::MtimeOnlyMismatch => "mtime_only_mismatch",
            Self::TagParseError => "tag_parse_error",
            Self::WaveformReadError => "waveform_read_error",
            Self::InodeChanged => "inode_changed",
            Self::CorruptFile => "corrupt_file",
            Self::ShitFormat => "shit_format",
            Self::SubparDuplicate => "subpar_duplicate",
        }
    }

    pub fn to_signal_type(&self) -> SignalType {
        match self {
            Self::FileInCorpus => SignalType::FileInCorpus,
            Self::UnindexedFile => SignalType::UnindexedFile,
            Self::HealthyFile => SignalType::HealthyFile,
            Self::MissingFile => SignalType::MissingFile,
            Self::CorpusFileModifiedOutOfBand => SignalType::CorpusFileModifiedOutOfBand,
            Self::MovedFile => SignalType::MovedFile,
            Self::OutOfBandTagSync => SignalType::OutOfBandTagSync,
            Self::OutOfBandTagConflict => SignalType::OutOfBandTagConflict,
            Self::MtimeOnlyMismatch => SignalType::MtimeOnlyMismatch,
            Self::TagParseError => SignalType::TagParseError,
            Self::WaveformReadError => SignalType::WaveformReadError,
            Self::InodeChanged => SignalType::InodeChanged,
            Self::CorruptFile => SignalType::CorruptFile,
            Self::ShitFormat => SignalType::ShitFormat,
            Self::SubparDuplicate => SignalType::SubparDuplicate,
        }
    }
}

impl std::fmt::Display for CorpusFileSignalType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Types of library file signals.
///
/// These signals track the state of files in library directories (deployment targets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LibraryFileSignalType {
    /// Library file without corpus backing
    LibraryLeftover,
    /// Library file at wrong path (tags changed since deploy)
    LibraryStale,
    /// Healthy corpus file ready for deployment (not yet in any library)
    DeployReady,
    /// Healthy corpus file deployed at correct library path
    DeployedHealthy,
}

impl LibraryFileSignalType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LibraryLeftover => "library_leftover",
            Self::LibraryStale => "library_stale",
            Self::DeployReady => "deploy_ready",
            Self::DeployedHealthy => "deployed_healthy",
        }
    }

    pub fn to_signal_type(&self) -> SignalType {
        match self {
            Self::LibraryLeftover => SignalType::LibraryLeftover,
            Self::LibraryStale => SignalType::LibraryStale,
            // DeployReady/DeployedHealthy don't have SignalType equivalents
            Self::DeployReady | Self::DeployedHealthy => {
                panic!("DeployReady/DeployedHealthy should not use to_signal_type")
            }
        }
    }
}

impl std::fmt::Display for LibraryFileSignalType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Unified file signal type for database storage compatibility.
///
/// This enum wraps both corpus and library signal types for cases where
/// we need to handle both in a unified way (e.g., database queries).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileSignalType {
    Corpus(CorpusFileSignalType),
    Library(LibraryFileSignalType),
}

impl FileSignalType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Corpus(t) => t.as_str(),
            Self::Library(t) => t.as_str(),
        }
    }

    /// Convert to SignalType for DB queries.
    ///
    /// Panics for library types that don't have SignalType equivalents
    /// (DeployReady, DeployedHealthy).
    pub fn to_signal_type(&self) -> SignalType {
        match self {
            Self::Corpus(t) => t.to_signal_type(),
            Self::Library(t) => t.to_signal_type(),
        }
    }
}

impl std::fmt::Display for FileSignalType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl From<CorpusFileSignalType> for FileSignalType {
    fn from(t: CorpusFileSignalType) -> Self {
        Self::Corpus(t)
    }
}

impl From<LibraryFileSignalType> for FileSignalType {
    fn from(t: LibraryFileSignalType) -> Self {
        Self::Library(t)
    }
}

/// Aggregate health signal.
///
/// Groups multiple tracks under a common key. Metadata contains details.
#[derive(Debug, Clone)]
pub struct AggregateSignal {
    pub id: Option<i64>,
    pub signal_type: AggregateSignalType,
    pub key: String,
    pub discovered_at: Option<String>,
    pub metadata_json: Option<String>,
}

impl AggregateSignal {
    /// Get track IDs from metadata_json.
    pub fn track_ids(&self) -> Vec<i64> {
        self.metadata_json
            .as_ref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .and_then(|v| v.get("track_ids").cloned())
            .and_then(|v| v.as_array().cloned())
            .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect())
            .unwrap_or_default()
    }

    /// Create a new aggregate signal with track IDs embedded in metadata.
    pub fn with_track_ids(mut self, ids: &[i64]) -> Self {
        let mut meta = self
            .metadata_json
            .as_ref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        meta["track_ids"] = serde_json::json!(ids);
        meta["track_count"] = serde_json::json!(ids.len());
        self.metadata_json = Some(meta.to_string());
        self
    }
}

/// Types of aggregate signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AggregateSignalType {
    /// Multiple files with same fingerprint (internal overlap detection, not surfaced directly)
    FingerprintOverlap,
    /// Multiple files with same artist/album/title
    MetadataDuplicate,
    /// Multiple index entries with same inode
    DuplicateInode,
    /// Tracks missing a required tag
    MissingTag,
    /// Multiple corpus files deploy to same library path
    DeployConflict,
    /// Tag value collision needing canonicalization
    /// Key: "{tag_name}:{normalized_key}" (e.g., "artist:dragonforce")
    /// Metadata: { "variants": {"DragonForce": 47, "Dragonforce": 3}, "track_ids": [...] }
    TagCanonicity,
    /// Album has tracks with different artists + missing/inconsistent album_artist
    /// Key: "{normalized_album}" (e.g., "clockwork hearts")
    /// Metadata: { "album": "...", "artist_variants": {...}, "album_artist_variants": {...}, "track_ids": [...] }
    InconsistentAlbumArtist,
    /// Tag value contains separator characters needing to be split
    /// Key: "{tag_name}:{compound_value_hash}" (e.g., "genre:abc123")
    /// Metadata: { "tag_name", "compound_value", "split_parts": [...], "separator", "track_ids": [...] }
    CompoundTagValue,
    /// Directory-level overlap cluster (derived from FingerprintOverlap signals)
    /// Key: sorted|path|suffixes (e.g., "bandcamp|indie/msx")
    /// Metadata: { "cluster_key", "directories": [...], "fingerprint_overlap_keys": [...] }
    DirectoryOverlapCluster,
}

impl AggregateSignalType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FingerprintOverlap => "fingerprint_dup",
            Self::MetadataDuplicate => "metadata_dup",
            Self::DuplicateInode => "duplicate_inode",
            Self::MissingTag => "missing_tag",
            Self::DeployConflict => "deploy_conflict",
            Self::TagCanonicity => "tag_canonicity",
            Self::InconsistentAlbumArtist => "inconsistent_album_artist",
            Self::CompoundTagValue => "compound_tag_value",
            Self::DirectoryOverlapCluster => "directory_overlap_cluster",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "fingerprint_dup" => Some(Self::FingerprintOverlap),
            "metadata_dup" => Some(Self::MetadataDuplicate),
            "duplicate_inode" => Some(Self::DuplicateInode),
            "missing_tag" => Some(Self::MissingTag),
            "deploy_conflict" => Some(Self::DeployConflict),
            "tag_canonicity" => Some(Self::TagCanonicity),
            "inconsistent_album_artist" => Some(Self::InconsistentAlbumArtist),
            "compound_tag_value" => Some(Self::CompoundTagValue),
            "directory_overlap_cluster" => Some(Self::DirectoryOverlapCluster),
            _ => None,
        }
    }
}

// ============================================================================
// Signal (Unified Signal Type)
// ============================================================================

/// Unified signal representing a fact about corpus state.
///
/// Signals are created by computations and deleted when stale. They record
/// file health, duplicate detection, missing tags, deploy conflicts, etc.
#[derive(Debug, Clone)]
pub struct Signal {
    pub id: Option<i64>,
    pub issue_type: SignalType,
    pub issue_key: String,
    pub discovered_at: Option<String>,
    pub metadata_json: Option<String>,
}

impl From<FileSignal> for Signal {
    fn from(sig: FileSignal) -> Self {
        Self {
            id: sig.id,
            issue_type: SignalType::from_str(sig.signal_type.as_str())
                .unwrap_or(SignalType::FileInCorpus),
            issue_key: sig.path,
            discovered_at: sig.discovered_at,
            metadata_json: None,
        }
    }
}

impl From<AggregateSignal> for Signal {
    fn from(sig: AggregateSignal) -> Self {
        Self {
            id: sig.id,
            issue_type: SignalType::from_str(sig.signal_type.as_str())
                .unwrap_or(SignalType::FingerprintOverlap),
            issue_key: sig.key,
            discovered_at: sig.discovered_at,
            metadata_json: sig.metadata_json,
        }
    }
}

/// Summary of corpus signals.
#[derive(Debug, Clone, Default)]
pub struct SignalSummary {
    /// Total health issues (excluding deploy_conflicts)
    pub total_issues: usize,
    // Content-level breakdowns (may not sum to total_issues due to other issue types)
    pub metadata_duplicates: usize,
    pub canonicalization_issues: usize,
    pub missing_tag_issues: usize,
    pub known_variants: usize,
}

/// Aggregated corpus summary for UI display.
#[derive(Debug, Clone, Default)]
pub struct CorpusSummary {
    pub _track_count: usize,
    /// Number of unresolved deployment conflicts (tracks that would deploy to same path)
    pub deploy_conflicts: usize,
    pub signal_summary: SignalSummary,
    pub _deployment_stats: Option<DeploymentStats>,
    pub _pending_changes: HashMap<String, usize>,
    pub _last_scan: Option<String>,

    // File-level stats (benign signals, shown separately from issues)
    /// Total files discovered in corpus directories
    pub files_in_corpus: usize,
    /// Files that are healthy (in corpus + indexed + matching mtime)
    pub healthy_files: usize,
    /// Files in corpus but not indexed
    pub unindexed_files: usize,
    /// Files in index but no longer exist on disk
    pub missing_files: usize,
    /// Files that were moved (same inode, different path)
    pub moved_files: usize,

    // Library deployment signals
    /// Library files that are stale (deployed from wrong source)
    pub library_stale: usize,
    /// Library files that are leftovers (no corpus backing)
    pub library_leftover: usize,

    // Out-of-band change signals
    /// Files with syncable tag extras (one direction only)
    pub oob_tag_sync: usize,
    /// Files with tag value conflicts or mixed-direction extras
    pub oob_tag_conflict: usize,
    /// Files with mtime changed but tags identical (requires acknowledgement)
    pub mtime_only_mismatch: usize,
    /// Multiple index entries sharing same inode
    pub duplicate_inodes: usize,
}

// ============================================================================
// Insights View Data Types
// ============================================================================

/// Insights data for the bucketed Insights view.
/// Computed at cache refresh time, never in render.
#[derive(Debug, Clone, Default)]
pub struct InsightsData {
    pub bucket_corpus: CorpusFilesBucket,
    pub bucket_placeholder: PlaceholderBucket,
    pub bucket_library: LibraryDeployBucket,
    pub bucket_other: OtherSignalsBucket,
}

/// Bucket 1: Corpus Files - file state overview
#[derive(Debug, Clone, Default)]
pub struct CorpusFilesBucket {
    // OOB signals at top - highest priority within bucket
    pub oob_tag_sync: usize,
    pub oob_tag_conflict: usize,
    pub mtime_only_mismatch: usize,
    pub inode_changed: usize,
    // Standard corpus file signals
    pub files_in_corpus: usize,
    pub files_indexed: usize,
    pub files_unindexed: usize,
    pub files_missing: usize,
    pub files_relocated: usize,
    /// Files that are corrupt (unreadable tags or waveform decode failure)
    pub corrupt_files: usize,
    /// Files in non-Vorbis container formats (MP3, M4A, AAC, WMA, etc.)
    pub shit_format_files: usize,
    /// Filetype breakdown for files_in_corpus
    pub file_type_breakdown: Vec<(String, usize)>,
    /// Directory-level aggregation for selected signal
    pub _directory_breakdown: DirectoryBreakdown,
}

/// Bucket 2: Tag Squash - duplicates, tag canonicity, album_artist, and compound tag issues
#[derive(Debug, Clone, Default)]
pub struct TagSquashBucket {
    /// Directory overlap clusters (grouped fingerprint overlaps for bulk resolution)
    pub directory_overlap_cluster_count: usize,
    /// Subpar duplicates (lower quality versions, easy stash candidates)
    pub subpar_duplicate_count: usize,
    /// Tag canonicity issues grouped by tag name (e.g., "artist": 50 clusters)
    pub tag_canonicity: Vec<TagSquashEntry>,
    /// Inconsistent album_artist issues count
    pub inconsistent_album_artist_count: usize,
    /// Compound tag values needing split (e.g., "Rock; Metal")
    pub compound_tag_value_count: usize,
}

/// Entry for tag squash signals (grouped by tag name)
#[derive(Debug, Clone)]
pub struct TagSquashEntry {
    /// Tag name (e.g., "artist", "genre", "album")
    pub tag_name: String,
    /// Number of clusters needing resolution
    pub cluster_count: usize,
    /// Total tracks affected (for ordering - higher = more important)
    pub _total_tracks: usize,
}

// Aliases for compatibility
pub type PlaceholderBucket = TagSquashBucket;

/// Bucket 3: Library/Deploy state
#[derive(Debug, Clone, Default)]
pub struct LibraryDeployBucket {
    pub library_stale: usize,
    pub library_leftover: usize,
    /// Healthy files NOT in any library (DeployReady signals)
    pub deploy_ready: usize,
    /// Healthy files with correct library match (DeployedHealthy signals)
    pub deployed_healthy: usize,
}

/// Bucket 4: Other signals (sorted by magnitude)
#[derive(Debug, Clone, Default)]
pub struct OtherSignalsBucket {
    /// Sorted descending by count
    pub entries: Vec<OtherSignalEntry>,
}

/// Entry for other signals bucket
#[derive(Debug, Clone)]
pub struct OtherSignalEntry {
    pub signal_type: String,
    pub display_label: String,
    pub count: usize,
    /// For aggregate signals that track affected files/tracks
    pub affected_count: Option<usize>,
}

/// Directory breakdown for detail pane
#[derive(Debug, Clone, Default)]
pub struct DirectoryBreakdown {
    /// Sorted by count descending
    pub _entries: Vec<DirectoryBreakdownEntry>,
}

/// Single entry in directory breakdown
#[derive(Debug, Clone)]
pub struct DirectoryBreakdownEntry {
    pub _directory: String,
    pub _count: usize,
}

// ============================================================================
// Deploy Modal Data Types
// ============================================================================

/// A file with deploy info (for healthy/new files).
#[derive(Debug, Clone)]
pub struct DeploySignalFile {
    /// Path in the corpus
    pub corpus_path: String,
    /// Computed deploy path in library
    pub deploy_path: String,
    /// Track ID for mutation generation
    pub _track_id: i64,
}

/// A stale library file (deployed path differs from expected).
#[derive(Debug, Clone)]
pub struct StaleSignalFile {
    /// Current path in library (wrong)
    pub library_path: String,
    /// Expected path (computed from current tags)
    pub expected_path: String,
    /// Corpus file path (source)
    pub _corpus_path: String,
    /// Track ID for mutation generation
    pub _track_id: i64,
}

/// A leftover file (in library but no corpus backing).
#[derive(Debug, Clone)]
pub struct LeftoverSignalFile {
    /// Path in the library
    pub library_path: String,
}

/// A deploy conflict group (multiple corpus files → same library path).
#[derive(Debug, Clone)]
pub struct ConflictGroup {
    /// The library path they all would deploy to
    pub deploy_path: String,
    /// List of conflicting corpus files: (corpus_path, track_id)
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
    /// Reason for being subpar (e.g., "SubparBitrate", "SubparFormat")
    pub reason: String,
    /// Path to the superior version
    pub superior_path: String,
    /// Quality score of this file
    pub _quality_score: i64,
    /// Quality score of the superior file
    pub _superior_quality_score: i64,
}

// ============================================================================
// OOB Tag Resolution Types
// ============================================================================

/// Direction of a syncable OOB tag mismatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OobSyncDirection {
    /// Extra tags exist on disk only (db_value IS NULL) — sync disk → index
    DiskToIndex,
    /// Extra tags exist in DB only (disk_value IS NULL) — sync index → disk
    IndexToDisk,
}

/// A single tag mismatch entry between DB and disk.
///
/// Contains both display strings (for UI) and individual values (for mutations).
/// Multi-value tags (e.g., multiple TRACKNUMBER fields) are stored as individual
/// values in the `*_values` vecs, joined for display in `*_value` fields.
#[derive(Debug, Clone)]
pub struct TagMismatchEntry {
    pub field: String,
    /// Display string (values joined with "; ") - for UI
    pub db_value: Option<String>,
    /// Display string (values joined with "; ") - for UI
    pub disk_value: Option<String>,
    /// Individual tag values from DB (for mutations)
    pub _db_values: Vec<String>,
    /// Individual tag values from disk (for mutations)
    pub _disk_values: Vec<String>,
}

/// A file with purely sync-direction tag mismatches (all extras in one direction).
#[derive(Debug, Clone)]
pub struct OobSyncFile {
    pub track_id: i64,
    /// Relative path (as stored in signals/tracks)
    pub path: String,
    pub direction: OobSyncDirection,
    pub mismatches: Vec<TagMismatchEntry>,
}

/// Classification bucket for OOB signal files.
///
/// Determined by SQL CASE expression against signal type and `tag_mismatches` table:
/// - MtimeOnly: mtime_only_mismatch signal (mtime changed, tags identical) - needs acknowledgement
/// - DbOnly: all mismatches have `disk_value IS NULL`
/// - DiskOnly: all mismatches have `db_value IS NULL`
/// - Conflict: both values present, or mixed null directions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictBucket {
    MtimeOnly,
    DbOnly,
    DiskOnly,
    Conflict,
}

impl ConflictBucket {
    pub fn from_int(i: i32) -> Self {
        match i {
            0 => Self::MtimeOnly,
            1 => Self::DbOnly,
            2 => Self::DiskOnly,
            _ => Self::Conflict,
        }
    }

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
#[derive(Debug, Clone)]
pub struct BucketedOobFile {
    pub track_id: i64,
    pub path: String,
    pub bucket: ConflictBucket,
}

/// A file with an InodeChanged signal (file was replaced).
#[derive(Debug, Clone)]
pub struct InodeChangedFile {
    pub track_id: i64,
    pub path: String,
    pub old_inode: i64,
    pub new_inode: i64,
}

