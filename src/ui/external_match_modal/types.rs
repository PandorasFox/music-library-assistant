//! State and input handling for the external match review modal.
//!
//! Read-only browser: track → MB recording URL + confidence.
//! Cached MB recording data shown inline when available.
//! Uses StandardList with wizard system for recording detail (Z key).

use std::collections::BTreeSet;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::external::musicbrainz::{MbArtist, MbRecording, MbRelease};
use crate::meta::views::ExternalMatchReviewEntry;
use crate::ui::input::InputAction;
use crate::ui::widgets::standard_list::{
    ListEntry, ListInputResult, StandardListConfig, StandardListState,
};
use crate::ui::widgets::wizard::{WizardItem, WizardOffer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalMatchReviewAction {
    None,
    /// Esc → close modal, return to lateral view.
    Cancel,
    /// Open MusicBrainz recording URL in browser.
    OpenRecordingUrl(String),
}

/// Pre-loaded MB recording summary for inline display.
pub struct RecordingSummary {
    pub title: String,
    pub artist_credit: String,
    pub length_ms: Option<u64>,
    pub release_count: usize,
}

/// Full recording detail data for building wizard pane lines.
pub struct RecordingDetail {
    pub recording: MbRecording,
    pub artists: Vec<(String, Option<MbArtist>)>,
    pub releases: Vec<(String, Option<MbRelease>)>,
}

/// A single item in the external match review list.
/// Wraps an entry with baked wizard lines.
pub struct MatchReviewItem {
    pub entry: ExternalMatchReviewEntry,
    /// Popup lines (recording summary).
    pub popup_lines: Vec<Line<'static>>,
    /// Pane lines (full recording detail). Empty if no detail available.
    pub pane_lines: Vec<Line<'static>>,
}

impl WizardItem for MatchReviewItem {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        let has_popup = !self.popup_lines.is_empty();
        let has_pane = !self.pane_lines.is_empty();
        match (has_popup, has_pane) {
            (true, true) => Some(WizardOffer::Both {
                popup: self.popup_lines.clone(),
                pane_title: "Recording Detail".to_string(),
                pane_lines: self.pane_lines.clone(),
            }),
            (true, false) => Some(WizardOffer::Popup(self.popup_lines.clone())),
            (false, true) => Some(WizardOffer::Pane {
                title: "Recording Detail".to_string(),
                lines: self.pane_lines.clone(),
            }),
            (false, false) => None,
        }
    }
}

impl ListEntry for MatchReviewItem {
    type Action = String; // MB recording URL

    fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<String> {
        // Enter opens the MB recording URL
        Some(format!(
            "https://musicbrainz.org/recording/{}",
            self.entry.recording_id
        ))
    }
}

pub struct ExternalMatchReviewState {
    pub items: Vec<MatchReviewItem>,
    pub list: StandardListState,
}

impl ExternalMatchReviewState {
    pub fn new(
        entries: Vec<ExternalMatchReviewEntry>,
        summaries: Vec<(String, RecordingSummary)>,
        details: Vec<(String, RecordingDetail)>,
    ) -> Self {
        let items = build_items(entries, &summaries, &details);
        Self {
            items,
            list: StandardListState::new(StandardListConfig::default()),
        }
    }

    pub fn selected_path(&self) -> Option<&str> {
        self.items.get(self.list.cursor).map(|i| i.entry.path.as_str())
    }

    pub fn handle_click(&mut self, x: u16, y: u16) -> Option<ExternalMatchReviewAction> {
        self.list.handle_click(x, y, &self.items);
        None
    }

    pub fn handle_input(&mut self, action: &InputAction) -> ExternalMatchReviewAction {
        match self.list.handle_input(action, &self.items) {
            ListInputResult::Confirm(url) => ExternalMatchReviewAction::OpenRecordingUrl(url),
            ListInputResult::Consumed | ListInputResult::CursorMoved | ListInputResult::Toggled => {
                ExternalMatchReviewAction::None
            }
            ListInputResult::Unhandled => match action {
                InputAction::Cancel => ExternalMatchReviewAction::Cancel,
                _ => ExternalMatchReviewAction::None,
            },
        }
    }
}

// ============================================================================
// Item construction with baked wizard lines
// ============================================================================

fn build_items(
    entries: Vec<ExternalMatchReviewEntry>,
    summaries: &[(String, RecordingSummary)],
    details: &[(String, RecordingDetail)],
) -> Vec<MatchReviewItem> {
    entries
        .into_iter()
        .map(|entry| {
            let popup_lines = build_popup_lines(&entry, summaries);
            let pane_lines = build_pane_lines(&entry, details);
            MatchReviewItem {
                entry,
                popup_lines,
                pane_lines,
            }
        })
        .collect()
}

fn build_popup_lines(
    entry: &ExternalMatchReviewEntry,
    summaries: &[(String, RecordingSummary)],
) -> Vec<Line<'static>> {
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

    // Summary if available
    if let Some((_, summary)) = summaries.iter().find(|(id, _)| *id == entry.recording_id) {
        lines.push(Line::from(vec![
            Span::styled("Title:  ", label),
            Span::styled(summary.title.clone(), value),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Artist: ", label),
            Span::styled(summary.artist_credit.clone(), value),
        ]));
        if let Some(length_ms) = summary.length_ms {
            let mins = length_ms / 60000;
            let secs = (length_ms % 60000) / 1000;
            lines.push(Line::from(vec![
                Span::styled("Length: ", label),
                Span::styled(format!("{}:{:02}", mins, secs), value),
            ]));
        }
        if summary.release_count > 0 {
            lines.push(Line::from(vec![
                Span::styled("Releases: ", label),
                Span::styled(format!("{}", summary.release_count), value),
            ]));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "MB recording data not cached.",
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines
}

fn build_pane_lines(
    entry: &ExternalMatchReviewEntry,
    details: &[(String, RecordingDetail)],
) -> Vec<Line<'static>> {
    let Some((_, detail)) = details.iter().find(|(id, _)| *id == entry.recording_id) else {
        return vec![];
    };

    let label = Style::default().fg(Color::DarkGray);
    let value = Style::default().fg(Color::White);
    let heading = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Color::DarkGray);

    let rec = &detail.recording;
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Recording title + length
    lines.push(Line::from(vec![
        Span::styled("Recording: ", label),
        Span::styled(format!("\"{}\"", rec.title), value),
    ]));
    if let Some(length_ms) = rec.length {
        let mins = length_ms / 60000;
        let secs = (length_ms % 60000) / 1000;
        lines.push(Line::from(vec![
            Span::styled("Length:    ", label),
            Span::styled(format!("{}:{:02}", mins, secs), value),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled("MBID:      ", label),
        Span::styled(rec.id.clone(), dim),
    ]));
    lines.push(Line::raw(""));

    // Artist credits
    if !rec.artist_credit.is_empty() {
        lines.push(Line::from(Span::styled(
            "Artist Credits:".to_string(),
            heading,
        )));
        for credit in &rec.artist_credit {
            let mut spans = vec![
                Span::styled("  ", label),
                Span::styled(credit.name.clone(), value),
            ];
            if credit.artist.sort_name != credit.artist.name {
                spans.push(Span::styled(
                    format!(" (sort: {})", credit.artist.sort_name),
                    dim,
                ));
            }
            if !credit.joinphrase.is_empty() {
                spans.push(Span::styled(
                    format!(" [join: \"{}\"]", credit.joinphrase.trim()),
                    dim,
                ));
            }
            lines.push(Line::from(spans));

            // Show aliases if we have cached artist data
            if let Some((_, Some(ref artist))) = detail
                .artists
                .iter()
                .find(|(id, _)| *id == credit.artist.id)
            {
                if artist.name != credit.name {
                    lines.push(Line::from(vec![
                        Span::styled("    canonical: ", dim),
                        Span::styled(artist.name.clone(), dim),
                        Span::styled(format!(" [{}]", artist.id), dim),
                    ]));
                }
                if artist.sort_name != artist.name {
                    lines.push(Line::from(vec![
                        Span::styled("    sort: ", dim),
                        Span::styled(artist.sort_name.clone(), dim),
                    ]));
                }
                if !artist.aliases.is_empty() {
                    let alias_strs: Vec<String> = artist
                        .aliases
                        .iter()
                        .take(5)
                        .map(|a| {
                            let mut s = a.name.clone();
                            if let Some(ref locale) = a.locale {
                                s = format!("{} ({})", s, locale);
                            }
                            if let Some(ref t) = a.type_ {
                                if a.primary.as_deref() == Some("primary") {
                                    s = format!("{} [{}*]", s, t);
                                }
                            }
                            s
                        })
                        .collect();
                    lines.push(Line::from(vec![
                        Span::styled("    aka: ", dim),
                        Span::styled(alias_strs.join(", "), dim),
                    ]));
                }
            }
        }
        lines.push(Line::raw(""));
    }

    // Relations
    let relevant_relations: Vec<_> = rec
        .relations
        .iter()
        .filter(|r| r.artist.is_some() && r.direction.as_deref() != Some("forward"))
        .collect();
    if !relevant_relations.is_empty() {
        lines.push(Line::from(Span::styled("Relations:".to_string(), heading)));
        for relation in &relevant_relations {
            if let Some(ref artist) = relation.artist {
                let mut role = relation.type_.clone();
                if !relation.attributes.is_empty() {
                    role = format!("{} ({})", role, relation.attributes.join(", "));
                }
                lines.push(Line::from(vec![
                    Span::styled("  ", label),
                    Span::styled(format!("{}: ", role), dim),
                    Span::styled(artist.name.clone(), value),
                ]));
            }
        }
        lines.push(Line::raw(""));
    }

    // Releases
    if !detail.releases.is_empty() {
        lines.push(Line::from(Span::styled("Releases:".to_string(), heading)));
        for (id, parsed) in &detail.releases {
            match parsed {
                Some(release) => {
                    let artist_str: String = release
                        .artist_credit
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    let mut spans = vec![
                        Span::styled("  ", label),
                        Span::styled(release.title.clone(), value),
                    ];
                    if !artist_str.is_empty() {
                        spans.push(Span::styled(format!(" by {}", artist_str), dim));
                    }
                    lines.push(Line::from(spans));
                    lines.push(Line::from(vec![
                        Span::styled("    ", label),
                        Span::styled(release.id.clone(), dim),
                    ]));
                }
                None => {
                    let fallback_title = rec
                        .releases
                        .iter()
                        .find(|r| r.id == *id)
                        .and_then(|r| r.title.as_deref());
                    let fallback_rg = rec
                        .releases
                        .iter()
                        .find(|r| r.id == *id)
                        .and_then(|r| r.release_group.as_ref());
                    let mut spans = vec![Span::styled("  ", label)];
                    if let Some(title) = fallback_title {
                        spans.push(Span::styled(title.to_string(), value));
                        spans.push(Span::styled(" (not cached)", dim));
                    } else {
                        spans.push(Span::styled(id.clone(), dim));
                        spans.push(Span::styled(" (not cached)", dim));
                    }
                    lines.push(Line::from(spans));
                    if let Some(rg) = fallback_rg {
                        lines.push(Line::from(vec![
                            Span::styled("    release-group: ", dim),
                            Span::styled(rg.id.clone(), dim),
                        ]));
                    }
                }
            }
        }
        lines.push(Line::raw(""));
    }

    lines
}
