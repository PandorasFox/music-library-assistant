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
    pub fingerprint: Option<String>,   // chromaprint acoustic fingerprint
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
    pub source: String,
    pub inode: i64,
    pub path: String,
    pub mtime_secs: i64,
    pub mtime_nanos: i64, // SQLite INTEGER is i64; cast to u32 at comparison time
    pub file_size: i64,
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
/// Signals are organized into levels:
/// - **First-level**: Computed directly from corpus + index state (WalkCorpus, etc.)
/// - **Second-level**: Derived from comparing first-level signals
/// - **Third-level**: Triggered when files become healthy (e.g., deploy conflicts)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HealthIssueType {
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
    /// Same fingerprint across multiple files
    FingerprintDuplicate,
    /// Same metadata (artist/album/title) across multiple files
    MetadataDuplicate,
    /// Missing required tags (e.g., album_artist)
    MissingTag,
    /// Tags on disk differ from indexed tags (out-of-band tag change)
    OutOfBandTagChange,
    /// Multiple corpus entries share the same inode (hard links or DB inconsistency)
    DuplicateInode,
}

impl HealthIssueType {
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
            Self::FingerprintDuplicate => "fingerprint_dup",
            Self::MetadataDuplicate => "metadata_dup",
            Self::MissingTag => "missing_tag",
            Self::OutOfBandTagChange => "oob_tag",
            Self::DuplicateInode => "duplicate_inode",
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
            "fingerprint_dup" => Some(Self::FingerprintDuplicate),
            "metadata_dup" => Some(Self::MetadataDuplicate),
            "missing_tag" => Some(Self::MissingTag),
            "oob_tag" => Some(Self::OutOfBandTagChange),
            "duplicate_inode" => Some(Self::DuplicateInode),

            // Legacy DB values → map to new types
            "missing_from_disk" => Some(Self::MissingFile),
            "missing_from_index" => Some(Self::FileInCorpus),
            "file_relocated" => Some(Self::MovedFile),
            "oob_file_change" => Some(Self::CorpusFileModifiedOutOfBand),

            _ => None,
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

/// Types of file-based signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileSignalType {
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
    /// Tags on disk differ from indexed tags
    OutOfBandTagChange,
    /// Library file without corpus backing
    LibraryLeftover,
    /// Library file at wrong path
    LibraryStale,
    /// Healthy corpus file ready for deployment (not yet in any library)
    DeployReady,
    /// Healthy corpus file deployed at correct library path
    DeployedHealthy,
}

impl FileSignalType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FileInCorpus => "file_in_corpus",
            Self::UnindexedFile => "unindexed_file",
            Self::HealthyFile => "healthy_file",
            Self::MissingFile => "missing_file",
            Self::CorpusFileModifiedOutOfBand => "corpus_file_modified_oob",
            Self::MovedFile => "moved_file",
            Self::OutOfBandTagChange => "oob_tag",
            Self::LibraryLeftover => "library_leftover",
            Self::LibraryStale => "library_stale",
            Self::DeployReady => "deploy_ready",
            Self::DeployedHealthy => "deployed_healthy",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "file_in_corpus" => Some(Self::FileInCorpus),
            "unindexed_file" => Some(Self::UnindexedFile),
            "healthy_file" => Some(Self::HealthyFile),
            "missing_file" | "missing_from_disk" => Some(Self::MissingFile),
            "corpus_file_modified_oob" | "oob_file_change" => Some(Self::CorpusFileModifiedOutOfBand),
            "moved_file" | "file_relocated" => Some(Self::MovedFile),
            "oob_tag" => Some(Self::OutOfBandTagChange),
            "library_leftover" => Some(Self::LibraryLeftover),
            "library_stale" => Some(Self::LibraryStale),
            "deploy_ready" => Some(Self::DeployReady),
            "deployed_healthy" => Some(Self::DeployedHealthy),
            _ => None,
        }
    }

    /// Convert to HealthIssueType for signals that need metadata.
    /// Panics for DeployReady/DeployedHealthy which use different storage.
    pub fn to_health_issue_type(&self) -> HealthIssueType {
        match self {
            Self::FileInCorpus => HealthIssueType::FileInCorpus,
            Self::UnindexedFile => HealthIssueType::UnindexedFile,
            Self::HealthyFile => HealthIssueType::HealthyFile,
            Self::MissingFile => HealthIssueType::MissingFile,
            Self::CorpusFileModifiedOutOfBand => HealthIssueType::CorpusFileModifiedOutOfBand,
            Self::MovedFile => HealthIssueType::MovedFile,
            Self::OutOfBandTagChange => HealthIssueType::OutOfBandTagChange,
            Self::LibraryLeftover => HealthIssueType::LibraryLeftover,
            Self::LibraryStale => HealthIssueType::LibraryStale,
            Self::DeployReady | Self::DeployedHealthy => {
                panic!("DeployReady/DeployedHealthy should not use to_health_issue_type")
            }
        }
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
    /// Multiple files with same fingerprint
    FingerprintDuplicate,
    /// Multiple files with same artist/album/title
    MetadataDuplicate,
    /// Multiple index entries with same inode
    DuplicateInode,
    /// Tracks missing a required tag
    MissingTag,
    /// Multiple corpus files deploy to same library path
    DeployConflict,
}

impl AggregateSignalType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FingerprintDuplicate => "fingerprint_dup",
            Self::MetadataDuplicate => "metadata_dup",
            Self::DuplicateInode => "duplicate_inode",
            Self::MissingTag => "missing_tag",
            Self::DeployConflict => "deploy_conflict",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "fingerprint_dup" => Some(Self::FingerprintDuplicate),
            "metadata_dup" => Some(Self::MetadataDuplicate),
            "duplicate_inode" => Some(Self::DuplicateInode),
            "missing_tag" => Some(Self::MissingTag),
            "deploy_conflict" => Some(Self::DeployConflict),
            _ => None,
        }
    }
}

// ============================================================================
// Legacy HealthIssue (for migration compatibility)
// ============================================================================

/// Legacy unified signal type.
///
/// Kept for backwards compatibility. Prefer `Signal` enum for new code.
#[derive(Debug, Clone)]
pub struct HealthIssue {
    pub id: Option<i64>,
    pub issue_type: HealthIssueType,
    pub issue_key: String,
    pub discovered_at: Option<String>,
    pub metadata_json: Option<String>,
}

impl From<FileSignal> for HealthIssue {
    fn from(sig: FileSignal) -> Self {
        Self {
            id: sig.id,
            issue_type: HealthIssueType::from_str(sig.signal_type.as_str())
                .unwrap_or(HealthIssueType::FileInCorpus),
            issue_key: sig.path,
            discovered_at: sig.discovered_at,
            metadata_json: None,
        }
    }
}

impl From<AggregateSignal> for HealthIssue {
    fn from(sig: AggregateSignal) -> Self {
        Self {
            id: sig.id,
            issue_type: HealthIssueType::from_str(sig.signal_type.as_str())
                .unwrap_or(HealthIssueType::FingerprintDuplicate),
            issue_key: sig.key,
            discovered_at: sig.discovered_at,
            metadata_json: sig.metadata_json,
        }
    }
}

/// A canonical tag value mapping (unified for artist, album_artist, genre, album).
#[derive(Debug, Clone)]
pub struct TagCanonicalization {
    pub id: Option<i64>,
    pub tag_name: String, // "artist", "album_artist", "genre", "album"
    pub canonical_value: String,
    pub variant_value: String,
    pub confidence: Option<f64>,
    pub auto_detected: bool,
    pub confirmed_at: Option<String>,
}

/// Summary of corpus health.
#[derive(Debug, Clone, Default)]
pub struct HealthSummary {
    /// Total health issues (excluding deploy_conflicts)
    pub total_issues: usize,
    // Content-level breakdowns (may not sum to total_issues due to other issue types)
    pub fingerprint_duplicates: usize,
    pub metadata_duplicates: usize,
    pub canonicalization_issues: usize,
    pub missing_tag_issues: usize,
    pub known_variants: usize,
}

/// Aggregated corpus summary for UI display.
#[derive(Debug, Clone, Default)]
pub struct CorpusSummary {
    pub track_count: usize,
    /// Number of unresolved deployment conflicts (tracks that would deploy to same path)
    pub deploy_conflicts: usize,
    pub health_summary: HealthSummary,
    pub deployment_stats: Option<DeploymentStats>,
    pub pending_changes: HashMap<String, usize>,
    pub last_scan: Option<String>,

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
    /// Files with mtime changed outside MLA
    pub modified_oob: usize,
    /// Files with tags changed outside MLA
    pub tags_changed_oob: usize,
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
    pub modified_oob: usize,
    pub tags_changed_oob: usize,
    // Standard corpus file signals
    pub files_in_corpus: usize,
    pub files_indexed: usize,
    pub files_unindexed: usize,
    pub files_missing: usize,
    pub files_relocated: usize,
    /// Filetype breakdown for files_in_corpus
    pub file_type_breakdown: Vec<(String, usize)>,
    /// Directory-level aggregation for selected signal
    pub directory_breakdown: DirectoryBreakdown,
}

/// Bucket 2: Placeholder
#[derive(Debug, Clone)]
pub struct PlaceholderBucket {
    pub description: &'static str,
}

impl Default for PlaceholderBucket {
    fn default() -> Self {
        Self { description: ":)" }
    }
}

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
    pub entries: Vec<DirectoryBreakdownEntry>,
}

/// Single entry in directory breakdown
#[derive(Debug, Clone)]
pub struct DirectoryBreakdownEntry {
    pub directory: String,
    pub count: usize,
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
    pub track_id: i64,
}

/// A stale library file (deployed path differs from expected).
#[derive(Debug, Clone)]
pub struct StaleSignalFile {
    /// Current path in library (wrong)
    pub library_path: String,
    /// Expected path (computed from current tags)
    pub expected_path: String,
    /// Corpus file path (source)
    pub corpus_path: String,
    /// Track ID for mutation generation
    pub track_id: i64,
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

