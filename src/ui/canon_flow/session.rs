//! Session types for artist canonicalization flow.
//!
//! Defines the data structures for tracking canonicalization decisions
//! across the multi-phase dialogue workflow.

use crate::db::changes::PendingChange;

/// Represents one artist name bucket with all its variants.
/// Buckets are formed by normalizing artist names (lowercase, trimmed).
#[derive(Debug, Clone)]
pub struct ArtistBucket {
    /// Normalized form used for grouping (e.g., "the beatles")
    pub normalized_key: String,
    /// Variants with their track counts, sorted descending by count
    pub variants: Vec<ArtistVariant>,
}

impl ArtistBucket {
    /// Get total track count across all variants
    pub fn total_tracks(&self) -> usize {
        self.variants.iter().map(|v| v.track_count).sum()
    }

    /// Get count of selected variants
    pub fn selected_count(&self) -> usize {
        self.variants.iter().filter(|v| v.selected_for_squash).count()
    }

    /// Get the selected variants
    pub fn selected_variants(&self) -> Vec<&ArtistVariant> {
        self.variants.iter().filter(|v| v.selected_for_squash).collect()
    }
}

/// A single artist name variant within a bucket.
#[derive(Debug, Clone)]
pub struct ArtistVariant {
    /// The exact artist name as it appears in track metadata
    pub name: String,
    /// Number of tracks with this exact artist name
    pub track_count: usize,
    /// Whether this variant is marked for squashing (Phase 1 selection)
    pub selected_for_squash: bool,
}

/// A decision made about one bucket.
#[derive(Debug, Clone)]
pub struct CanonDecision {
    /// The bucket this decision applies to
    pub bucket: ArtistBucket,
    /// The canonical name chosen (may be custom-edited)
    pub canonical_name: String,
    /// Variant names that will be renamed to canonical
    pub variants_to_rename: Vec<String>,
    /// Generated pending changes for this decision
    pub pending_changes: Vec<PendingChange>,
}

impl CanonDecision {
    /// Get count of tracks that will be modified
    pub fn affected_track_count(&self) -> usize {
        self.pending_changes.len()
    }
}

/// Session state for the entire canonicalization workflow.
#[derive(Debug, Clone)]
pub struct CanonSession {
    /// Unique session identifier
    pub session_id: String,
    /// All buckets to process
    pub buckets: Vec<ArtistBucket>,
    /// Current bucket index
    pub current_index: usize,
    /// Completed decisions
    pub decisions: Vec<CanonDecision>,
}

impl CanonSession {
    /// Create a new session with the given buckets
    pub fn new(session_id: String, buckets: Vec<ArtistBucket>) -> Self {
        Self {
            session_id,
            buckets,
            current_index: 0,
            decisions: Vec::new(),
        }
    }

    /// Get the current bucket being processed
    pub fn current_bucket(&self) -> Option<&ArtistBucket> {
        self.buckets.get(self.current_index)
    }

    /// Get mutable reference to current bucket
    pub fn current_bucket_mut(&mut self) -> Option<&mut ArtistBucket> {
        self.buckets.get_mut(self.current_index)
    }

    /// Check if all buckets have been processed
    pub fn is_complete(&self) -> bool {
        self.current_index >= self.buckets.len()
    }

    /// Get total count of tracks affected by all decisions
    pub fn total_affected_tracks(&self) -> usize {
        self.decisions.iter().map(|d| d.affected_track_count()).sum()
    }

    /// Get count of remaining buckets
    pub fn remaining_buckets(&self) -> usize {
        self.buckets.len().saturating_sub(self.current_index)
    }

    /// Advance to next bucket
    pub fn advance(&mut self) {
        self.current_index += 1;
    }

    /// Go back to previous bucket.
    /// Decisions are preserved - this is navigation, not undo.
    pub fn go_back(&mut self) -> bool {
        if self.current_index > 0 {
            self.current_index -= 1;
            true
        } else {
            false
        }
    }

    /// Get all pending changes from all decisions
    pub fn all_pending_changes(&self) -> Vec<&PendingChange> {
        self.decisions
            .iter()
            .flat_map(|d| &d.pending_changes)
            .collect()
    }
}

// TODO: Health check for tags mismatching on-disk vs in-index
// - Resolution options: flush index to disk OR accept out-of-band changes
// - Granularity TBD (potentially per-directory config)
// - Uses existing ChangeType::OutOfBandTagChange
// - Similar flow structure could be reused for tag mismatch resolution
