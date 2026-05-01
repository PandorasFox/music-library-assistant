//! Domain query response types defined alongside the query system.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ============================================================================
// Packing Directory Data
// ============================================================================

/// Cached packing directory data for tree browser markers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PackingDirsData {
    /// Paths of files with release packing assignments.
    pub file_paths: HashSet<PathBuf>,
    /// Parent directories mapped to their best packing category.
    pub dir_categories: HashMap<PathBuf, crate::signals::PackingCategory>,
}

// ============================================================================
// Packing Browser Data
// ============================================================================

/// All data needed to build a release packing browser view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackingBrowserData {
    pub packed: Vec<crate::signals::data::PackedReleaseData>,
    pub packing: Vec<(i64, String, crate::signals::data::ReleasePackingData)>,
    pub unfilled: Vec<crate::signals::data::UnfilledReleaseSlotData>,
    pub alternatives: Vec<crate::signals::data::AlternativeReleasePackingData>,
    pub va_overrides: Vec<crate::signals::data::VariousArtistsOverrideData>,
}

// ============================================================================
// VA Override Review
// ============================================================================

/// One reviewable VariousArtistsOverride suggestion, enriched with the inode
/// count (so the operator knows the apply blast radius) and a representative
/// current `ALBUMARTIST` value (so they can compare what's being replaced
/// against the suggestion).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaOverrideReviewRow {
    pub release_id: String,
    pub release_title: String,
    pub suggested_artist: String,
    pub source: crate::signals::data::VariousArtistsOverrideSource,
    /// Number of inodes currently packed to this release.
    pub packed_inode_count: usize,
    /// A representative `ALBUMARTIST` value across the packed inodes. If
    /// every packed inode shares the same value, that's it; if values
    /// disagree, the most-common one is returned. `None` means none of
    /// the packed inodes have an `ALBUMARTIST` tag.
    pub current_albumartist: Option<String>,
    /// True if `current_albumartist` is the same across every packed inode.
    pub albumartist_uniform: bool,
}

/// Response type for `GetVaOverrideReview`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VaOverrideReview {
    pub rows: Vec<VaOverrideReviewRow>,
}

// ============================================================================
// Recording Batch Result
// ============================================================================

/// Response type for batch recording data loading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingBatchResult {
    pub summaries: Vec<(String, crate::views::review_match::RecordingSummary)>,
    pub details: Vec<(String, crate::views::review_match::RecordingDetail)>,
}

// ============================================================================
// Release Staging Data
// ============================================================================

/// Response type for release staging data loading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseStagingData {
    pub bundle: crate::external::musicbrainz::MbCacheBundle,
    pub inode_tags: HashMap<i64, Vec<(String, String)>>,
}

// ============================================================================
// Tag Editor Load Mode
// ============================================================================

/// Tag editor file loading mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TagEditorLoadMode {
    /// Load all files in directory tree (recursive).
    Directory,
    /// Load siblings in parent directory, select the target file.
    SingleFile,
}

// ============================================================================
// Bulk Tag Aggregate
// ============================================================================

/// Server-computed aggregate of tags across multiple files in a directory.
/// Avoids sending per-file tag data to the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkTagAggregate {
    /// How many files were included.
    pub file_count: usize,
    /// The inodes of all files (needed by client for save mutations).
    pub inodes: Vec<i64>,
    /// Directory label (zone-relative path).
    pub dir_label: String,
    /// Aggregated tags across all files.
    pub tags: Vec<AggregateTag>,
}

/// A single tag aggregated across multiple files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateTag {
    /// Tag name (e.g. "ARTIST").
    pub name: String,
    /// If all files that have this tag share the same value, it's here.
    /// `None` means values differ across files.
    pub uniform_value: Option<String>,
    /// How many files have this tag.
    pub presence: usize,
    /// For non-uniform tags: value → inodes mapping, so the client can
    /// build correct `drop_tag` ops without an extra round-trip.
    /// Empty for uniform tags (all inodes share the single value).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub value_inodes: Vec<(String, Vec<i64>)>,
}

// ============================================================================
// Audio File With Tags
// ============================================================================

/// An audio file paired with its tag map (tag name -> values).
/// Tag names are uppercased; values are collected into Vec since a
/// single tag name can have multiple values (e.g. multiple genres).
pub type AudioFileWithTags = (crate::db_types::AudioFile, HashMap<String, Vec<String>>);

// ============================================================================
// Search Condition Wire Type
// ============================================================================

/// Wire-safe projection of `SearchCondition` (replaces `TextInputState` fields
/// with plain `String`s, enum fields with serde-compatible copies).
/// Used by `SearchWithConditions` query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConditionWire {
    pub condition_type: WireConditionType,
    pub operator: WireLogicalOperator,
    pub tag_name: String,
    pub comparison: WireComparisonOperator,
    pub search_value: String,
    pub file_type_category: WireFileTypeCategory,
    pub range_min: String,
    pub range_max: String,
}

/// Wire-safe mirror of `mm_ui::domain_types::ConditionType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireConditionType {
    Tag,
    FileType,
    SampleRate,
    Bitrate,
    Duration,
}

/// Wire-safe mirror of `mm_ui::domain_types::LogicalOperator`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireLogicalOperator {
    And,
    Or,
    Xor,
}

/// Wire-safe mirror of `mm_ui::domain_types::ComparisonOperator`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireComparisonOperator {
    Is,
    Not,
    Contains,
    Like,
}

/// Wire-safe mirror of `mm_ui::domain_types::FileTypeCategory`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireFileTypeCategory {
    Any,
    Lossless,
    Lossy,
    Flac,
    Mp3,
    Opus,
    Ogg,
    Wav,
    Aac,
}

// ============================================================================
// Directory Listing
// ============================================================================

/// A single entry in a directory listing (either a subdirectory or an audio file).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryListingEntry {
    /// Just the directory or file name (last path component).
    pub name: String,
    /// Relative path within the zone.
    pub path: String,
    /// True for directories, false for files.
    pub is_dir: bool,
    /// Number of direct-child audio files (directories only).
    pub file_count: usize,
    /// Inode (files only).
    pub inode: Option<i64>,
    /// Duration in milliseconds (files only).
    pub duration_ms: Option<i64>,
    /// Bitrate in kbps (files only).
    pub bitrate_kbps: Option<i32>,
}

// ============================================================================
// Search Result
// ============================================================================

/// A single result from server-side corpus file search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub inode: i64,
    pub path: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub title: Option<String>,
}
