//! Inbox View Module
//!
//! Aggregate signal overview for inbox files, similar to the Insights view
//! but scoped to inbox-specific signals. Shows bucket entries with counts
//! rather than individual files.
//!
//! Part of the lateral view ring - can cycle to adjacent views with Tab/Shift-Tab.
//!
//! Bucket entries (shown when count > 0):
//! - Corpus matches (magenta) — inbox files matching corpus fingerprints
//! - Unindexed (yellow) — inbox files not yet indexed
//! - Files in inbox (gray) — total file count (informational)
//!
//! Actions:
//! - Enter on "Unindexed": launch intake confirmation for inbox files
//! - Enter on other entries: informational (future actions)

mod render;

use std::collections::BTreeSet;

use crate::ui::input::InputAction;
use crate::ui::widgets::standard_list::{ListEntry, ListInputResult, StandardListConfig, StandardListState};
use crate::ui::widgets::wizard::{WizardItem, WizardOffer};
use ratatui::style::Color;

use crate::meta::views::InboxOverviewData;

pub use render::render_inbox_view;

/// Action returned from input handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboxAction {
    /// No action needed
    None,
    /// Request to quit the application
    RequestQuit,
    /// Cycle to next view in ring
    CycleNext,
    /// Cycle to previous view in ring
    CyclePrev,
    /// Launch intake confirmation for inbox unindexed files
    LaunchIntake,
    /// Launch inbox corpus match resolution modal
    LaunchCorpusMatchResolution,
    /// Launch inbox tag canonicity view
    LaunchInboxTagCanonicity,
    /// Launch inbox organize workflow
    LaunchOrganize,
    /// Launch inbox compound tag split resolution
    LaunchInboxCompoundSplit,
}

/// What action an inbox bucket entry triggers on Enter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxInsightAction {
    /// Launch inbox intake confirmation
    LaunchIntake,
    /// Launch inbox corpus match resolution modal
    LaunchCorpusMatchResolution,
    /// Launch inbox tag canonicity view
    LaunchInboxTagCanonicity,
    /// Launch inbox organize workflow
    LaunchOrganize,
    /// Launch inbox compound tag split resolution
    LaunchInboxCompoundSplit,
    /// Informational only, no action
    Informational,
}

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

/// State for the inbox view.
#[derive(Debug)]
pub struct InboxViewState {
    /// Aggregate bucket entries (not individual files)
    pub entries: Vec<InboxBucketEntry>,
    /// StandardList state machine
    pub list: StandardListState,
    /// True when the Witch has pending work (dims UI, blocks actions).
    pub busy: bool,
}

impl InboxViewState {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            list: StandardListState::new(StandardListConfig::default()),
            busy: false,
        }
    }

    /// Handle mouse click for cursor selection.
    pub fn handle_click(&mut self, x: u16, y: u16) {
        self.list.handle_click(x, y, &self.entries);
    }

    /// Update entries from cached overview data.
    pub fn update(&mut self, data: Option<InboxOverviewData>) {
        let Some(data) = data else { return };

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
        self.list.clamp_cursor(&self.entries);
    }

    /// Get the currently selected entry, if any.
    pub fn selected_entry(&self) -> Option<&InboxBucketEntry> {
        self.entries.get(self.list.cursor)
    }

    /// Handle a semantic input action and return the resulting action.
    pub fn handle_input(&mut self, action: &InputAction) -> InboxAction {
        let result = self.list.handle_input(action, &self.entries);

        match result {
            ListInputResult::Consumed | ListInputResult::CursorMoved | ListInputResult::Toggled => {
                InboxAction::None
            }
            ListInputResult::Confirm(insight_action) => {
                if self.busy {
                    return InboxAction::None;
                }
                match insight_action {
                    InboxInsightAction::LaunchIntake => InboxAction::LaunchIntake,
                    InboxInsightAction::LaunchCorpusMatchResolution => {
                        InboxAction::LaunchCorpusMatchResolution
                    }
                    InboxInsightAction::LaunchInboxTagCanonicity => {
                        InboxAction::LaunchInboxTagCanonicity
                    }
                    InboxInsightAction::LaunchOrganize => InboxAction::LaunchOrganize,
                    InboxInsightAction::LaunchInboxCompoundSplit => {
                        InboxAction::LaunchInboxCompoundSplit
                    }
                    InboxInsightAction::Informational => InboxAction::None,
                }
            }
            ListInputResult::Unhandled => match action {
                InputAction::Cancel => InboxAction::RequestQuit,
                InputAction::CycleNext => InboxAction::CycleNext,
                InputAction::CyclePrev => InboxAction::CyclePrev,
                _ => InboxAction::None,
            },
        }
    }
}
