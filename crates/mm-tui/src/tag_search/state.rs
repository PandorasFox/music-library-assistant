//! Tag Search State
//!
//! Wraps `mm_ui::search_widget::SearchWidget` for TUI-specific state
//! (result data, modals, bulk edit).

use std::collections::BTreeSet;

use mm_meta::domain_query_types::SearchResult;
use crate::widgets::standard_list::ListEntry;
use crate::widgets::wizard::{WizardItem, WizardOffer};

use super::types::TagSearchModal;

/// A search result with WizardItem and ListEntry impls for the TUI list.
#[derive(Debug, Clone)]
pub struct SearchResultEntry {
    pub result: SearchResult,
}

impl WizardItem for SearchResultEntry {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        use ratatui::style::{Color, Style};
        use crate::widgets::rich_text::{RichBlock, RichSpan};

        let tag_block = |label: &'static str, value: &Option<String>| -> RichBlock {
            let v = value.as_deref().unwrap_or("-");
            RichBlock::Paragraph(vec![
                RichSpan::new(format!("{}: ", label), Style::default().fg(Color::DarkGray)),
                RichSpan::new(v.to_string(), Style::default().fg(Color::White)),
            ])
        };

        let content = vec![
            tag_block("Title", &self.result.title),
            tag_block("Artist", &self.result.artist),
            tag_block("Album", &self.result.album),
            RichBlock::Blank,
            RichBlock::Paragraph(vec![
                RichSpan::new("Path: ", Style::default().fg(Color::DarkGray)),
                RichSpan::new(self.result.path.clone(), Style::default().fg(Color::White)),
            ]),
        ];

        Some(WizardOffer::Pane {
            title: "Track Info".to_string(),
            content,
        })
    }
}

impl ListEntry for SearchResultEntry {
    type Action = i64; // inode

    fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<i64> {
        Some(self.result.inode)
    }
}

/// State for the tag search view.
///
/// Wraps the shared SearchWidget (condition management, focus navigation)
/// and adds TUI-specific data (result entries, modals, bulk edit).
#[derive(Debug)]
pub struct TagSearchState {
    /// Shared widget state (conditions, focus, mode).
    pub widget: mm_ui::search_widget::SearchWidget,

    /// Search results from server (protocol query response).
    pub results: Vec<SearchResultEntry>,

    /// Active modal dialog (if any).
    pub modal: Option<TagSearchModal>,

    /// Pending bulk edit inodes (set when showing "gathering" modal).
    pub pending_bulk_edit: Option<Vec<i64>>,
}

impl Default for TagSearchState {
    fn default() -> Self {
        Self::new()
    }
}

impl TagSearchState {
    /// Create a new tag search state.
    pub fn new() -> Self {
        Self {
            widget: mm_ui::search_widget::SearchWidget::new(),
            results: Vec::new(),
            modal: None,
            pending_bulk_edit: None,
        }
    }

    /// Handle mouse click for cursor selection (results mode only).
    pub fn handle_click(&mut self, x: u16, y: u16) {
        if self.widget.mode != mm_ui::domain_types::TagSearchMode::Results {
            return;
        }
        self.widget.results_list.handle_click(x, y, &self.results);
    }

    /// Set search results from server response.
    pub fn set_search_results(&mut self, results: Vec<SearchResult>) {
        if results.is_empty() {
            self.modal = Some(TagSearchModal::NoResults);
            return;
        }
        self.results = results
            .into_iter()
            .map(|result| SearchResultEntry { result })
            .collect();
        let count = self.results.len();
        self.widget.set_results(count);
    }

    /// Check if there's a pending bulk edit and take it.
    pub fn take_pending_bulk_edit(&mut self) -> Option<Vec<i64>> {
        if self.pending_bulk_edit.is_some() {
            self.modal = None;
            self.pending_bulk_edit.take()
        } else {
            None
        }
    }

    /// Get all result inodes (for bulk edit).
    pub fn all_result_inodes(&self) -> Vec<i64> {
        self.results.iter().map(|r| r.result.inode).collect()
    }
}
