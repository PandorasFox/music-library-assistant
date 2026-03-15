//! AcoustID Browse View — browse AcoustID matches by confidence tier.
//!
//! Read-only browser: shows matched files with confidence % and recording info.
//! Uses StandardList with wizard system for recording detail popups.

pub mod render;

use std::collections::BTreeSet;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use mm_meta::views::external_matches::AcoustidMatchEntry;
use crate::input::InputAction;
use crate::widgets::standard_list::{
    ListEntry, ListInputResult, StandardListConfig, StandardListState,
};
use crate::widgets::wizard::{WizardItem, WizardOffer};

// ============================================================================
// Actions
// ============================================================================

pub(crate) enum AcoustidBrowseAction {
    None,
    Cancel,
    OpenRecordingUrl(String),
}

// ============================================================================
// List item wrapper
// ============================================================================

pub(crate) struct AcoustidBrowseItem {
    pub entry: AcoustidMatchEntry,
    pub popup_lines: Vec<Line<'static>>,
}

impl WizardItem for AcoustidBrowseItem {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        if self.popup_lines.is_empty() {
            None
        } else {
            Some(WizardOffer::Popup(self.popup_lines.clone()))
        }
    }
}

impl ListEntry for AcoustidBrowseItem {
    type Action = String; // MB recording URL

    fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<String> {
        Some(format!(
            "https://musicbrainz.org/recording/{}",
            self.entry.recording_id
        ))
    }
}

// ============================================================================
// State
// ============================================================================

pub(crate) struct AcoustidBrowseState {
    pub items: Vec<AcoustidBrowseItem>,
    pub list: StandardListState,
}

impl AcoustidBrowseState {
    pub fn new(entries: Vec<AcoustidMatchEntry>) -> Self {
        let items = build_items(entries);
        Self {
            items,
            list: StandardListState::new(StandardListConfig::default()),
        }
    }

    pub fn selected_path(&self) -> Option<&str> {
        self.items
            .get(self.list.cursor)
            .map(|i| i.entry.display_name.as_str())
    }

    pub fn handle_input(&mut self, action: &InputAction) -> AcoustidBrowseAction {
        match self.list.handle_input(action, &self.items) {
            ListInputResult::Confirm(url) => AcoustidBrowseAction::OpenRecordingUrl(url),
            ListInputResult::Consumed
            | ListInputResult::CursorMoved
            | ListInputResult::Toggled => AcoustidBrowseAction::None,
            ListInputResult::Unhandled => match action {
                InputAction::Cancel => AcoustidBrowseAction::Cancel,
                _ => AcoustidBrowseAction::None,
            },
        }
    }

    pub fn handle_click(&mut self, x: u16, y: u16) {
        self.list.handle_click(x, y, &self.items);
    }
}

// ============================================================================
// Item construction
// ============================================================================

fn build_items(entries: Vec<AcoustidMatchEntry>) -> Vec<AcoustidBrowseItem> {
    entries
        .into_iter()
        .map(|entry| {
            let popup_lines = build_popup_lines(&entry);
            AcoustidBrowseItem { entry, popup_lines }
        })
        .collect()
}

fn build_popup_lines(entry: &AcoustidMatchEntry) -> Vec<Line<'static>> {
    let label = Style::default().fg(Color::DarkGray);
    let value = Style::default().fg(Color::White);

    let mut lines = Vec::new();

    // Confidence
    lines.push(Line::from(vec![
        Span::styled("Confidence: ", label),
        Span::styled(
            format!("{:.1}%", entry.confidence * 100.0),
            Style::default().fg(Color::Yellow),
        ),
    ]));

    // MB URL
    let mb_url = format!("https://musicbrainz.org/recording/{}", entry.recording_id);
    lines.push(Line::from(Span::styled(
        mb_url,
        Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::UNDERLINED),
    )));
    lines.push(Line::raw(""));

    // Recording info if available
    if let Some(ref title) = entry.recording_title {
        lines.push(Line::from(vec![
            Span::styled("Title:  ", label),
            Span::styled(title.clone(), value),
        ]));
    }
    if let Some(ref artist) = entry.recording_artist {
        lines.push(Line::from(vec![
            Span::styled("Artist: ", label),
            Span::styled(artist.clone(), value),
        ]));
    }
    if let Some(length_ms) = entry.recording_length_ms {
        lines.push(Line::from(vec![
            Span::styled("Length: ", label),
            Span::styled(crate::helpers::format_duration_ms(length_ms), value),
        ]));
    }

    if entry.recording_title.is_none() {
        lines.push(Line::from(Span::styled(
            "MB recording data not cached.",
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines
}
