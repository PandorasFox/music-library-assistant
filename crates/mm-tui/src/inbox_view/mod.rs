//! Inbox View Module
//!
//! Aggregate signal overview for inbox files, similar to the Insights view
//! but scoped to inbox-specific signals. Shows bucket entries with counts
//! rather than individual files.
//!
//! Part of the lateral view ring - can cycle to adjacent views with Tab/Shift-Tab.

mod render;

use std::collections::BTreeSet;

use ratatui::style::Color;

use mm_meta::views::InboxOverviewData;
use crate::widgets::standard_list::ListEntry;
use crate::widgets::wizard::{WizardItem, WizardOffer};

pub use render::render_inbox_view;

/// Re-export interaction type from mm-ui.
pub use mm_ui::view_state::lateral::inbox::InboxInteraction;
pub use mm_ui::domain_types::InboxInsightAction;

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

/// Server-fetched data for the inbox view.
///
/// Interaction state lives separately in [`InboxInteraction`].
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
