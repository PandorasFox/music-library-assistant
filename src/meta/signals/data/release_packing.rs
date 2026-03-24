//! Release packing signal types.


// Re-export pure data types from mm-meta
pub use mm_meta::signals::data::{
    ReleasePackingData, UnsolvedCategory,
    UnmatchedCorpusTrackData, UnfilledReleaseSlotData, PackedReleaseData,
    PackingKnotData,
    AlternativeReleasePackingData,
    VariousArtistsOverrideData,
    PinnedReleaseConflictData,
    PinnedReleasePackFailureData,
};

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

// ============================================================================
// Release Packing Gap Analysis Signals
// ============================================================================

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

/// Release track position with no matching corpus file after global assignment.
/// (Aggregate signal, key = `{release_id}:{medium_pos}:{track_pos}`)
#[derive(Debug, Clone)]
pub struct UnfilledReleaseSlotSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: UnfilledReleaseSlotData,
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

// ============================================================================
// Pinned Release Pack Failure signal (aggregate, key-keyed)
// ============================================================================

/// A pinned release failed to fully pack against its directory.
/// Key = `{release_id}:{dir_path}`.
#[derive(Debug, Clone)]
pub struct PinnedReleasePackFailureSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: PinnedReleasePackFailureData,
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
impl_content_hash!(PinnedReleasePackFailureSignal => blob(data));
