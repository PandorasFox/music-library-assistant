//! Core database types for track metadata and scan state.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Universal audio file representation.
/// Stored in the `tracks` table.
#[derive(Debug, Clone)]
pub struct Track {
    pub id: Option<i64>,
    pub path: String,
    pub source: String, // corpus/library name/legacy
    pub inode: i64,
    pub file_size: i64,
    pub file_type: String, // flac, mp3, ogg, etc.
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub title: Option<String>,
    pub track_number: Option<i32>,
    pub genre: Option<String>,
    pub duration_ms: Option<i64>,
    pub bitrate_kbps: Option<i32>,
    pub sample_rate: Option<i32>,
    pub fingerprint: Option<String>, // chromaprint acoustic fingerprint
    pub isrc: Option<String>,        // International Standard Recording Code
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

/// Type of health issue detected in the corpus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HealthIssueType {
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
    /// Multiple corpus files would deploy to the same library path
    DeployConflict,
    /// Tags on disk differ from indexed tags (out-of-band change)
    OutOfBandTagChange,
    /// Files exist on disk but are not in the index (need to be scanned)
    MissingFromIndex,
    /// File moved to different path (same inode detected at new location)
    FileRelocated,
    /// Multiple corpus entries share the same inode (hard links or DB inconsistency)
    DuplicateInode,
    /// Indexed file no longer exists on disk
    MissingFromDisk,
    /// File's inode/size/duration changed out-of-band (file was replaced externally)
    OutOfBandFileChange,
}

impl HealthIssueType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FingerprintDuplicate => "fingerprint_dup",
            Self::MetadataDuplicate => "metadata_dup",
            Self::TagCanonical => "tag_canon",
            Self::MissingTag => "missing_tag",
            Self::QualityVariant => "quality",
            Self::DeployConflict => "deploy_conflict",
            Self::OutOfBandTagChange => "oob_tag",
            Self::MissingFromIndex => "missing_from_index",
            Self::FileRelocated => "file_relocated",
            Self::DuplicateInode => "duplicate_inode",
            Self::MissingFromDisk => "missing_from_disk",
            Self::OutOfBandFileChange => "oob_file_change",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "fingerprint_dup" => Some(Self::FingerprintDuplicate),
            "metadata_dup" => Some(Self::MetadataDuplicate),
            "tag_canon" => Some(Self::TagCanonical),
            // Legacy support: map old types to unified TagCanonical
            "canon" => Some(Self::TagCanonical),
            "genre_canon" => Some(Self::TagCanonical),
            "missing_tag" => Some(Self::MissingTag),
            "quality" => Some(Self::QualityVariant),
            "deploy_conflict" => Some(Self::DeployConflict),
            "oob_tag" => Some(Self::OutOfBandTagChange),
            "missing_from_index" => Some(Self::MissingFromIndex),
            "file_relocated" => Some(Self::FileRelocated),
            "duplicate_inode" => Some(Self::DuplicateInode),
            "missing_from_disk" => Some(Self::MissingFromDisk),
            "oob_file_change" => Some(Self::OutOfBandFileChange),
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

/// How an issue was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionType {
    /// One track was kept, others stashed
    Kept,
    /// Files were stashed to stash directory
    Stashed,
    /// Tags were merged/unified
    Merged,
    /// Marked as known variant (re-release, remix, etc.)
    MarkedVariant,
    /// Issue was ignored/dismissed
    Ignored,
}

impl ResolutionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Kept => "kept",
            Self::Stashed => "stashed",
            Self::Merged => "merged",
            Self::MarkedVariant => "marked_variant",
            Self::Ignored => "ignored",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "kept" => Some(Self::Kept),
            "stashed" => Some(Self::Stashed),
            "merged" => Some(Self::Merged),
            "marked_variant" => Some(Self::MarkedVariant),
            "ignored" => Some(Self::Ignored),
            _ => None,
        }
    }
}

/// A health issue detected in the corpus.
#[derive(Debug, Clone)]
pub struct HealthIssue {
    pub id: Option<i64>,
    pub issue_type: HealthIssueType,
    pub issue_key: String, // Fingerprint, normalized metadata key, etc.
    pub severity: HealthIssueSeverity,
    pub discovered_at: Option<String>,
    pub resolved_at: Option<String>,
    pub resolution_type: Option<ResolutionType>,
    pub resolution_session: Option<String>,
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
