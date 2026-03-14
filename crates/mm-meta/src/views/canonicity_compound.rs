//! Tag canonicity and compound split modal data types.

use serde::{Deserialize, Serialize};

// ============================================================================
// Shared File Info
// ============================================================================

/// File info with cached tag values for display.
///
/// Used by both compound split and tag canonicity modals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTagInfo {
    /// Inode of the file
    pub inode: i64,
    /// Display name (basename)
    pub filename: String,
    /// Full corpus-relative path
    pub path: String,
    /// All tags for this file (tag_name, tag_value)
    pub tag_values: Vec<(String, String)>,
}

// ============================================================================
// Canonicity Signal Kind
// ============================================================================

/// Which typed signal table a canonicity cluster modal targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CanonicitySignalKind {
    /// `signal_tag_canonicity` table (pre-fill enabled)
    TagCanonicity,
    /// `signal_inconsistent_album_artist` table (no pre-fill)
    InconsistentAlbumArtist,
    /// `signal_inbox_tag_canonicity` table (pre-fill enabled, inbox zone)
    InboxTagCanonicity,
}

// ============================================================================
// Compound Split Data Types
// ============================================================================

/// A single compound value from the signal's compounds array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompoundEntry {
    /// Tag name (e.g., "artist", "genre")
    pub tag_name: String,
    /// Original compound value (e.g., "Rock; Metal")
    pub compound_value: String,
    /// Split parts (e.g., ["Rock", "Metal"])
    pub split_parts: Vec<String>,
    /// Which parts exist in corpus
    pub matching_parts: Vec<String>,
}

impl CompoundEntry {
    /// Check if a specific part exists in corpus.
    pub fn part_exists(&self, part: &str) -> bool {
        self.matching_parts.iter().any(|m| m == part)
    }
}

/// Modal data loaded from a compound tag signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompoundSplitDataV2 {
    /// The first compound entry (we process one at a time)
    pub compound: CompoundEntry,
    /// Per-file tag info with cached tag values
    pub files: Vec<FileTagInfo>,
}

// ============================================================================
// Tag Canonicity Data Types
// ============================================================================

/// A tag variant with its occurrence count.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagVariantEntry {
    /// The tag value
    pub value: String,
    /// Number of tracks with this value
    pub count: usize,
}

/// Extended modal data with per-file tag info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagCanonicalityModalDataV2 {
    /// The tag name being canonicalized (e.g., "artist", "album_artist")
    pub tag_name: String,
    /// Optional context label (e.g., "Album: Clockwork Hearts")
    pub context_label: Option<String>,
    /// Variants sorted by count DESC, then alphabetically for ties
    pub variants: Vec<TagVariantEntry>,
    /// Inodes affected by this canonicalization
    pub inodes: Vec<i64>,
    /// Per-file tag info with cached tag values
    pub files: Vec<FileTagInfo>,
    /// Override for the default canonical suggestion (e.g. top corpus variant for inbox canonicity)
    pub default_canonical_override: Option<String>,
}

impl TagCanonicalityModalDataV2 {
    /// Get the default canonical value for pre-filling.
    ///
    /// Returns the most common value. For ties, uses alphabetical order.
    /// Returns empty string if no variants.
    pub fn default_canonical(&self) -> String {
        if let Some(ref override_val) = self.default_canonical_override {
            return override_val.clone();
        }
        self.variants
            .first()
            .map(|v| v.value.clone())
            .unwrap_or_default()
    }
}

// ============================================================================
// Packed Resolution Types (slim, for cluster-nav resolution queries)
// ============================================================================

/// Per-file info for resolution display. Slim — no full tag dump.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolutionFileInfo {
    pub inode: i64,
    pub display_name: String,
}

// === Tag Canonicity (packed, all clusters) ===

/// All canonicity clusters for a tag+zone in one response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagCanonicityResolutionData {
    pub tag_name: String,
    pub clusters: Vec<CanonicityCluster>,
}

/// A single canonicity cluster: the canonical candidate, its count, and outlier variants.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicityCluster {
    pub signal_key: String,
    pub canonical_candidate: String,
    pub canonical_count: usize,
    pub outlier_variants: Vec<OutlierVariant>,
    pub default_canonical: Option<String>,
}

/// A non-canonical variant with the files that carry it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutlierVariant {
    pub value: String,
    pub files: Vec<ResolutionFileInfo>,
}

// === Compound Split (packed, all groups) ===

/// All compound split groups for a tag+zone in one response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompoundSplitResolutionData {
    pub groups: Vec<CompoundSplitCluster>,
}

/// A single compound split group with affected files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompoundSplitCluster {
    pub tag_name: String,
    pub compound_value: String,
    pub split_parts: Vec<String>,
    pub matching_parts: Vec<String>,
    pub files: Vec<ResolutionFileInfo>,
}
