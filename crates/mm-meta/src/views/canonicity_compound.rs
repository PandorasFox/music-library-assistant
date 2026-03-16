//! Tag canonicity and compound split modal data types.

use serde::{Deserialize, Serialize};

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
// Resolution Types (slim, for cluster-nav resolution queries)
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

/// A single canonicity cluster: all variants in the collision group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicityCluster {
    pub signal_key: String,
    /// All tag value variants in this collision group (including the majority).
    pub variants: Vec<Variant>,
    /// If a CanonicalTagSignal exists, the confirmed canonical value.
    /// None means no canonical has been established — operator must decide.
    pub confirmed_canonical: Option<String>,
    /// Suggested canonical for DecisionField pre-fill (majority value or confirmed).
    pub suggested_canonical: Option<String>,
}

/// A tag value variant with the files that carry it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variant {
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
