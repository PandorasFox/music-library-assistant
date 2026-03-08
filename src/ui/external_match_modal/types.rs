//! State and input handling for the external match review modal.
//!
//! Read-only browser: track → MB recording URL + confidence.
//! Cached MB recording data shown inline when available.

use std::collections::HashMap;

use ratatui::layout::Rect;

use crate::external::musicbrainz::{MbArtist, MbRecording, MbRelease};
use crate::ui::input::InputAction;

use crate::meta::views::ExternalMatchReviewEntry;
use crate::ui::widgets::{rect_contains, ListClickTargets};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalMatchReviewAction {
    None,
    /// Esc → close modal, return to lateral view.
    Cancel,
    /// Open MusicBrainz recording URL in browser.
    OpenRecordingUrl(String),
    /// Show structured MB recording detail overlay ('v' key).
    ViewRecordingDetail,
    /// Close the recording detail overlay (Esc while viewing).
    CloseRecordingDetail,
}

/// Structured MB recording detail for the 'v' overlay.
pub struct RecordingDetailState {
    pub recording: MbRecording,
    pub artists: Vec<(String, Option<MbArtist>)>,
    pub releases: Vec<(String, Option<MbRelease>)>,
    pub scroll: usize,
}

/// Pre-loaded MB recording summary for inline display.
pub struct RecordingSummary {
    pub title: String,
    pub artist_credit: String,
    pub length_ms: Option<u64>,
    pub release_count: usize,
}

pub struct ExternalMatchReviewState {
    pub entries: Vec<ExternalMatchReviewEntry>,
    pub cursor: usize,
    pub scroll: usize,
    pub click_targets: ListClickTargets,
    /// Click rect for the MusicBrainz recording link (set during render).
    pub recording_link_rect: Option<Rect>,
    /// Pre-loaded MB recording summaries keyed by recording_id.
    pub recording_summaries: HashMap<String, RecordingSummary>,
    /// Scroll offset for the detail pane.
    pub detail_scroll: usize,
    /// Recording detail overlay (shown with 'v' key).
    pub viewing_detail: Option<RecordingDetailState>,
}

impl ExternalMatchReviewState {
    pub fn new(entries: Vec<ExternalMatchReviewEntry>) -> Self {
        Self {
            entries,
            cursor: 0,
            scroll: 0,
            click_targets: ListClickTargets::new(),
            recording_link_rect: None,
            recording_summaries: HashMap::new(),
            detail_scroll: 0,
            viewing_detail: None,
        }
    }

    pub fn selected_path(&self) -> Option<&str> {
        self.entries.get(self.cursor).map(|e| e.path.as_str())
    }

    pub fn current_entry(&self) -> Option<&ExternalMatchReviewEntry> {
        self.entries.get(self.cursor)
    }

    /// MusicBrainz recording URL for the current entry.
    pub fn current_recording_url(&self) -> Option<String> {
        self.entries
            .get(self.cursor)
            .map(|e| format!("https://musicbrainz.org/recording/{}", e.recording_id))
    }

    /// Handle a mouse click at (x, y).
    pub fn handle_click(&mut self, x: u16, y: u16) -> Option<ExternalMatchReviewAction> {
        // Check MusicBrainz recording link
        if let Some(rect) = self.recording_link_rect {
            if rect_contains(rect, x, y) {
                if let Some(url) = self.current_recording_url() {
                    return Some(ExternalMatchReviewAction::OpenRecordingUrl(url));
                }
            }
        }
        // Check list items (cursor change, no action)
        if let Some(id) = self.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.entries.len() {
                    self.cursor = idx;
                    self.detail_scroll = 0;
                }
            }
        }
        None
    }

    pub fn handle_input(&mut self, action: &InputAction) -> ExternalMatchReviewAction {
        // When viewing recording detail overlay, only Up/Down scroll and Esc closes
        if let Some(ref mut detail) = self.viewing_detail {
            match action {
                InputAction::NavUp => {
                    detail.scroll = detail.scroll.saturating_sub(1);
                    return ExternalMatchReviewAction::None;
                }
                InputAction::NavDown => {
                    detail.scroll += 1;
                    return ExternalMatchReviewAction::None;
                }
                InputAction::Cancel => {
                    return ExternalMatchReviewAction::CloseRecordingDetail;
                }
                _ => return ExternalMatchReviewAction::None,
            }
        }

        // 'v' key: view full recording detail overlay
        if matches!(action, InputAction::Char('v')) {
            return ExternalMatchReviewAction::ViewRecordingDetail;
        }

        match action {
            InputAction::NavUp => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.detail_scroll = 0;
                }
                ExternalMatchReviewAction::None
            }
            InputAction::NavDown => {
                if self.cursor + 1 < self.entries.len() {
                    self.cursor += 1;
                    self.detail_scroll = 0;
                }
                ExternalMatchReviewAction::None
            }
            InputAction::Home => {
                self.cursor = 0;
                self.detail_scroll = 0;
                ExternalMatchReviewAction::None
            }
            InputAction::End => {
                if !self.entries.is_empty() {
                    self.cursor = self.entries.len() - 1;
                    self.detail_scroll = 0;
                }
                ExternalMatchReviewAction::None
            }
            // PageUp/PageDown for detail pane scrolling
            InputAction::PageUp => {
                self.detail_scroll = self.detail_scroll.saturating_sub(10);
                ExternalMatchReviewAction::None
            }
            InputAction::PageDown => {
                self.detail_scroll += 10;
                ExternalMatchReviewAction::None
            }
            InputAction::Cancel => ExternalMatchReviewAction::Cancel,
            _ => ExternalMatchReviewAction::None,
        }
    }
}
