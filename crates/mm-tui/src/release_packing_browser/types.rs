//! View model types for the release packing browser.

use std::collections::BTreeSet;

use ratatui::text::Line;

use mm_meta::signals::data::PackingScoreBreakdown;
use crate::widgets::rich_text::RichBlock;
use crate::widgets::standard_list::ListEntry;
use crate::widgets::{WizardItem, WizardOffer};

// Re-export shared PackingCategory from its canonical location.
pub use mm_meta::signals::packing_category::PackingCategory;

/// A release group with its assigned tracks and unfilled slots.
pub struct ReleaseGroup {
    pub release_id: String,
    pub release_title: String,
    pub release_artist: String,
    pub tracks: Vec<AssignedTrackInfo>,
    pub unfilled: Vec<UnfilledSlotInfo>,
    pub coverage: f32,
    pub total_tracks: u32,
    pub alternatives: Vec<AlternativeReleaseInfo>,
    pub va_override: Option<VaOverrideInfo>,
    /// For LowConfidence releases: the metrics that triggered the downgrade.
    pub low_confidence_reason: Option<LowConfidenceReason>,
}

/// Why a release was downgraded to LowConfidence.
pub struct LowConfidenceReason {
    pub acoustid_ratio: f64,
    pub avg_album_match: f64,
}

/// An alternative release that packs identically to a winning release.
pub struct AlternativeReleaseInfo {
    pub release_id: String,
    pub release_title: String,
    pub release_artist: String,
    pub alternative_score: f64,
    pub winner_score: f64,
    pub inode_count: u32,
}

/// A VA override suggestion for a winning release.
pub struct VaOverrideInfo {
    pub suggested_artist: String,
    pub source: String,
}

/// An assigned track within a release.
pub struct AssignedTrackInfo {
    pub inode: i64,
    pub path: String,
    pub track_title: String,
    pub medium_position: u32,
    pub track_position: u32,
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
}

/// An unmatched corpus file (not assigned to any release).
pub struct UnmatchedEntry {
    pub path: String,
}

/// An entry in the flat navigable list.
///
/// Release entries carry pre-built wizard content (popup for release overview,
/// pane for interleaved tracks + score breakdowns).
pub(crate) enum PackingListEntry {
    /// A release row with wizard content.
    Release {
        idx: usize,
        /// Release overview for wizard popup (z key).
        popup_lines: Vec<Line<'static>>,
        /// Title for wizard pane.
        pane_title: String,
        /// Interleaved track list + score cards for wizard pane (Z key).
        pane_content: Vec<RichBlock>,
    },
    /// An unmatched corpus file.
    Unmatched { idx: usize },
}

impl WizardItem for PackingListEntry {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        match self {
            PackingListEntry::Release {
                popup_lines,
                pane_title,
                pane_content,
                ..
            } => Some(WizardOffer::Both {
                popup: popup_lines.clone(),
                pane_title: pane_title.clone(),
                pane_content: pane_content.clone(),
            }),
            PackingListEntry::Unmatched { .. } => None,
        }
    }
}

impl ListEntry for PackingListEntry {
    type Action = super::ReleasePackingBrowserAction;

    fn on_confirm(&self, selected: &BTreeSet<usize>) -> Option<Self::Action> {
        Some(super::ReleasePackingBrowserAction::ApproveSelected {
            selected_indices: selected.clone(),
        })
    }

    fn is_selectable(&self) -> bool {
        matches!(self, PackingListEntry::Release { .. })
    }
}
