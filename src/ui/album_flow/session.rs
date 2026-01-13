//! Session types for album canonicalization flow.

use super::types::{AlbumBucket, AlbumDecision};

/// Session state for the album canonicalization workflow.
#[derive(Debug, Clone)]
pub struct AlbumCanonSession {
    /// Unique session identifier
    pub session_id: String,
    /// All buckets to process
    pub buckets: Vec<AlbumBucket>,
    /// Current bucket index
    pub current_index: usize,
    /// Completed decisions
    pub decisions: Vec<AlbumDecision>,
}

impl AlbumCanonSession {
    /// Create a new session with the given buckets.
    pub fn new(session_id: String, buckets: Vec<AlbumBucket>) -> Self {
        Self {
            session_id,
            buckets,
            current_index: 0,
            decisions: Vec::new(),
        }
    }

    /// Get the current bucket being processed.
    pub fn current_bucket(&self) -> Option<&AlbumBucket> {
        self.buckets.get(self.current_index)
    }

    /// Check if all buckets have been processed.
    pub fn is_complete(&self) -> bool {
        self.current_index >= self.buckets.len()
    }

    /// Advance to next bucket.
    pub fn advance(&mut self) {
        self.current_index += 1;
    }

    /// Go back to previous bucket.
    pub fn go_back(&mut self) -> bool {
        if self.current_index > 0 {
            self.current_index -= 1;
            true
        } else {
            false
        }
    }

    /// Get count of decisions made.
    pub fn decision_count(&self) -> usize {
        self.decisions.len()
    }

    /// Get total affected tracks across all decisions.
    pub fn total_affected_tracks(&self) -> usize {
        self.decisions.iter().map(|d| d.affected_track_count()).sum()
    }
}
