//! View model types for the knot browser.

use mm_meta::signals::data::PackingScoreBreakdown;

/// Sort mode for releases within a knot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnotReleaseSortMode {
    /// Sort by number of inodes this release claims within the knot (descending).
    ByInodeCount,
    /// Sort by total proposal score (descending).
    ByScore,
}

impl KnotReleaseSortMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::ByInodeCount => "by inode count",
            Self::ByScore => "by score",
        }
    }

    pub fn toggle(&self) -> Self {
        match self {
            Self::ByInodeCount => Self::ByScore,
            Self::ByScore => Self::ByInodeCount,
        }
    }
}

/// A knot (conflict component) for display.
pub struct KnotEntry {
    pub tier: String,
    pub proposal_count: usize,
    pub inode_count: usize,
    pub ratio: f64,
    pub classification: String,
    pub proposals: Vec<KnotProposal>,
}

/// A release proposal within a knot.
pub struct KnotProposal {
    pub release_id: String,
    pub release_title: String,
    pub release_artist: String,
    pub total_tracks: i32,
    pub total_score: f64,
    /// Number of inodes this release claims within the knot.
    pub covered_inode_count: usize,
    /// Whether this proposal was selected by greedy resolution.
    pub selected: bool,
    pub tracks: Vec<KnotProposalTrack>,
}

/// A track assignment within a knot proposal.
pub struct KnotProposalTrack {
    pub path: String,
    pub medium_pos: i32,
    pub track_pos: i32,
    pub track_title: String,
    pub score: f64,
    pub score_breakdown: PackingScoreBreakdown,
}
