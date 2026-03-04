//! View model types for the release packing browser.

use crate::meta::signals::data::{PackingScoreBreakdown, UnmatchedCorpusTrackData};

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

/// An entry in the flat navigable list.
pub enum PackingListEntry {
    /// Section header for releases.
    ReleaseSectionHeader { count: usize },
    /// A release header row (expandable).
    ReleaseHeader { release_idx: usize, expanded: bool },
    /// An assigned track under a release.
    AssignedTrack { release_idx: usize, track_idx: usize },
    /// An unfilled slot under a release.
    UnfilledSlot { release_idx: usize, slot_idx: usize },
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
