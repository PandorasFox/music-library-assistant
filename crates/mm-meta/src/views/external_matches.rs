//! External match browse + review types.
//!
//! Shared filter enums for URL routes and domain queries,
//! plus response types for packed queries.

use serde::{Deserialize, Serialize};

// ============================================================================
// Filter Enums (shared by routes + queries)
// ============================================================================

/// AcoustID confidence filter for browse routes.
///
/// Simplified grouping over `ConfidenceTier` for URL parameters:
/// - `High` → Perfect + VeryHigh + High tiers
/// - `Medium` → Medium tier
/// - `Low` → Low tier
/// - `All` → all tiers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AcoustidConfidence {
    High,
    Medium,
    Low,
    All,
}

impl AcoustidConfidence {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::All => "all",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "high" => Some(Self::High),
            "medium" => Some(Self::Medium),
            "low" => Some(Self::Low),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::High => "High Confidence (95%+)",
            Self::Medium => "Medium Confidence (90-95%)",
            Self::Low => "Low Confidence (<90%)",
            Self::All => "All Matches",
        }
    }

    /// Whether a given confidence score passes this filter.
    pub fn matches(&self, confidence: f64) -> bool {
        match self {
            Self::High => confidence >= 0.95,
            Self::Medium => confidence >= 0.90 && confidence < 0.95,
            Self::Low => confidence < 0.90,
            Self::All => true,
        }
    }
}

/// Release review filter for browse routes.
///
/// Maps to `PackingCategory` variants for releases (excludes Knots and Unsolved*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReleaseReviewFilter {
    Perfect,
    FullMatch,
    Singles,
    Incomplete,
    LowConfidence,
    All,
}

impl ReleaseReviewFilter {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Perfect => "perfect",
            Self::FullMatch => "full-match",
            Self::Singles => "singles",
            Self::Incomplete => "incomplete",
            Self::LowConfidence => "low-confidence",
            Self::All => "all",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "perfect" => Some(Self::Perfect),
            "full-match" => Some(Self::FullMatch),
            "singles" => Some(Self::Singles),
            "incomplete" => Some(Self::Incomplete),
            "low-confidence" => Some(Self::LowConfidence),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Perfect => "Perfect Matches",
            Self::FullMatch => "Full Matches",
            Self::Singles => "Singles",
            Self::Incomplete => "Incomplete Releases",
            Self::LowConfidence => "Low Confidence",
            Self::All => "All Releases",
        }
    }
}

// ============================================================================
// AcoustID Browse Response Types
// ============================================================================

/// A single AcoustID match entry for the browse view.
///
/// Server joins ExternalMatchSignal data with MB recording cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcoustidMatchEntry {
    pub inode: i64,
    pub display_name: String,
    pub confidence: f64,
    pub recording_id: String,
    pub recording_title: Option<String>,
    pub recording_artist: Option<String>,
    pub recording_length_ms: Option<i64>,
}

// ============================================================================
// Release Review Response Types
// ============================================================================

/// Packed response for release review browse.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReleaseReviewData {
    pub releases: Vec<ReviewableRelease>,
}

/// A release with its track assignments, ready for operator review/approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewableRelease {
    pub release_id: String,
    pub title: String,
    pub artist: String,
    pub track_count: usize,
    pub matched_count: usize,
    pub avg_confidence: f64,
    pub category: String,
    pub tracks: Vec<ReviewableTrack>,
}

/// A single track within a reviewable release.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewableTrack {
    pub position: usize,
    pub medium_position: u32,
    pub mb_title: String,
    pub mb_artist: String,
    pub recording_id: String,
    pub matched_inode: Option<i64>,
    pub matched_display_name: Option<String>,
    pub confidence: Option<f64>,
}

// ============================================================================
// Approval Types (shared between TUI and web)
// ============================================================================

/// Input for building approval decisions from selected releases.
///
/// Both TUI and web clients construct this from their selection state
/// and pass it to the shared approval builder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseApprovalInput {
    pub release_id: String,
    pub tracks: Vec<ApprovalTrackInput>,
}

/// Per-track data needed for approval tag generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalTrackInput {
    pub inode: i64,
    pub recording_id: String,
    pub track_title: String,
    pub track_position: u32,
    pub medium_position: u32,
}

/// A computed approval decision ready for staging.
///
/// Produced by the shared approval builder, consumed by
/// client-specific staging code.
#[derive(Debug, Clone)]
pub struct ApprovalDecision {
    pub release_id: String,
    pub label: String,
    pub ops: Vec<crate::mutations::TagOp>,
}
