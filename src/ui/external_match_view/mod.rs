//! External Matches lateral view — browse AcoustID matches by confidence tier.
//!
//! Two sections in one flat navigable list:
//! - **Actions**: Fetch AcoustID Data (kicks off lookup via Witch)
//! - **Matches**: Confidence-bucketed entries (untagged files + confidence tiers)
//!
//! Enter on a match bucket launches the existing `external_match_modal` review flow.

pub mod render;

use crossterm::event::{KeyCode, KeyEvent};

use crate::meta::views::{ConfidenceTier, ExternalMatchesData};

// ============================================================================
// Actions
// ============================================================================

/// Actions produced by Phase 1 key dispatch.
pub(crate) enum ExternalMatchesAction {
    None,
    /// Tab → next lateral view
    CycleNext,
    /// Shift-Tab → previous lateral view
    CyclePrev,
    /// Esc → quit request
    RequestQuit,
    /// Enter on "Fetch AcoustID Data" entry
    RequestFetch,
    /// Enter on "Untagged files" bucket → launch review for MetadataOnly entries
    LaunchUntaggedReview,
    /// Enter on a confidence bucket → launch review for entries in that tier
    LaunchTierReview(ConfidenceTier),
}

// ============================================================================
// Navigable Entry
// ============================================================================

/// A navigable entry in the left pane (cursor indexes into this list).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NavigableEntry {
    /// "Fetch AcoustID Data" action entry (always present)
    FetchAction,
    /// "Untagged files" bucket (MetadataOnly entries)
    UntaggedBucket,
    /// Confidence tier bucket (ContentDiff entries at this tier)
    ConfidenceBucket(ConfidenceTier),
}

// ============================================================================
// State
// ============================================================================

/// Main view state for the External Matches lateral view.
pub(crate) struct ExternalMatchesViewState {
    /// Cached data from the cache thread
    pub cached_data: Option<ExternalMatchesData>,
    /// Flat cursor across navigable entries
    pub cursor: usize,
    pub scroll: usize,
    /// Whether the Witch has an active fetch batch
    pub fetch_active: bool,
    /// Whether an AcoustID API key is configured
    pub has_api_key: bool,
    /// Latest fetch progress snapshot from the Witch
    pub fetch_progress: Option<crate::witch::external_fetch::FetchProgress>,
}

// ============================================================================
// Construction & Update
// ============================================================================

impl ExternalMatchesViewState {
    pub fn new(fetch_active: bool, has_api_key: bool) -> Self {
        Self {
            cached_data: None,
            cursor: 0,
            scroll: 0,
            fetch_active,
            has_api_key,
            fetch_progress: None,
        }
    }

    /// Update cached data from cache thread.
    pub fn update(&mut self, data: ExternalMatchesData) {
        self.cached_data = Some(data);
        // Clamp cursor to valid range
        let count = self.navigable_entries().len();
        if count > 0 && self.cursor >= count {
            self.cursor = count - 1;
        }
    }

    /// Build the navigable entry list from cached data.
    pub fn navigable_entries(&self) -> Vec<NavigableEntry> {
        let mut entries = vec![NavigableEntry::FetchAction];

        if let Some(ref data) = self.cached_data {
            if !data.untagged_entries.is_empty() {
                entries.push(NavigableEntry::UntaggedBucket);
            }
            for bucket in &data.confidence_buckets {
                entries.push(NavigableEntry::ConfidenceBucket(bucket.tier));
            }
        }

        entries
    }

    /// Get the currently selected navigable entry.
    pub fn selected_entry(&self) -> Option<NavigableEntry> {
        self.navigable_entries().into_iter().nth(self.cursor)
    }
}

// ============================================================================
// Key Handling (Phase 1)
// ============================================================================

impl ExternalMatchesViewState {
    pub fn handle_key(&mut self, key: KeyEvent) -> ExternalMatchesAction {
        let entries = self.navigable_entries();
        let entry_count = entries.len();

        match key.code {
            KeyCode::Up => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                ExternalMatchesAction::None
            }
            KeyCode::Down => {
                if entry_count > 0 && self.cursor < entry_count - 1 {
                    self.cursor += 1;
                }
                ExternalMatchesAction::None
            }
            KeyCode::Home => {
                self.cursor = 0;
                ExternalMatchesAction::None
            }
            KeyCode::End => {
                if entry_count > 0 {
                    self.cursor = entry_count - 1;
                }
                ExternalMatchesAction::None
            }
            KeyCode::Enter => {
                match entries.get(self.cursor) {
                    Some(NavigableEntry::FetchAction) => {
                        if self.has_api_key && !self.fetch_active {
                            ExternalMatchesAction::RequestFetch
                        } else {
                            ExternalMatchesAction::None
                        }
                    }
                    Some(NavigableEntry::UntaggedBucket) => {
                        ExternalMatchesAction::LaunchUntaggedReview
                    }
                    Some(NavigableEntry::ConfidenceBucket(tier)) => {
                        ExternalMatchesAction::LaunchTierReview(*tier)
                    }
                    None => ExternalMatchesAction::None,
                }
            }
            KeyCode::Tab => ExternalMatchesAction::CycleNext,
            KeyCode::BackTab => ExternalMatchesAction::CyclePrev,
            KeyCode::Esc => ExternalMatchesAction::RequestQuit,
            _ => ExternalMatchesAction::None,
        }
    }
}
