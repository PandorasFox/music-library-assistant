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

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Color;

use crate::corpus::db::types::InboxOverviewData;

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

/// State for the inbox view.
#[derive(Debug)]
pub struct InboxViewState {
    /// Aggregate bucket entries (not individual files)
    pub entries: Vec<InboxBucketEntry>,
    /// Currently selected index
    pub selected: usize,
}

impl InboxViewState {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            selected: 0,
        }
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
        // Clamp selection
        if !self.entries.is_empty() && self.selected >= self.entries.len() {
            self.selected = self.entries.len() - 1;
        }
    }

    /// Get the currently selected entry, if any.
    pub fn selected_entry(&self) -> Option<&InboxBucketEntry> {
        self.entries.get(self.selected)
    }

    /// Handle a key event and return the resulting action.
    pub fn handle_key(&mut self, key: KeyEvent) -> InboxAction {
        match key.code {
            KeyCode::Esc => InboxAction::RequestQuit,
            KeyCode::Tab => InboxAction::CycleNext,
            KeyCode::BackTab => InboxAction::CyclePrev,

            KeyCode::Enter => {
                if let Some(entry) = self.selected_entry() {
                    match entry.action {
                        InboxInsightAction::LaunchIntake => InboxAction::LaunchIntake,
                        InboxInsightAction::LaunchCorpusMatchResolution => InboxAction::LaunchCorpusMatchResolution,
                        InboxInsightAction::LaunchInboxTagCanonicity => InboxAction::LaunchInboxTagCanonicity,
                        InboxInsightAction::LaunchOrganize => InboxAction::LaunchOrganize,
                        InboxInsightAction::Informational => InboxAction::None,
                    }
                } else {
                    InboxAction::None
                }
            }

            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                InboxAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.entries.is_empty() && self.selected < self.entries.len() - 1 {
                    self.selected += 1;
                }
                InboxAction::None
            }
            KeyCode::Home => {
                self.selected = 0;
                InboxAction::None
            }
            KeyCode::End => {
                if !self.entries.is_empty() {
                    self.selected = self.entries.len() - 1;
                }
                InboxAction::None
            }
            _ => InboxAction::None,
        }
    }
}
