//! View model types for the release packing browser.

use crate::meta::signals::data::{PackingScoreBreakdown, UnmatchedCorpusTrackData};

/// Which pane currently has focus.
pub enum FocusedPane {
    /// Left pane: release/near-miss/unmatched list.
    LeftPane,
    /// Middle pane: tracks list for selected release.
    MiddlePane,
    /// Bottom-right pane: per-track detail (scrollable).
    DetailPane,
}

/// A release group with its assigned tracks and unfilled slots.
pub struct ReleaseGroup {
    pub release_id: String,
    pub release_title: String,
    pub release_artist: String,
    pub tracks: Vec<AssignedTrackInfo>,
    pub unfilled: Vec<UnfilledSlotInfo>,
    pub coverage: f32,
    pub total_tracks: u32,
}

/// An assigned track within a release.
pub struct AssignedTrackInfo {
    pub path: String,
    pub track_number: String,
    pub track_title: String,
    pub medium_position: u32,
    pub track_position: u32,
    pub medium_format: Option<String>,
    pub recording_id: String,
    pub score: f64,
    pub score_breakdown: PackingScoreBreakdown,
    pub alternatives_count: u16,
}

/// An unfilled slot within a release.
pub struct UnfilledSlotInfo {
    pub medium_pos: u32,
    pub track_pos: u32,
    pub track_title: String,
    pub recording_id: String,
}

/// An unmatched corpus file (not assigned to any release).
pub struct UnmatchedEntry {
    pub path: String,
    pub data: UnmatchedCorpusTrackData,
}

/// An entry in the flat left-pane navigable list.
/// Tracks are shown in the middle pane, not inline here.
pub enum PackingListEntry {
    /// Section header for releases.
    ReleaseSectionHeader { count: usize },
    /// A release row.
    ReleaseHeader { release_idx: usize },
    /// Section header for near-misses.
    NearMissSectionHeader { count: usize },
    /// A near-miss release entry.
    NearMissEntry { idx: usize },
    /// Section header for unmatched files.
    UnmatchedSectionHeader { count: usize },
    /// An unmatched corpus file.
    UnmatchedFile { idx: usize },
}

impl PackingListEntry {
    /// Whether this entry is a non-navigable section header.
    pub fn is_section_header(&self) -> bool {
        matches!(
            self,
            Self::ReleaseSectionHeader { .. }
                | Self::NearMissSectionHeader { .. }
                | Self::UnmatchedSectionHeader { .. }
        )
    }
}
