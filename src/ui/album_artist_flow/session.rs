//! Session types for album artist canonicalization flow.
//!
//! Similar to canon_flow/session.rs but for album_artist field.

use crate::corpus::db::changes::PendingChange;

/// Represents one album_artist name bucket with all its variants.
#[derive(Debug, Clone)]
pub struct AlbumArtistBucket {
    /// Normalized form used for grouping (e.g., "the beatles")
    pub normalized_key: String,
    /// Variants with their track counts, sorted descending by count
    pub variants: Vec<AlbumArtistVariant>,
}

impl AlbumArtistBucket {
    /// Get total track count across all variants
    pub fn total_tracks(&self) -> usize {
        self.variants.iter().map(|v| v.track_count).sum()
    }
}

/// A single album_artist name variant within a bucket.
#[derive(Debug, Clone)]
pub struct AlbumArtistVariant {
    /// The exact album_artist name as it appears in track metadata
    pub name: String,
    /// Number of tracks with this exact album_artist name
    pub track_count: usize,
}

/// A decision made about one bucket.
#[derive(Debug, Clone)]
pub struct AlbumArtistDecision {
    /// The bucket this decision applies to
    pub bucket: AlbumArtistBucket,
    /// The canonical name chosen (may be custom-edited)
    pub canonical_name: String,
    /// Variant names that will be renamed to canonical
    pub variants_to_rename: Vec<String>,
    /// Generated pending changes for this decision
    pub pending_changes: Vec<PendingChange>,
}

impl AlbumArtistDecision {
    /// Get count of tracks that will be modified
    pub fn affected_track_count(&self) -> usize {
        self.pending_changes.len()
    }
}

/// Session state for the album_artist canonicalization workflow.
#[derive(Debug, Clone)]
pub struct AlbumArtistCanonSession {
    /// Unique session identifier
    pub session_id: String,
    /// All buckets to process
    pub buckets: Vec<AlbumArtistBucket>,
    /// Current bucket index
    pub current_index: usize,
    /// Completed decisions
    pub decisions: Vec<AlbumArtistDecision>,
}

impl AlbumArtistCanonSession {
    /// Create a new session with the given buckets
    pub fn new(session_id: String, buckets: Vec<AlbumArtistBucket>) -> Self {
        Self {
            session_id,
            buckets,
            current_index: 0,
            decisions: Vec::new(),
        }
    }

    /// Get the current bucket being processed
    pub fn current_bucket(&self) -> Option<&AlbumArtistBucket> {
        self.buckets.get(self.current_index)
    }

    /// Check if all buckets have been processed
    pub fn is_complete(&self) -> bool {
        self.current_index >= self.buckets.len()
    }

    /// Advance to next bucket
    pub fn advance(&mut self) {
        self.current_index += 1;
    }

    /// Go back to previous bucket
    pub fn go_back(&mut self) -> bool {
        if self.current_index > 0 {
            self.current_index -= 1;
            true
        } else {
            false
        }
    }
}
