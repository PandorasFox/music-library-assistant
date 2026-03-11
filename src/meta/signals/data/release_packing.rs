//! Release packing signal types.

use serde::{Deserialize, Serialize};

use super::PackingScoreBreakdown;

// ============================================================================
// Release Packing (inode-keyed)
// ============================================================================

/// Release bin-packing result for a corpus file.
/// Inode-keyed, one signal per corpus file that was successfully packed into a release.
#[derive(Debug, Clone)]
pub struct ReleasePackingSignal {
    pub inode: i64,
    pub path: String,
    /// Serialized as bincode BLOB.
    pub data: ReleasePackingData,
}

/// How a corpus file was matched to a release track slot.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MatchMethod {
    /// Matched via AcoustID fingerprint lookup.
    AcoustId,
    /// Matched by elimination: all siblings assigned to same release,
    /// remaining files fill remaining slots.
    Elimination,
}

/// Bincode-serialized payload for ReleasePacking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleasePackingData {
    /// Best-matching MusicBrainz release MBID.
    pub release_id: String,
    /// Release title (denormalized for display without cache lookup).
    pub release_title: String,
    /// Release artist credit (joined string).
    pub release_artist: String,
    /// Track position within the medium (1-indexed).
    pub track_position: u32,
    /// Medium position (disc number, 1-indexed).
    pub medium_position: u32,
    /// Medium format (e.g. "CD", "Digital Media", "12\" Vinyl").
    pub medium_format: Option<String>,
    /// Track number string from the release (e.g. "A1" for vinyl, "3" for CD).
    pub track_number: String,
    /// Recording MBID at this track position.
    pub recording_id: String,
    /// Track title from the release tracklist.
    pub track_title: String,
    /// Composite confidence score (0.0-1.0).
    pub score: f64,
    /// Score breakdown for debugging/display.
    pub score_breakdown: PackingScoreBreakdown,
    /// Number of alternative releases considered for this inode.
    pub alternatives_count: u16,
    /// Release coverage: fraction of this release's tracks that are matched.
    pub release_coverage: f32,
    /// How this file was matched to its track slot.
    pub match_method: MatchMethod,
}

// ============================================================================
// Release Packing Gap Analysis Signals
// ============================================================================

/// Classification of unsolved corpus tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsolvedCategory {
    /// Had AcoustID match, was scored for releases, lost MIS conflict resolution.
    Conflict,
    /// Had AcoustID match but was never optimally scored for any release.
    NoRelease,
    /// Fingerprinted but no AcoustID recording match at all.
    NoMatch,
}

impl_as_str!(UnsolvedCategory, as_str, [
    Conflict => "conflict",
    NoRelease => "no_release",
    NoMatch => "no_match",
]);

/// Fingerprinted corpus inode not assigned to any release after packing.
/// (Corpus signal, inode PK)
#[derive(Debug, Clone)]
pub struct UnmatchedCorpusTrackSignal {
    pub inode: i64,
    pub path: String,
    pub category: UnsolvedCategory,
    /// Serialized as bincode BLOB.
    pub data: UnmatchedCorpusTrackData,
}

/// Bincode payload for UnmatchedCorpusTrack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnmatchedCorpusTrackData {
    /// Recording MBIDs this inode matched via AcoustID.
    pub recording_ids: Vec<String>,
    /// Release MBIDs where this inode was a candidate but lost conflict resolution.
    pub considered_release_ids: Vec<String>,
}

/// Release track position with no matching corpus file after global assignment.
/// (Aggregate signal, key = `{release_id}:{medium_pos}:{track_pos}`)
#[derive(Debug, Clone)]
pub struct UnfilledReleaseSlotSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: UnfilledReleaseSlotData,
}

/// Bincode payload for UnfilledReleaseSlot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnfilledReleaseSlotData {
    pub release_id: String,
    pub release_title: String,
    pub release_artist: String,
    pub medium_pos: u32,
    pub track_pos: u32,
    pub track_title: String,
    pub recording_id: String,
    pub filled_count: u32,
    pub total_tracks: u32,
}

/// Per-release aggregate packing result.
/// Key = `{category_prefix}:{release_id}` for SQL-level filtering.
/// (Aggregate signal, key = `full_match:{release_id}` | `single:{release_id}` | `incomplete:{release_id}`)
#[derive(Debug, Clone)]
pub struct PackedReleaseSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: PackedReleaseData,
}

/// Category of a packed release in the optimal solution.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PackedReleaseCategory {
    /// Every track matched via AcoustID fingerprint (highest confidence).
    Perfect,
    /// All slots filled, but some tracks matched via elimination/scoring.
    FullMatch,
    /// Single-track release.
    Single,
    /// Some but not all slots filled (includes former near-misses).
    Incomplete,
    /// FullMatch/Incomplete with poor AcoustID coverage and low album match —
    /// likely mispack from elimination filling slots on wrong release.
    LowConfidence,
}

impl_as_str!(PackedReleaseCategory, key_prefix, [
    Perfect => "perfect",
    FullMatch => "full_match",
    Single => "single",
    Incomplete => "incomplete",
    LowConfidence => "low_confidence",
]);

/// Bincode payload for PackedRelease.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackedReleaseData {
    pub release_id: String,
    pub release_title: String,
    pub release_artist: String,
    pub category: PackedReleaseCategory,
    pub assigned_count: u32,
    pub total_tracks: u32,
    /// For LowConfidence: the AcoustID ratio that triggered the downgrade.
    pub low_confidence_acoustid_ratio: Option<f64>,
    /// For LowConfidence: the avg album_match that triggered the downgrade.
    pub low_confidence_avg_album_match: Option<f64>,
}

impl PackedReleaseData {
    /// Deserialize from bincode, handling blobs written before the
    /// low_confidence metric fields were added.
    pub fn deserialize_compat(blob: &[u8]) -> Result<Self, bincode::Error> {
        match bincode::deserialize::<Self>(blob) {
            Ok(data) => Ok(data),
            Err(_) => {
                #[derive(Deserialize)]
                struct Legacy {
                    release_id: String,
                    release_title: String,
                    release_artist: String,
                    category: PackedReleaseCategory,
                    assigned_count: u32,
                    total_tracks: u32,
                }
                let legacy: Legacy = bincode::deserialize(blob)?;
                Ok(Self {
                    release_id: legacy.release_id,
                    release_title: legacy.release_title,
                    release_artist: legacy.release_artist,
                    category: legacy.category,
                    assigned_count: legacy.assigned_count,
                    total_tracks: legacy.total_tracks,
                    low_confidence_acoustid_ratio: None,
                    low_confidence_avg_album_match: None,
                })
            }
        }
    }
}

// ============================================================================
// Packing Knot signal (aggregate, key-keyed)
// ============================================================================

/// Knot component from MIS conflict resolution.
/// Key = `{tier}:{knot_id}` (e.g., `full_match:3`).
#[derive(Debug, Clone)]
pub struct PackingKnotSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: PackingKnotData,
}

/// Bincode payload for PackingKnot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackingKnotData {
    pub tier: String,
    pub knot_id: usize,
    pub classification: KnotClassification,
    pub ratio: f64,
    pub contested_inodes: Vec<i64>,
    pub proposals: Vec<KnotProposalEntry>,
}

/// How a knot component was classified for extraction.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum KnotClassification {
    /// proposals/inodes ratio exceeded threshold.
    ByRatio,
    /// Component size exceeded limit.
    BySize,
}

/// A release proposal within a knot component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnotProposalEntry {
    pub release_id: String,
    pub release_title: String,
    pub release_artist: String,
    pub total_tracks: i32,
    pub total_score: f64,
    /// Whether this proposal was selected by greedy resolution.
    pub selected: bool,
    pub assignments: Vec<KnotAssignment>,
}

/// A single track assignment within a knot proposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnotAssignment {
    pub inode: i64,
    pub recording_id: String,
    pub medium_pos: i32,
    pub track_pos: i32,
    pub track_title: String,
    pub score: f64,
    pub score_breakdown: PackingScoreBreakdown,
    pub match_method: i32,
}

// ============================================================================
// Alternative Release Packing signal (aggregate, key-keyed)
// ============================================================================

/// Alternative release with identical inode signature to a winning release.
/// Key = `{winner_release_id}:{alt_release_id}`.
#[derive(Debug, Clone)]
pub struct AlternativeReleasePackingSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: AlternativeReleasePackingData,
}

/// Bincode payload for AlternativeReleasePacking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlternativeReleasePackingData {
    pub winner_release_id: String,
    pub winner_release_title: String,
    pub alternative_release_id: String,
    pub alternative_release_title: String,
    pub alternative_release_artist: String,
    pub alternative_score: f64,
    pub winner_score: f64,
    pub inode_count: u32,
}

// ============================================================================
// Various Artists Override signal (aggregate, key-keyed)
// ============================================================================

/// Suggested non-VA artist override for a winning release with "Various Artists".
/// Key = winner `release_id`.
#[derive(Debug, Clone)]
pub struct VariousArtistsOverrideSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: VariousArtistsOverrideData,
}

/// Bincode payload for VariousArtistsOverride.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariousArtistsOverrideData {
    pub release_id: String,
    pub release_title: String,
    pub suggested_artist: String,
    pub source: VariousArtistsOverrideSource,
}

/// How the VA override artist name was discovered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VariousArtistsOverrideSource {
    /// From an exact alternative (same inode signature, different release).
    ExactAlternative,
    /// From a competing proposal in the same component that overlaps the winner's inodes.
    CompetingProposal,
}

// ============================================================================
// Pinned Release Conflict signal (aggregate, key-keyed)
// ============================================================================

/// Invariant violation: a release is pinned by more directories than it has media.
/// Key = release_id. This is a hard stop — the release is skipped entirely.
#[derive(Debug, Clone)]
pub struct PinnedReleaseConflictSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: PinnedReleaseConflictData,
}

/// Bincode payload for PinnedReleaseConflict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinnedReleaseConflictData {
    pub release_id: String,
    pub release_title: String,
    pub release_artist: String,
    pub media_count: i32,
    pub directories: Vec<String>,
    pub reason: String,
}

// ============================================================================
// impl_content_hash! invocations for release packing signals
// ============================================================================

use crate::meta::signals::registry::SignalContentHash;
use std::hash::Hash;

impl_content_hash!(ReleasePackingSignal => blob(data));
impl_content_hash!(UnmatchedCorpusTrackSignal => blob(data));
impl_content_hash!(UnfilledReleaseSlotSignal => blob(data));
impl_content_hash!(PackedReleaseSignal => blob(data));
impl_content_hash!(PackingKnotSignal => blob(data));
impl_content_hash!(AlternativeReleasePackingSignal => blob(data));
impl_content_hash!(VariousArtistsOverrideSignal => blob(data));
impl_content_hash!(PinnedReleaseConflictSignal => blob(data));
