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
    /// Library file exists and matches corpus inode (healthy deployment)
    /// Aggregated count per library - no per-file signals for healthy files.
    /// issue_key: "library_health:{library_name}"
    LibraryHealthSummary,
    /// Library file exists but deployed at wrong path (tags changed since deploy)
    /// issue_key: "library_stale:{library_name}:{library_path}"
    LibraryStale,
    /// Corpus track should be deployed to library but isn't
    /// issue_key: "library_not_deployed:{library_name}:{corpus_path}"
    LibraryNotDeployed,
    /// Library file exists without corpus backing (orphan)
    /// issue_key: "library_orphan:{library_name}:{library_path}"
    LibraryOrphan,

    // =========================================================================
    // Content-level signals (tag and fingerprint analysis)
    // =========================================================================
    /// Same fingerprint across multiple files
    FingerprintDuplicate,
    /// Same metadata (artist/album/title) across multiple files
    MetadataDuplicate,
    /// Tag value variants that should be canonicalized (artist, genre, album, album_artist)
    /// The specific tag is stored in metadata_json.tag_name
    TagCanonical,
    /// Missing required tags (e.g., album_artist)
    MissingTag,
    /// Tags on disk differ from indexed tags (out-of-band tag change)
    OutOfBandTagChange,
    /// Multiple corpus entries share the same inode (hard links or DB inconsistency)
    DuplicateInode,

    // =========================================================================
    // Legacy types (kept for DB compatibility, will be migrated)
    // =========================================================================
    /// Legacy: renamed to MissingFile
    #[deprecated(note = "Use MissingFile instead")]
    MissingFromDisk,
    /// Legacy: renamed to FileInCorpus + UnindexedFile derivation
    #[deprecated(note = "Use FileInCorpus/UnindexedFile instead")]
    MissingFromIndex,
    /// Legacy: renamed to MovedFile
    #[deprecated(note = "Use MovedFile instead")]
    FileRelocated,
    /// Legacy: renamed to CorpusFileModifiedOutOfBand
    #[deprecated(note = "Use CorpusFileModifiedOutOfBand instead")]
    OutOfBandFileChange,
}

impl HealthIssueType {
    #[allow(deprecated)]
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
            Self::LibraryHealthSummary => "library_health_summary",
            Self::LibraryStale => "library_stale",
            Self::LibraryNotDeployed => "library_not_deployed",
            Self::LibraryOrphan => "library_orphan",

            // Content-level signals
            Self::FingerprintDuplicate => "fingerprint_dup",
            Self::MetadataDuplicate => "metadata_dup",
            Self::TagCanonical => "tag_canon",
            Self::MissingTag => "missing_tag",
            Self::OutOfBandTagChange => "oob_tag",
            Self::DuplicateInode => "duplicate_inode",

            // Legacy types (write using new names for forwards compatibility)
            Self::MissingFromDisk => "missing_file",
            Self::MissingFromIndex => "file_in_corpus",
            Self::FileRelocated => "moved_file",
            Self::OutOfBandFileChange => "corpus_file_modified_oob",
        }
    }

    #[allow(deprecated)]
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
            "library_health_summary" => Some(Self::LibraryHealthSummary),
            "library_stale" => Some(Self::LibraryStale),
            "library_not_deployed" => Some(Self::LibraryNotDeployed),
            "library_orphan" => Some(Self::LibraryOrphan),

            // Content-level signals
            "fingerprint_dup" => Some(Self::FingerprintDuplicate),
            "metadata_dup" => Some(Self::MetadataDuplicate),
            "tag_canon" => Some(Self::TagCanonical),
            "canon" => Some(Self::TagCanonical),        // Legacy
            "genre_canon" => Some(Self::TagCanonical),  // Legacy
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

/// A health signal - either file-based or aggregate.
///
/// Uses Rust's type system to enforce:
/// - File signals have no metadata (path is the key)
/// - Aggregate signals have metadata (grouping key + details)
#[derive(Debug, Clone)]
pub enum Signal {
    /// File-based signal (key = path, no metadata)
    File(FileSignal),
    /// Aggregate signal (key = grouping key, has metadata)
    Aggregate(AggregateSignal),
}

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
    LibraryOrphan,
    /// Library file at wrong path
    LibraryStale,
    /// Corpus track not deployed to library
    LibraryNotDeployed,
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
            Self::LibraryOrphan => "library_orphan",
            Self::LibraryStale => "library_stale",
            Self::LibraryNotDeployed => "library_not_deployed",
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
            "library_orphan" => Some(Self::LibraryOrphan),
            "library_stale" => Some(Self::LibraryStale),
            "library_not_deployed" => Some(Self::LibraryNotDeployed),
            _ => None,
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
    /// Tag value variants that should be canonicalized
    TagCanonical,
    /// Multiple corpus files deploy to same library path
    DeployConflict,
    /// Per-library health summary (counts)
    LibraryHealthSummary,
}

impl AggregateSignalType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FingerprintDuplicate => "fingerprint_dup",
            Self::MetadataDuplicate => "metadata_dup",
            Self::DuplicateInode => "duplicate_inode",
            Self::MissingTag => "missing_tag",
            Self::TagCanonical => "tag_canon",
            Self::DeployConflict => "deploy_conflict",
            Self::LibraryHealthSummary => "library_health_summary",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "fingerprint_dup" => Some(Self::FingerprintDuplicate),
            "metadata_dup" => Some(Self::MetadataDuplicate),
            "duplicate_inode" => Some(Self::DuplicateInode),
            "missing_tag" => Some(Self::MissingTag),
            "tag_canon" | "canon" | "genre_canon" => Some(Self::TagCanonical),
            "deploy_conflict" => Some(Self::DeployConflict),
            "library_health_summary" => Some(Self::LibraryHealthSummary),
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

impl HealthIssue {
    /// Convert to the new Signal type.
    pub fn to_signal(&self) -> Option<Signal> {
        // Try file signal first
        if let Some(file_type) = FileSignalType::from_str(self.issue_type.as_str()) {
            return Some(Signal::File(FileSignal {
                id: self.id,
                signal_type: file_type,
                path: self.issue_key.clone(),
                discovered_at: self.discovered_at.clone(),
            }));
        }
        // Try aggregate signal
        if let Some(agg_type) = AggregateSignalType::from_str(self.issue_type.as_str()) {
            return Some(Signal::Aggregate(AggregateSignal {
                id: self.id,
                signal_type: agg_type,
                key: self.issue_key.clone(),
                discovered_at: self.discovered_at.clone(),
                metadata_json: self.metadata_json.clone(),
            }));
        }
        None
    }
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

/// Role of a track in a health issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackRole {
    /// Track selected to keep in resolution
    Winner,
    /// Track to be removed/stashed in resolution
    Loser,
    /// General member of the issue (unresolved)
    Member,
    /// Canonical version (for artist canonicalization)
    Canonical,
    /// Variant version (for artist canonicalization)
    Variant,
}

impl TrackRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Winner => "winner",
            Self::Loser => "loser",
            Self::Member => "member",
            Self::Canonical => "canonical",
            Self::Variant => "variant",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "winner" => Some(Self::Winner),
            "loser" => Some(Self::Loser),
            "member" => Some(Self::Member),
            "canonical" => Some(Self::Canonical),
            "variant" => Some(Self::Variant),
            _ => None,
        }
    }
}

/// Type of known variant relationship.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariantType {
    /// Same recording released on different albums
    Rerelease,
    /// Remix of the original
    Remix,
    /// Remastered version
    Remaster,
    /// Live recording of studio track
    Live,
}

impl VariantType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rerelease => "re-release",
            Self::Remix => "remix",
            Self::Remaster => "remaster",
            Self::Live => "live",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "re-release" => Some(Self::Rerelease),
            "remix" => Some(Self::Remix),
            "remaster" => Some(Self::Remaster),
            "live" => Some(Self::Live),
            _ => None,
        }
    }
}

/// A known variant relationship between tracks.
#[derive(Debug, Clone)]
pub struct KnownVariant {
    pub id: Option<i64>,
    pub variant_type: VariantType,
    pub canonical_fingerprint: String,
    pub variant_fingerprint: Option<String>,
    pub canonical_track_id: Option<i64>,
    pub variant_track_id: Option<i64>,
    pub marked_at: Option<String>,
    pub notes: Option<String>,
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
}

// ============================================================================
// Album Artist Collation and Population Types
// ============================================================================

/// An album with multiple distinct artist values that may need collation.
/// Used in the Album Artist Collation flow to suggest "Various Artists" unification.
#[derive(Debug, Clone)]
pub struct AlbumArtistCollation {
    /// The album name
    pub album_name: String,
    /// Artists found on this album with track counts: (artist_name, track_count)
    pub artists: Vec<(String, usize)>,
    /// Total tracks in this album
    pub total_tracks: usize,
    /// Existing album_artist value if any tracks have it set
    pub existing_album_artist: Option<String>,
    /// Suggested album_artist (single artist if uniform, "Various Artists" if mixed)
    pub suggested_album_artist: Option<String>,
}

/// A group of tracks missing album_artist tags, grouped for bulk population.
#[derive(Debug, Clone)]
pub struct AlbumArtistPopulationGroup {
    /// Album name if tracks share an album tag
    pub album_name: Option<String>,
    /// Directory path if tracks are grouped by directory (no album tag)
    pub directory: Option<String>,
    /// Artists found in this group with track counts: (artist_name, track_count)
    pub artists: Vec<(String, usize)>,
    /// Total tracks in this group
    pub total_tracks: usize,
    /// Suggested album_artist (single artist if uniform, "Various Artists" if mixed)
    pub suggested_album_artist: Option<String>,
}
