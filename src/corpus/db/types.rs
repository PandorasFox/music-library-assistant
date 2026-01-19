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
    /// Quality variants (same content, different quality)
    QualityVariant,
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

            // Content-level signals
            Self::FingerprintDuplicate => "fingerprint_dup",
            Self::MetadataDuplicate => "metadata_dup",
            Self::TagCanonical => "tag_canon",
            Self::MissingTag => "missing_tag",
            Self::QualityVariant => "quality",
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

            // Content-level signals
            "fingerprint_dup" => Some(Self::FingerprintDuplicate),
            "metadata_dup" => Some(Self::MetadataDuplicate),
            "tag_canon" => Some(Self::TagCanonical),
            "canon" => Some(Self::TagCanonical),        // Legacy
            "genre_canon" => Some(Self::TagCanonical),  // Legacy
            "missing_tag" => Some(Self::MissingTag),
            "quality" => Some(Self::QualityVariant),
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

/// Severity of a health issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthIssueSeverity {
    /// Can be resolved automatically by quality comparison
    AutoResolvable,
    /// Requires manual review
    ManualReview,
    /// Informational only (e.g., canonicalization suggestions)
    Informational,
}

impl HealthIssueSeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AutoResolvable => "auto_resolvable",
            Self::ManualReview => "manual_review",
            Self::Informational => "informational",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "auto_resolvable" => Some(Self::AutoResolvable),
            "manual_review" => Some(Self::ManualReview),
            "informational" => Some(Self::Informational),
            _ => None,
        }
    }
}

/// A health signal detected in the corpus.
///
/// Signals are facts about corpus state. They are created by computations and
/// deleted when they become stale (not "resolved" - there is no resolution concept).
#[derive(Debug, Clone)]
pub struct HealthIssue {
    pub id: Option<i64>,
    pub issue_type: HealthIssueType,
    pub issue_key: String, // Path, fingerprint, normalized metadata key, etc.
    pub severity: HealthIssueSeverity,
    pub discovered_at: Option<String>,
    pub metadata_json: Option<String>,
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
    pub fingerprint_duplicates: usize,
    pub metadata_duplicates: usize,
    pub canonicalization_issues: usize,
    pub missing_tag_issues: usize,
    pub quality_variants: usize,
    pub auto_resolvable: usize,
    pub manual_review: usize,
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
