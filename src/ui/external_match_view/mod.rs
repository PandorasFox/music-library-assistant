//! External Matches lateral view — browse AcoustID matches by confidence tier.
//!
//! Two sections in one flat navigable list:
//! - **Actions**: Cache external metadata matches (kicks off lookup via Witch)
//! - **Matches**: Confidence-bucketed entries (untagged files + confidence tiers)
//!
//! Enter on a match bucket launches the existing `external_match_modal` review flow.

pub mod render;

use crate::ui::input::InputAction;

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
    /// Enter on "Cache external metadata matches" entry
    RequestFetch,
    /// Enter on "Analyze release matches" entry
    RequestReleasePacking,
    /// Enter on a confidence bucket → launch review for entries in that tier
    LaunchTierReview(ConfidenceTier),
}

// ============================================================================
// Navigable Entry
// ============================================================================

/// A navigable entry in the left pane (cursor indexes into this list).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NavigableEntry {
    /// "Cache external metadata matches" action entry (always present)
    FetchAction,
    /// "Analyze release matches" action entry (always present)
    PackReleasesAction,
    /// Confidence tier bucket
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
    /// Click targets for navigable entries (set during render).
    pub click_targets: crate::ui::widgets::ListClickTargets,
    /// Animation tick counter (incremented each UI tick while fetch is active).
    pub tick_count: u32,
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
            click_targets: Default::default(),
            tick_count: 0,
        }
    }

    /// Handle mouse click for cursor selection.
    pub fn handle_click(&mut self, x: u16, y: u16) {
        if let Some(id) = self.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                let total = self.navigable_entries().len();
                if idx < total {
                    self.cursor = idx;
                }
            }
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
        let mut entries = vec![NavigableEntry::FetchAction, NavigableEntry::PackReleasesAction];

        if let Some(ref data) = self.cached_data {
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
    pub fn handle_input(&mut self, action: &InputAction) -> ExternalMatchesAction {
        let entries = self.navigable_entries();
        let entry_count = entries.len();

        match action {
            InputAction::NavUp => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                ExternalMatchesAction::None
            }
            InputAction::NavDown => {
                if entry_count > 0 && self.cursor < entry_count - 1 {
                    self.cursor += 1;
                }
                ExternalMatchesAction::None
            }
            InputAction::Home => {
                self.cursor = 0;
                ExternalMatchesAction::None
            }
            InputAction::End => {
                if entry_count > 0 {
                    self.cursor = entry_count - 1;
                }
                ExternalMatchesAction::None
            }
            InputAction::Confirm => {
                match entries.get(self.cursor) {
                    Some(NavigableEntry::FetchAction) => {
                        if self.has_api_key && !self.fetch_active {
                            ExternalMatchesAction::RequestFetch
                        } else {
                            ExternalMatchesAction::None
                        }
                    }
                    Some(NavigableEntry::PackReleasesAction) => {
                        let has_data = self.cached_data.as_ref().is_some_and(|d| {
                            !d.confidence_buckets.is_empty()
                        });
                        if !self.fetch_active && has_data {
                            ExternalMatchesAction::RequestReleasePacking
                        } else {
                            ExternalMatchesAction::None
                        }
                    }
                    Some(NavigableEntry::ConfidenceBucket(tier)) => {
                        ExternalMatchesAction::LaunchTierReview(*tier)
                    }
                    None => ExternalMatchesAction::None,
                }
            }
            InputAction::CycleNext => ExternalMatchesAction::CycleNext,
            InputAction::CyclePrev => ExternalMatchesAction::CyclePrev,
            InputAction::Cancel => ExternalMatchesAction::RequestQuit,
            _ => ExternalMatchesAction::None,
        }
    }
}
