//! External Match Review — action type only.
//!
//! Read-only browser for AcoustID/MusicBrainz matches. No mutations.
//! The state uses StandardList with wizard system, not ResolutionState.
//! Only the action enum is lifted here.
//!
//! Route: `/resolve/external-match-review`

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalMatchReviewAction {
    None,
    Cancel,
    OpenRecordingUrl(String),
}
