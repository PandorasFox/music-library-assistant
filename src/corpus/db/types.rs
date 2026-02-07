//! Core database types for file metadata and audio info.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// New Schema Types (inode-based identity)
// ============================================================================

/// File source classification.
///
/// Determines which root directory the file belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileSource {
    /// File is in the corpus (source of truth)
    Corpus,
    /// File is in a library (deployment target)
    Library,
    /// File is in the inbox (pending triage)
    Inbox,
}

impl FileSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Corpus => "corpus",
            Self::Library => "library",
            Self::Inbox => "inbox",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "corpus" => Some(Self::Corpus),
            "library" => Some(Self::Library),
            "inbox" => Some(Self::Inbox),
            _ => None,
        }
    }
}

impl std::fmt::Display for FileSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A path manifestation in the files table.
///
/// Represents a single path (file or directory) in one of the source locations.
/// Multiple paths can share the same inode (hard links across corpus + library).
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// The inode number (content identity)
    pub inode: i64,
    /// Which root this path belongs to
    pub source: FileSource,
    /// Relative path within the source root
    pub path: String,
    /// True if this is a directory, false if a file
    pub is_dir: bool,
    /// Filesystem modification time (seconds since epoch)
    pub mtime_secs: i64,
    /// Filesystem modification time (nanoseconds component)
    pub mtime_nanos: i64,
    /// File size in bytes
    pub file_size: i64,
    /// When this entry was last scanned (Unix timestamp)
    pub scanned_at: i64,
}

/// Audio-specific metadata for audio files.
///
/// Only audio file inodes have entries in audio_info.
/// Directories do not have audio_info records.
#[derive(Debug, Clone)]
pub struct AudioInfo {
    /// The inode number (links to files.inode)
    pub inode: i64,
    /// Audio format (flac, mp3, opus, ogg, etc.)
    pub file_type: String,
    /// Duration in milliseconds
    pub duration_ms: Option<i64>,
    /// Bitrate in kbps
    pub bitrate_kbps: Option<i32>,
    /// Sample rate in Hz
    pub sample_rate: Option<i32>,
    /// Chromaprint acoustic fingerprint as raw u32 values
    pub fingerprint: Option<Vec<u32>>,
    /// True when DB tags have changed but disk hasn't been updated yet
    pub needs_tag_flush: bool,
}

/// Combined view of a file entry with its audio info.
///
/// Used for audio files that have both a files row and audio_info row.
#[derive(Debug, Clone)]
pub struct AudioFile {
    pub entry: FileEntry,
    pub audio: AudioInfo,
}

impl AudioFile {
    /// Convenience accessor for the path
    pub fn path(&self) -> &str {
        &self.entry.path
    }

    /// Convenience accessor for the inode
    pub fn inode(&self) -> i64 {
        self.entry.inode
    }
}

/// A single tag associated with an audio file.
///
/// Stored in corpus_tags or inbox_tags tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTag {
    pub inode: i64,
    pub tag_name: String,
    pub tag_value: String,
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
// Signal Key Types
// ============================================================================

/// How a signal type is keyed in the database.
///
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
    /// File in index with different path but same inode (file was moved)
    /// issue_key: new path, metadata: {"old_path": "...", "inode": i64}
    MovedFile,
    /// File in index but no longer exists in corpus
    MissingFile,
    /// Directory in index but no longer exists in corpus
    MissingDirectory,

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
    /// Tag value collision needing canonicalization
    TagCanonicity,
    /// Album has tracks with different artists + missing/inconsistent album_artist
    InconsistentAlbumArtist,
    /// Tag value contains separators that should be split into multiple values
    CompoundTagValue,
    /// Cross-source fingerprint overlap cluster (derived from FingerprintOverlap signals)
    /// Key: sorted source pair, e.g., "web/releases/bandcamp|web/releases/indie"
    /// Metadata: { source_a, source_b, overlap_count, track_pairs: [...] }
    CrossSourceOverlap,

    // =========================================================================
    // Error signals (discovery-time parse/read failures)
    // =========================================================================
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
            Self::MovedFile => "moved_file",
            Self::MissingFile => "missing_file",
            Self::MissingDirectory => "missing_directory",

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
            Self::TagCanonicity => "tag_canonicity",
            Self::InconsistentAlbumArtist => "inconsistent_album_artist",
            Self::CompoundTagValue => "compound_tag_value",
            Self::CrossSourceOverlap => "cross_source_overlap",

            // Error signals
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
            "moved_file" => Some(Self::MovedFile),
            "missing_file" => Some(Self::MissingFile),
            "missing_directory" => Some(Self::MissingDirectory),

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
            // Legacy: inode_changed was removed in v3 migration
            // These are now exposed as MissingFile + UnindexedFile pair
            "inode_changed" => Some(Self::MissingFile),
            "tag_canonicity" => Some(Self::TagCanonicity),
            "inconsistent_album_artist" => Some(Self::InconsistentAlbumArtist),
            "compound_tag_value" => Some(Self::CompoundTagValue),
            "compound_tag" => Some(Self::CompoundTagValue),
            "cross_source_overlap" => Some(Self::CrossSourceOverlap),
            // Legacy: map old error signal types to CorruptFile
            "tag_parse_error" => Some(Self::CorruptFile),
            "waveform_read_error" => Some(Self::CorruptFile),
            "corrupt_file" => Some(Self::CorruptFile),
            "shit_format" => Some(Self::ShitFormat),
            "subpar_duplicate" => Some(Self::SubparDuplicate),

            // Legacy DB values → map to new types
            "missing_from_disk" => Some(Self::MissingFile),
            "missing_from_index" => Some(Self::FileInCorpus),
            "file_relocated" => Some(Self::MovedFile),
            // Legacy: oob_file_change was split into OOB trio
            "oob_file_change" => Some(Self::OutOfBandTagConflict),
            "corpus_file_modified_oob" => Some(Self::OutOfBandTagConflict),

            _ => None,
        }
    }
}

// NOTE: From<CorpusFileSignalType> for SignalType has been intentionally removed.
// This prevents accidental coercion that could lead to keying mismatches.
// If you need the string representation, use signal_type.as_str() directly.


// ============================================================================
// Signal Types (Type-Safe Signal System)
// ============================================================================

// NOTE: FileSignal struct has been removed.
// Corpus file signals are now inode-keyed (use CorpusFileSignalType).
// Library signals are now aggregate signals (use AggregateSignalType).

/// Types of corpus file signals.
///
/// These signals track the state of files in the corpus directory.
/// **All corpus file signals are keyed by inode** - use `clear_corpus_signal(type, inode)`.
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
    /// Directory in index but missing from corpus
    MissingDirectory,
    /// File moved (same inode, different path than indexed)
    /// issue_key: new path, metadata: {"old_path": "...", "inode": i64}
    MovedFile,
    /// Tags on disk have extras in one direction only (syncable)
    OutOfBandTagSync,
    /// Tags on disk conflict with indexed tags (value differences or mixed directions)
    OutOfBandTagConflict,
    /// File mtime changed but tags are identical (requires operator acknowledgement)
    MtimeOnlyMismatch,
    /// File is corrupt (unreadable tags or waveform decode failure)
    CorruptFile,
    /// File is in a non-Vorbis container format (MP3, M4A, AAC, WMA, or lossless needing remux)
    ShitFormat,
    /// Track is an subpar duplicate (lower quality version of another track)
    SubparDuplicate,
    /// Tag value contains separator characters needing split (per-file)
    /// Metadata: { "inode", "compounds": [{ "tag_name", "compound_value", "split_parts", "separator" }] }
    CompoundTag,
    /// Healthy corpus file ready for deployment (not yet in any library)
    /// Metadata: { "deploy_path": "..." }
    DeployReady,
    /// Healthy corpus file deployed at correct library path
    /// Metadata: { "library_path": "..." }
    DeployedHealthy,
}

impl CorpusFileSignalType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FileInCorpus => "file_in_corpus",
            Self::UnindexedFile => "unindexed_file",
            Self::HealthyFile => "healthy_file",
            Self::MissingFile => "missing_file",
            Self::MissingDirectory => "missing_directory",
            Self::MovedFile => "moved_file",
            Self::OutOfBandTagSync => "oob_tag_sync",
            Self::OutOfBandTagConflict => "oob_tag_conflict",
            Self::MtimeOnlyMismatch => "mtime_only_mismatch",
            Self::CorruptFile => "corrupt_file",
            Self::ShitFormat => "shit_format",
            Self::SubparDuplicate => "subpar_duplicate",
            Self::CompoundTag => "compound_tag",
            Self::DeployReady => "deploy_ready",
            Self::DeployedHealthy => "deployed_healthy",
        }
    }
}

impl std::fmt::Display for CorpusFileSignalType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// NOTE: LibraryFileSignalType and FileSignalType have been removed.
//
// - DeployReady/DeployedHealthy moved to CorpusFileSignalType (inode-keyed)
// - LibraryStale/LibraryLeftover moved to AggregateSignalType (semantic-keyed)
//
// This ensures compile-time enforcement of keying semantics:
// - CorpusFileSignalType -> must use clear_corpus_signal(type, inode)
// - AggregateSignalType -> must use clear_aggregate_signal(type, key)

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
    /// Get inodes from metadata_json.
    pub fn inodes(&self) -> Vec<i64> {
        self.metadata_json
            .as_ref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .and_then(|v| v.get("inodes").cloned())
            .and_then(|v| v.as_array().cloned())
            .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect())
            .unwrap_or_default()
    }

    /// Create a new aggregate signal with inodes embedded in metadata.
    pub fn with_inodes(mut self, ids: &[i64]) -> Self {
        let mut meta = self
            .metadata_json
            .as_ref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        meta["inodes"] = serde_json::json!(ids);
        meta["inode_count"] = serde_json::json!(ids.len());
        self.metadata_json = Some(meta.to_string());
        self
    }
}

/// Types of aggregate signals (semantic-keyed).
///
/// These signals use arbitrary string keys (not inodes). Use `clear_aggregate_signal(type, key)`.
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
    /// Metadata: { "variants": {"DragonForce": 47, "Dragonforce": 3}, "inodes": [...] }
    TagCanonicity,
    /// Album has tracks with different artists + missing/inconsistent album_artist
    /// Key: "{normalized_album}" (e.g., "clockwork hearts")
    /// Metadata: { "album": "...", "artist_variants": {...}, "album_artist_variants": {...}, "inodes": [...] }
    InconsistentAlbumArtist,
    /// Tag value contains separator characters needing to be split
    /// Key: "{tag_name}:{compound_value_hash}" (e.g., "genre:abc123")
    /// Metadata: { "tag_name", "compound_value", "split_parts": [...], "separator", "inodes": [...] }
    CompoundTagValue,
    /// Cross-source fingerprint overlap cluster (derived from FingerprintOverlap signals)
    /// Key: sorted source pair, e.g., "web/releases/bandcamp|web/releases/indie"
    /// Metadata: { "source_a", "source_b", "overlap_count", "fingerprint_keys": [...], "track_pairs": [...] }
    CrossSourceOverlap,
    /// Operator-confirmed canonical tag value (whitelist - skip split detection)
    /// Key: "{tag_name}:{tag_value}" (e.g., "artist:Rinse & Repeat")
    /// Metadata: { "tag_name", "canonical_value", "created_at" }
    CanonicalTag,
    /// Library file without corpus backing
    /// Key: "library_leftover:{library_name}:{library_path}"
    LibraryLeftover,
    /// Library file at wrong path (tags changed since deploy)
    /// Key: "library_stale:{library_name}:{library_path}"
    /// Metadata: { "library_path", "expected_path" }
    LibraryStale,
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
            Self::CrossSourceOverlap => "cross_source_overlap",
            Self::CanonicalTag => "canonical_tag",
            Self::LibraryLeftover => "library_leftover",
            Self::LibraryStale => "library_stale",
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
            "cross_source_overlap" => Some(Self::CrossSourceOverlap),
            "canonical_tag" => Some(Self::CanonicalTag),
            "library_leftover" => Some(Self::LibraryLeftover),
            "library_stale" => Some(Self::LibraryStale),
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
///
/// ## Key Types
///
/// - **File signals** (FileInCorpus, UnindexedFile, HealthyFile, MissingFile, etc.):
///   `issue_key` stores the inode as a string. Path is in `metadata_json.path`.
/// - **Library signals** (LibraryStale, LibraryLeftover):
///   `issue_key` uses compound format like `"library_leftover:{name}:{path}"`.
/// - **Aggregate signals** (FingerprintOverlap, TagCanonicity, etc.):
///   `issue_key` uses semantic string keys.
#[derive(Debug, Clone)]
pub struct Signal {
    pub id: Option<i64>,
    pub issue_type: SignalType,
    pub issue_key: String,
    pub discovered_at: Option<String>,
    pub metadata_json: Option<String>,
    /// Native inode for inode-keyed signals (corpus file signals).
    /// Path-keyed signals (MissingDirectory, library signals) have None.
    pub inode: Option<i64>,
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
            inode: None, // AggregateSignal is semantic-keyed, not inode-keyed
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
    /// Directories in index but no longer exist on disk
    pub missing_directories: usize,
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
    // Standard corpus file signals
    pub files_in_corpus: usize,
    pub files_indexed: usize,
    pub files_unindexed: usize,
    pub files_missing: usize,
    pub directories_missing: usize,
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
    /// Compound tag values - safe splits (all parts exist in corpus)
    pub compound_safe_count: usize,
    /// Compound tag values - needs review (some/all parts are new)
    pub compound_review_count: usize,
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
}

/// A stale library file (deployed path differs from expected).
#[derive(Debug, Clone)]
pub struct StaleSignalFile {
    /// Current path in library (wrong)
    pub library_path: String,
    /// Expected path (computed from current tags)
    pub expected_path: String,
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
    /// List of conflicting corpus files: (corpus_path, inode)
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
    pub inode: i64,
    /// Relative path (as stored in signals/files)
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
    pub inode: i64,
    pub path: String,
    pub bucket: ConflictBucket,
}

/// A file with a MovedFile signal (same inode, different path).
#[derive(Debug, Clone)]
pub struct MovedFileInfo {
    pub inode: i64,
    pub old_path: String,
    pub new_path: String,
}

