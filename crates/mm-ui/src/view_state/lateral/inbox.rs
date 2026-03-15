//! Inbox view state: data + interaction bundled.
//!
//! The inbox view shows aggregate bucket entries for inbox-scoped signals.
//! Structurally identical to health — a single StandardListState with busy-blocking.

use std::collections::BTreeSet;

use ratatui::style::Color;

use mm_meta::views::InboxOverviewData;

use crate::domain_types::InboxInsightAction;
use crate::input::InputAction;
use crate::route::InboxRoute;
use crate::standard_list::{ListEntry, ListInputResult, StandardListConfig, StandardListState};
use crate::view_state::ViewCore;
use crate::wizard::{WizardItem, WizardOffer};

// ============================================================================
// InboxBucketEntry — a single row in the inbox overview list
// ============================================================================

/// A single bucket entry in the inbox overview.
#[derive(Debug, Clone)]
pub struct InboxBucketEntry {
    pub label: String,
    pub count: usize,
    pub color: Color,
    pub action: InboxInsightAction,
}

impl WizardItem for InboxBucketEntry {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        None
    }
}

impl ListEntry for InboxBucketEntry {
    type Action = InboxInsightAction;

    fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<InboxInsightAction> {
        match self.action {
            InboxInsightAction::Informational => None,
            other => Some(other),
        }
    }
}

// ============================================================================
// InboxViewData — server-fetched data
// ============================================================================

/// Server-fetched data for the inbox view.
pub struct InboxViewData {
    /// Aggregate bucket entries (not individual files).
    pub entries: Vec<InboxBucketEntry>,
    /// True when the Witch has pending work (dims UI, blocks actions).
    pub busy: bool,
}

impl Default for InboxViewData {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            busy: false,
        }
    }
}

impl InboxViewData {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update entries from cached overview data.
    /// Returns true if entries changed (caller should clamp cursor).
    pub fn update(&mut self, data: Option<InboxOverviewData>) -> bool {
        let Some(data) = data else { return false };

        let mut entries = Vec::new();

        if data.corpus_match > 0 {
            entries.push(InboxBucketEntry {
                label: "Corpus matches".to_string(),
                count: data.corpus_match,
                color: Color::Magenta,
                action: InboxInsightAction::LaunchCorpusMatchResolution,
            });
        }

        if data.tag_canonicity > 0 {
            entries.push(InboxBucketEntry {
                label: "Tag canonicity".to_string(),
                count: data.tag_canonicity,
                color: Color::Cyan,
                action: InboxInsightAction::LaunchInboxTagCanonicity,
            });
        }

        if data.missing_tags > 0 {
            entries.push(InboxBucketEntry {
                label: "Missing tags".to_string(),
                count: data.missing_tags,
                color: Color::LightRed,
                action: InboxInsightAction::Informational,
            });
        }

        if data.compound_tags > 0 {
            entries.push(InboxBucketEntry {
                label: "Compound tags".to_string(),
                count: data.compound_tags,
                color: Color::LightBlue,
                action: InboxInsightAction::LaunchInboxCompoundSplit,
            });
        }

        if data.organizable > 0 {
            entries.push(InboxBucketEntry {
                label: "Organize into corpus".to_string(),
                count: data.organizable,
                color: Color::Green,
                action: InboxInsightAction::LaunchOrganize,
            });
        }

        if data.unindexed > 0 {
            entries.push(InboxBucketEntry {
                label: "Unindexed".to_string(),
                count: data.unindexed,
                color: Color::Yellow,
                action: InboxInsightAction::LaunchIntake,
            });
        }

        if data.file_in_inbox > 0 {
            entries.push(InboxBucketEntry {
                label: "Files in inbox".to_string(),
                count: data.file_in_inbox,
                color: Color::DarkGray,
                action: InboxInsightAction::Informational,
            });
        }

        self.entries = entries;
        true
    }

    /// Get the currently selected entry at a cursor position.
    pub fn selected_entry(&self, cursor: usize) -> Option<&InboxBucketEntry> {
        self.entries.get(cursor)
    }
}

// ============================================================================
// InboxInteraction — UI navigation state
// ============================================================================

/// Interaction state for the inbox view.
pub struct InboxInteraction {
    pub list: StandardListState,
}

impl ViewCore for InboxInteraction {
    type Route = InboxRoute;
    type Action = InboxInsightAction;
    type Data = ();

    fn from_route(route: &InboxRoute) -> Self {
        let mut list = StandardListState::new(StandardListConfig::default());
        if let Some(cursor) = route.cursor {
            list.cursor = cursor;
        }
        Self { list }
    }

    fn to_route(&self) -> InboxRoute {
        InboxRoute {
            cursor: if self.list.cursor > 0 {
                Some(self.list.cursor)
            } else {
                None
            },
        }
    }

    fn handle_input(&mut self, _action: &InputAction, _data: &()) -> Option<InboxInsightAction> {
        None
    }
}

impl InboxInteraction {
    pub fn new() -> Self {
        Self {
            list: StandardListState::new(StandardListConfig::default()),
        }
    }

    /// Clamp cursor position to valid range after data refresh.
    pub fn clamp_to_data(&mut self, items: &[InboxBucketEntry]) {
        self.list.clamp_cursor(items);
    }
}

// ============================================================================
// InboxViewState — bundled data + interaction
// ============================================================================

/// Complete view state for the inbox view.
///
/// Bundles server-fetched data with interaction state so that ActiveView
/// carries a single struct instead of loose `{ data, interaction }` fields.
pub struct InboxViewState {
    pub data: InboxViewData,
    pub interaction: InboxInteraction,
}

impl InboxViewState {
    /// Create a new inbox view state from fetched data.
    pub fn new(data: InboxViewData) -> Self {
        let mut interaction = InboxInteraction::new();
        interaction.clamp_to_data(&data.entries);
        Self { data, interaction }
    }

    /// Create an empty inbox view state (used when no data is available).
    pub fn empty() -> Self {
        Self {
            data: InboxViewData::new(),
            interaction: InboxInteraction::new(),
        }
    }

    /// Handle a semantic input action, returning a domain action if one was produced.
    pub fn handle_input(&mut self, action: &InputAction) -> Option<InboxInsightAction> {
        let result = self.interaction.list.handle_input(action, &self.data.entries);

        match result {
            ListInputResult::Confirm(insight_action) => {
                if self.data.busy {
                    None
                } else {
                    Some(insight_action)
                }
            }
            ListInputResult::Consumed
            | ListInputResult::CursorMoved
            | ListInputResult::Toggled
            | ListInputResult::Unhandled => None,
        }
    }

    /// Route serialization.
    pub fn to_route(&self) -> InboxRoute {
        self.interaction.to_route()
    }

    /// Restore cursor/scroll position from a route.
    pub fn apply_route(&mut self, route: &InboxRoute) {
        if let Some(cursor) = route.cursor {
            self.interaction.list.cursor = cursor;
        }
        self.interaction.clamp_to_data(&self.data.entries);
    }
}
