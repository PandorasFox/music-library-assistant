//! Inbox View Module
//!
//! A full-screen view displaying inbox files pending triage.
//! Part of the lateral view ring - can cycle to adjacent views with Tab/Shift-Tab.
//!
//! Inbox files are grouped by signal type:
//! - Unindexed: files on disk not yet in audio_info
//! - Healthy: files indexed and ready for operations
//!
//! Navigation:
//! - Up/Down: Navigate within file list
//! - Tab/Shift-Tab: Cycle to adjacent view
//! - Esc: Return to main menu

mod render;

use crossterm::event::{KeyCode, KeyEvent};

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
}

/// An entry in the inbox file list.
#[derive(Debug, Clone)]
pub struct InboxEntry {
    pub inode: i64,
    pub path: String,
    pub status: InboxEntryStatus,
}

/// Status of an inbox entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxEntryStatus {
    /// File not yet indexed
    Unindexed,
    /// File indexed and ready
    Healthy,
}

/// State for the inbox view.
#[derive(Debug)]
pub struct InboxViewState {
    /// Combined list of inbox entries
    pub entries: Vec<InboxEntry>,
    /// Currently selected index
    pub selected: usize,
    /// Vertical scroll offset
    pub scroll: usize,
}

impl InboxViewState {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            selected: 0,
            scroll: 0,
        }
    }

    /// Refresh entries from the database.
    pub fn refresh(&mut self, db: &crate::corpus::db::ReadOnlyDb<'_>) {
        let mut entries = Vec::new();

        // Unindexed files first
        if let Ok(unindexed) = db.get_inbox_unindexed_files() {
            for (inode, path) in unindexed {
                entries.push(InboxEntry {
                    inode,
                    path,
                    status: InboxEntryStatus::Unindexed,
                });
            }
        }

        // Healthy files
        if let Ok(healthy) = db.get_inbox_healthy_files() {
            for (inode, path) in healthy {
                entries.push(InboxEntry {
                    inode,
                    path,
                    status: InboxEntryStatus::Healthy,
                });
            }
        }

        self.entries = entries;
        // Clamp selection
        if self.selected >= self.entries.len() && !self.entries.is_empty() {
            self.selected = self.entries.len() - 1;
        }
    }

    /// Handle a key event and return the resulting action.
    pub fn handle_key(&mut self, key: KeyEvent) -> InboxAction {
        match key.code {
            KeyCode::Esc => InboxAction::RequestQuit,
            KeyCode::Tab => InboxAction::CycleNext,
            KeyCode::BackTab => InboxAction::CyclePrev,

            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected > 0 {
                    self.selected -= 1;
                    self.ensure_visible();
                }
                InboxAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.entries.is_empty() && self.selected < self.entries.len() - 1 {
                    self.selected += 1;
                    self.ensure_visible();
                }
                InboxAction::None
            }
            KeyCode::Home => {
                self.selected = 0;
                self.scroll = 0;
                InboxAction::None
            }
            KeyCode::End => {
                if !self.entries.is_empty() {
                    self.selected = self.entries.len() - 1;
                    self.ensure_visible();
                }
                InboxAction::None
            }
            _ => InboxAction::None,
        }
    }

    fn ensure_visible(&mut self) {
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        // Scroll down is handled during render based on area height
    }
}
